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

import argparse
import contextlib
import json
import pathlib
import subprocess
import threading
import time
from collections import Counter
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from collections.abc import Iterable, Iterator

import numpy as np
import torch
from torch import nn

from export import export
from mapgen import cache_dir
from mapgen import generate as _generate
from models import load_policy, make_policy, save_policy
from oxide_gym import (
    ACTIONS,
    FEATURE_NAMES,
    GYM_VERSION,
    NET_FEATURES,
    Frame,
    SeatView,
    Worker,
)
from ppo import gae, ppo_update

TIERS = ["scrapheap", "standard", "veteran", "prime"]

# Per-update phase clocks, drained into every log entry — optimization
# without a stable meter is guessing. Keys: env_sec (worker RPC),
# policy_sec (learner forward passes), mapgen_sec, reset_sec, resets.
TEL: Counter = Counter()


@contextlib.contextmanager
def timed(key: str) -> Iterator[None]:
    """Accumulates wall time under a telemetry key."""
    t = time.perf_counter()
    try:
        yield
    finally:
        TEL[key] += time.perf_counter() - t


def generate(
    seed: int,
    out_dir: str,
    players: int = 2,
    teams: bool = False,
    pace: str | None = None,
) -> str:
    """mapgen.generate with its wall time metered."""
    with timed("mapgen_sec"):
        return _generate(seed, out_dir, players=players, teams=teams, pace=pace)


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


F = {name: i for i, name in enumerate(FEATURE_NAMES)}


def potential(raw: list[int]) -> float:
    # Standing buildings enter the potential at a third of their scrap
    # cost (roughly the strength their price buys in units), so
    # Salvage moves value between forms instead of climbing a
    # unit-only potential for free — sell-Bastion-train-scuttlers was
    # monotone positive before this term existed.
    my_strength = raw[F["my_strength"]]
    harvesters = raw[F["my_harvesters"]]
    buildings = raw[F["my_building_value"]] / 3.0
    return (my_strength + 25 * harvesters + buildings) / 500.0


def style_reward(raw: list[int], aggression: int) -> float:
    lean = (aggression - 500) / 500.0
    out_fighting = 1.0 if raw[F["army_state"]] in (2, 3) else -1.0
    return STYLE_K * lean * out_fighting


FAB_BUILT = FEATURE_NAMES.index("fab_built")
# Action index the gym assigns Salvage (v5's appended verb).
SALVAGE_ACTION = 21
# The v6 weld pair, appended in this order: unit repair and the
# Repair Bay's build slot.
REPAIR_ACTION = 22
BUILD_BAY_ACTION = 23

# The agent's own army-count features with rough unit costs (varied
# roles use the midpoint of their two faction kinds) — the same
# cost-weighted lens the fun gate judges with.
ARMY_FEATURES = [
    (FEATURE_NAMES.index("my_harvesters"), 50.0),
    (FEATURE_NAMES.index("my_sentinels"), 90.0),
    (FEATURE_NAMES.index("my_scuttlers"), 40.0),
    (FEATURE_NAMES.index("my_lancers"), 110.0),
    (FEATURE_NAMES.index("my_bombards"), 200.0),
    (FEATURE_NAMES.index("my_antiair"), 67.0),
    (FEATURE_NAMES.index("my_airground"), 125.0),
    (FEATURE_NAMES.index("my_airair"), 90.0),
]


def comp_entropy(raw: list[int]) -> float:
    """Shannon entropy (bits) of the seat's OWN cost-weighted army mix —
    fog-safe by construction, exactly the fun gate's spam metric applied
    to the one army the agent always sees: its own."""
    weights = [raw[i] * cost for i, cost in ARMY_FEATURES]
    total = sum(weights)
    if total <= 0.0:
        return 0.0
    h = 0.0
    for w in weights:
        if w > 0.0:
            p = w / total
            h -= p * float(np.log2(p))
    return h


def tech_bonus_at(base: float, rel_update: int, span: int) -> float:
    """The own-tech terminal bonus's annealing schedule: full at the
    run's first update, linearly down to zero at `span` updates in.

    The bonus itself is fog-safe by construction — it reads the seat's
    OWN fabricator count, never enemy state (a reward built from what
    the agent happens to know about the enemy teaches blindness). The
    anneal hands the argument back to winning: the bonus exists to get
    the tech tree explored early, not to be farmed at convergence."""
    if base == 0.0 or span <= 0:
        return 0.0
    return base * max(0.0, 1.0 - rel_update / span)


