"""Policy architectures. Every variant shares the contract: forward
(obs, mask) -> (masked logits, value). Pick with --arch; checkpoints
record which one they are."""

from typing import TYPE_CHECKING

import torch
from torch import nn

if TYPE_CHECKING:
    from pathlib import Path

from oxide_gym import ACTIONS, NET_FEATURES


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
        self, obs: torch.Tensor, mask: torch.Tensor
    ) -> tuple[torch.Tensor, torch.Tensor]:
        h = self.trunk(obs)
        logits = self.pi(h).masked_fill(~mask, float("-inf"))
        return logits, self.v(h).squeeze(-1)


def make_policy(arch: str) -> Mlp:
    if arch == "mlp":
        return Mlp(128, 2)
    if arch == "wide":
        return Mlp(256, 3)
    if arch == "deep":
        return Mlp(384, 3)
    raise KeyError(arch)


def load_policy(path: str, device: str = "cpu") -> tuple[Mlp, dict]:
    """Loads a checkpoint saved by save_policy."""
    blob = torch.load(path, map_location=device, weights_only=True)
    if isinstance(blob, dict) and "arch" in blob:
        policy = make_policy(blob["arch"])
        policy.load_state_dict(blob["state"])
        return policy, blob
    # Legacy: bare state_dict from the first trainer (mlp-128).
    policy = make_policy("mlp")
    policy.load_state_dict(blob)
    return policy, {"arch": "mlp"}


def save_policy(
    policy: nn.Module, arch: str, path: str | Path, extra: dict | None = None
) -> None:
    blob = {"arch": arch, "state": policy.state_dict(), **(extra or {})}
    torch.save(blob, path)
