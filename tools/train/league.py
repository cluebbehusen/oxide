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
from oxide_gym import ACTIONS, GYM_VERSION, NET_FEATURES, Frame, Worker
from ppo import gae, ppo_update

TIERS = ["scrapheap", "standard", "veteran", "prime"]

# Potential-based shaping: a small dense signal that guides the value
# net through the thousand-decision desert between terminal rewards.
# Own material only — an earlier version subtracted *known* enemy
# strength, and under fog "known" is an information artifact: potential
# dropped whenever the enemy came into view, so the shaping taught the
# policy to stay blind and avoid contact. Never build a reward out of
# what the agent happens to know about the enemy.
SHAPE_K = 0.05

# Style shaping: a per-step nudge that makes the aggression knob mean
# something. Aggressive settings are paid for being out fighting (army
# state Pushing/Engaging); turtle settings for standing home defense.
# Sized so a full game's worth of style adds up to the order of the
# terminal reward — style must be able to argue with winning, or the
# greedy policy plays one line at every knob setting.
STYLE_K = 0.0025


def potential(raw: list[int]) -> float:
    my_strength, harvesters = raw[20], raw[2]
    return (my_strength + 25 * harvesters) / 500.0


def style_reward(raw: list[int], aggression: int) -> float:
    lean = (aggression - 500) / 500.0
    out_fighting = 1.0 if raw[12] in (2, 3) else -1.0
    return STYLE_K * lean * out_fighting


def sample_condition(rng) -> tuple[int, int]:
    """Per-episode knobs: skill favors the strong end (that end must
    stay sharpest) but visits the whole range; aggression is uniform."""
    skill = int(rng.choice([1000, 1000, 850, 700, 550, 400]))
    return skill, int(rng.integers(0, 1001))


def maybe_blunder(action: int, logits, mask, skill: int, rng) -> int:
    """Env-noise blunders, sticky-actions style: the executed action is
    degraded, the policy trains on what it intended. Near-best picks —
    a blunder is a plausible mistake, not madness."""
    eps = (1000 - skill) / 2000.0  # skill 400 -> 30% blunders
    if eps <= 0 or rng.random() >= eps:
        return action
    order = np.argsort(-logits)
    legal = [int(i) for i in order if mask[i]]
    if len(legal) < 2:
        return action
    return int(rng.choice(legal[1 : min(3, len(legal))]))

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
        self.last_pot = 0.0


