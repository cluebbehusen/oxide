"""Policy architectures. Every variant shares the contract: forward
(obs, mask) -> (masked logits, value). Pick with --arch; checkpoints
record which one they are."""

from typing import TYPE_CHECKING

import torch
from torch import nn

from lineage import validate_lineage

if TYPE_CHECKING:
    from pathlib import Path

from oxide_gym import ACTION_HEADS, ACTION_PLAN_DIMS, ACTIONS, GYM_VERSION, NET_FEATURES


def checkpoint_critic_ready(metadata: dict | None) -> bool:
    """Whether a checkpoint carries a critic suitable for immediate PPO."""
    if metadata is None:
        return True
    recorded = metadata.get("critic_ready")
    if recorded is not None:
        if not isinstance(recorded, bool):
            raise TypeError("checkpoint critic_ready must be a boolean")
        return recorded
    # Compatibility for checkpoints written before critic readiness became
    # explicit. BC never trains the value head, old revival checkpoints may
    # derive from recovered Q12, and exact Q12 recovery deterministically
    # zeros it. Conservatively warming an ambiguous legacy revival is safe.
    return not (
        metadata.get("q12_recovered") is True
        or "bc_epoch" in metadata
        or "revival" in metadata
    )


# One constant index tensor per (head, device), built once. The rollout
# rebuilds head distributions on every decision, and allocating four
# small index tensors per call was pure dispatch cost. Callers only read
# through these — never mutate a cached row.
_HEAD_INDICES: dict[tuple[tuple[int, ...], str], torch.Tensor] = {}


def head_indices(head: tuple[int, ...], device: torch.device) -> torch.Tensor:
    """Returns the cached global-index tensor for one action head."""
    key = (head, str(device))
    indices = _HEAD_INDICES.get(key)
    if indices is None:
        indices = torch.as_tensor(head, device=device)
        _HEAD_INDICES[key] = indices
    return indices


def _factorized_distributions(
    logits: torch.Tensor,
) -> list[torch.distributions.Categorical]:
    if logits.ndim == 0 or logits.shape[-1] != ACTIONS:
        raise ValueError(
            "factorized logits must end in "
            f"{ACTIONS} actions, got {tuple(logits.shape)}"
        )
    distributions = []
    for head_index, head in enumerate(ACTION_HEADS):
        indices = head_indices(head, logits.device)
        head_logits = logits.index_select(-1, indices)
        invalid = torch.isnan(head_logits) | torch.isposinf(head_logits)
        if bool(invalid.any().item()):
            raise ValueError(f"action head {head_index} contains NaN or +inf logits")
        if not bool(torch.isfinite(head_logits).any(dim=-1).all().item()):
            raise ValueError(f"action head {head_index} has no legal finite action")
        # The checks above are the real guard, and they are stricter than
        # torch's own parameter validation; asking the distribution to
        # repeat a weaker version of them costs the rollout a second pass
        # over every head on every decision.
        distributions.append(
            torch.distributions.Categorical(logits=head_logits, validate_args=False)
        )
    return distributions


def _local_actions(actions: torch.Tensor) -> list[torch.Tensor]:
    if actions.ndim == 0 or actions.shape[-1] != ACTION_PLAN_DIMS:
        raise ValueError(
            "factorized actions must end in "
            f"{ACTION_PLAN_DIMS} global indices, got {tuple(actions.shape)}"
        )
    local_actions = []
    for head_index, head in enumerate(ACTION_HEADS):
        indices = head_indices(head, actions.device)
        selected = actions[..., head_index]
        matches = selected.unsqueeze(-1) == indices
        valid = matches.any(dim=-1)
        if not bool(valid.all().item()):
            bad = torch.unique(selected[~valid]).detach().cpu().tolist()
            raise ValueError(
                f"global actions {bad} do not belong to action head "
                f"{head_index}: {head}"
            )
        local_actions.append(matches.to(torch.int64).argmax(dim=-1))
    return local_actions


def factorized_sample(logits: torch.Tensor) -> torch.Tensor:
    """Samples one global action index from each independent head."""
    choices = []
    for head, distribution in zip(
        ACTION_HEADS, _factorized_distributions(logits), strict=True
    ):
        local = distribution.sample()
        indices = head_indices(head, logits.device)
        choices.append(indices[local])
    return torch.stack(choices, dim=-1)


def factorized_sample_with_log_prob(
    logits: torch.Tensor,
) -> tuple[torch.Tensor, torch.Tensor]:
    """Samples one plan per row and returns its joint log-probability.

    The collector needs both halves of every decision, and building the
    head distributions a second time to score the draw was its largest
    per-decision torch cost. Each head is drawn by the same call in the
    same order as :func:`factorized_sample`, so the random stream is
    untouched, and the score is read from the very distribution that
    produced the draw, so both results are identical to the split path.
    """
    choices = []
    terms = []
    for head, distribution in zip(
        ACTION_HEADS, _factorized_distributions(logits), strict=True
    ):
        local = distribution.sample()
        terms.append(distribution.log_prob(local))
        indices = head_indices(head, logits.device)
        choices.append(indices[local])
    return torch.stack(choices, dim=-1), torch.stack(terms, dim=-1).sum(dim=-1)


