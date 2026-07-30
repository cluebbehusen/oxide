"""Tests for recovering a trainable checkpoint from a Q12 artifact."""

import argparse
import json
from typing import TYPE_CHECKING

import pytest
import torch

import dequantize
from export import export
from lineage import build_lineage, checkpoint_metadata
from models import load_policy, make_policy, save_policy
from oxide_gym import ACTIONS, GYM_VERSION

if TYPE_CHECKING:
    import pathlib


def _bridge_artifact(
    tmp_path: pathlib.Path,
    *,
    lineaged: bool = False,
) -> pathlib.Path:
    torch.manual_seed(11)
    policy = make_policy("mlp")
    with torch.no_grad():
        for action in (ACTIONS - 2, ACTIONS - 1):
            policy.pi.weight[action].zero_()
            policy.pi.bias[action].fill_(-8.0)
    ckpt = tmp_path / "source.pt"
    metadata = {"gym_version": GYM_VERSION, "update": 17}
    if lineaged:
        metadata = checkpoint_metadata(
            build_lineage(
                phase="test-parent",
                phase_start_update=17,
                hyperparameters={"fixture": True},
            ),
            metadata,
        )
    save_policy(policy, "mlp", ckpt, metadata)
    weights = tmp_path / "source.json"
    export(str(ckpt), str(weights))
    return weights


class TestExactRecovery:
    def test_the_recovered_actor_round_trips_exactly_and_the_critic_is_zero(
        self, tmp_path: pathlib.Path
    ) -> None:
        weights = _bridge_artifact(tmp_path)
        out = tmp_path / "recovered.pt"
        dequantize.dequantize(weights, out)

        policy, blob = load_policy(str(out))
        assert blob["critic_ready"] is False
        assert blob["q12_recovered"] is True
        assert blob["unfloored_actions"] == []
        assert torch.count_nonzero(policy.v.weight) == 0
        assert torch.count_nonzero(policy.v.bias) == 0

        roundtrip = tmp_path / "roundtrip.json"
        export(str(out), str(roundtrip))
        assert json.loads(roundtrip.read_text()) == json.loads(weights.read_text())

    def test_exact_recovery_preserves_a_valid_source_lineage(
        self, tmp_path: pathlib.Path
    ) -> None:
        weights = _bridge_artifact(tmp_path, lineaged=True)
        source = json.loads(weights.read_text())
        out = tmp_path / "recovered.pt"

        dequantize.dequantize(weights, out)

        _policy, blob = load_policy(str(out))
        assert blob["lineage"] == source["lineage"]

    def test_a_deep_v7_artifact_is_recoverable(self, tmp_path: pathlib.Path) -> None:
        torch.manual_seed(12)
        source = make_policy("deep")
        ckpt = tmp_path / "deep.pt"
        save_policy(
            source,
            "deep",
            ckpt,
            {"gym_version": GYM_VERSION, "update": 18},
        )
        weights = tmp_path / "deep.json"
        export(str(ckpt), str(weights))
        out = tmp_path / "ladder.pt"
        dequantize.dequantize(weights, out)
        policy, blob = load_policy(str(out))
        assert blob["arch"] == "deep"
        assert torch.count_nonzero(policy.v.weight) == 0
        assert torch.count_nonzero(policy.v.bias) == 0


class TestUnflooring:
    def test_only_the_requested_bridge_rows_become_reachable(
        self, tmp_path: pathlib.Path
    ) -> None:
        weights = _bridge_artifact(tmp_path)
        out = tmp_path / "trainable.pt"
        action = ACTIONS - 2
        dequantize.dequantize(weights, out, (action,))

        policy, blob = load_policy(str(out))
        assert blob["unfloored_actions"] == [action]
        assert torch.count_nonzero(policy.pi.weight[action]) == 0
        assert policy.pi.bias[action] == 0
        assert policy.pi.bias[ACTIONS - 1] == -8

    def test_unflooring_derives_a_new_lineage_from_the_exact_source(
        self, tmp_path: pathlib.Path
    ) -> None:
        weights = _bridge_artifact(tmp_path, lineaged=True)
        source = json.loads(weights.read_text())
        out = tmp_path / "trainable.pt"

        dequantize.dequantize(weights, out, (ACTIONS - 2,))

        _policy, blob = load_policy(str(out))
        lineage = blob["lineage"]
        assert lineage["phase"] == "q12-unfloor"
        assert lineage["lineage_id"] != source["lineage"]["lineage_id"]
        assert (
            lineage["inputs"]["source"]["lineage_id"] == source["lineage"]["lineage_id"]
        )

    def test_a_trained_row_cannot_be_erased_under_the_name_unfloor(
        self, tmp_path: pathlib.Path
    ) -> None:
        weights = _bridge_artifact(tmp_path)
        with pytest.raises(ValueError, match="action 0 is not"):
            dequantize.dequantize(weights, tmp_path / "bad.pt", (0,))

    def test_the_cli_list_is_bounded_and_duplicate_free(self) -> None:
        assert dequantize.parse_action_indices("24, 25") == (24, 25)
        with pytest.raises(argparse.ArgumentTypeError, match="must not repeat"):
            dequantize.parse_action_indices("24,24")
        with pytest.raises(argparse.ArgumentTypeError, match="must be in"):
            dequantize.parse_action_indices(str(ACTIONS))
