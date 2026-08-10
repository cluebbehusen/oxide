"""The fun gate's promotion contract, including direct tail evidence."""

from __future__ import annotations

import copy
import json
import math
import pathlib
import subprocess
import sys

import pytest

import fun_gate

MIN_ENTROPY = 2.00
MIN_SEAT_P25_ENTROPY = 1.35
MIN_COUNT_ENTROPY = 1.95
MIN_SEAT_P25_COUNT_ENTROPY = 1.25
CATASTROPHIC_VALUE_ENTROPY = 0.75
MAX_CATASTROPHIC_VALUE_RATE = 0.075
CATASTROPHIC_COUNT_ENTROPY = 0.65
MAX_CATASTROPHIC_COUNT_RATE = 0.075
CATASTROPHIC_COUNT_DOMINANCE = 0.80
MAX_CATASTROPHIC_DOMINANCE_RATE = 0.10
MAX_MEAN_COUNT_SHARE = 0.50
MAX_UNHEALTHY_CAP_RATE = 0.10
MIN_TECH = 0.45
MIN_TOP_TECH = 0.15
MIN_FABRICATOR_REACH = 0.90
MIN_TURRET_REACH = 0.40
MIN_ARRAY_REACH = 0.60
MIN_RECLAIMER_REACH = 0.25

GOOD_SHARES = {
    "sentinel": 0.30,
    "scuttler": 0.25,
    "lancer": 0.15,
    "bombard": 0.15,
    "flakhound": 0.10,
    "buzzard": 0.05,
}

AIR_GOOD_SHARES = {
    "sentinel": 0.25,
    "scuttler": 0.20,
    "lancer": 0.15,
    "bombard": 0.15,
    "flakhound": 0.10,
    "buzzard": 0.075,
    "darter": 0.075,
}


def mix_entropy(shares: dict[str, float]) -> float:
    return -sum(p * math.log2(p) for p in shares.values() if p > 0)


def cohort(
    shares: dict[str, float],
    count_shares: dict[str, float] | None = None,
    *,
    diagnostic_shares: dict[str, float] | None = None,
) -> dict:
    count_shares = count_shares or shares
    diagnostic_shares = diagnostic_shares or shares
    value_entropy = mix_entropy(shares)
    count_entropy = mix_entropy(count_shares)
    diagnostic_entropy = mix_entropy(diagnostic_shares)
    return {
        "mean_share": diagnostic_shares,
        "entropy_bits": diagnostic_entropy,
        "seat_entropy": {"p10": diagnostic_entropy, "p25": diagnostic_entropy},
        "mean_count_share": diagnostic_shares,
        "count_entropy_bits": diagnostic_entropy,
        "seat_count_entropy": {
            "p10": diagnostic_entropy,
            "p25": diagnostic_entropy,
        },
        "seat_count_dominance": {"p90": max(diagnostic_shares.values())},
        "mean_combat_share": shares,
        "combat_entropy_bits": value_entropy,
        "seat_combat_entropy": {"p10": value_entropy, "p25": value_entropy},
        "mean_combat_count_share": count_shares,
        "combat_count_entropy_bits": count_entropy,
        "seat_combat_count_entropy": {
            "p10": count_entropy,
            "p25": count_entropy,
        },
        "seat_combat_count_dominance": {"p90": max(count_shares.values())},
        "competitive_seats_with_building": {
            "fabricator": 1.0,
            "turret": 0.50,
            "array": 0.70,
            "reclaimer": 0.30,
        },
    }


