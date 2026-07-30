"""Selectively revive production actions without moving the rest of a policy.

The trainer starts from an exact Q12 artifact or a float checkpoint, collects
factorized behavior-cloning demonstrations, and updates only explicitly named
production-head rows. Non-target teacher states distill the parent logits so a
rare action learns context instead of receiving a global usage boost.

Usage (from tools/train/):
    uv run revive.py \
      --initialize-from runs/parent.json \
      --actions 3,8 \
      --promote-actions 3 \
      --out runs/revived.pt
    uv run export.py --ckpt runs/revived.pt --out runs/revived.json

`--actions` names the rows optimized together. `--promote-actions` can retain
only a gated subset of those jointly trained rows in the saved checkpoint.
"""

import argparse
import copy
import json
import math
import pathlib
from dataclasses import asdict, dataclass
from typing import TYPE_CHECKING

import numpy as np
import torch
from torch import nn

import bc
from dequantize import recover_actor
from lineage import (
    build_lineage,
    checkpoint_metadata,
    content_digest,
    input_identity,
)
from models import Mlp, checkpoint_critic_ready, load_policy, save_policy
from oxide_gym import (
    ACTION_HEADS,
    GYM_VERSION,
    NET_FEATURES,
    Worker,
    condition_from_profile,
    policy_skill_for_aggression,
    with_condition,
)

if TYPE_CHECKING:
    from collections.abc import Sequence

PRODUCTION_HEAD = ACTION_HEADS[0]
STRATEGY_AGGRESSION = {
    "fortify": 125,
    "industry": 375,
    "combined": 550,
    "pressure": 875,
}


def teacher_condition(strategy: str, faction: int) -> tuple[int, ...]:
    """Builds the exact shipped policy condition for one teacher strategy."""
    aggression = STRATEGY_AGGRESSION[strategy]
    return condition_from_profile(
        policy_skill_for_aggression(aggression),
        aggression,
        faction,
    )


def _positive_int(text: str) -> int:
    value = int(text)
    if value <= 0:
        raise argparse.ArgumentTypeError(f"expected a positive integer, got {text!r}")
    return value


def unit_interval(text: str) -> float:
    value = float(text)
    if not math.isfinite(value) or not 0.0 <= value <= 1.0:
        raise argparse.ArgumentTypeError(f"expected a value in 0..1, got {text!r}")
    return value


def nonnegative_float(text: str) -> float:
    value = float(text)
    if not math.isfinite(value) or value < 0.0:
        raise argparse.ArgumentTypeError(f"expected a nonnegative value, got {text!r}")
    return value


def parse_action_indices(text: str) -> tuple[int, ...]:
    """Parses unique global action ids belonging to the production head."""
    try:
        actions = tuple(int(part.strip()) for part in text.split(",") if part.strip())
    except ValueError as err:
        raise argparse.ArgumentTypeError(
            f"actions must be comma-separated integers, got {text!r}"
        ) from err
    if not actions:
        raise argparse.ArgumentTypeError("at least one production action is required")
    if len(set(actions)) != len(actions):
        raise argparse.ArgumentTypeError("production actions must not repeat")
    invalid = [action for action in actions if action not in PRODUCTION_HEAD]
    if invalid:
        raise argparse.ArgumentTypeError(
            f"actions {invalid} are not in the production head {PRODUCTION_HEAD}"
        )
    return actions


def validate_promoted_actions(
    trained_actions: Sequence[int],
    promoted_actions: Sequence[int],
) -> None:
    """Requires promotion to be a nonempty subset of the trained rows."""
    trained = set(trained_actions)
    promoted = set(promoted_actions)
    if not promoted:
        raise ValueError("at least one trained action must be promoted")
    if not promoted.issubset(trained):
        extra = sorted(promoted - trained)
        raise ValueError(f"promoted actions were not trained: {extra}")


