"""Tests for the GAE arithmetic in ``ppo``, checked against advantages
computed by hand.

gamma=0.5, lam=0.5 (so gamma*lam=0.25) keeps every intermediate exactly
representable in binary floating point, so the literals below are exact,
not rounded."""

import numpy as np
import pytest
import torch

from models import (
    factorized_entropy,
    factorized_greedy,
    factorized_joint_log_prob,
    factorized_kl,
    factorized_production_entropy,
    factorized_sample,
    make_policy,
)
from oxide_gym import ACTION_HEADS, ACTIONS, NET_FEATURES
from ppo import TRAIN_GAMMA, gae, ppo_update


class TestGae:
    def test_the_training_discount_matches_the_long_game_contract(self) -> None:
        assert TRAIN_GAMMA == 0.9997

    def test_the_bootstrap_flows_through_a_done_free_window(self) -> None:
        # One lane, no episode boundary: every step bootstraps off the next
        # value, and the final step off last_val.
        #   t=2: d = 3 + 0.5*40 - 30 = -7 ; A2 = -7
        #   t=1: d = 2 + 0.5*30 - 20 = -3 ; A1 = -3 + 0.25*(-7)   = -4.75
        #   t=0: d = 1 + 0.5*20 - 10 =  1 ; A0 =  1 + 0.25*(-4.75) = -0.1875
        rew = np.array([[1.0], [2.0], [3.0]])
        val = np.array([[10.0], [20.0], [30.0]])
        done = np.array([[False], [False], [False]])
        last_val = np.array([40.0])

        adv, ret = gae(rew, done, val, last_val, gamma=0.5, lam=0.5)

        np.testing.assert_allclose(adv[:, 0], [-0.1875, -4.75, -7.0], atol=1e-9)
        # returns are advantage + value, by definition.
        np.testing.assert_allclose(ret[:, 0], [9.8125, 15.25, 23.0], atol=1e-9)

    def test_a_mid_sequence_done_cuts_the_bootstrap(self) -> None:
        # done[1]=True marks the transition out of state 1 as terminal, so
        # step 1 gets no bootstrap (its huge last_val=40 must not leak in)
        # and the lam recursion resets there, sealing step 2's future away
        # from steps 0 and 1.
        #   t=2: d = 3 + 0.5*40 - 30 = -7            ; A2 = -7
        #   t=1: d = 5 + 0(cut) - 20 = -15           ; A1 = -15 (recursion reset)
        #   t=0: d = 1 + 0.5*20 - 10 =  1            ; A0 = 1 + 0.25*(-15) = -2.75
        rew = np.array([[1.0], [5.0], [3.0]])
        val = np.array([[10.0], [20.0], [30.0]])
        done = np.array([[False], [True], [False]])
        last_val = np.array([40.0])

        adv, _ = gae(rew, done, val, last_val, gamma=0.5, lam=0.5)

        # The terminal step's advantage is exactly reward-minus-value: no
        # bootstrap term survives the cut.
        assert adv[1, 0] == 5.0 - 20.0
        # Had the bootstrap NOT been cut, step 1 would carry 0.5*40 and step
        # 2's -7 future, giving A1=+3.25 and a very different A0. It doesn't.
        np.testing.assert_allclose(adv[:, 0], [-2.75, -15.0, -7.0], atol=1e-9)

    def test_each_lane_is_an_independent_column(self) -> None:
        # Two lanes stepped together: lane 0 terminates at t=1, lane 1 never
        # does. The batch is vectorized, so the mask and the running sum are
        # per-column — a done in lane 0 must not disturb lane 1's estimates.
        # Lane 1 (done-free):
        #   t=2: d = 6 + 0.5*35 - 25 = -1.5  ; A2 = -1.5
        #   t=1: d = 4 + 0.5*25 - 15 =  1.5  ; A1 = 1.5 + 0.25*(-1.5) = 1.125
        #   t=0: d = 2 + 0.5*15 -  5 =  4.5  ; A0 = 4.5 + 0.25*1.125  = 4.78125
        rew = np.array([[1.0, 2.0], [5.0, 4.0], [3.0, 6.0]])
        val = np.array([[10.0, 5.0], [20.0, 15.0], [30.0, 25.0]])
        done = np.array([[False, False], [True, False], [False, False]])
        last_val = np.array([40.0, 35.0])

        adv, _ = gae(rew, done, val, last_val, gamma=0.5, lam=0.5)

        np.testing.assert_allclose(adv[:, 0], [-2.75, -15.0, -7.0], atol=1e-9)
        np.testing.assert_allclose(adv[:, 1], [4.78125, 1.125, -1.5], atol=1e-9)

    def test_lambda_one_carries_a_terminal_impulse_across_a_full_game(self) -> None:
        horizon = 2_500
        rew = np.zeros((horizon, 1), dtype=np.float32)
        rew[-1, 0] = 1.0
        done = np.zeros((horizon, 1), dtype=bool)
        done[-1, 0] = True
        val = np.zeros_like(rew)

        adv, returns = gae(
            rew,
            done,
            val,
            np.zeros(1, dtype=np.float32),
            gamma=TRAIN_GAMMA,
            lam=1.0,
        )

        assert adv[0, 0] == pytest.approx(TRAIN_GAMMA ** (horizon - 1), rel=2e-5)
        assert adv[horizon // 2, 0] > adv[0, 0]
        np.testing.assert_array_equal(returns, adv)


class TestFactorizedDistribution:
    def test_sampling_and_greedy_return_global_indices_per_head(self) -> None:
        logits = torch.full((5, ACTIONS), -10.0)
        logits[:, 8] = 3.0
        logits[:, 23] = 4.0
        logits[:, 20] = 5.0

        assert torch.equal(
            factorized_greedy(logits),
            torch.tensor([[8, 23, 42, 20]]).expand(5, -1),
        )
        torch.manual_seed(4)
        sampled = factorized_sample(logits)
        assert sampled.shape == (5, 4)
        for head_index, head in enumerate(ACTION_HEADS):
            assert set(sampled[:, head_index].tolist()).issubset(head)

    def test_joint_log_prob_sums_while_entropy_and_kl_average(self) -> None:
        logits = torch.zeros(2, ACTIONS)
        actions = torch.tensor([[0, 24, 42, 25], [8, 23, 40, 20]])

        logp = factorized_joint_log_prob(logits, actions)
        expected_logp = -sum(np.log(len(head)) for head in ACTION_HEADS)
        torch.testing.assert_close(
            logp,
            torch.full((2,), expected_logp, dtype=logp.dtype),
        )

        entropy = factorized_entropy(logits)
        expected_entropy = np.mean([np.log(len(head)) for head in ACTION_HEADS])
        torch.testing.assert_close(
            entropy,
            torch.full((2,), expected_entropy, dtype=entropy.dtype),
        )
        production_entropy = factorized_production_entropy(logits)
        torch.testing.assert_close(
            production_entropy,
            torch.full(
                (2,),
                np.log(len(ACTION_HEADS[0])),
                dtype=production_entropy.dtype,
            ),
        )

        shifted = logits.clone()
        shifted[:, [8, 23, 20]] = 2.0
        expected_kl = torch.stack(
            [
                torch.distributions.kl_divergence(
                    torch.distributions.Categorical(logits=logits[:, list(head)]),
                    torch.distributions.Categorical(logits=shifted[:, list(head)]),
                )
                for head in ACTION_HEADS
            ],
            dim=-1,
        ).mean(dim=-1)
        torch.testing.assert_close(factorized_kl(logits, shifted), expected_kl)
        torch.testing.assert_close(factorized_kl(logits, logits), torch.zeros(2))

    def test_wrong_global_head_indices_and_empty_heads_fail_loudly(self) -> None:
        logits = torch.zeros(1, ACTIONS)
        with pytest.raises(ValueError, match="head 1"):
            factorized_joint_log_prob(logits, torch.tensor([[0, 8, 42, 25]]))

        logits[:, list(ACTION_HEADS[2])] = float("-inf")
        with pytest.raises(ValueError, match="head 2 has no legal"):
            factorized_sample(logits)


class TestCriticWarmup:
    def test_value_only_learning_leaves_every_actor_coefficient_bit_identical(
        self,
    ) -> None:
        torch.manual_seed(7)
        policy = make_policy("mlp")
        optimizer = torch.optim.Adam(policy.parameters(), lr=1e-3)
        rng = np.random.default_rng(7)
        rows = 256
        obs = rng.normal(size=(rows, NET_FEATURES)).astype(np.float32)
        mask = np.ones((rows, ACTIONS), dtype=bool)
        with torch.no_grad():
            logits, values_before = policy(
                torch.as_tensor(obs),
                torch.as_tensor(mask),
            )
            actions = factorized_greedy(logits)
            old_logp = factorized_joint_log_prob(logits, actions)
        actor_before = {
            name: parameter.detach().clone()
            for name, parameter in policy.named_parameters()
            if not name.startswith("v.")
        }
        batch = (
            obs,
            mask,
            actions.numpy(),
            old_logp.numpy(),
            np.ones(rows, dtype=np.float32),
            np.full(rows, 5.0, dtype=np.float32),
        )

        ppo_update(
            policy,
            optimizer,
            batch,
            "cpu",
            epochs=2,
            minibatch=rows,
            value_only=True,
        )

        for name, before in actor_before.items():
            assert torch.equal(policy.state_dict()[name], before), name
        with torch.no_grad():
            logits_after, values_after = policy(
                torch.as_tensor(obs),
                torch.as_tensor(mask),
            )
        assert torch.equal(logits_after, logits)
        assert not torch.equal(values_after, values_before), "the critic must learn"

    def test_minibatch_order_uses_the_explicit_reproducible_rng(self) -> None:
        torch.manual_seed(17)
        first = make_policy("mlp")
        second = make_policy("mlp")
        second.load_state_dict(first.state_dict())
        first_optimizer = torch.optim.Adam(first.parameters(), lr=1e-3)
        second_optimizer = torch.optim.Adam(second.parameters(), lr=1e-3)
        data_rng = np.random.default_rng(9)
        rows = 64
        obs = data_rng.normal(size=(rows, NET_FEATURES)).astype(np.float32)
        mask = np.ones((rows, ACTIONS), dtype=bool)
        with torch.no_grad():
            logits, _ = first(torch.as_tensor(obs), torch.as_tensor(mask))
            actions = factorized_sample(logits)
            logp = factorized_joint_log_prob(logits, actions)
        batch = (
            obs,
            mask,
            actions.numpy(),
            logp.numpy(),
            data_rng.normal(size=rows).astype(np.float32),
            data_rng.normal(size=rows).astype(np.float32),
        )

        for policy, optimizer in [
            (first, first_optimizer),
            (second, second_optimizer),
        ]:
            ppo_update(
                policy,
                optimizer,
                batch,
                "cpu",
                epochs=2,
                minibatch=16,
                kl_stop=0.0,
                rng=np.random.default_rng(23),
            )

        for name, parameter in first.state_dict().items():
            assert torch.equal(parameter, second.state_dict()[name]), name


class TestProductionEntropy:
    @staticmethod
    def _zero_loss_batch(
        policy: torch.nn.Module,
        rows: int = 32,
    ) -> tuple[np.ndarray, ...]:
        rng = np.random.default_rng(29)
        obs = rng.normal(size=(rows, NET_FEATURES)).astype(np.float32)
        mask = np.ones((rows, ACTIONS), dtype=bool)
        with torch.no_grad():
            logits, values = policy(
                torch.as_tensor(obs),
                torch.as_tensor(mask),
            )
            actions = factorized_greedy(logits)
            logp = factorized_joint_log_prob(logits, actions)
        return (
            obs,
            mask,
            actions.numpy(),
            logp.numpy(),
            np.ones(rows, dtype=np.float32),
            values.numpy(),
        )

    def test_zero_is_bit_identical_to_the_default(self) -> None:
        torch.manual_seed(31)
        default_policy = make_policy("mlp")
        explicit_zero_policy = make_policy("mlp")
        explicit_zero_policy.load_state_dict(default_policy.state_dict())
        batch = self._zero_loss_batch(default_policy)

        ppo_update(
            default_policy,
            torch.optim.SGD(default_policy.parameters(), lr=1e-2),
            batch,
            "cpu",
            epochs=1,
            minibatch=len(batch[0]),
            kl_stop=0.0,
        )
        ppo_update(
            explicit_zero_policy,
            torch.optim.SGD(explicit_zero_policy.parameters(), lr=1e-2),
            batch,
            "cpu",
            epochs=1,
            minibatch=len(batch[0]),
            production_ent_coef=0.0,
            kl_stop=0.0,
        )

        for name, parameter in default_policy.state_dict().items():
            assert torch.equal(parameter, explicit_zero_policy.state_dict()[name]), name

    def test_additional_bonus_updates_only_production_policy_rows(self) -> None:
        torch.manual_seed(37)
        policy = make_policy("mlp")
        batch = self._zero_loss_batch(policy)
        output_before = policy.pi.weight.detach().clone()
        bias_before = policy.pi.bias.detach().clone()

        stats = ppo_update(
            policy,
            torch.optim.SGD(policy.parameters(), lr=1e-2),
            batch,
            "cpu",
            epochs=1,
            minibatch=len(batch[0]),
            ent_coef=0.0,
            production_ent_coef=0.05,
            kl_stop=0.0,
        )

        production_rows = list(ACTION_HEADS[0])
        other_rows = [action for head in ACTION_HEADS[1:] for action in head]
        assert not torch.equal(
            policy.pi.weight[production_rows],
            output_before[production_rows],
        )
        assert not torch.equal(
            policy.pi.bias[production_rows],
            bias_before[production_rows],
        )
        assert torch.equal(policy.pi.weight[other_rows], output_before[other_rows])
        assert torch.equal(policy.pi.bias[other_rows], bias_before[other_rows])
        assert stats["production_ent"] > 0.0


class TestGuards:
    def _batch(self, policy, rows: int = 128, seed: int = 5):
        rng = np.random.default_rng(seed)
        obs = rng.normal(size=(rows, NET_FEATURES)).astype(np.float32)
        mask = np.ones((rows, ACTIONS), dtype=bool)
        with torch.no_grad():
            logits, _ = policy(torch.as_tensor(obs), torch.as_tensor(mask))
            actions = factorized_sample(logits)
            logp = factorized_joint_log_prob(logits, actions)
        return (
            obs,
            mask,
            actions.numpy(),
            logp.numpy(),
            rng.normal(size=rows).astype(np.float32),
            rng.normal(size=rows).astype(np.float32),
        )

    def test_the_kl_stop_halts_a_runaway_update(self) -> None:
        # The guard the coverage audit measured at zero: with a
        # microscopic budget the first minibatch's divergence trips it,
        # and the surviving policy stays closer to its start than an
        # unguarded twin after the same epochs.
        torch.manual_seed(23)
        guarded = make_policy("mlp")
        unguarded = make_policy("mlp")
        unguarded.load_state_dict(guarded.state_dict())
        start = {k: v.detach().clone() for k, v in guarded.state_dict().items()}
        batch = self._batch(guarded)

        stats = ppo_update(
            guarded,
            torch.optim.Adam(guarded.parameters(), lr=5e-2),
            batch,
            "cpu",
            epochs=8,
            minibatch=32,
            kl_stop=1e-9,
        )
        ppo_update(
            unguarded,
            torch.optim.Adam(unguarded.parameters(), lr=5e-2),
            batch,
            "cpu",
            epochs=8,
            minibatch=32,
            kl_stop=0.0,
        )
        assert "kl" in stats, "the stop records the divergence it saw"
        drift = lambda policy: sum(  # noqa: E731
            (policy.state_dict()[k] - start[k]).abs().sum().item() for k in start
        )
        assert drift(guarded) < drift(unguarded), (
            "the guard must leave the policy nearer its start"
        )

    def test_the_anchor_tether_holds_the_policy_near_its_reference(self) -> None:
        # The dormant regularizer the audit surfaced: a frozen anchor
        # with a heavy coefficient must keep the updated policy closer
        # to the anchor (in its own KL) than the same update without it.
        torch.manual_seed(29)
        anchor = make_policy("mlp")
        tethered = make_policy("mlp")
        free = make_policy("mlp")
        tethered.load_state_dict(anchor.state_dict())
        free.load_state_dict(anchor.state_dict())
        batch = self._batch(anchor)
        obs_t = torch.as_tensor(batch[0])
        mask_t = torch.as_tensor(batch[1])

        for policy, coef in ((tethered, 50.0), (free, 0.0)):
            ppo_update(
                policy,
                torch.optim.Adam(policy.parameters(), lr=5e-2),
                batch,
                "cpu",
                epochs=6,
                minibatch=32,
                kl_stop=0.0,
                anchor=anchor if coef else None,
                anchor_coef=coef,
            )

        with torch.no_grad():
            a_logits, _ = anchor(obs_t, mask_t)
            t_logits, _ = tethered(obs_t, mask_t)
            f_logits, _ = free(obs_t, mask_t)
            tether_kl = factorized_kl(a_logits, t_logits).mean().item()
            free_kl = factorized_kl(a_logits, f_logits).mean().item()
        assert tether_kl < free_kl, (
            f"anchored KL {tether_kl:.4f} must undercut unanchored {free_kl:.4f}"
        )