def raw_match(
    *,
    capped: bool,
    scenario: str = "skirmish",
    seed: int = 7_000,
    combat_shares: dict[str, float] | None = None,
    count_shares: dict[str, float] | None = None,
    competitive_seats: int = 2,
    ticks: int = 40_000,
    last_combat: int = 0,
    last_economy: int = 0,
    last_roster: int = 0,
    salvage: int = 0,
    reclaimers: int = 0,
    reclaimer_resigned: bool = False,
    reclaimer_foundries: int = 1,
    recovery_income_active: bool = False,
) -> dict:
    combat_shares = GOOD_SHARES if combat_shares is None else combat_shares
    count_shares = combat_shares if count_shares is None else count_shares
    value_entropy = mix_entropy(combat_shares)
    count_entropy = mix_entropy(count_shares)
    return {
        "scenario": scenario,
        "seed": seed,
        "capped": capped,
        "ticks": ticks,
        "last_progress_tick": last_roster,
        "activity": {
            "last_combat_tick": last_combat,
            "last_economy_tick": last_economy,
        },
        "combat_seats": [dict(combat_shares) for _ in range(competitive_seats)],
        "combat_entropy_bits": [value_entropy] * competitive_seats,
        "combat_count_seats": [dict(count_shares) for _ in range(competitive_seats)],
        "combat_count_entropy_bits": [count_entropy] * competitive_seats,
        "final_economy": {
            "remaining_map_salvage": salvage,
            "seats": [
                {
                    "completed_reclaimers": reclaimers,
                    "living_foundries": reclaimer_foundries,
                    "resigned": reclaimer_resigned,
                    "recovery_income_active": recovery_income_active,
                },
                {
                    "completed_reclaimers": 0,
                    "living_foundries": 1,
                    "resigned": False,
                    "recovery_income_active": False,
                },
            ],
        },
    }


def tail_rates(
    matches: list[dict],
) -> dict[str, float | int]:
    return fun_gate.combat_tail_rates(
        matches,
        CATASTROPHIC_VALUE_ENTROPY,
        CATASTROPHIC_COUNT_ENTROPY,
        CATASTROPHIC_COUNT_DOMINANCE,
    )


def verdict(
    shares: dict[str, float],
    count_shares: dict[str, float] | None = None,
    **kwargs: float,
) -> list[str]:
    return _verdict_for(cohort(shares, count_shares), None, kwargs)


def verdict_for(
    data: dict,
    *,
    tails: dict[str, float | int] | None = None,
    **kwargs: float,
) -> list[str]:
    return _verdict_for(data, tails, kwargs)


def _verdict_for(
    data: dict,
    tails: dict[str, float | int] | None,
    kwargs: dict[str, float],
) -> list[str]:
    dials = {
        "unhealthy_cap_rate": 0.0,
        "max_unhealthy_cap_rate": MAX_UNHEALTHY_CAP_RATE,
        "min_entropy": MIN_ENTROPY,
        "min_seat_p25_entropy": MIN_SEAT_P25_ENTROPY,
        "min_count_entropy": MIN_COUNT_ENTROPY,
        "min_seat_p25_count_entropy": MIN_SEAT_P25_COUNT_ENTROPY,
        "max_catastrophic_value_rate": MAX_CATASTROPHIC_VALUE_RATE,
        "max_catastrophic_count_rate": MAX_CATASTROPHIC_COUNT_RATE,
        "max_catastrophic_dominance_rate": MAX_CATASTROPHIC_DOMINANCE_RATE,
        "max_mean_count_share": MAX_MEAN_COUNT_SHARE,
        "min_tech_share": MIN_TECH,
        "min_top_tech_share": MIN_TOP_TECH,
        "min_fabricator_reach": MIN_FABRICATOR_REACH,
        "min_turret_reach": MIN_TURRET_REACH,
        "min_array_reach": MIN_ARRAY_REACH,
        "min_reclaimer_reach": MIN_RECLAIMER_REACH,
    } | kwargs
    if tails is None:
        tails = tail_rates(
            [
                raw_match(
                    capped=False,
                    combat_shares=data["mean_combat_share"],
                    count_shares=data["mean_combat_count_share"],
                )
            ]
        )
    failures = fun_gate.judge_composition(
        data,
        tails,
        dials["min_entropy"],
        dials["min_seat_p25_entropy"],
        dials["min_count_entropy"],
        dials["min_seat_p25_count_entropy"],
        dials["max_catastrophic_value_rate"],
        dials["max_catastrophic_count_rate"],
        dials["max_catastrophic_dominance_rate"],
        dials["max_mean_count_share"],
        dials["min_tech_share"],
        dials["min_top_tech_share"],
    )
    failures.extend(
        fun_gate.judge_health(
            dials["unhealthy_cap_rate"],
            dials["max_unhealthy_cap_rate"],
        )
    )
    failures.extend(
        fun_gate.judge_structures(
            data,
            dials["min_fabricator_reach"],
            dials["min_turret_reach"],
            dials["min_array_reach"],
            dials["min_reclaimer_reach"],
        )
    )
    return failures