def factorized_greedy(logits: torch.Tensor) -> torch.Tensor:
    """Picks the highest-logit global action independently per head."""
    _factorized_distributions(logits)
    choices = []
    for head in ACTION_HEADS:
        indices = head_indices(head, logits.device)
        local = logits.index_select(-1, indices).argmax(dim=-1)
        choices.append(indices[local])
    return torch.stack(choices, dim=-1)


def factorized_joint_log_prob(
    logits: torch.Tensor,
    actions: torch.Tensor,
) -> torch.Tensor:
    """Returns the sum of the independent head log-probabilities."""
    distributions = _factorized_distributions(logits)
    local_actions = _local_actions(actions)
    terms = [
        distribution.log_prob(local)
        for distribution, local in zip(distributions, local_actions, strict=True)
    ]
    return torch.stack(terms, dim=-1).sum(dim=-1)


def factorized_entropy(logits: torch.Tensor) -> torch.Tensor:
    """Returns entropy per sample, averaged equally across action heads."""
    terms = [
        distribution.entropy() for distribution in _factorized_distributions(logits)
    ]
    return torch.stack(terms, dim=-1).mean(dim=-1)


def factorized_production_entropy(logits: torch.Tensor) -> torch.Tensor:
    """Returns entropy per sample for the production head only."""
    return _factorized_distributions(logits)[0].entropy()


def factorized_kl(
    anchor_logits: torch.Tensor,
    logits: torch.Tensor,
) -> torch.Tensor:
    """Returns KL(anchor || policy) per sample, averaged across heads."""
    anchor_distributions = _factorized_distributions(anchor_logits)
    distributions = _factorized_distributions(logits)
    terms = [
        torch.distributions.kl_divergence(anchor, distribution)
        for anchor, distribution in zip(
            anchor_distributions, distributions, strict=True
        )
    ]
    return torch.stack(terms, dim=-1).mean(dim=-1)


class Mlp(nn.Module):
    """The baseline: two tanh layers, like the plan's small MLP."""

    def __init__(self, hidden: int = 128, depth: int = 2) -> None:
        super().__init__()
        layers: list[nn.Module] = []
        last = NET_FEATURES
        for _ in range(depth):
            layers += [nn.Linear(last, hidden), nn.Tanh()]
            last = hidden
        self.trunk = nn.Sequential(*layers)
        self.pi = nn.Linear(last, ACTIONS)
        self.v = nn.Linear(last, 1)

    def forward(
        self,
        obs: torch.Tensor,
        mask: torch.Tensor,
        *,
        detach_value_trunk: bool = False,
    ) -> tuple[torch.Tensor, torch.Tensor]:
        h = self.trunk(obs)
        logits = self.pi(h).masked_fill(~mask, float("-inf"))
        value_h = h.detach() if detach_value_trunk else h
        return logits, self.v(value_h).squeeze(-1)


def make_policy(arch: str) -> Mlp:
    if arch == "mlp":
        return Mlp(128, 2)
    if arch == "wide":
        return Mlp(256, 3)
    if arch == "deep":
        return Mlp(384, 3)
    if arch == "deep512":
        return Mlp(512, 3)
    if arch == "deep4":
        return Mlp(384, 4)
    raise KeyError(arch)


def load_policy(path: str, device: str = "cpu") -> tuple[Mlp, dict]:
    """Loads a checkpoint saved by save_policy, refusing one recorded
    under a different gym contract — a stale pool checkpoint drawn by
    a past-lane must fail here, not shape-error mid-rollout."""
    blob = torch.load(path, map_location=device, weights_only=True)
    if isinstance(blob, dict) and "arch" in blob:
        recorded = blob.get("gym_version")
        if recorded is not None and recorded != GYM_VERSION:
            raise RuntimeError(
                f"{path} speaks gym v{recorded}, trainer speaks v{GYM_VERSION}"
            )
        if "lineage" in blob:
            blob = {**blob, "lineage": validate_lineage(blob["lineage"])}
        arch = blob["arch"]
        if not isinstance(arch, str):
            raise TypeError("checkpoint arch must be a string")
        policy = make_policy(arch)
        policy.load_state_dict(blob["state"])
        return policy, blob
    # Legacy: bare state_dict from the first trainer (mlp-128).
    policy = make_policy("mlp")
    policy.load_state_dict(blob)
    return policy, {"arch": "mlp"}


def save_policy(
    policy: nn.Module, arch: str, path: str | Path, extra: dict | None = None
) -> None:
    metadata = extra or {}
    if "lineage" in metadata:
        metadata = {
            **metadata,
            "lineage": validate_lineage(metadata["lineage"]),
        }
    blob = {"arch": arch, "state": policy.state_dict(), **metadata}
    torch.save(blob, path)
