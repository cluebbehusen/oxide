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

from __future__ import annotations

import json
import pathlib
import subprocess

import numpy as np

DRIVER = "../../target/release/oxide-driver"


def _carve(seed: int, players: int = 2) -> dict:
    rng = np.random.default_rng(seed)
    w = int(rng.integers(30, 46))
    h = int(rng.integers(18, 28))
    if players == 4:
        return _carve4(rng, seed, w, h)
    grid = [["." for _ in range(w)] for _ in range(h)]

    def mirror(x: int, y: int) -> tuple[int, int]:
        return w - 1 - x, h - 1 - y

    def set_pair(x: int, y: int, ch: str):
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
    # away from both bases.
    blobs = int(rng.integers(4, 9))
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
    for (x, y), kind in zip(spots, kinds):
        units.append({"player": 0, "kind": kind, "x": x, "y": y})
    for (x, y), kind in zip(spots, kinds):
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


def _carve4(rng, seed: int, w: int, h: int) -> dict:
    """Four-player maps by double mirroring: author the top-left
    quadrant, reflect across both axes — every corner seat plays the
    same quadrant. Anchor characters 1-4; spawn lists are emitted in
    the same reflected order per seat."""
    grid = [["." for _ in range(w)] for _ in range(h)]

    def images(x: int, y: int):
        return [
            (x, y),
            (w - 1 - x, y),
            (x, h - 1 - y),
            (w - 1 - x, h - 1 - y),
        ]

    def set_all(x: int, y: int, ch: str):
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
        for (x, y), kind in zip(spots, kinds):
            ix, iy = images(x, y)[player]
            units.append({"player": player, "kind": kind, "x": ix, "y": iy})

    factions = ["ferrous", "cupric", "ferrous", "cupric"]
    return {
        "name": f"generated4-{seed}",
        "seed": seed,
        "map": ["".join(row) for row in grid],
        "players": [
            {"name": f"Seat{p}", "faction": factions[p], "scrap": 150, "bot": False}
            for p in range(4)
        ],
        "units": units,
    }


def generate(seed: int, out_dir: str, players: int = 2, driver: str = DRIVER) -> str:
    """Writes a validated scenario for `seed` and returns its path.
    Same seed, same file. Random rock blobs carry no connectivity
    guarantee, so every candidate is checked against the real sim
    (`Scenario::build` rejects sealed maps) before it is cached —
    a bad draw retries deterministically on a derived seed, and only
    validated files ever land in the cache."""
    out = pathlib.Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    path = out / f"gen{players if players != 2 else ''}-{seed}.json"
    if path.exists():
        return str(path)
    for attempt in range(16):
        candidate = _carve(seed + attempt * 10_000_019, players)
        trial = path.with_suffix(".candidate.json")
        trial.write_text(json.dumps(candidate))
        ok = (
            subprocess.run(
                [driver, "run", str(trial), "--ticks", "0"],
                capture_output=True,
            ).returncode
            == 0
        )
        if ok:
            trial.rename(path)
            return str(path)
        trial.unlink(missing_ok=True)
    raise RuntimeError(f"no valid map within 16 attempts of seed {seed}")