def unit_interval(text: str) -> float:
    """argparse type for decay factors: finite, in [0, 1]. A negative
    decay flips the KL sign on odd updates and actively rewards
    diverging from the anchor; nan poisons the loss silently."""
    value = float(text)
    if not np.isfinite(value) or not 0.0 <= value <= 1.0:
        raise argparse.ArgumentTypeError(f"decay must be finite in [0, 1], got {text}")
    return value


def faction_knob(seat: int) -> int:
    """The seat's faction, by the map convention every shipped and
    generated scenario follows: even seats run Ferrous (0), odd seats
    Cupric (1000). The knob is honest, never sampled — a policy trained
    on lies about its own roster learns nothing about either."""
    return 0 if seat % 2 == 0 else 1000


def sample_condition(rng: np.random.Generator, seat: int) -> tuple[int, int, int]:
    """Per-episode knobs: skill favors the strong end (that end must
    stay sharpest) but visits the whole range; aggression is uniform;
    faction follows the seat."""
    skill = int(rng.choice([1000, 1000, 850, 700, 550, 400]))
    return skill, int(rng.integers(0, 1001)), faction_knob(seat)


def maybe_blunder(
    action: int,
    _logits: np.ndarray,
    _mask: np.ndarray,
    skill: int,
    rng: np.random.Generator,
) -> int:
    """Env-noise blunders, sticky-actions style: the executed action is
    degraded, the policy trains on what it intended. A blunder is
    HESITATION (the decision window passes unused) — matching the
    shipped sim's model, so sub-1000 conditioning trains under exactly
    the degradation it deploys with. The old near-best-pick blunders
    kept spending the Fabricator fund mid-save, which both taught the
    policy that low skill means spam and mismatched the runtime."""
    eps = (1000 - skill) / 2000.0  # skill 400 -> 30% blunders
    if eps <= 0 or rng.random() >= eps:
        return action
    return 0  # Action IDLE


# Rush teacher — the v3 action menu; feature indices resolved by name.
IDLE, TRAIN_H, TRAIN_S, FORM, PUSH, SCOUT = 0, 1, 2, 17, 18, 20


def rusher(raw: list[int], mask: np.ndarray, tick: int) -> int:
    harvesters, staging_size = raw[F["my_harvesters"]], raw[F["staging_army_size"]]
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

    def __init__(self, worker: Worker, seat: int) -> None:
        self.worker = worker
        self.seat = seat
        self.obs, self.mask, self.act = [], [], []
        self.logp, self.val, self.rew, self.done = [], [], [], []
        # False on rows collected while the seat was dead (frozen-view
        # padding): they stay in the batch so GAE can flow the episode's
        # team payoff backward, but the update must not learn from them.
        self.valid: list[bool] = []
        self.last_pot = 0.0


