"""The fun gate's judgment, pinned: spam fails, a real mix passes."""

import fun_gate


def overall(shares):
    import math
    entropy = -sum(p * math.log2(p) for p in shares.values() if p > 0)
    return {"mean_share": shares, "entropy_bits": entropy}


def test_sentinel_spam_fails_both_ways():
    failures = fun_gate.judge(
        overall({"sentinel": 0.55, "harvester": 0.45}), 1.8, 0.03
    )
    assert len(failures) == 2, failures


def test_a_real_mix_opens_the_gate():
    shares = {
        "sentinel": 0.30,
        "harvester": 0.25,
        "lancer": 0.15,
        "bombard": 0.15,
        "flakhound": 0.10,
        "buzzard": 0.05,
    }
    assert fun_gate.judge(overall(shares), 1.8, 0.03) == []


def test_the_tech_clause_fires_even_when_entropy_is_satisfied():
    # Only sentinel and harvester sit outside the Fabricator gate, so a
    # basics-only army can pass a lax entropy bar yet must still fail
    # the climb.
    shares = {"sentinel": 0.55, "harvester": 0.45}
    failures = fun_gate.judge(overall(shares), 0.9, 0.03)
    assert failures and all("tree" in f for f in failures)
