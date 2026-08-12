"""Population-based training autopilot: explore the knob space, keep
the weights that survive the battery.

Each generation trains every population member for a fixed number of
league updates from its own checkpoint, exports the result, and scores
it on the same instruments promotion uses: the native neural-cup is
the fitness, and the fun gate is a hard constraint (a candidate that
fails it can win nothing). The bottom half of the population is then
replaced by perturbed clones of the top half and the loop continues.

The knobs the autopilot explores are exactly the ones the 0.15
campaign hand-titrated between rounds: the opponent mix, the map mix,
and the production-entropy regularizer. What it deliberately does NOT
explore: balance constants, gate floors, and reward shaping — those
are design decisions, not search dimensions. Anomalies (lopsided seat
splits, gate failures across the whole population) are printed as
WARN lines for the human or agent reviewing between generations;
searching around a systematic defect produces confidently wrong
weights, so the autopilot surfaces evidence instead of burying it.

League episode seeds are deterministic per phase position, so every
member of a generation trains and evaluates on the same slates: the
comparison is paired, and a fitness difference is a config
difference, not a seed draw.

Usage, from tools/train:

  uv run autopilot.py --name auto-1 \
      --initialize-from lineage-checkpoints/r17-distilled.pt \
      --population 4 --updates 60 --generations 3
"""

import argparse
import json
import os
import pathlib
import random
import subprocess
import sys

MIX_KEYS = ("self", "past", "overseer", "rusher", "ffa", "team")
MAP_KEYS = ("fixed", "random", "grand")

SEED_CONFIG = {
    "mix": {
        "self": 0.20,
        "past": 0.10,
        "overseer": 0.25,
        "rusher": 0.20,
        "ffa": 0.10,
        "team": 0.15,
    },
    "map_mix": {"fixed": 0.35, "random": 0.30, "grand": 0.35},
    "production_entropy_coef": 0.0,
}


def perturb(config: dict, rng: random.Random) -> dict:
    """One mutation step: jitter a couple of mix weights and sometimes
    the entropy regularizer, then renormalize the mixes."""
    out = json.loads(json.dumps(config))
    for key_set, field in ((MIX_KEYS, "mix"), (MAP_KEYS, "map_mix")):
        weights = out[field]
        for key in rng.sample(key_set, k=2 if field == "mix" else 1):
            weights[key] = max(0.02, weights[key] * rng.uniform(0.7, 1.4))
        total = sum(weights.values())
        for key in weights:
            weights[key] = round(weights[key] / total, 4)
    if rng.random() < 0.5:
        current = out["production_entropy_coef"]
        out["production_entropy_coef"] = round(
            min(0.006, max(0.0, current + rng.uniform(-0.001, 0.001))), 5
        )
    return out


def mix_arg(weights: dict) -> str:
    return ",".join(f"{key}={value}" for key, value in weights.items())


def run_league(
    name: str, ckpt: str, anchor: str, config: dict, updates: int, log: pathlib.Path
) -> pathlib.Path:
    """One training phase; returns the final pool checkpoint."""
    command = [
        sys.executable,
        "league.py",
        "--name",
        name,
        "--initialize-from",
        ckpt,
        "--anchor",
        anchor,
        "--collection",
        "episodes",
        "--updates",
        str(updates),
        "--mix",
        mix_arg(config["mix"]),
        "--map-mix",
        mix_arg(config["map_mix"]),
        "--faction-mix",
        "ff=.25,fc=.25,cf=.25,cc=.25",
        "--probe-every",
        str(updates),
    ]
    coefficient = config["production_entropy_coef"]
    if coefficient > 0.0:
        command.extend(["--production-entropy-coef", str(coefficient)])
    run_dir = pathlib.Path("runs") / name
    finished = phase_checkpoint(run_dir, updates)
    if finished is not None:
        print(f"    reusing completed phase {name}")
        return finished
    if run_dir.exists():
        crashed = run_dir.with_name(f"{run_dir.name}.crashed")
        suffix = 0
        while crashed.exists():
            suffix += 1
            crashed = run_dir.with_name(f"{run_dir.name}.crashed{suffix}")
        run_dir.rename(crashed)
        print(f"    moved partial phase aside: {crashed}")
    with log.open("w") as sink:
        subprocess.run(command, check=True, stdout=sink, stderr=subprocess.STDOUT)
    checkpoints = sorted((run_dir / "pool").glob("ckpt-*.pt"))
    if not checkpoints:
        raise RuntimeError(f"{name}: league finished without pool checkpoints")
    return checkpoints[-1]


