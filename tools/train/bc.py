"""Action-balanced behavior cloning for the factorized policy.

The old two-teacher corpus was 89% Idle and omitted most of the roster
and every new v6 verb. The teachers demonstrate four coherent match
strategies through independent production, capital/maintenance,
upgrade, and operations heads (the scripted teachers hold the upgrade
head at its no-op). Per-head class weighting keeps rare strategic
labels from being buried under no-ops. Every uncontrolled seat is the
Rust-side Overseer.

Usage (from tools/train/):
    uv run bc.py --episodes 40 --out runs/bc.pt
"""

import argparse
import json
import pathlib
from typing import TYPE_CHECKING

import numpy as np
import torch
from torch import nn

if TYPE_CHECKING:
    from collections.abc import Sequence

from lineage import (
    build_lineage,
    checkpoint_metadata,
    content_digest,
    input_identity,
)
from models import load_policy, make_policy, save_policy
from oxide_gym import (
    ACTION_HEADS,
    FEATURE_NAMES,
    GYM_VERSION,
    ActionPlan,
    Worker,
    condition_from_profile,
    validate_action_plan,
    with_condition,
)

# Global action indices (see sim/src/bot/gym.rs).
IDLE, TRAIN_H, TRAIN_S = 0, 1, 2
TRAIN_SCUTTLER, TRAIN_LANCER, TRAIN_BOMBARD = 3, 4, 5
TRAIN_AA, TRAIN_WING, TRAIN_AIR_AA = 6, 7, 8
BUILD_FAB = 9
BUILD_TURRET, BUILD_FLAK, BUILD_BASTION, BUILD_ARRAY = 10, 11, 12, 13
BUILD_RECLAIMER, REPAIR, AIR_RAID = 14, 15, 16
FORM, PUSH, RECALL, SCOUT = 17, 18, 19, 20
SALVAGE, REPAIR_UNIT, BUILD_BAY = 21, 22, 23
NO_CONSTRUCTION, NO_OPERATION, NO_UPGRADE = 24, 25, 42

STRATEGIES = ("fortify", "industry", "combined", "pressure")
AGGRESSION_RANGES = {
    "fortify": (0, 250),
    "industry": (250, 500),
    "combined": (500, 750),
    "pressure": (750, 1001),
}
FACTION_PAIRS = ("ff", "fc", "cf", "cc")

# Feature indices by name (the Worker asserts the same list against
# the gym hello, so these lookups cannot skew).
F = {name: i for i, name in enumerate(FEATURE_NAMES)}


def _first_legal(mask: np.ndarray, actions: tuple[int, ...], fallback: int) -> int:
    return next((action for action in actions if mask[action]), fallback)


