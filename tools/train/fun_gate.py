"""The fun gate, executable: probe a candidate artifact's composition
and pass judgment. A checkpoint that spams one kind fails promotion the
way an inactive capped match fails the liveness rule.

    uv run fun_gate.py --weights runs/candidate.json

Promotion runs the shipped personality deal plus the named
Industrial Attrition and Air Combined profiles on the same map/seed slate.
The dealt profile receives the complete gate; named specialist profiles
receive composition and tech checks only, because specialization is their
purpose and a long active match is not a defect.

The composition contract separates broad quality from catastrophic tails:

  --min-entropy (2.00 bits) and --min-count-entropy (1.95 bits) judge
    the mean value and body-time mixes.
  --min-seat-p25-entropy (1.35) and
    --min-seat-p25-count-entropy (1.25) require the lower quartile to
    field a real mix without pretending every brief losing seat had time
    to diversify.
  Raw competitive seat arrays count catastrophic lifetimes directly:
    value entropy below 0.75 in at most 7.5% of seats, body entropy below
    0.65 in at most 7.5%, and a single kind above 80% body-time in at most
    10%. This replaces volatile p10/p90 approximations.
  --max-mean-count-share (0.50) caps the leading kind across the slate.
  --max-unhealthy-cap-rate rejects dead tails, classified from recent
    combat/economy/roster activity rather than match duration. A long
    active war is not a stall.
  --min-tech-share (0.45) on the SUM over the Fabricator-gated kinds:
    was the tech tree climbed at all.
  --min-top-tech-share (0.15) on the LARGEST single tech kind demands
    that something on the tree was actually worth building.
  --min-fabricator-reach (0.90), --min-turret-reach (0.40),
    --min-array-reach (0.60), and --min-reclaimer-reach (0.25) require
    those completed structures across competitive lifetimes. Repair
    Bays remain diagnostic because field repair is the dedicated
    `repair-probe` gate and the building is intentionally niche.
  Industrial Attrition must independently reach a Reclaimer in at least
    25% of competitive lifetimes, and Air Combined must carry at least
    13% of its army value in faction-appropriate air units. Named profiles
    therefore have to express their advertised identity, not merely clear
    the broad anti-spam floor.

Composition judgment reads the competitive-lifetime combat fields from
`driver balance-probe --out`. Every seat contributes while it is
non-resigned and holds a completed Foundry, including a loser's whole
pre-defeat army history; post-elimination autonomous remnants do not
contribute. The parallel all-unit fields remain diagnostics. Recent
combat or economic work keeps a long match healthy, while a quiet tail
fails whether its cause is exhausted scrap, an impossible target, or a
broken policy.

Promotion requires at least three seeds. The explicitly named
``--allow-fewer-seeds-for-diagnostics`` override produces a marked,
non-promotional diagnostic result. ``--baseline-weights`` optionally
runs the same profiles and seeds against a pinned artifact and rejects
material paired-slate regressions in entropy, lower-quartile diversity,
catastrophic-tail rates, and leading-kind share.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import subprocess
import sys
import tempfile
from dataclasses import dataclass

# The Fabricator's produce list (sim/src/stats.rs) — the roster a match
# only reaches by building the tech gate first.
TECH_KINDS = {
    "scuttler",
    "lancer",
    "bombard",
    "flakhound",
    "stinger",
    "buzzard",
    "darter",
    "talon",
    "wisp",
}

AIR_KINDS = {
    "buzzard",
    "darter",
    "talon",
    "wisp",
}

# The exact `--out` payload shape this gate reads. Schema 8 identifies
# explicitly selected named styles and variants alongside the legacy
# aggression component and carries no scripted-tier dial; accepting
# another schema risks silently judging a raw zero-facet profile while
# labeling it as a shipped personality.
EXPECTED_SCHEMA = 8
DEFAULT_STALE_CAP_TICKS = 2_000
MIN_PROMOTION_SEEDS = 3


@dataclass(frozen=True)
class ProbeProfile:
    """One exact shipped-profile path exercised by the promotion gate."""

    label: str
    style: str | None
    variant: int | None
    full_gate: bool


PROFILES = (
    ProbeProfile("dealt", None, None, True),
    ProbeProfile("industrial-attrition", "turtle", 1, False),
    ProbeProfile("air-combined", "balanced", 1, False),
)


def cap_health(matches: list[dict], stale_ticks: int) -> dict[str, int]:
    """Classifies caps by meaningful recent activity.

    Roster change alone is too narrow: a stable army can fight, repair,
    and harvest for a long time. Conversely, resources remaining on the
    map do not make a silent policy healthy. The activity age is the
    verdict; exhausted salvage and passive income are reported as cause
    evidence, not used to excuse an inactive match.
    """
    if stale_ticks < 0:
        raise ValueError("stale cap threshold must be non-negative")
    counts = {
        "capped": 0,
        "active_caps": 0,
        "unhealthy_caps": 0,
        "resource_exhausted_caps": 0,
    }
    for match in matches:
        if not match["capped"]:
            continue
        counts["capped"] += 1
        activity = match["activity"]
        last = max(
            int(activity["last_combat_tick"]),
            int(activity["last_economy_tick"]),
            int(match["last_progress_tick"]),
        )
        inactive = int(match["ticks"]) - last > stale_ticks
        if inactive:
            counts["unhealthy_caps"] += 1
        else:
            counts["active_caps"] += 1
        economy = match["final_economy"]
        has_recovery_income = any(
            bool(seat["recovery_income_active"]) for seat in economy["seats"]
        )
        if (
            inactive
            and int(economy["remaining_map_salvage"]) == 0
            and not has_recovery_income
        ):
            counts["resource_exhausted_caps"] += 1
    return counts


def combat_tail_rates(
    matches: list[dict],
    catastrophic_value_entropy: float,
    catastrophic_count_entropy: float,
    catastrophic_count_dominance: float,
) -> dict[str, float | int]:
    """Measures catastrophic competitive lifetimes from raw seat arrays.

    Aggregate quantiles hide the exact number of bad seats and small
    cohorts make nearest-rank p10/p90 jump sharply. The raw schema-8 arrays
    let the gate state its real contract as rates. Seats with no competitive
    combat mix are skipped, exactly as ``Aggregate.combat_seats`` skips them.
    """
    seats = 0
    low_value = 0
    low_count = 0
    dominant = 0
    for match in matches:
        value_mixes = match["combat_seats"]
        value_entropies = match["combat_entropy_bits"]
        count_mixes = match["combat_count_seats"]
        count_entropies = match["combat_count_entropy_bits"]
        lengths = {
            len(value_mixes),
            len(value_entropies),
            len(count_mixes),
            len(count_entropies),
        }
        if len(lengths) != 1:
            raise RuntimeError(
                f"{match.get('scenario', 'unknown')} seed "
                f"{match.get('seed', 'unknown')} has misaligned competitive "
                "combat arrays"
            )
        for value_mix, value_entropy, count_mix, count_entropy in zip(
            value_mixes,
            value_entropies,
            count_mixes,
            count_entropies,
            strict=True,
        ):
            if not value_mix:
                continue
            if not count_mix:
                raise RuntimeError(
                    f"{match.get('scenario', 'unknown')} seed "
                    f"{match.get('seed', 'unknown')} has value combat without "
                    "a body-time mix"
                )
            seats += 1
            low_value += float(value_entropy) < catastrophic_value_entropy
            low_count += float(count_entropy) < catastrophic_count_entropy
            dominance = max(float(share) for share in count_mix.values())
            dominant += dominance > catastrophic_count_dominance
    if seats == 0:
        raise RuntimeError("raw probe records contain no competitive combat seats")
    return {
        "seats": seats,
        "low_value": low_value,
        "low_value_rate": low_value / seats,
        "low_count": low_count,
        "low_count_rate": low_count / seats,
        "dominant": dominant,
        "dominant_rate": dominant / seats,
    }


def judge_composition(
    cohort: dict,
    tails: dict[str, float | int],
    min_entropy: float,
    min_seat_p25_entropy: float,
    min_count_entropy: float,
    min_seat_p25_count_entropy: float,
    max_catastrophic_value_rate: float,
    max_catastrophic_count_rate: float,
    max_catastrophic_dominance_rate: float,
    max_mean_count_share: float,
    min_tech_share: float,
    min_top_tech_share: float,
) -> list[str]:
    """Returns composition and tech failures for one personality profile."""
    failures = []
    entropy = cohort["combat_entropy_bits"]
    if entropy < min_entropy:
        failures.append(f"mix entropy {entropy:.2f} bits < {min_entropy} — spam")
    seat_entropy = cohort["seat_combat_entropy"]["p25"]
    if seat_entropy < min_seat_p25_entropy:
        failures.append(
            f"per-seat value entropy p25 {seat_entropy:.2f} bits "
            f"< {min_seat_p25_entropy} — too much of the lower quartile spams"
        )
    count_entropy = cohort["combat_count_entropy_bits"]
    if count_entropy < min_count_entropy:
        failures.append(
            f"body-time entropy {count_entropy:.2f} bits "
            f"< {min_count_entropy} — cheap-unit presence is too narrow"
        )
    seat_count_entropy = cohort["seat_combat_count_entropy"]["p25"]
    if seat_count_entropy < min_seat_p25_count_entropy:
        failures.append(
            f"per-seat body-time entropy p25 {seat_count_entropy:.2f} bits "
            f"< {min_seat_p25_count_entropy} — too much of the lower quartile "
            "relies on too few units"
        )
    low_value_rate = float(tails["low_value_rate"])
    if low_value_rate > max_catastrophic_value_rate:
        failures.append(
            f"catastrophic value-mix rate {low_value_rate * 100:.1f}% "
            f"> {max_catastrophic_value_rate * 100:.1f}%"
        )
    low_count_rate = float(tails["low_count_rate"])
    if low_count_rate > max_catastrophic_count_rate:
        failures.append(
            f"catastrophic body-mix rate {low_count_rate * 100:.1f}% "
            f"> {max_catastrophic_count_rate * 100:.1f}%"
        )
    dominant_rate = float(tails["dominant_rate"])
    if dominant_rate > max_catastrophic_dominance_rate:
        failures.append(
            f"catastrophic body-dominance rate {dominant_rate * 100:.1f}% "
            f"> {max_catastrophic_dominance_rate * 100:.1f}%"
        )
    top_count_kind, top_count_share = max(
        cohort["mean_combat_count_share"].items(), key=lambda item: item[1]
    )
    if top_count_share > max_mean_count_share:
        failures.append(
            f"mean {top_count_kind} body-time share {top_count_share * 100:.1f}% "
            f"> {max_mean_count_share * 100:.0f}% — one unit dominates the slate"
        )
    shares = cohort["mean_combat_share"]
    tech = {k: v for k, v in shares.items() if k in TECH_KINDS}
    total = sum(tech.values())
    if total < min_tech_share:
        failures.append(
            f"tech kinds carry {total * 100:.1f}% of army value "
            f"< {min_tech_share * 100:.0f}% — the tree was never climbed"
        )
    top_kind, top = max(tech.items(), key=lambda kv: kv[1], default=("none", 0.0))
    if top < min_top_tech_share:
        failures.append(
            f"the fattest tech kind is {top_kind} at {top * 100:.1f}% "
            f"< {min_top_tech_share * 100:.0f}% — the tree was visited, "
            f"nothing on it was worth building"
        )
    return failures


def judge_health(
    unhealthy_cap_rate: float,
    max_unhealthy_cap_rate: float,
) -> list[str]:
    """Returns failures for inactive capped matches."""
    if unhealthy_cap_rate <= max_unhealthy_cap_rate:
        return []
    return [
        f"inactive cap rate {unhealthy_cap_rate * 100:.1f}% "
        f"> {max_unhealthy_cap_rate * 100:.0f}% — too many dead tails"
    ]


def judge_structures(
    cohort: dict,
    min_fabricator_reach: float,
    min_turret_reach: float,
    min_array_reach: float,
    min_reclaimer_reach: float,
) -> list[str]:
    """Returns completed-structure reach failures for the dealt profile."""
    failures = []
    reach = cohort["competitive_seats_with_building"]
    for kind, minimum in (
        ("fabricator", min_fabricator_reach),
        ("turret", min_turret_reach),
        ("array", min_array_reach),
        ("reclaimer", min_reclaimer_reach),
    ):
        actual = float(reach.get(kind, 0.0))
        if actual < minimum:
            failures.append(
                f"{kind} reach {actual * 100:.1f}% "
                f"< {minimum * 100:.0f}% — too few competitive lifetimes "
                f"completed one"
            )
    return failures


def judge_profile_identity(
    profile: ProbeProfile,
    cohort: dict,
    min_industrial_reclaimer_reach: float,
    min_air_wing_share: float,
) -> list[str]:
    """Returns failures when a named specialist does not express its role."""
    if profile.label == "industrial-attrition":
        reach = float(cohort["competitive_seats_with_building"].get("reclaimer", 0.0))
        if reach < min_industrial_reclaimer_reach:
            return [
                f"Reclaimer reach {reach * 100:.1f}% "
                f"< {min_industrial_reclaimer_reach * 100:.0f}% — the industrial "
                "profile did not establish its economy"
            ]
    elif profile.label == "air-combined":
        shares = cohort["mean_combat_share"]
        air_share = sum(float(shares.get(kind, 0.0)) for kind in AIR_KINDS)
        if air_share < min_air_wing_share:
            return [
                f"air-wing value share {air_share * 100:.1f}% "
                f"< {min_air_wing_share * 100:.0f}% — the air profile did not "
                "field a meaningful air wing"
            ]
    return []


def regression_failures(
    candidate: dict,
    candidate_tails: dict[str, float | int],
    baseline: dict,
    baseline_tails: dict[str, float | int],
    max_entropy_drop: float,
    max_p25_drop: float,
    max_catastrophic_rate_increase: float,
    max_leading_share_increase: float,
) -> list[str]:
    """Compares one candidate/baseline profile on an identical seed slate."""
    failures = []
    for label, key in (
        ("value entropy", "combat_entropy_bits"),
        ("body entropy", "combat_count_entropy_bits"),
    ):
        drop = float(baseline[key]) - float(candidate[key])
        if drop > max_entropy_drop:
            failures.append(
                f"{label} dropped {drop:.2f} bits from baseline "
                f"> {max_entropy_drop:.2f}"
            )
    for label, key in (
        ("value p25", "seat_combat_entropy"),
        ("body p25", "seat_combat_count_entropy"),
    ):
        drop = float(baseline[key]["p25"]) - float(candidate[key]["p25"])
        if drop > max_p25_drop:
            failures.append(
                f"{label} dropped {drop:.2f} bits from baseline > {max_p25_drop:.2f}"
            )
    for label, key in (
        ("catastrophic value-mix rate", "low_value_rate"),
        ("catastrophic body-mix rate", "low_count_rate"),
        ("catastrophic body-dominance rate", "dominant_rate"),
    ):
        increase = float(candidate_tails[key]) - float(baseline_tails[key])
        if increase > max_catastrophic_rate_increase:
            failures.append(
                f"{label} rose {increase * 100:.1f} points from baseline "
                f"> {max_catastrophic_rate_increase * 100:.1f}"
            )
    candidate_top = max(candidate["mean_combat_count_share"].values())
    baseline_top = max(baseline["mean_combat_count_share"].values())
    increase = float(candidate_top) - float(baseline_top)
    if increase > max_leading_share_increase:
        failures.append(
            f"leading mean body share rose {increase * 100:.1f} points "
            f"from baseline > {max_leading_share_increase * 100:.1f}"
        )
    return failures


def run_probe(
    weights: str,
    driver: str,
    scenarios: str,
    level: str,
    seeds: int,
    ticks: int,
    profile: ProbeProfile,
) -> dict:
    """Runs one profile and returns its schema-8 JSON payload."""
    with tempfile.TemporaryDirectory(prefix="oxide-fun-gate-") as directory:
        out = pathlib.Path(directory) / "probe.json"
        command = [
            driver,
            "balance-probe",
            "--dir",
            scenarios,
            "--level",
            level,
            "--seeds",
            str(seeds),
            "--ticks",
            str(ticks),
            "--weights",
            weights,
            "--out",
            str(out),
        ]
        if profile.style is not None:
            command.extend(["--style", profile.style])
            command.extend(["--variant", str(profile.variant)])
        subprocess.run(command, check=True, capture_output=True)
        return json.loads(out.read_text())


def validate_profile_payload(
    payload: dict,
    expected_seeds: int,
    profile: ProbeProfile,
    label: str,
) -> None:
    """Rejects a payload whose shape or sample does not match this run."""
    schema = payload.get("schema", 1)
    if schema != EXPECTED_SCHEMA:
        raise RuntimeError(
            f"{label} probe payload is schema {schema}; this gate reads exactly "
            f"{EXPECTED_SCHEMA}"
        )
    if int(payload.get("seeds", -1)) != expected_seeds:
        raise RuntimeError(
            f"{label} probe reported {payload.get('seeds')} seeds, expected "
            f"{expected_seeds}"
        )
    dials = payload.get("dials", {})
    for dial, expected in (
        ("style", profile.style),
        ("variant", profile.variant),
        # Named selectors must not silently fall back to the raw,
        # zero-facet compatibility path.
        ("aggression", None),
    ):
        actual = dials.get(dial)
        if actual != expected:
            raise RuntimeError(
                f"{label} probe reported {dial} {actual}, expected {expected}"
            )
    overall = payload["overall"]
    if overall["combat_seats"] == 0:
        raise RuntimeError(
            f"{label}: {overall['matches']} matches produced no "
            "competitive-lifetime combat seats"
        )


def probe_slate(payload: dict) -> list[tuple[str, int]]:
    """The exact map/seed rows a paired baseline comparison must share."""
    return sorted(
        (str(match["scenario"]), int(match["seed"])) for match in payload["matches"]
    )


def validate_same_slate(candidate: dict, baseline: dict, label: str) -> None:
    """Requires a baseline profile to cover the candidate's exact rows."""
    if probe_slate(candidate) != probe_slate(baseline):
        raise RuntimeError(
            f"{label} baseline did not run the candidate's exact map/seed slate"
        )