def catastrophic_match(
    value_entropies: list[float],
    count_entropies: list[float],
    dominances: list[float],
) -> dict:
    assert len(value_entropies) == len(count_entropies) == len(dominances)
    match = raw_match(capped=False, competitive_seats=0)
    match["combat_seats"] = [{"sentinel": 0.5, "lancer": 0.5} for _ in value_entropies]
    match["combat_entropy_bits"] = value_entropies
    match["combat_count_seats"] = [
        {"scuttler": dominance, "lancer": 1.0 - dominance} for dominance in dominances
    ]
    match["combat_count_entropy_bits"] = count_entropies
    return match


def good_payload(
    *,
    seeds: int = 3,
    style: str | None = None,
    variant: int | None = None,
    aggression: int | None = None,
    fixed_profile: bool = False,
    scenario_suffix: str = "",
) -> dict:
    shares = AIR_GOOD_SHARES if style == "balanced" and variant == 1 else GOOD_SHARES
    matches = [
        raw_match(
            capped=fixed_profile or index == 9,
            scenario=f"map-{index % 2}{scenario_suffix}",
            seed=7_000 + index,
            combat_shares=shares,
        )
        for index in range(10)
    ]
    overall = {
        "matches": len(matches),
        "decided": 0 if fixed_profile else 9,
        "capped": len(matches) if fixed_profile else 1,
        "seats": 20,
        "combat_seats": 20,
        **cohort(
            shares,
            diagnostic_shares={"sentinel": 0.55, "harvester": 0.45},
        ),
    }
    if fixed_profile:
        overall["competitive_seats_with_building"] = {
            "fabricator": 0.0,
            "turret": 0.0,
            "array": 0.0,
            "reclaimer": 0.30 if style == "turtle" and variant == 1 else 0.0,
        }
    return {
        "schema": 9,
        "seeds": seeds,
        "dials": {
            "style": style,
            "variant": variant,
            "aggression": aggression,
        },
        "overall": overall,
        "matches": matches,
    }


def requested_profile(argv: list[str]) -> fun_gate.ProbeProfile:
    style = argv[argv.index("--style") + 1] if "--style" in argv else None
    variant = int(argv[argv.index("--variant") + 1]) if "--variant" in argv else None
    return next(
        profile
        for profile in fun_gate.PROFILES
        if profile.style == style and profile.variant == variant
    )


def test_sentinel_spam_fails_every_way() -> None:
    failures = verdict({"sentinel": 1.0})
    assert any("mix entropy" in failure for failure in failures)
    assert any("body-time entropy" in failure for failure in failures)
    assert any("catastrophic value-mix" in failure for failure in failures)
    assert any("catastrophic body-mix" in failure for failure in failures)
    assert any("catastrophic body-dominance" in failure for failure in failures)
    assert any("tree was never climbed" in failure for failure in failures)


def test_harvesters_cannot_inflate_the_combat_gate() -> None:
    diagnostic = {
        "harvester": 0.20,
        "sentinel": 0.20,
        "scuttler": 0.20,
        "lancer": 0.20,
        "bombard": 0.20,
    }
    data = cohort({"sentinel": 1.0}, diagnostic_shares=diagnostic)
    failures = verdict_for(data)
    assert data["entropy_bits"] > 2.0, "the all-unit diagnostic looks varied"
    assert any("mix entropy" in failure for failure in failures)
    assert any("body-time entropy" in failure for failure in failures)


def test_a_real_mix_opens_the_gate() -> None:
    assert verdict(GOOD_SHARES) == []


def test_p25_replaces_the_volatile_p10_floor() -> None:
    data = cohort(GOOD_SHARES)
    data["seat_combat_entropy"]["p10"] = 0.0
    data["seat_combat_count_entropy"]["p10"] = 0.0
    assert verdict_for(data) == []

    data["seat_combat_entropy"]["p25"] = MIN_SEAT_P25_ENTROPY - 0.01
    data["seat_combat_count_entropy"]["p25"] = MIN_SEAT_P25_COUNT_ENTROPY - 0.01
    failures = verdict_for(data)
    assert any("value entropy p25" in failure for failure in failures)
    assert any("body-time entropy p25" in failure for failure in failures)


