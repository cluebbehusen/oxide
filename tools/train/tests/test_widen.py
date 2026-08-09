"""Tests for the behavior-identical v7 -> v8 profile bridge."""

import json
from typing import TYPE_CHECKING

import pytest
import torch

import widen
from lineage import content_digest, validate_lineage
from models import load_policy, make_policy, save_policy

if TYPE_CHECKING:
    import pathlib


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
    def test_new_profile_columns_are_zero_and_every_old_byte_is_preserved(
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
        assert lineage["phase"] == "contract-widen-v7-v8"
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
        assert art["features"] == source["features"]
        assert art["actions"] == source["actions"]
        assert art["head"] == source["head"]
        assert art["tanh_lut"] == source["tanh_lut"]
        # Only five zero profile columns are appended to the first layer.
        for row in art["layers"][0]["w"]:
            assert len(row) == widen.DST_FEATURES + widen.DST_CONDITIONING
            assert row[-5:] == [0, 0, 0, 0, 0]
        for source_row, widened_row in zip(
            source["layers"][0]["w"],
            art["layers"][0]["w"],
            strict=True,
        ):
            assert widened_row[: len(source_row)] == source_row
        # Recips grew in step with the inputs.
        assert len(art["recips"]) == widen.DST_FEATURES + widen.DST_CONDITIONING
        assert art["recips"][: len(source["recips"])] == source["recips"]
        assert art["recips"][-5:] == [round((1 << 24) / 1_000)] * 5

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
    def test_v7_checkpoint_rejection_names_the_ckpt_migration(
        self, tmp_path: pathlib.Path
    ) -> None:
        src = tmp_path / "old.pt"
        torch.save({"arch": "mlp", "gym_version": 7, "state": {}}, src)

        with pytest.raises(RuntimeError, match=r"widen\.py --ckpt --src OLD\.pt"):
            load_policy(str(src))

    def test_the_float_resume_gets_only_zero_profile_columns(
        self, tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        # A v7-shaped policy checkpoint, stamped v7.
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
        source_state = {
            name: tensor.clone() for name, tensor in policy.state_dict().items()
        }
        source_first = source_state["trunk.0.weight"]
        src = tmp_path / "src.pt"
        save_policy(policy, "mlp", src, {"gym_version": widen.SRC_VERSION, "update": 9})
        out = tmp_path / "out.pt"
        widen.widen_ckpt(str(src), str(out))
        blob = torch.load(out, weights_only=True)
        lineage = validate_lineage(blob["lineage"])
        assert lineage["phase"] == "contract-widen-v7-v8"
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
        assert state.keys() == source_state.keys()
        torch.testing.assert_close(
            state["trunk.0.weight"][:, : source_first.shape[1]],
            source_first,
            rtol=0,
            atol=0,
        )
        profile_columns = state["trunk.0.weight"][:, source_first.shape[1] :]
        assert profile_columns.shape == (
            source_first.shape[0],
            widen.DST_CONDITIONING - widen.SRC_CONDITIONING,
        )
        torch.testing.assert_close(
            profile_columns,
            torch.zeros_like(profile_columns),
            rtol=0,
            atol=0,
        )
        for name, source_tensor in source_state.items():
            if name == "trunk.0.weight":
                continue
            torch.testing.assert_close(state[name], source_tensor, rtol=0, atol=0)