def phase_checkpoint(run_dir: pathlib.Path, updates: int) -> pathlib.Path | None:
    """The final pool checkpoint of an already-completed phase, or None.

    Crash resilience: a relaunched autopilot reuses every member phase
    that already trained to its target update instead of re-spending
    the compute. The phase log's last row is the completion witness."""
    log_path = run_dir / "log.jsonl"
    checkpoints = sorted((run_dir / "pool").glob("ckpt-*.pt"))
    if not checkpoints or not log_path.exists():
        return None
    try:
        last = json.loads(log_path.read_text().splitlines()[-1])
    except ValueError, IndexError:
        return None
    if last.get("phase_update") == updates:
        return checkpoints[-1]
    return None


def run_battery(candidate: pathlib.Path, driver: str, cup_seeds: int) -> dict:
    """Export-side scoring: cup fitness plus the fun-gate constraint.

    Scores persist beside the candidate so a crash-resumed autopilot
    reuses them; the whole battery is deterministic per candidate, so
    a cached verdict is the verdict."""
    memo = candidate.with_suffix(".scores.json")
    if memo.exists():
        try:
            cached = json.loads(memo.read_text())
        except ValueError:
            cached = None
        if isinstance(cached, dict) and cached.get("cup_seeds") == cup_seeds:
            print(f"    reusing battery scores for {candidate.name}")
            return cached
    exported = candidate.with_suffix(".json")
    subprocess.run(
        [sys.executable, "export.py", "--ckpt", str(candidate), "--out", str(exported)],
        check=True,
        capture_output=True,
    )
    cup = subprocess.run(
        [driver, "neural-cup", "--weights", str(exported), "--seeds", str(cup_seeds)],
        check=True,
        capture_output=True,
        text=True,
    )
    scores: dict = {"candidate": str(exported)}
    for raw_line in cup.stdout.splitlines():
        line = raw_line.strip()
        if not line.startswith("{"):
            continue
        row = json.loads(line)
        if "opponent" not in row:
            continue
        key = row["opponent"].lower()
        scores[f"{key}_wins"] = row["wins"]
        scores[f"{key}_games"] = row["games"]
        scores[f"{key}_by_seat"] = [seat["wins"] for seat in row["by_seat"]]
    gate = subprocess.run(
        [sys.executable, "fun_gate.py", "--weights", str(exported)],
        capture_output=True,
        text=True,
        check=False,
    )
    scores["fun_gate_pass"] = gate.returncode == 0
    scores["fun_gate_failures"] = [
        line.strip() for line in gate.stdout.splitlines() if "FUN GATE FAIL" in line
    ]
    scores["cup_seeds"] = cup_seeds
    memo.write_text(json.dumps(scores))
    return scores


def fitness(scores: dict) -> tuple:
    """Constraint first, then fewest gate failures, then combined cup
    wins. A gate failure can never outrank a pass whatever its cup
    says — and while a whole generation sits in the fine-tune dip
    with nothing passing, selection pressure points at gate recovery
    before raw strength."""
    return (
        scores.get("fun_gate_pass", False),
        -len(scores.get("fun_gate_failures", ())),
        scores.get("overseer_wins", 0) + scores.get("rusher_wins", 0),
        scores.get("overseer_wins", 0),
    )


