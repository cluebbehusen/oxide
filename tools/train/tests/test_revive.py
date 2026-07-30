"""Focused tests for selective production-row revival."""

from __future__ import annotations

import argparse
import copy
from typing import TYPE_CHECKING

import pytest
import torch

import revive
from models import make_policy
from oxide_gym import ACTIONS, NET_FEATURES

if TYPE_CHECKING:
    from collections.abc import Callable


def _dataset() -> revive.RevivalDataset:
    generator = torch.Generator().manual_seed(17)
    rows = 60
    obs = torch.randn(rows, NET_FEATURES, generator=generator)
    mask = torch.ones(rows, ACTIONS, dtype=torch.bool)
    target_pattern = torch.tensor([3, 8, 2, 4, 3, 8, 1, 2, 4, 2])
    target = target_pattern.repeat(rows // len(target_pattern))
    episode = torch.arange(rows).floor_divide(6)
    return revive.RevivalDataset(obs, mask, target, episode)


def _config() -> revive.RevivalConfig:
    return revive.RevivalConfig(
        actions=(3, 8),
        steps=4,
        learning_rate=1e-3,
        positive_batch=16,
        retention_batch=24,
        retention_coefficient=0.01,
        parameter_coefficient=1e-4,
        heldout_modulo=5,
        sample_seed=91,
    )


def test_action_parser_accepts_only_unique_production_rows() -> None:
    assert revive.parse_action_indices("3,8") == (3, 8)
    with pytest.raises(argparse.ArgumentTypeError, match="must not repeat"):
        revive.parse_action_indices("3,3")
    with pytest.raises(argparse.ArgumentTypeError, match="not in the production head"):
        revive.parse_action_indices("3,24")


def test_teacher_conditions_match_the_shipped_strategy_skills() -> None:
    assert revive.teacher_condition("industry", 0)[:2] == (620, 375)
    assert revive.teacher_condition("combined", 1000)[:2] == (1000, 550)
    assert revive.teacher_condition("fortify", 0)[:2] == (1000, 125)
    assert revive.teacher_condition("pressure", 1000)[:2] == (1000, 875)


def test_promoted_actions_must_be_a_nonempty_trained_subset() -> None:
    revive.validate_promoted_actions((3, 8), (3,))

    with pytest.raises(ValueError, match="at least one"):
        revive.validate_promoted_actions((3, 8), ())
    with pytest.raises(ValueError, match=r"not trained: \[9\]"):
        revive.validate_promoted_actions((3, 8), (3, 9))


def test_restore_unpromoted_rows_keeps_only_the_gated_subset() -> None:
    policy = make_policy("mlp")
    trained = (3, 8)
    indices = torch.as_tensor(trained)
    with torch.no_grad():
        parent_weight = policy.pi.weight.index_select(0, indices).clone()
        parent_bias = policy.pi.bias.index_select(0, indices).clone()
        policy.pi.weight[3].add_(1.0)
        policy.pi.bias[3].add_(1.0)
        policy.pi.weight[8].add_(2.0)
        policy.pi.bias[8].add_(2.0)
        promoted_weight = policy.pi.weight[3].clone()
        promoted_bias = policy.pi.bias[3].clone()

    revive.restore_unpromoted_rows(
        policy,
        parent_weight,
        parent_bias,
        trained,
        (3,),
    )

    assert torch.equal(policy.pi.weight[3], promoted_weight)
    assert torch.equal(policy.pi.bias[3], promoted_bias)
    assert torch.equal(policy.pi.weight[8], parent_weight[1])
    assert torch.equal(policy.pi.bias[8], parent_bias[1])


def test_final_artifact_audit_excludes_a_restored_joint_training_row() -> None:
    torch.manual_seed(13)
    parent = make_policy("mlp")
    candidate = copy.deepcopy(parent)
    dataset = _dataset()
    revive.train_selected_rows(candidate, dataset, _config())
    trained = (3, 8)
    indices = torch.as_tensor(trained)
    revive.restore_unpromoted_rows(
        candidate,
        parent.pi.weight.index_select(0, indices),
        parent.pi.bias.index_select(0, indices),
        trained,
        (3,),
    )

    audit = revive.audit_selected_policy(parent, candidate, dataset, (3,), 5)

    assert set(audit["held_target_counts"]) == {"3"}
    assert set(audit["held_target_greedy_rates"]) == {"3"}
    assert torch.equal(candidate.pi.weight[8], parent.pi.weight[8])
    assert torch.equal(candidate.pi.bias[8], parent.pi.bias[8])


@pytest.mark.parametrize(
    ("parser", "text"),
    [
        (revive.unit_interval, "nan"),
        (revive.unit_interval, "inf"),
        (revive.nonnegative_float, "nan"),
        (revive.nonnegative_float, "inf"),
    ],
)
def test_numeric_threshold_parsers_reject_non_finite_values(
    parser: Callable[[str], float],
    text: str,
) -> None:
    with pytest.raises(argparse.ArgumentTypeError):
        parser(text)


def test_masked_teacher_targets_are_rejected_before_cross_entropy() -> None:
    logits = torch.zeros(2, len(revive.PRODUCTION_HEAD))
    mask = torch.ones_like(logits, dtype=torch.bool)
    mask[1, 8] = False
    with pytest.raises(ValueError, match="masked teacher targets"):
        revive.balanced_selected_loss(
            logits,
            mask,
            torch.tensor([3, 8]),
            (3, 8),
        )


def test_positive_loss_rejects_a_batch_missing_one_selected_action() -> None:
    logits = torch.zeros(2, len(revive.PRODUCTION_HEAD))
    mask = torch.ones_like(logits, dtype=torch.bool)
    with pytest.raises(ValueError, match="no rows for action 8"):
        revive.balanced_selected_loss(
            logits,
            mask,
            torch.tensor([3, 3]),
            (3, 8),
        )


def test_pooled_positive_batch_repairs_an_omitted_rare_action() -> None:
    targets = torch.tensor([3, 3, 3, 3, 8])
    batch = torch.tensor([0, 1, 2, 3])
    repaired = revive.ensure_selected_actions(
        batch,
        targets,
        {3: torch.tensor([0, 1, 2, 3]), 8: torch.tensor([4])},
        torch.Generator().manual_seed(9),
    )
    assert set(targets[repaired].tolist()) == {3, 8}
    assert torch.equal(batch, torch.tensor([0, 1, 2, 3])), (
        "repairing a sample must not mutate the caller's original batch"
    )


def test_training_moves_only_selected_policy_rows() -> None:
    torch.manual_seed(5)
    policy = make_policy("mlp")
    before = {name: value.clone() for name, value in policy.state_dict().items()}
    revive.train_selected_rows(policy, _dataset(), _config())
    after = policy.state_dict()

    for name, value in before.items():
        if name not in ("pi.weight", "pi.bias"):
            torch.testing.assert_close(after[name], value, rtol=0, atol=0)
    selected = torch.tensor([3, 8])
    other = torch.tensor([action for action in range(ACTIONS) if action not in (3, 8)])
    assert not torch.equal(after["pi.weight"][selected], before["pi.weight"][selected])
    assert not torch.equal(after["pi.bias"][selected], before["pi.bias"][selected])
    torch.testing.assert_close(
        after["pi.weight"][other],
        before["pi.weight"][other],
        rtol=0,
        atol=0,
    )
    torch.testing.assert_close(
        after["pi.bias"][other],
        before["pi.bias"][other],
        rtol=0,
        atol=0,
    )


def test_split_and_training_are_deterministic() -> None:
    data = _dataset()
    first_split = revive.split_corpus(data, (3, 8), 5)
    second_split = revive.split_corpus(data, (3, 8), 5)
    assert torch.equal(first_split.train_positive, second_split.train_positive)
    assert torch.equal(first_split.held_retention, second_split.held_retention)

    torch.manual_seed(11)
    first = make_policy("mlp")
    second = make_policy("mlp")
    second.load_state_dict(first.state_dict())
    first_audit = revive.train_selected_rows(first, data, _config())
    second_audit = revive.train_selected_rows(second, data, _config())
    torch.testing.assert_close(first.pi.weight, second.pi.weight, rtol=0, atol=0)
    torch.testing.assert_close(first.pi.bias, second.pi.bias, rtol=0, atol=0)
    assert first_audit == second_audit


def test_retention_audit_reports_new_choices_and_kl() -> None:
    data = _dataset()
    split = revive.split_corpus(data, (3, 8), 5)
    production_mask = data.mask[:, : len(revive.PRODUCTION_HEAD)]
    parent = torch.zeros(len(data.target), len(revive.PRODUCTION_HEAD))
    parent[:, 2] = 2.0
    candidate = parent.clone()
    candidate[:, 3] = 3.0
    candidate[data.target == 8, 8] = 4.0
    audit = revive.retention_audit(
        parent,
        candidate,
        production_mask,
        data.target,
        split,
        (3, 8),
    )
    assert audit["held_target_counts"] == {"3": 4, "8": 4}
    assert audit["held_target_greedy_rates"] == {"3": 1.0, "8": 1.0}
    assert audit["held_retention_new_selected_greedy"] > 0
    assert audit["held_retention_new_selected_rate"] > 0.0
    assert audit["held_retention_mean_kl"] > 0.0


def test_audit_thresholds_are_enforced() -> None:
    audit = {
        "held_target_greedy_rates": {"3": 0.4},
        "held_retention_new_selected_rate": 0.02,
        "held_retention_mean_kl": 0.03,
    }
    with pytest.raises(ValueError, match="action 3"):
        revive.enforce_audit(
            audit,
            min_target_greedy=0.5,
            max_new_selected_rate=0.01,
            max_mean_kl=0.02,
        )
    revive.enforce_audit(
        {
            "held_target_greedy_rates": {"3": 0.6},
            "held_retention_new_selected_rate": 0.005,
            "held_retention_mean_kl": 0.01,
        },
        min_target_greedy=0.5,
        max_new_selected_rate=0.01,
        max_mean_kl=0.02,
    )
