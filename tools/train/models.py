"""Policy architectures. Every variant shares the contract: forward
(obs, mask) -> (masked logits, value). Pick with --arch; checkpoints
record which one they are."""

from __future__ import annotations

import torch
import torch.nn as nn

from oxide_gym import ACTIONS, FEATURES


class Mlp(nn.Module):
    """The baseline: two tanh layers, like the plan's small MLP."""

    def __init__(self, hidden: int = 128, depth: int = 2):
        super().__init__()
        layers: list[nn.Module] = []
        last = FEATURES
        for _ in range(depth):
            layers += [nn.Linear(last, hidden), nn.Tanh()]
            last = hidden
        self.trunk = nn.Sequential(*layers)
        self.pi = nn.Linear(last, ACTIONS)
        self.v = nn.Linear(last, 1)

    def forward(self, obs: torch.Tensor, mask: torch.Tensor):
        h = self.trunk(obs)
        logits = self.pi(h).masked_fill(~mask, float("-inf"))
        return logits, self.v(h).squeeze(-1)


ARCHS = {
    "mlp": lambda: Mlp(128, 2),
    "wide": lambda: Mlp(256, 3),
}


def make_policy(arch: str) -> nn.Module:
    return ARCHS[arch]()


def load_policy(path: str, device: str = "cpu") -> tuple[nn.Module, dict]:
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


def save_policy(policy: nn.Module, arch: str, path, extra: dict | None = None):
    blob = {"arch": arch, "state": policy.state_dict(), **(extra or {})}
    torch.save(blob, path)