def production_teacher(
    strategy: str,
    raw: list[int],
    mask: np.ndarray,
) -> int:
    harvesters = raw[F["my_harvesters"]]
    target_harvesters = {
        "fortify": 5,
        "industry": 6,
        "combined": 5,
        "pressure": 4,
    }[strategy]
    if harvesters < target_harvesters and mask[TRAIN_H]:
        return TRAIN_H
    target_sentinels = {
        "fortify": 2,
        "industry": 3,
        "combined": 3,
        "pressure": 2,
    }[strategy]
    if raw[F["my_sentinels"]] < target_sentinels and mask[TRAIN_S]:
        return TRAIN_S

    enemy_air = raw[F["enemy_airground"]] + raw[F["enemy_airair"]]
    if enemy_air > raw[F["my_antiair"]] and mask[TRAIN_AA]:
        return TRAIN_AA

    counts = {
        TRAIN_SCUTTLER: raw[F["my_scuttlers"]],
        TRAIN_LANCER: raw[F["my_lancers"]],
        TRAIN_BOMBARD: raw[F["my_bombards"]],
        TRAIN_AA: raw[F["my_antiair"]],
        TRAIN_WING: raw[F["my_airground"]],
        TRAIN_AIR_AA: raw[F["my_airair"]],
    }
    targets = {
        "fortify": (
            (TRAIN_BOMBARD, 2),
            (TRAIN_AA, 2),
            (TRAIN_LANCER, 3),
            (TRAIN_AIR_AA, 1),
        ),
        "industry": (
            (TRAIN_LANCER, 3),
            (TRAIN_BOMBARD, 1),
            (TRAIN_WING, 2),
            (TRAIN_AA, 1),
            (TRAIN_AIR_AA, 1),
        ),
        "combined": (
            (TRAIN_LANCER, 3),
            (TRAIN_BOMBARD, 2),
            (TRAIN_AA, 2),
            (TRAIN_WING, 3),
            (TRAIN_AIR_AA, 1),
            (TRAIN_SCUTTLER, 2),
        ),
        "pressure": (
            (TRAIN_SCUTTLER, 4),
            (TRAIN_WING, 3),
            (TRAIN_LANCER, 2),
            (TRAIN_AIR_AA, 1),
            (TRAIN_BOMBARD, 1),
        ),
    }[strategy]
    for action, target in targets:
        if counts[action] < target and mask[action]:
            return action

    if strategy == "pressure":
        return _first_legal(
            mask,
            (TRAIN_SCUTTLER, TRAIN_WING, TRAIN_LANCER, TRAIN_S),
            IDLE,
        )
    if strategy == "fortify":
        return _first_legal(mask, (TRAIN_BOMBARD, TRAIN_AA, TRAIN_S), IDLE)
    return _first_legal(
        mask,
        (TRAIN_LANCER, TRAIN_BOMBARD, TRAIN_WING, TRAIN_S),
        IDLE,
    )


def construction_teacher(
    strategy: str,
    raw: list[int],
    mask: np.ndarray,
) -> int:
    if raw[F["repair_deficit"]] > 150 and mask[REPAIR]:
        return REPAIR
    if raw[F["damaged_unit_value"]] > 50 and mask[REPAIR_UNIT]:
        return REPAIR_UNIT

    enemy_air = raw[F["enemy_airground"]] + raw[F["enemy_airair"]]
    if (
        enemy_air > raw[F["my_antiair"]]
        and raw[F["my_flak_built"]] < 1
        and mask[BUILD_FLAK]
    ):
        return BUILD_FLAK
    ground_threat = raw[F["home_enemy_pressure"]] > 0
    if (
        ground_threat
        and raw[F["my_turrets_built"]] < 1
        and raw[F["my_strength"]] >= 50
        and mask[BUILD_TURRET]
    ):
        return BUILD_TURRET

    # A saved goal is already consuming this head. NoConstruction means
    # "keep it", not "forget it". Let a paid site stand before
    # commissioning another too. Maintenance and emergency defense stay
    # above this guard so a long project cannot suppress them.
    if raw[F["construction_plan"]] != 0:
        return NO_CONSTRUCTION
    if raw[F["my_construction_sites"]] != 0:
        return NO_CONSTRUCTION

    if raw[F["fab_built"]] == 0 and mask[BUILD_FAB]:
        return BUILD_FAB

    # The capital head has first claim on the shared bank. Chaining an
    # Array or Bastion immediately after the Fabricator therefore blocks
    # every unit order while the opponent builds an army. Demonstrate a
    # stable economy and a small screen before optional infrastructure.
    screen_strength = {
        "fortify": 100,
        "industry": 90,
        "combined": 100,
        "pressure": 80,
    }[strategy]
    if raw[F["my_harvesters"]] < 4 or raw[F["my_strength"]] < screen_strength:
        return NO_CONSTRUCTION

    threatened = ground_threat or raw[F["blip_count"]] > 0

    if strategy == "fortify":
        candidates = (
            (BUILD_TURRET, raw[F["my_turrets_built"]] < 2),
            (BUILD_ARRAY, raw[F["my_arrays_built"]] < 1),
            (BUILD_FLAK, raw[F["my_flak_built"]] < 1),
            (BUILD_BASTION, raw[F["my_bastions_built"]] < 1),
            (BUILD_BAY, raw[F["my_repair_bays_built"]] < 1),
            (
                BUILD_RECLAIMER,
                raw[F["near_home_salvage_value"]] < 500
                and raw[F["my_reclaimers_built"]] < 2,
            ),
        )
    elif strategy == "industry":
        candidates = (
            (BUILD_TURRET, raw[F["my_turrets_built"]] < 1),
            (
                BUILD_RECLAIMER,
                (raw[F["near_home_salvage_value"]] < 900 or raw[F["tick"]] >= 2_500)
                and raw[F["my_reclaimers_built"]] < 1,
            ),
            (
                BUILD_ARRAY,
                raw[F["my_arrays_built"]] < 1 and raw[F["tick"]] >= 4_000,
            ),
        )
    elif strategy == "combined":
        candidates = (
            (BUILD_TURRET, raw[F["my_turrets_built"]] < 2),
            (
                BUILD_ARRAY,
                raw[F["my_arrays_built"]] < 1 and raw[F["tick"]] >= 2_500,
            ),
            (
                BUILD_BASTION,
                raw[F["my_bastions_built"]] < 1 and raw[F["tick"]] >= 5_000,
            ),
        )
    else:
        candidates = (
            (
                BUILD_ARRAY,
                raw[F["my_arrays_built"]] < 1 and raw[F["tick"]] >= 2_000,
            ),
            (BUILD_TURRET, threatened and raw[F["my_turrets_built"]] < 1),
        )
    for action, wanted in candidates:
        if wanted and mask[action]:
            return action

    if raw[F["known_salvage_value"]] == 0 and raw[F["scrap"]] < 50 and mask[SALVAGE]:
        return SALVAGE
    return NO_CONSTRUCTION


