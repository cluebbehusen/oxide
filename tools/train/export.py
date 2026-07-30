"""Exports a checkpoint as integer weights for the sim's inference.

The sim may not touch floats (determinism is clippy-enforced), so the
shipped policy is a fixed-point artifact: Q12 weights, a Q12 tanh
lookup table, and per-feature reciprocal scales — everything the Rust
side needs to reproduce this network with integer ops only. The
quantized bot is the shipped artifact; it re-runs the tournament after
export, because 12 bits of mantissa is a (slightly) different player.

Usage (from tools/train/):
    uv run export.py --ckpt runs/league4w/latest.pt --out runs/prime.json
"""

import argparse
import json

import numpy as np
import torch

from lineage import export_lineage
from models import Mlp, load_policy
from oxide_gym import ACTIONS, CONDITION_DIMS, FEATURES, GYM_VERSION, SCALES

Q = 12  # fractional bits

# The loader's numeric contract (sim/src/bot/neural.rs): the magnitude
# ceilings that let the sim's i64 kernel stay total. An artifact that
# breaks one is refused at load, so catch it here, where the offending
# checkpoint is still in hand.
MAX_RECIP = 1 << 26
MAX_LUT = 1 << 13
MAX_COEFF = 1 << 20
MAX_LAYERS = 16
MAX_WIDTH = 4096


def quant(t: torch.Tensor) -> list:
    return (t.detach().numpy() * (1 << Q)).round().astype(int).tolist()


def build_artifact(policy: Mlp, blob: dict) -> dict:
    """Builds the Q12 artifact payload for an already-loaded policy.

    Keeping construction separate from file I/O gives the inverse
    conversion a single authoritative round-trip check: a recovered
    actor is accepted only when this function reproduces every
    semantic field of its source artifact.
    """
    policy.eval()

    linears = [m for m in policy.trunk.modules() if isinstance(m, torch.nn.Linear)]
    layers = [{"w": quant(lin.weight), "b": quant(lin.bias)} for lin in linears]
    head = {"w": quant(policy.pi.weight), "b": quant(policy.pi.bias)}

    # tanh as a 513-entry Q12 table over [-8, 8]; Rust interpolates
    # linearly between entries with integer math.
    xs = np.linspace(-8.0, 8.0, 513)
    lut = (np.tanh(xs) * (1 << Q)).round().astype(int).tolist()

    # feature -> Q12 normalization: (feature * recip) >> Q with
    # recip = round(2^(2Q) / scale). All seven conditioning knobs ride
    # at the end with scale 1000, matching
    # oxide_gym.with_condition.
    recips = [round((1 << (2 * Q)) / float(s)) for s in SCALES]
    recips += [round((1 << (2 * Q)) / 1000.0)] * CONDITION_DIMS

    # Shape tripwires: a checkpoint from another contract must fail
    # here, not produce an artifact Rust rejects (or worse, accepts).
    net_inputs = FEATURES + CONDITION_DIMS
    if layers[0]["w"] and len(layers[0]["w"][0]) != net_inputs:
        got = len(layers[0]["w"][0])
        raise SystemExit(f"first layer reads {got} inputs, contract wants {net_inputs}")
    if len(head["w"]) != ACTIONS:
        got = len(head["w"])
        raise SystemExit(f"head emits {got} logits, contract wants {ACTIONS}")

    # Magnitude tripwires: the same ceilings the loader enforces.
    if not all(1 <= r <= MAX_RECIP for r in recips):
        raise SystemExit(f"a recip is outside 1..={MAX_RECIP} — check SCALES")
    if max(abs(v) for v in lut) > MAX_LUT:
        raise SystemExit(f"the tanh table exceeds +/-{MAX_LUT}")
    if len(layers) > MAX_LAYERS:
        raise SystemExit(f"{len(layers)} trunk layers, over the {MAX_LAYERS} ceiling")
    named = [(f"layer {i}", lay) for i, lay in enumerate(layers)] + [("head", head)]
    for name, lay in named:
        if len(lay["w"]) > MAX_WIDTH:
            raise SystemExit(f"{name} is {len(lay['w'])} wide, over {MAX_WIDTH}")
        peak = max(max(abs(v) for v in row) for row in [*lay["w"], lay["b"]] if row)
        if peak > MAX_COEFF:
            raise SystemExit(f"{name} peaks at {peak}, over the +/-{MAX_COEFF} ceiling")

    artifact = {
        "gym_version": GYM_VERSION,
        "arch": blob.get("arch"),
        "update": blob.get("update"),
        "q_bits": Q,
        "features": FEATURES,
        "conditioning": CONDITION_DIMS,
        "actions": ACTIONS,
        "recips": recips,
        "tanh_lut": lut,
        "layers": layers,
        "head": head,
    }
    lineage = export_lineage(blob)
    if lineage is not None:
        artifact["lineage"] = lineage
    return artifact


def export(ckpt: str, out: str) -> tuple[int, str | None]:
    """Quantizes `ckpt` to the Q12 artifact at `out`; returns the
    parameter count and recorded arch. Importable so league.py's
    in-loop composition probe can export a snapshot without a
    subprocess."""
    policy, blob = load_policy(ckpt)
    artifact = build_artifact(policy, blob)
    with open(out, "w") as f:
        json.dump(artifact, f)
    layers = artifact["layers"]
    head = artifact["head"]
    n = sum(len(lay["w"]) * len(lay["w"][0]) + len(lay["b"]) for lay in layers)
    n += len(head["w"]) * len(head["w"][0]) + len(head["b"])
    return n, blob.get("arch")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--ckpt", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    n, arch = export(args.ckpt, args.out)
    print(f"exported {n} params (arch {arch}) to {args.out}")


if __name__ == "__main__":
    main()
