"""League PPO: self-play with anchors.

Pure self-play drifts into self-referential conventions; pure
scripted-opponent play overfits one opponent. The league mixes both,
AlphaStar-fashion, at laptop scale: every rollout each worker is dealt
an opponent kind —

  self    both seats driven by the current policy (both trajectories
          train — the arms race lives here)
  past    a frozen checkpoint from this run's pool (stops cycling:
          you must still beat who you used to be)
  tier    a scripted ladder bot (the anchor that keeps play grounded
          against sensible opponents, and the eventual yardstick)
  rusher  the scripted rush teacher (the known exploit, kept in the
          curriculum forever so the answer to it never fades)

Guards from the first collapsed run: value warm-up before the policy
moves, a KL early stop each update, conservative learning rate.

Usage (from tools/train/):
    uv run league.py --name league1 --resume runs/bc.pt --updates 2000
    uv run league.py --name league1 --resume runs/league1/latest.pt \
        --updates 2000   # continue
"""

from __future__ import annotations

import argparse
import json
import pathlib
import time

import numpy as np
import torch

from models import load_policy, make_policy, save_policy
from oxide_gym import ACTIONS, FEATURES, GYM_VERSION, Frame, Worker
from ppo import gae, ppo_update

TIERS = ["scrapheap", "standard", "veteran", "prime"]

# Rush teacher (indices into the raw feature vector; see gym.rs).
IDLE, TRAIN_H, TRAIN_S, FORM, PUSH, SCOUT = 0, 1, 2, 7, 8, 10


def rusher(raw: list[int], mask: np.ndarray, tick: int) -> int:
    harvesters, staging_size = raw[2], raw[11]
    if harvesters < 4 and mask[TRAIN_H]:
        return TRAIN_H
    if mask[PUSH] and staging_size >= 5:
        return PUSH
    if mask[FORM]:
        return FORM
    if mask[TRAIN_S]:
        return TRAIN_S
    if mask[SCOUT] and tick % 1024 == 0:
        return SCOUT
    return IDLE


class Lane:
    """One learner-controlled seat's trajectory stream."""

    def __init__(self, worker: Worker, seat: int):
        self.worker = worker
        self.seat = seat
        self.obs, self.mask, self.act = [], [], []
        self.logp, self.val, self.rew, self.done = [], [], [], []


class Job:
    """One worker's permanent role. Roles are fixed for the run — the
    lane geometry must never change, because episodes span many rollouts
    and a trajectory stream has to stay contiguous. What varies per
    episode is the detail: which tier, which past checkpoint."""

    def __init__(self, worker: Worker, kind: str, seat: int, pool_dir, rng, device):
        self.worker = worker
        self.kind = kind
        self.pool_dir = pool_dir
        self.rng = rng
        self.device = device
        self.detail = None  # tier name | frozen policy
        self.frame: Frame | None = None
        if kind == "self":
            self.learner_seats = [0, 1]
            self.opp_seat = None
        elif kind == "tier":
            self.learner_seats = [seat]
            self.opp_seat = None
        else:  # past | rusher: both seats controlled, one driven locally
            self.learner_seats = [seat]
            self.opp_seat = 1 - seat

    def reset(self, seed: int):
        if self.kind == "tier":
            self.detail = TIERS[int(self.rng.integers(len(TIERS)))]
            self.frame = self.worker.reset(
                seed, control=(self.learner_seats[0],), tier=self.detail
            )
            return
        if self.kind == "past":
            pool = sorted(self.pool_dir.glob("ckpt-*.pt"))
            if pool:
                pick = pool[int(self.rng.integers(len(pool)))]
                self.detail, _ = load_policy(str(pick), self.device)
                self.detail.eval()
            else:
                self.detail = None  # empty pool: play the rusher instead
        self.frame = self.worker.reset(seed, control=(0, 1))

    def opponent_action(self, policy_device) -> dict[int, int]:
        """Actions for locally-driven seats (empty for self/tier)."""
        if self.opp_seat is None:
            return {}
        view = self.frame.seats[self.opp_seat]
        if self.kind == "rusher" or self.detail is None:
            return {self.opp_seat: rusher(view.raw, view.mask, self.frame.tick)}
        policy, device = self.detail, policy_device
        with torch.no_grad():
            logits, _ = policy(
                torch.as_tensor(view.obs[None], device=device),
                torch.as_tensor(view.mask[None], device=device),
            )
            a = torch.distributions.Categorical(logits=logits).sample()
        return {self.opp_seat: int(a)}