def operations_teacher(
    strategy: str,
    raw: list[int],
    mask: np.ndarray,
    tick: int,
) -> int:
    staging_size = raw[F["staging_army_size"]]
    push_size = {
        "fortify": 10,
        "industry": 10,
        "combined": 8,
        "pressure": 8,
    }[strategy]
    if mask[RECALL] and (
        raw[F["incoming_shells"]] > 1 or raw[F["damaged_unit_value"]] > 250
    ):
        return RECALL
    if mask[AIR_RAID] and raw[F["my_airground"]] >= 3:
        return AIR_RAID
    if mask[PUSH] and staging_size >= push_size:
        return PUSH
    if mask[FORM]:
        return FORM
    scout_period = 2048 if strategy in ("fortify", "industry") else 1024
    if mask[SCOUT] and tick % scout_period < 16:
        return SCOUT
    return NO_OPERATION


def teacher(
    strategy: str,
    raw: list[int],
    mask: np.ndarray,
    tick: int,
) -> ActionPlan:
    proposed = validate_action_plan(
        (
            production_teacher(strategy, raw, mask),
            construction_teacher(strategy, raw, mask),
            NO_UPGRADE,
            operations_teacher(strategy, raw, mask, tick),
        )
    )
    legal = []
    for head_index, (action, head) in enumerate(
        zip(proposed, ACTION_HEADS, strict=True)
    ):
        if mask[action]:
            legal.append(action)
            continue
        fallback = next((candidate for candidate in head if mask[candidate]), None)
        if fallback is None:
            raise ValueError(f"action head {head_index} has no legal teacher target")
        legal.append(fallback)
    return validate_action_plan(legal)


def local_action_targets(plans: Sequence[ActionPlan]) -> list[np.ndarray]:
    """Converts global plan indices to local class indices per head."""
    validated = [validate_action_plan(plan) for plan in plans]
    targets = []
    for head_index, head in enumerate(ACTION_HEADS):
        local = np.asarray(
            [head.index(plan[head_index]) for plan in validated],
            dtype=np.int64,
        )
        targets.append(local)
    return targets


