"""Masked-PPO trainer for Oxide's gym.

Curriculum over the scripted ladder: train against Scrapheap until the
eval win rate clears the bar, then climb. Seats alternate every episode
so the policy never learns a chair instead of the game. Checkpoints
land in runs/<name>/ as plain state_dicts plus a config stamp.

Usage (from tools/train/):
    uv run train.py --name first --updates 200
    uv run train.py --name first --eval-only --tier veteran
"""

from __future__ import annotations

import argparse
import json
import pathlib
import time

import numpy as np
import torch
import torch.nn as nn

from oxide_gym import ACTIONS, FEATURES, GYM_VERSION, Worker

TIERS = ["scrapheap", "standard", "veteran", "prime"]
ADVANCE_AT = 0.8  # eval win rate that unlocks the next tier


class Policy(nn.Module):
    def __init__(self, hidden: int = 128):
        super().__init__()
        self.trunk = nn.Sequential(
            nn.Linear(FEATURES, hidden),
            nn.Tanh(),
            nn.Linear(hidden, hidden),
            nn.Tanh(),
        )
        self.pi = nn.Linear(hidden, ACTIONS)
        self.v = nn.Linear(hidden, 1)

    def forward(self, obs: torch.Tensor, mask: torch.Tensor):
        h = self.trunk(obs)
        logits = self.pi(h).masked_fill(~mask, float("-inf"))
        return logits, self.v(h).squeeze(-1)


def rollout(policy, workers, seeds, tier, steps, device, rng):
    """Collects `steps` decisions per worker; episodes restart inline."""
    n = len(workers)
    obs_b = np.zeros((steps, n, FEATURES), dtype=np.float32)
    mask_b = np.zeros((steps, n, ACTIONS), dtype=bool)
    act_b = np.zeros((steps, n), dtype=np.int64)
    logp_b = np.zeros((steps, n), dtype=np.float32)
    val_b = np.zeros((steps, n), dtype=np.float32)
    rew_b = np.zeros((steps, n), dtype=np.float32)
    done_b = np.zeros((steps, n), dtype=bool)
    finished: list[bool | None] = []

    states = []
    for w in workers:
        seat = int(rng.integers(2))
        states.append(w.reset(int(next(seeds)), seat, tier))

    for t in range(steps):
        obs = np.stack([s.obs for s in states])
        mask = np.stack([s.mask for s in states])
        with torch.no_grad():
            logits, value = policy(
                torch.as_tensor(obs, device=device),
                torch.as_tensor(mask, device=device),
            )
            dist = torch.distributions.Categorical(logits=logits)
            action = dist.sample()
            logp = dist.log_prob(action)
        obs_b[t], mask_b[t] = obs, mask
        act_b[t] = action.cpu().numpy()
        logp_b[t] = logp.cpu().numpy()
        val_b[t] = value.cpu().numpy()
        for i, w in enumerate(workers):
            r = w.step(act_b[t, i])
            rew_b[t, i] = r.reward
            done_b[t, i] = r.done
            if r.done:
                finished.append(r.win)
                seat = int(rng.integers(2))
                r = w.reset(int(next(seeds)), seat, tier)
            states[i] = r

    obs = np.stack([s.obs for s in states])
    mask = np.stack([s.mask for s in states])
    with torch.no_grad():
        _, last_value = policy(
            torch.as_tensor(obs, device=device),
            torch.as_tensor(mask, device=device),
        )
    return (obs_b, mask_b, act_b, logp_b, val_b, rew_b, done_b), last_value.cpu().numpy(), finished


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


def ppo_update(policy, opt, batch, device, epochs=4, minibatch=1024, clip=0.2, ent_coef=0.01):
    obs, mask, act, logp_old, adv, ret = (torch.as_tensor(x, device=device) for x in batch)
    adv = (adv - adv.mean()) / (adv.std() + 1e-8)
    idx = np.arange(obs.shape[0])
    stats = {"pi": 0.0, "v": 0.0, "ent": 0.0}
    for _ in range(epochs):
        np.random.shuffle(idx)
        for start in range(0, len(idx), minibatch):
            mb = idx[start : start + minibatch]
            logits, value = policy(obs[mb], mask[mb])
            dist = torch.distributions.Categorical(logits=logits)
            logp = dist.log_prob(act[mb])
            ratio = (logp - logp_old[mb]).exp()
            pi_loss = -torch.min(
                ratio * adv[mb],
                ratio.clamp(1 - clip, 1 + clip) * adv[mb],
            ).mean()
            v_loss = (value - ret[mb]).pow(2).mean()
            ent = dist.entropy().mean()
            loss = pi_loss + 0.5 * v_loss - ent_coef * ent
            opt.zero_grad()
            loss.backward()
            nn.utils.clip_grad_norm_(policy.parameters(), 0.5)
            opt.step()
            stats["pi"] += float(pi_loss.detach())
            stats["v"] += float(v_loss.detach())
            stats["ent"] += float(ent.detach())
    return stats