class Job:
    """One worker's permanent role. Roles are fixed for the run — the
    lane geometry must never change, because episodes span many rollouts
    and a trajectory stream has to stay contiguous. What varies per
    episode is the detail: which tier, which past checkpoint."""

    def __init__(
        self,
        worker: Worker,
        kind: str,
        seat: int,
        pool_dir: pathlib.Path,
        rng: np.random.Generator,
        device: str,
        maps: str = "fixed",
    ) -> None:
        # seat: 0/1 for duel kinds; 0..3 for ffa.
        self.worker = worker
        self.kind = kind
        self.pool_dir = pool_dir
        self.rng = rng
        self.device = device
        self.maps = maps
        self.tier: str | None = None
        self.past: nn.Module | None = None
        self.frame: Frame | None = None
        self.conditions: dict[int, tuple[int, ...]] = {}
        # Team episodes truncate per seat: a dead learner's lane pads on
        # its frozen last view (zero reward, policy still queried so the
        # batch stays rectangular) until the episode really ends and the
        # team outcome pays every lane its truth. Padded rows are marked
        # invalid and masked out of the PPO update.
        self.dead: set[int] = set()
        self.last_views: dict[int, SeatView] = {}
        # Learner seats that stood a Fabricator at any point this
        # episode. Lives on the Job, not the Lane, because episodes span
        # rollout windows and Lanes are recreated per window.
        self.salvaged: set[int] = set()
        # Learner seats that picked each v6 weld verb this episode —
        # the --repair-bonus evidence, tracked exactly like salvaged.
        self.repaired: set[int] = set()
        self.built_bay: set[int] = set()
        if kind == "self":
            self.learner_seats = [0, 1]
            self.opp_seat = None
        elif kind == "team":
            # 2v2: the west column (seats 0 and 2 by the mapgen
            # convention) learns as one team against scripted tiers.
            self.learner_seats = [0, 2]
            self.opp_seat = None
        elif kind == "team2":
            # 2v2 beside a scripted ally: the learner holds one west
            # chair, a tier Brain drives its teammate (and both foes) —
            # the robustness half of team training, so the policy
            # learns to fight NEXT TO conventions it doesn't share.
            self.learner_seats = [seat * 2]  # 0 or 2, the west chairs
            self.opp_seat = None
        elif kind in ("tier", "ffa"):
            self.learner_seats = [seat]
            self.opp_seat = None
        else:  # past | rusher: both seats controlled, one driven locally
            self.learner_seats = [seat]
            self.opp_seat = 1 - seat

    @property
    def view(self) -> Frame:
        """The live frame; jobs are always reset before stepping."""
        if self.frame is None:
            raise RuntimeError("job stepped before reset")
        return self.frame

    def seat_view(self, seat: int) -> SeatView:
        """The seat's live view, or its frozen last one if the seat
        died while teammates play on."""
        live = self.view.seats.get(seat)
        if live is not None:
            self.last_views[seat] = live
            return live
        return self.last_views[seat]

    def reset(self, seed: int) -> None:
        TEL["resets"] += 1
        with timed("reset_sec"):
            self._reset(seed)

    def _reset(self, seed: int) -> None:
        self.dead = set()
        self.last_views = {}
        self.salvaged = set()
        self.repaired = set()
        self.built_bay = set()
        self.conditions = {s: sample_condition(self.rng, s) for s in self.learner_seats}
        scenario = None
        if self.maps == "random":
            scenario = generate(seed % 100_000, cache_dir("oxide-maps-train"))
        elif self.maps == "grand":
            # The pacing curriculum: 1v1 lanes on the big classes only,
            # where the shipped tens-of-minutes game lives. The ffa and
            # team arms below keep their own draws — four bases at vast
            # scale price the sim out of a laptop rollout.
            scenario = generate(
                seed % 100_000, cache_dir("oxide-maps-train-grand"), pace="grand"
            )
        if self.kind == "ffa":
            scenario = generate(
                seed % 100_000, cache_dir("oxide-maps-train4"), players=4
            )
            self.tier = TIERS[int(self.rng.integers(len(TIERS)))]
            self.frame = self.worker.reset(
                seed,
                control=(self.learner_seats[0],),
                tier=self.tier,
                conditions=self.conditions,
                scenario=scenario,
            )
            return
        if self.kind in ("team", "team2"):
            scenario = generate(
                seed % 100_000, cache_dir("oxide-maps-train2v2"), players=4, teams=True
            )
            self.tier = TIERS[int(self.rng.integers(len(TIERS)))]
            self.frame = self.worker.reset(
                seed,
                control=tuple(self.learner_seats),
                tier=self.tier,
                conditions=self.conditions,
                scenario=scenario,
            )
            return
        if self.kind == "tier":
            self.tier = TIERS[int(self.rng.integers(len(TIERS)))]
            self.frame = self.worker.reset(
                seed,
                control=(self.learner_seats[0],),
                tier=self.tier or "veteran",
                conditions=self.conditions,
                scenario=scenario,
            )
            return
        if self.kind == "past":
            pool = sorted(self.pool_dir.glob("ckpt-*.pt"))
            if pool:
                pick = pool[int(self.rng.integers(len(pool)))]
                past, _ = load_policy(str(pick), self.device)
                past.eval()
                self.past = past
            else:
                self.past = None  # empty pool: play the rusher instead
        all_conds = dict(self.conditions)
        if self.opp_seat is not None:
            # Frozen opponents play straight, under their honest faction.
            all_conds[self.opp_seat] = (1000, 500, faction_knob(self.opp_seat))
        self.frame = self.worker.reset(
            seed, control=(0, 1), conditions=all_conds, scenario=scenario
        )

    def opponent_action(self, policy_device: str) -> dict[int, int]:
        """Actions for locally-driven seats (empty for self/tier)."""
        if self.opp_seat is None:
            return {}
        view = self.view.seats[self.opp_seat]
        if self.kind == "rusher" or self.past is None:
            return {self.opp_seat: rusher(view.raw, view.mask, self.view.tick)}
        policy, device = self.past, policy_device
        with torch.no_grad():
            logits, _ = policy(
                torch.as_tensor(view.obs[None], device=device),
                torch.as_tensor(view.mask[None], device=device),
            )
            a = torch.distributions.Categorical(logits=logits).sample()
        return {self.opp_seat: int(a)}


