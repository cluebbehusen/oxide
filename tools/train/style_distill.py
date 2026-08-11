"""Distill teacher openings into the profile-conditioning columns.

The league's terminal style bonus aligns play with facets too weakly to
move the five profile input columns on its own (r14 measured a flat
bonus across 80 profile-columns-only updates). This tool teaches the
columns directly: each scripted v9 teacher strategy demonstrates under
the *named* facet condition whose personality it embodies, and cloning
loss flows only through the profile columns — the trunk, heads, and
raw-aggression path stay byte-identical, which the promotion battery's
parent-match gate verifies.

Teacher-to-profile mapping (styles and variants are Rust-authored):

  fortify  -> turtle/fortress        industry -> turtle/industrial-attrition
  combined -> balanced/ground+air    pressure -> aggressive (all variants)

Usage, from tools/train:

  uv run style_distill.py --initialize-from runs/ckpt.pt \
      --episodes 24 --epochs 12 --out runs/distilled.pt
"""

import argparse
import os
import pathlib

import numpy as np
import torch

from bc import (
    duel_scenarios,
    local_action_targets,
    masked_head_cross_entropy,
    teacher,
)
from league import PROFILE_CONDITION_COUNT, profile_column_parameters
from lineage import build_lineage, content_digest, input_identity
from models import load_policy, save_policy
from oxide_gym import (
    ACTION_HEADS,
    FEATURES,
    GYM_VERSION,
    Worker,
    with_condition,
)