def test_catastrophic_rates_are_counted_directly_and_boundaries_are_inclusive() -> None:
    passing = catastrophic_match(
        [0.74] * 3 + [0.75] + [1.0] * 36,
        [0.64] * 3 + [0.65] + [1.0] * 36,
        [0.81] * 4 + [0.80] + [0.5] * 35,
    )
    tails = tail_rates([passing])
    assert tails["low_value_rate"] == 0.075
    assert tails["low_count_rate"] == 0.075
    assert tails["dominant_rate"] == 0.10
    assert verdict_for(cohort(GOOD_SHARES), tails=tails) == []

    failing = catastrophic_match(
        [0.74] * 4 + [1.0] * 36,
        [0.64] * 4 + [1.0] * 36,
        [0.81] * 5 + [0.5] * 35,
    )
    failures = verdict_for(cohort(GOOD_SHARES), tails=tail_rates([failing]))
    assert any("catastrophic value-mix" in failure for failure in failures)
    assert any("catastrophic body-mix" in failure for failure in failures)
    assert any("catastrophic body-dominance" in failure for failure in failures)


def test_tail_evidence_rejects_misaligned_raw_arrays() -> None:
    match = raw_match(capped=False)
    match["combat_entropy_bits"].pop()
    with pytest.raises(RuntimeError, match="misaligned competitive combat arrays"):
        tail_rates([match])


def test_the_tech_clauses_fire_even_when_entropy_is_relaxed() -> None:
    failures = verdict(
        {"sentinel": 1.0},
        min_entropy=0.0,
        min_seat_p25_entropy=0.0,
        min_count_entropy=0.0,
        min_seat_p25_count_entropy=0.0,
        max_catastrophic_value_rate=1.0,
        max_catastrophic_count_rate=1.0,
        max_catastrophic_dominance_rate=1.0,
        max_mean_count_share=1.0,
    )
    assert sum("tree" in failure for failure in failures) == 2


def test_a_thin_spread_over_the_whole_tree_climbs_nothing() -> None:
    shares = {"sentinel": 0.46} | dict.fromkeys(sorted(fun_gate.TECH_KINDS), 0.06)
    failures = verdict(shares, min_top_tech_share=0.10)
    assert sum("worth building" in failure for failure in failures) == 1


def test_one_deep_tech_kind_clears_the_top_rule_but_not_the_sum() -> None:
    shares = {"sentinel": 0.80, "darter": 0.20}
    failures = verdict(
        shares,
        min_entropy=0.0,
        min_seat_p25_entropy=0.0,
        min_count_entropy=0.0,
        min_seat_p25_count_entropy=0.0,
        max_catastrophic_value_rate=1.0,
        max_catastrophic_count_rate=1.0,
        max_catastrophic_dominance_rate=1.0,
        max_mean_count_share=1.0,
    )
    assert sum("never climbed" in failure for failure in failures) == 1
    assert not any("worth building" in failure for failure in failures)


def test_each_structure_reach_floor_fails_independently() -> None:
    floors = {
        "fabricator": MIN_FABRICATOR_REACH,
        "turret": MIN_TURRET_REACH,
        "array": MIN_ARRAY_REACH,
        "reclaimer": MIN_RECLAIMER_REACH,
    }
    for kind, floor in floors.items():
        data = cohort(GOOD_SHARES)
        data["competitive_seats_with_building"][kind] = floor - 0.01
        failures = verdict_for(data)
        matching = [failure for failure in failures if f"{kind} reach" in failure]
        assert len(matching) == 1
        assert not any(
            f"{other} reach" in failure
            for other in floors
            if other != kind
            for failure in failures
        )


def test_structure_reach_boundaries_are_inclusive_and_repair_bay_is_not_gated() -> None:
    data = cohort(GOOD_SHARES)
    data["competitive_seats_with_building"] = {
        "fabricator": MIN_FABRICATOR_REACH,
        "turret": MIN_TURRET_REACH,
        "array": MIN_ARRAY_REACH,
        "reclaimer": MIN_RECLAIMER_REACH,
        "repairbay": 0.0,
    }
    assert verdict_for(data) == []


