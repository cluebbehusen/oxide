"""Widen a quantized artifact to a new gym contract, changing nothing
about its behavior.

The broken-window bridge: when GYM_VERSION grows new features or
actions, the shipped artifact must speak the new shape in the same
commit as the contract bump — but play identically, so the ladder
tests, hash fixtures, and smoke stay green for the whole campaign. New
feature columns enter with ZERO weights (the input is read and
ignored), and new action rows enter with zero weights and a bias of
-(8 << q_bits), a floor no real logit visits, so the argmax can never
pick them and the blunder picker never sees them win.

The v5 -> v6 hop: one feature (damaged_unit_value, appended at 64)
and two actions (RepairUnit and Build RepairBay, indices 22 and 23).

Usage:
    uv run widen.py --src ../../sim/src/bot/ladder_weights.json \
        --out ../../sim/src/bot/ladder_weights.json
"""

import argparse
import json

import torch

# The v6 contract this bridge widens TO. Kept as explicit literals so
# a rerun against the wrong source fails loudly instead of stacking.
SRC_VERSION = 5
SRC_FEATURES = 64
SRC_ACTIONS = 22
DST_VERSION = 6
DST_FEATURES = 65
DST_ACTIONS = 24
# Appended features, index -> normalization scale (mirrors
# SCALE_BY_NAME in oxide_gym.py; the recip formula is 2^(2q)/scale).
NEW_FEATURE_SCALES = {64: 500}  # damaged_unit_value


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument(
        "--ckpt",
        action="store_true",
        help="widen a float .pt checkpoint (explorable new action) "
        "instead of a quantized .json artifact (unreachable new action)",
    )
    args = ap.parse_args()

    if args.ckpt:
        widen_ckpt(args.src, args.out)
        return

    with open(args.src) as f:
        art = json.load(f)
    if art["gym_version"] != SRC_VERSION:
        raise SystemExit(
            f"source speaks gym v{art['gym_version']}, widen expects v{SRC_VERSION}"
        )
    if art["features"] != SRC_FEATURES or art["actions"] != SRC_ACTIONS:
        raise SystemExit("source shape mismatch")
    q = art["q_bits"]
    cond = art["conditioning"]
    if len(art["recips"]) != SRC_FEATURES + cond:
        raise SystemExit("recips do not cover the source inputs")

    # New feature columns: zero weight in the first layer, a sane recip
    # (value irrelevant behind zero weights, but a later fine-tune
    # resumes from here and inherits the normalization).
    for inserted, idx in enumerate(sorted(NEW_FEATURE_SCALES)):
        recip = (1 << (2 * q)) // NEW_FEATURE_SCALES[idx]
        art["recips"].insert(idx, recip)
        for row in art["layers"][0]["w"]:
            if len(row) != SRC_FEATURES + cond + inserted:
                raise SystemExit("layer-0 width drifted mid-insert")
            row.insert(idx, 0)

    # New action rows: unreachable by construction.
    width = len(art["head"]["w"][0])
    floor = -(8 << q)
    for _ in range(DST_ACTIONS - SRC_ACTIONS):
        art["head"]["w"].append([0] * width)
        art["head"]["b"].append(floor)

    art["gym_version"] = DST_VERSION
    art["features"] = DST_FEATURES
    art["actions"] = DST_ACTIONS

    with open(args.out, "w") as f:
        json.dump(art, f)
        f.write("\n")
    print(f"widened v{SRC_VERSION} -> v{DST_VERSION}: {args.out}")


def widen_ckpt(src: str, out: str) -> None:
    """Widens a v5 FLOAT checkpoint to the v6 shape for training: the
    new feature column enters at zero weight, and the new action rows
    enter at zero weight and ZERO bias — reachable, so PPO can
    explore the verbs (the shipped artifact's unreachable floor is the
    opposite choice, made for the opposite reason)."""
    blob = torch.load(src, map_location="cpu", weights_only=True)
    if not (isinstance(blob, dict) and "state" in blob):
        raise SystemExit("expected a save_policy blob with arch + state")
    recorded = blob.get("gym_version")
    if recorded is not None and recorded != SRC_VERSION:
        raise SystemExit(
            f"checkpoint speaks gym v{recorded}, widen expects v{SRC_VERSION}"
        )
    state = blob["state"]
    first = "trunk.0.weight"
    w = state[first]
    if w.shape[1] != SRC_FEATURES + 3:
        raise SystemExit(
            f"first layer reads {w.shape[1]} inputs, expected {SRC_FEATURES + 3}"
        )
    for idx in sorted(NEW_FEATURE_SCALES):
        zero_col = torch.zeros(w.shape[0], 1, dtype=w.dtype)
        w = torch.cat([w[:, :idx], zero_col, w[:, idx:]], dim=1)
    state[first] = w
    pi_w, pi_b = state["pi.weight"], state["pi.bias"]
    if pi_w.shape[0] != SRC_ACTIONS:
        raise SystemExit(f"head emits {pi_w.shape[0]} logits, expected {SRC_ACTIONS}")
    grow = DST_ACTIONS - SRC_ACTIONS
    state["pi.weight"] = torch.cat(
        [pi_w, torch.zeros(grow, pi_w.shape[1], dtype=pi_w.dtype)]
    )
    state["pi.bias"] = torch.cat([pi_b, torch.zeros(grow, dtype=pi_b.dtype)])
    blob["gym_version"] = DST_VERSION
    torch.save(blob, out)
    print(f"widened checkpoint v{SRC_VERSION} -> v{DST_VERSION}: {out}")


if __name__ == "__main__":
    main()