TEACHER_PROFILES: dict[str, list[tuple[str, int]]] = {
    # No industry -> turtle/industrial-attrition pair: the industry
    # teacher caps its opening at one Reclaimer, and cloning that onto
    # the industrial profile taught it to under-build the economy the
    # fun gate demands of it. The trunk already carries turtle-led
    # development; distillation only needs to instill the signatures
    # PPO washed out — fortress walls and pressure aggression.
    "fortify": [("turtle", 0)],
    "combined": [("balanced", 0), ("balanced", 1)],
    "pressure": [("aggressive", 0), ("aggressive", 1), ("aggressive", 2)],
}


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--initialize-from", required=True, help="checkpoint to continue")
    ap.add_argument("--episodes", type=int, default=24, help="episodes per strategy")
    ap.add_argument(
        "--max-ticks",
        type=int,
        default=20_000,
        help="episode tick cap; a low cap concentrates demonstrations on "
        "the opening the style gates measure",
    )
    ap.add_argument("--epochs", type=int, default=12)
    ap.add_argument("--lr", type=float, default=3e-4)
    ap.add_argument("--out", required=True, help="output checkpoint path")
    ap.add_argument(
        "--driver",
        default=os.environ.get("OXIDE_DRIVER_BIN", "../../target/release/oxide-driver"),
    )
    ap.add_argument("--scenarios", default="../../scenarios")
    args = ap.parse_args()

    torch.manual_seed(0)
    scenarios = duel_scenarios(pathlib.Path(args.scenarios))
    worker = Worker(args.driver)
    obs_all: list[np.ndarray] = []
    mask_all: list[np.ndarray] = []
    act_all: list[tuple[int, int, int, int]] = []
    wins = 0
    episodes = 0
    try:
        catalog = worker.profile_catalog
        for strategy, profiles in TEACHER_PROFILES.items():
            for episode in range(args.episodes):
                style, variant = profiles[episode % len(profiles)]
                scenario = scenarios[episode % len(scenarios)]
                seed = 30_000 + episode
                seat = episode % 2
                condition = catalog.condition(
                    style,
                    variant,
                    catalog.default_role,
                    "ferrous" if seat % 2 == 0 else "cupric",
                )
                frame = worker.reset(
                    seed=seed,
                    control=(seat,),
                    max_ticks=args.max_ticks,
                    scenario=str(scenario),
                    conditions={seat: condition},
                )
                while not frame.done:
                    view = frame.seats.get(seat)
                    if view is None:
                        frame = worker.step({})
                        continue
                    plan = teacher(strategy, view.raw, view.mask, frame.tick)
                    observed = (
                        with_condition(view.obs, condition)
                        if len(view.obs) == FEATURES
                        else np.asarray(view.obs)
                    )
                    obs_all.append(observed)
                    mask_all.append(np.asarray(view.mask))
                    act_all.append(plan)
                    frame = worker.step({seat: plan})
                episodes += 1
                if frame.reward(seat) > 0:
                    wins += 1
        print(f"teacher: {wins}/{episodes} wins, {len(act_all)} samples")
    finally:
        worker.close()

    obs = torch.as_tensor(np.stack(obs_all))
    mask = torch.as_tensor(np.stack(mask_all))
    local_targets = []
    class_weights = []
    for head, local in zip(ACTION_HEADS, local_action_targets(act_all), strict=True):
        counts = np.bincount(local, minlength=len(head))
        weights = np.sqrt(max(int(counts.max()), 1) / np.maximum(counts, 1))
        weights = np.minimum(weights, 10.0)
        local_targets.append(torch.as_tensor(local))
        class_weights.append(torch.as_tensor(weights, dtype=torch.float32))

    policy, blob = load_policy(args.initialize_from)
    trainable = profile_column_parameters(policy)
    columns = trainable[0]
    opt = torch.optim.Adam([columns], lr=args.lr)
    frozen_reference = {
        name: parameter.detach().clone()
        for name, parameter in policy.named_parameters()
    }

    n = len(act_all)
    for epoch in range(args.epochs):
        perm = torch.randperm(n)
        total = 0.0
        for start in range(0, n, 1024):
            mb = perm[start : start + 1024]
            logits, _ = policy(obs[mb], mask[mb])
            losses = []
            for head_index, head in enumerate(ACTION_HEADS):
                indices = torch.as_tensor(head)
                losses.append(
                    masked_head_cross_entropy(
                        logits.index_select(-1, indices),
                        local_targets[head_index][mb],
                        class_weights[head_index],
                        head_index,
                    )
                )
            loss = torch.stack(losses).mean()
            opt.zero_grad()
            loss.backward()
            opt.step()
            total += float(loss.detach()) * len(mb)
        print(f"epoch {epoch}: loss {total / n:.4f}")

    training_dir = pathlib.Path(__file__).resolve().parent
    run_lineage = build_lineage(
        phase="style-distillation",
        phase_start_update=int(blob.get("update", 0) or 0) if blob else 0,
        hyperparameters={
            "episodes_per_strategy": args.episodes,
            "epochs": args.epochs,
            "gym_version": GYM_VERSION,
            "learning_rate": args.lr,
            "scenario_content_sha256": [content_digest(s) for s in scenarios],
            "teacher_profiles": {
                strategy: [f"{style}/{variant}" for style, variant in profiles]
                for strategy, profiles in TEACHER_PROFILES.items()
            },
            "torch_seed": 0,
            "trainable_scope": "profile-columns-only",
        },
        inputs={
            "gym_client": input_identity(training_dir / "oxide_gym.py"),
            "initializer": input_identity(args.initialize_from, blob),
            "model_code": input_identity(training_dir / "models.py"),
            "trainer": input_identity(training_dir / "style_distill.py"),
        },
    )
    first = next(m for m in policy.modules() if isinstance(m, torch.nn.Linear))
    span = first.in_features - PROFILE_CONDITION_COUNT
    for name, parameter in policy.named_parameters():
        reference = frozen_reference[name]
        if parameter is columns:
            moved = (parameter.detach()[:, :span] - reference[:, :span]).abs().max()
            if float(moved) != 0.0:
                raise SystemExit(f"non-profile columns moved by {moved}")
        elif not torch.equal(parameter.detach(), reference):
            raise SystemExit(f"frozen parameter {name} moved")
    print("freeze verified: only profile columns moved")

    save_policy(
        policy,
        blob.get("arch", "deep"),
        args.out,
        {
            "update": int(blob.get("update", 0) or 0),
            "lineage": run_lineage,
        },
    )
    print(f"saved {args.out}")


if __name__ == "__main__":
    main()
