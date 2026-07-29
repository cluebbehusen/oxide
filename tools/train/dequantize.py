"""Recovers a trainable float actor from a shipped Q12 artifact.

The artifact contains the policy trunk and action head, but not the
training-only value head. Recovery therefore restores every actor
coefficient exactly at its Q12 value and initializes the critic to
zero. Before writing a checkpoint, the recovered actor is fed through
``export.build_artifact`` and must reproduce every semantic field of
the source.

The shipped bridge deliberately floors newly appended action rows.
``--unfloor-actions`` may make selected all-zero rows explorable by
raising their bias from -8 to 0, after the exact round-trip has passed.

Usage (from tools/train/):
    uv run dequantize.py \
      --weights ../../sim/src/bot/ladder_weights.json \
      --out runs/incumbent-v6-exact.pt
    uv run dequantize.py \
      --weights ../../sim/src/bot/ladder_weights.json \
      --out runs/incumbent-v6-trainable.pt \
      --unfloor-actions 22,23
"""

import argparse
import json
import pathlib
from typing import cast

import torch

from export import Q, build_artifact
from models import Mlp, make_policy, save_policy
from oxide_gym import ACTIONS, CONDITION_DIMS, FEATURES, GYM_VERSION

SEMANTIC_FIELDS = (
    "gym_version",
    "arch",
    "update",
    "q_bits",
    "features",
    "conditioning",
    "actions",
    "recips",
    "tanh_lut",
    "layers",
    "head",
)


def parse_action_indices(text: str) -> tuple[int, ...]:
    """Parses a comma-separated, duplicate-free list of action indices."""
    try:
        actions = tuple(int(part.strip()) for part in text.split(",") if part.strip())
    except ValueError as err:
        raise argparse.ArgumentTypeError(
            f"action indices must be comma-separated integers, got {text!r}"
        ) from err
    if not actions:
        raise argparse.ArgumentTypeError("at least one action index is required")
    if len(set(actions)) != len(actions):
        raise argparse.ArgumentTypeError("action indices must not repeat")
    if any(action < 0 or action >= ACTIONS for action in actions):
        raise argparse.ArgumentTypeError(f"action indices must be in 0..{ACTIONS - 1}")
    return actions


def _semantic_view(artifact: dict) -> dict:
    return {field: artifact.get(field) for field in SEMANTIC_FIELDS}


def recover_actor(artifact: dict) -> tuple[Mlp, dict]:
    """Recovers the exact Q12 actor and a deterministic zero critic."""
    if artifact.get("gym_version") != GYM_VERSION:
        raise ValueError(
            f"weights speak gym v{artifact.get('gym_version')}, "
            f"trainer speaks v{GYM_VERSION}"
        )
    if artifact.get("q_bits") != Q:
        raise ValueError(f"weights use Q{artifact.get('q_bits')}, trainer uses Q{Q}")
    if artifact.get("features") != FEATURES:
        raise ValueError("feature count does not match the training contract")
    if artifact.get("conditioning") != CONDITION_DIMS:
        raise ValueError("conditioning width does not match the training contract")
    if artifact.get("actions") != ACTIONS:
        raise ValueError("action count does not match the training contract")
    arch = artifact.get("arch")
    if not isinstance(arch, str):
        raise TypeError("artifact must name a supported architecture")

    policy = make_policy(arch)
    linears = [
        module
        for module in policy.trunk.modules()
        if isinstance(module, torch.nn.Linear)
    ]
    layers = artifact.get("layers")
    if not isinstance(layers, list) or len(layers) != len(linears):
        raise ValueError("artifact trunk depth does not match its named architecture")

    scale = 1 << Q
    with torch.no_grad():
        for index, (linear, layer) in enumerate(zip(linears, layers, strict=True)):
            if not isinstance(layer, dict):
                raise TypeError(f"artifact layer {index} must be an object")
            typed_layer = cast("dict[str, object]", layer)
            weight_values = cast("list[list[int]]", typed_layer["w"])
            bias_values = cast("list[int]", typed_layer["b"])
            weight = torch.as_tensor(weight_values, dtype=linear.weight.dtype) / scale
            bias = torch.as_tensor(bias_values, dtype=linear.bias.dtype) / scale
            if weight.shape != linear.weight.shape or bias.shape != linear.bias.shape:
                raise ValueError(f"artifact layer {index} shape mismatch")
            linear.weight.copy_(weight)
            linear.bias.copy_(bias)

        head = artifact.get("head")
        if not isinstance(head, dict):
            raise TypeError("artifact has no policy head")
        head_weight = torch.as_tensor(head["w"], dtype=policy.pi.weight.dtype) / scale
        head_bias = torch.as_tensor(head["b"], dtype=policy.pi.bias.dtype) / scale
        if (
            head_weight.shape != policy.pi.weight.shape
            or head_bias.shape != policy.pi.bias.shape
        ):
            raise ValueError("artifact policy head shape mismatch")
        policy.pi.weight.copy_(head_weight)
        policy.pi.bias.copy_(head_bias)

        # The value function is training-only and is not present in a
        # shipped artifact. Zero is deterministic and lets a critic-only
        # warm-up learn without injecting a random actor-adjacent state.
        policy.v.weight.zero_()
        policy.v.bias.zero_()

    blob = {
        "arch": arch,
        "gym_version": GYM_VERSION,
        "update": artifact.get("update"),
    }
    roundtrip = build_artifact(policy, blob)
    source = _semantic_view(artifact)
    recovered = _semantic_view(roundtrip)
    if recovered != source:
        drifted = [
            field for field in SEMANTIC_FIELDS if recovered[field] != source[field]
        ]
        raise ValueError(
            "recovered actor does not round-trip to its source artifact: "
            + ", ".join(drifted)
        )
    return policy, blob


def unfloor_actions(policy: Mlp, artifact: dict, actions: tuple[int, ...]) -> None:
    """Makes selected bridge rows reachable after exact recovery."""
    floor = -(8 << Q)
    head = artifact["head"]
    for action in actions:
        if head["b"][action] != floor or any(head["w"][action]):
            raise ValueError(
                f"action {action} is not an all-zero bridge row at the Q12 floor"
            )
    with torch.no_grad():
        for action in actions:
            policy.pi.weight[action].zero_()
            policy.pi.bias[action].zero_()


def dequantize(
    weights: str | pathlib.Path,
    out: str | pathlib.Path,
    actions: tuple[int, ...] = (),
) -> None:
    """Recovers, verifies, optionally unfloors, and saves one checkpoint."""
    weights_path = pathlib.Path(weights)
    out_path = pathlib.Path(out)
    artifact = json.loads(weights_path.read_text())
    policy, blob = recover_actor(artifact)
    if actions:
        unfloor_actions(policy, artifact, actions)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    save_policy(
        policy,
        blob["arch"],
        out_path,
        {
            "gym_version": blob["gym_version"],
            "update": blob["update"],
            "q12_recovered": True,
            "unfloored_actions": list(actions),
        },
    )


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--weights", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument(
        "--unfloor-actions",
        type=parse_action_indices,
        default=(),
        help="comma-separated all-zero bridge rows whose -8 bias becomes 0",
    )
    args = ap.parse_args()
    dequantize(args.weights, args.out, args.unfloor_actions)
    suffix = (
        f"; unfloored actions {','.join(str(a) for a in args.unfloor_actions)}"
        if args.unfloor_actions
        else ""
    )
    print(f"recovered exact Q12 actor at {args.out} with zero critic{suffix}")


if __name__ == "__main__":
    main()
