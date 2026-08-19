"""Weight-space evolution on the shipped Q12 artifact.

The autopilot searches curriculum knobs around a PPO inner loop; this
instrument searches the weights themselves. No torch, no league, no
export: a candidate IS a Q12 artifact (the same JSON the sim loads),
mutation is integer perturbation, and fitness is the same native
neural-cup the promotion battery trusts. Where gradient fine-tuning
must be constrained after the fact (auto-3/auto-4: the gates only
selected after PPO had already spent the personalities), selection
IS the optimizer here, so the style gate can sit inside the loop as
a per-generation trust region: a center update that worsens the
signature is rejected and the search step shrinks.

The sim's determinism contract binds the artifact and the cup, not
this searcher — Python floats are fine on this side because every
evaluated candidate is written back as integer Q12 before any game
runs. Candidate fitness is deterministic per artifact: the cup plays
a fixed seed suite from both seats, so a generation is a paired
comparison and a fitness difference is a weights difference.

Usage, from tools/train:

  uv run es.py --name es-1 --hours 9
  uv run es.py --name es-1 --hours 2   # resumes from the journal
"""

from __future__ import annotations

import argparse
import copy
import json
import os
import pathlib
import shutil
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor

import numpy as np

from autopilot import style_failures

MAX_COEFF = 1 << 20  # the loader's magnitude ceiling (export.py)

TensorSpec = list[tuple[tuple[str | int, ...], tuple[int, ...]]]

FAMILY_NAMES = ("development", "fortification", "force", "mobile pressure")


def artifact_paths(artifact: dict) -> TensorSpec:
    """Every weight tensor's (path, shape), in a fixed order."""
    spec: TensorSpec = []
    for index, layer in enumerate(artifact["layers"]):
        w = np.asarray(layer["w"], dtype=np.float64)
        b = np.asarray(layer["b"], dtype=np.float64)
        spec.append((("layers", index, "w"), w.shape))
        spec.append((("layers", index, "b"), b.shape))
    head_w = np.asarray(artifact["head"]["w"], dtype=np.float64)
    head_b = np.asarray(artifact["head"]["b"], dtype=np.float64)
    spec.append((("head", "w"), head_w.shape))
    spec.append((("head", "b"), head_b.shape))
    return spec


def leaf_parent(artifact: dict, path: tuple[str | int, ...]) -> dict:
    """The dict holding a spec path's terminal "w"/"b" entry: paths are
    ("layers", index, key) or ("head", key), so the parent is always
    the per-layer (or head) dict."""
    if len(path) == 3:
        return artifact[path[0]][path[1]]
    return artifact[path[0]]


def flatten(artifact: dict) -> tuple[np.ndarray, TensorSpec]:
    spec = artifact_paths(artifact)
    parts = [
        np.asarray(leaf_parent(artifact, path)[path[-1]], dtype=np.float64).ravel()
        for path, _ in spec
    ]
    return np.concatenate(parts), spec


def unflatten(vector: np.ndarray, spec: TensorSpec, template: dict) -> dict:
    """A fresh artifact with `vector` written back as clamped Q12 ints.

    Candidates drop the founder's lineage stanza: a mutated net is not
    the artifact that lineage describes, and the loader accepts its
    absence (export.py omits it for lineage-less checkpoints too).
    """
    out = copy.deepcopy(template)
    out.pop("lineage", None)
    ints = np.clip(np.rint(vector), -MAX_COEFF, MAX_COEFF).astype(np.int64)
    cursor = 0
    for path, shape in spec:
        size = int(np.prod(shape))
        block = ints[cursor : cursor + size].reshape(shape)
        leaf_parent(out, path)[path[-1]] = block.tolist()
        cursor += size
    if cursor != vector.size:
        raise ValueError(f"vector has {vector.size} params, spec covers {cursor}")
    return out


def sigma_vector(
    vector: np.ndarray, spec: TensorSpec, rel: float, floor: float = 1.0
) -> np.ndarray:
    """Per-parameter mutation scale: `rel` x its tensor's weight std.

    The floor keeps near-constant tensors (biases can be) mutable at
    all — one Q12 unit is the smallest representable nudge.
    """
    sigma = np.empty_like(vector)
    cursor = 0
    for _, shape in spec:
        size = int(np.prod(shape))
        block = vector[cursor : cursor + size]
        sigma[cursor : cursor + size] = max(floor, float(np.std(block)) * rel)
        cursor += size
    return sigma


