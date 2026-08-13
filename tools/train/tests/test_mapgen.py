"""Tests for ``mapgen``: the size-class draws, and the grand pace bias
the round-8 pacing curriculum trains on."""

from __future__ import annotations

import json
import pathlib
import subprocess
from typing import TYPE_CHECKING

from mapgen import _carve, cache_name, generate

if TYPE_CHECKING:
    import pytest


def dims(candidate: dict) -> tuple[int, int]:
    grid = candidate["map"]
    return len(grid[0]), len(grid)


class TestGrandPace:
    def test_grand_draws_only_the_big_classes(self) -> None:
        # The pacing curriculum exists because small maps end games
        # before tech amortizes; a single quick-class draw would leak
        # exactly the lesson the pool is built to unlearn.
        for seed in range(40):
            w, h = dims(_carve(seed, pace="grand"))
            assert w >= 50, f"seed {seed}: width {w} is not a big class"
            assert h >= 30, f"seed {seed}: height {h} is not a big class"

    def test_grand_is_deterministic_per_seed(self) -> None:
        for seed in (0, 7, 31):
            assert _carve(seed, pace="grand") == _carve(seed, pace="grand")

    def test_the_default_draw_still_spans_small_classes(self) -> None:
        # The unbiased pool must keep its quick/standard weight — the
        # grand bias is a separate cache, not a rewrite of the default
        # curriculum (old runs must stay reproducible from their seeds).
        smallest = min(dims(_carve(seed))[0] for seed in range(40))
        assert smallest < 50, "the default draw lost its small classes"

    def test_default_carve_is_untouched_by_the_pace_parameter(self) -> None:
        for seed in (3, 11):
            assert _carve(seed) == _carve(seed, pace=None)


class TestCacheName:
    def test_every_drawing_input_reaches_the_key(self) -> None:
        # A grand request once returned the plain map cached under the
        # same seed in a shared directory — the pace bias must be part
        # of cache identity like players and teams already are.
        names = {
            cache_name(7, 2, False, None),
            cache_name(7, 2, False, "grand"),
            cache_name(7, 4, False, None),
            cache_name(7, 4, True, None),
            cache_name(8, 2, False, None),
            cache_name(7, 2, False, "island"),
            cache_name(7, 8, True, None),
        }
        assert len(names) == 7, f"cache identities collided: {sorted(names)}"


