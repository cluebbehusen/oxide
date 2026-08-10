"""Focused tests for the factorized behavior-cloning teachers."""

import json
from typing import TYPE_CHECKING

import numpy as np
import pytest
import torch

import bc
from oxide_gym import ACTION_HEADS, ACTIONS, FEATURES

if TYPE_CHECKING:
    import pathlib


def _raw() -> list[int]:
    raw = [0] * FEATURES
    raw[bc.F["fab_built"]] = 1
    raw[bc.F["my_harvesters"]] = 6
    raw[bc.F["known_salvage_value"]] = 1_000
    raw[bc.F["near_home_salvage_value"]] = 1_000
    return raw


def test_every_teacher_emits_one_legal_global_index_per_head() -> None:
    raw = _raw()
    mask = np.ones(ACTIONS, dtype=bool)
    for strategy in bc.STRATEGIES:
        plan = bc.teacher(strategy, raw, mask, 1_024)
        for head_index, head in enumerate(ACTION_HEADS):
            assert plan[head_index] in head
            assert mask[plan[head_index]]


def test_teacher_reconciles_a_masked_noop_to_the_legal_head_action() -> None:
    raw = _raw()
    mask = np.zeros(ACTIONS, dtype=bool)
    mask[[bc.IDLE, bc.NO_CONSTRUCTION, bc.NO_UPGRADE, bc.RECALL]] = True
    assert bc.teacher("industry", raw, mask, 1_024) == (
        bc.IDLE,
        bc.NO_CONSTRUCTION,
        bc.NO_UPGRADE,
        bc.RECALL,
    )


def test_teacher_refuses_an_action_head_without_a_legal_target() -> None:
    raw = _raw()
    mask = np.zeros(ACTIONS, dtype=bool)
    mask[[bc.IDLE, bc.NO_CONSTRUCTION, bc.NO_UPGRADE]] = True
    with pytest.raises(ValueError, match="action head 3 has no legal"):
        bc.teacher("industry", raw, mask, 1_024)


def test_masked_head_loss_rejects_an_illegal_teacher_target() -> None:
    logits = torch.tensor([[0.0, float("-inf"), 1.0]])
    targets = torch.tensor([1])
    weights = torch.tensor([1.0, 2.0, 1.0])
    with pytest.raises(ValueError, match="masked or non-finite in head 2"):
        bc.masked_head_cross_entropy(logits, targets, weights, 2)


def test_masked_head_loss_retains_masking_and_class_weights() -> None:
    logits = torch.tensor(
        [
            [0.0, float("-inf"), 1.0],
            [0.0, float("-inf"), 1.0],
        ],
        requires_grad=True,
    )
    targets = torch.tensor([0, 2])
    weights = torch.tensor([3.0, 1.0, 1.0])
    actual = bc.masked_head_cross_entropy(logits, targets, weights, 0)
    expected = torch.nn.functional.cross_entropy(logits, targets, weight=weights)
    torch.testing.assert_close(actual, expected)
    actual.backward()
    assert logits.grad is not None
    assert bool(torch.isfinite(logits.grad).all().item())


def test_production_teaches_intent_without_reading_affordability() -> None:
    # Tier-one wants stay affordability-blind: intent is the lesson and
    # the lowering owns the bank. The one scout flyer comes first on the
    # v9 surface (it is the only scout an island map allows), so the
    # fixture already fields it.
    raw = _raw()
    raw[bc.F["scrap"]] = 0
    raw[bc.F["my_sentinels"]] = 4
    raw[bc.F["my_scout_flyers"]] = 1
    mask = np.ones(ACTIONS, dtype=bool)
    assert bc.production_teacher("combined", raw, mask) == bc.TRAIN_LANCER


def test_tier_wants_wait_for_the_bank() -> None:
    # The deliberate exception: an unaffordable tier-two want would
    # label half the corpus with purchases the lowering cannot make.
    raw = _raw()
    raw[bc.F["my_sentinels"]] = 4
    raw[bc.F["my_lancers"]] = 3
    raw[bc.F["my_bombards"]] = 2
    raw[bc.F["my_scout_flyers"]] = 1
    mask = np.ones(ACTIONS, dtype=bool)
    raw[bc.F["scrap"]] = 0
    poor = bc.production_teacher("combined", raw, mask)
    assert poor != bc.TRAIN_WARDEN
    raw[bc.F["scrap"]] = 300
    assert bc.production_teacher("combined", raw, mask) == bc.TRAIN_WARDEN


