"""The promotion tournament: a checkpoint against the world.

Determinism means a single seed proves nothing, so every matchup runs
a fixed seed suite × both seat assignments, and the table reports
Wilson 95% intervals. Opponents: every scripted tier plus the rush
teacher (the known exploit). This is the gate the 0.7 plan defines —
the learned policy ships as the top difficulty only if it clears the
bar here with no degenerate stall games.

Usage (from tools/train/):
    uv run tournament.py --ckpt runs/league1/latest.pt
    uv run tournament.py --ckpt runs/bc.pt --seeds 10   # quick look
"""

from __future__ import annotations

import argparse
import json
import math

import numpy as np
import torch

from league import TIERS, rusher
from models import load_policy
from oxide_gym import Worker


def wilson(wins: int, games: int, z: float = 1.96) -> tuple[float, float]:
    if games == 0:
        return (0.0, 1.0)
    p = wins / games
    denom = 1 + z * z / games
    center = (p + z * z / (2 * games)) / denom
    half = z * math.sqrt(p * (1 - p) / games + z * z / (4 * games * games)) / denom
    return (center - half, center + half)


def play(policy, worker: Worker, opponent: str, seed: int, seat: int) -> tuple[bool | None, int]:
    """One greedy match; returns (won, ticks)."""
    if opponent == "rusher":
        frame = worker.reset(seed, control=(0, 1))
    else:
        frame = worker.reset(seed, control=(seat,), tier=opponent)
    while not frame.done:
        view = frame.seats[seat]
        with torch.no_grad():
            logits, _ = policy(
                torch.as_tensor(view.obs[None]),
                torch.as_tensor(view.mask[None]),
            )
        acts = {seat: int(logits.argmax())}
        if opponent == "rusher":
            ov = frame.seats[1 - seat]
            acts[1 - seat] = rusher(ov.raw, ov.mask, frame.tick)
        frame = worker.step(acts)
    if frame.winner is None:
        return None, frame.tick
    return frame.winner == seat, frame.tick


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ckpt", required=True)
    ap.add_argument("--driver", default="../../target/release/oxide-driver")
    ap.add_argument("--seeds", type=int, default=30)
    ap.add_argument("--opponents", default=",".join(TIERS + ["rusher"]))
    args = ap.parse_args()

    policy, blob = load_policy(args.ckpt)
    policy.eval()
    worker = Worker(args.driver)
    print(f"# {args.ckpt} (arch {blob.get('arch')}, update {blob.get('update', '?')})")
    try:
        for opponent in args.opponents.split(","):
            wins = draws = games = 0
            ticks = []
            for seed in range(3000, 3000 + args.seeds):
                for seat in (0, 1):
                    won, t = play(policy, worker, opponent, seed, seat)
                    games += 1
                    ticks.append(t)
                    if won is None:
                        draws += 1
                    elif won:
                        wins += 1
            lo, hi = wilson(wins, games)
            print(
                json.dumps(
                    {
                        "opponent": opponent,
                        "wins": wins,
                        "draws": draws,
                        "games": games,
                        "rate": round(wins / games, 3),
                        "ci95": [round(lo, 3), round(hi, 3)],
                        "median_ticks": int(np.median(ticks)),
                    }
                )
            )
    finally:
        worker.close()


if __name__ == "__main__":
    main()
