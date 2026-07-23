"""Behavior-cloning warm start.

PPO from scratch never finds its first win (terminal-only reward,
hundreds of decisions per episode), so the run starts from imitation:
a tiny scripted teacher — the same line the Rust gym test plays, which
beats Scrapheap — generates (features, mask, action) tuples, and the
policy net learns to match them. The result is a checkpoint that
already plays a coherent game for `train.py --resume` to improve on.

Usage (from tools/train/):
    uv run bc.py --episodes 40 --out runs/bc.pt
"""

import argparse
import pathlib

import numpy as np
import torch
from torch import nn

from models import make_policy, save_policy
from oxide_gym import FEATURE_NAMES, Worker, with_condition

# Action indices (see sim/src/bot/gym.rs — the v3 menu).
IDLE, TRAIN_H, TRAIN_S = 0, 1, 2
TRAIN_AA, TRAIN_WING = 6, 7
BUILD_FAB = 9
BUILD_TURRET, BUILD_FLAK, BUILD_RECLAIMER, REPAIR, AIR_RAID = 10, 11, 14, 15, 16
FORM, PUSH, SCOUT = 17, 18, 20

# Feature indices by name (the Worker asserts the same list against
# the gym hello, so these lookups cannot skew).
F = {name: i for i, name in enumerate(FEATURE_NAMES)}


def rusher(raw: list[int], mask: np.ndarray, tick: int) -> int:
    """The aggressive teacher: economy to four, pressure forever —
    and a wing at the harvest line once the Fabricator stands."""
    harvesters = raw[F["my_harvesters"]]
    staging_size = raw[F["staging_army_size"]]
    enemy_air = raw[F["enemy_airground"]] + raw[F["enemy_airair"]]
    if harvesters < 4 and mask[TRAIN_H]:
        return TRAIN_H
    # The tech step the lineage never had: without this, fab_built
    # stays zero forever and the wing branch below is dead code — and
    # every policy cloned from these teachers spams basics for life.
    if raw[F["fab_built"]] == 0 and raw[F["scrap"]] >= 130 and mask[BUILD_FAB]:
        return BUILD_FAB
    if enemy_air > raw[F["my_antiair"]] and mask[TRAIN_AA]:
        return TRAIN_AA
    if mask[AIR_RAID] and raw[F["my_airground"]] >= 3:
        return AIR_RAID
    if mask[PUSH] and staging_size >= 5:
        return PUSH
    if mask[FORM]:
        return FORM
    if raw[F["my_airground"]] < 3 and raw[F["fab_built"]] > 0 and mask[TRAIN_WING]:
        return TRAIN_WING
    # Pre-tech the line caps at three: sentinel training at 75 a pop
    # otherwise eats the bank faster than it can reach the Fabricator
    # price — which is exactly how the lineage stayed techless.
    if (raw[F["fab_built"]] > 0 or staging_size < 3) and mask[TRAIN_S]:
        return TRAIN_S
    if mask[SCOUT] and tick % 1024 == 0:
        return SCOUT
    return IDLE