def assign_roles(
    workers: list[Worker],
    mix: dict[str, float],
    pool_dir: pathlib.Path,
    rng: np.random.Generator,
    device: str,
    maps: str = "fixed",
) -> list[Job]:
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
    # One independent stream per job, spawned deterministically from
    # the master: with a SHARED generator, pipelined stepping reordered
    # draws whenever an episode reset interleaved differently than the
    # old serial loop, so seeded rollouts silently diverged. Split
    # streams make the draw order a per-job fact, immune to completion
    # order.
    streams = rng.spawn(len(workers))
    for kind, count in zip(kinds, counts, strict=False):
        # team2 alternates its single learner between the two west
        # chairs (k % 2 -> seat 0 or 2 inside the Job), everything else
        # keeps its established seat arithmetic.
        seats = 4 if kind in ("ffa", "team") else 2
        for k in range(count):
            jobs.append(
                Job(workers[i], kind, k % seats, pool_dir, streams[i], device, maps)
            )
            i += 1
    return jobs


def rollout(
    policy: nn.Module,
    jobs: list[Job],
    seeds: Iterator[int],
    steps: int,
    device: str,
    tech_bonus: float = 0.0,
    mix_bonus: float = 0.0,
    salvage_bonus: float = 0.0,
    repair_bonus: float = 0.0,
) -> tuple[tuple[np.ndarray, ...], np.ndarray, list[float]]:
    lanes = {(id(j), s): Lane(j.worker, s) for j in jobs for s in j.learner_seats}
    finished_rewards = []
    for j in jobs:
        if j.frame is None:
            j.reset(next(seeds))
    for j in jobs:
        for s in j.learner_seats:
            lanes[(id(j), s)].last_pot = potential(j.seat_view(s).raw)

    for _ in range(steps):
        views = []
        keys = []
        live = []
        for j in jobs:
            for s in j.learner_seats:
                v = j.seat_view(s)
                views.append(v)
                keys.append((id(j), s))
                live.append(s not in j.dead)
        obs = np.stack([v.obs for v in views])
        mask = np.stack([v.mask for v in views])
        with timed("policy_sec"), torch.no_grad():
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
            lane.valid.append(live[k])

        row = {key: k for k, key in enumerate(keys)}
        # Pipelined env step: every job's actions — opponent minds
        # included — are computed before any worker hears from us, then
        # all sends go out, then replies collect in the same
        # deterministic job order. Eight simulations advance
        # concurrently instead of one at a time; the batch is
        # bit-identical to the serial loop because nothing about a
        # job's step depends on another job's reply.
        all_acts = []
        for j in jobs:
            acts = {}
            for s in j.learner_seats:
                if s in j.dead:
                    continue  # a frozen lane sends nothing to the sim
                k = row[(id(j), s)]
                acts[s] = maybe_blunder(
                    int(action[k]),
                    logits_np[k],
                    mask[k],
                    j.conditions[s][0],
                    j.rng,
                )
                if acts[s] == SALVAGE_ACTION:
                    j.salvaged.add(s)
                elif acts[s] == REPAIR_ACTION:
                    j.repaired.add(s)
                elif acts[s] == BUILD_BAY_ACTION:
                    j.built_bay.add(s)
            acts.update(j.opponent_action(device))
            all_acts.append(acts)
        with timed("env_sec"):
            for j, acts in zip(jobs, all_acts, strict=True):
                j.worker.send_step(acts)
        for j in jobs:
            with timed("env_sec"):
                frame = j.worker.recv()
            if frame.done:
                # v5: the terminal frame carries observations for
                # living seats. Install it as the live frame BEFORE any
                # bonus reads a view, so tech and mix pay off the true
                # final position — a dead seat, absent from the
                # terminal seats, keeps its frozen last view.
                j.frame = frame
                for s in j.learner_seats:
                    lane = lanes[(id(j), s)]
                    # The shaping rides only the training reward;
                    # finished_rewards stays the pure game outcome so
                    # avg_final telemetry compares across runs. The
                    # tech bonus pays the TERMINAL frame's fab_built —
                    # a Fabricator lost (or sold) by the end earns
                    # nothing, unlike the old sticky flag.
                    teched = j.seat_view(s).raw[FAB_BUILT] > 0
                    mut_bonus = tech_bonus if teched else 0.0
                    if teched:
                        TEL["ep_teched"] += 1
                    if s in j.salvaged:
                        TEL["ep_salvage"] += 1
                        # Same instrument as the tech bonus, same
                        # rules: own-state evidence (the seat's own
                        # picked action), annealed to zero so the true
                        # objective decides whether the verb survives.
                        mut_bonus += salvage_bonus
                    # One flag seeds both v6 weld verbs, each paying
                    # independently — a policy that found only the Bay
                    # (or only the field weld) still gets that verb
                    # seeded, and the anneal hands both back to the
                    # game's own economics.
                    if s in j.repaired:
                        TEL["ep_repair"] += 1
                        mut_bonus += repair_bonus
                    if s in j.built_bay:
                        TEL["ep_bay"] += 1
                        mut_bonus += repair_bonus
                    if mix_bonus > 0.0:
                        # The seat's frozen last view carries its final
                        # army; two bits (a real three-way mix) earns
                        # the full bonus.
                        h = comp_entropy(j.seat_view(s).raw)
                        TEL["mix_ent"] += h
                        mut_bonus += mix_bonus * min(h, 2.0) / 2.0
                    # Close the potential on the true final position:
                    # without this delta, a salvage (or an army loss)
                    # landing on the terminal cadence escaped the
                    # shaping entirely — the anti-salvage
                    # building-value term never priced the final step.
                    # A dead lane's frozen view prices a zero delta by
                    # construction (last_pot froze with it).
                    final_pot = potential(j.seat_view(s).raw)
                    shape = SHAPE_K * (final_pot - lane.last_pot)
                    lane.rew.append(frame.reward(s) + mut_bonus + shape)
                    lane.done.append(True)
                    finished_rewards.append(frame.reward(s))
                j.reset(next(seeds))
                for s in j.learner_seats:
                    lanes[(id(j), s)].last_pot = potential(j.seat_view(s).raw)
            else:
                for s in j.learner_seats:
                    lane = lanes[(id(j), s)]
                    if s not in frame.seats:
                        # Died this step (or earlier): the lane pads at
                        # zero until the team's episode resolves.
                        j.dead.add(s)
                        lane.rew.append(0.0)
                        lane.done.append(False)
                        continue
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
            views.append(j.seat_view(s))
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
        np.stack([np.stack(lane.obs) for lane in ordered], axis=1),
        np.stack([np.stack(lane.mask) for lane in ordered], axis=1),
        np.stack([np.asarray(lane.act) for lane in ordered], axis=1),
        np.stack([np.asarray(lane.logp, dtype=np.float32) for lane in ordered], axis=1),
        np.stack([np.asarray(lane.val, dtype=np.float32) for lane in ordered], axis=1),
        np.stack([np.asarray(lane.rew, dtype=np.float32) for lane in ordered], axis=1),
        np.stack([np.asarray(lane.done) for lane in ordered], axis=1),
        np.stack([np.asarray(lane.valid) for lane in ordered], axis=1),
    )
    return batch, last_val, finished_rewards