def evaluate_profile(
    payload: dict,
    args: argparse.Namespace,
    full_gate: bool,
) -> tuple[dict, dict[str, float | int], dict[str, int] | None, list[str]]:
    """Evaluates one profile; named specialists deliberately skip game-length
    and structure judgments."""
    overall = payload["overall"]
    tails = combat_tail_rates(
        payload["matches"],
        args.catastrophic_value_entropy,
        args.catastrophic_count_entropy,
        args.catastrophic_count_dominance,
    )
    if int(tails["seats"]) != int(overall["combat_seats"]):
        raise RuntimeError(
            f"raw records contain {tails['seats']} competitive seats but the "
            f"aggregate reports {overall['combat_seats']}"
        )
    failures = judge_composition(
        overall,
        tails,
        args.min_entropy,
        args.min_seat_p25_entropy,
        args.min_count_entropy,
        args.min_seat_p25_count_entropy,
        args.max_catastrophic_value_rate,
        args.max_catastrophic_count_rate,
        args.max_catastrophic_dominance_rate,
        args.max_mean_count_share,
        args.min_tech_share,
        args.min_top_tech_share,
    )
    health = None
    if full_gate:
        health = cap_health(payload["matches"], args.stale_cap_ticks)
        unhealthy_cap_rate = health["unhealthy_caps"] / overall["matches"]
        failures.extend(judge_health(unhealthy_cap_rate, args.max_unhealthy_cap_rate))
        failures.extend(
            judge_structures(
                overall,
                args.min_fabricator_reach,
                args.min_turret_reach,
                args.min_array_reach,
                args.min_reclaimer_reach,
            )
        )
    return overall, tails, health, failures


