"""Tests for path-independent, content-addressed training lineage."""

from __future__ import annotations

import shutil
from typing import TYPE_CHECKING

import pytest

from lineage import (
    build_lineage,
    checkpoint_metadata,
    input_identity,
    validate_lineage,
)

if TYPE_CHECKING:
    import pathlib


def test_moved_same_content_inputs_keep_the_same_identity(
    tmp_path: pathlib.Path,
) -> None:
    original = tmp_path / "parent.pt"
    original.write_bytes(b"same checkpoint bytes")
    moved = tmp_path / "renamed" / "different-name.pt"
    moved.parent.mkdir()
    shutil.copyfile(original, moved)

    first = input_identity(original)
    second = input_identity(moved)
    assert first == second
    assert (
        build_lineage(
            phase="league",
            phase_start_update=75,
            hyperparameters={"lr": 1e-4},
            inputs={"initializer": first},
        )["lineage_id"]
        == build_lineage(
            phase="league",
            phase_start_update=75,
            hyperparameters={"lr": 1e-4},
            inputs={"initializer": second},
        )["lineage_id"]
    )


def test_changed_input_content_changes_the_lineage_id(
    tmp_path: pathlib.Path,
) -> None:
    source = tmp_path / "parent.pt"
    source.write_bytes(b"first")
    first = build_lineage(
        phase="revival",
        phase_start_update=95,
        hyperparameters={"actions": [3, 8]},
        inputs={"source": input_identity(source)},
    )
    source.write_bytes(b"second")
    second = build_lineage(
        phase="revival",
        phase_start_update=95,
        hyperparameters={"actions": [3, 8]},
        inputs={"source": input_identity(source)},
    )
    assert first["lineage_id"] != second["lineage_id"]


def test_lineage_ids_are_deterministic_across_mapping_order() -> None:
    first = build_lineage(
        phase="bc",
        phase_start_update=0,
        hyperparameters={"epochs": 20, "lr": 1e-3},
        inputs={
            "anchor": {"content_sha256": "sha256:" + "a" * 64},
            "initializer": {"content_sha256": "sha256:" + "b" * 64},
        },
    )
    second = build_lineage(
        phase="bc",
        phase_start_update=0,
        hyperparameters={"lr": 1e-3, "epochs": 20},
        inputs={
            "initializer": {"content_sha256": "sha256:" + "b" * 64},
            "anchor": {"content_sha256": "sha256:" + "a" * 64},
        },
    )
    assert first == second


def test_checkpoint_metadata_propagates_a_verified_manifest() -> None:
    lineage = build_lineage(
        phase="league",
        phase_start_update=1_450,
        hyperparameters={"updates": 12},
    )
    metadata = checkpoint_metadata(
        lineage,
        {"gym_version": 7, "update": 1_462},
    )
    assert metadata["lineage"] == lineage
    assert metadata["update"] == 1_462


def test_tampered_lineage_is_rejected() -> None:
    lineage = build_lineage(
        phase="league",
        phase_start_update=75,
        hyperparameters={"lr": 1e-4},
    )
    lineage["phase_start_update"] = 76
    with pytest.raises(ValueError, match="does not match"):
        validate_lineage(lineage)