@dataclass(frozen=True)
class RevivalDataset:
    """One deterministic teacher corpus."""

    obs: torch.Tensor
    mask: torch.Tensor
    target: torch.Tensor
    episode: torch.Tensor

    def validate(self) -> None:
        rows = self.obs.shape[0]
        if self.obs.ndim != 2 or self.obs.shape[1] != NET_FEATURES:
            raise ValueError(
                f"observations must have shape (N, {NET_FEATURES}), "
                f"got {tuple(self.obs.shape)}"
            )
        if self.mask.shape != (rows, sum(len(head) for head in ACTION_HEADS)):
            raise ValueError(
                f"mask shape does not match observations: {self.mask.shape}"
            )
        if self.target.shape != (rows,) or self.episode.shape != (rows,):
            raise ValueError("targets and episode ids must have one entry per row")
        if self.mask.dtype != torch.bool:
            raise TypeError("action masks must be boolean")
        if rows == 0:
            raise ValueError("the revival corpus is empty")


@dataclass(frozen=True)
class RevivalConfig:
    """Optimization and held-out audit settings."""

    actions: tuple[int, ...]
    steps: int = 201
    learning_rate: float = 3e-3
    positive_batch: int = 1024
    retention_batch: int = 4096
    retention_coefficient: float = 0.01
    parameter_coefficient: float = 1e-4
    heldout_modulo: int = 5
    sample_seed: int = 91

    def validate(self) -> None:
        parse_action_indices(",".join(str(action) for action in self.actions))
        if self.steps <= 0:
            raise ValueError("steps must be positive")
        if self.learning_rate <= 0.0:
            raise ValueError("learning rate must be positive")
        if self.positive_batch <= 0 or self.retention_batch <= 0:
            raise ValueError("batch sizes must be positive")
        if self.retention_coefficient < 0.0 or self.parameter_coefficient < 0.0:
            raise ValueError("retention coefficients must be nonnegative")
        if self.heldout_modulo < 2:
            raise ValueError("heldout modulo must be at least two")


@dataclass(frozen=True)
class CorpusSplit:
    """Boolean row masks for optimization and audit."""

    train_positive: torch.Tensor
    train_retention: torch.Tensor
    held_positive: torch.Tensor
    held_retention: torch.Tensor


def split_corpus(
    dataset: RevivalDataset,
    actions: Sequence[int],
    heldout_modulo: int,
) -> CorpusSplit:
    """Splits by whole episode so held states cannot leak into training."""
    dataset.validate()
    selected = torch.as_tensor(tuple(actions), dtype=dataset.target.dtype)
    positive = torch.isin(dataset.target, selected)
    held = dataset.episode.remainder(heldout_modulo) == 0
    split = CorpusSplit(
        train_positive=positive & ~held,
        train_retention=~positive & ~held,
        held_positive=positive & held,
        held_retention=~positive & held,
    )
    for name, rows in (
        ("training positives", split.train_positive),
        ("training retention", split.train_retention),
        ("held positives", split.held_positive),
        ("held retention", split.held_retention),
    ):
        if not bool(rows.any().item()):
            raise ValueError(f"corpus has no {name}")
    for action in actions:
        for name, rows in (
            ("training", split.train_positive),
            ("held", split.held_positive),
        ):
            count = int((rows & (dataset.target == action)).sum().item())
            if count == 0:
                raise ValueError(f"action {action} has no {name} positive rows")
    return split


def balanced_selected_loss(
    production_logits: torch.Tensor,
    production_mask: torch.Tensor,
    global_targets: torch.Tensor,
    actions: Sequence[int],
) -> torch.Tensor:
    """Returns class-balanced CE after proving every teacher target is legal."""
    if production_logits.shape != production_mask.shape:
        raise ValueError("production logits and masks must have the same shape")
    if production_logits.ndim != 2 or production_logits.shape[1] != len(
        PRODUCTION_HEAD
    ):
        raise ValueError("production logits have the wrong width")
    if global_targets.shape != (production_logits.shape[0],):
        raise ValueError("targets must have one entry per logit row")

    local_by_global = {action: index for index, action in enumerate(PRODUCTION_HEAD)}
    try:
        local_targets = torch.as_tensor(
            [local_by_global[int(action)] for action in global_targets],
            dtype=torch.long,
            device=global_targets.device,
        )
    except KeyError as err:
        raise ValueError(
            f"teacher target {err.args[0]} is not a production action"
        ) from err
    selected = torch.as_tensor(tuple(actions), device=global_targets.device)
    if not bool(torch.isin(global_targets, selected).all().item()):
        raise ValueError("positive batch contains a teacher target not being revived")

    legal = production_mask.gather(-1, local_targets.unsqueeze(-1)).squeeze(-1)
    if not bool(legal.all().item()):
        bad = torch.unique(global_targets[~legal]).detach().cpu().tolist()
        raise ValueError(f"masked teacher targets would poison revival loss: {bad}")
    masked = production_logits.masked_fill(~production_mask, float("-inf"))
    target_logits = masked.gather(-1, local_targets.unsqueeze(-1)).squeeze(-1)
    if not bool(torch.isfinite(target_logits).all().item()):
        raise ValueError("teacher target logits are non-finite")
    if bool((torch.isnan(masked) | torch.isposinf(masked)).any().item()):
        raise ValueError("production logits contain NaN or +inf")

    per_row = nn.functional.cross_entropy(masked, local_targets, reduction="none")
    terms = []
    for action in actions:
        rows = global_targets == action
        if not bool(rows.any().item()):
            raise ValueError(f"positive batch contains no rows for action {action}")
        terms.append(per_row[rows].mean())
    return torch.stack(terms).mean()