def assign_roles(workers, mix, pool_dir, rng, device):
    """Splits the worker fleet by the mix (largest remainder), seats
    alternating; the assignment is permanent for the run."""
    kinds = list(mix)
    weights = np.asarray([mix[k] for k in kinds], dtype=float)
    weights /= weights.sum()
    exact = weights * len(workers)
    counts = np.floor(exact).astype(int)
    while counts.sum() < len(workers):
        counts[int(np.argmax(exact - counts))] += 1
    jobs = []
    i = 0
    for kind, count in zip(kinds, counts):
        for k in range(count):
            jobs.append(Job(workers[i], kind, k % 2, pool_dir, rng, device))
            i += 1
    return jobs


def rollout(policy, jobs, seeds, steps, device):
    lanes = {(id(j), s): Lane(j.worker, s) for j in jobs for s in j.learner_seats}
    finished_rewards = []
    for j in jobs:
        if j.frame is None:
            j.reset(next(seeds))

    for _ in range(steps):
        views = []
        keys = []
        for j in jobs:
            for s in j.learner_seats:
                v = j.frame.seats[s]
                views.append(v)
                keys.append((id(j), s))
        obs = np.stack([v.obs for v in views])
        mask = np.stack([v.mask for v in views])
        with torch.no_grad():
            logits, value = policy(
                torch.as_tensor(obs, device=device),
                torch.as_tensor(mask, device=device),
            )
            dist = torch.distributions.Categorical(logits=logits)
            action = dist.sample()
            logp = dist.log_prob(action).cpu().numpy()
        action = action.cpu().numpy()
        value = value.cpu().numpy()

        for k, key in enumerate(keys):
            lane = lanes[key]
            lane.obs.append(obs[k])
            lane.mask.append(mask[k])
            lane.act.append(action[k])
            lane.logp.append(logp[k])
            lane.val.append(value[k])

        cursor = 0
        for j in jobs:
            acts = {s: int(action[cursor + i]) for i, s in enumerate(j.learner_seats)}
            cursor += len(j.learner_seats)
            acts.update(j.opponent_action(device))
            frame = j.worker.step(acts)
            if frame.done:
                for s in j.learner_seats:
                    lane = lanes[(id(j), s)]
                    lane.rew.append(frame.reward(s))
                    lane.done.append(True)
                    finished_rewards.append(frame.reward(s))
                j.reset(next(seeds))
            else:
                for s in j.learner_seats:
                    lane = lanes[(id(j), s)]
                    lane.rew.append(-1e-4)
                    lane.done.append(False)
            if not frame.done:
                j.frame = frame

    # Bootstrap values for unfinished lanes.
    views = []
    for j in jobs:
        for s in j.learner_seats:
            views.append(j.frame.seats[s])
    obs = np.stack([v.obs for v in views])
    mask = np.stack([v.mask for v in views])
    with torch.no_grad():
        _, last_val = policy(
            torch.as_tensor(obs, device=device),
            torch.as_tensor(mask, device=device),
        )
    last_val = last_val.cpu().numpy()

    ordered = list(lanes.values())
    batch = (
        np.stack([np.stack(l.obs) for l in ordered], axis=1),
        np.stack([np.stack(l.mask) for l in ordered], axis=1),
        np.stack([np.asarray(l.act) for l in ordered], axis=1),
        np.stack([np.asarray(l.logp, dtype=np.float32) for l in ordered], axis=1),
        np.stack([np.asarray(l.val, dtype=np.float32) for l in ordered], axis=1),
        np.stack([np.asarray(l.rew, dtype=np.float32) for l in ordered], axis=1),
        np.stack([np.asarray(l.done) for l in ordered], axis=1),
    )
    return batch, last_val, finished_rewards