def rank_weights(n: int) -> np.ndarray:
    """Centered rank weights in [-0.5, 0.5], summing to zero, for the
    fitness-sorted population (worst first)."""
    if n < 2:
        raise ValueError("rank weights need at least two candidates")
    return np.arange(n, dtype=np.float64) / (n - 1) - 0.5


def parse_cup(stdout: str) -> dict:
    scores: dict = {}
    for raw_line in stdout.splitlines():
        line = raw_line.strip()
        if not line.startswith("{"):
            continue
        row = json.loads(line)
        if "opponent" not in row:
            continue
        key = str(row["opponent"]).lower()
        scores[f"{key}_wins"] = row["wins"]
        scores[f"{key}_games"] = row["games"]
    return scores


RUSHER_WEIGHT = 2


def cup_wins(scores: dict) -> int:
    """Fitness weights the rush canary double: the shipped line's one
    open weakness is Cupric-side rush defense, and an equal-weight cup
    lets Overseer polish crowd out canary recovery — es-4 spent 2.5
    hours re-earning 11 canary wins at weight one."""
    return scores.get("overseer_wins", 0) + RUSHER_WEIGHT * scores.get("rusher_wins", 0)


def parse_family_counts(report: str) -> dict[str, int] | None:
    """The per-family held-seed counts from the style gate's summary
    line, or None when the gate died before printing one."""
    for raw_line in report.splitlines():
        line = raw_line.strip()
        if not line.startswith("style-family signatures"):
            continue
        _, _, tail = line.partition(":")
        counts: dict[str, int] = {}
        for cell in tail.split(","):
            name, _, held = cell.strip().rpartition(" ")
            if name and held.isdigit():
                counts[name] = int(held)
        return counts
    return None


def accept_center(previous: dict, candidate: dict, win_slack: int) -> tuple[bool, str]:
    """The trust region: a new center must not worsen the style gate
    and must hold cup strength within `win_slack` games. Both sides
    are measured on the same deterministic suites, so a comparison is
    exact, not statistical."""
    if candidate["style_failures"] > previous["style_failures"]:
        return False, "style gate worsened"
    families_prev = previous.get("families") or {}
    families_new = candidate.get("families") or {}
    for name, held in families_prev.items():
        if families_new.get(name, 0) < min(held, 4):
            return False, f"family '{name}' fell below its held floor"
    if candidate["wins"] < previous["wins"] - win_slack:
        return False, "cup strength fell past the slack"
    return True, "ok"


class Battery:
    """Subprocess wrappers around the instruments the battery trusts."""

    def __init__(self, driver: str, repo_root: pathlib.Path) -> None:
        self.driver = driver
        self.repo_root = repo_root

    def cup(self, weights: pathlib.Path, seeds: int) -> dict:
        result = subprocess.run(
            [
                self.driver,
                "neural-cup",
                "--weights",
                str(weights),
                "--seeds",
                str(seeds),
            ],
            check=True,
            capture_output=True,
            text=True,
        )
        scores = parse_cup(result.stdout)
        scores["seeds"] = seeds
        return scores

    def style(self, weights: pathlib.Path) -> dict:
        result = subprocess.run(
            [
                shutil.which("cargo") or "cargo",
                "test",
                "--release",
                "-p",
                "oxide-sim",
                "--test",
                "bot_profiles",
                "candidate_profile_behavior_gates",
                "--locked",
                "--",
                "--ignored",
                "--nocapture",
            ],
            capture_output=True,
            text=True,
            check=False,
            cwd=self.repo_root,
            env={**os.environ, "OXIDE_PROFILE_WEIGHTS": str(weights.resolve())},
        )
        report = result.stdout + "\n" + result.stderr
        return {
            "style_pass": result.returncode == 0,
            "style_failures": style_failures(report) if result.returncode else [],
            "families": parse_family_counts(report),
        }

    def fun(self, weights: pathlib.Path) -> dict:
        result = subprocess.run(
            [sys.executable, "fun_gate.py", "--weights", str(weights)],
            capture_output=True,
            text=True,
            check=False,
        )
        return {
            "fun_pass": result.returncode == 0,
            "fun_failures": [
                line.strip()
                for line in result.stdout.splitlines()
                if "FUN GATE FAIL" in line
            ],
        }


