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

from models import load_policy
from oxide_gym import ACTIONS, CONDITION_DIMS, FEATURES, GYM_VERSION, SCALES

Q = 12  # fractional bits


def quant(t: torch.Tensor) -> list:
    return (t.detach().numpy() * (1 << Q)).round().astype(int).tolist()


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--ckpt", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    policy, blob = load_policy(args.ckpt)
    policy.eval()

    linears = [m for m in policy.trunk.modules() if isinstance(m, torch.nn.Linear)]
    layers = [{"w": quant(lin.weight), "b": quant(lin.bias)} for lin in linears]
    head = {"w": quant(policy.pi.weight), "b": quant(policy.pi.bias)}

    # tanh as a 513-entry Q12 table over [-8, 8]; Rust interpolates
    # linearly between entries with integer math.
    xs = np.linspace(-8.0, 8.0, 513)
    lut = (np.tanh(xs) * (1 << Q)).round().astype(int).tolist()

    # feature -> Q12 normalization: (feature * recip) >> Q with
    # recip = round(2^(2Q) / scale). Conditioning knobs (skill,
    # aggression) ride at the end with scale 1000, matching
    # oxide_gym.with_condition.
    recips = [round((1 << (2 * Q)) / float(s)) for s in SCALES]
    recips += [round((1 << (2 * Q)) / 1000.0)] * CONDITION_DIMS

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
    with open(args.out, "w") as f:
        json.dump(artifact, f)
    n = sum(len(lay["w"]) * len(lay["w"][0]) + len(lay["b"]) for lay in layers)
    n += len(head["w"]) * len(head["w"][0]) + len(head["b"])
    print(f"exported {n} params (arch {blob.get('arch')}) to {args.out}")


if __name__ == "__main__":
    main()
