"""Shared PPO machinery: GAE and the clipped update, with the guards
the first collapsed run taught us — a KL early stop so fine-tuning
can't sprint away from a working policy, and an optional policy freeze
for value warm-up."""

import numpy as np
import torch
from torch import nn


def gae(
    rew: np.ndarray,
    done: np.ndarray,
    val: np.ndarray,
    last_val: np.ndarray,
    gamma: float = 0.999,
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
    policy: nn.Module,
    opt: torch.optim.Optimizer,
    batch: tuple[np.ndarray, ...],
    device: str,
    epochs: int = 4,
    minibatch: int = 1024,
    clip: float = 0.2,
    ent_coef: float = 0.002,
    kl_stop: float = 0.03,
    value_only: bool = False,
    anchor: nn.Module | None = None,
    anchor_coef: float = 0.05,
) -> dict[str, float]:
    obs, mask, act, logp_old, adv, ret = (
        torch.as_tensor(x, device=device) for x in batch
    )
    adv = (adv - adv.mean()) / (adv.std() + 1e-8)
    idx = np.arange(obs.shape[0])
    stats = {"pi": 0.0, "v": 0.0, "ent": 0.0, "kl": 0.0, "batches": 0}
    for _ in range(epochs):
        np.random.shuffle(idx)
        for start in range(0, len(idx), minibatch):
            mb = idx[start : start + minibatch]
            logits, value = policy(obs[mb], mask[mb])
            dist = torch.distributions.Categorical(logits=logits)
            logp = dist.log_prob(act[mb])
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
            ent = dist.entropy().mean()
            if value_only:
                loss = 0.5 * v_loss
            else:
                loss = pi_loss + 0.5 * v_loss - ent_coef * ent
                if anchor is not None:
                    # Stay near the prior that already plays the game:
                    # a narrow behavior-cloned policy dissolves under
                    # entropy pressure and off-distribution drift long
                    # before PPO's own KL guard notices anything.
                    with torch.no_grad():
                        a_logits, _ = anchor(obs[mb], mask[mb])
                    a_dist = torch.distributions.Categorical(logits=a_logits)
                    loss = (
                        loss
                        + anchor_coef
                        * torch.distributions.kl_divergence(a_dist, dist).mean()
                    )
            opt.zero_grad()
            loss.backward()
            nn.utils.clip_grad_norm_(policy.parameters(), 0.5)
            opt.step()
            stats["pi"] += float(pi_loss.detach())
            stats["v"] += float(v_loss.detach())
            stats["ent"] += float(ent.detach())
            stats["kl"] = kl
            stats["batches"] += 1
    return stats