def test_industrial_profile_requires_reclaimer_reach() -> None:
    profile = fun_gate.PROFILES[1]
    data = cohort(GOOD_SHARES)
    data["competitive_seats_with_building"]["reclaimer"] = 0.25
    assert fun_gate.judge_profile_identity(profile, data, 0.25, 0.13) == []

    data["competitive_seats_with_building"]["reclaimer"] = 0.249
    failures = fun_gate.judge_profile_identity(profile, data, 0.25, 0.13)
    assert len(failures) == 1
    assert "industrial profile did not establish its economy" in failures[0]


def test_air_profile_requires_meaningful_air_wing() -> None:
    profile = fun_gate.PROFILES[2]
    data = cohort(
        {
            "sentinel": 0.45,
            "scuttler": 0.20,
            "lancer": 0.15,
            "bombard": 0.07,
            "buzzard": 0.065,
            "wisp": 0.065,
        }
    )
    assert fun_gate.judge_profile_identity(profile, data, 0.25, 0.13) == []

    data["mean_combat_share"]["wisp"] = 0.064
    failures = fun_gate.judge_profile_identity(profile, data, 0.25, 0.13)
    assert len(failures) == 1
    assert "air profile did not field a meaningful air wing" in failures[0]


def test_dealt_profile_has_no_specialist_identity_floor() -> None:
    data = cohort({"sentinel": 1.0})
    data["competitive_seats_with_building"]["reclaimer"] = 0.0
    assert fun_gate.judge_profile_identity(fun_gate.PROFILES[0], data, 1.0, 1.0) == []


def test_body_count_catches_scuttlers_hidden_by_value_mix() -> None:
    bodies = {
        "sentinel": 0.0475,
        "scuttler": 0.81,
        "lancer": 0.0475,
        "bombard": 0.0475,
        "flakhound": 0.0475,
    }
    failures = verdict(GOOD_SHARES, bodies)
    assert not any(failure.startswith("mix entropy") for failure in failures)
    assert any("body-time entropy" in failure for failure in failures)
    assert any("catastrophic body-dominance" in failure for failure in failures)
    assert any("dominates the slate" in failure for failure in failures)


def test_inactive_caps_fail_even_with_a_varied_army() -> None:
    failures = verdict(GOOD_SHARES, unhealthy_cap_rate=0.11)
    assert any("too many dead tails" in failure for failure in failures)


def test_recent_combat_makes_a_long_cap_healthy() -> None:
    health = fun_gate.cap_health(
        [raw_match(capped=True, last_combat=39_500)],
        stale_ticks=2_000,
    )
    assert health == {
        "capped": 1,
        "active_caps": 1,
        "unhealthy_caps": 0,
        "resource_exhausted_caps": 0,
    }


def test_quiet_resource_exhaustion_is_an_unhealthy_cap() -> None:
    health = fun_gate.cap_health(
        [raw_match(capped=True, last_economy=10_000)],
        stale_ticks=2_000,
    )
    assert health == {
        "capped": 1,
        "active_caps": 0,
        "unhealthy_caps": 1,
        "resource_exhausted_caps": 1,
    }


def test_quiet_cap_with_resources_is_still_unhealthy_but_not_exhausted() -> None:
    health = fun_gate.cap_health(
        [raw_match(capped=True, last_roster=30_000, salvage=500)],
        stale_ticks=2_000,
    )
    assert health["unhealthy_caps"] == 1
    assert health["resource_exhausted_caps"] == 0


def test_dead_or_resigned_reclaimers_do_not_mask_resource_exhaustion() -> None:
    for match in (
        raw_match(capped=True, reclaimers=1, reclaimer_resigned=True),
        raw_match(capped=True, reclaimers=1, reclaimer_foundries=0),
    ):
        health = fun_gate.cap_health([match], stale_ticks=2_000)
        assert health["resource_exhausted_caps"] == 1

    active = raw_match(
        capped=True,
        reclaimers=1,
        recovery_income_active=True,
    )
    health = fun_gate.cap_health([active], stale_ticks=2_000)
    assert health["resource_exhausted_caps"] == 0


