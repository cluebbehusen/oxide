"""The fun gate's judgment, pinned: spam fails, a real mix passes, and
the two tech clauses catch two different failures."""

import math

import fun_gate

MIN_ENTROPY = 1.8
MIN_SEAT_ENTROPY = 1.45
MIN_COUNT_ENTROPY = 2.05
MIN_SEAT_COUNT_ENTROPY = 1.45
MAX_COUNT_DOMINANCE = 0.60
MAX_MEAN_COUNT_SHARE = 0.40
MIN_DECIDED_RATE = 0.70
MIN_TECH = 0.25
MIN_TOP_TECH = 0.03


def mix_entropy(shares: dict[str, float]) -> float:
    return -sum(p * math.log2(p) for p in shares.values() if p > 0)


def cohort(
    shares: dict[str, float], count_shares: dict[str, float] | None = None
) -> dict:
    count_shares = count_shares or shares
    value_entropy = mix_entropy(shares)
    count_entropy = mix_entropy(count_shares)
    return {
        "mean_share": shares,
        "entropy_bits": value_entropy,
        "seat_entropy": {"p10": value_entropy},
        "mean_count_share": count_shares,
        "count_entropy_bits": count_entropy,
        "seat_count_entropy": {"p10": count_entropy},
        "seat_count_dominance": {"p90": max(count_shares.values())},
    }


def verdict(
    shares: dict[str, float],
    count_shares: dict[str, float] | None = None,
    **kwargs: float,
) -> list[str]:
    return verdict_for(cohort(shares, count_shares), **kwargs)


def verdict_for(data: dict, **kwargs: float) -> list[str]:
    dials = {
        "decided_rate": 1.0,
        "min_decided_rate": MIN_DECIDED_RATE,
        "min_entropy": MIN_ENTROPY,
        "min_seat_entropy": MIN_SEAT_ENTROPY,
        "min_count_entropy": MIN_COUNT_ENTROPY,
        "min_seat_count_entropy": MIN_SEAT_COUNT_ENTROPY,
        "max_count_dominance": MAX_COUNT_DOMINANCE,
        "max_mean_count_share": MAX_MEAN_COUNT_SHARE,
        "min_tech_share": MIN_TECH,
        "min_top_tech_share": MIN_TOP_TECH,
    } | kwargs
    return fun_gate.judge(data, **dials)


def test_sentinel_spam_fails_every_way() -> None:
    failures = verdict({"sentinel": 0.55, "harvester": 0.45})
    assert any("mix entropy" in failure for failure in failures)
    assert any("body-time entropy" in failure for failure in failures)
    assert any("tree was never climbed" in failure for failure in failures)


def test_a_real_mix_opens_the_gate() -> None:
    shares = {
        "sentinel": 0.30,
        "harvester": 0.25,
        "lancer": 0.15,
        "bombard": 0.15,
        "flakhound": 0.10,
        "buzzard": 0.05,
    }
    assert verdict(shares) == []


def test_the_tech_clauses_fire_even_when_entropy_is_satisfied() -> None:
    # Only sentinel and harvester sit outside the Fabricator gate, so a
    # basics-only army can pass a lax entropy bar yet must still fail
    # the climb.
    failures = verdict({"sentinel": 0.55, "harvester": 0.45}, min_entropy=0.9)
    assert sum("tree" in failure for failure in failures) == 2


def test_a_thin_spread_over_the_whole_tree_climbs_nothing() -> None:
    # The failure the summed rule alone cannot see: every tech kind
    # fielded, none of them ever worth building. The sum clears its bar
    # on breadth; no single kind clears three percent.
    shares = {"sentinel": 0.35, "harvester": 0.335} | dict.fromkeys(
        sorted(fun_gate.TECH_KINDS), 0.035
    )
    failures = verdict(shares, min_top_tech_share=0.05)
    assert sum("worth building" in failure for failure in failures) == 1


def test_one_deep_tech_kind_clears_the_top_rule_but_not_the_sum() -> None:
    # And the mirror failure: the tree was climbed to exactly one thing.
    # The top rule opens, the sum stays shut.
    shares = {"sentinel": 0.50, "harvester": 0.40, "darter": 0.10}
    failures = verdict(shares, min_entropy=0.9)
    assert sum("never climbed" in failure for failure in failures) == 1


def test_body_count_catches_scuttlers_hidden_by_value_mix() -> None:
    value = {
        "sentinel": 0.20,
        "harvester": 0.20,
        "scuttler": 0.20,
        "lancer": 0.20,
        "bombard": 0.20,
    }
    bodies = {
        "sentinel": 0.05,
        "harvester": 0.05,
        "scuttler": 0.80,
        "lancer": 0.05,
        "bombard": 0.05,
    }
    failures = verdict(value, bodies)
    assert not any("mix entropy" in failure for failure in failures)
    assert any("one unit dominates over time" in failure for failure in failures)
    assert any("dominates the slate" in failure for failure in failures)


def test_per_seat_body_gates_are_independent_of_the_mean_mix() -> None:
    shares = {
        "sentinel": 0.20,
        "harvester": 0.20,
        "scuttler": 0.20,
        "lancer": 0.20,
        "bombard": 0.20,
    }
    data = cohort(shares)
    data["seat_count_entropy"]["p10"] = 1.44
    data["seat_count_dominance"]["p90"] = 0.61
    failures = verdict_for(data)
    assert any("per-seat body-time entropy" in failure for failure in failures)
    assert any("dominates over time" in failure for failure in failures)


def test_body_share_boundaries_are_inclusive() -> None:
    shares = {
        "scuttler": 0.40,
        "sentinel": 0.15,
        "harvester": 0.15,
        "lancer": 0.15,
        "bombard": 0.15,
    }
    data = cohort(shares)
    data["seat_count_dominance"]["p90"] = 0.60
    failures = verdict_for(data)
    assert not any("dominates over time" in failure for failure in failures)
    assert not any("dominates the slate" in failure for failure in failures)


def test_stalls_fail_even_with_a_varied_army() -> None:
    shares = {
        "sentinel": 0.20,
        "harvester": 0.20,
        "scuttler": 0.20,
        "lancer": 0.20,
        "bombard": 0.20,
    }
    failures = verdict(shares, decided_rate=0.62)
    assert any("too many stalls" in failure for failure in failures)
