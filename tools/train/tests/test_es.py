"""The searcher's pure parts: artifact round-trips, mutation scales,
rank weights, battery parsing, and the trust region's acceptance rule.
The subprocess loop itself is exercised by shakedown runs, not here.
"""

import json

import numpy as np
import pytest

from es import (
    MAX_COEFF,
    accept_center,
    cup_wins,
    flatten,
    parse_cup,
    parse_family_counts,
    rank_weights,
    sigma_vector,
    unflatten,
)


def tiny_artifact() -> dict:
    return {
        "q_bits": 12,
        "layers": [
            {"w": [[100, -200], [300, 400]], "b": [5, -6]},
            {"w": [[7, 8], [9, 10]], "b": [0, 1]},
        ],
        "head": {"w": [[11, 12], [13, 14], [15, 16]], "b": [1, 2, 3]},
        "lineage": {"founder": "someone"},
    }


def test_flatten_unflatten_round_trips_every_weight() -> None:
    artifact = tiny_artifact()
    vector, spec = flatten(artifact)
    rebuilt = unflatten(vector, spec, artifact)
    for field in ("layers", "head"):
        assert rebuilt[field] == artifact[field]


def test_unflatten_drops_lineage_and_keeps_metadata() -> None:
    artifact = tiny_artifact()
    vector, spec = flatten(artifact)
    rebuilt = unflatten(vector, spec, artifact)
    assert "lineage" not in rebuilt
    assert rebuilt["q_bits"] == 12
    assert "lineage" in artifact, "the template must not be mutated"


def test_unflatten_rounds_and_clamps_to_the_loader_ceiling() -> None:
    artifact = tiny_artifact()
    vector, spec = flatten(artifact)
    vector = vector + 0.4  # rounds back down
    vector[0] = MAX_COEFF * 3.0  # clamps
    rebuilt = unflatten(vector, spec, artifact)
    assert rebuilt["layers"][0]["w"][0][0] == MAX_COEFF
    assert rebuilt["layers"][0]["w"][0][1] == -200
    assert json.loads(json.dumps(rebuilt)) == rebuilt


def test_unflatten_rejects_a_vector_of_the_wrong_size() -> None:
    artifact = tiny_artifact()
    vector, spec = flatten(artifact)
    with pytest.raises(ValueError, match="params"):
        unflatten(np.append(vector, 1.0), spec, artifact)


def test_sigma_scales_per_tensor_with_a_floor() -> None:
    artifact = tiny_artifact()
    vector, spec = flatten(artifact)
    sigma = sigma_vector(vector, spec, rel=0.1, floor=1.0)
    assert sigma.shape == vector.shape
    first_w = np.asarray(artifact["layers"][0]["w"], dtype=np.float64)
    assert sigma[0] == pytest.approx(max(1.0, float(np.std(first_w)) * 0.1))
    # A near-constant bias tensor still gets the one-unit floor.
    assert np.all(sigma >= 1.0)


def test_rank_weights_are_centered_and_monotone() -> None:
    weights = rank_weights(24)
    assert weights.sum() == pytest.approx(0.0)
    assert np.all(np.diff(weights) > 0)
    assert weights[0] == -0.5
    assert weights[-1] == 0.5
    with pytest.raises(ValueError, match="two candidates"):
        rank_weights(1)


def test_parse_cup_reads_both_opponents_and_skips_banners() -> None:
    stdout = "\n".join(
        [
            "artifact: x · digest abc",
            json.dumps({"opponent": "Overseer", "wins": 20, "games": 24}),
            json.dumps({"opponent": "Rusher", "wins": 12, "games": 24}),
            json.dumps({"not_a_cup_row": True}),
        ]
    )
    scores = parse_cup(stdout)
    assert scores == {
        "overseer_wins": 20,
        "overseer_games": 24,
        "rusher_wins": 12,
        "rusher_games": 24,
    }
    assert cup_wins(scores) == 32


def test_parse_family_counts_reads_the_gate_summary() -> None:
    report = (
        "noise\nstyle-family signatures / 7 seeds: development 0, "
        "fortification 4, force 7, mobile pressure 7\nmore noise"
    )
    assert parse_family_counts(report) == {
        "development": 0,
        "fortification": 4,
        "force": 7,
        "mobile pressure": 7,
    }
    assert parse_family_counts("the gate panicked early") is None


def base_scores(**overrides: object) -> dict:
    scores = {
        "wins": 60,
        "style_failures": 1,
        "families": {"development": 0, "fortification": 4, "force": 7},
    }
    scores.update(overrides)
    return scores


def test_accept_center_rejects_new_style_failures() -> None:
    ok, verdict = accept_center(base_scores(), base_scores(style_failures=2), 2)
    assert not ok
    assert "style" in verdict


def test_accept_center_rejects_a_family_falling_below_its_floor() -> None:
    fallen = base_scores(families={"development": 0, "fortification": 3, "force": 7})
    ok, verdict = accept_center(base_scores(), fallen, 2)
    assert not ok
    assert "fortification" in verdict


def test_accept_center_lets_a_saturated_family_breathe_above_four() -> None:
    # force held 7/7; dropping to 5 stays above the gate floor of 4
    # and must not block a step.
    eased = base_scores(families={"development": 1, "fortification": 4, "force": 5})
    ok, _ = accept_center(base_scores(), eased, 2)
    assert ok


def test_accept_center_enforces_the_win_slack_exactly() -> None:
    ok, _ = accept_center(base_scores(), base_scores(wins=58), 2)
    assert ok
    ok, verdict = accept_center(base_scores(), base_scores(wins=57), 2)
    assert not ok
    assert "slack" in verdict


def test_accept_center_welcomes_improvement_on_both_axes() -> None:
    better = base_scores(
        wins=64,
        style_failures=0,
        families={"development": 5, "fortification": 6, "force": 7},
    )
    ok, verdict = accept_center(base_scores(), better, 2)
    assert ok
    assert verdict == "ok"