def evaluate(policy, workers, device, opponent: str, seeds=range(1000, 1010)) -> float:
    """Greedy, fixed suite, both seats per seed. `opponent` is a tier
    name or 'rusher'."""
    wins = games = 0
    jobs = [(seed, seat) for seed in seeds for seat in (0, 1)]
    for start in range(0, len(jobs), len(workers)):
        chunk = jobs[start : start + len(workers)]
        live = []
        for i, (seed, seat) in enumerate(chunk):
            w = workers[i]
            if opponent == "rusher":
                frame = w.reset(seed, control=(0, 1))
            else:
                frame = w.reset(seed, control=(seat,), tier=opponent)
            live.append((i, seat, frame))
        while live:
            still = []
            obs = np.stack([f.seats[seat].obs for _, seat, f in live])
            mask = np.stack([f.seats[seat].mask for _, seat, f in live])
            with torch.no_grad():
                logits, _ = policy(
                    torch.as_tensor(obs, device=device),
                    torch.as_tensor(mask, device=device),
                )
                action = logits.argmax(dim=-1).cpu().numpy()
            for k, (i, seat, frame) in enumerate(live):
                acts = {seat: int(action[k])}
                if opponent == "rusher":
                    ov = frame.seats[1 - seat]
                    acts[1 - seat] = rusher(ov.raw, ov.mask, frame.tick)
                nxt = workers[i].step(acts)
                if nxt.done:
                    games += 1
                    wins += 1 if nxt.winner == seat else 0
                else:
                    still.append((i, seat, nxt))
            live = still
    return wins / games if games else 0.0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--name", required=True)
    ap.add_argument("--driver", default="../../target/release/oxide-driver")
    ap.add_argument("--arch", default="mlp", help="mlp | wide (ignored with --resume)")
    ap.add_argument("--workers", type=int, default=8)
    ap.add_argument("--steps", type=int, default=384)
    ap.add_argument("--updates", type=int, default=2000)
    ap.add_argument("--lr", type=float, default=1e-4)
    ap.add_argument("--value-warmup", type=int, default=15)
    ap.add_argument("--pool-every", type=int, default=25)
    ap.add_argument("--eval-every", type=int, default=25)
    ap.add_argument("--resume", default=None)
    ap.add_argument(
        "--mix",
        default="self=0.45,past=0.20,tier=0.20,rusher=0.15",
        help="opponent kind weights",
    )
    args = ap.parse_args()

    device = "cpu"
    torch.manual_seed(0)
    run_dir = pathlib.Path("runs") / args.name
    pool_dir = run_dir / "pool"
    pool_dir.mkdir(parents=True, exist_ok=True)
    mix = {k: float(v) for k, v in (kv.split("=") for kv in args.mix.split(","))}

    if args.resume:
        policy, blob = load_policy(args.resume, device)
        arch = blob.get("arch", "mlp")
    else:
        arch = args.arch
        policy = make_policy(arch)
    opt = torch.optim.Adam(policy.parameters(), lr=args.lr)
    workers = [Worker(args.driver) for _ in range(args.workers)]
    rng = np.random.default_rng(0)

    def seed_stream():
        s = 50_000
        while True:
            yield s
            s += 1

    seeds = seed_stream()
    log = (run_dir / "log.jsonl").open("a")

    try:
        jobs = assign_roles(workers, mix, pool_dir, rng, device)
        for update in range(1, args.updates + 1):
            t0 = time.time()
            batch, last_val, finals = rollout(policy, jobs, seeds, args.steps, device)
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
            stats = ppo_update(
                policy,
                opt,
                flat,
                device,
                value_only=update <= args.value_warmup,
            )
            entry = {
                "update": update,
                "kinds": sorted(j.kind for j in jobs),
                "episodes": len(finals),
                "avg_final": round(float(np.mean(finals)), 3) if finals else None,
                "ent": round(stats["ent"] / max(stats["batches"], 1), 3),
                "kl": round(stats["kl"], 4),
                "sec": round(time.time() - t0, 1),
            }
            if update % args.pool_every == 0:
                save_policy(
                    policy,
                    arch,
                    pool_dir / f"ckpt-{update:05d}.pt",
                    {"gym_version": GYM_VERSION, "update": update},
                )
            if update % args.eval_every == 0:
                entry["eval"] = {
                    op: round(evaluate(policy, workers, device, op), 3)
                    for op in ("veteran", "prime", "rusher")
                }
                save_policy(
                    policy,
                    arch,
                    run_dir / "latest.pt",
                    {"gym_version": GYM_VERSION, "update": update},
                )
                # Eval borrowed the workers; the standing episodes are
                # gone. Fresh ones start next rollout.
                for j in jobs:
                    j.frame = None
            print(json.dumps(entry), flush=True)
            log.write(json.dumps(entry) + "\n")
            log.flush()
    finally:
        for w in workers:
            w.close()


if __name__ == "__main__":
    main()
