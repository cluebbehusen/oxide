"""The doctrine override's pure decision table."""

import numpy as np
import torch

from bc import masked_head_cross_entropy
from doctrine_bc import (
    FORM_ARMY,
    LESSONS,
    PUSH,
    SCOUT,
    anchored_head_loss,
    apply_doctrine,
    doctrine_weighted_cross_entropy,
)
from oxide_gym import ACTIONS, FEATURE_NAMES

F = {name: i for i, name in enumerate(FEATURE_NAMES)}
IDLE_PLAN = (0, 24, 42, 25)


def raw_with(**counts: int) -> list[int]:
    raw = [0] * len(FEATURE_NAMES)
    for name, value in counts.items():
        raw[F[name]] = value
    return raw


def open_mask(*actions: int) -> np.ndarray:
    mask = np.zeros(ACTIONS, dtype=bool)
    for action in (*IDLE_PLAN, *actions):
        mask[action] = True
    return mask


def test_a_met_quota_never_forces() -> None:
    action, counter, quota, _ = LESSONS["skyhook"]
    plan, forced = apply_doctrine(
        IDLE_PLAN, "skyhook", raw_with(**{counter: quota}), open_mask(action)
    )
    assert plan == IDLE_PLAN
    assert not forced


def test_a_legal_production_target_overrides_that_head_only() -> None:
    action, _, _, _ = LESSONS["skyhook"]
    plan, forced = apply_doctrine(IDLE_PLAN, "skyhook", raw_with(), open_mask(action))
    assert forced
    assert plan == (action, IDLE_PLAN[1], IDLE_PLAN[2], IDLE_PLAN[3])


def test_a_legal_construction_target_overrides_the_construction_head() -> None:
    action, _, _, _ = LESSONS["bastion"]
    plan, forced = apply_doctrine(IDLE_PLAN, "bastion", raw_with(), open_mask(action))
    assert forced
    assert plan == (IDLE_PLAN[0], action, IDLE_PLAN[2], IDLE_PLAN[3])


def test_a_closed_target_chains_through_its_missing_producer() -> None:
    _, _, _, chain = LESSONS["skyhook"]
    link, _ = chain[0]
    plan, forced = apply_doctrine(IDLE_PLAN, "skyhook", raw_with(), open_mask(link))
    assert forced
    assert plan == (IDLE_PLAN[0], link, IDLE_PLAN[2], IDLE_PLAN[3])


def test_a_standing_producer_stops_the_chain() -> None:
    _, _, _, chain = LESSONS["skyhook"]
    link, link_counter = chain[0]
    plan, forced = apply_doctrine(
        IDLE_PLAN, "skyhook", raw_with(**{link_counter: 1}), open_mask(link)
    )
    assert plan == IDLE_PLAN
    assert not forced


def test_an_everything_closed_think_passes_through() -> None:
    plan, forced = apply_doctrine(IDLE_PLAN, "skyhook", raw_with(), open_mask())
    assert plan == IDLE_PLAN
    assert not forced


def test_the_hunt_scouts_then_stages_then_commits() -> None:
    dark = raw_with()
    plan, forced = apply_doctrine(IDLE_PLAN, "hunt", dark, open_mask(SCOUT))
    assert forced
    assert plan[3] == SCOUT

    known = raw_with(enemy_foundry_known=1)
    plan, forced = apply_doctrine(IDLE_PLAN, "hunt", known, open_mask(FORM_ARMY))
    assert forced
    assert plan[3] == FORM_ARMY

    staged = raw_with(enemy_foundry_known=1, staging_army_size=6)
    plan, forced = apply_doctrine(IDLE_PLAN, "hunt", staged, open_mask(PUSH))
    assert forced
    assert plan[3] == PUSH


def test_every_lesson_counter_is_a_real_feature() -> None:
    for name, (_, counter, quota, chain) in LESSONS.items():
        assert counter in F, f"{name} meters a missing feature"
        assert quota >= 1, name
        for _, link_counter in chain:
            assert link_counter in F, f"{name} chains through a missing feature"


def test_z_doctrine_weight_of_one_matches_the_unweighted_teach() -> None:
    torch.manual_seed(7)
    logits = torch.randn(6, 5)
    targets = torch.tensor([0, 1, 2, 3, 4, 0])
    class_weights = torch.tensor([1.0, 2.0, 0.5, 1.0, 3.0])
    ones = torch.ones(6)
    weighted = doctrine_weighted_cross_entropy(logits, targets, class_weights, ones, 0)
    plain = masked_head_cross_entropy(logits, targets, class_weights, 0)
    assert torch.allclose(weighted, plain)


def test_z_doctrine_weight_pulls_loss_toward_forced_samples() -> None:
    torch.manual_seed(7)
    logits = torch.randn(4, 3)
    targets = torch.tensor([0, 1, 2, 0])
    class_weights = torch.ones(3)
    per_sample = torch.nn.functional.cross_entropy(logits, targets, reduction="none")
    forced = torch.tensor([1.0, 1.0, 8.0, 1.0])
    weighted = doctrine_weighted_cross_entropy(
        logits, targets, class_weights, forced, 0
    )
    unweighted = per_sample.mean()
    assert (weighted - unweighted).sign() == (per_sample[2] - unweighted).sign()


def test_z_anchored_loss_is_zero_when_the_student_is_the_founder() -> None:
    torch.manual_seed(7)
    logits = torch.randn(5, 4)
    logits[:, 3] = float("-inf")
    targets = torch.tensor([0, 1, 2, 0, 1])
    natural_only = torch.zeros(5, dtype=torch.bool)
    loss = anchored_head_loss(
        logits, logits.clone(), targets, torch.ones(4), natural_only, 0
    )
    assert torch.allclose(loss, torch.zeros(()), atol=1e-6)


def test_z_anchored_loss_weighs_forced_samples_like_the_plain_teach() -> None:
    torch.manual_seed(7)
    logits = torch.randn(4, 3)
    targets = torch.tensor([2, 2, 2, 2])
    forced = torch.tensor([True, False, True, False])
    anchored = anchored_head_loss(
        logits, logits.clone(), targets, torch.ones(3), forced, 0
    )
    ce = torch.nn.functional.cross_entropy(
        logits[forced], targets[forced], reduction="none"
    )
    assert torch.allclose(anchored, ce.sum() / 4)
