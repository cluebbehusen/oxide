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

from __future__ import annotations

import argparse
import pathlib

import numpy as np
import torch
import torch.nn as nn

from models import make_policy, save_policy
from oxide_gym import Worker, with_condition

# Action indices (see sim/src/bot/gym.rs).
IDLE, TRAIN_H, TRAIN_S = 0, 1, 2
FORM, PUSH, SCOUT = 7, 8, 10


def teacher(raw: list[int], mask: np.ndarray, tick: int) -> int:
    harvesters, staging_size = raw[2], raw[11]
    if harvesters < 4 and mask[TRAIN_H]:
        return TRAIN_H
    if mask[PUSH] and staging_size >= 5:
        return PUSH
    if mask[FORM]:
        return FORM
    if mask[TRAIN_S]:
        return TRAIN_S
    if mask[SCOUT] and tick % 1024 == 0:
        return SCOUT
    return IDLE


def main():
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
        for ep in range(args.episodes):
            seat = ep % 2
            frame = worker.reset(20_000 + ep, control=(seat,), tier=args.tier)
            rng = np.random.default_rng(ep)
            while not frame.done:
                view = frame.seats[seat]
                a = teacher(view.raw, view.mask, frame.tick)
                # The teacher ignores the knobs, so the prior is defined
                # across the whole conditioning space: replicate each
                # sample at several random knob settings.
                for _ in range(3):
                    cond = (int(rng.integers(300, 1001)), int(rng.integers(0, 1001)))
                    obs_all.append(with_condition(view.obs, cond))
                    mask_all.append(view.mask)
                    act_all.append(a)
                frame = worker.step({seat: a})
            wins += 1 if frame.winner == seat else 0
        print(f"teacher: {wins}/{args.episodes} wins vs {args.tier}")
    finally:
        worker.close()

    obs = torch.as_tensor(np.stack(obs_all))
    mask = torch.as_tensor(np.stack(mask_all))
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