def ensure_selected_actions(
    batch: torch.Tensor,
    targets: torch.Tensor,
    selected_indices: dict[int, torch.Tensor],
    generator: torch.Generator,
) -> torch.Tensor:
    """Deterministically repairs a pooled batch that omitted a rare class.

    Ordinary balanced corpora keep the original sample byte-for-byte. Only
    an actually absent selected action replaces one leading slot, avoiding
    the empty mean that would otherwise poison the optimization.
    """
    repaired = batch
    for offset, (action, indices) in enumerate(selected_indices.items()):
        if bool((targets[repaired] == action).any().item()):
            continue
        if indices.numel() == 0:
            raise ValueError(f"training split contains no rows for action {action}")
        if repaired.data_ptr() == batch.data_ptr():
            repaired = batch.clone()
        chosen = torch.randint(len(indices), (1,), generator=generator)
        repaired[offset % len(repaired)] = indices[chosen]
    return repaired


def _head_with_selected_rows(
    parent_head: torch.Tensor,
    hidden: torch.Tensor,
    row_weight: torch.Tensor,
    row_bias: torch.Tensor,
    selected_local: torch.Tensor,
) -> torch.Tensor:
    logits = parent_head.clone()
    logits[:, selected_local] = nn.functional.linear(hidden, row_weight, row_bias)
    return logits


def retention_audit(
    parent_head: torch.Tensor,
    candidate_head: torch.Tensor,
    production_mask: torch.Tensor,
    global_targets: torch.Tensor,
    split: CorpusSplit,
    actions: Sequence[int],
) -> dict:
    """Measures held target reach and policy drift on non-target states."""
    local_by_global = {action: index for index, action in enumerate(PRODUCTION_HEAD)}
    selected_local = torch.as_tensor([local_by_global[action] for action in actions])
    masked_parent = parent_head.masked_fill(~production_mask, float("-inf"))
    masked_candidate = candidate_head.masked_fill(~production_mask, float("-inf"))

    held_positive = split.held_positive
    candidate_choice = masked_candidate[held_positive].argmax(dim=-1)
    positive_targets = global_targets[held_positive]
    target_rates = {}
    target_counts = {}
    for action, local in zip(actions, selected_local, strict=True):
        rows = positive_targets == action
        target_counts[str(action)] = int(rows.sum().item())
        target_rates[str(action)] = float(
            (candidate_choice[rows] == local).to(torch.float32).mean().item()
        )

    held_retention = split.held_retention
    parent_retention = masked_parent[held_retention]
    candidate_retention = masked_candidate[held_retention]
    old_choice = parent_retention.argmax(dim=-1)
    new_choice = candidate_retention.argmax(dim=-1)
    selected_old = torch.isin(old_choice, selected_local)
    selected_new = torch.isin(new_choice, selected_local)
    new_selected = selected_new & ~selected_old

    parent_log_prob = torch.log_softmax(parent_retention, dim=-1)
    candidate_log_prob = torch.log_softmax(candidate_retention, dim=-1)
    parent_prob = torch.softmax(parent_retention, dim=-1)
    kl_terms = torch.where(
        parent_prob > 0,
        parent_prob * (parent_log_prob - candidate_log_prob),
        torch.zeros_like(parent_prob),
    )
    kl = kl_terms.sum(dim=-1)
    raw_delta = candidate_head[held_retention].index_select(
        -1, selected_local
    ) - parent_head[held_retention].index_select(-1, selected_local)
    retention_rows = int(held_retention.sum().item())
    new_count = int(new_selected.sum().item())
    return {
        "held_target_counts": target_counts,
        "held_target_greedy_rates": target_rates,
        "held_retention_rows": retention_rows,
        "held_retention_new_selected_greedy": new_count,
        "held_retention_new_selected_rate": new_count / retention_rows,
        "held_retention_mean_kl": float(kl.mean().item()),
        "held_retention_max_kl": float(kl.max().item()),
        "held_retention_mean_abs_logit_delta": float(raw_delta.abs().mean().item()),
        "held_retention_max_abs_logit_delta": float(raw_delta.abs().max().item()),
    }


