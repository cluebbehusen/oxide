"""Tests for path-independent, content-addressed training lineage."""

from __future__ import annotations

import shutil
from typing import TYPE_CHECKING

import pytest

if TYPE_CHECKING:
    from collections.abc import Callable

from lineage import (
    build_lineage,
    checkpoint_metadata,
    inherited_lineage_id,
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


def _valid_manifest() -> dict:
    return build_lineage(
        phase="league-r1",
        phase_start_update=5,
        hyperparameters={"lr": 0.001},
        inputs={"prior": {"content_sha256": "sha256:" + "a" * 64}},
    )


@pytest.mark.parametrize(
    ("mutate", "message"),
    [
        (lambda _m: "not a dict", "must be an object"),
        (lambda m: {1: "x", **m}, "keys must be strings"),
        (
            lambda m: {k: v for k, v in m.items() if k != "lineage_id"},
            "carry a lineage_id",
        ),
        (lambda m: {**m, "schema": 2}, "unsupported lineage schema"),
        (
            lambda m: {**m, "lineage_id": "sha256:" + "A" * 64},
            "must be a SHA-256 digest",
        ),
        (
            lambda m: {**m, "lineage_id": "sha256:" + "a" * 63},
            "must be a SHA-256 digest",
        ),
        (lambda m: {**m, "phase": ""}, "non-empty string"),
        (lambda m: {**m, "phase_start_update": True}, "non-negative integer"),
        (lambda m: {**m, "phase_start_update": -1}, "non-negative integer"),
        (lambda m: {**m, "phase_start_update": 1.0}, "non-negative integer"),
        (lambda m: {**m, "hyperparameters": []}, "hyperparameters must be an object"),
        (lambda m: {**m, "inputs": []}, "inputs must be an object"),
        (
            lambda m: {**m, "inputs": {"": {"content_sha256": "sha256:" + "a" * 64}}},
            "non-empty strings",
        ),
        (lambda m: {**m, "inputs": {"prior": "bytes"}}, "must be an object"),
        (
            lambda m: {**m, "inputs": {"prior": {"content_sha256": "not-a-digest"}}},
            "content digest",
        ),
        (
            lambda m: {
                **m,
                "inputs": {
                    "prior": {
                        "content_sha256": "sha256:" + "a" * 64,
                        "lineage_id": "bogus",
                    }
                },
            },
            "invalid upstream lineage id",
        ),
        (lambda m: {**m, "phase": m["phase"] + "-tampered"}, "does not match"),
    ],
)
def test_every_structural_forgery_is_rejected_by_name(
    mutate: Callable[[dict], object], message: str
) -> None:
    # The audit measured every raise in validate_lineage at zero
    # execution: provenance is the training stack's trust boundary, and
    # an unexercised rejection is one refactor away from accepting a
    # forged manifest silently. Each row pins its specific message so a
    # reordered check cannot quietly swallow a case.
    forged = mutate(_valid_manifest())
    with pytest.raises((TypeError, ValueError), match=message):
        validate_lineage(forged)


def test_build_lineage_refuses_degenerate_arguments() -> None:
    with pytest.raises(ValueError, match="must not be empty"):
        build_lineage(phase="", phase_start_update=0, hyperparameters={})
    with pytest.raises(ValueError, match="non-negative"):
        build_lineage(phase="x", phase_start_update=-1, hyperparameters={})


def test_inherited_lineage_id_verifies_before_propagating() -> None:
    manifest = _valid_manifest()
    assert inherited_lineage_id({"lineage": manifest}) == manifest["lineage_id"]
    assert inherited_lineage_id(None) is None
    assert inherited_lineage_id({}) is None
    tampered = {**manifest, "phase": "forged-history"}
    with pytest.raises(ValueError, match="does not match"):
        inherited_lineage_id({"lineage": tampered})