def print_profile(
    label: str,
    overall: dict,
    tails: dict[str, float | int],
    health: dict[str, int] | None,
    args: argparse.Namespace,
) -> None:
    """Prints the evidence the profile's verdict consumed."""
    shares = sorted(overall["mean_combat_share"].items(), key=lambda kv: -kv[1])
    listed = ", ".join(f"{k} {v * 100:.1f}%" for k, v in shares)
    count_shares = sorted(
        overall["mean_combat_count_share"].items(), key=lambda kv: -kv[1]
    )
    count_listed = ", ".join(f"{k} {v * 100:.1f}%" for k, v in count_shares)
    print(f"\n{label} profile")
    print(
        f"judging {overall['combat_seats']} competitive lifetimes from "
        f"{overall['matches']} matches ({overall['decided']} decided)"
    )
    if health is None:
        print("composition-only: cap health and structure reach are diagnostic")
    else:
        print(
            f"caps {health['capped']} · active {health['active_caps']} · "
            f"inactive {health['unhealthy_caps']} · resource-exhausted "
            f"{health['resource_exhausted_caps']} "
            f"(activity window {args.stale_cap_ticks} ticks)"
        )
    print(
        f"entropy {overall['combat_entropy_bits']:.2f} bits · "
        f"per-seat p25 {overall['seat_combat_entropy']['p25']:.2f} bits · "
        f"shares {listed}"
    )
    print(
        f"body-time entropy {overall['combat_count_entropy_bits']:.2f} bits · "
        f"per-seat p25 {overall['seat_combat_count_entropy']['p25']:.2f} bits · "
        f"shares {count_listed}"
    )
    print(
        f"catastrophic seats: value {tails['low_value']}/{tails['seats']} "
        f"({float(tails['low_value_rate']) * 100:.1f}%) · body "
        f"{tails['low_count']}/{tails['seats']} "
        f"({float(tails['low_count_rate']) * 100:.1f}%) · dominance "
        f"{tails['dominant']}/{tails['seats']} "
        f"({float(tails['dominant_rate']) * 100:.1f}%)"
    )


