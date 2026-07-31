"""Widen a v7 actor into the behavior-identical gym-v8 profile contract.

V8 appends five Rust-authored named-profile facets after the existing policy
condition. Every new first-layer column is exactly zero, so the widened actor's
logits and factorized action plan are identical for every possible facet value.
Only later continuation training can make the new inputs matter.

Usage:
    uv run widen.py --src runs/incumbent-v7.json \
        --out runs/incumbent-v8.json
"""

import argparse
import json
import pathlib

import torch

from lineage import build_lineage, input_identity

# The v8 contract this bridge widens TO. Kept as explicit literals so
# a rerun against the wrong source fails loudly instead of stacking.
SRC_VERSION = 7
SRC_FEATURES = 81
SRC_ACTIONS = 26
SRC_CONDITIONING = 7
DST_VERSION = 8
DST_FEATURES = 81
DST_ACTIONS = 26
DST_CONDITIONING = 12
CONDITION_SCALE = 1_000


def widening_lineage(src: str, metadata: dict) -> dict[str, object]:
    """Builds provenance for the behavior-preserving contract bridge."""
    return build_lineage(
        phase="contract-widen-v7-v8",
        phase_start_update=int(metadata.get("update", 0) or 0),
        hyperparameters={
            "dst_actions": DST_ACTIONS,
            "dst_conditioning": DST_CONDITIONING,
            "dst_features": DST_FEATURES,
            "dst_gym_version": DST_VERSION,
            "new_condition_scale": CONDITION_SCALE,
            "new_columns": DST_CONDITIONING - SRC_CONDITIONING,
            "src_actions": SRC_ACTIONS,
            "src_conditioning": SRC_CONDITIONING,
            "src_features": SRC_FEATURES,
            "src_gym_version": SRC_VERSION,
        },
        inputs={
            "source": input_identity(src, metadata),
            "transformer_code": input_identity(pathlib.Path(__file__)),
        },
    )


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument(
        "--ckpt",
        action="store_true",
        help="widen a float .pt checkpoint instead of a quantized .json artifact",
    )
    args = ap.parse_args()

    if args.ckpt:
        widen_ckpt(args.src, args.out)
        return

    with open(args.src) as f:
        art = json.load(f)
    lineage = widening_lineage(args.src, art)
    if art["gym_version"] != SRC_VERSION:
        raise SystemExit(
            f"source speaks gym v{art['gym_version']}, widen expects v{SRC_VERSION}"
        )
    if art["features"] != SRC_FEATURES or art["actions"] != SRC_ACTIONS:
        raise SystemExit("source shape mismatch")
    q = art["q_bits"]
    cond = art["conditioning"]
    if cond != SRC_CONDITIONING:
        raise SystemExit(
            f"source carries {cond} conditions, widen expects {SRC_CONDITIONING}"
        )
    if len(art["recips"]) != SRC_FEATURES + cond:
        raise SystemExit("recips do not cover the source inputs")

    if any(len(row) != SRC_FEATURES + cond for row in art["layers"][0]["w"]):
        raise SystemExit("source layer-0 width does not match its contract")

    # Five named-profile facets follow the old conditions. Their zero
    # columns are the behavior-preservation proof; the reciprocal is still
    # useful because the first continuation resumes from this artifact.
    for _ in range(DST_CONDITIONING - SRC_CONDITIONING):
        art["recips"].append(round((1 << (2 * q)) / CONDITION_SCALE))
        for row in art["layers"][0]["w"]:
            row.append(0)

    art["gym_version"] = DST_VERSION
    art["features"] = DST_FEATURES
    art["actions"] = DST_ACTIONS
    art["conditioning"] = DST_CONDITIONING
    art["lineage"] = lineage

    with open(args.out, "w") as f:
        json.dump(art, f)
        f.write("\n")
    print(f"widened v{SRC_VERSION} -> v{DST_VERSION}: {args.out}")


def widen_ckpt(src: str, out: str) -> None:
    """Widens a v7 float checkpoint with five zero profile columns."""
    blob = torch.load(src, map_location="cpu", weights_only=True)
    if not (isinstance(blob, dict) and "state" in blob):
        raise SystemExit("expected a save_policy blob with arch + state")
    lineage = widening_lineage(src, blob)
    recorded = blob.get("gym_version")
    if recorded is not None and recorded != SRC_VERSION:
        raise SystemExit(
            f"checkpoint speaks gym v{recorded}, widen expects v{SRC_VERSION}"
        )
    state = blob["state"]
    first = "trunk.0.weight"
    w = state[first]
    if w.shape[1] != SRC_FEATURES + SRC_CONDITIONING:
        raise SystemExit(
            "first layer reads "
            f"{w.shape[1]} inputs, expected {SRC_FEATURES + SRC_CONDITIONING}"
        )
    extra_conditions = DST_CONDITIONING - SRC_CONDITIONING
    w = torch.cat(
        [w, torch.zeros(w.shape[0], extra_conditions, dtype=w.dtype)],
        dim=1,
    )
    state[first] = w
    if state["pi.weight"].shape[0] != SRC_ACTIONS:
        got = state["pi.weight"].shape[0]
        raise SystemExit(f"head emits {got} logits, expected {SRC_ACTIONS}")
    blob["gym_version"] = DST_VERSION
    blob["lineage"] = lineage
    torch.save(blob, out)
    print(f"widened checkpoint v{SRC_VERSION} -> v{DST_VERSION}: {out}")


if __name__ == "__main__":
    main()
