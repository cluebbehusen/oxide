"""Tests for the v6 -> v7 factorized-policy bridge."""

import json
from typing import TYPE_CHECKING

import torch

import widen
from lineage import content_digest, validate_lineage
from models import make_policy, save_policy

if TYPE_CHECKING:
    import pathlib

    import pytest


def _fake_artifact(tmp_path: pathlib.Path) -> pathlib.Path:
    hidden = 2
    width = widen.SRC_FEATURES + widen.SRC_CONDITIONING
    src = {
        "gym_version": widen.SRC_VERSION,
        "arch": "tiny",
        "update": 1,
        "q_bits": 12,
        "features": widen.SRC_FEATURES,
        "conditioning": widen.SRC_CONDITIONING,
        "actions": widen.SRC_ACTIONS,
        "recips": [1_000 + index for index in range(width)],
        "tanh_lut": [0] * 513,
        "layers": [
            {
                "w": [
                    [row * width + index + 1 for index in range(width)]
                    for row in range(hidden)
                ],
                "b": [0] * hidden,
            },
        ],
        "head": {"w": [[2] * hidden] * widen.SRC_ACTIONS, "b": [0] * widen.SRC_ACTIONS},
    }
    path = tmp_path / "src.json"
    path.write_text(json.dumps(src))
    return path


