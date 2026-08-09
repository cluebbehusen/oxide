"""Round-trips a random float net through export.py and reproduces the
sim's integer forward pass from the artifact, asserting the two agree.

This is the property the Rust ladder depends on: the shipped Q12 artifact
must play the same game the trained float net does. The integer forward
below mirrors ``sim/src/bot/neural.rs`` (arithmetic right shifts, the
513-entry tanh LUT, per-feature reciprocal scaling) — the *consumer's*
math, deliberately re-derived rather than borrowed from export.py."""

import json
import sys
from typing import TYPE_CHECKING

import numpy as np
import torch

import export
from lineage import build_lineage, checkpoint_metadata
from models import factorized_greedy, load_policy, make_policy, save_policy
from oxide_gym import (
    ACTIONS,
    CONDITION_DIMS,
    FEATURES,
    SCALES,
    normalize,
    with_condition,
)

if TYPE_CHECKING:
    from pathlib import Path

    import pytest

Q = 12
TANH_SPAN = 8 << Q  # the LUT spans [-8, 8] in Q12


def _tanh(lut: list[int], x: int) -> int:
    x = max(-TANH_SPAN, min(TANH_SPAN, x))
    pos = x + TANH_SPAN
    idx = pos >> 7
    frac = pos & 127
    a = lut[min(idx, 512)]
    b = lut[min(idx + 1, 512)]
    return a + (((b - a) * frac) >> 7)


def _affine(w: list[list[int]], b: list[int], inp: list[int]) -> list[int]:
    out = []
    for row, bias in zip(w, b, strict=True):
        acc = sum(wi * xi for wi, xi in zip(row, inp, strict=True))
        # arithmetic right shift == floor-divide by 2^Q, matching i64 >>.
        out.append((acc >> Q) + bias)
    return out


def _int_logits(art: dict, features: list[int], knobs: list[int]) -> list[int]:
    combined = list(features) + list(knobs)[: art["conditioning"]]
    act = [(f * r) >> Q for f, r in zip(combined, art["recips"], strict=True)]
    for layer in art["layers"]:
        act = [_tanh(art["tanh_lut"], v) for v in _affine(layer["w"], layer["b"], act)]
    return _affine(art["head"]["w"], art["head"]["b"], act)


class TestMain:
    def test_the_q12_artifact_reproduces_the_float_forward(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        torch.manual_seed(123)
        policy = make_policy("mlp")
        with torch.no_grad():
            # Give each head a stable winner so the parity check covers
            # factorized selection as well as raw logit fidelity.
            policy.pi.bias[8] += 2.0
            policy.pi.bias[23] += 2.5
            policy.pi.bias[20] += 3.0
        policy.eval()
        ckpt = tmp_path / "net.pt"
        save_policy(policy, "mlp", ckpt)
        out = tmp_path / "artifact.json"

        # Round-trip through export.main() itself (argparse CLI), so the test
        # exercises the real quantizer, not a re-implementation of it.
        monkeypatch.setattr(
            sys, "argv", ["export.py", "--ckpt", str(ckpt), "--out", str(out)]
        )
        export.main()
        art = json.loads(out.read_text())
        assert art["features"] == 81
        assert art["conditioning"] == 12
        assert art["actions"] == 26
        assert len(art["head"]["w"]) == ACTIONS

        rng = np.random.default_rng(0)
        max_err = 0.0
        for _ in range(64):
            # normalize() maps each feature to roughly [-1, 1] by its scale;
            # draw integer features that land in that operating band (a value
            # near its own scale) rather than uniformly, so the fidelity check
            # covers the inputs the trained net actually sees.
            features = np.rint(rng.random(FEATURES) * SCALES).astype(int).tolist()
            knobs = rng.integers(0, 1001, size=CONDITION_DIMS).tolist()

            obs = with_condition(normalize(features), tuple(knobs))
            mask = torch.ones(1, ACTIONS, dtype=torch.bool)
            with torch.no_grad():
                logits_f, _ = policy(torch.as_tensor(obs[None]).float(), mask)
            float_logits = logits_f[0].numpy()

            int_logits = np.asarray(_int_logits(art, features, knobs)) / (1 << Q)
            max_err = max(max_err, float(np.max(np.abs(int_logits - float_logits))))
            float_plan = factorized_greedy(logits_f)[0]
            int_plan = factorized_greedy(
                torch.as_tensor(int_logits[None], dtype=torch.float32)
            )[0]
            assert torch.equal(int_plan, float_plan)

        # Q12 resolution is 2^-12 ~= 2.4e-4; two 128-wide tanh layers plus the
        # input requantization accumulate a few multiples of that per logit.
        # A gap this small is quantization noise; anything larger is a broken
        # forward (wrong shift, transposed weights, mis-scaled input).
        assert max_err < 5e-3

    def test_export_propagates_lineage_without_changing_semantic_weights(
        self, tmp_path: Path
    ) -> None:
        torch.manual_seed(7)
        policy = make_policy("mlp")
        lineage = build_lineage(
            phase="league",
            phase_start_update=75,
            hyperparameters={"updates": 30, "lr": 1e-4},
        )
        plain_ckpt = tmp_path / "plain.pt"
        save_policy(
            policy,
            "mlp",
            plain_ckpt,
            {"gym_version": 8, "update": 105},
        )
        traced_ckpt = tmp_path / "traced.pt"
        save_policy(
            policy,
            "mlp",
            traced_ckpt,
            checkpoint_metadata(
                lineage,
                {"gym_version": 8, "update": 105},
            ),
        )
        _reloaded, blob = load_policy(str(traced_ckpt))
        assert blob["lineage"] == lineage

        plain_out = tmp_path / "plain.json"
        traced_out = tmp_path / "traced.json"
        export.export(str(plain_ckpt), str(plain_out))
        export.export(str(traced_ckpt), str(traced_out))
        plain = json.loads(plain_out.read_text())
        traced = json.loads(traced_out.read_text())
        assert traced["lineage"] == lineage
        assert {
            key: value for key, value in traced.items() if key != "lineage"
        } == plain
