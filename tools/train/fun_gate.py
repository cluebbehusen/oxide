"""The fun gate, executable: probe a candidate artifact's composition
and pass judgment. A checkpoint that spams one kind fails promotion the
way a stalling one fails the draw rule.

    uv run fun_gate.py --weights runs/candidate.json

Three thresholds, each catching a different failure:

  --min-entropy (1.8 bits) on the entropy of the mean mix: a real army
    is three-plus kinds pulling weight.
  --min-tech-share (0.25) on the SUM over the Fabricator-gated kinds:
    was the tech tree climbed at all.
  --min-top-tech-share (0.03) on the LARGEST single tech kind: nine
    tech kinds at 0.4% each clear the sum while not one of them was
    ever worth building. The sum says the tree was visited; this says
    something on it was chosen.

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

# The `--out` payload shape this gate reads. Below it, `decided` does
# not exist and the gate would silently judge a different cohort.
MIN_SCHEMA = 2


def judge(
    cohort: dict,
    min_entropy: float,
    min_tech_share: float,
    min_top_tech_share: float,
) -> list[str]:
    """Returns the list of failures (empty = the gate opens)."""
    failures = []
    entropy = cohort["entropy_bits"]
    if entropy < min_entropy:
        failures.append(f"mix entropy {entropy:.2f} bits < {min_entropy} — spam")
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
    ap.add_argument("--min-entropy", type=float, default=1.8)
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
    if schema < MIN_SCHEMA:
        print(
            f"FUN GATE FAIL: probe payload is schema {schema}, this gate reads "
            f"{MIN_SCHEMA} — rebuild the driver"
        )
        return 1
    overall, decided = payload["overall"], payload["decided"]
    if decided["seats"] == 0:
        print(
            f"FUN GATE FAIL: none of {overall['matches']} matches was decided — "
            "a slate of stalemates is no evidence about army choice"
        )
        return 1
    failures = judge(
        decided, args.min_entropy, args.min_tech_share, args.min_top_tech_share
    )
    shares = sorted(decided["mean_share"].items(), key=lambda kv: -kv[1])
    listed = ", ".join(f"{k} {v * 100:.1f}%" for k, v in shares)
    print(
        f"judging {decided['decided']} decided of {overall['matches']} matches "
        f"({decided['seats']} seats)"
    )
    print(
        f"entropy {decided['entropy_bits']:.2f} bits · "
        f"per-seat p10 {decided['seat_entropy']['p10']:.2f} bits · shares {listed}"
    )
    if failures:
        for f in failures:
            print(f"FUN GATE FAIL: {f}")
        return 1
    print("fun gate: open")
    return 0


if __name__ == "__main__":
    sys.exit(main())
