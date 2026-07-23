"""The fun gate, executable: probe a candidate artifact's composition
and pass judgment. A checkpoint that spams one kind fails promotion the
way a stalling one fails the draw rule.

    uv run fun_gate.py --weights runs/candidate.json

Thresholds are deliberate: entropy >= 1.8 bits demands a real mix
(three-plus kinds pulling weight), and at least one Fabricator-gated
kind must carry >= 3% of army value — the tech tree must actually be
climbed, not visited.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import subprocess
import sys
import tempfile

TECH_KINDS = {
    "scuttler", "lancer", "bombard", "flakhound",
    "stinger", "buzzard", "darter", "talon", "wisp",
}


def judge(overall: dict, min_entropy: float, min_tech_share: float) -> list[str]:
    """Returns the list of failures (empty = the gate opens)."""
    failures = []
    entropy = overall["entropy_bits"]
    if entropy < min_entropy:
        failures.append(f"mix entropy {entropy:.2f} bits < {min_entropy} — spam")
    shares = overall["mean_share"]
    tech = sum(v for k, v in shares.items() if k in TECH_KINDS)
    if tech < min_tech_share:
        failures.append(
            f"tech kinds carry {tech * 100:.1f}% of army value < {min_tech_share * 100:.0f}% — the tree was never climbed"
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
    ap.add_argument("--min-tech-share", type=float, default=0.03)
    args = ap.parse_args()

    out = pathlib.Path(tempfile.mkstemp(suffix=".json")[1])
    subprocess.run(
        [
            args.driver, "balance-probe",
            "--dir", args.scenarios,
            "--level", args.level,
            "--seeds", str(args.seeds),
            "--weights", args.weights,
            "--out", str(out),
        ],
        check=True,
        capture_output=True,
    )
    overall = json.loads(out.read_text())["overall"]
    failures = judge(overall, args.min_entropy, args.min_tech_share)
    print(f"entropy {overall['entropy_bits']:.2f} bits · shares "
          + ", ".join(f"{k} {v*100:.1f}%" for k, v in sorted(overall["mean_share"].items(), key=lambda kv: -kv[1])))
    if failures:
        for f in failures:
            print(f"FUN GATE FAIL: {f}")
        return 1
    print("fun gate: open")
    return 0


if __name__ == "__main__":
    sys.exit(main())
