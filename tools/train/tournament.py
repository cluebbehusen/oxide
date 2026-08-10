"""The promotion tournament: a checkpoint against the world.

Determinism means a single seed proves nothing, so every matchup runs
a fixed seed suite x both seat assignments, and the table reports
Wilson 95% intervals. Opponents: the scripted Overseer commander (the
fixed yardstick) plus the rush teacher (the known exploit). The
learned policy ships only if it clears the bar here with no degenerate
stall games.

Usage (from tools/train/):
    uv run tournament.py --ckpt runs/league1/latest.pt
    uv run tournament.py --ckpt runs/bc.pt --seeds 10   # quick look
"""

from __future__ import annotations

import argparse
import json
import math
from typing import TYPE_CHECKING, Protocol

import numpy as np
import torch
from torch import nn

from league import (
    faction_knob,
    maybe_blunder,
    policy_skill_for_aggression,
    rusher,
)
from mapgen import cache_dir, generate
from models import factorized_greedy, load_policy
from oxide_gym import (
    ActionPlan,
    FactionName,
    Frame,
    ProfileCatalog,
    Worker,
    condition_from_profile,
)

if TYPE_CHECKING:
    from collections.abc import Sequence


class TournamentWorker(Protocol):
    """The gym surface one tournament match needs."""

    profile_catalog: ProfileCatalog

    def reset(
        self,
        seed: int,
        control: tuple[int, ...] = (0,),
        max_ticks: int = 40_000,
        cadence: int = 16,
        scenario: str | None = None,
        conditions: dict[int, tuple[int, ...]] | None = None,
        factions: str | Sequence[FactionName] | None = None,
    ) -> Frame: ...

    def step(self, actions: dict[int, ActionPlan]) -> Frame: ...


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
    worker: TournamentWorker,
    opponent: str,
    seed: int,
    seat: int,
    scenario: str | None = None,
    condition: tuple[int, int] | None = None,
    profile_index: int = 0,
    role: str | None = None,
    hesitation_permille: int = 0,
    cadence: int = 28,
    seats: int = 2,
) -> tuple[bool | None, int]:
    """One greedy match; returns (won, ticks). `won` is None only for a
    true draw (tick cap with the learner standing) — elimination in a
    multiplayer game is a loss even while others fight on. The faction
    knob is appended per seat, honestly (even = ferrous). By default the
    actor consumes one complete Rust-authored named profile; ``condition``
    selects the explicit zero-facet research path. Policy conditioning
    and execution handicap remain independent."""
    if not 0 <= hesitation_permille <= 1000:
        raise ValueError(
            f"hesitation must be in 0..1000 permille, got {hesitation_permille}"
        )
    if cadence <= 0:
        raise ValueError(f"cadence must be positive, got {cadence}")
    if condition is None:
        catalog = worker.profile_catalog
        if not catalog.profiles:
            raise RuntimeError("tournament requires Rust canonical profiles")
        profile = catalog.profiles[profile_index % len(catalog.profiles)]
        default_role = catalog.default_role
        conds = {
            s: catalog.condition(
                profile.style,
                profile.variant,
                role if s == seat and role is not None else default_role,
                "cupric" if faction_knob(s) == 1000 else "ferrous",
            )
            for s in range(8)
        }
    else:
        conds = {
            s: condition_from_profile(*condition, faction_knob(s)) for s in range(8)
        }
    rusher_seat = None
    # The wire requires conditions to name exactly the controlled
    # seats; scripted opponents take none.
    if opponent == "rusher":
        # The rusher is driven locally, so its seat must be controlled
        # too — whichever seat the learner isn't (any of them in FFA).
        rusher_seat = (seat + 1) % seats
        frame = worker.reset(
            seed,
            control=(seat, rusher_seat),
            scenario=scenario,
            conditions={s: conds[s] for s in (seat, rusher_seat)},
            cadence=cadence,
        )
    else:
        frame = worker.reset(
            seed,
            control=(seat,),
            scenario=scenario,
            conditions={seat: conds[seat]},
            cadence=cadence,
        )
    rng = np.random.default_rng(seed * 2 + seat)
    while not frame.done:
        view = frame.seats.get(seat)
        acts: dict[int, ActionPlan]
        if view is None:
            # Team games: the seat's foundry fell while its team plays
            # on — the gym stops shipping views for dead seats and
            # expects no actions for them. Step the world empty and let
            # the final team outcome classify the game.
            acts = {}
        else:
            with torch.no_grad():
                logits, _ = policy(
                    torch.as_tensor(view.obs[None]),
                    torch.as_tensor(view.mask[None]),
                )
            # The shipped weakening is an execution-side hesitation,
            # independent of the policy's strategy conditioning.
            plan = factorized_greedy(logits)[0].cpu()
            intended: ActionPlan = (
                int(plan[0]),
                int(plan[1]),
                int(plan[2]),
                int(plan[3]),
            )
            acts = {
                seat: maybe_blunder(
                    intended,
                    logits[0].numpy(),
                    view.mask,
                    hesitation_permille,
                    rng,
                )
            }
        if rusher_seat is not None and rusher_seat in frame.seats:
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
    ap.add_argument("--opponents", default="overseer,rusher")
    ap.add_argument(
        "--scenario", default=None, help="map (default: the built-in skirmish)"
    )
    ap.add_argument(
        "--skill",
        type=int,
        default=None,
        help="raw skill override; omission uses the Rust canonical profile slate",
    )
    ap.add_argument(
        "--aggression",
        type=int,
        default=None,
        help="raw aggression override; omission uses the Rust canonical profile slate",
    )
    ap.add_argument(
        "--hesitation",
        type=int,
        default=0,
        help="exact execution-side hesitation per mille (default: Expert 0)",
    )
    ap.add_argument(
        "--cadence",
        type=int,
        default=28,
        help="decision stride in ticks (default: shipped Expert 28)",
    )
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
        "west seat, the Overseer drives its teammate and both foes",
    )
    args = ap.parse_args()

    if args.skill is not None and not 0 <= args.skill <= 1000:
        ap.error("--skill must be in 0..1000")
    if args.aggression is not None and not 0 <= args.aggression <= 1000:
        ap.error("--aggression must be in 0..1000")
    raw_condition = None
    if args.skill is not None or args.aggression is not None:
        aggression = 550 if args.aggression is None else args.aggression
        raw_condition = (
            (
                policy_skill_for_aggression(aggression)
                if args.skill is None
                else args.skill
            ),
            aggression,
        )
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
            for job_index, (seed, scenario) in enumerate(jobs):
                if team:
                    seats: tuple[int, ...] = (0, 2)  # both west chairs
                elif ffa:
                    seats = (seed % 4,)
                else:
                    seats = (0, 1)
                specialist_roles = [
                    role
                    for role in worker.profile_catalog.team_roles
                    if role != worker.profile_catalog.default_role
                ]
                for seat_index, seat in enumerate(seats):
                    role = None
                    if team and raw_condition is None:
                        if not specialist_roles:
                            raise RuntimeError(
                                "Rust profile catalog has no specialist team roles"
                            )
                        role = specialist_roles[
                            (job_index * len(seats) + seat_index)
                            % len(specialist_roles)
                        ]
                    won, t = play(
                        policy,
                        worker,
                        opponent,
                        seed,
                        seat,
                        scenario,
                        raw_condition,
                        profile_index=job_index,
                        role=role,
                        hesitation_permille=args.hesitation,
                        cadence=args.cadence,
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
                        "skill": args.skill,
                        "aggression": args.aggression,
                        "profile_curriculum": (
                            "rust-canonical-slate" if raw_condition is None else "raw"
                        ),
                        "hesitation_permille": args.hesitation,
                        "cadence": args.cadence,
                    }
                )
            )
    finally:
        worker.close()


if __name__ == "__main__":
    main()