def measure_center(battery: Battery, weights: pathlib.Path, seeds: int) -> dict:
    cup = battery.cup(weights, seeds)
    style = battery.style(weights)
    return {
        "wins": cup_wins(cup),
        "cup": cup,
        "style_failures": len(style["style_failures"]),
        "style_failure_lines": style["style_failures"],
        "families": style["families"],
        "style_pass": style["style_pass"],
    }


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--name", required=True, help="run name under runs/")
    ap.add_argument(
        "--founder",
        default="../../sim/src/bot/ladder_weights.json",
        help="Q12 artifact to evolve from",
    )
    ap.add_argument("--pairs", type=int, default=12, help="antithetic pairs per gen")
    ap.add_argument("--screen-seeds", type=int, default=12)
    ap.add_argument("--center-seeds", type=int, default=24)
    ap.add_argument("--confirm-seeds", type=int, default=48)
    ap.add_argument("--sigma-rel", type=float, default=0.02)
    ap.add_argument("--lr", type=float, default=1.0)
    ap.add_argument(
        "--win-slack",
        type=int,
        default=2,
        help="cup games a center update may give back and still land",
    )
    ap.add_argument("--generations", type=int, default=10_000)
    ap.add_argument("--hours", type=float, default=None, help="wall-clock budget")
    ap.add_argument("--confirm-every", type=int, default=25)
    ap.add_argument("--seed", type=int, default=0, help="mutation rng seed")
    ap.add_argument("--jobs", type=int, default=2, help="concurrent cup processes")
    ap.add_argument(
        "--driver",
        default=os.environ.get("OXIDE_DRIVER_BIN", "../../target/release/oxide-driver"),
    )
    args = ap.parse_args()

    repo_root = pathlib.Path(__file__).resolve().parent.parent.parent
    battery = Battery(args.driver, repo_root)
    root = pathlib.Path("runs") / args.name
    root.mkdir(parents=True, exist_ok=True)
    journal_path = root / "journal.jsonl"
    latest_path = root / "latest.json"
    scratch = root / "scratch"
    scratch.mkdir(exist_ok=True)

    start_generation = 0
    sigma_scale = 1.0
    if latest_path.exists() and journal_path.exists():
        rows = [
            json.loads(line)
            for line in journal_path.read_text().splitlines()
            if line.strip()
        ]
        if rows:
            last = rows[-1]
            start_generation = int(last["generation"]) + 1
            sigma_scale = float(last.get("sigma_scale", 1.0))
        template = json.loads(latest_path.read_text())
        print(f"resuming {args.name} at generation {start_generation}")
    else:
        template = json.loads(pathlib.Path(args.founder).read_text())

    center, spec = flatten(template)
    base_sigma = sigma_vector(center, spec, args.sigma_rel)

    center_path = root / "center.json"
    center_path.write_text(json.dumps(unflatten(center, spec, template)))
    center_scores = measure_center(battery, center_path, args.center_seeds)
    print(
        f"center @g{start_generation}: wins {center_scores['wins']}"
        f" · families {center_scores['families']}"
        f" · style {'PASS' if center_scores['style_pass'] else 'FAIL'}"
    )

    deadline = time.monotonic() + args.hours * 3600 if args.hours else None
    # (generation, wins, fun_pass); a fun-passing confirm outranks any
    # failing one so selection cannot trade the fun gate for cup wins.
    best_confirmed: tuple[int, int, bool] | None = None

    for generation in range(start_generation, start_generation + args.generations):
        if deadline is not None and time.monotonic() > deadline:
            print(f"wall-clock budget reached before generation {generation}")
            break
        began = time.monotonic()
        rng = np.random.default_rng((args.seed << 20) + generation)
        sigma = base_sigma * sigma_scale
        noises = [rng.standard_normal(center.size) * sigma for _ in range(args.pairs)]
        deltas = [signed * n for n in noises for signed in (1.0, -1.0)]

        candidate_paths = []
        for index, delta in enumerate(deltas):
            path = scratch / f"cand-{index:03d}.json"
            path.write_text(json.dumps(unflatten(center + delta, spec, template)))
            candidate_paths.append(path)

        with ThreadPoolExecutor(max_workers=args.jobs) as pool:
            futures = [
                pool.submit(battery.cup, path, args.screen_seeds)
                for path in candidate_paths
            ]
            fitness = np.array(
                [float(cup_wins(f.result())) for f in futures], dtype=np.float64
            )

        spread = float(fitness.max() - fitness.min())
        if spread == 0.0:
            # Every mutant played an identical cup: the step is below
            # the sim's decision granularity. Search wider.
            sigma_scale = min(sigma_scale * 1.3, 8.0)
            row = {
                "generation": generation,
                "kind": "flat",
                "sigma_scale": sigma_scale,
                "wins": center_scores["wins"],
                "elapsed": round(time.monotonic() - began, 1),
            }
            with journal_path.open("a") as sink:
                sink.write(json.dumps(row) + "\n")
            print(f"g{generation}: flat fitness at sigma {sigma_scale:.2f}, widened")
            continue

        order = np.argsort(fitness, kind="stable")
        weights = np.empty_like(fitness)
        weights[order] = rank_weights(len(deltas))
        step = (
            args.lr
            * (2.0 / len(deltas))
            * sum(w * d for w, d in zip(weights, deltas, strict=True))
        )
        proposal = center + step

        proposal_path = scratch / "proposal.json"
        proposal_path.write_text(json.dumps(unflatten(proposal, spec, template)))
        proposal_scores = measure_center(battery, proposal_path, args.center_seeds)
        accepted, verdict = accept_center(
            center_scores, proposal_scores, args.win_slack
        )
        if accepted:
            center = proposal
            center_scores = proposal_scores
            center_path.write_text(json.dumps(unflatten(center, spec, template)))
            latest_path.write_text(center_path.read_text())
            sigma_scale = min(sigma_scale * 1.05, 8.0)
        else:
            sigma_scale = max(sigma_scale * 0.7, 0.05)

        row = {
            "generation": generation,
            "kind": "step",
            "accepted": accepted,
            "verdict": verdict,
            "sigma_scale": round(sigma_scale, 4),
            "screen_best": int(fitness.max()),
            "screen_spread": int(spread),
            "wins": center_scores["wins"],
            "families": center_scores["families"],
            "style_pass": center_scores["style_pass"],
            "elapsed": round(time.monotonic() - began, 1),
        }
        with journal_path.open("a") as sink:
            sink.write(json.dumps(row) + "\n")
        print(
            f"g{generation}: {'ACCEPT' if accepted else 'reject (' + verdict + ')'}"
            f" · wins {center_scores['wins']} · families {center_scores['families']}"
            f" · sigma {sigma_scale:.2f} · {row['elapsed']}s"
        )

        confirm_due = accepted and generation % args.confirm_every == 0
        if confirm_due:
            confirm_cup = battery.cup(center_path, args.confirm_seeds)
            fun = battery.fun(center_path)
            keep = root / f"center-g{generation:04d}.json"
            keep.write_text(center_path.read_text())
            confirmed = cup_wins(confirm_cup)
            rank = (fun["fun_pass"], confirmed)
            if best_confirmed is None or rank > (best_confirmed[2], best_confirmed[1]):
                best_confirmed = (generation, confirmed, fun["fun_pass"])
                (root / "best.json").write_text(center_path.read_text())
            row = {
                "generation": generation,
                "kind": "confirm",
                "confirm_wins": confirmed,
                "confirm_games": confirm_cup.get("overseer_games", 0)
                + confirm_cup.get("rusher_games", 0),
                "fun_pass": fun["fun_pass"],
                "fun_failures": fun["fun_failures"],
                "sigma_scale": round(sigma_scale, 4),
            }
            with journal_path.open("a") as sink:
                sink.write(json.dumps(row) + "\n")
            print(
                f"g{generation} confirm: {confirmed}/{row['confirm_games']}"
                f" · fun {'PASS' if fun['fun_pass'] else 'FAIL'}"
            )

    print("es complete")
    if best_confirmed is not None:
        print(
            f"best confirmed center: g{best_confirmed[0]}"
            f" ({best_confirmed[1]} wins"
            f" · fun {'PASS' if best_confirmed[2] else 'FAIL'})"
        )
    print(f"journal: {journal_path}")


if __name__ == "__main__":
    main()