def evaluate(
    policy: nn.Module,
    workers: list[Worker],
    device: str,
    opponent: str,
    seeds: Iterable[int] | None = None,
) -> float:
    """Greedy, fixed suite, both seats per seed. `opponent` is a tier
    name or 'rusher'."""
    seeds = range(1000, 1010) if seeds is None else seeds
    wins = games = 0
    jobs = [(seed, seat) for seed in seeds for seat in (0, 1)]
    for start in range(0, len(jobs), len(workers)):
        chunk = jobs[start : start + len(workers)]
        live = []
        for i, (seed, seat) in enumerate(chunk):
            w = workers[i]
            straight: dict[int, tuple[int, ...]] = {
                s: (1000, 500, faction_knob(s)) for s in (0, 1)
            }
            if opponent == "rusher":
                frame = w.reset(seed, control=(0, 1), conditions=straight)
            else:
                frame = w.reset(
                    seed, control=(seat,), tier=opponent, conditions=straight
                )
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
            # Send-all, collect-in-order: the eval bracket's games are
            # independent, so the workers may as well all be simulating.
            sends = []
            for k, (i, seat, frame) in enumerate(live):
                acts = {seat: int(action[k])}
                if opponent == "rusher":
                    ov = frame.seats[1 - seat]
                    acts[1 - seat] = rusher(ov.raw, ov.mask, frame.tick)
                sends.append((i, seat, acts))
            for i, _seat, acts in sends:
                workers[i].send_step(acts)
            for i, seat, _acts in sends:
                nxt = workers[i].recv()
                if nxt.done:
                    games += 1
                    wins += 1 if nxt.winner == seat else 0
                else:
                    still.append((i, seat, nxt))
            live = still
    return wins / games if games else 0.0


