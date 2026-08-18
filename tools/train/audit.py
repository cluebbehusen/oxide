"""The game auditor: finds gaps nobody reported yet.

Plays bot-vs-bot games across shipped and procedurally generated maps
with every seat under the candidate policy, samples each seat's state,
mask, and chosen plan at every think, then screens the traces for the
failure shapes that make games feel wrong: dominant seats idling,
economies starving in place instead of expanding, quiet time-cap
draws, doctrine oscillators, frozen menus, discovery failures, and
actions the policy never uses. Findings land in a ranked report with
the evidence attached — the point is to surface entries for
docs/known-gaps.md before a human has to feel them in play.

Usage (from tools/train/):
    uv run audit.py --weights runs/night2/es1-exact.pt \
        --name sweep1 --procgen 6 --seeds 1
"""

from __future__ import annotations

import argparse
import json
import pathlib
from collections import Counter
from dataclasses import dataclass, field

import torch

import mapgen
from bc import F
from models import factorized_greedy, load_policy
from oxide_gym import ActionPlan, Worker

TICKS_PER_SECOND = 20
ACTION_NAMES = {
    0: "idle",
    1: "harvester",
    2: "sentinel",
    3: "scuttler",
    4: "lancer",
    5: "bombard",
    6: "anti-air",
    7: "air-ground",
    8: "air-air",
    9: "fabricator",
    10: "turret",
    11: "flak",
    12: "bastion",
    13: "array",
    14: "reclaimer",
    15: "repair",
    16: "air-raid",
    17: "form-army",
    18: "push",
    19: "recall",
    20: "scout",
    21: "salvage",
    22: "repair-unit",
    23: "repair-bay",
    24: "no-construction",
    25: "no-operation",
    26: "warden",
    27: "tender",
    28: "excavator",
    29: "scout-flyer",
    30: "interceptor",
    31: "bomber",
    32: "transport",
    33: "sapper",
    34: "breaker",
    35: "avalanche",
    36: "airworks",
    37: "crucible",
    38: "foundry",
    39: "extractor",
    40: "upgrade",
    41: "airlift",
    42: "no-upgrade",
}
PRODUCTION_IDS = tuple(range(1, 9)) + tuple(range(26, 36))
CONSTRUCTION_IDS = (9, 10, 11, 12, 13, 14, 23, 36, 37, 38, 39)


def clock(ticks: int) -> str:
    """Ticks as a human-readable match duration."""
    seconds = ticks // TICKS_PER_SECOND
    return f"{seconds // 60}m{seconds % 60:02d}s"


def head_shares(counts: Counter, ids: tuple[int, ...], top: int = 0) -> str:
    total = sum(counts[i] for i in ids)
    if total == 0:
        return "none"
    ranked = sorted(ids, key=lambda i: -counts[i])
    if top:
        ranked = ranked[:top]
    return ", ".join(
        f"{ACTION_NAMES[i]} {counts[i] * 100 // total}%"
        for i in ranked
        if counts[i] > 0
    )


OP_HEAD = (25, 16, 17, 18, 19, 20, 41)
OP_NAMES = {
    25: "idle",
    16: "raid",
    17: "form",
    18: "push",
    19: "recall",
    20: "scout",
    41: "lift",
}
PROFILE_ROTATION = [
    ("balanced", 0),
    ("aggressive", 1),
    ("turtle", 2),
    ("balanced", 1),
    ("aggressive", 0),
    ("turtle", 0),
    ("balanced", 2),
    ("aggressive", 2),
]
TAIL_START = 30_000
DOMINANCE = 2
IDLE_SHARE = 0.8
EXPANSION_MIN_TICKS = 25_000
STARVED_IDLE_SHARE = 0.7
OSCILLATION_MIN = 20
FROZEN_WIDTH = 5
DISCOVERY_DEADLINE = 20_000


@dataclass
class SeatTrace:
    """Everything the screens need about one seat's game."""

    decisions: int = 0
    ops: Counter = field(default_factory=Counter)
    tail_n: int = 0
    tail_idle: int = 0
    tail_width: int = 0
    tail_mine: int = 0
    tail_seen: int = 0
    tail_scrap: int = 0
    tail_harv: int = 0
    tail_idle_harv: int = 0
    tail_legal: Counter = field(default_factory=Counter)
    max_mine: int = 0
    alive: bool = True
    site_known_at: int | None = None
    max_foundries: int = 0
    last_narrowed: str | None = None
    alternations: int = 0
    chosen_actions: Counter = field(default_factory=Counter)


@dataclass
class GameTrace:
    map_name: str
    seed: int
    winner: int | None
    tick: int
    seats: dict[int, SeatTrace]
    mode: str = "?"