def main() -> int:
    ap = argparse.ArgumentParser(
        description="Gate a neural artifact on dealt and named-profile composition.",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    ap.add_argument("--weights", required=True, help="candidate Q12 weights JSON")
    ap.add_argument(
        "--baseline-weights",
        help="optional pinned artifact for a same-map/seed regression envelope",
    )
    ap.add_argument(
        "--driver",
        default="../../target/release/oxide-driver",
        help="oxide-driver executable",
    )
    ap.add_argument(
        "--scenarios",
        default="../../scenarios",
        help="shipped scenario directory",
    )
    ap.add_argument("--level", default="medium", help="neural difficulty level")
    ap.add_argument(
        "--seeds",
        type=int,
        default=MIN_PROMOTION_SEEDS,
        help="seeds per map; promotion requires at least 3",
    )
    ap.add_argument(
        "--allow-fewer-seeds-for-diagnostics",
        action="store_true",
        help=(
            "allow an underpowered <3-seed run, clearly marked DIAGNOSTIC ONLY "
            "and never valid for promotion"
        ),
    )
    ap.add_argument(
        "--ticks",
        type=int,
        default=40_000,
        help="maximum ticks per match; reaching it is not itself a failure",
    )
    ap.add_argument(
        "--stale-cap-ticks",
        type=int,
        default=DEFAULT_STALE_CAP_TICKS,
        help="inactivity window used to classify capped dealt-profile matches",
    )
    ap.add_argument(
        "--max-unhealthy-cap-rate",
        type=float,
        default=0.10,
        help="maximum dealt-profile rate of inactive capped matches",
    )
    ap.add_argument(
        "--min-entropy",
        type=float,
        default=2.00,
        help="minimum mean competitive value-mix entropy in bits",
    )
    ap.add_argument(
        "--min-seat-p25-entropy",
        type=float,
        default=1.35,
        help="minimum competitive value-mix entropy at the seat p25",
    )
    ap.add_argument(
        "--min-count-entropy",
        type=float,
        default=1.95,
        help="minimum mean competitive body-time entropy in bits",
    )
    ap.add_argument(
        "--min-seat-p25-count-entropy",
        type=float,
        default=1.25,
        help="minimum competitive body-time entropy at the seat p25",
    )
    ap.add_argument(
        "--catastrophic-value-entropy",
        type=float,
        default=0.75,
        help="value entropy below this counts as a catastrophic seat",
    )
    ap.add_argument(
        "--max-catastrophic-value-rate",
        type=float,
        default=0.075,
        help="maximum rate of catastrophic value-mix seats",
    )
    ap.add_argument(
        "--catastrophic-count-entropy",
        type=float,
        default=0.65,
        help="body-time entropy below this counts as a catastrophic seat",
    )
    ap.add_argument(
        "--max-catastrophic-count-rate",
        type=float,
        default=0.075,
        help="maximum rate of catastrophic body-mix seats",
    )
    ap.add_argument(
        "--catastrophic-count-dominance",
        type=float,
        default=0.80,
        help="body-time share above this counts as catastrophic dominance",
    )
    ap.add_argument(
        "--max-catastrophic-dominance-rate",
        type=float,
        default=0.10,
        help="maximum rate of catastrophically dominated seats",
    )
    ap.add_argument(
        "--max-mean-count-share",
        type=float,
        default=0.50,
        help="maximum leading mean competitive body-time share",
    )
    ap.add_argument(
        "--min-tech-share",
        type=float,
        default=0.45,
        help="minimum summed competitive army-value share from tech units",
    )
    ap.add_argument(
        "--min-top-tech-share",
        type=float,
        default=0.15,
        help="minimum competitive army-value share of the leading tech unit",
    )
    ap.add_argument(
        "--min-fabricator-reach",
        type=float,
        default=0.90,
        help="minimum dealt-profile competitive-seat Fabricator reach",
    )
    ap.add_argument(
        "--min-turret-reach",
        type=float,
        default=0.40,
        help="minimum dealt-profile competitive-seat Turret reach",
    )
    ap.add_argument(
        "--min-array-reach",
        type=float,
        default=0.60,
        help="minimum dealt-profile competitive-seat Array reach",
    )
    ap.add_argument(
        "--min-reclaimer-reach",
        type=float,
        default=0.25,
        help="minimum dealt-profile competitive-seat Reclaimer reach",
    )
    ap.add_argument(
        "--min-industrial-reclaimer-reach",
        type=float,
        default=0.25,
        help="minimum Industrial Attrition competitive-seat Reclaimer reach",
    )
    ap.add_argument(
        "--min-air-wing-share",
        type=float,
        default=0.13,
        help="minimum Air Combined army-value share carried by air units",
    )
    ap.add_argument(
        "--max-baseline-entropy-drop",
        type=float,
        default=0.10,
        help="maximum mean value/body entropy loss versus a paired baseline",
    )
    ap.add_argument(
        "--max-baseline-p25-drop",
        type=float,
        default=0.15,
        help="maximum seat-p25 entropy loss versus a paired baseline",
    )
    ap.add_argument(
        "--max-baseline-catastrophic-rate-increase",
        type=float,
        default=0.05,
        help="maximum catastrophic-seat rate increase versus a paired baseline",
    )
    ap.add_argument(
        "--max-baseline-leading-share-increase",
        type=float,
        default=0.05,
        help="maximum leading body-share increase versus a paired baseline",
    )
    args = ap.parse_args()

    if args.seeds < 1:
        ap.error("--seeds must be positive")
    diagnostic_only = args.seeds < MIN_PROMOTION_SEEDS
    if diagnostic_only and not args.allow_fewer_seeds_for_diagnostics:
        print(
            f"FUN GATE FAIL: promotion requires at least {MIN_PROMOTION_SEEDS} "
            "seeds per map; use --allow-fewer-seeds-for-diagnostics only for "
            "a non-promotional diagnostic"
        )
        return 1
    if diagnostic_only:
        print(
            "DIAGNOSTIC ONLY: fewer than 3 seeds; this result cannot promote "
            "an artifact"
        )

    candidate_profiles = {}
    all_failures = []
    try:
        for profile in PROFILES:
            payload = run_probe(
                args.weights,
                args.driver,
                args.scenarios,
                args.level,
                args.seeds,
                args.ticks,
                profile,
            )
            validate_profile_payload(payload, args.seeds, profile, profile.label)
            candidate_profiles[profile.label] = payload
            overall, tails, health, failures = evaluate_profile(
                payload, args, full_gate=profile.full_gate
            )
            failures.extend(
                judge_profile_identity(
                    profile,
                    overall,
                    args.min_industrial_reclaimer_reach,
                    args.min_air_wing_share,
                )
            )
            print_profile(profile.label, overall, tails, health, args)
            all_failures.extend(f"{profile.label}: {failure}" for failure in failures)

        if args.baseline_weights:
            for profile in PROFILES:
                baseline = run_probe(
                    args.baseline_weights,
                    args.driver,
                    args.scenarios,
                    args.level,
                    args.seeds,
                    args.ticks,
                    profile,
                )
                validate_profile_payload(
                    baseline,
                    args.seeds,
                    profile,
                    f"baseline {profile.label}",
                )
                candidate = candidate_profiles[profile.label]
                validate_same_slate(candidate, baseline, profile.label)
                candidate_overall, candidate_tails, _, _ = evaluate_profile(
                    candidate, args, full_gate=False
                )
                baseline_overall, baseline_tails, _, _ = evaluate_profile(
                    baseline, args, full_gate=False
                )
                regressions = regression_failures(
                    candidate_overall,
                    candidate_tails,
                    baseline_overall,
                    baseline_tails,
                    args.max_baseline_entropy_drop,
                    args.max_baseline_p25_drop,
                    args.max_baseline_catastrophic_rate_increase,
                    args.max_baseline_leading_share_increase,
                )
                all_failures.extend(
                    f"{profile.label} vs baseline: {failure}" for failure in regressions
                )
    except (KeyError, TypeError, ValueError, RuntimeError) as error:
        print(f"FUN GATE FAIL: invalid probe evidence: {error}")
        return 1

    if all_failures:
        for failure in all_failures:
            print(f"FUN GATE FAIL: {failure}")
        return 1
    if diagnostic_only:
        print("diagnostic fun gate: open (NOT VALID FOR PROMOTION)")
    else:
        print("fun gate: open")
    return 0


if __name__ == "__main__":
    sys.exit(main())