def probe_canary(payload: dict) -> dict:
    """One canary row from a `balance-probe --out` payload:
    decisiveness, the decided cohort's mix entropy with its per-seat
    p10, and unit AND building shares — read beside the rusher eval in
    the run log. Observed, never rewarded: nothing here may feed a
    loss term, or the probe stops being a measurement.

    Judgment reads the DECIDED cohort like the fun gate does — a
    stalemate's army mix is evidence about a stalemate — while the
    decided/capped counts keep the decisiveness reading itself."""
    overall, decided = payload["overall"], payload["decided"]
    spread = decided.get("seat_entropy")
    return {
        "matches": overall["matches"],
        "decided": overall["decided"],
        "capped": overall["capped"],
        "entropy_bits": round(decided["entropy_bits"], 2),
        "seat_p10": round(spread["p10"], 2) if spread else None,
        "unit_share": {k: round(v, 3) for k, v in decided["mean_share"].items()},
        "building_share": {
            k: round(v, 3) for k, v in decided["seats_with_building"].items()
        },
    }


# The `--out` payload schema this loop reads; below it the decided
# cohort does not exist (same seam fun_gate.py pins).
PROBE_SCHEMA = 2


def composition_probe(
    policy: nn.Module,
    arch: str,
    update: int,
    run_dir: pathlib.Path,
    driver: str,
    scenarios: str,
    level: str,
    seeds: int,
) -> dict:
    """Snapshots the current policy and runs the enriched composition
    probe against the anchor slate (the shipped maps): checkpoint ->
    Q12 export -> `driver balance-probe --weights` — the fun gate's
    instrument played in-loop, so composition collapse shows up beside
    the rusher canary instead of after the campaign. The snapshot,
    artifact, and raw payload all land under runs/<name>/probe/ for
    post-hoc reading."""
    probe_dir = run_dir / "probe"
    probe_dir.mkdir(parents=True, exist_ok=True)
    ckpt = probe_dir / f"ckpt-{update:05d}.pt"
    save_policy(policy, arch, ckpt, {"gym_version": GYM_VERSION, "update": update})
    weights = probe_dir / f"weights-{update:05d}.json"
    export(str(ckpt), str(weights))
    out = probe_dir / f"probe-{update:05d}.json"
    subprocess.run(
        [
            driver,
            "balance-probe",
            "--dir",
            scenarios,
            "--level",
            level,
            "--seeds",
            str(seeds),
            "--weights",
            str(weights),
            "--out",
            str(out),
        ],
        check=True,
        capture_output=True,
    )
    payload = json.loads(out.read_text())
    schema = payload.get("schema", 1)
    if schema < PROBE_SCHEMA:
        raise RuntimeError(
            f"probe payload is schema {schema}, this loop reads {PROBE_SCHEMA} "
            "— rebuild the driver"
        )
    return probe_canary(payload)