def screen_game(game: GameTrace) -> list[dict]:
    """Pure anomaly screens over one game's trace."""
    findings: list[dict] = []
    where = {"map": game.map_name, "seed": game.seed}
    undecided = game.winner is None
    for seat, t in game.seats.items():
        if not t.alive:
            continue
        tn = max(t.tail_n, 1)
        idle_share = t.tail_idle / tn
        mine = t.tail_mine // tn
        seen = t.tail_seen // tn
        if (
            undecided
            and t.tail_n > 0
            and mine >= seen * DOMINANCE
            and mine > 0
            and idle_share >= IDLE_SHARE
        ):
            findings.append(
                {
                    "screen": "IDLE_DOMINANT",
                    "seat": seat,
                    "evidence": f"tail strength {mine} vs seen {seen}, "
                    f"idle {idle_share:.0%} of {t.tail_n} decisions; legal ["
                    + ", ".join(
                        f"{k} {v * 100 // tn}%" for k, v in t.tail_legal.most_common()
                    )
                    + "]",
                    **where,
                }
            )
        harv = t.tail_harv // tn
        idle_harv = t.tail_idle_harv // tn
        if (
            t.tail_n > 0
            and harv >= 3
            and idle_harv * 10 >= harv * int(STARVED_IDLE_SHARE * 10)
        ):
            findings.append(
                {
                    "screen": "ECONOMY_STARVED",
                    "seat": seat,
                    "evidence": f"{idle_harv}/{harv} harvesters idle in the tail "
                    f"(scrap bank {t.tail_scrap // tn}) — nothing left to mine "
                    "where they stand",
                    **where,
                }
            )
        if t.alternations >= OSCILLATION_MIN:
            findings.append(
                {
                    "screen": "OSCILLATOR",
                    "seat": seat,
                    "evidence": f"{t.alternations} narrowed A-B alternations",
                    **where,
                }
            )
        if t.tail_n > 0 and t.tail_width // tn <= FROZEN_WIDTH:
            findings.append(
                {
                    "screen": "FROZEN_MENU",
                    "seat": seat,
                    "evidence": f"tail mask width {t.tail_width // tn}",
                    **where,
                }
            )
        # tail_n > 0 keeps eliminated seats out: the dead cannot scout.
        if (
            undecided
            and game.tick >= DISCOVERY_DEADLINE
            and t.site_known_at is None
            and t.tail_n > 0
        ):
            findings.append(
                {
                    "screen": "DISCOVERY_FAIL",
                    "seat": seat,
                    "evidence": f"no enemy foundry known by tick {game.tick}",
                    **where,
                }
            )
    for seat, t in game.seats.items():
        # Runs decided or not: a horde that never attacks is a defect
        # even when it "wins" — measured as a 259-unit army shoved into
        # the enemy base by collision physics, which the outcome-gated
        # screens waved through as a healthy victory.
        offensive = t.ops["push"] + t.ops["raid"] + t.ops["lift"]
        if (
            t.decisions >= 300
            and t.max_mine >= 300
            and offensive * 100 < t.decisions
            and t.ops["idle"] * 100 >= t.decisions * 85
        ):
            findings.append(
                {
                    "screen": "PASSIVE_GIANT",
                    "seat": seat,
                    "evidence": f"peak strength {t.max_mine}, "
                    f"{offensive} offensive orders in {t.decisions} "
                    f"decisions, idle {t.ops['idle'] * 100 // t.decisions}%",
                    **where,
                }
            )
    if game.tick >= EXPANSION_MIN_TICKS and all(
        t.max_foundries < 2 for t in game.seats.values()
    ):
        findings.append(
            {
                "screen": "NEVER_EXPANDS",
                "seat": None,
                "evidence": f"{game.tick}-tick game, no seat ever built a "
                "second Foundry",
                **where,
            }
        )
    if undecided and all(
        t.tail_n > 0 and t.tail_idle / max(t.tail_n, 1) >= 0.9
        for t in game.seats.values()
    ):
        findings.append(
            {
                "screen": "QUIET_CAP",
                "seat": None,
                "evidence": "undecided at the horizon with every seat idle "
                ">=90% of the tail",
                **where,
            }
        )
    return findings


