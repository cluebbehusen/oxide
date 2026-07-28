"""The fun gate's judgment, pinned: spam fails, a real mix passes, and
the two tech clauses catch two different failures."""

import math

import fun_gate

MIN_ENTROPY = 1.8
MIN_TECH = 0.25
MIN_TOP_TECH = 0.03


def cohort(shares: dict[str, float]) -> dict:
    entropy = -sum(p * math.log2(p) for p in shares.values() if p > 0)
    return {"mean_share": shares, "entropy_bits": entropy}


def verdict(shares: dict[str, float], **kwargs: float) -> list[str]:
    dials = {
        "min_entropy": MIN_ENTROPY,
        "min_tech_share": MIN_TECH,
        "min_top_tech_share": MIN_TOP_TECH,
    } | kwargs
    return fun_gate.judge(cohort(shares), **dials)


def test_sentinel_spam_fails_every_way() -> None:
    failures = verdict({"sentinel": 0.55, "harvester": 0.45})
    assert len(failures) == 3, failures


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
    assert len(failures) == 2
    assert all("tree" in f for f in failures)


def test_a_thin_spread_over_the_whole_tree_climbs_nothing() -> None:
    # The failure the summed rule alone cannot see: every tech kind
    # fielded, none of them ever worth building. The sum clears its bar
    # on breadth; no single kind clears three percent.
    shares = {"sentinel": 0.35, "harvester": 0.335} | dict.fromkeys(
        sorted(fun_gate.TECH_KINDS), 0.035
    )
    failures = verdict(shares, min_top_tech_share=0.05)
    assert len(failures) == 1
    assert "worth building" in failures[0]


def test_one_deep_tech_kind_clears_the_top_rule_but_not_the_sum() -> None:
    # And the mirror failure: the tree was climbed to exactly one thing.
    # The top rule opens, the sum stays shut.
    shares = {"sentinel": 0.50, "harvester": 0.40, "darter": 0.10}
    failures = verdict(shares, min_entropy=0.9)
    assert len(failures) == 1
    assert "never climbed" in failures[0]
