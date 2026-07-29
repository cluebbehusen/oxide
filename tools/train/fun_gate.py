"""The fun gate, executable: probe a candidate artifact's composition
and pass judgment. A checkpoint that spams one kind fails promotion the
way a stalling one fails the draw rule.

    uv run fun_gate.py --weights runs/candidate.json

Promotion checks each failure mode separately:

  --min-entropy (1.8 bits) on the entropy of the mean mix: a real army
    is three-plus kinds pulling weight.
  --min-seat-entropy (1.45 bits) on the tenth-percentile seat: two
    players spamming different kinds must not average into a pass.
  --min-count-entropy (2.05), --min-seat-count-entropy (1.45), and
    --max-count-dominance (0.60) apply the same question to integrated
    body-time rather than scrap value. --max-mean-count-share (0.40)
    also caps the leading kind across the slate, so a replacement
    cannot clear the generic diversity checks while leaving the
    Scuttler wall intact.
  --min-decided-rate rejects a candidate whose varied-looking armies
    mostly came from matches it would not finish.
  --min-tech-share (0.25) on the SUM over the Fabricator-gated kinds:
    was the tech tree climbed at all.
  --min-top-tech-share (0.03) on the LARGEST single tech kind: the
    sum can be cleared by many kinds each individually negligible
    (say ten at 2.6%); this floor demands that at least one of them
    was actually chosen. The sum says the tree was visited; this says
    something on it was worth building.

Judgment reads the DECIDED cohort of `driver balance-probe --out`. A
stalemate's army mix is evidence about a stalemate, not about what a
policy chose to build.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import subprocess
import sys
import tempfile

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

# The exact `--out` payload shape this gate reads. Schema 3 introduced
# the body-count lens; accepting any other schema risks silently judging
# a different contract.
EXPECTED_SCHEMA = 3


def judge(
    cohort: dict,
    decided_rate: float,
    min_decided_rate: float,
    min_entropy: float,
    min_seat_entropy: float,
    min_count_entropy: float,
    min_seat_count_entropy: float,
    max_count_dominance: float,
    max_mean_count_share: float,
    min_tech_share: float,
    min_top_tech_share: float,
) -> list[str]:
    """Returns the list of failures (empty = the gate opens)."""
    failures = []
    if decided_rate < min_decided_rate:
        failures.append(
            f"decided rate {decided_rate * 100:.1f}% "
            f"< {min_decided_rate * 100:.0f}% — too many stalls"
        )
    entropy = cohort["entropy_bits"]
    if entropy < min_entropy:
        failures.append(f"mix entropy {entropy:.2f} bits < {min_entropy} — spam")
    seat_entropy = cohort["seat_entropy"]["p10"]
    if seat_entropy < min_seat_entropy:
        failures.append(
            f"per-seat value entropy p10 {seat_entropy:.2f} bits "
            f"< {min_seat_entropy} — some seats spam"
        )
    count_entropy = cohort["count_entropy_bits"]
    if count_entropy < min_count_entropy:
        failures.append(
            f"body-time entropy {count_entropy:.2f} bits "
            f"< {min_count_entropy} — cheap-unit presence is too narrow"
        )
    seat_count_entropy = cohort["seat_count_entropy"]["p10"]
    if seat_count_entropy < min_seat_count_entropy:
        failures.append(
            f"per-seat body-time entropy p10 {seat_count_entropy:.2f} bits "
            f"< {min_seat_count_entropy} — some seats rely on too few units"
        )
    dominance = cohort["seat_count_dominance"]["p90"]
    if dominance > max_count_dominance:
        failures.append(
            f"per-seat largest body-time share p90 {dominance * 100:.1f}% "
            f"> {max_count_dominance * 100:.0f}% — one unit dominates over time"
        )
    top_count_kind, top_count_share = max(
        cohort["mean_count_share"].items(), key=lambda item: item[1]
    )
    if top_count_share > max_mean_count_share:
        failures.append(
            f"mean {top_count_kind} body-time share {top_count_share * 100:.1f}% "
            f"> {max_mean_count_share * 100:.0f}% — one unit dominates the slate"
        )
    shares = cohort["mean_share"]
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


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--weights", required=True)
    ap.add_argument("--driver", default="../../target/release/oxide-driver")
    ap.add_argument("--scenarios", default="../../scenarios")
    ap.add_argument("--level", default="medium")
    ap.add_argument("--seeds", type=int, default=2)
    ap.add_argument("--min-decided-rate", type=float, default=0.70)
    ap.add_argument("--min-entropy", type=float, default=1.8)
    ap.add_argument("--min-seat-entropy", type=float, default=1.45)
    ap.add_argument("--min-count-entropy", type=float, default=2.05)
    ap.add_argument("--min-seat-count-entropy", type=float, default=1.45)
    ap.add_argument("--max-count-dominance", type=float, default=0.60)
    ap.add_argument("--max-mean-count-share", type=float, default=0.40)
    ap.add_argument("--min-tech-share", type=float, default=0.25)
    ap.add_argument("--min-top-tech-share", type=float, default=0.03)
    args = ap.parse_args()

    out = pathlib.Path(tempfile.mkstemp(suffix=".json")[1])
    subprocess.run(
        [
            args.driver,
            "balance-probe",
            "--dir",
            args.scenarios,
            "--level",
            args.level,
            "--seeds",
            str(args.seeds),
            "--weights",
            args.weights,
            "--out",
            str(out),
        ],
        check=True,
        capture_output=True,
    )
    payload = json.loads(out.read_text())
    schema = payload.get("schema", 1)
    if schema != EXPECTED_SCHEMA:
        print(
            f"FUN GATE FAIL: probe payload is schema {schema}, this gate reads "
            f"exactly {EXPECTED_SCHEMA} — use the matching driver"
        )
        return 1
    overall, decided = payload["overall"], payload["decided"]
    if decided["seats"] == 0:
        print(
            f"FUN GATE FAIL: none of {overall['matches']} matches was decided — "
            "a slate of stalemates is no evidence about army choice"
        )
        return 1
    decided_rate = overall["decided"] / overall["matches"]
    failures = judge(
        decided,
        decided_rate,
        args.min_decided_rate,
        args.min_entropy,
        args.min_seat_entropy,
        args.min_count_entropy,
        args.min_seat_count_entropy,
        args.max_count_dominance,
        args.max_mean_count_share,
        args.min_tech_share,
        args.min_top_tech_share,
    )
    shares = sorted(decided["mean_share"].items(), key=lambda kv: -kv[1])
    listed = ", ".join(f"{k} {v * 100:.1f}%" for k, v in shares)
    count_shares = sorted(decided["mean_count_share"].items(), key=lambda kv: -kv[1])
    count_listed = ", ".join(f"{k} {v * 100:.1f}%" for k, v in count_shares)
    print(
        f"judging {decided['decided']} decided of {overall['matches']} matches "
        f"({decided['seats']} seats)"
    )
    print(
        f"entropy {decided['entropy_bits']:.2f} bits · "
        f"per-seat p10 {decided['seat_entropy']['p10']:.2f} bits · shares {listed}"
    )
    print(
        f"body-time entropy {decided['count_entropy_bits']:.2f} bits · "
        f"per-seat p10 {decided['seat_count_entropy']['p10']:.2f} bits · "
        f"largest-body-time p90 "
        f"{decided['seat_count_dominance']['p90'] * 100:.1f}% · "
        f"shares {count_listed}"
    )
    if failures:
        for f in failures:
            print(f"FUN GATE FAIL: {f}")
        return 1
    print("fun gate: open")
    return 0


if __name__ == "__main__":
    sys.exit(main())
