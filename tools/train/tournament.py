"""The promotion tournament: a checkpoint against the world.

Determinism means a single seed proves nothing, so every matchup runs
a fixed seed suite x both seat assignments, and the table reports
Wilson 95% intervals. Opponents: every scripted tier plus the rush
teacher (the known exploit). This is the gate the 0.7 plan defines —
the learned policy ships as the top difficulty only if it clears the
bar here with no degenerate stall games.

Usage (from tools/train/):
    uv run tournament.py --ckpt runs/league1/latest.pt
    uv run tournament.py --ckpt runs/bc.pt --seeds 10   # quick look
"""

import argparse
import json
import math

import numpy as np
import torch
from torch import nn

from league import TIERS, faction_knob, maybe_blunder, rusher
from mapgen import cache_dir, generate
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


def play(
    policy: nn.Module,
    worker: Worker,
    opponent: str,
    seed: int,
    seat: int,
    scenario: str | None = None,
    condition: tuple[int, int] = (1000, 500),
    seats: int = 2,
) -> tuple[bool | None, int]:
    """One greedy match; returns (won, ticks). `won` is None only for a
    true draw (tick cap with the learner standing) — elimination in a
    multiplayer game is a loss even while others fight on. The faction
    knob is appended per seat, honestly (even = ferrous)."""
    conds: dict[int, tuple[int, ...]] = {
        s: (*condition, faction_knob(s)) for s in range(8)
    }
    rusher_seat = None
    if opponent == "rusher":
        # The rusher is driven locally, so its seat must be controlled
        # too — whichever seat the learner isn't (any of them in FFA).
        rusher_seat = (seat + 1) % seats
        frame = worker.reset(
            seed, control=(seat, rusher_seat), scenario=scenario, conditions=conds
        )
    else:
        frame = worker.reset(
            seed, control=(seat,), tier=opponent, scenario=scenario, conditions=conds
        )
    rng = np.random.default_rng(seed * 2 + seat)
    while not frame.done:
        view = frame.seats[seat]
        with torch.no_grad():
            logits, _ = policy(
                torch.as_tensor(view.obs[None]),
                torch.as_tensor(view.mask[None]),
            )
        # The shipped weakening is knob input + forced near-best
        # blunders, exactly as trained — evaluate that mechanism.
        intended = int(logits.argmax())
        acts = {
            seat: maybe_blunder(
                intended, logits[0].numpy(), view.mask, condition[0], rng
            )
        }
        if rusher_seat is not None:
            ov = frame.seats[rusher_seat]
            acts[rusher_seat] = rusher(ov.raw, ov.mask, frame.tick)
        frame = worker.step(acts)
    if frame.winners:
        return seat in frame.winners, frame.tick
    if frame.winner is not None:
        return frame.winner == seat, frame.tick
    if frame.alive is not None and seat not in frame.alive:
        # Eliminated; the game merely outlived us. A draw is only a
        # tick-cap with the learner still standing.
        return False, frame.tick
    return None, frame.tick


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--ckpt", required=True)
    ap.add_argument("--driver", default="../../target/release/oxide-driver")
    ap.add_argument("--seeds", type=int, default=30)
    ap.add_argument("--opponents", default=",".join([*TIERS, "rusher"]))
    ap.add_argument(
        "--scenario", default=None, help="map (default: the built-in skirmish)"
    )
    ap.add_argument("--skill", type=int, default=1000)
    ap.add_argument("--aggression", type=int, default=500)
    ap.add_argument(
        "--random-maps",
        type=int,
        default=0,
        help="evaluate across N generated maps instead of seed variations",
    )
    ap.add_argument(
        "--ffa-maps",
        type=int,
        default=0,
        help="evaluate as 1-of-4 on N generated FFA maps (seat rotates)",
    )
    ap.add_argument(
        "--team-maps",
        type=int,
        default=0,
        help="evaluate on N generated 2v2 maps: the learner takes one "
        "west seat, a scripted tier drives its teammate and both foes",
    )
    args = ap.parse_args()

    policy, blob = load_policy(args.ckpt)
    policy.eval()
    worker = Worker(args.driver)
    print(f"# {args.ckpt} (arch {blob.get('arch')}, update {blob.get('update', '?')})")
    try:
        jobs: list[tuple[int, str | None]]
        ffa = args.ffa_maps > 0
        team = args.team_maps > 0
        if team:
            jobs = [
                (
                    9800 + i,
                    generate(
                        9800 + i,
                        cache_dir("oxide-maps2v2"),
                        players=4,
                        teams=True,
                    ),
                )
                for i in range(args.team_maps)
            ]
        elif ffa:
            jobs = [
                (9500 + i, generate(9500 + i, cache_dir("oxide-maps4"), players=4))
                for i in range(args.ffa_maps)
            ]
        elif args.random_maps:
            jobs = [
                (9000 + i, generate(9000 + i, cache_dir("oxide-maps")))
                for i in range(args.random_maps)
            ]
        else:
            jobs = [(seed, args.scenario) for seed in range(3000, 3000 + args.seeds)]
        for opponent in args.opponents.split(","):
            if (ffa or team) and opponent == "rusher":
                # The scripted rusher is a duel-era probe; in FFA its
                # seat shares the episode's fate with the learner's,
                # which muddies the classification. Skip it.
                print(json.dumps({"opponent": "rusher", "skipped": "ffa"}))
                continue
            wins = draws = games = 0
            ticks = []
            for seed, scenario in jobs:
                if team:
                    seats: tuple[int, ...] = (0, 2)  # both west chairs
                elif ffa:
                    seats = (seed % 4,)
                else:
                    seats = (0, 1)
                for seat in seats:
                    won, t = play(
                        policy,
                        worker,
                        opponent,
                        seed,
                        seat,
                        scenario,
                        (args.skill, args.aggression),
                        seats=4 if (ffa or team) else 2,
                    )
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