def train_selected_rows(
    policy: Mlp,
    dataset: RevivalDataset,
    config: RevivalConfig,
) -> dict:
    """Mutates only selected policy-head rows and returns its held-out audit."""
    dataset.validate()
    config.validate()
    split = split_corpus(dataset, config.actions, config.heldout_modulo)
    policy.eval()
    for parameter in policy.parameters():
        parameter.requires_grad_(False)

    production = torch.as_tensor(PRODUCTION_HEAD)
    selected_global = torch.as_tensor(config.actions)
    selected_local = torch.as_tensor(
        [PRODUCTION_HEAD.index(action) for action in config.actions]
    )
    with torch.no_grad():
        hidden = policy.trunk(dataset.obs)
        parent_logits = policy.pi(hidden)
        parent_head = parent_logits.index_select(-1, production)
        parent_weight = policy.pi.weight.index_select(0, selected_global).clone()
        parent_bias = policy.pi.bias.index_select(0, selected_global).clone()

    row_weight = nn.Parameter(parent_weight.clone())
    row_bias = nn.Parameter(parent_bias.clone())
    optimizer = torch.optim.Adam(
        [row_weight, row_bias],
        lr=config.learning_rate,
    )
    positive_indices = torch.where(split.train_positive)[0]
    retention_indices = torch.where(split.train_retention)[0]
    selected_indices = {
        action: torch.where(split.train_positive & (dataset.target == action))[0]
        for action in config.actions
    }
    generator = torch.Generator().manual_seed(config.sample_seed)
    final_loss = torch.zeros(())
    final_positive_loss = torch.zeros(())
    final_retention_loss = torch.zeros(())
    for _step in range(config.steps):
        positive_batch = positive_indices[
            torch.randint(
                len(positive_indices),
                (config.positive_batch,),
                generator=generator,
            )
        ]
        positive_batch = ensure_selected_actions(
            positive_batch,
            dataset.target,
            selected_indices,
            generator,
        )
        retention_batch = retention_indices[
            torch.randint(
                len(retention_indices),
                (config.retention_batch,),
                generator=generator,
            )
        ]
        positive_head = _head_with_selected_rows(
            parent_head[positive_batch],
            hidden[positive_batch],
            row_weight,
            row_bias,
            selected_local,
        )
        positive_loss = balanced_selected_loss(
            positive_head,
            dataset.mask[positive_batch].index_select(-1, production),
            dataset.target[positive_batch],
            config.actions,
        )
        new_retention = nn.functional.linear(
            hidden[retention_batch],
            row_weight,
            row_bias,
        )
        old_retention = parent_logits[retention_batch].index_select(-1, selected_global)
        retention_loss = nn.functional.mse_loss(new_retention, old_retention)
        parameter_loss = (row_weight - parent_weight).square().mean()
        parameter_loss = parameter_loss + (row_bias - parent_bias).square().mean()
        loss = (
            positive_loss
            + config.retention_coefficient * retention_loss
            + config.parameter_coefficient * parameter_loss
        )
        if not bool(torch.isfinite(loss).item()):
            raise ValueError("revival loss became non-finite")
        optimizer.zero_grad()
        loss.backward()
        optimizer.step()
        final_loss = loss
        final_positive_loss = positive_loss
        final_retention_loss = retention_loss

    with torch.no_grad():
        policy.pi.weight.index_copy_(0, selected_global, row_weight)
        policy.pi.bias.index_copy_(0, selected_global, row_bias)
        candidate_head = policy.pi(hidden).index_select(-1, production)
    audit = retention_audit(
        parent_head,
        candidate_head,
        dataset.mask.index_select(-1, production),
        dataset.target,
        split,
        config.actions,
    )
    audit.update(
        {
            "rows": int(dataset.obs.shape[0]),
            "train_positive_rows": int(split.train_positive.sum().item()),
            "train_retention_rows": int(split.train_retention.sum().item()),
            "final_loss": float(final_loss.detach().item()),
            "final_positive_loss": float(final_positive_loss.detach().item()),
            "final_retention_loss": float(final_retention_loss.detach().item()),
        }
    )
    return audit