def masked_head_cross_entropy(
    logits: torch.Tensor,
    targets: torch.Tensor,
    class_weights: torch.Tensor,
    head_index: int,
) -> torch.Tensor:
    """Returns weighted cloning loss while retaining the action mask."""
    selected = logits.gather(-1, targets.unsqueeze(-1)).squeeze(-1)
    if not bool(torch.isfinite(selected).all().item()):
        bad_targets = torch.unique(targets[~torch.isfinite(selected)])
        raise ValueError(
            f"behavior-cloning targets are masked or non-finite in head {head_index}: "
            f"local classes {bad_targets.detach().cpu().tolist()}"
        )
    if bool((torch.isnan(logits) | torch.isposinf(logits)).any().item()):
        raise ValueError(
            f"behavior-cloning logits contain NaN or +inf in head {head_index}"
        )
    return nn.functional.cross_entropy(logits, targets, weight=class_weights)


def duel_scenarios(directory: pathlib.Path) -> list[pathlib.Path]:
    """Returns the shipped two-seat maps in stable filename order."""
    scenarios = []
    for path in sorted(directory.glob("*.json")):
        payload = json.loads(path.read_text())
        if len(payload.get("players", [])) == 2:
            scenarios.append(path)
    if not scenarios:
        raise ValueError(f"no two-seat scenarios found under {directory}")
    return scenarios