def warn_anomalies(member: str, scores: dict) -> None:
    for opponent in ("overseer", "rusher"):
        seats = scores.get(f"{opponent}_by_seat")
        games = scores.get(f"{opponent}_games", 0)
        if seats and games and max(seats) - min(seats) > games // 3:
            print(f"WARN {member}: lopsided {opponent} seat split {seats}")
    if not scores.get("fun_gate_pass", False):
        for failure in scores.get("fun_gate_failures", []):
            print(f"WARN {member}: {failure}")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--name", required=True, help="autopilot run name")
    ap.add_argument("--initialize-from", required=True, help="founder checkpoint")
    ap.add_argument("--anchor", default="lineage-checkpoints/prior-v9.pt")
    ap.add_argument("--population", type=int, default=4)
    ap.add_argument(
        "--updates",
        type=int,
        default=60,
        help="league updates per member per generation",
    )
    ap.add_argument("--generations", type=int, default=3)
    ap.add_argument("--cup-seeds", type=int, default=30)
    ap.add_argument("--perturb-seed", type=int, default=0, help="mutation rng seed")
    ap.add_argument(
        "--driver",
        default=os.environ.get("OXIDE_DRIVER_BIN", "../../target/release/oxide-driver"),
    )
    args = ap.parse_args()

    rng = random.Random(args.perturb_seed)
    root = pathlib.Path("runs") / args.name
    root.mkdir(parents=True, exist_ok=True)
    journal = root / "generations.jsonl"

    members = []
    for index in range(args.population):
        config = SEED_CONFIG if index == 0 else perturb(SEED_CONFIG, rng)
        members.append({"config": config, "ckpt": args.initialize_from})

    for generation in range(args.generations):
        results = []
        for index, member in enumerate(members):
            name = f"{args.name}/g{generation}m{index}"
            print(f"=== generation {generation} member {index}: training {name}")
            print(f"    config {json.dumps(member['config'])}")
            log = root / f"g{generation}m{index}.log"
            final = run_league(
                name,
                str(member["ckpt"]),
                args.anchor,
                dict(member["config"]),
                args.updates,
                log,
            )
            scores = run_battery(final, args.driver, args.cup_seeds)
            warn_anomalies(f"g{generation}m{index}", scores)
            results.append(
                {
                    "member": index,
                    "ckpt": str(final),
                    "scores": scores,
                    "config": member["config"],
                }
            )
            overseer = f"{scores.get('overseer_wins')}/{scores.get('overseer_games')}"
            print(
                f"    overseer {overseer}"
                f" · rusher {scores.get('rusher_wins')}/{scores.get('rusher_games')}"
                f" · fun gate {'PASS' if scores.get('fun_gate_pass') else 'FAIL'}"
            )

        ranked = sorted(results, key=lambda row: fitness(row["scores"]), reverse=True)
        with journal.open("a") as sink:
            sink.write(json.dumps({"generation": generation, "ranked": ranked}) + "\n")
        print(f"=== generation {generation} ranking:")
        for row in ranked:
            scores = row["scores"]
            verdict = "PASS" if scores.get("fun_gate_pass") else "FAIL"
            print(f"    m{row['member']}: gate {verdict} · cup {fitness(scores)[1]}")

        survivors = ranked[: max(1, len(ranked) // 2)]
        members = []
        for row in survivors:
            members.append({"config": row["config"], "ckpt": row["ckpt"]})
        while len(members) < args.population:
            parent = rng.choice(survivors)
            members.append(
                {"config": perturb(parent["config"], rng), "ckpt": parent["ckpt"]}
            )

    best = ranked[0]
    print("=== autopilot complete")
    best_scores = best["scores"]
    print(f"best: {best_scores.get('candidate')} · fitness {fitness(best_scores)}")
    print(f"journal: {journal}")
    if not best["scores"].get("fun_gate_pass"):
        print(
            "NOTE: no candidate passed the fun gate — "
            "review WARN lines before continuing"
        )
        sys.exit(1)


if __name__ == "__main__":
    main()
