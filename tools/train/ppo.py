"""Shared PPO machinery: GAE and the clipped update, with the guards
the first collapsed run taught us — a KL early stop so fine-tuning
can't sprint away from a working policy, and an optional policy freeze
for value warm-up."""

from __future__ import annotations

import numpy as np
import torch
import torch.nn as nn


def gae(rew, done, val, last_val, gamma=0.999, lam=0.95):
    steps, n = rew.shape
    adv = np.zeros_like(rew)
    running = np.zeros(n, dtype=np.float32)
    next_val = last_val
    for t in reversed(range(steps)):
        nonterminal = ~done[t]
        delta = rew[t] + gamma * next_val * nonterminal - val[t]
        running = delta + gamma * lam * nonterminal * running
        adv[t] = running
        next_val = np.where(done[t], 0.0, val[t])
    return adv, adv + val


def ppo_update(
    policy,
    opt,
    batch,
    device,
    epochs=4,
    minibatch=1024,
    clip=0.2,
    ent_coef=0.005,
    kl_stop=0.03,
    value_only=False,
):
    obs, mask, act, logp_old, adv, ret = (torch.as_tensor(x, device=device) for x in batch)
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