def episode_assignment(
    episode: int,
    scenario_count: int,
) -> tuple[str, int, int, str]:
    """Returns strategy, seat, map index, and faction pair.

    One 128-episode pass crosses every strategy/seat pair with every
    shipped duel map. Faction pairs vary within each map instead of
    aliasing map index; subsequent 128-episode passes rotate the pair
    for the same strategy/seat/map cell.
    """
    if scenario_count <= 0:
        raise ValueError("scenario count must be positive")
    local = episode % (2 * len(STRATEGIES))
    block = episode // (2 * len(STRATEGIES))
    strategy = STRATEGIES[local // 2]
    seat = local % 2
    scenario_index = (block * 5 + local) % scenario_count
    faction_rotation = block // scenario_count
    faction_index = (block + 2 * local + faction_rotation) % len(FACTION_PAIRS)
    return strategy, seat, scenario_index, FACTION_PAIRS[faction_index]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--driver", default="../../target/release/oxide-driver")
    ap.add_argument("--episodes", type=int, default=64)
    ap.add_argument(
        "--scenario-dir",
        default="../../scenarios",
        help="directory whose shipped two-seat maps form the demonstration slate",
    )
    ap.add_argument("--epochs", type=int, default=20)
    ap.add_argument("--lr", type=float, default=1e-3)
    ap.add_argument(
        "--save-every",
        type=int,
        default=0,
        help="also save an epoch-numbered checkpoint every N epochs",
    )
    ap.add_argument("--arch", default="mlp")
    ap.add_argument(
        "--resume",
        default=None,
        help="initialize from an existing checkpoint instead of fresh weights",
    )
    ap.add_argument("--out", default="runs/bc.pt")
    args = ap.parse_args()

    torch.manual_seed(0)
    scenarios = duel_scenarios(pathlib.Path(args.scenario_dir))
    driver_identity = input_identity(args.driver)
    worker = Worker(args.driver)
    obs_all, mask_all, act_all = [], [], []
    wins = 0
    try:
        strategy_wins = dict.fromkeys(STRATEGIES, 0)
        scenario_wins = dict.fromkeys((path.stem for path in scenarios), 0)
        for ep in range(args.episodes):
            strategy, seat, scenario_index, factions = episode_assignment(
                ep,
                len(scenarios),
            )
            scenario = scenarios[scenario_index]
            frame = worker.reset(
                20_000 + ep,
                control=(seat,),
                scenario=str(scenario),
                factions=factions,
            )
            rng = np.random.default_rng(ep)
            while not frame.done:
                view = frame.seats[seat]
                plan = teacher(strategy, view.raw, view.mask, frame.tick)
                # Two profile variations per world state teach the whole
                # skill range while keeping the discrete strategy label
                # causally aligned with its demonstration.
                for _ in range(2):
                    agg_lo, agg_hi = AGGRESSION_RANGES[strategy]
                    faction = 1000 if view.raw[F["faction"]] else 0
                    cond = condition_from_profile(
                        int(rng.integers(300, 1001)),
                        int(rng.integers(agg_lo, agg_hi)),
                        faction,
                    )
                    obs_all.append(with_condition(view.obs, cond))
                    mask_all.append(view.mask)
                    act_all.append(plan)
                frame = worker.step({seat: plan})
            if frame.reward(seat) > 0:
                wins += 1
                strategy_wins[strategy] += 1
                scenario_wins[scenario.stem] += 1
        print(f"per-strategy wins: {strategy_wins}")
        print(f"per-scenario wins: {scenario_wins}")
        print(f"teacher: {wins}/{args.episodes} wins")
    finally:
        worker.close()

    obs = torch.as_tensor(np.stack(obs_all))
    mask = torch.as_tensor(np.stack(mask_all))
    # Recording telemetry and per-head class weights. A dataset that
    # never contains an action cannot teach it; a dataset where it is
    # outnumbered 10,000:1 barely can.
    local_targets = []
    class_weights = []
    target_arrays = local_action_targets(act_all)
    for head_index, (head, local) in enumerate(
        zip(ACTION_HEADS, target_arrays, strict=True)
    ):
        counts = np.bincount(local, minlength=len(head))
        print(
            f"head {head_index} action counts:",
            {
                action: int(counts[local_index])
                for local_index, action in enumerate(head)
                if counts[local_index]
            },
        )
        # Inverse-square-root is strong enough to surface rare labels
        # without making one demonstration outweigh a whole strategy.
        weights = np.sqrt(max(int(counts.max()), 1) / np.maximum(counts, 1))
        weights = np.minimum(weights, 10.0)
        local_targets.append(torch.as_tensor(local))
        class_weights.append(torch.as_tensor(weights, dtype=torch.float32))

    if args.resume:
        policy, blob = load_policy(args.resume)
        arch = blob.get("arch", args.arch)
    else:
        arch = args.arch
        policy = make_policy(arch)
        blob = None
    training_dir = pathlib.Path(__file__).resolve().parent
    lineage_inputs = {
        "gym_client": input_identity(training_dir / "oxide_gym.py"),
        "gym_driver": driver_identity,
        "model_code": input_identity(training_dir / "models.py"),
        "trainer": input_identity(training_dir / "bc.py"),
    }
    if args.resume:
        lineage_inputs["initializer"] = input_identity(args.resume, blob)
    run_lineage = build_lineage(
        phase="behavior-cloning",
        phase_start_update=int(blob.get("update", 0) or 0) if blob is not None else 0,
        hyperparameters={
            "arch": arch,
            "batch_size": 1024,
            "class_weighting": "inverse-square-root-cap-10",
            "episodes": args.episodes,
            "episode_seed_base": 20_000,
            "epochs": args.epochs,
            "gym_version": GYM_VERSION,
            "learning_rate": args.lr,
            "scenario_content_sha256": [
                content_digest(scenario) for scenario in scenarios
            ],
            "strategy_aggression_ranges": {
                strategy: list(bounds) for strategy, bounds in AGGRESSION_RANGES.items()
            },
            "torch_seed": 0,
        },
        inputs=lineage_inputs,
    )
    opt = torch.optim.Adam(policy.parameters(), lr=args.lr)
    out_path = pathlib.Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
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
        completed_epoch = epoch + 1
        if args.save_every > 0 and completed_epoch % args.save_every == 0:
            checkpoint = out_path.with_name(
                f"{out_path.stem}-epoch{completed_epoch:02d}{out_path.suffix}"
            )
            save_policy(
                policy,
                arch,
                checkpoint,
                checkpoint_metadata(
                    run_lineage,
                    {
                        "critic_ready": False,
                        "gym_version": GYM_VERSION,
                        "bc_epoch": completed_epoch,
                    },
                ),
            )
    save_policy(
        policy,
        arch,
        out_path,
        checkpoint_metadata(
            run_lineage,
            {
                "critic_ready": False,
                "gym_version": GYM_VERSION,
                "bc_epoch": args.epochs,
            },
        ),
    )
    print(f"saved {args.out} ({n} samples)")


if __name__ == "__main__":
    main()
