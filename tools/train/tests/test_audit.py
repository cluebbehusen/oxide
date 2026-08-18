"""The auditor's pure anomaly screens."""

from collections import Counter

from audit import GameTrace, SeatTrace, screen_game


def trace(**kwargs: int | None) -> SeatTrace:
    t = SeatTrace()
    for key, value in kwargs.items():
        setattr(t, key, value)
    return t


def screens(game: GameTrace) -> set[str]:
    return {f["screen"] for f in screen_game(game)}


def test_idle_dominant_fires_only_undecided_and_dominant() -> None:
    seat = trace(tail_n=100, tail_idle=90, tail_mine=100_000, tail_seen=1_000)
    capped = GameTrace("m", 0, None, 40_000, {0: seat})
    assert "IDLE_DOMINANT" in screens(capped)
    decided = GameTrace("m", 0, 1, 30_000, {0: seat})
    assert "IDLE_DOMINANT" not in screens(decided)


def test_never_expands_needs_a_long_game() -> None:
    seat = trace(max_foundries=1)
    long_game = GameTrace("m", 0, None, 30_000, {0: seat})
    assert "NEVER_EXPANDS" in screens(long_game)
    short_game = GameTrace("m", 0, 1, 8_000, {0: seat})
    assert "NEVER_EXPANDS" not in screens(short_game)
    expanded = GameTrace("m", 0, None, 30_000, {0: trace(max_foundries=2)})
    assert "NEVER_EXPANDS" not in screens(expanded)


def test_starved_economy_reads_idle_harvesters() -> None:
    seat = trace(tail_n=100, tail_harv=800, tail_idle_harv=700)
    game = GameTrace("m", 0, None, 40_000, {0: seat})
    assert "ECONOMY_STARVED" in screens(game)


def test_oscillator_and_frozen_menu() -> None:
    seat = trace(alternations=25, tail_n=100, tail_width=400)
    game = GameTrace("m", 0, None, 40_000, {0: seat})
    named = screens(game)
    assert "OSCILLATOR" in named
    assert "FROZEN_MENU" in named


def test_discovery_fail_only_when_never_known() -> None:
    blind = trace(tail_n=10)
    game = GameTrace("m", 0, None, 40_000, {0: blind})
    assert "DISCOVERY_FAIL" in screens(game)
    sighted = trace(tail_n=10, site_known_at=5_000)
    game = GameTrace("m", 0, None, 40_000, {0: sighted})
    assert "DISCOVERY_FAIL" not in screens(game)


def test_z_passive_giant_fires_even_when_the_game_decided() -> None:
    giant = trace(decisions=1000, max_mine=1200)
    giant.ops = Counter({"idle": 950, "scout": 45, "push": 5})
    decided = GameTrace("m", 0, 1, 78_000, {1: giant})
    assert "PASSIVE_GIANT" in screens(decided)
    fighter = trace(decisions=1000, max_mine=1200)
    fighter.ops = Counter({"idle": 700, "push": 300})
    healthy = GameTrace("m", 0, 1, 78_000, {1: fighter})
    assert "PASSIVE_GIANT" not in screens(healthy)