def play(
    worker: Worker,
    actor: torch.nn.Module,
    scenario: pathlib.Path,
    seat_count: int,
    seed: int,
    ticks: int,
    record_dir: pathlib.Path | None = None,
) -> GameTrace:
    catalog = worker.profile_catalog
    control = tuple(range(seat_count))
    factions = "".join("f" if s % 2 == 0 else "c" for s in control)
    conds = {
        s: catalog.condition(
            *PROFILE_ROTATION[(seed + s) % len(PROFILE_ROTATION)],
            catalog.default_role,
            "ferrous" if s % 2 == 0 else "cupric",
        )
        for s in control
    }
    frame = worker.reset(
        61_000 + seed,
        control=control,
        max_ticks=ticks,
        scenario=str(scenario),
        conditions=conds,
        factions=factions,
        cadence=28,
        record=record_dir is not None,
    )
    seats = {s: SeatTrace() for s in control}
    while not frame.done:
        acts: dict[int, ActionPlan] = {}
        for seat, view in frame.seats.items():
            with torch.no_grad():
                logits, _ = actor(
                    torch.as_tensor(view.obs[None]),
                    torch.as_tensor(view.mask[None]),
                )
            greedy = factorized_greedy(logits)[0].cpu()
            plan: ActionPlan = (
                int(greedy[0]),
                int(greedy[1]),
                int(greedy[2]),
                int(greedy[3]),
            )
            t = seats[seat]
            t.decisions += 1
            t.ops[OP_NAMES.get(plan[3], str(plan[3]))] += 1
            for head_value in plan:
                t.chosen_actions[head_value] += 1
            if t.site_known_at is None and view.raw[F["enemy_foundry_known"]]:
                t.site_known_at = frame.tick
            t.max_foundries = max(
                t.max_foundries, int(view.raw[F["my_foundries_built"]])
            )
            t.max_mine = max(t.max_mine, int(view.raw[F["my_strength"]]))
            ops_open = [a for a in OP_HEAD if view.mask[a]]
            if len(ops_open) == 1:
                name = OP_NAMES.get(ops_open[0], str(ops_open[0]))
                # Only a push<->recall flip counts as thrash: the ferry
                # alternates lift legs by design, and a change of
                # narrowed verb is otherwise just the doctrine moving
                # through its phases.
                if {t.last_narrowed, name} == {"push", "recall"}:
                    t.alternations += 1
                t.last_narrowed = name
            else:
                t.last_narrowed = None
            if frame.tick > TAIL_START:
                t.tail_n += 1
                t.tail_idle += int(plan[3] == 25)
                for action, name in (
                    (18, "push"),
                    (17, "form"),
                    (19, "recall"),
                    (20, "scout"),
                    (41, "lift"),
                ):
                    if view.mask[action]:
                        t.tail_legal[name] += 1
                t.tail_width += int(view.mask.sum())
                t.tail_mine += int(view.raw[F["my_strength"]])
                t.tail_seen += int(view.raw[F["seen_strength"]])
                t.tail_scrap += int(view.raw[F["scrap"]])
                t.tail_harv += int(view.raw[F["my_harvesters"]])
                t.tail_idle_harv += int(view.raw[F["idle_harvesters"]])
            acts[seat] = plan
        frame = worker.step(acts)
    survivors = set(frame.alive or [])
    for seat, t in seats.items():
        t.alive = seat in survivors
    if record_dir is not None and frame.replay is not None:
        record_dir.mkdir(parents=True, exist_ok=True)
        out = record_dir / f"audit-{scenario.stem}-s{seed}.json"
        out.write_text(json.dumps(frame.replay))
        print(f"  replay: {out}")
    return GameTrace(
        map_name=scenario.stem,
        seed=seed,
        winner=frame.winner,
        tick=frame.tick,
        seats=seats,
    )