def evaluate(policy, workers, tier, device, seeds=range(1000, 1020)) -> float:
    """Greedy policy, fixed seed suite, both seats per seed."""
    wins = games = 0
    jobs = [(seed, seat) for seed in seeds for seat in (0, 1)]
    for chunk_start in range(0, len(jobs), len(workers)):
        chunk = jobs[chunk_start : chunk_start + len(workers)]
        states = [workers[i].reset(seed, seat, tier) for i, (seed, seat) in enumerate(chunk)]
        live = list(range(len(chunk)))
        while live:
            obs = np.stack([states[i].obs for i in live])
            mask = np.stack([states[i].mask for i in live])
            with torch.no_grad():
                logits, _ = policy(
                    torch.as_tensor(obs, device=device),
                    torch.as_tensor(mask, device=device),
                )
                action = logits.argmax(dim=-1).cpu().numpy()
            still = []
            for k, i in enumerate(live):
                r = workers[i].step(int(action[k]))
                states[i] = r
                if r.done:
                    games += 1
                    wins += 1 if r.win else 0
                else:
                    still.append(i)
            live = still
    return wins / games if games else 0.0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--name", required=True)
    ap.add_argument("--driver", default="../../target/release/oxide-driver")
    ap.add_argument("--workers", type=int, default=8)
    ap.add_argument("--steps", type=int, default=512, help="decisions per worker per update")
    ap.add_argument("--updates", type=int, default=200)
    ap.add_argument("--lr", type=float, default=3e-4)
    ap.add_argument("--tier", default=None, help="pin the opponent tier (else curriculum)")
    ap.add_argument("--resume", default=None, help="checkpoint to load")
    ap.add_argument("--eval-only", action="store_true")
    args = ap.parse_args()

    device = "cpu"
    run_dir = pathlib.Path("runs") / args.name
    run_dir.mkdir(parents=True, exist_ok=True)
    policy = Policy()
    if args.resume:
        policy.load_state_dict(torch.load(args.resume, map_location=device, weights_only=True))
    opt = torch.optim.Adam(policy.parameters(), lr=args.lr)
    workers = [Worker(args.driver) for _ in range(args.workers)]
    rng = np.random.default_rng(0)

    def seed_stream():
        s = 10_000
        while True:
            yield s
            s += 1

    seeds = seed_stream()

    try:
        if args.eval_only:
            tier = args.tier or "veteran"
            rate = evaluate(policy, workers, tier, device)
            print(json.dumps({"eval": tier, "win_rate": rate}))
            return

        tier_i = TIERS.index(args.tier) if args.tier else 0
        log = (run_dir / "log.jsonl").open("a")
        for update in range(1, args.updates + 1):
            tier = TIERS[tier_i]
            t0 = time.time()
            batch, last_val, finished = rollout(
                policy, workers, seeds, tier, args.steps, device, rng
            )
            obs_b, mask_b, act_b, logp_b, val_b, rew_b, done_b = batch
            adv, ret = gae(rew_b, done_b, val_b, last_val)
            flat = (
                obs_b.reshape(-1, FEATURES),
                mask_b.reshape(-1, ACTIONS),
                act_b.reshape(-1),
                logp_b.reshape(-1),
                adv.reshape(-1),
                ret.reshape(-1),
            )
            stats = ppo_update(policy, opt, flat, device)
            wins = sum(1 for w in finished if w is True)
            losses = sum(1 for w in finished if w is False)
            draws = sum(1 for w in finished if w is None)
            entry = {
                "update": update,
                "tier": tier,
                "episodes": len(finished),
                "w": wins,
                "l": losses,
                "d": draws,
                "ent": round(stats["ent"], 3),
                "sec": round(time.time() - t0, 1),
            }
            if update % 10 == 0:
                rate = evaluate(policy, workers, tier, device)
                entry["eval"] = round(rate, 3)
                torch.save(policy.state_dict(), run_dir / f"ckpt-{update:04d}.pt")
                (run_dir / "config.json").write_text(
                    json.dumps({"gym_version": GYM_VERSION, "tier": tier, "update": update})
                )
                if args.tier is None and rate >= ADVANCE_AT and tier_i + 1 < len(TIERS):
                    tier_i += 1
                    entry["advanced_to"] = TIERS[tier_i]
            print(json.dumps(entry), flush=True)
            log.write(json.dumps(entry) + "\n")
            log.flush()
    finally:
        for w in workers:
            w.close()


if __name__ == "__main__":
    main()