def main() -> None:
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
        "--anchor", default="runs/bc.pt", help="KL anchor prior ('' disables)"
    )
    ap.add_argument("--anchor-coef", type=float, default=0.05)
    ap.add_argument(
        "--anchor-decay",
        type=unit_interval,
        default=0.995,
        help="per-update anchor decay; 1.0 holds the anchor constant "
        "(style retention for the whole run — the round-3 lesson: a "
        "decayed anchor lets PPO grind imitation-taught tech back out)",
    )
    ap.add_argument(
        "--tech-bonus",
        type=float,
        default=0.0,
        help="terminal bonus paid when the episode ever stood a "
        "Fabricator (own-state only, fog-safe); annealed linearly to "
        "zero across --tech-anneal updates. 0 disables.",
    )
    ap.add_argument(
        "--tech-anneal",
        type=int,
        default=0,
        help="updates from this run's start until --tech-bonus reaches "
        "zero (0 = the full --updates span)",
    )
    ap.add_argument(
        "--salvage-bonus",
        type=float,
        default=0.0,
        help="terminal bonus paid when the seat picked Salvage this "
        "episode (own-state only, fog-safe); annealed on the "
        "--tech-anneal schedule. 0 disables.",
    )
    ap.add_argument(
        "--repair-bonus",
        type=float,
        default=0.0,
        help="terminal bonus paid per v6 weld verb the seat picked this "
        "episode (RepairUnit and Build RepairBay each earn it once; "
        "own-state only, fog-safe); annealed on the --tech-anneal "
        "schedule. 0 disables.",
    )
    ap.add_argument(
        "--mix-bonus",
        type=float,
        default=0.0,
        help="terminal bonus scaled by the seat's OWN cost-weighted "
        "army-mix entropy (fog-safe; 2 bits earns the full bonus); "
        "annealed on the same schedule as --tech-bonus. 0 disables.",
    )
    ap.add_argument(
        "--maps",
        default="fixed",
        help="fixed | random (fresh map per episode) | grand (fresh map "
        "per episode, 1v1 lanes drawn from the large/vast classes only "
        "— the pacing curriculum)",
    )
    ap.add_argument(
        "--mix",
        default="self=0.45,past=0.20,tier=0.20,rusher=0.15",
        help="opponent kind weights",
    )
    ap.add_argument(
        "--probe-every",
        type=int,
        default=100,
        help="run the composition probe on the current checkpoint every "
        "N updates and log its canary row (0 disables)",
    )
    ap.add_argument(
        "--probe-dir",
        default="../../scenarios",
        help="the anchor slate the composition probe fights across",
    )
    ap.add_argument("--probe-level", default="medium")
    ap.add_argument("--probe-seeds", type=int, default=2)
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

    # A one-cell cursor the warmer reads without locking: worst case it
    # warms a seed twice, and generate() is idempotent per seed.
    consumed = [50_000]

    def seed_stream() -> Iterator[int]:
        s = 50_000
        while True:
            consumed[0] = s
            yield s
            s += 1

    seeds = seed_stream()

    if args.maps in ("random", "grand"):
        # Cold-cache map generation costs a driver subprocess per map
        # (~34% of an update when the cache is empty). A daemon warmer
        # stays a few seeds ahead of the cursor so the hot path only
        # ever sees cache hits; generate() is atomic-rename safe, so
        # the race with a foreground miss is harmless. Determinism is
        # untouched: same seed, same file, whoever writes it.
        def warm() -> None:
            warmed = 0
            while True:
                target = consumed[0] + 1 + 2 * args.workers
                while warmed < target:
                    warmed = max(warmed, consumed[0] + 1)
                    if args.maps == "grand":
                        _generate(
                            warmed % 100_000,
                            cache_dir("oxide-maps-train-grand"),
                            pace="grand",
                        )
                    else:
                        _generate(warmed % 100_000, cache_dir("oxide-maps-train"))
                    _generate(
                        warmed % 100_000, cache_dir("oxide-maps-train4"), players=4
                    )
                    _generate(
                        warmed % 100_000,
                        cache_dir("oxide-maps-train2v2"),
                        players=4,
                        teams=True,
                    )
                    warmed += 1
                time.sleep(0.25)

        threading.Thread(target=warm, daemon=True, name="map-warmer").start()
    log = (run_dir / "log.jsonl").open("a")

    try:
        jobs = assign_roles(workers, mix, pool_dir, rng, device, args.maps)
        for update in range(start_update + 1, start_update + args.updates + 1):
            t0 = time.time()
            TEL.clear()
            # The anneal runs on THIS run's clock, not the absolute one:
            # a resumed consolidation wants its exploration push at its
            # own start, wherever the parent's clock stands.
            tb = tech_bonus_at(
                args.tech_bonus,
                update - start_update - 1,
                args.tech_anneal or args.updates,
            )
            mb = tech_bonus_at(
                args.mix_bonus,
                update - start_update - 1,
                args.tech_anneal or args.updates,
            )
            sb = tech_bonus_at(
                args.salvage_bonus,
                update - start_update - 1,
                args.tech_anneal or args.updates,
            )
            rb = tech_bonus_at(
                args.repair_bonus,
                update - start_update - 1,
                args.tech_anneal or args.updates,
            )
            batch, last_val, finals = rollout(
                policy,
                jobs,
                seeds,
                args.steps,
                device,
                tech_bonus=tb,
                mix_bonus=mb,
                salvage_bonus=sb,
                repair_bonus=rb,
            )
            rollout_sec = time.time() - t0
            obs_b, mask_b, act_b, logp_b, val_b, rew_b, done_b, valid_b = batch
            adv, ret = gae(rew_b, done_b, val_b, last_val)
            # GAE ran over the full rectangle so a dead teammate's lane
            # still carries the episode's team payoff backward; the
            # frozen-view padding rows themselves train nothing.
            rows = valid_b.reshape(-1)
            flat = (
                obs_b.reshape(-1, NET_FEATURES)[rows],
                mask_b.reshape(-1, ACTIONS)[rows],
                act_b.reshape(-1)[rows],
                logp_b.reshape(-1)[rows],
                adv.reshape(-1)[rows],
                ret.reshape(-1)[rows],
            )
            # The anchor is scaffolding: essential while the policy is a
            # fragile clone, a straitjacket once the league is teaching —
            # and it pins every knob setting to the teacher's one style.
            # Anneal it away (halves roughly every 140 updates).
            t_update = time.time()
            stats = ppo_update(
                policy,
                opt,
                flat,
                device,
                value_only=update <= args.value_warmup,
                anchor=anchor,
                anchor_coef=args.anchor_coef * (args.anchor_decay**update),
            )
            decisions = int(obs_b.shape[0]) * int(obs_b.shape[1])
            entry = {
                "update": update,
                "kinds": sorted(j.kind for j in jobs),
                "episodes": len(finals),
                "avg_final": round(float(np.mean(finals)), 3) if finals else None,
                "ent": round(stats["ent"] / max(stats["batches"], 1), 3),
                "kl": round(stats["kl"], 4),
                "sec": round(time.time() - t0, 1),
                # The phase clocks: where an update's wall time actually
                # went, so optimization is measurement, not folklore.
                "rollout_sec": round(rollout_sec, 2),
                "update_sec": round(time.time() - t_update, 2),
                "decisions_s": round(decisions / max(rollout_sec, 1e-9)),
                **{
                    k: (
                        int(v)
                        if k
                        in ("resets", "ep_teched", "ep_salvage", "ep_repair", "ep_bay")
                        else round(v, 2)
                    )
                    for k, v in sorted(TEL.items())
                },
            }
            if args.tech_bonus:
                entry["tech_bonus"] = round(tb, 4)
            if args.salvage_bonus:
                entry["salvage_bonus"] = round(sb, 4)
            if args.repair_bonus:
                entry["repair_bonus"] = round(rb, 4)
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
            if args.probe_every and update % args.probe_every == 0:
                t_probe = time.time()
                try:
                    entry["probe"] = composition_probe(
                        policy,
                        arch,
                        update,
                        run_dir,
                        args.driver,
                        args.probe_dir,
                        args.probe_level,
                        args.probe_seeds,
                    )
                except (
                    subprocess.CalledProcessError,
                    OSError,
                    ValueError,
                    KeyError,
                    RuntimeError,
                ) as e:
                    # A broken canary is a log line, not a dead campaign.
                    entry["probe_error"] = str(e)
                entry["probe_sec"] = round(time.time() - t_probe, 1)
            print(json.dumps(entry), flush=True)
            log.write(json.dumps(entry) + "\n")
            log.flush()
    finally:
        for w in workers:
            w.close()


if __name__ == "__main__":
    main()