def test_cached_maps_are_bound_to_generator_and_validator_content(
    tmp_path: pathlib.Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    validations: list[str] = []

    def accept(
        command: list[str], **_kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        validations.append(command[0])
        return subprocess.CompletedProcess(command, 0)

    monkeypatch.setattr(subprocess, "run", accept)
    first_driver = tmp_path / "driver-a"
    second_driver = tmp_path / "driver-b"
    first_driver.write_bytes(b"sim-a")
    second_driver.write_bytes(b"sim-b")

    first_path = pathlib.Path(
        generate(7, str(tmp_path / "maps"), driver=str(first_driver))
    )
    assert json.loads(first_path.read_text()) == _carve(7)

    stale = json.loads(first_path.read_text())
    stale["name"] = "stale-generator-output"
    first_path.write_text(json.dumps(stale))
    repaired_path = pathlib.Path(
        generate(7, str(tmp_path / "maps"), driver=str(first_driver))
    )
    assert repaired_path == first_path
    assert json.loads(repaired_path.read_text()) == _carve(7)

    second_path = pathlib.Path(
        generate(7, str(tmp_path / "maps"), driver=str(second_driver))
    )
    assert second_path != first_path
    assert validations == [str(first_driver), str(first_driver), str(second_driver)]


class TestIslandPace:
    def test_island_draws_only_the_big_classes(self) -> None:
        # A guaranteed gulf needs room for two whole economies and an
        # air war on each side of it.
        for seed in range(30):
            w, h = dims(_carve(seed, pace="island"))
            assert w >= 50 and h >= 30, f"seed {seed}: {w}x{h} is not a big class"

    def test_the_gulf_severs_every_ground_route(self) -> None:
        # The family exists to TEACH the severed war; a single bridged
        # draw would leak ground rushes back into the island lessons.
        from collections import deque

        for seed in range(30):
            grid = _carve(seed, pace="island")["map"]
            w, h = len(grid[0]), len(grid)
            blocked = set("#^~")
            seen = [[False] * w for _ in range(h)]
            queue = deque()
            for x in range(w):
                if grid[0][x] not in blocked:
                    seen[0][x] = True
                    queue.append((0, x))
            while queue:
                y, x = queue.popleft()
                for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                    ny, nx = y + dy, x + dx
                    if (
                        0 <= ny < h
                        and 0 <= nx < w
                        and not seen[ny][nx]
                        and grid[ny][nx] not in blocked
                    ):
                        seen[ny][nx] = True
                        queue.append((ny, nx))
            crossed = any(seen[h - 1][x] or seen[h - 2][x] for x in range(w))
            assert not crossed, f"seed {seed}: a ground route survived the gulf"

    def test_island_is_deterministic_per_seed(self) -> None:
        for seed in (0, 7, 31):
            assert _carve(seed, pace="island") == _carve(seed, pace="island")


class TestEightPlayers:
    def test_anchors_teams_and_factions(self) -> None:
        for seed in range(20):
            m = _carve(seed, players=8, teams=True)
            chars = "".join(m["map"])
            for anchor in "12345678":
                assert chars.count(anchor) == 1, f"seed {seed}: anchor {anchor}"
            assert [p["team"] for p in m["players"]] == [0, 1] * 4
            assert [p["faction"] for p in m["players"]] == ["ferrous", "cupric"] * 4
            assert len({p["name"] for p in m["players"]}) == 8

    def test_every_seat_spawns_its_units(self) -> None:
        for seed in range(20):
            m = _carve(seed, players=8, teams=True)
            by_player: dict[int, int] = {}
            for unit in m["units"]:
                by_player[unit["player"]] = by_player.get(unit["player"], 0) + 1
            assert set(by_player) == set(range(8)), f"seed {seed}: {sorted(by_player)}"
            assert len(set(by_player.values())) == 1, f"seed {seed}: uneven spawns"

    def test_the_four_player_draw_is_untouched(self) -> None:
        # The 2v2 and 4-FFA caches predate the eight-player arm; their
        # seeds must keep reconstructing byte-identical maps.
        for seed in (3, 11, 19):
            assert _carve(seed, players=4, teams=True) == _carve(
                seed, players=4, teams=True
            )


class TestSymmetryProperties:
    """The docstrings promise mirrored fairness; nothing asserted it.

    Anchors and derelict frames are 2x2 footprints, so the mirrored
    grid is compared after expanding every footprint char across its
    tiles — the byte sits at the top-left, its image at the mirrored
    footprint's top-left, and a raw byte-for-byte compare would wrongly
    demand the image byte sit at the mirrored BYTE position.
    """

    @staticmethod
    def _footprint_mask(grid: list[str]) -> list[list[str]]:
        h, w = len(grid), len(grid[0])
        out = [["." for _ in range(w)] for _ in range(h)]
        for y in range(h):
            for x in range(w):
                c = grid[y][x]
                if c in "12345678":
                    for dy in range(2):
                        for dx in range(2):
                            if y + dy < h and x + dx < w:
                                out[y + dy][x + dx] = "F"
                elif c == "E":
                    for dy in range(2):
                        for dx in range(2):
                            if y + dy < h and x + dx < w:
                                out[y + dy][x + dx] = "E"
                elif out[y][x] == ".":
                    out[y][x] = c
        return out

    def test_two_player_terrain_mirrors_at_180_degrees(self) -> None:
        for seed in range(20):
            for pace in (None, "grand", "island"):
                grid = _carve(seed, pace=pace)["map"]
                mask = self._footprint_mask(grid)
                h, w = len(mask), len(mask[0])
                for y in range(h):
                    for x in range(w):
                        assert mask[y][x] == mask[h - 1 - y][w - 1 - x], (
                            f"seed {seed} pace={pace}: ({x},{y}) breaks the mirror"
                        )

    def test_two_player_spawns_mirror_entry_by_entry(self) -> None:
        for seed in range(20):
            m = _carve(seed)
            grid = m["map"]
            h, w = len(grid), len(grid[0])
            p0 = [u for u in m["units"] if u["player"] == 0]
            p1 = [u for u in m["units"] if u["player"] == 1]
            assert len(p0) == len(p1), f"seed {seed}: uneven rosters"
            for a, b in zip(p0, p1, strict=True):
                assert a["kind"] == b["kind"], f"seed {seed}: role mismatch"
                assert (b["x"], b["y"]) == (w - 1 - a["x"], h - 1 - a["y"]), (
                    f"seed {seed}: spawn {a} mirrors to {b}"
                )

    def test_quadrant_maps_reflect_every_corner(self) -> None:
        for seed in range(12):
            for players in (4, 8):
                m = _carve(seed, players=players, teams=True)
                mask = self._footprint_mask(m["map"])
                h, w = len(mask), len(mask[0])
                for y in range(h):
                    for x in range(w):
                        for iy, ix in (
                            (y, w - 1 - x),
                            (h - 1 - y, x),
                            (h - 1 - y, w - 1 - x),
                        ):
                            assert mask[y][x] == mask[iy][ix], (
                                f"seed {seed} {players}p: ({x},{y}) breaks a reflection"
                            )

    def test_quadrant_spawns_mirror_per_corner(self) -> None:
        for seed in range(12):
            m = _carve(seed, players=4, teams=True)
            grid = m["map"]
            h, w = len(grid), len(grid[0])
            by_player: dict[int, list[dict]] = {}
            for u in m["units"]:
                by_player.setdefault(u["player"], []).append(u)
            base = by_player[0]
            images = [
                lambda x, y: (x, y),
                lambda x, y: (w - 1 - x, y),
                lambda x, y: (x, h - 1 - y),
                lambda x, y: (w - 1 - x, h - 1 - y),
            ]
            for player, image in enumerate(images):
                for a, b in zip(base, by_player[player], strict=True):
                    assert a["kind"] == b["kind"]
                    assert (b["x"], b["y"]) == image(a["x"], a["y"]), (
                        f"seed {seed}: corner {player} spawn {b} is not {a}'s image"
                    )
