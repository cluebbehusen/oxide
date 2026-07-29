"""Shared PPO machinery: GAE and the clipped update, with the guards
the first collapsed run taught us — a KL early stop so fine-tuning
can't sprint away from a working policy, and an optional policy freeze
for value warm-up. The equal-head entropy bonus can be supplemented by
independent exploration pressure on the production head."""

from typing import TYPE_CHECKING

import numpy as np
import torch
from torch import nn

if TYPE_CHECKING:
    from models import Mlp

from models import (
    factorized_entropy,
    factorized_joint_log_prob,
    factorized_kl,
    factorized_production_entropy,
)

TRAIN_GAMMA = 0.9997


def gae(
    rew: np.ndarray,
    done: np.ndarray,
    val: np.ndarray,
    last_val: np.ndarray,
    gamma: float = TRAIN_GAMMA,
    lam: float = 0.95,
) -> tuple[np.ndarray, np.ndarray]:
    steps, n = rew.shape
    adv = np.zeros_like(rew)
    running = np.zeros(n, dtype=np.float32)
    next_val = last_val
    for t in reversed(range(steps)):
        nonterminal = ~done[t]
        delta = rew[t] + gamma * next_val * nonterminal - val[t]
        running = delta + gamma * lam * nonterminal * running
        adv[t] = running
        # done[t] describes the transition *after* state t; V(s_t) is
        # still the bootstrap the preceding step needs. The nonterminal
        # mask above is what cuts credit across episode boundaries —
        # zeroing here instead robbed every penultimate step of
        # gamma * V(s_t).
        next_val = val[t]
    return adv, adv + val


def ppo_update(
    policy: Mlp,
    opt: torch.optim.Optimizer,
    batch: tuple[np.ndarray, ...],
    device: str,
    epochs: int = 4,
    minibatch: int = 1024,
    clip: float = 0.2,
    ent_coef: float = 0.002,
    production_ent_coef: float = 0.0,
    kl_stop: float = 0.03,
    value_only: bool = False,
    anchor: Mlp | None = None,
    anchor_coef: float = 0.05,
    rng: np.random.Generator | None = None,
) -> dict[str, float]:
    obs, mask, act, logp_old, adv, ret = (
        torch.as_tensor(x, device=device) for x in batch
    )
    adv = (adv - adv.mean()) / (adv.std() + 1e-8)
    shuffle_rng = rng if rng is not None else np.random.default_rng(0)
    stats = {
        "pi": 0.0,
        "v": 0.0,
        "ent": 0.0,
        "production_ent": 0.0,
        "kl": 0.0,
        "batches": 0,
    }
    for _ in range(epochs):
        idx = shuffle_rng.permutation(obs.shape[0])
        for start in range(0, len(idx), minibatch):
            mb = idx[start : start + minibatch]
            # A recovered Q12 artifact has no critic. During its value
            # warm-up the trunk must remain an exact actor: detaching
            # the value path lets only `v` learn while the policy logits
            # and every actor coefficient stay bit-identical.
            logits, value = policy(
                obs[mb],
                mask[mb],
                detach_value_trunk=value_only,
            )
            logp = factorized_joint_log_prob(logits, act[mb])
            kl = float((logp_old[mb] - logp).mean().detach())
            if not value_only and kl_stop and abs(kl) > kl_stop:
                stats["kl"] = kl
                return stats  # far enough for one rollout
            ratio = (logp - logp_old[mb]).exp()
            pi_loss = -torch.min(
                ratio * adv[mb],
                ratio.clamp(1 - clip, 1 + clip) * adv[mb],
            ).mean()
            v_loss = (value - ret[mb]).pow(2).mean()
            ent = factorized_entropy(logits).mean()
            production_ent = factorized_production_entropy(logits).mean()
            if value_only:
                loss = 0.5 * v_loss
            else:
                loss = pi_loss + 0.5 * v_loss - ent_coef * ent
                if production_ent_coef:
                    loss = loss - production_ent_coef * production_ent
                if anchor is not None:
                    # Stay near the prior that already plays the game:
                    # a narrow behavior-cloned policy dissolves under
                    # entropy pressure and off-distribution drift long
                    # before PPO's own KL guard notices anything.
                    with torch.no_grad():
                        a_logits, _ = anchor(obs[mb], mask[mb])
                    loss = loss + anchor_coef * factorized_kl(a_logits, logits).mean()
            opt.zero_grad()
            loss.backward()
            nn.utils.clip_grad_norm_(policy.parameters(), 0.5)
            opt.step()
            stats["pi"] += float(pi_loss.detach())
            stats["v"] += float(v_loss.detach())
            stats["ent"] += float(ent.detach())
            stats["production_ent"] += float(production_ent.detach())
            stats["kl"] = kl
            stats["batches"] += 1
    return stats
