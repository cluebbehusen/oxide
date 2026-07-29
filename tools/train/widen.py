"""Widen a v6 actor into a measured initialization for gym v7.

The v7 bridge is intentionally different from the older behavior-preserving
bridges. V7 changes one categorical action into three factorized heads. New
feature and strategy-condition columns still enter with ZERO weights, but the
two appended rows are the construction and operation no-ops and therefore copy
the old Idle row instead of entering at an unreachable floor. The bridge is a
measured initialization, not a claim of behavior identity.

The v6 -> v7 hop appends sixteen strategic features, four one-hot strategy
conditions, and two head-specific no-op rows.

Usage:
    uv run widen.py --src ../../sim/src/bot/ladder_weights.json \
        --out ../../sim/src/bot/ladder_weights.json
"""

import argparse
import json

import torch

from lineage import build_lineage, input_identity

# The v7 contract this bridge widens TO. Kept as explicit literals so
# a rerun against the wrong source fails loudly instead of stacking.
SRC_VERSION = 6
SRC_FEATURES = 65
SRC_ACTIONS = 24
SRC_CONDITIONING = 3
DST_VERSION = 7
DST_FEATURES = 81
DST_ACTIONS = 26
DST_CONDITIONING = 7
# Appended features, index -> normalization scale (mirrors
# SCALE_BY_NAME in oxide_gym.py; the recip formula is 2^(2q)/scale).
NEW_FEATURE_SCALES = {
    65: 2_000,  # known_salvage_value
    66: 1_000,  # near_home_salvage_value
    67: 200,  # nearest_salvage_distance
    68: 8,  # idle_harvesters
    69: 200,  # carried_scrap
    70: 1_000,  # queued_unit_value
    71: 1_000,  # construction_site_value
    72: 2_000,  # my_unit_health_value
    73: 1_000,  # my_building_health_value
    74: 2,  # my_bastions_built
    75: 1,  # my_repair_bays_built
    76: 4,  # my_construction_sites
    77: 500,  # home_enemy_pressure
    78: 200,  # nearest_enemy_distance
    79: 7,  # construction_plan
    80: 250,  # construction_reserve
}


def widening_lineage(src: str, metadata: dict) -> dict[str, object]:
    """Builds provenance for the behavior-changing contract bridge."""
    return build_lineage(
        phase="contract-widen-v6-v7",
        phase_start_update=int(metadata.get("update", 0) or 0),
        hyperparameters={
            "copied_noop_action": 0,
            "dst_actions": DST_ACTIONS,
            "dst_conditioning": DST_CONDITIONING,
            "dst_features": DST_FEATURES,
            "dst_gym_version": DST_VERSION,
            "new_feature_scales": NEW_FEATURE_SCALES,
            "src_actions": SRC_ACTIONS,
            "src_conditioning": SRC_CONDITIONING,
            "src_features": SRC_FEATURES,
            "src_gym_version": SRC_VERSION,
        },
        inputs={"source": input_identity(src, metadata)},
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

    # New feature columns: zero weight in the first layer, a sane recip
    # (value irrelevant behind zero weights, but a later fine-tune
    # resumes from here and inherits the normalization).
    for inserted, idx in enumerate(sorted(NEW_FEATURE_SCALES)):
        recip = round((1 << (2 * q)) / NEW_FEATURE_SCALES[idx])
        art["recips"].insert(idx, recip)
        for row in art["layers"][0]["w"]:
            if len(row) != SRC_FEATURES + cond + inserted:
                raise SystemExit("layer-0 width drifted mid-insert")
            row.insert(idx, 0)

    # Four strategy one-hot conditions follow the old profile knobs.
    # The recovered actor initially ignores them.
    for _ in range(DST_CONDITIONING - SRC_CONDITIONING):
        art["recips"].append(round((1 << (2 * q)) / 1_000))
        for row in art["layers"][0]["w"]:
            row.append(0)

    # New action rows are the no-ops for the construction and operation
    # heads. Copying the old Idle row gives each independent head a sane
    # inherited baseline; flooring either row would force that head to act.
    idle_w = list(art["head"]["w"][0])
    idle_b = art["head"]["b"][0]
    for _ in range(DST_ACTIONS - SRC_ACTIONS):
        art["head"]["w"].append(list(idle_w))
        art["head"]["b"].append(idle_b)

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
    """Widens a v6 float checkpoint into the factorized v7 shape."""
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
    for idx in sorted(NEW_FEATURE_SCALES):
        zero_col = torch.zeros(w.shape[0], 1, dtype=w.dtype)
        w = torch.cat([w[:, :idx], zero_col, w[:, idx:]], dim=1)
    extra_conditions = DST_CONDITIONING - SRC_CONDITIONING
    w = torch.cat(
        [w, torch.zeros(w.shape[0], extra_conditions, dtype=w.dtype)],
        dim=1,
    )
    state[first] = w
    pi_w, pi_b = state["pi.weight"], state["pi.bias"]
    if pi_w.shape[0] != SRC_ACTIONS:
        raise SystemExit(f"head emits {pi_w.shape[0]} logits, expected {SRC_ACTIONS}")
    grow = DST_ACTIONS - SRC_ACTIONS
    idle_w = pi_w[0:1].repeat(grow, 1)
    idle_b = pi_b[0:1].repeat(grow)
    state["pi.weight"] = torch.cat([pi_w, idle_w])
    state["pi.bias"] = torch.cat([pi_b, idle_b])
    blob["gym_version"] = DST_VERSION
    blob["lineage"] = lineage
    torch.save(blob, out)
    print(f"widened checkpoint v{SRC_VERSION} -> v{DST_VERSION}: {out}")


if __name__ == "__main__":
    main()