class TestArtifactBridge:
    def test_new_columns_are_zero_and_new_heads_copy_idle(
        self, tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        src = _fake_artifact(tmp_path)
        source = json.loads(src.read_text())
        out = tmp_path / "out.json"
        monkeypatch.setattr(
            "sys.argv", ["widen.py", "--src", str(src), "--out", str(out)]
        )
        widen.main()
        art = json.loads(out.read_text())
        lineage = validate_lineage(art["lineage"])
        assert lineage["phase"] == "contract-widen-v6-v7"
        assert lineage["inputs"] == {
            "source": {"content_sha256": content_digest(src)},
            "transformer_code": {
                "content_sha256": content_digest(widen.__file__),
            },
        }
        assert art["gym_version"] == widen.DST_VERSION
        assert art["features"] == widen.DST_FEATURES
        assert art["actions"] == widen.DST_ACTIONS
        assert art["conditioning"] == widen.DST_CONDITIONING
        # The inserted feature column reads zero in every first-layer row.
        for row in art["layers"][0]["w"]:
            assert len(row) == widen.DST_FEATURES + widen.DST_CONDITIONING
            for idx in widen.NEW_FEATURE_SCALES:
                assert row[idx] == 0
            assert row[-4:] == [0, 0, 0, 0]
        for source_row, widened_row in zip(
            source["layers"][0]["w"],
            art["layers"][0]["w"],
            strict=True,
        ):
            assert widened_row[: widen.SRC_FEATURES] == source_row[: widen.SRC_FEATURES]
            assert (
                widened_row[
                    widen.DST_FEATURES : widen.DST_FEATURES + widen.SRC_CONDITIONING
                ]
                == source_row[widen.SRC_FEATURES :]
            )
        # Each appended row is a head-specific no-op initialized from
        # the actor's old Idle row.
        assert len(art["head"]["w"]) == widen.DST_ACTIONS
        for row in art["head"]["w"][widen.SRC_ACTIONS :]:
            assert row == art["head"]["w"][0]
        for bias in art["head"]["b"][widen.SRC_ACTIONS :]:
            assert bias == art["head"]["b"][0]
        # Recips grew in step with the inputs.
        assert len(art["recips"]) == widen.DST_FEATURES + widen.DST_CONDITIONING
        for idx, scale in widen.NEW_FEATURE_SCALES.items():
            assert art["recips"][idx] == round((1 << 24) / scale)
        assert (
            art["recips"][
                widen.DST_FEATURES : widen.DST_FEATURES + widen.SRC_CONDITIONING
            ]
            == source["recips"][widen.SRC_FEATURES :]
        )
        assert art["recips"][-4:] == [round((1 << 24) / 1_000)] * 4

    def test_a_wrong_version_refuses_instead_of_stacking(
        self, tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        src = _fake_artifact(tmp_path)
        doctored = json.loads(src.read_text())
        doctored["gym_version"] = widen.DST_VERSION
        src.write_text(json.dumps(doctored))
        out = tmp_path / "out.json"
        monkeypatch.setattr(
            "sys.argv", ["widen.py", "--src", str(src), "--out", str(out)]
        )
        try:
            widen.main()
        except SystemExit:
            return
        raise AssertionError("re-widening an already-widened artifact must refuse")

    def test_transformer_changes_produce_different_lineage_ids(
        self, tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        src = _fake_artifact(tmp_path)
        metadata = json.loads(src.read_text())
        transformer = tmp_path / "widen.py"
        transformer.write_text("first implementation")
        monkeypatch.setattr(widen, "__file__", str(transformer))
        first = widen.widening_lineage(str(src), metadata)

        transformer.write_text("second implementation")
        second = widen.widening_lineage(str(src), metadata)

        assert first["lineage_id"] != second["lineage_id"]


class TestCheckpointBridge:
    def test_the_float_resume_gets_head_specific_noops(
        self, tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        # A v6-shaped policy checkpoint, stamped v6.
        monkeypatch.setattr("oxide_gym.FEATURES", widen.SRC_FEATURES)
        monkeypatch.setattr("oxide_gym.ACTIONS", widen.SRC_ACTIONS)
        monkeypatch.setattr(
            "oxide_gym.NET_FEATURES",
            widen.SRC_FEATURES + widen.SRC_CONDITIONING,
            raising=True,
        )
        monkeypatch.setattr("models.ACTIONS", widen.SRC_ACTIONS)
        monkeypatch.setattr(
            "models.NET_FEATURES", widen.SRC_FEATURES + widen.SRC_CONDITIONING
        )
        policy = make_policy("mlp")
        source_first = policy.state_dict()["trunk.0.weight"].clone()
        source_pi_weight = policy.state_dict()["pi.weight"].clone()
        source_pi_bias = policy.state_dict()["pi.bias"].clone()
        src = tmp_path / "src.pt"
        save_policy(policy, "mlp", src, {"gym_version": widen.SRC_VERSION, "update": 9})
        out = tmp_path / "out.pt"
        widen.widen_ckpt(str(src), str(out))
        blob = torch.load(out, weights_only=True)
        lineage = validate_lineage(blob["lineage"])
        assert lineage["phase"] == "contract-widen-v6-v7"
        assert lineage["inputs"] == {
            "source": {"content_sha256": content_digest(src)},
            "transformer_code": {
                "content_sha256": content_digest(widen.__file__),
            },
        }
        assert blob["gym_version"] == widen.DST_VERSION
        state = blob["state"]
        assert (
            state["trunk.0.weight"].shape[1]
            == widen.DST_FEATURES + widen.DST_CONDITIONING
        )
        assert state["pi.weight"].shape[0] == widen.DST_ACTIONS
        for idx in widen.NEW_FEATURE_SCALES:
            assert torch.all(state["trunk.0.weight"][:, idx] == 0)
        assert torch.all(state["trunk.0.weight"][:, -4:] == 0)
        assert torch.equal(
            state["trunk.0.weight"][:, : widen.SRC_FEATURES],
            source_first[:, : widen.SRC_FEATURES],
        )
        assert torch.equal(
            state["trunk.0.weight"][
                :, widen.DST_FEATURES : widen.DST_FEATURES + widen.SRC_CONDITIONING
            ],
            source_first[:, widen.SRC_FEATURES :],
        )
        assert torch.equal(
            state["pi.weight"][: widen.SRC_ACTIONS],
            source_pi_weight,
        )
        assert torch.equal(state["pi.bias"][: widen.SRC_ACTIONS], source_pi_bias)
        for action in range(widen.SRC_ACTIONS, widen.DST_ACTIONS):
            assert torch.equal(state["pi.bias"][action], state["pi.bias"][0])
            assert torch.equal(state["pi.weight"][action], state["pi.weight"][0])
