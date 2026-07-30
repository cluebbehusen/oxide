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
        }
        assert len(names) == 5, f"cache identities collided: {sorted(names)}"


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