def audit_selected_policy(
    parent: Mlp,
    candidate: Mlp,
    dataset: RevivalDataset,
    actions: Sequence[int],
    heldout_modulo: int,
) -> dict:
    """Audits the actual candidate rows against the untouched parent."""
    split = split_corpus(dataset, actions, heldout_modulo)
    production = torch.as_tensor(PRODUCTION_HEAD)
    parent.eval()
    candidate.eval()
    with torch.no_grad():
        parent_head = parent.pi(parent.trunk(dataset.obs)).index_select(
            -1,
            production,
        )
        candidate_head = candidate.pi(candidate.trunk(dataset.obs)).index_select(
            -1,
            production,
        )
    audit = retention_audit(
        parent_head,
        candidate_head,
        dataset.mask.index_select(-1, production),
        dataset.target,
        split,
        actions,
    )
    audit["rows"] = int(dataset.obs.shape[0])
    audit["held_positive_rows"] = int(split.held_positive.sum().item())
    return audit


def restore_unpromoted_rows(
    policy: Mlp,
    parent_weight: torch.Tensor,
    parent_bias: torch.Tensor,
    trained_actions: Sequence[int],
    promoted_actions: Sequence[int],
) -> None:
    """Restores jointly trained rows that failed the downstream promotion gate."""
    validate_promoted_actions(trained_actions, promoted_actions)
    if parent_weight.shape != (len(trained_actions), policy.pi.weight.shape[1]):
        raise ValueError("parent weight snapshot does not match trained actions")
    if parent_bias.shape != (len(trained_actions),):
        raise ValueError("parent bias snapshot does not match trained actions")
    promoted = set(promoted_actions)
    restore_offsets = [
        offset
        for offset, action in enumerate(trained_actions)
        if action not in promoted
    ]
    if not restore_offsets:
        return
    restore_actions = torch.as_tensor(
        [trained_actions[offset] for offset in restore_offsets],
        dtype=torch.long,
    )
    offsets = torch.as_tensor(restore_offsets, dtype=torch.long)
    with torch.no_grad():
        policy.pi.weight.index_copy_(
            0,
            restore_actions,
            parent_weight.index_select(0, offsets),
        )
        policy.pi.bias.index_copy_(
            0,
            restore_actions,
            parent_bias.index_select(0, offsets),
        )


def enforce_audit(
    audit: dict,
    *,
    min_target_greedy: float,
    max_new_selected_rate: float,
    max_mean_kl: float,
) -> None:
    """Refuses an output that missed its targets or moved too broadly."""
    failures = []
    for action, rate in audit["held_target_greedy_rates"].items():
        if not math.isfinite(rate) or rate < min_target_greedy:
            failures.append(
                f"action {action} held target greedy rate {rate:.3f} "
                f"< {min_target_greedy:.3f}"
            )
    new_rate = audit["held_retention_new_selected_rate"]
    if not math.isfinite(new_rate) or new_rate > max_new_selected_rate:
        failures.append(
            f"held retention new-selected rate {new_rate:.4f} "
            f"> {max_new_selected_rate:.4f}"
        )
    mean_kl = audit["held_retention_mean_kl"]
    if not math.isfinite(mean_kl) or mean_kl > max_mean_kl:
        failures.append(f"held retention mean KL {mean_kl:.5f} > {max_mean_kl:.5f}")
    if failures:
        raise ValueError("revival audit failed: " + "; ".join(failures))


