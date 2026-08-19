"""The conduct gate: does a candidate still play whole games?

The skirmish cup prices duels and nothing else; the es-5 run proved that
eight hours of selection will quietly spend FFA and island conduct the
fitness never measured (every cup metric improved while three maps the
incumbent decides fell into passive 133-minute caps). This gate is the
guard: a small discriminating suite of full games — the FFA the cup
never plays, the many-team scramble, and the ground-sealed archipelago
— each of which must DECIDE within its horizon and carry no conduct
findings (passive giants, quiet caps, discovery failures).

Built for confirm cadence in the training loop and for promotion
batteries. Validation fixture pair: the shipped es-1 recovery passes,
the refused es-5 g800 fails, on identical maps and seeds — a gate that
cannot separate those two is not ready to steer a run.

Usage, from tools/train:

  uv run conduct_gate.py --weights runs/night2/es1-exact.pt
  uv run conduct_gate.py --weights runs/es-5/es5-exact.pt   # expect FAIL
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    import torch

from audit import GameTrace, play, scenario_info, screen_game
from models import load_policy
from oxide_gym import Worker

#: Screens that indict conduct even in a decided game.
CONDUCT_SCREENS = frozenset(
    {"PASSIVE_GIANT", "QUIET_CAP", "DISCOVERY_FAIL", "FROZEN_MENU"}
)

#: The discriminating suite: (scenario path relative to tools/train,
#: seed, tick horizon). Three seeds per map because single FFA seeds
#: flip on deterministic chaos; the refused es-5 artifact capped every
#: cell while the incumbent decides most.
SUITE: tuple[tuple[str, int, int], ...] = tuple(
    (scenario, seed, 60_000)
    for scenario in (
        "../../scenarios/pentangle-claim.json",
        "../../scenarios/scramble-basin.json",
        "../../map-drafts/the-scattering.json",
    )
    for seed in (0, 1, 2)
)


def run_gate(
    worker: Worker, actor: torch.nn.Module, suite: tuple[tuple[str, int, int], ...]
) -> dict:
    """Plays the suite and returns the verdict with per-game evidence."""
    games: list[dict] = []
    verdict = True
    for scenario, seed, ticks in suite:
        path = pathlib.Path(scenario)
        seat_count, mode = scenario_info(path)
        game: GameTrace = play(worker, actor, path, seat_count, seed, ticks)
        game.mode = mode
        findings = screen_game(game)
        conduct = [f for f in findings if f["screen"] in CONDUCT_SCREENS]
        decided = game.winner is not None
        ok = decided and not conduct
        verdict = verdict and ok
        games.append(
            {
                "scenario": path.stem,
                "mode": mode,
                "seed": seed,
                "decided": decided,
                "tick": game.tick,
                "conduct_findings": conduct,
                "ok": ok,
            }
        )
    decided = sum(g["decided"] for g in games)
    conduct = sum(len(g["conduct_findings"]) for g in games)
    return {
        "pass": verdict,
        "decided": decided,
        "conduct_findings": conduct,
        "games": games,
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--weights", required=True, help="policy checkpoint (.pt)")
    ap.add_argument("--driver", default="../../target/release/oxide-driver")
    ap.add_argument(
        "--baseline",
        default=None,
        help="incumbent baseline JSON (this script's own output): the "
        "candidate passes by not being WORSE — at least as many decided "
        "games and no more conduct findings. Single cells flip on "
        "deterministic chaos; the paired margin is the honest bar.",
    )
    ap.add_argument(
        "--save-baseline",
        default=None,
        help="write this run's report here (recorded on promotion)",
    )
    args = ap.parse_args()

    actor, _ = load_policy(args.weights)
    worker = Worker(args.driver)
    try:
        report = run_gate(worker, actor, SUITE)
    finally:
        worker.close()

    if args.baseline:
        base = json.loads(pathlib.Path(args.baseline).read_text())
        report["baseline"] = {
            "decided": base["decided"],
            "conduct_findings": base["conduct_findings"],
        }
        report["pass"] = (
            report["decided"] >= base["decided"]
            and report["conduct_findings"] <= base["conduct_findings"]
        )
    if args.save_baseline:
        pathlib.Path(args.save_baseline).write_text(json.dumps(report, indent=1))

    print(json.dumps(report, indent=1))
    print(
        f"conduct gate: {'PASS' if report['pass'] else 'FAIL'} "
        f"({report['decided']}/{len(report['games'])} decided, "
        f"{report['conduct_findings']} conduct findings)"
    )
    return 0 if report["pass"] else 1


if __name__ == "__main__":
    sys.exit(main())