def test_late_foundry_baseline_prevents_a_false_exhaustion_cause() -> None:
    match = raw_match(
        capped=True,
        reclaimers=0,
        recovery_income_active=True,
    )
    health = fun_gate.cap_health([match], stale_ticks=2_000)
    assert health["unhealthy_caps"] == 1, "quiet play remains unhealthy"
    assert health["resource_exhausted_caps"] == 0, (
        "Rust's active recovery-income fact prevents a false exhaustion cause"
    )


def test_main_runs_dealt_and_two_named_composition_profiles(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    calls = []

    def fake_run(
        argv: list[str],
        **_kwargs: object,
    ) -> subprocess.CompletedProcess[str]:
        calls.append(argv)
        profile = requested_profile(argv)
        out = pathlib.Path(argv[argv.index("--out") + 1])
        out.write_text(
            json.dumps(
                good_payload(
                    style=profile.style,
                    variant=profile.variant,
                    aggression=None,
                    fixed_profile=not profile.full_gate,
                )
            )
        )
        return subprocess.CompletedProcess(argv, 0)

    monkeypatch.setattr(fun_gate.subprocess, "run", fake_run)
    monkeypatch.setattr(sys, "argv", ["fun_gate.py", "--weights", "candidate.json"])

    assert fun_gate.main() == 0
    assert len(calls) == 3
    assert [
        (
            call[call.index("--style") + 1],
            int(call[call.index("--variant") + 1]),
        )
        for call in calls
        if "--style" in call
    ] == [("turtle", 1), ("balanced", 1)]
    assert all("--aggression" not in call for call in calls)
    output = capsys.readouterr().out
    assert "dealt profile" in output
    assert "industrial-attrition profile" in output
    assert "air-combined profile" in output
    assert output.count("composition-only") == 2
    assert "inactive 1" in output
    assert "fun gate: open" in output


def test_main_rejects_a_probe_with_no_competitive_combat(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    payload = good_payload()
    payload["overall"]["combat_seats"] = 0

    def fake_run(
        argv: list[str],
        **_kwargs: object,
    ) -> subprocess.CompletedProcess[str]:
        out = pathlib.Path(argv[argv.index("--out") + 1])
        out.write_text(json.dumps(payload))
        return subprocess.CompletedProcess(argv, 0)

    monkeypatch.setattr(fun_gate.subprocess, "run", fake_run)
    monkeypatch.setattr(sys, "argv", ["fun_gate.py", "--weights", "candidate.json"])

    assert fun_gate.main() == 1
    assert "no competitive-lifetime combat seats" in capsys.readouterr().out


def test_promotion_refuses_fewer_than_three_seeds_without_running_probe(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    def unexpected_run(*_args: object, **_kwargs: object) -> None:
        raise AssertionError("an invalid promotion sample must not run")

    monkeypatch.setattr(fun_gate.subprocess, "run", unexpected_run)
    monkeypatch.setattr(
        sys,
        "argv",
        ["fun_gate.py", "--weights", "candidate.json", "--seeds", "2"],
    )

    assert fun_gate.main() == 1
    assert "promotion requires at least 3 seeds" in capsys.readouterr().out


def test_underpowered_seed_override_is_marked_diagnostic_only(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    calls = 0

    def fake_run(
        argv: list[str],
        **_kwargs: object,
    ) -> subprocess.CompletedProcess[str]:
        nonlocal calls
        calls += 1
        profile = requested_profile(argv)
        out = pathlib.Path(argv[argv.index("--out") + 1])
        out.write_text(
            json.dumps(
                good_payload(
                    seeds=2,
                    style=profile.style,
                    variant=profile.variant,
                    aggression=None,
                    fixed_profile=not profile.full_gate,
                )
            )
        )
        return subprocess.CompletedProcess(argv, 0)

    monkeypatch.setattr(fun_gate.subprocess, "run", fake_run)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "fun_gate.py",
            "--weights",
            "candidate.json",
            "--seeds",
            "2",
            "--allow-fewer-seeds-for-diagnostics",
        ],
    )

    assert fun_gate.main() == 0
    assert calls == 3
    output = capsys.readouterr().out
    assert "DIAGNOSTIC ONLY" in output
    assert "NOT VALID FOR PROMOTION" in output


def test_regression_envelope_boundaries_are_inclusive() -> None:
    baseline = cohort(GOOD_SHARES)
    candidate = copy.deepcopy(baseline)
    baseline["combat_entropy_bits"] = 2.25
    candidate["combat_entropy_bits"] = 2.125
    baseline["combat_count_entropy_bits"] = 2.0
    candidate["combat_count_entropy_bits"] = 1.875
    baseline["seat_combat_entropy"]["p25"] = 1.50
    candidate["seat_combat_entropy"]["p25"] = 1.25
    baseline["seat_combat_count_entropy"]["p25"] = 1.50
    candidate["seat_combat_count_entropy"]["p25"] = 1.25
    baseline["mean_combat_count_share"] = {
        "sentinel": 0.375,
        "lancer": 0.375,
        "bombard": 0.25,
    }
    candidate["mean_combat_count_share"] = {
        "sentinel": 0.50,
        "lancer": 0.25,
        "bombard": 0.25,
    }
    baseline_tails = {
        "low_value_rate": 0.0,
        "low_count_rate": 0.0,
        "dominant_rate": 0.0,
    }
    candidate_tails = {
        "low_value_rate": 0.125,
        "low_count_rate": 0.125,
        "dominant_rate": 0.125,
    }
    assert (
        fun_gate.regression_failures(
            candidate,
            candidate_tails,
            baseline,
            baseline_tails,
            0.125,
            0.25,
            0.125,
            0.125,
        )
        == []
    )

    candidate["combat_entropy_bits"] -= 0.001
    candidate["seat_combat_entropy"]["p25"] -= 0.001
    candidate["mean_combat_count_share"]["sentinel"] += 0.001
    candidate_tails["low_value_rate"] += 0.001
    failures = fun_gate.regression_failures(
        candidate,
        candidate_tails,
        baseline,
        baseline_tails,
        0.125,
        0.25,
        0.125,
        0.125,
    )
    assert any("value entropy dropped" in failure for failure in failures)
    assert any("value p25 dropped" in failure for failure in failures)
    assert any("catastrophic value-mix rate rose" in failure for failure in failures)
    assert any("leading mean body share rose" in failure for failure in failures)


def test_main_runs_a_same_profile_and_seed_baseline_envelope(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    calls = []

    def fake_run(
        argv: list[str],
        **_kwargs: object,
    ) -> subprocess.CompletedProcess[str]:
        calls.append(argv)
        profile = requested_profile(argv)
        out = pathlib.Path(argv[argv.index("--out") + 1])
        out.write_text(
            json.dumps(
                good_payload(
                    style=profile.style,
                    variant=profile.variant,
                    aggression=None,
                )
            )
        )
        return subprocess.CompletedProcess(argv, 0)

    monkeypatch.setattr(fun_gate.subprocess, "run", fake_run)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "fun_gate.py",
            "--weights",
            "candidate.json",
            "--baseline-weights",
            "baseline.json",
        ],
    )

    assert fun_gate.main() == 0
    assert len(calls) == 6
    assert [
        (
            call[call.index("--weights") + 1],
            call[call.index("--style") + 1],
            call[call.index("--variant") + 1],
        )
        for call in calls
        if "--style" in call
    ] == [
        ("candidate.json", "turtle", "1"),
        ("candidate.json", "balanced", "1"),
        ("baseline.json", "turtle", "1"),
        ("baseline.json", "balanced", "1"),
    ]
    assert "fun gate: open" in capsys.readouterr().out


def test_main_rejects_a_baseline_with_a_different_seed_slate(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    def fake_run(
        argv: list[str],
        **_kwargs: object,
    ) -> subprocess.CompletedProcess[str]:
        weights = argv[argv.index("--weights") + 1]
        profile = requested_profile(argv)
        suffix = "-different" if weights == "baseline.json" else ""
        out = pathlib.Path(argv[argv.index("--out") + 1])
        out.write_text(
            json.dumps(
                good_payload(
                    style=profile.style,
                    variant=profile.variant,
                    aggression=None,
                    scenario_suffix=suffix,
                )
            )
        )
        return subprocess.CompletedProcess(argv, 0)

    monkeypatch.setattr(fun_gate.subprocess, "run", fake_run)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "fun_gate.py",
            "--weights",
            "candidate.json",
            "--baseline-weights",
            "baseline.json",
        ],
    )

    assert fun_gate.main() == 1
    assert "exact map/seed slate" in capsys.readouterr().out