def collect_teacher_corpus(
    *,
    driver: str,
    scenario_dir: pathlib.Path,
    episodes: int,
    tiers: tuple[str, ...],
    episode_seed_base: int,
    max_ticks: int,
    cadence: int,
) -> RevivalDataset:
    """Collects deterministic corrected-teacher states across duel maps."""
    scenarios = bc.duel_scenarios(scenario_dir)
    obs_rows = []
    mask_rows = []
    targets = []
    episode_rows = []
    worker = Worker(driver)
    try:
        for episode in range(episodes):
            strategy, seat, scenario_index, factions, tier_index = (
                bc.episode_assignment(episode, len(scenarios), len(tiers))
            )
            frame = worker.reset(
                episode_seed_base + episode,
                control=(seat,),
                tier=tiers[tier_index],
                max_ticks=max_ticks,
                cadence=cadence,
                scenario=str(scenarios[scenario_index]),
                factions=factions,
            )
            while not frame.done:
                view = frame.seats[seat]
                plan = bc.teacher(strategy, view.raw, view.mask, frame.tick)
                if not all(view.mask[action] for action in plan):
                    raise ValueError(
                        f"teacher emitted a masked action at tick {frame.tick}: {plan}"
                    )
                condition = teacher_condition(strategy, view.faction_knob)
                obs_rows.append(with_condition(view.obs, condition))
                mask_rows.append(view.mask.copy())
                targets.append(plan[0])
                episode_rows.append(episode)
                frame = worker.step({seat: plan})
    finally:
        worker.close()
    dataset = RevivalDataset(
        obs=torch.as_tensor(np.stack(obs_rows), dtype=torch.float32),
        mask=torch.as_tensor(np.stack(mask_rows), dtype=torch.bool),
        target=torch.as_tensor(targets, dtype=torch.long),
        episode=torch.as_tensor(episode_rows, dtype=torch.long),
    )
    dataset.validate()
    return dataset


