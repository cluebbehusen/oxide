"""Tests for the GAE arithmetic in ``ppo``, checked against advantages
computed by hand.

gamma=0.5, lam=0.5 (so gamma*lam=0.25) keeps every intermediate exactly
representable in binary floating point, so the literals below are exact,
not rounded."""

import numpy as np
import torch

from models import make_policy
from oxide_gym import ACTIONS, NET_FEATURES
from ppo import gae, ppo_update


class TestGae:
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
            actions = logits.argmax(dim=1)
            old_logp = torch.log_softmax(logits, dim=1).gather(1, actions[:, None])[
                :, 0
            ]
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
