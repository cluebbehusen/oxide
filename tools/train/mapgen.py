"""Procedural 180°-symmetric skirmish maps.

Distinguishes "good at Oxide" from "learned the map": evaluation on
maps no model has seen, and (for the generalist run) training where
every episode is a fresh world. Layout honors the repo's authoring
rules — author the top half, mirror the rest, so both seats play the
same map rotated. Spawn units are emitted mirror-ordered (the 0.7
seat-fairness lesson: p1's list must be the exact mirror of p0's,
entry by entry).

Generation is deterministic per seed; scenario validation (reachable
doorsteps, sealed maps) is enforced by retrying seeds that fail to
build — the caller gets a path to a scenario JSON that loads.

Usage:
    from mapgen import generate
    path = generate(seed=7, out_dir="/tmp/maps")
"""

import json
import os
import pathlib
import subprocess
import tempfile
import threading

import numpy as np

# Bump when _carve's output distribution changes (sizes, terrain
# alphabet, densities): cache identity is schema + mode + seed.
MAPGEN_SCHEMA = 3

DRIVER = "../../target/release/oxide-driver"


def cache_dir(name: str) -> str:
    """A per-purpose map cache under the system temp directory."""
    return str(pathlib.Path(tempfile.gettempdir()) / name)


def _carve(seed: int, players: int = 2, teams: bool = False) -> dict:
    rng = np.random.default_rng(seed)
    # Size classes: the v4 schema rides relative coordinates, so the
    # curriculum must actually vary the field. Quick, standard, and a
    # large stretch that exercises the 0-1000 range like the shipped
    # Ferric Reach class does.
    # Schema 3 (0.10): the vast class joins the draw — the pacing work
    # aims matches at tens of minutes, and the curriculum has to teach
    # marches that long or the ladder never fights them well.
    roll = rng.random()
    if roll < 0.20:
        w, h = int(rng.integers(26, 36)), int(rng.integers(16, 24))
    elif roll < 0.60:
        w, h = int(rng.integers(36, 50)), int(rng.integers(22, 32))
    elif roll < 0.85:
        w, h = int(rng.integers(50, 64)), int(rng.integers(30, 40))
    else:
        w, h = int(rng.integers(84, 108)), int(rng.integers(48, 64))
    if players == 4:
        # Four bases need more floor: widen the draw a class.
        w, h = int(w * 1.3), int(h * 1.3)
        return _carve4(rng, seed, w, h, teams)
    grid = [["." for _ in range(w)] for _ in range(h)]

    def mirror(x: int, y: int) -> tuple[int, int]:
        return w - 1 - x, h - 1 - y

    def set_pair(x: int, y: int, ch: str) -> None:
        grid[y][x] = ch
        mx, my = mirror(x, y)
        grid[my][mx] = ch

    # Border.
    for x in range(w):
        set_pair(x, 0, "#")
    for y in range(h):
        set_pair(0, y, "#")

    # Foundry anchors: top-left quadrant for p0, mirrored for p1. The
    # '2' marks the anchor of the mirrored 2x2 footprint, so it sits at
    # mirror(anchor) - (1, 1).
    ax = int(rng.integers(3, max(4, w // 4)))
    ay = int(rng.integers(3, max(4, h // 4)))
    grid[ay][ax] = "1"
    mx, my = mirror(ax, ay)
    grid[my - 1][mx - 1] = "2"

    # Rock formations: blobs authored in the top half, mirrored, kept
    # away from both bases. Density scales with floor area so large
    # fields don't come out empty.
    blobs = int(rng.integers(4, 9)) * max(1, (w * h) // 1100)
    for _ in range(blobs):
        cx = int(rng.integers(2, w - 2))
        cy = int(rng.integers(2, h // 2 + 1))
        if abs(cx - ax) + abs(cy - ay) < 7:
            continue
        if abs(cx - (mx - 1)) + abs(cy - (my - 1)) < 7:
            continue
        size = int(rng.integers(2, 6))
        for _ in range(size * 2):
            dx, dy = int(rng.integers(-2, 3)), int(rng.integers(-1, 2))
            x, y = cx + dx, cy + dy
            if 1 < x < w - 2 and 1 < y < h - 2 and grid[y][x] == ".":
                set_pair(x, y, "#")

    # Peak ridges, sometimes: short mirrored segments of '^' that block
    # ground, air, and artillery arcs alike — the curriculum's exposure
    # to siege-safe geography. Validation (a driver build per candidate)
    # rejects any draw that seals the seats apart.
    if rng.random() < 0.4:
        ridges = int(rng.integers(1, 3))
        for _ in range(ridges):
            cx = int(rng.integers(4, w - 4))
            cy = int(rng.integers(2, h // 2 + 1))
            if abs(cx - ax) + abs(cy - ay) < 8:
                continue
            if abs(cx - (mx - 1)) + abs(cy - (my - 1)) < 8:
                continue
            dx, dy = [(1, 0), (0, 1), (1, 1), (1, -1)][int(rng.integers(0, 4))]
            length = int(rng.integers(4, 9))
            for i in range(length):
                x, y = cx + dx * i, cy + dy * i
                if 1 < x < w - 2 and 1 < y < h - 2 and grid[y][x] == ".":
                    set_pair(x, y, "^")

    # Scrap: a home cluster near each base (mirrored) plus contested
    # center nodes, rich ones sometimes.
    home_nodes = int(rng.integers(3, 5))
    placed = 0
    for _ in range(40):
        if placed >= home_nodes:
            break
        dx, dy = int(rng.integers(-4, 5)), int(rng.integers(-4, 5))
        x, y = ax + 1 + dx, ay + 1 + dy
        if (
            1 < x < w - 2
            and 1 < y < h - 2
            and grid[y][x] == "."
            and abs(dx) + abs(dy) >= 2
        ):
            set_pair(x, y, "s")
            placed += 1
    center_nodes = int(rng.integers(1, 4))
    for _ in range(center_nodes):
        dx, dy = int(rng.integers(-3, 4)), int(rng.integers(-2, 3))
        x, y = w // 2 + dx, h // 2 + dy
        if 1 < x < w - 2 and 1 < y < h - 2 and grid[y][x] == ".":
            ch = "S" if rng.random() < 0.5 else "s"
            set_pair(x, y, ch)

    # Starting units near each base, mirror-ordered entry by entry.
    units = []
    spots = []
    for _ in range(60):
        if len(spots) >= 4:
            break
        dx, dy = int(rng.integers(-3, 5)), int(rng.integers(-3, 5))
        x, y = ax + 1 + dx, ay + 1 + dy
        if (
            1 < x < w - 2
            and 1 < y < h - 2
            and grid[y][x] == "."
            and not (ax - 1 <= x <= ax + 2 and ay - 1 <= y <= ay + 2)
            and (x, y) not in spots
        ):
            spots.append((x, y))
    kinds = ["harvester", "harvester", "harvester", "sentinel"]
    for (x, y), kind in zip(spots, kinds, strict=False):
        units.append({"player": 0, "kind": kind, "x": x, "y": y})
    for (x, y), kind in zip(spots, kinds, strict=False):
        mx2, my2 = mirror(x, y)
        units.append({"player": 1, "kind": kind, "x": mx2, "y": my2})

    return {
        "name": f"generated-{seed}",
        "seed": seed,
        "map": ["".join(row) for row in grid],
        "players": [
            {"name": "Ferrous", "faction": "ferrous", "scrap": 150, "bot": False},
            {"name": "Cupric", "faction": "cupric", "scrap": 150, "bot": False},
        ],
        "units": units,
    }


def _carve4(
    rng: np.random.Generator, seed: int, w: int, h: int, teams: bool = False
) -> dict:
    """Four-player maps by double mirroring: author the top-left
    quadrant, reflect across both axes — every corner seat plays the
    same quadrant. Anchor characters 1-4; spawn lists are emitted in
    the same reflected order per seat. With `teams`, the west column
    (seats 0, 2) faces the east column (seats 1, 3) — reflection makes
    the pairing fair from every corner."""
    grid = [["." for _ in range(w)] for _ in range(h)]

    def images(x: int, y: int) -> list[tuple[int, int]]:
        return [
            (x, y),
            (w - 1 - x, y),
            (x, h - 1 - y),
            (w - 1 - x, h - 1 - y),
        ]

    def set_all(x: int, y: int, ch: str) -> None:
        for ix, iy in images(x, y):
            grid[iy][ix] = ch

    for x in range(w):
        set_all(x, 0, "#")
    for y in range(h):
        set_all(0, y, "#")

    ax = int(rng.integers(3, max(4, w // 4)))
    ay = int(rng.integers(3, max(4, h // 4)))
    # Anchors are top-left of a 2x2, so each reflected image shifts.
    grid[ay][ax] = "1"
    grid[ay][w - 2 - ax] = "2"
    grid[h - 2 - ay][ax] = "3"
    grid[h - 2 - ay][w - 2 - ax] = "4"

    for _ in range(int(rng.integers(2, 5))):
        cx = int(rng.integers(2, w // 2))
        cy = int(rng.integers(2, h // 2))
        if abs(cx - ax) + abs(cy - ay) < 6:
            continue
        for _ in range(int(rng.integers(3, 9))):
            dx, dy = int(rng.integers(-2, 3)), int(rng.integers(-1, 2))
            x, y = cx + dx, cy + dy
            if 1 < x < w - 2 and 1 < y < h - 2 and grid[y][x] == ".":
                set_all(x, y, "#")

    # Peak ridges, sometimes — quadrant-reflected like everything else,
    # so each corner seat faces the same geography.
    if rng.random() < 0.4:
        cx = int(rng.integers(4, w // 2))
        cy = int(rng.integers(3, h // 2))
        if abs(cx - ax) + abs(cy - ay) >= 8:
            dx, dy = [(1, 0), (0, 1), (1, 1)][int(rng.integers(0, 3))]
            for i in range(int(rng.integers(3, 7))):
                x, y = cx + dx * i, cy + dy * i
                if 1 < x < w - 2 and 1 < y < h - 2 and grid[y][x] == ".":
                    set_all(x, y, "^")

    placed = 0
    for _ in range(40):
        if placed >= 3:
            break
        dx, dy = int(rng.integers(-4, 5)), int(rng.integers(-4, 5))
        x, y = ax + 1 + dx, ay + 1 + dy
        if (
            1 < x < w - 2
            and 1 < y < h - 2
            and grid[y][x] == "."
            and abs(dx) + abs(dy) >= 2
        ):
            set_all(x, y, "s")
            placed += 1
    cx, cy = w // 2, h // 2
    if grid[cy][cx] == ".":
        set_all(cx, cy, "S")

    spots = []
    for _ in range(60):
        if len(spots) >= 4:
            break
        dx, dy = int(rng.integers(-3, 5)), int(rng.integers(-3, 5))
        x, y = ax + 1 + dx, ay + 1 + dy
        if (
            1 < x < w // 2 - 1
            and 1 < y < h // 2 - 1
            and grid[y][x] == "."
            and not (ax - 1 <= x <= ax + 2 and ay - 1 <= y <= ay + 2)
            and (x, y) not in spots
        ):
            spots.append((x, y))
    kinds = ["harvester", "harvester", "harvester", "sentinel"]
    units = []
    for player in range(4):
        for (x, y), kind in zip(spots, kinds, strict=False):
            ix, iy = images(x, y)[player]
            units.append({"player": player, "kind": kind, "x": ix, "y": iy})

    factions = ["ferrous", "cupric", "ferrous", "cupric"]
    # Seats reflect across x for 1|2 and 3|4, so west = seats 0, 2.
    team_of = [0, 1, 0, 1] if teams else [None] * 4
    players_spec = []
    for p in range(4):
        spec = {"name": f"Seat{p}", "faction": factions[p], "scrap": 150, "bot": False}
        if team_of[p] is not None:
            spec["team"] = team_of[p]
        players_spec.append(spec)
    return {
        "name": f"generated{'2v2' if teams else '4'}-{seed}",
        "seed": seed,
        "map": ["".join(row) for row in grid],
        "players": players_spec,
        "units": units,
    }


def generate(
    seed: int, out_dir: str, players: int = 2, teams: bool = False, driver: str = DRIVER
) -> str:
    """Writes a validated scenario for `seed` and returns its path.
    Same seed, same file. Random rock blobs carry no connectivity
    guarantee, so every candidate is checked against the real sim
    (`Scenario::build` rejects sealed maps) before it is cached —
    a bad draw retries deterministically on a derived seed, and only
    validated files ever land in the cache."""
    out = pathlib.Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    tag = "2v2" if teams else (str(players) if players != 2 else "")
    # The schema fingerprint invalidates every cached map when the
    # generator's output changes shape — without it, a retrain quietly
    # reused tens of thousands of pre-peak small-class maps whose seeds
    # matched, training on a curriculum the code no longer describes.
    path = out / f"gen{tag}-s{MAPGEN_SCHEMA}-{seed}.json"
    if path.exists():
        return str(path)
    for attempt in range(16):
        candidate = _carve(seed + attempt * 10_000_019, players, teams)
        # Unique per caller: the map warmer and a foreground reset may
        # generate the same seed concurrently, and a shared candidate
        # name lets one unlink the other's file mid-rename. Both publish
        # identical bytes, so replace semantics (not rename — Windows
        # raises FileExistsError when the loser finishes second) make
        # the race harmless on every platform.
        tag = f"{os.getpid()}-{threading.get_ident()}"
        trial = path.with_suffix(f".candidate-{tag}.json")
        trial.write_text(json.dumps(candidate))
        ok = (
            subprocess.run(
                [driver, "run", str(trial), "--ticks", "0"],
                capture_output=True,
                check=False,
            ).returncode
            == 0
        )
        if ok:
            trial.replace(path)
            return str(path)
        trial.unlink(missing_ok=True)
    raise RuntimeError(f"no valid map within 16 attempts of seed {seed}")