def load_initializer(path: pathlib.Path) -> tuple[Mlp, dict, str]:
    """Loads an exact Q12 artifact or an ordinary float checkpoint."""
    if path.suffix.lower() == ".json":
        artifact = json.loads(path.read_text())
        policy, blob = recover_actor(artifact)
        return policy, blob, "exact-q12-artifact"
    policy, blob = load_policy(str(path))
    return policy, blob, "float-checkpoint"


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--initialize-from", required=True)
    ap.add_argument("--actions", required=True, type=parse_action_indices)
    ap.add_argument(
        "--promote-actions",
        type=parse_action_indices,
        default=None,
        help="subset of jointly trained rows to retain (default: all --actions)",
    )
    ap.add_argument("--out", required=True)
    ap.add_argument("--audit-out", default=None)
    ap.add_argument("--driver", default="../../target/release/oxide-driver")
    ap.add_argument("--scenario-dir", default="../../scenarios")
    ap.add_argument("--episodes", type=_positive_int, default=64)
    ap.add_argument("--tiers", default="scrapheap")
    ap.add_argument("--episode-seed-base", type=int, default=42_000)
    ap.add_argument("--max-ticks", type=_positive_int, default=40_000)
    ap.add_argument("--cadence", type=_positive_int, default=16)
    ap.add_argument("--steps", type=_positive_int, default=201)
    ap.add_argument("--lr", type=float, default=3e-3)
    ap.add_argument("--positive-batch", type=_positive_int, default=1024)
    ap.add_argument("--retention-batch", type=_positive_int, default=4096)
    ap.add_argument("--retention-coef", type=nonnegative_float, default=0.01)
    ap.add_argument("--parameter-coef", type=nonnegative_float, default=1e-4)
    ap.add_argument("--heldout-modulo", type=_positive_int, default=5)
    ap.add_argument("--sample-seed", type=int, default=91)
    ap.add_argument(
        "--min-held-target-greedy",
        type=unit_interval,
        default=0.5,
    )
    ap.add_argument(
        "--max-held-retention-new-choice-rate",
        type=unit_interval,
        default=0.01,
    )
    ap.add_argument(
        "--max-held-retention-mean-kl",
        type=nonnegative_float,
        default=0.02,
    )
    args = ap.parse_args()

    tiers = tuple(entry.strip() for entry in args.tiers.split(","))
    if not all(tiers):
        ap.error("--tiers must be a non-empty comma-separated list")
    config = RevivalConfig(
        actions=args.actions,
        steps=args.steps,
        learning_rate=args.lr,
        positive_batch=args.positive_batch,
        retention_batch=args.retention_batch,
        retention_coefficient=args.retention_coef,
        parameter_coefficient=args.parameter_coef,
        heldout_modulo=args.heldout_modulo,
        sample_seed=args.sample_seed,
    )
    config.validate()
    promoted_actions = args.promote_actions or args.actions
    try:
        validate_promoted_actions(args.actions, promoted_actions)
    except ValueError as err:
        ap.error(str(err))
    driver_identity = input_identity(args.driver)
    dataset = collect_teacher_corpus(
        driver=args.driver,
        scenario_dir=pathlib.Path(args.scenario_dir),
        episodes=args.episodes,
        tiers=tiers,
        episode_seed_base=args.episode_seed_base,
        max_ticks=args.max_ticks,
        cadence=args.cadence,
    )
    source_path = pathlib.Path(args.initialize_from)
    policy, blob, source_kind = load_initializer(source_path)
    parent_policy = copy.deepcopy(policy)
    source_identity = input_identity(source_path, blob)
    training_dir = pathlib.Path(__file__).resolve().parent
    run_lineage = build_lineage(
        phase="selective-revival",
        phase_start_update=int(blob.get("update", 0) or 0),
        hyperparameters={
            "cadence": args.cadence,
            "config": asdict(config),
            "episode_seed_base": args.episode_seed_base,
            "episodes": args.episodes,
            "gym_version": GYM_VERSION,
            "max_ticks": args.max_ticks,
            "promoted_actions": list(promoted_actions),
            "scenario_content_sha256": [
                content_digest(scenario)
                for scenario in bc.duel_scenarios(pathlib.Path(args.scenario_dir))
            ],
            "source_kind": source_kind,
            "strategy_profiles": {
                strategy: {
                    "aggression": aggression,
                    "policy_skill": policy_skill_for_aggression(aggression),
                }
                for strategy, aggression in STRATEGY_AGGRESSION.items()
            },
            "tiers": list(tiers),
        },
        inputs={
            "dequantize_code": input_identity(training_dir / "dequantize.py"),
            "gym_client": input_identity(training_dir / "oxide_gym.py"),
            "gym_driver": driver_identity,
            "model_code": input_identity(training_dir / "models.py"),
            "source": source_identity,
            "teacher": input_identity(training_dir / "bc.py"),
            "trainer": input_identity(training_dir / "revive.py"),
        },
    )
    trained = torch.as_tensor(args.actions, dtype=torch.long)
    with torch.no_grad():
        parent_weight = policy.pi.weight.index_select(0, trained).clone()
        parent_bias = policy.pi.bias.index_select(0, trained).clone()
    joint_audit = train_selected_rows(policy, dataset, config)
    enforce_audit(
        joint_audit,
        min_target_greedy=args.min_held_target_greedy,
        max_new_selected_rate=args.max_held_retention_new_choice_rate,
        max_mean_kl=args.max_held_retention_mean_kl,
    )
    restore_unpromoted_rows(
        policy,
        parent_weight,
        parent_bias,
        args.actions,
        promoted_actions,
    )
    audit = audit_selected_policy(
        parent_policy,
        policy,
        dataset,
        promoted_actions,
        config.heldout_modulo,
    )
    enforce_audit(
        audit,
        min_target_greedy=args.min_held_target_greedy,
        max_new_selected_rate=args.max_held_retention_new_choice_rate,
        max_mean_kl=args.max_held_retention_mean_kl,
    )
    audit["promoted_actions"] = list(promoted_actions)
    audit["joint_training_audit"] = joint_audit

    out_path = pathlib.Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    save_policy(
        policy,
        blob["arch"],
        out_path,
        checkpoint_metadata(
            run_lineage,
            {
                "critic_ready": checkpoint_critic_ready(blob),
                "gym_version": GYM_VERSION,
                "update": blob.get("update"),
                "revival": {
                    "source_content_sha256": source_identity["content_sha256"],
                    "source_kind": source_kind,
                    "config": asdict(config),
                    "episodes": args.episodes,
                    "tiers": list(tiers),
                    "episode_seed_base": args.episode_seed_base,
                    "max_ticks": args.max_ticks,
                    "cadence": args.cadence,
                    "audit": audit,
                },
            },
        ),
    )
    audit_path = (
        pathlib.Path(args.audit_out)
        if args.audit_out
        else out_path.with_suffix(out_path.suffix + ".audit.json")
    )
    audit_path.write_text(json.dumps(audit, indent=2) + "\n")
    print(json.dumps(audit, indent=2))
    print(f"saved {out_path}; audit {audit_path}")


if __name__ == "__main__":
    main()
