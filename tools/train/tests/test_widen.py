"""Tests for ``widen``: both bridges must change shape without changing
behavior — the json artifact gets an UNREACHABLE new action (fixtures
stay green), the float checkpoint a REACHABLE one (PPO can explore)."""

import json
from typing import TYPE_CHECKING

import torch

import widen
from models import make_policy, save_policy

if TYPE_CHECKING:
    import pathlib

    import pytest


def _fake_artifact(tmp_path: pathlib.Path) -> pathlib.Path:
    hidden = 2
    src = {
        "gym_version": widen.SRC_VERSION,
        "arch": "tiny",
        "update": 1,
        "q_bits": 12,
        "features": widen.SRC_FEATURES,
        "conditioning": 3,
        "actions": widen.SRC_ACTIONS,
        "recips": [7] * (widen.SRC_FEATURES + 3),
        "tanh_lut": [0] * 513,
        "layers": [
            {"w": [[1] * (widen.SRC_FEATURES + 3)] * hidden, "b": [0] * hidden},
        ],
        "head": {"w": [[2] * hidden] * widen.SRC_ACTIONS, "b": [0] * widen.SRC_ACTIONS},
    }
    path = tmp_path / "src.json"
    path.write_text(json.dumps(src))
    return path


class TestArtifactBridge:
    def test_new_columns_are_zero_and_new_actions_floored(
        self, tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        src = _fake_artifact(tmp_path)
        out = tmp_path / "out.json"
        monkeypatch.setattr(
            "sys.argv", ["widen.py", "--src", str(src), "--out", str(out)]
        )
        widen.main()
        art = json.loads(out.read_text())
        assert art["gym_version"] == widen.DST_VERSION
        assert art["features"] == widen.DST_FEATURES
        assert art["actions"] == widen.DST_ACTIONS
        # The inserted feature column reads zero in every first-layer row.
        for row in art["layers"][0]["w"]:
            assert len(row) == widen.DST_FEATURES + 3
            for idx in widen.NEW_FEATURE_SCALES:
                assert row[idx] == 0
        # The appended action can never win an argmax.
        assert len(art["head"]["w"]) == widen.DST_ACTIONS
        floor = -(8 << art["q_bits"])
        for bias in art["head"]["b"][widen.SRC_ACTIONS :]:
            assert bias == floor
        # Recips grew in step with the inputs.
        assert len(art["recips"]) == widen.DST_FEATURES + 3

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


class TestCheckpointBridge:
    def test_the_float_resume_gets_a_reachable_verb(
        self, tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        # A v4-shaped policy checkpoint (the old contract's tensor
        # shapes), stamped v4.
        monkeypatch.setattr("oxide_gym.FEATURES", widen.SRC_FEATURES)
        monkeypatch.setattr("oxide_gym.ACTIONS", widen.SRC_ACTIONS)
        monkeypatch.setattr(
            "oxide_gym.NET_FEATURES", widen.SRC_FEATURES + 3, raising=True
        )
        monkeypatch.setattr("models.ACTIONS", widen.SRC_ACTIONS)
        monkeypatch.setattr("models.NET_FEATURES", widen.SRC_FEATURES + 3)
        policy = make_policy("mlp")
        src = tmp_path / "src.pt"
        save_policy(policy, "mlp", src, {"gym_version": widen.SRC_VERSION, "update": 9})
        out = tmp_path / "out.pt"
        widen.widen_ckpt(str(src), str(out))
        blob = torch.load(out, weights_only=True)
        assert blob["gym_version"] == widen.DST_VERSION
        state = blob["state"]
        assert state["trunk.0.weight"].shape[1] == widen.DST_FEATURES + 3
        assert state["pi.weight"].shape[0] == widen.DST_ACTIONS
        # Reachable: zero bias and zero weights — a logit of 0, not a
        # floor, so sampling can explore the verb.
        for idx in widen.NEW_FEATURE_SCALES:
            assert torch.all(state["trunk.0.weight"][:, idx] == 0)
        assert torch.all(state["pi.bias"][widen.SRC_ACTIONS :] == 0)
        assert torch.all(state["pi.weight"][widen.SRC_ACTIONS :] == 0)