def scenario_info(path: pathlib.Path) -> tuple[int, str]:
    """Seat count and game mode from the scenario's authored data."""
    payload = json.loads(path.read_text())
    rows = payload["map"] if isinstance(payload["map"], list) else []
    seats = sum(ch.isdigit() for row in rows for ch in row)
    teams = [p.get("team") for p in payload.get("players", [])]
    distinct = {t for t in teams if t is not None}
    if seats == 2:
        mode = "1v1"
    elif not distinct:
        mode = f"ffa{seats}"
    else:
        mode = f"{len(distinct) + teams.count(None)}-team"
    return seats, mode


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--weights", required=True, help="policy checkpoint (.pt)")
    ap.add_argument("--name", required=True, help="run name under runs/audit/")
    ap.add_argument("--driver", default="../../target/release/oxide-driver")
    ap.add_argument("--scenarios", default="../../scenarios")
    ap.add_argument(
        "--maps",
        default=None,
        help="comma-separated scenario stems; omission sweeps every shipped map",
    )
    ap.add_argument(
        "--procgen",
        type=int,
        default=0,
        help="procedurally generated maps to add (players cycle 2/4/6)",
    )
    ap.add_argument(
        "--colossal",
        type=int,
        default=0,
        help="colossal (200+ per side) generated maps to add (players cycle 2/4)",
    )
    ap.add_argument(
        "--colossal-ticks",
        type=int,
        default=80_000,
        help="tick horizon for colossal games (a 200-tile march needs "
        "the longer clock)",
    )
    ap.add_argument("--seeds", type=int, default=1, help="games per map")
    ap.add_argument("--ticks", type=int, default=40_000)
    ap.add_argument(
        "--save-replays",
        default=None,
        help="directory to write full replays of every audited game, "
        "ready for the shell's replay viewer",
    )
    args = ap.parse_args()

    actor, _ = load_policy(args.weights)
    actor.eval()
    out = pathlib.Path("runs/audit") / args.name
    out.mkdir(parents=True, exist_ok=True)

    scenario_dir = pathlib.Path(args.scenarios)
    wanted = set(args.maps.split(",")) if args.maps else None
    slate: list[tuple[pathlib.Path, int]] = []
    if args.maps != "none":
        slate = [
            (path, args.ticks)
            for path in sorted(scenario_dir.glob("*.json"))
            if wanted is None or path.stem in wanted
        ]
    # Mode mix: shipped maps carry their authored modes (1v1, teams,
    # FFA); generated maps cycle player counts and the teams flag so
    # the sweep also covers team-vs-team and free-for-all shapes.
    for i in range(args.procgen):
        players, teams = ((2, False), (4, True), (6, False), (4, False))[i % 4]
        generated = mapgen.generate(
            97_000 + i,
            str(out / "procgen"),
            players=players,
            teams=teams,
            driver=args.driver,
        )
        slate.append((pathlib.Path(generated), args.ticks))
    for i in range(args.colossal):
        players, teams = ((2, False), (4, True), (4, False))[i % 3]
        generated = mapgen.generate(
            98_000 + i,
            str(out / "procgen"),
            players=players,
            teams=teams,
            driver=args.driver,
            pace="colossal",
        )
        slate.append((pathlib.Path(generated), args.colossal_ticks))

    worker = Worker(args.driver)
    games: list[GameTrace] = []
    findings: list[dict] = []
    chosen_pool: Counter = Counter()
    try:
        for path, horizon in slate:
            seat_count, mode = scenario_info(path)
            if seat_count < 2:
                continue
            for seed in range(args.seeds):
                game = play(
                    worker,
                    actor,
                    path,
                    seat_count,
                    seed,
                    horizon,
                    record_dir=pathlib.Path(args.save_replays)
                    if args.save_replays
                    else None,
                )
                game.mode = mode
                games.append(game)
                new = screen_game(game)
                findings.extend(new)
                trained: Counter = Counter()
                for t in game.seats.values():
                    chosen_pool.update(t.chosen_actions)
                    trained.update(t.chosen_actions)
                outcome = f"winner {game.winner}" if game.winner is not None else "CAP"
                print(
                    f"{game.map_name} seed {seed} [{mode}]: {outcome} at "
                    f"{game.tick} ({clock(game.tick)}) · {len(new)} finding(s) · "
                    f"trained [{head_shares(trained, PRODUCTION_IDS, top=4)}]"
                )
    finally:
        worker.close()

    unused = sorted(set(range(43)) - set(chosen_pool))
    order = Counter(f["screen"] for f in findings)
    lines = [
        "# Audit report",
        "",
        f"policy: {args.weights} · {len(games)} games · "
        f"{sum(1 for g in games if g.winner is None)} capped",
        "",
        "## Games",
        "",
        "| map | mode | outcome | clock | trained |",
        "|---|---|---|---|---|",
    ]
    for g in games:
        result = f"winner {g.winner}" if g.winner is not None else "CAP"
        trained = Counter()
        for t in g.seats.values():
            trained.update(t.chosen_actions)
        lines.append(
            f"| {g.map_name} | {g.mode} | {result} | {clock(g.tick)} | "
            f"{head_shares(trained, PRODUCTION_IDS, top=4)} |"
        )
    lines += [
        "",
        "## Findings by screen",
        "",
    ]
    for screen, count in order.most_common():
        lines.append(f"### {screen} ({count})")
        for f in findings:
            if f["screen"] == screen:
                seat = f"seat {f['seat']}" if f["seat"] is not None else "all"
                lines.append(f"- {f['map']} seed {f['seed']} {seat}: {f['evidence']}")
        lines.append("")
    lines.append("## Pool-wide unit mix (production choices)")
    lines.append("")
    lines.append(head_shares(chosen_pool, PRODUCTION_IDS))
    lines.append("")
    lines.append("## Pool-wide construction mix")
    lines.append("")
    lines.append(head_shares(chosen_pool, CONSTRUCTION_IDS))
    lines.append("")
    lines.append("## Actions never chosen pool-wide")
    lines.append("")
    lines.append(", ".join(ACTION_NAMES[i] for i in unused) or "none")
    lines.append("")
    report = out / "report.md"
    report.write_text("\n".join(lines))
    (out / "findings.json").write_text(json.dumps(findings, indent=1))
    print(f"report: {report} · {len(findings)} findings")


if __name__ == "__main__":
    main()