def turtle(raw: list[int], mask: np.ndarray, tick: int) -> int:
    """The defensive teacher: deep economy, turrets and flak, welds
    its wounds, retires onto Reclaimers, one late hammer."""
    harvesters = raw[F["my_harvesters"]]
    turrets = raw[F["my_turrets_built"]]
    staging_size = raw[F["staging_army_size"]]
    if harvesters < 5 and mask[TRAIN_H]:
        return TRAIN_H
    if turrets < 3 and mask[BUILD_TURRET]:
        return BUILD_TURRET
    threatened = raw[F["blip_count"]] > 0 or raw[F["enemy_airground"]] > 0
    if threatened and raw[F["my_flak_built"]] < 2 and mask[BUILD_FLAK]:
        return BUILD_FLAK
    if raw[F["repair_deficit"]] > 150 and mask[REPAIR]:
        return REPAIR
    if raw[F["wreck_value"]] + raw[F["scrap"]] < 200 and mask[BUILD_RECLAIMER]:
        return BUILD_RECLAIMER
    if raw[F["fab_built"]] == 0 and raw[F["scrap"]] >= 150 and mask[BUILD_FAB]:
        return BUILD_FAB
    if raw[F["fab_built"]] > 0 and raw[F["enemy_airground"]] == 0 and mask[TRAIN_WING]:
        return TRAIN_WING
    if mask[PUSH] and staging_size >= 10:
        return PUSH
    if mask[FORM]:
        return FORM
    if (raw[F["fab_built"]] > 0 or staging_size < 4) and mask[TRAIN_S]:
        return TRAIN_S
    if mask[SCOUT] and tick % 2048 == 0:
        return SCOUT
    return IDLE


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--driver", default="../../target/release/oxide-driver")
    ap.add_argument("--episodes", type=int, default=40)
    ap.add_argument("--tier", default="scrapheap")
    ap.add_argument("--epochs", type=int, default=20)
    ap.add_argument("--arch", default="mlp")
    ap.add_argument("--out", default="runs/bc.pt")
    args = ap.parse_args()

    worker = Worker(args.driver)
    obs_all, mask_all, act_all = [], [], []
    wins = 0
    try:
        style_wins = {"rusher": 0, "turtle": 0}
        for ep in range(args.episodes):
            seat = ep % 2
            style = "rusher" if (ep // 2) % 2 == 0 else "turtle"
            teach = rusher if style == "rusher" else turtle
            # The faction knob is honest: skirmish seats Ferrous at 0
            # and Cupric at 1, so the label follows the seat.
            faction = 0 if seat == 0 else 1000
            frame = worker.reset(20_000 + ep, control=(seat,), tier=args.tier)
            rng = np.random.default_rng(ep)
            while not frame.done:
                view = frame.seats[seat]
                a = teach(view.raw, view.mask, frame.tick)
                # Aggression is labeled to match the teacher who produced
                # the sample — the prior is style-conditional from day
                # one. Skill stays randomized (teachers don't model it).
                for _ in range(3):
                    agg_lo, agg_hi = (600, 1001) if style == "rusher" else (0, 401)
                    cond = (
                        int(rng.integers(300, 1001)),
                        int(rng.integers(agg_lo, agg_hi)),
                        faction,
                    )
                    obs_all.append(with_condition(view.obs, cond))
                    mask_all.append(view.mask)
                    act_all.append(a)
                frame = worker.step({seat: a})
            if frame.winner == seat:
                wins += 1
                style_wins[style] += 1
        print(f"per-style wins: {style_wins}")
        print(f"teacher: {wins}/{args.episodes} wins vs {args.tier}")
    finally:
        worker.close()

    obs = torch.as_tensor(np.stack(obs_all))
    mask = torch.as_tensor(np.stack(mask_all))
    # Recording telemetry: a dataset that never contains an action can
    # never teach it — the techless-lineage bug hid here.
    counts = np.bincount(np.asarray(act_all), minlength=21)
    print("action counts:", {i: int(c) for i, c in enumerate(counts) if c})
    act = torch.as_tensor(np.asarray(act_all))
    policy = make_policy(args.arch)
    opt = torch.optim.Adam(policy.parameters(), lr=1e-3)
    n = len(act)
    for epoch in range(args.epochs):
        perm = torch.randperm(n)
        total = 0.0
        for start in range(0, n, 1024):
            mb = perm[start : start + 1024]
            logits, _ = policy(obs[mb], mask[mb])
            loss = nn.functional.cross_entropy(logits, act[mb])
            opt.zero_grad()
            loss.backward()
            opt.step()
            total += float(loss.detach()) * len(mb)
        print(f"epoch {epoch}: loss {total / n:.4f}")
    pathlib.Path(args.out).parent.mkdir(parents=True, exist_ok=True)
    save_policy(policy, args.arch, args.out)
    print(f"saved {args.out} ({n} samples)")


if __name__ == "__main__":
    main()