def test_an_existing_capital_plan_is_preserved() -> None:
    raw = _raw()
    raw[bc.F["construction_plan"]] = 3
    mask = np.ones(ACTIONS, dtype=bool)
    for strategy in bc.STRATEGIES:
        assert bc.construction_teacher(strategy, raw, mask) == bc.NO_CONSTRUCTION


def test_a_live_construction_site_prevents_capital_chaining() -> None:
    raw = _raw()
    raw[bc.F["my_construction_sites"]] = 1
    mask = np.ones(ACTIONS, dtype=bool)
    for strategy in bc.STRATEGIES:
        assert bc.construction_teacher(strategy, raw, mask) == bc.NO_CONSTRUCTION


def test_live_projects_do_not_suppress_maintenance_or_emergency_defense() -> None:
    mask = np.ones(ACTIONS, dtype=bool)

    repair = _raw()
    repair[bc.F["construction_plan"]] = 3
    repair[bc.F["repair_deficit"]] = 200
    assert bc.construction_teacher("industry", repair, mask) == bc.REPAIR

    weld = _raw()
    weld[bc.F["my_construction_sites"]] = 1
    weld[bc.F["damaged_unit_value"]] = 100
    assert bc.construction_teacher("combined", weld, mask) == bc.REPAIR_UNIT

    flak = _raw()
    flak[bc.F["construction_plan"]] = 5
    flak[bc.F["enemy_airground"]] = 1
    assert bc.construction_teacher("fortify", flak, mask) == bc.BUILD_FLAK


def test_pressure_commits_earlier_than_fortify() -> None:
    raw = _raw()
    raw[bc.F["staging_army_size"]] = 9
    mask = np.zeros(ACTIONS, dtype=bool)
    mask[[bc.NO_OPERATION, bc.FORM, bc.PUSH]] = True
    assert bc.operations_teacher("fortify", raw, mask, 500) == bc.FORM
    assert bc.operations_teacher("pressure", raw, mask, 500) == bc.PUSH


def test_global_plans_map_to_the_correct_noncontiguous_local_classes() -> None:
    targets = bc.local_action_targets(
        [
            (0, 24, 42, 25),
            (8, 9, 40, 20),
            (1, 23, 42, 16),
        ]
    )
    np.testing.assert_array_equal(targets[0], [0, 8, 1])
    np.testing.assert_array_equal(targets[1], [0, 1, 10])
    np.testing.assert_array_equal(targets[2], [0, 1, 0])
    np.testing.assert_array_equal(targets[3], [0, 5, 1])

    with pytest.raises(ValueError, match="head 1"):
        bc.local_action_targets([(0, 8, 42, 25)])


def test_the_demonstration_slate_keeps_only_duel_maps(
    tmp_path: pathlib.Path,
) -> None:
    (tmp_path / "b.json").write_text(json.dumps({"players": [{}, {}]}))
    (tmp_path / "a.json").write_text(json.dumps({"players": [{}, {}, {}, {}]}))
    (tmp_path / "c.json").write_text(json.dumps({"players": [{}, {}]}))
    assert [path.name for path in bc.duel_scenarios(tmp_path)] == [
        "b.json",
        "c.json",
    ]


def test_the_128_episode_schedule_crosses_maps_without_faction_aliasing() -> None:
    cases = [bc.episode_assignment(ep, 16) for ep in range(128)]
    for strategy in bc.STRATEGIES:
        for seat in (0, 1):
            maps = {
                map_index
                for assigned, assigned_seat, map_index, _factions in cases
                if assigned == strategy and assigned_seat == seat
            }
            assert maps == set(range(16))

    for map_index in range(16):
        pairs = {
            factions
            for _strategy, _seat, assigned_map, factions in cases
            if assigned_map == map_index
        }
        assert pairs == set(bc.FACTION_PAIRS)


def test_a_second_schedule_pass_rotates_each_exact_map_cell() -> None:
    cases = [bc.episode_assignment(ep, 16) for ep in range(256)]
    for strategy in bc.STRATEGIES:
        for seat in (0, 1):
            for map_index in range(16):
                pairs = {
                    factions
                    for assigned, assigned_seat, assigned_map, factions in cases
                    if assigned == strategy
                    and assigned_seat == seat
                    and assigned_map == map_index
                }
                assert len(pairs) == 2
