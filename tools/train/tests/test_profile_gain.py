"""Tests for the exact profile-column calibration transform."""

from __future__ import annotations

import copy
import json
from typing import TYPE_CHECKING

import pytest

from lineage import build_lineage, validate_lineage
from oxide_gym import CONDITION_NAMES, FEATURES, GYM_VERSION
from profile_gain import (
    MAX_COEFF,
    apply_profile_gain,
    calibration_lineage,
    round_ratio,
)

if TYPE_CHECKING:
    import pathlib


def artifact() -> dict:
    width = FEATURES + len(CONDITION_NAMES)
    return {
        "gym_version": GYM_VERSION,
        "update": 210,
        "features": FEATURES,
        "conditioning": len(CONDITION_NAMES),
        "layers": [
            {
                "w": [
                    list(range(width)),
                    [-value for value in range(width)],
                ],
                "b": [1, -1],
            }
        ],
        "head": {"w": [[3, 4]], "b": [5]},
    }


def test_gain_changes_only_the_selected_first_layer_column() -> None:
    source = artifact()
    before = copy.deepcopy(source)
    calibrated = apply_profile_gain(source, "profile_siege", 2)
    selected = FEATURES + CONDITION_NAMES.index("profile_siege")

    assert source == before
    for row_index, row in enumerate(calibrated["layers"][0]["w"]):
        for column, value in enumerate(row):
            expected = before["layers"][0]["w"][row_index][column]
            if column == selected:
                expected *= 2
            assert value == expected
    expected = copy.deepcopy(before)
    for row in expected["layers"][0]["w"]:
        row[selected] *= 2
    assert calibrated == expected


def test_rational_rounding_is_integer_exact_and_symmetric() -> None:
    assert round_ratio(1, 1, 2) == 1
    assert round_ratio(-1, 1, 2) == -1
    assert round_ratio(3, 1, 2) == 2
    assert round_ratio(-3, 1, 2) == -2


def test_gain_rejects_contract_drift_and_unsafe_coefficients() -> None:
    wrong = artifact()
    wrong["conditioning"] -= 1
    with pytest.raises(ValueError, match="conditioning count"):
        apply_profile_gain(wrong, "profile_siege", 2)

    unsafe = artifact()
    selected = FEATURES + CONDITION_NAMES.index("profile_siege")
    unsafe["layers"][0]["w"][0][selected] = MAX_COEFF
    with pytest.raises(ValueError, match="calibrated coefficient"):
        apply_profile_gain(unsafe, "profile_siege", 2)


def test_calibration_lineage_names_source_code_and_exact_gain(
    tmp_path: pathlib.Path,
) -> None:
    source = tmp_path / "source.json"
    metadata = artifact()
    metadata["lineage"] = build_lineage(
        phase="test-parent",
        phase_start_update=200,
        hyperparameters={},
    )
    source.write_text(json.dumps(metadata))

    lineage = calibration_lineage(source, metadata, "profile_siege", 2, 1)

    assert validate_lineage(lineage) == lineage
    assert lineage["phase"] == "profile-column-gain"
    assert lineage["phase_start_update"] == 210
    assert lineage["hyperparameters"] == {
        "column": "profile_siege",
        "denominator": 1,
        "numerator": 2,
        "rounding": "nearest-ties-away-from-zero",
    }
    inputs = lineage["inputs"]
    assert isinstance(inputs, dict)
    assert set(inputs) == {"source", "transformer_code"}