class Job:
    """One worker's permanent role. Roles are fixed for the run — the
    lane geometry must never change, because episodes span many rollouts
    and a trajectory stream has to stay contiguous. What varies per
    episode is the detail: which tier, which past checkpoint."""

    def __init__(self, worker: Worker, kind: str, seat: int, pool_dir, rng, device, maps="fixed"):
        # seat: 0/1 for duel kinds; 0..3 for ffa.
        self.worker = worker
        self.kind = kind
        self.pool_dir = pool_dir
        self.rng = rng
        self.device = device
        self.maps = maps
        self.detail = None  # tier name | frozen policy
        self.frame: Frame | None = None
        self.conditions: dict[int, tuple[int, int]] = {}
        if kind == "self":
            self.learner_seats = [0, 1]
            self.opp_seat = None
        elif kind in ("tier", "ffa"):
            self.learner_seats = [seat]
            self.opp_seat = None
        else:  # past | rusher: both seats controlled, one driven locally
            self.learner_seats = [seat]
            self.opp_seat = 1 - seat

    def reset(self, seed: int):
        self.conditions = {s: sample_condition(self.rng) for s in self.learner_seats}
        scenario = None
        if self.maps == "random":
            from mapgen import generate

            scenario = generate(seed % 100_000, "/tmp/oxide-maps-train")
        if self.kind == "ffa":
            from mapgen import generate

            scenario = generate(seed % 100_000, "/tmp/oxide-maps-train4", players=4)
            self.detail = TIERS[int(self.rng.integers(len(TIERS)))]
            self.frame = self.worker.reset(
                seed,
                control=(self.learner_seats[0],),
                tier=self.detail,
                conditions=self.conditions,
                scenario=scenario,
            )
            return
        if self.kind == "tier":
            self.detail = TIERS[int(self.rng.integers(len(TIERS)))]
            self.frame = self.worker.reset(
                seed,
                control=(self.learner_seats[0],),
                tier=self.detail,
                conditions=self.conditions,
                scenario=scenario,
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
        all_conds = dict(self.conditions)
        if self.opp_seat is not None:
            all_conds[self.opp_seat] = (1000, 500)  # frozen opponents play straight
        self.frame = self.worker.reset(
            seed, control=(0, 1), conditions=all_conds, scenario=scenario
        )

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


def assign_roles(workers, mix, pool_dir, rng, device, maps="fixed"):
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
        seats = 4 if kind == "ffa" else 2
        for k in range(count):
            jobs.append(Job(workers[i], kind, k % seats, pool_dir, rng, device, maps))
            i += 1
    return jobs


def rollout(policy, jobs, seeds, steps, device):
    lanes = {(id(j), s): Lane(j.worker, s) for j in jobs for s in j.learner_seats}
    finished_rewards = []
    for j in jobs:
        if j.frame is None:
            j.reset(next(seeds))
    for j in jobs:
        for s in j.learner_seats:
            lanes[(id(j), s)].last_pot = potential(j.frame.seats[s].raw)

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
        logits_np = logits.cpu().numpy()
        action = action.cpu().numpy()
        value = value.cpu().numpy()

        for k, key in enumerate(keys):
            lane = lanes[key]
            lane.obs.append(obs[k])
            lane.mask.append(mask[k])
            lane.act.append(action[k])
            lane.logp.append(logp[k])
            lane.val.append(value[k])

        row = {key: k for k, key in enumerate(keys)}
        py_rng = np.random.default_rng(int(action.sum()) + len(finished_rewards))
        cursor = 0
        for j in jobs:
            acts = {}
            for s in j.learner_seats:
                k = row[(id(j), s)]
                acts[s] = maybe_blunder(
                    int(action[k]), logits_np[k], mask[k], j.conditions[s][0], py_rng
                )
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
                for s in j.learner_seats:
                    lanes[(id(j), s)].last_pot = potential(j.frame.seats[s].raw)
            else:
                for s in j.learner_seats:
                    lane = lanes[(id(j), s)]
                    raw = frame.seats[s].raw
                    pot = potential(raw)
                    lane.rew.append(
                        -1e-4
                        + SHAPE_K * (pot - lane.last_pot)
                        + style_reward(raw, j.conditions[s][1])
                    )
                    lane.done.append(False)
                    lane.last_pot = pot
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
            straight = {0: (1000, 500), 1: (1000, 500)}
            if opponent == "rusher":
                frame = w.reset(seed, control=(0, 1), conditions=straight)
            else:
                frame = w.reset(seed, control=(seat,), tier=opponent, conditions=straight)
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
    ap.add_argument("--anchor", default="runs/bc.pt", help="KL anchor prior ('' disables)")
    ap.add_argument("--anchor-coef", type=float, default=0.05)
    ap.add_argument("--maps", default="fixed", help="fixed | random (fresh map per episode)")
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

    start_update = 0
    if args.resume:
        policy, blob = load_policy(args.resume, device)
        arch = blob.get("arch", "mlp")
        # Continue the run's clock: pool numbering, value warm-up, and
        # anchor annealing all key off the absolute update, so resuming
        # must not rewind them (or overwrite pool history).
        start_update = int(blob.get("update", 0) or 0)
    else:
        arch = args.arch
        policy = make_policy(arch)
    opt = torch.optim.Adam(policy.parameters(), lr=args.lr)
    anchor = None
    if args.anchor:
        anchor, _ = load_policy(args.anchor, device)
        anchor.eval()
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
        jobs = assign_roles(workers, mix, pool_dir, rng, device, args.maps)
        for update in range(start_update + 1, start_update + args.updates + 1):
            t0 = time.time()
            batch, last_val, finals = rollout(policy, jobs, seeds, args.steps, device)
            obs_b, mask_b, act_b, logp_b, val_b, rew_b, done_b = batch
            adv, ret = gae(rew_b, done_b, val_b, last_val)
            flat = (
                obs_b.reshape(-1, NET_FEATURES),
                mask_b.reshape(-1, ACTIONS),
                act_b.reshape(-1),
                logp_b.reshape(-1),
                adv.reshape(-1),
                ret.reshape(-1),
            )
            # The anchor is scaffolding: essential while the policy is a
            # fragile clone, a straitjacket once the league is teaching —
            # and it pins every knob setting to the teacher's one style.
            # Anneal it away (halves roughly every 140 updates).
            stats = ppo_update(
                policy,
                opt,
                flat,
                device,
                value_only=update <= args.value_warmup,
                anchor=anchor,
                anchor_coef=args.anchor_coef * (0.995**update),
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
