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

Usage:
    uv run widen.py --src ../../sim/src/bot/ladder_weights.json \
        --out ../../sim/src/bot/ladder_weights.json
"""

import argparse
import json

# The v5 contract this bridge widens TO. Kept as explicit literals so
# a rerun against the wrong source fails loudly instead of stacking.
SRC_VERSION = 4
SRC_FEATURES = 63
SRC_ACTIONS = 21
DST_VERSION = 5
DST_FEATURES = 64
DST_ACTIONS = 22
# Appended features, index -> normalization scale (mirrors
# SCALE_BY_NAME in oxide_gym.py; the recip formula is 2^(2q)/scale).
NEW_FEATURE_SCALES = {63: 500}  # my_building_value


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

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
    for idx in sorted(NEW_FEATURE_SCALES):
        recip = (1 << (2 * q)) // NEW_FEATURE_SCALES[idx]
        art["recips"].insert(idx, recip)
        for row in art["layers"][0]["w"]:
            if len(row) != SRC_FEATURES + cond + idx - 63:
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


if __name__ == "__main__":
    main()
