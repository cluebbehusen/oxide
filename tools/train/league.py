"""League PPO: self-play with anchors.

Pure self-play drifts into self-referential conventions; pure
scripted-opponent play overfits one opponent. The league mixes both,
AlphaStar-fashion, at laptop scale: every rollout each worker is dealt
an opponent kind —

  self    both seats driven by the current policy (both trajectories
          train — the arms race lives here)
  past    a frozen checkpoint from this run's pool (stops cycling:
          you must still beat who you used to be)
  overseer
          the scripted Overseer commander (the anchor that keeps play
          grounded against a sensible opponent, and the yardstick)
  rusher  the scripted rush teacher (the known exploit, kept in the
          curriculum forever so the answer to it never fades)

Guards from the first collapsed run: value warm-up before the policy
moves, a KL early stop each update, conservative learning rate.
``--production-entropy-coef`` can add exploration pressure to production
without changing the equal-head ``--entropy-coef``.

Usage (from tools/train/):
    uv run league.py --name league1 --initialize-from runs/bc.pt --updates 2000
    uv run league.py --name phase2 \
        --initialize-from runs/league1/latest.pt --updates 2000

``--initialize-from`` starts a new training phase from an actor/critic
checkpoint. It inherits the checkpoint's update number for lineage and
pool numbering, but deliberately starts fresh optimizer, RNG, episode
state, and annealing clocks. ``--resume`` remains a deprecated
compatibility spelling for that same weights-only initialization; it
is not exact interruption recovery.

Every invocation owns a fresh ``runs/<name>`` directory. A non-empty
directory is refused before workers launch or any run file is opened;
``--initialize-from`` reads its parent checkpoint but still requires a
new ``--name`` for the new phase.
"""

import argparse
import contextlib
import json
import pathlib
import subprocess
import sys
import threading
import time
from collections import Counter
from dataclasses import dataclass
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from collections.abc import Iterable, Iterator

import numpy as np
import torch
from torch import nn

from export import export
from fun_gate import cap_health
from lineage import build_lineage, checkpoint_metadata, input_identity
from mapgen import DRIVER as MAPGEN_DRIVER
from mapgen import cache_dir
from mapgen import generate as _generate
from models import (
    checkpoint_critic_ready,
    factorized_greedy,
    factorized_joint_log_prob,
    factorized_sample,
    load_policy,
    make_policy,
    save_policy,
)
from oxide_gym import (
    ACTION_HEADS,
    ACTIONS,
    CADENCE,
    CONDITION_NAMES,
    FEATURE_NAMES,
    GYM_VERSION,
    NET_FEATURES,
    ActionPlan,
    CanonicalProfile,
    Frame,
    ProfileCatalog,
    SeatView,
    Worker,
    condition_from_profile,
    policy_skill_for_aggression,
)
from ppo import TRAIN_GAMMA, gae, ppo_update

MAP_FAMILIES = ("fixed", "random", "grand")
FACTION_PAIRS = ("ff", "fc", "cf", "cc")
OPPONENT_KINDS = (
    "self",
    "past",
    "overseer",
    "rusher",
    "ffa",
    "team",
    "team2",
)
type AggressionDistribution = tuple[tuple[int, int, float], ...]

TRAIN_DIR = pathlib.Path(__file__).resolve().parent
REPO_ROOT = TRAIN_DIR.parents[1]
FIXED_SCENARIO_PATH = REPO_ROOT / "scenarios" / "skirmish.json"
MAP_GENERATOR_PATH = TRAIN_DIR / "mapgen.py"
TRAIN_ENVIRONMENT_PATH = TRAIN_DIR / "uv.lock"
LEAGUE_TRAINER_PATH = TRAIN_DIR / "league.py"
MODEL_CODE_PATH = TRAIN_DIR / "models.py"
PPO_CODE_PATH = TRAIN_DIR / "ppo.py"
GYM_CLIENT_PATH = TRAIN_DIR / "oxide_gym.py"
PROFILE_CONDITION_START = CONDITION_NAMES.index("profile_economy")
PROFILE_CONDITION_COUNT = len(CONDITION_NAMES) - PROFILE_CONDITION_START
PROFILE_CONDITION_NAMES = CONDITION_NAMES[PROFILE_CONDITION_START:]


@dataclass(frozen=True)
class ExecutionProfile:
    """One shipped difficulty's environment-side handicap."""

    name: str
    hesitation_permille: int
    cadence: int


@dataclass(frozen=True)
class EpisodeDials:
    """The independently selected policy and execution knobs for one seat."""

    policy_skill: int
    aggression: int
    execution: ExecutionProfile
    style: str | None = None
    variant: int | None = None
    role: str | None = None

    def condition(
        self,
        faction: int,
        catalog: ProfileCatalog | None = None,
    ) -> tuple[int, ...]:
        """Builds the policy condition without coupling it to execution."""
        if faction not in (0, 1000):
            raise ValueError(f"faction must be 0 or 1000, got {faction}")
        if self.style is not None:
            if catalog is None or self.variant is None or self.role is None:
                raise RuntimeError(
                    "named episode dials require the Rust profile catalog"
                )
            faction_name = "cupric" if faction == 1000 else "ferrous"
            return catalog.condition(
                self.style,
                self.variant,
                self.role,
                faction_name,
            )
        return condition_from_profile(self.policy_skill, self.aggression, faction)


SHIPPED_EXECUTION_PROFILES = (
    ExecutionProfile("easy", 350, 56),
    ExecutionProfile("medium", 190, 36),
    ExecutionProfile("hard", 5, 34),
    ExecutionProfile("expert", 0, 37),
)
SHIPPED_AGGRESSION_DISTRIBUTION: AggressionDistribution = (
    (250, 399, 0.6),
    (500, 600, 0.4),
)


class ProfileCurriculum:
    """Seeded policy profiles crossed independently with execution.

    With a v8 Rust catalog, the default block is every authored named
    variant crossed with all four execution profiles. Custom aggression
    distributions remain raw, zero-facet experiments with their own
    four-cell execution cycle.
    """

    def __init__(
        self,
        rng: np.random.Generator,
        aggression_distribution: AggressionDistribution,
        profile_catalog: ProfileCatalog | None = None,
        *,
        use_named_profiles: bool | None = None,
    ) -> None:
        self.rng = rng
        self.aggression_distribution = aggression_distribution
        self.profile_catalog = profile_catalog
        self._named = (
            aggression_distribution == SHIPPED_AGGRESSION_DISTRIBUTION
            if use_named_profiles is None
            else use_named_profiles
        )
        self._cells: list[tuple[CanonicalProfile, ExecutionProfile]] = []
        self._execution: list[ExecutionProfile] = []

    def _refill_default(self) -> None:
        catalog = self.profile_catalog
        if catalog is None:
            raise RuntimeError("the named curriculum requires a Rust profile catalog")
        cells = [
            (profile, execution)
            for execution in SHIPPED_EXECUTION_PROFILES
            for profile in catalog.profiles
        ]
        order = self.rng.permutation(len(cells))
        self._cells = [cells[int(index)] for index in reversed(order)]

    def _next_execution(self) -> ExecutionProfile:
        if not self._execution:
            order = self.rng.permutation(len(SHIPPED_EXECUTION_PROFILES))
            self._execution = [
                SHIPPED_EXECUTION_PROFILES[int(index)] for index in reversed(order)
            ]
        return self._execution.pop()

    def sample(
        self,
        factions: dict[int, int],
        *,
        specialize_roles: bool = False,
    ) -> dict[int, EpisodeDials]:
        """Samples one job-level execution profile and per-seat policy dials."""
        if self._named:
            if not self._cells:
                self._refill_default()
            catalog = self.profile_catalog
            if catalog is None:
                raise RuntimeError(
                    "the named curriculum requires a Rust profile catalog"
                )
            profile, execution = self._cells.pop()
            roles = [catalog.default_role] * len(factions)
            if specialize_roles:
                specialist_roles = [
                    role for role in catalog.team_roles if role != catalog.default_role
                ]
                if not specialist_roles:
                    raise RuntimeError(
                        "the Rust profile catalog has no specialist team roles"
                    )
                order = self.rng.permutation(len(specialist_roles))
                roles = [
                    specialist_roles[int(order[index % len(order)])]
                    for index in range(len(factions))
                ]
            return {
                seat: EpisodeDials(
                    policy_skill_for_aggression(profile.aggression),
                    profile.aggression,
                    execution,
                    profile.style,
                    profile.variant,
                    roles[index],
                )
                for index, seat in enumerate(factions)
            }

        execution = self._next_execution()
        return {
            seat: EpisodeDials(
                policy_skill_for_aggression(
                    aggression := sample_aggression(
                        self.rng,
                        self.aggression_distribution,
                    )
                ),
                aggression,
                execution,
            )
            for seat in factions
        }


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
    driver: str = MAPGEN_DRIVER,
    pace: str | None = None,
) -> str:
    """mapgen.generate with its wall time metered."""
    with timed("mapgen_sec"):
        return _generate(
            seed,
            out_dir,
            players=players,
            teams=teams,
            driver=driver,
            pace=pace,
        )


# Potential-based shaping: a small dense signal that guides the value
# net through the thousand-decision desert between terminal rewards.
# It prices conserved OWN material, independent of what form the policy
# chose. Enemy knowledge never enters: under fog, "known enemy value" is
# an information artifact that rewards staying blind.
SHAPE_K = 0.05
SHAPE_GAMMA = TRAIN_GAMMA

# Style shaping is disabled by default. When explicitly enabled, the
# episode's mean posture alignment becomes ONE terminal adjustment,
# capped below the win/loss reward. The retired per-step reward could
# accumulate +6.25 by the tick cap and directly paid a turtle to stall.
MAX_STYLE_BONUS = 0.1
MAX_ENTROPY_COEFFICIENT = 0.1
MAX_EPISODE_DECISIONS = 40_000 // CADENCE
DEFAULT_VALUE_WARMUP = 15


F = {name: i for i, name in enumerate(FEATURE_NAMES)}
C = {name: i for i, name in enumerate(CONDITION_NAMES)}


def potential(raw: list[int]) -> float:
    # Scrap stays worth scrap while moving bank -> queue/site -> standing
    # asset. HP discounts damage for EVERY unit and purchasable building,
    # so AA, air-superiority, defenses, and repairs are no longer scored
    # through ground DPS. Carried scrap closes the extraction->deposit
    # gap. This potential supplies credit, not a composition preference.
    owned = (
        raw[F["scrap"]]
        + raw[F["carried_scrap"]]
        + raw[F["queued_unit_value"]]
        + raw[F["construction_site_value"]]
        + raw[F["my_unit_health_value"]]
        + raw[F["my_building_health_value"]]
    )
    return owned / 500.0


def aggression_alignment(raw: list[int], aggression: int) -> float:
    """Signed family-level posture agreement in [-1, 1]."""
    lean = (aggression - 500) / 500.0
    out_fighting = 1.0 if raw[F["army_state"]] in (2, 3) else -1.0
    return max(-1.0, min(1.0, lean * out_fighting))


def _ratio_posture(part: int, total: int) -> float:
    """Maps a 0..50% composition share onto -1..1."""
    if total <= 0:
        return -1.0
    return max(-1.0, min(1.0, 4.0 * part / total - 1.0))


def style_alignment(
    raw: list[int],
    condition: int | tuple[int, ...] | list[int],
) -> float:
    """Signed own-state agreement with a raw or named profile.

    Raw-aggression experiments retain the old commitment-only signal.
    Named profiles align the five Rust-authored facets with productive
    economy, air and siege composition, support infrastructure, and
    army commitment. The terminal mean stays capped by ``style_bonus``;
    these own-state signals teach the new inputs without paying an action
    quota or letting a long turtle game accumulate more reward.
    """
    if isinstance(condition, int):
        return aggression_alignment(raw, condition)
    if len(condition) != len(CONDITION_NAMES):
        raise ValueError(
            f"profile condition has {len(condition)} values, "
            f"expected {len(CONDITION_NAMES)}"
        )
    facets = condition[C["profile_economy"] :]
    if not any(facets):
        return aggression_alignment(raw, condition[C["aggression"]])

    harvesters = raw[F["my_harvesters"]]
    reclaimers = raw[F["my_reclaimers_built"]]
    economy = max(
        -1.0, min(1.0, 2.0 * min((harvesters + 2 * reclaimers) / 8, 1.0) - 1.0)
    )

    combat = sum(
        raw[F[name]]
        for name in (
            "my_sentinels",
            "my_scuttlers",
            "my_lancers",
            "my_bombards",
            "my_antiair",
            "my_airground",
            "my_airair",
        )
    )
    air = _ratio_posture(raw[F["my_airground"]] + raw[F["my_airair"]], combat)
    siege = _ratio_posture(raw[F["my_lancers"]] + raw[F["my_bombards"]], combat)
    support_count = sum(
        raw[F[name]]
        for name in (
            "my_turrets_built",
            "my_flak_built",
            "my_arrays_built",
            "my_bastions_built",
            "my_repair_bays_built",
        )
    )
    support = 2.0 * min(support_count / 4, 1.0) - 1.0
    commitment = 1.0 if raw[F["army_state"]] in (2, 3) else -1.0

    postures = (economy, air, siege, support, commitment)
    leans = tuple((value - 500) / 500.0 for value in facets)
    weight = sum(abs(lean) for lean in leans)
    if weight == 0.0:
        return 0.0
    return max(
        -1.0,
        min(
            1.0,
            sum(lean * posture for lean, posture in zip(leans, postures, strict=True))
            / weight,
        ),
    )


def style_bonus(total: float, steps: int, coefficient: float) -> float:
    """A duration-invariant terminal style adjustment capped at 0.1."""
    if steps <= 0 or coefficient == 0.0:
        return 0.0
    mean = max(-1.0, min(1.0, total / steps))
    return max(
        -MAX_STYLE_BONUS,
        min(MAX_STYLE_BONUS, coefficient * mean),
    )


FAB_BUILT = FEATURE_NAMES.index("fab_built")
# Action index the gym assigns Salvage (v5's appended verb).
SALVAGE_ACTION = 21
BUILD_TURRET_ACTION = 10
BUILD_ARRAY_ACTION = 13
REPAIR_ACTION = 22
BUILD_BAY_ACTION = 23
# Successful completions seeded by --structure-bonus. Fabricators have
# their established tech bonus, Foundries are not constructible, and the
# Repair Bay belongs to the repair bonus.
SEEDED_STRUCTURES = (
    "turret",
    "array",
)
MAX_STRUCTURE_KIND_BONUS = 0.02
MAX_RECLAIMER_BONUS = 0.02

# The agent's own army-count features with rough unit costs (varied
# roles use the midpoint of their two faction kinds) — the same
# cost-weighted combat lens the fun gate judges with. Harvesters are
# economy, not a free second army kind.
ARMY_FEATURES = [
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
    return entropy_from_weights([raw[i] * cost for i, cost in ARMY_FEATURES])


def entropy_from_weights(weights: list[float]) -> float:
    """Shannon entropy of non-negative composition weights."""
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


def reward_anneal_index(update: int, start_update: int, value_warmup: int) -> int:
    """Returns the zero-based actor-update clock for reward seeding.

    Critic-only warm-up learns the shaped return but cannot teach the actor,
    so it holds exploration seeds at their initial value. The anneal begins
    with the first update that is allowed to move the actor.
    """
    phase_update = update - start_update
    if phase_update <= 0:
        raise ValueError("reward annealing requires an update after phase start")
    return max(phase_update - value_warmup - 1, 0)


def value_warmup_active(update: int, start_update: int, warmup: int) -> bool:
    """Whether this update belongs to this invocation's critic warm-up."""
    relative = update - start_update
    return warmup > 0 and 1 <= relative <= warmup


def anchor_coefficient_at(
    base: float,
    decay: float,
    update: int,
    start_update: int,
) -> float:
    """Anneals a KL anchor on this training phase's clock.

    ``--initialize-from`` carries the parent's update number for artifact
    provenance, but starts a fresh optimizer and rollout phase. Applying the
    decay to that inherited absolute number would silently remove the anchor
    before the first update of a late checkpoint. The phase's first update
    uses ``base``; decay begins with the second.
    """
    relative = update - start_update
    if relative < 1:
        raise ValueError("anchor coefficient requires an update after phase start")
    return base * (decay ** (relative - 1))


def phase_interval_due(update: int, start_update: int, every: int) -> bool:
    """Whether this update lands on an interval in the current phase."""
    relative = update - start_update
    if relative < 1:
        raise ValueError("phase interval requires an update after phase start")
    return every > 0 and relative % every == 0


def non_negative_int(text: str) -> int:
    """Argparse type for counts that may be disabled with zero."""
    value = int(text)
    if value < 0:
        raise argparse.ArgumentTypeError(f"value must be non-negative, got {text}")
    return value


def add_initialization_arguments(ap: argparse.ArgumentParser) -> None:
    """Adds the mutually exclusive checkpoint-initialization options."""
    ap.add_argument(
        "--value-warmup",
        type=non_negative_int,
        default=None,
        metavar="UPDATES",
        help="value-only updates before PPO moves the actor (default: "
        f"{DEFAULT_VALUE_WARMUP} from scratch or an unready initialized "
        "critic, 0 for a ready initialized critic)",
    )
    initialization = ap.add_mutually_exclusive_group()
    initialization.add_argument(
        "--initialize-from",
        metavar="CHECKPOINT",
        help="start a new phase from checkpoint actor/critic weights; inherits "
        "the recorded update number but resets optimizer, RNG, and episodes",
    )
    initialization.add_argument(
        "--resume",
        metavar="CHECKPOINT",
        help="DEPRECATED alias for --initialize-from; this is a weights-only "
        "warm-start, not exact interruption recovery",
    )


def resolved_value_warmup(
    requested: int | None,
    initialized: bool,
    critic_ready: bool = True,
) -> int:
    """Resolves the context-sensitive warm-up default."""
    if requested is not None:
        if requested < 0:
            raise ValueError("value warm-up must be non-negative")
        return requested
    return 0 if initialized and critic_ready else DEFAULT_VALUE_WARMUP


def claim_fresh_run_directory(run_dir: pathlib.Path) -> pathlib.Path:
    """Claims one write-once training phase directory.

    An existing empty directory is a valid pre-created destination. Any
    existing content is a stale or concurrently owned phase and must be
    rejected without touching it. Creating ``pool`` is the claim, so a
    second process cannot silently append to the same phase.
    """
    try:
        run_dir.mkdir(parents=True, exist_ok=False)
    except FileExistsError as err:
        if not run_dir.is_dir():
            raise RuntimeError(f"run path is not a directory: {run_dir}") from err
        existing = sorted(path.name for path in run_dir.iterdir())
        if existing:
            preview = ", ".join(existing[:4])
            if len(existing) > 4:
                preview += ", ..."
            raise RuntimeError(
                f"run directory is not empty: {run_dir} ({preview}); "
                "choose a new --name for this training phase"
            ) from err

    pool_dir = run_dir / "pool"
    try:
        pool_dir.mkdir()
    except FileExistsError as err:
        raise RuntimeError(
            f"run directory was claimed concurrently: {run_dir}; "
            "choose a new --name for this training phase"
        ) from err
    return pool_dir


def unit_interval(text: str) -> float:
    """argparse type for decay factors: finite, in [0, 1]. A negative
    decay flips the KL sign on odd updates and actively rewards
    diverging from the anchor; nan poisons the loss silently."""
    value = float(text)
    if not np.isfinite(value) or not 0.0 <= value <= 1.0:
        raise argparse.ArgumentTypeError(f"decay must be finite in [0, 1], got {text}")
    return value


def bounded_entropy_coefficient(text: str) -> float:
    """Argparse type for PPO's exploration pressure."""
    value = float(text)
    if not np.isfinite(value) or not 0.0 <= value <= MAX_ENTROPY_COEFFICIENT:
        raise argparse.ArgumentTypeError(
            "entropy coefficient must be finite in "
            f"[0, {MAX_ENTROPY_COEFFICIENT}], got {text}"
        )
    return value


def add_entropy_arguments(ap: argparse.ArgumentParser) -> None:
    """Adds the equal-head and production-specific PPO entropy controls."""
    ap.add_argument(
        "--entropy-coef",
        type=bounded_entropy_coefficient,
        default=0.002,
        help=f"PPO equal-head entropy coefficient in [0, {MAX_ENTROPY_COEFFICIENT}]",
    )
    ap.add_argument(
        "--production-entropy-coef",
        type=bounded_entropy_coefficient,
        default=0.0,
        help="additional production-head entropy coefficient in "
        f"[0, {MAX_ENTROPY_COEFFICIENT}]; its effective head weight also "
        "includes one quarter of --entropy-coef (default: 0)",
    )


def effective_production_entropy_coefficient(
    entropy_coefficient: float,
    production_entropy_coefficient: float,
) -> float:
    """Returns the total coefficient multiplying production-head entropy."""
    return entropy_coefficient / len(ACTION_HEADS) + production_entropy_coefficient


def bounded_style_coefficient(text: str) -> float:
    """Argparse type for the episode-level style bonus ceiling."""
    value = float(text)
    if not np.isfinite(value) or not 0.0 <= value <= MAX_STYLE_BONUS:
        raise argparse.ArgumentTypeError(
            f"style coefficient must be finite in [0, {MAX_STYLE_BONUS}], got {text}"
        )
    return value


def bounded_structure_bonus(text: str) -> float:
    """Argparse type for the per-kind successful-structure seed."""
    value = float(text)
    if not np.isfinite(value) or not 0.0 <= value <= MAX_STRUCTURE_KIND_BONUS:
        raise argparse.ArgumentTypeError(
            "structure bonus must be finite in "
            f"[0, {MAX_STRUCTURE_KIND_BONUS}], got {text}"
        )
    return value


def bounded_reclaimer_bonus(text: str) -> float:
    """Argparse type for the successful-Reclaimer exploration seed."""
    value = float(text)
    if not np.isfinite(value) or not 0.0 <= value <= MAX_RECLAIMER_BONUS:
        raise argparse.ArgumentTypeError(
            f"reclaimer bonus must be finite in [0, {MAX_RECLAIMER_BONUS}], got {text}"
        )
    return value


def add_reclaimer_bonus_argument(parser: argparse.ArgumentParser) -> None:
    """Adds the completion-backed Reclaimer seed to a training CLI."""
    parser.add_argument(
        "--reclaimer-bonus",
        type=bounded_reclaimer_bonus,
        default=0.0,
        help="terminal bonus paid once when the seat completes a Reclaimer "
        "during the episode (own-state effect telemetry, fog-safe); capped "
        "at 0.02 and annealed on the --tech-anneal schedule. 0 disables.",
    )


def reward_lineage_hyperparameters(args: argparse.Namespace) -> dict[str, float]:
    """Returns every optional terminal reward dial consumed by rollout."""
    return {
        "mix_bonus": args.mix_bonus,
        "reclaimer_bonus": args.reclaimer_bonus,
        "repair_bonus": args.repair_bonus,
        "salvage_bonus": args.salvage_bonus,
        "structure_bonus": args.structure_bonus,
        "tech_bonus": args.tech_bonus,
    }


def parse_opponent_mix(text: str) -> dict[str, float]:
    """Parses opponent weights in one behaviorally stable order."""
    parsed: dict[str, float] = {}
    for item in text.split(","):
        try:
            kind, raw_weight = item.split("=", 1)
        except ValueError as err:
            raise argparse.ArgumentTypeError(
                "opponent mix must be KIND=WEIGHT pairs separated by commas"
            ) from err
        kind = kind.strip().lower()
        if kind not in OPPONENT_KINDS:
            allowed = ", ".join(OPPONENT_KINDS)
            raise argparse.ArgumentTypeError(
                f"unknown opponent kind {kind!r}; expected one of {allowed}"
            )
        if kind in parsed:
            raise argparse.ArgumentTypeError(f"opponent kind {kind!r} appears twice")
        try:
            weight = float(raw_weight)
        except ValueError as err:
            raise argparse.ArgumentTypeError(
                f"opponent weight for {kind!r} must be a number"
            ) from err
        if not np.isfinite(weight) or weight < 0.0:
            raise argparse.ArgumentTypeError(
                f"opponent weight for {kind!r} must be finite and non-negative"
            )
        parsed[kind] = weight
    total = sum(parsed.values())
    if total <= 0.0:
        raise argparse.ArgumentTypeError("opponent mix must have positive total weight")
    return {
        kind: parsed[kind] / total
        for kind in OPPONENT_KINDS
        if parsed.get(kind, 0.0) > 0.0
    }


def parse_map_mix(text: str) -> dict[str, float]:
    """Parses and normalizes weighted duel-map families.

    Zero-weight entries are ignored. Returning families in canonical
    order makes equivalent CLI strings produce the same seeded draws.
    """
    parsed: dict[str, float] = {}
    for item in text.split(","):
        try:
            name, raw_weight = item.split("=", 1)
        except ValueError as err:
            raise argparse.ArgumentTypeError(
                "map mix must be NAME=WEIGHT pairs separated by commas"
            ) from err
        name = name.strip()
        if name not in MAP_FAMILIES:
            allowed = ", ".join(MAP_FAMILIES)
            raise argparse.ArgumentTypeError(
                f"unknown map family {name!r}; expected one of {allowed}"
            )
        if name in parsed:
            raise argparse.ArgumentTypeError(f"map family {name!r} appears twice")
        try:
            weight = float(raw_weight)
        except ValueError as err:
            raise argparse.ArgumentTypeError(
                f"map weight for {name!r} must be a number"
            ) from err
        if not np.isfinite(weight) or weight < 0.0:
            raise argparse.ArgumentTypeError(
                f"map weight for {name!r} must be finite and non-negative"
            )
        parsed[name] = weight
    total = sum(parsed.values())
    if total <= 0.0:
        raise argparse.ArgumentTypeError("map mix must have positive total weight")
    return {
        family: parsed[family] / total
        for family in MAP_FAMILIES
        if parsed.get(family, 0.0) > 0.0
    }


def parse_faction_mix(text: str) -> dict[str, float]:
    """Parses and normalizes weighted two-seat faction pairings.

    Pairings are ordered west/east. Four-seat modes repeat the sampled
    pair in seat order, so ``cf`` becomes ``cfcf``. Canonical output
    order makes equivalent CLI strings spend the seeded stream alike.
    """
    parsed: dict[str, float] = {}
    for item in text.split(","):
        try:
            pair, raw_weight = item.split("=", 1)
        except ValueError as err:
            raise argparse.ArgumentTypeError(
                "faction mix must be PAIR=WEIGHT entries separated by commas"
            ) from err
        pair = pair.strip().lower()
        if pair not in FACTION_PAIRS:
            allowed = ", ".join(FACTION_PAIRS)
            raise argparse.ArgumentTypeError(
                f"unknown faction pair {pair!r}; expected one of {allowed}"
            )
        if pair in parsed:
            raise argparse.ArgumentTypeError(f"faction pair {pair!r} appears twice")
        try:
            weight = float(raw_weight)
        except ValueError as err:
            raise argparse.ArgumentTypeError(
                f"faction weight for {pair!r} must be a number"
            ) from err
        if not np.isfinite(weight) or weight < 0.0:
            raise argparse.ArgumentTypeError(
                f"faction weight for {pair!r} must be finite and non-negative"
            )
        parsed[pair] = weight
    total = sum(parsed.values())
    if total <= 0.0:
        raise argparse.ArgumentTypeError("faction mix must have positive total weight")
    return {
        pair: parsed[pair] / total
        for pair in FACTION_PAIRS
        if parsed.get(pair, 0.0) > 0.0
    }


def resolve_faction_mix(
    faction_mix: dict[str, float] | None,
) -> dict[str, float]:
    """Uses the authored Ferrous/Cupric convention when unspecified."""
    return faction_mix if faction_mix is not None else {"fc": 1.0}


def sample_faction_pair(
    rng: np.random.Generator,
    faction_mix: dict[str, float],
) -> str:
    """Draws one faction pairing from a job-local deterministic stream."""
    if len(faction_mix) == 1:
        # The default authored pairing predates faction randomization.
        # Do not perturb established seeded rollouts to choose a certainty.
        return next(iter(faction_mix))
    pairs = tuple(faction_mix)
    weights = np.asarray([faction_mix[pair] for pair in pairs], dtype=float)
    weights /= weights.sum()
    return str(rng.choice(pairs, p=weights))


def expand_faction_pair(pair: str, seats: int) -> str:
    """Expands a sampled duel pairing to the scenario's full seat order."""
    if pair not in FACTION_PAIRS:
        raise ValueError(f"unknown faction pair {pair!r}")
    if seats <= 0 or seats % 2 != 0:
        raise ValueError(
            f"faction pairs require a positive even seat count, got {seats}"
        )
    return pair * (seats // 2)


def resolve_map_mix(
    maps: str,
    map_mix: dict[str, float] | None,
) -> dict[str, float]:
    """Keeps the original ``--maps`` spelling as a one-family mix."""
    if map_mix is not None:
        return map_mix
    if maps not in MAP_FAMILIES:
        allowed = ", ".join(MAP_FAMILIES)
        raise ValueError(f"unknown map family {maps!r}; expected one of {allowed}")
    return {maps: 1.0}


def sample_map_family(
    rng: np.random.Generator,
    map_mix: dict[str, float],
) -> str:
    """Draws one duel map family from a job-local deterministic stream."""
    if len(map_mix) == 1:
        # Preserve the old --maps stream exactly: a one-family curriculum
        # never spent an RNG draw selecting what was already certain.
        return next(iter(map_mix))
    families = tuple(map_mix)
    weights = np.asarray([map_mix[family] for family in families], dtype=float)
    weights /= weights.sum()
    return str(rng.choice(families, p=weights))


def generated_map_families(
    map_mix: dict[str, float],
    opponent_mix: dict[str, float],
) -> tuple[str, ...]:
    """Generated cache families that active jobs can request."""
    families = [
        family for family in ("random", "grand") if map_mix.get(family, 0.0) > 0.0
    ]
    if opponent_mix.get("ffa", 0.0) > 0.0:
        families.append("ffa")
    if any(opponent_mix.get(kind, 0.0) > 0.0 for kind in ("team", "team2")):
        families.append("team")
    return tuple(families)


def training_world_inputs(
    driver: str | pathlib.Path,
    map_mix: dict[str, float],
    opponent_mix: dict[str, float],
    *,
    fixed_scenario: pathlib.Path = FIXED_SCENARIO_PATH,
    map_generator: pathlib.Path = MAP_GENERATOR_PATH,
    environment_lock: pathlib.Path = TRAIN_ENVIRONMENT_PATH,
) -> dict[str, dict[str, object]]:
    """Content identities for the trainer and every world source it consumes."""
    inputs = {
        "gym_client": input_identity(GYM_CLIENT_PATH),
        "gym_driver": input_identity(driver),
        "model_code": input_identity(MODEL_CODE_PATH),
        "ppo_code": input_identity(PPO_CODE_PATH),
        "trainer": input_identity(LEAGUE_TRAINER_PATH),
    }
    if map_mix.get("fixed", 0.0) > 0.0:
        inputs["fixed_scenario"] = input_identity(fixed_scenario)
    if generated_map_families(map_mix, opponent_mix):
        inputs["map_generator"] = input_identity(map_generator)
        inputs["map_environment"] = input_identity(environment_lock)
    return inputs


def warm_generated_maps(
    seed: int,
    families: Iterable[str],
    driver: str = MAPGEN_DRIVER,
) -> None:
    """Populates every generated-map cache an active lane may use."""
    for family in families:
        if family == "random":
            _generate(
                seed % 100_000,
                cache_dir("oxide-maps-train"),
                driver=driver,
            )
        elif family == "grand":
            _generate(
                seed % 100_000,
                cache_dir("oxide-maps-train-grand"),
                driver=driver,
                pace="grand",
            )
        elif family == "ffa":
            _generate(
                seed % 100_000,
                cache_dir("oxide-maps-train4"),
                players=4,
                driver=driver,
            )
        elif family == "team":
            _generate(
                seed % 100_000,
                cache_dir("oxide-maps-train2v2"),
                players=4,
                teams=True,
                driver=driver,
            )
        else:
            raise ValueError(f"cannot warm unknown generated map family {family!r}")


def faction_knob(seat: int) -> int:
    """The seat's faction, by the map convention every shipped and
    generated scenario follows: even seats run Ferrous (0), odd seats
    Cupric (1000). The knob is honest, never sampled — a policy trained
    on lies about its own roster learns nothing about either."""
    return 0 if seat % 2 == 0 else 1000


def validate_aggression_range(
    aggression_min: int,
    aggression_max: int,
) -> tuple[int, int]:
    """Validates and returns one inclusive aggression curriculum."""
    aggression_range = (aggression_min, aggression_max)
    aggression_min, aggression_max = aggression_range
    if not 0 <= aggression_min <= aggression_max <= 1000:
        raise ValueError(
            "aggression range must satisfy "
            f"0 <= min <= max <= 1000, got {aggression_range}"
        )
    return aggression_range


def validate_aggression_distribution(
    distribution: AggressionDistribution,
) -> AggressionDistribution:
    """Validates, sorts, and normalizes weighted inclusive bands."""
    if not distribution:
        raise ValueError("aggression mix must contain at least one band")
    normalized: list[tuple[int, int, float]] = []
    for lower, upper, weight in sorted(distribution):
        if not 0 <= lower <= upper <= 1000:
            raise ValueError(
                "aggression bands must satisfy "
                f"0 <= lower <= upper <= 1000, got {(lower, upper)}"
            )
        if not np.isfinite(weight) or weight <= 0.0:
            raise ValueError(
                "aggression band weights must be finite and positive, "
                f"got {weight} for {lower}-{upper}"
            )
        if normalized and lower <= normalized[-1][1]:
            previous = normalized[-1]
            raise ValueError(
                "aggression bands must not overlap, got "
                f"{previous[0]}-{previous[1]} and {lower}-{upper}"
            )
        normalized.append((lower, upper, weight))
    total = sum(weight for _lower, _upper, weight in normalized)
    return tuple((lower, upper, weight / total) for lower, upper, weight in normalized)


def parse_aggression_mix(text: str) -> AggressionDistribution:
    """Parses weighted inclusive aggression bands from the CLI."""
    bands: list[tuple[int, int, float]] = []
    for item in text.split(","):
        try:
            raw_band, raw_weight = item.split("=", 1)
            raw_lower, raw_upper = raw_band.split("-", 1)
        except ValueError as err:
            raise argparse.ArgumentTypeError(
                "aggression mix must be LOWER-UPPER=WEIGHT pairs separated by commas"
            ) from err
        try:
            lower, upper = int(raw_lower), int(raw_upper)
        except ValueError as err:
            raise argparse.ArgumentTypeError(
                f"aggression band {raw_band.strip()!r} must contain integers"
            ) from err
        try:
            weight = float(raw_weight)
        except ValueError as err:
            raise argparse.ArgumentTypeError(
                f"aggression weight for {raw_band.strip()!r} must be a number"
            ) from err
        bands.append((lower, upper, weight))
    try:
        return validate_aggression_distribution(tuple(bands))
    except ValueError as err:
        raise argparse.ArgumentTypeError(str(err)) from err


def resolve_aggression_distribution(
    aggression_range: tuple[int, int],
    aggression_mix: AggressionDistribution | None,
) -> AggressionDistribution:
    """Resolves the weighted mix, or the legacy uniform range."""
    if aggression_mix is not None:
        return validate_aggression_distribution(aggression_mix)
    lower, upper = validate_aggression_range(*aggression_range)
    return ((lower, upper, 1.0),)


def resolve_training_aggression_distribution(
    aggression_range: tuple[int, int] | None,
    aggression_mix: AggressionDistribution | None,
) -> AggressionDistribution:
    """Resolves the shipped default or one explicit exploration curriculum."""
    if aggression_mix is not None:
        return validate_aggression_distribution(aggression_mix)
    if aggression_range is None:
        return SHIPPED_AGGRESSION_DISTRIBUTION
    return resolve_aggression_distribution(aggression_range, None)


def sample_aggression(
    rng: np.random.Generator,
    distribution: AggressionDistribution,
) -> int:
    """Samples a band, then an inclusive integer, from a local RNG."""
    if len(distribution) == 1:
        lower, upper, _weight = distribution[0]
    else:
        weights = np.asarray(
            [weight for _lower, _upper, weight in distribution],
            dtype=float,
        )
        band = int(rng.choice(len(distribution), p=weights))
        lower, upper, _weight = distribution[band]
    return int(rng.integers(lower, upper + 1))


def sample_condition(
    rng: np.random.Generator,
    faction: int,
    aggression_range: tuple[int, int] | None = None,
    aggression_mix: AggressionDistribution | None = None,
) -> tuple[int, ...]:
    """Samples one policy condition without an execution handicap.

    This compatibility helper follows the same style-specific policy
    skill as deployment. Jobs use :class:`ProfileCurriculum` so the
    condition is crossed independently with every named execution profile.
    """
    if faction not in (0, 1000):
        raise ValueError(f"faction knob must be 0 or 1000, got {faction}")
    aggression_distribution = resolve_training_aggression_distribution(
        aggression_range,
        aggression_mix,
    )
    aggression = sample_aggression(rng, aggression_distribution)
    return condition_from_profile(
        policy_skill_for_aggression(aggression),
        aggression,
        faction,
    )


def maybe_blunder(
    action: ActionPlan,
    _logits: np.ndarray,
    _mask: np.ndarray,
    hesitation_permille: int,
    rng: np.random.Generator,
) -> ActionPlan:
    """Env-noise blunders, sticky-actions style: the executed action is
    degraded, the policy trains on what it intended. A blunder is
    HESITATION (the decision window passes unused) — matching the
    shipped sim's model. Policy conditioning is deliberately not used
    to derive the rate: named execution difficulty is independent of
    learned strategy conditioning. The old near-best-pick blunders
    kept spending the Fabricator fund mid-save, which both taught the
    policy that low skill means spam and mismatched the runtime."""
    if not 0 <= hesitation_permille <= 1000:
        raise ValueError(
            f"hesitation must be in 0..1000 permille, got {hesitation_permille}"
        )
    if hesitation_permille == 0 or int(rng.integers(1000)) >= hesitation_permille:
        return action
    return (0, 24, 42, 25)


# Rush teacher — global v9 action indices in the four-head plan order;
# feature indices resolved by name. The logic mirrors the Rust-side
# `cup_rusher_plan` (driver/src/gym.rs) so both teachers stay one canary.
IDLE, TRAIN_H, TRAIN_S, FORM, PUSH, SCOUT = 0, 1, 2, 17, 18, 20
NO_CONSTRUCTION, NO_OPERATION, NO_UPGRADE = 24, 25, 42


def rusher(raw: list[int], mask: np.ndarray, tick: int) -> ActionPlan:
    harvesters, staging_size = raw[F["my_harvesters"]], raw[F["staging_army_size"]]
    production = IDLE
    if harvesters < 4 and mask[TRAIN_H]:
        production = TRAIN_H
    elif mask[TRAIN_S]:
        production = TRAIN_S

    operation = NO_OPERATION
    if mask[PUSH] and staging_size >= 5:
        operation = PUSH
    elif mask[FORM]:
        operation = FORM
    elif mask[SCOUT] and tick % 1024 == 0:
        operation = SCOUT
    return (production, NO_CONSTRUCTION, NO_UPGRADE, operation)


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
    episode is the detail: map family and past checkpoint."""

    def __init__(
        self,
        worker: Worker,
        kind: str,
        seat: int,
        pool_dir: pathlib.Path,
        rng: np.random.Generator,
        device: str,
        maps: str = "fixed",
        map_mix: dict[str, float] | None = None,
        faction_mix: dict[str, float] | None = None,
        aggression_range: tuple[int, int] | None = None,
        aggression_mix: AggressionDistribution | None = None,
        map_driver: str = MAPGEN_DRIVER,
    ) -> None:
        # seat: 0/1 for duel kinds; 0..3 for ffa.
        self.worker = worker
        self.kind = kind
        self.pool_dir = pool_dir
        self.rng = rng
        self.device = device
        self.map_driver = map_driver
        self.maps = maps
        self.map_mix = resolve_map_mix(maps, map_mix)
        self.faction_mix = resolve_faction_mix(faction_mix)
        self.aggression_range = aggression_range
        self.aggression_mix = aggression_mix
        self.aggression_distribution = resolve_training_aggression_distribution(
            aggression_range,
            aggression_mix,
        )
        self.profile_curriculum = ProfileCurriculum(
            rng,
            self.aggression_distribution,
            getattr(worker, "profile_catalog", None),
            use_named_profiles=aggression_range is None and aggression_mix is None,
        )
        self.past: nn.Module | None = None
        self.map_family: str | None = None
        self.faction_code: str | None = None
        self.frame: Frame | None = None
        self.conditions: dict[int, tuple[int, ...]] = {}
        self.episode_dials: dict[int, EpisodeDials] = {}
        # Team episodes truncate per seat: a dead learner's lane pads on
        # its frozen last view (zero reward, policy still queried so the
        # batch stays rectangular) until the episode really ends and the
        # team outcome pays every lane its truth. Padded rows are marked
        # invalid and masked out of the PPO update.
        self.dead: set[int] = set()
        self.last_views: dict[int, SeatView] = {}
        # Learner seats that completed a real building-salvage effect this
        # episode. Lives on the Job, not the Lane, because episodes span
        # rollout windows and Lanes are recreated per window.
        self.salvaged: set[int] = set()
        # Learner seats that produced a real field-weld effect this
        # episode. Commands are tracked separately: a sampled action or
        # a walking welder is not yet recovered value.
        self.repair_commanded: set[int] = set()
        self.repaired: set[int] = set()
        # Repair Bay credit waits for BuildingCompleted, not a sampled
        # build action or a scaffold that died unfinished.
        self.built_bay: set[int] = set()
        if kind == "self":
            self.learner_seats = [0, 1]
            self.opp_seat = None
        elif kind == "team":
            # 2v2: the west column (seats 0 and 2 by the mapgen
            # convention) learns as one team against the Overseer.
            self.learner_seats = [0, 2]
            self.opp_seat = None
        elif kind == "team2":
            # 2v2 beside a scripted ally: the learner holds one west
            # chair, the Overseer drives its teammate (and both foes) —
            # the robustness half of team training, so the policy
            # learns to fight NEXT TO conventions it doesn't share.
            self.learner_seats = [seat * 2]  # 0 or 2, the west chairs
            self.opp_seat = None
        elif kind in ("overseer", "ffa"):
            self.learner_seats = [seat]
            self.opp_seat = None
        elif kind in ("past", "rusher"):
            # Both seats are controlled, with the opponent driven
            # locally by a frozen policy or the scripted rush teacher.
            self.learner_seats = [seat]
            self.opp_seat = 1 - seat
        else:
            raise ValueError(f"unknown league opponent kind {kind!r}")
        self.episode_dials = {
            learner: EpisodeDials(1000, 500, SHIPPED_EXECUTION_PROFILES[-1])
            for learner in self.learner_seats
        }
        self.completed_structures: dict[int, set[str]] = {
            seat: set() for seat in self.learner_seats
        }
        self.style_total = dict.fromkeys(self.learner_seats, 0.0)
        self.style_steps = dict.fromkeys(self.learner_seats, 0)
        self.mix_value = {
            seat: [0.0] * len(ARMY_FEATURES) for seat in self.learner_seats
        }
        self.mix_count = {
            seat: [0.0] * len(ARMY_FEATURES) for seat in self.learner_seats
        }

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

    def note_style(self, seat: int, raw: list[int]) -> None:
        """Adds one posture and combat-composition sample."""
        self.style_total[seat] += style_alignment(raw, self.conditions[seat])
        self.style_steps[seat] += 1
        for index, (feature, cost) in enumerate(ARMY_FEATURES):
            count = raw[feature]
            self.mix_value[seat][index] += count * cost
            self.mix_count[seat][index] += count

    def mix_entropies(self, seat: int) -> tuple[float, float]:
        """Integrated combat-value and body-count entropy for this episode."""
        return (
            entropy_from_weights(self.mix_value[seat]),
            entropy_from_weights(self.mix_count[seat]),
        )

    def reset(self, seed: int) -> None:
        TEL["resets"] += 1
        with timed("reset_sec"):
            self._reset(seed)

    def _reset(self, seed: int) -> None:
        self.dead = set()
        self.last_views = {}
        self.salvaged = set()
        self.repair_commanded = set()
        self.repaired = set()
        self.built_bay = set()
        self.completed_structures = {seat: set() for seat in self.learner_seats}
        self.style_total = dict.fromkeys(self.learner_seats, 0.0)
        self.style_steps = dict.fromkeys(self.learner_seats, 0)
        self.mix_value = {
            seat: [0.0] * len(ARMY_FEATURES) for seat in self.learner_seats
        }
        self.mix_count = {
            seat: [0.0] * len(ARMY_FEATURES) for seat in self.learner_seats
        }
        seats = 4 if self.kind in ("ffa", "team", "team2") else 2
        pair = sample_faction_pair(self.rng, self.faction_mix)
        self.faction_code = expand_faction_pair(pair, seats)
        faction_knobs = {
            seat: 1000 if code == "c" else 0
            for seat, code in enumerate(self.faction_code)
        }
        learner_factions = {seat: faction_knobs[seat] for seat in self.learner_seats}
        self.episode_dials = self.profile_curriculum.sample(
            learner_factions,
            specialize_roles=self.kind in ("team", "team2"),
        )
        self.conditions = {
            seat: dials.condition(
                learner_factions[seat],
                self.profile_curriculum.profile_catalog,
            )
            for seat, dials in self.episode_dials.items()
        }
        cadence = next(iter(self.episode_dials.values())).execution.cadence
        for dials in self.episode_dials.values():
            if dials.execution.cadence != cadence:
                raise RuntimeError("one job episode cannot use multiple cadences")
            policy = (
                f"{dials.style}_{dials.variant}_{dials.role}"
                if dials.style is not None
                else f"raw_{dials.aggression}"
            )
            TEL[f"profile_{policy}_{dials.execution.name}"] += 1
        scenario = None
        if self.kind not in ("ffa", "team", "team2"):
            self.map_family = sample_map_family(self.rng, self.map_mix)
        if self.map_family == "random":
            scenario = generate(
                seed % 100_000,
                cache_dir("oxide-maps-train"),
                driver=self.map_driver,
            )
        elif self.map_family == "grand":
            # The pacing curriculum: 1v1 lanes on the big classes only,
            # where the shipped tens-of-minutes game lives. The ffa and
            # team arms below keep their own draws — four bases at vast
            # scale price the sim out of a laptop rollout.
            scenario = generate(
                seed % 100_000,
                cache_dir("oxide-maps-train-grand"),
                driver=self.map_driver,
                pace="grand",
            )
        if self.kind == "ffa":
            self.map_family = "ffa"
            scenario = generate(
                seed % 100_000,
                cache_dir("oxide-maps-train4"),
                players=4,
                driver=self.map_driver,
            )
            self.frame = self.worker.reset(
                seed,
                control=(self.learner_seats[0],),
                conditions=self.conditions,
                scenario=scenario,
                factions=self.faction_code,
                cadence=cadence,
            )
            self._sync_worker_conditions()
            return
        if self.kind in ("team", "team2"):
            self.map_family = "team"
            scenario = generate(
                seed % 100_000,
                cache_dir("oxide-maps-train2v2"),
                players=4,
                teams=True,
                driver=self.map_driver,
            )
            self.frame = self.worker.reset(
                seed,
                control=tuple(self.learner_seats),
                conditions=self.conditions,
                scenario=scenario,
                factions=self.faction_code,
                cadence=cadence,
            )
            self._sync_worker_conditions()
            return
        if self.kind == "overseer":
            self.frame = self.worker.reset(
                seed,
                control=(self.learner_seats[0],),
                conditions=self.conditions,
                scenario=scenario,
                factions=self.faction_code,
                cadence=cadence,
            )
            self._sync_worker_conditions()
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
            if self.kind == "past":
                # A frozen learned opponent sees the same named policy
                # profile as the learner, retinted for its own roster.
                # Otherwise training pits a faceted learner against a
                # zero-facet version of the same policy.
                learner_dials = self.episode_dials[self.learner_seats[0]]
                all_conds[self.opp_seat] = learner_dials.condition(
                    faction_knobs[self.opp_seat],
                    self.profile_curriculum.profile_catalog,
                )
            else:
                all_conds[self.opp_seat] = condition_from_profile(
                    1000,
                    500,
                    faction_knobs[self.opp_seat],
                )
        self.frame = self.worker.reset(
            seed,
            control=(0, 1),
            conditions=all_conds,
            scenario=scenario,
            factions=self.faction_code,
            cadence=cadence,
        )
        self._sync_worker_conditions()

    def _sync_worker_conditions(self) -> None:
        """Keeps learner metadata aligned with Rust-corrected conditions."""
        corrected = getattr(self.worker, "conditions", None)
        if not isinstance(corrected, dict):
            return
        for seat in self.learner_seats:
            if seat in corrected:
                self.conditions[seat] = corrected[seat]

    def opponent_action(self, policy_device: str) -> dict[int, ActionPlan]:
        """Actions for locally-driven seats (empty for worker-driven roles)."""
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
            plan = factorized_sample(logits)[0].cpu()
        return {
            self.opp_seat: (
                int(plan[0]),
                int(plan[1]),
                int(plan[2]),
                int(plan[3]),
            )
        }


def learner_lanes_for_kind(kind: str) -> int:
    """Learner trajectory columns contributed by one job of this kind."""
    return 2 if kind in ("self", "team") else 1


def allocate_role_counts(mix: dict[str, float], workers: int) -> dict[str, int]:
    """Allocates jobs so learner-row shares, rather than job shares,
    approximate the requested opponent mix."""
    if workers <= 0:
        raise ValueError("role allocation requires at least one worker")
    kinds = list(mix)
    adjusted = np.asarray(
        [mix[kind] / learner_lanes_for_kind(kind) for kind in kinds],
        dtype=float,
    )
    total = adjusted.sum()
    if not np.isfinite(adjusted).all() or bool((adjusted < 0.0).any()) or total <= 0.0:
        raise ValueError("opponent mix must contain finite non-negative weights")
    exact = adjusted / total * workers
    counts = np.floor(exact).astype(int)
    while counts.sum() < workers:
        counts[int(np.argmax(exact - counts))] += 1
    return {kind: int(count) for kind, count in zip(kinds, counts, strict=True)}


def realized_learner_row_mix(
    jobs: list[Job],
    valid: np.ndarray | None = None,
) -> dict[str, float]:
    """Reports assigned lane shares, or actual valid training-row shares."""
    expected_columns = sum(len(job.learner_seats) for job in jobs)
    if valid is not None and (valid.ndim != 2 or valid.shape[1] != expected_columns):
        raise ValueError(
            "valid rollout mask must have one column per learner lane, got "
            f"{valid.shape} for {expected_columns} lanes"
        )
    rows: Counter[str] = Counter()
    column = 0
    for job in jobs:
        for _seat in job.learner_seats:
            rows[job.kind] += (
                1 if valid is None else int(np.count_nonzero(valid[:, column]))
            )
            column += 1
    total = sum(rows.values())
    if total == 0:
        return {}
    return {kind: rows[kind] / total for kind in sorted(rows)}


def assign_roles(
    workers: list[Worker],
    mix: dict[str, float],
    pool_dir: pathlib.Path,
    rng: np.random.Generator,
    device: str,
    maps: str = "fixed",
    map_mix: dict[str, float] | None = None,
    faction_mix: dict[str, float] | None = None,
    aggression_range: tuple[int, int] | None = None,
    aggression_mix: AggressionDistribution | None = None,
    map_driver: str = MAPGEN_DRIVER,
) -> list[Job]:
    """Splits workers so trajectory-row shares approximate ``mix``.

    Self-play and learner-vs-team jobs each emit two learner lanes; a
    raw job-count allocation silently doubled those roles' training
    weight. Counts are largest-remainder allocations after dividing
    each requested weight by its learner-lane multiplicity.
    """
    kinds = list(mix)
    role_counts = allocate_role_counts(mix, len(workers))
    jobs = []
    i = 0
    # One independent stream per job, spawned deterministically from
    # the master: with a SHARED generator, pipelined stepping reordered
    # draws whenever an episode reset interleaved differently than the
    # old serial loop, so seeded rollouts silently diverged. Split
    # streams make the draw order a per-job fact, immune to completion
    # order.
    streams = rng.spawn(len(workers))
    for kind in kinds:
        count = role_counts[kind]
        # team2 alternates its single learner between the two west
        # chairs (k % 2 -> seat 0 or 2 inside the Job), everything else
        # keeps its established seat arithmetic.
        seats = 4 if kind in ("ffa", "team") else 2
        for k in range(count):
            jobs.append(
                Job(
                    workers[i],
                    kind,
                    k % seats,
                    pool_dir,
                    streams[i],
                    device,
                    maps,
                    map_mix,
                    faction_mix,
                    aggression_range,
                    aggression_mix,
                    map_driver,
                )
            )
            i += 1
    return jobs


def q12_initialization_provenance(blob: dict | None) -> bool:
    """Whether this invocation reconstructed its critic from a Q12 actor."""
    return blob is not None and blob.get("q12_recovered") is True


def profile_column_parameters(
    policy: nn.Module,
    selected: tuple[str, ...] | None = None,
) -> list[nn.Parameter]:
    """Freezes an actor except for profile inputs while retaining its critic.

    A fresh optimizer plus the gradient mask keeps every pre-v8 coefficient
    byte-identical. This is the narrow continuation for teaching the widened
    condition without moving the raw-profile ladder underneath it. The value
    head remains trainable so an exactly recovered Q12 actor can complete its
    detached-trunk critic warm-up through this same optimizer.
    """
    first = next(
        (module for module in policy.modules() if isinstance(module, nn.Linear)),
        None,
    )
    if first is None or first.in_features != NET_FEATURES:
        raise ValueError("policy lacks the expected first observation layer")
    if PROFILE_CONDITION_COUNT <= 0 or first.in_features <= PROFILE_CONDITION_COUNT:
        raise ValueError("invalid profile-conditioning column span")
    names = PROFILE_CONDITION_NAMES if selected is None else selected
    if not names:
        raise ValueError("at least one profile column must be selected")
    if len(set(names)) != len(names):
        raise ValueError("profile columns cannot be repeated")
    unknown = [name for name in names if name not in PROFILE_CONDITION_NAMES]
    if unknown:
        raise ValueError(f"unknown profile columns: {', '.join(unknown)}")
    for parameter in policy.parameters():
        parameter.requires_grad_(False)
    weight = first.weight
    if not isinstance(weight, nn.Parameter):
        raise TypeError("policy observation weights are not trainable parameters")
    weight.requires_grad_(True)
    mask = torch.zeros_like(weight)
    first_profile_column = first.in_features - PROFILE_CONDITION_COUNT
    for name in names:
        offset = PROFILE_CONDITION_NAMES.index(name)
        mask[:, first_profile_column + offset] = 1
    weight.register_hook(lambda gradient: gradient * mask)
    critic = getattr(policy, "v", None)
    if not isinstance(critic, nn.Linear):
        raise TypeError("policy lacks the expected value head")
    critic_parameters = list(critic.parameters())
    for parameter in critic_parameters:
        parameter.requires_grad_(True)
    return [weight, *critic_parameters]


def validate_profile_column_mode(
    enabled: bool,
    named_profile_curriculum: bool,
    style_coefficient: float,
    initialized: bool,
    selected: list[str] | None = None,
) -> None:
    """Rejects a profile-only phase that cannot teach the widened columns."""
    if selected and not enabled:
        raise ValueError("--profile-column requires --profile-columns-only")
    if not enabled:
        return
    if not named_profile_curriculum:
        raise ValueError(
            "--profile-columns-only requires the Rust named-profile curriculum"
        )
    if not style_coefficient:
        raise ValueError("--profile-columns-only requires --style-coef")
    if not initialized:
        raise ValueError("--profile-columns-only requires checkpoint initialization")


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
    reclaimer_bonus: float = 0.0,
    structure_bonus: float = 0.0,
    style_coefficient: float = 0.0,
    collection: str = "windows",
    episode_max_steps: int = MAX_EPISODE_DECISIONS,
) -> tuple[tuple[np.ndarray, ...], np.ndarray, list[float]]:
    if collection not in ("windows", "episodes"):
        raise ValueError(f"unknown collection mode {collection!r}")
    if steps <= 0:
        raise ValueError("rollout steps must be positive")
    if episode_max_steps <= 0:
        raise ValueError("episode max steps must be positive")
    lanes = {(id(j), s): Lane(j.worker, s) for j in jobs for s in j.learner_seats}
    finished_rewards = []
    for j in jobs:
        if collection == "episodes" or j.frame is None:
            j.reset(next(seeds))
    for j in jobs:
        for s in j.learner_seats:
            lanes[(id(j), s)].last_pot = potential(j.seat_view(s).raw)

    active = {id(job) for job in jobs}
    limit = steps if collection == "windows" else episode_max_steps
    for _ in range(limit):
        if collection == "episodes" and not active:
            break
        step_jobs = [job for job in jobs if id(job) in active]
        views = []
        keys = []
        live = []
        for j in step_jobs:
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
            action = factorized_sample(logits)
            logp = factorized_joint_log_prob(logits, action).cpu().numpy()
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
        for j in step_jobs:
            acts = {}
            for s in j.learner_seats:
                if s in j.dead:
                    continue  # a frozen lane sends nothing to the sim
                k = row[(id(j), s)]
                intended: ActionPlan = (
                    int(action[k, 0]),
                    int(action[k, 1]),
                    int(action[k, 2]),
                    int(action[k, 3]),
                )
                acts[s] = maybe_blunder(
                    intended,
                    logits_np[k],
                    mask[k],
                    j.episode_dials[s].execution.hesitation_permille,
                    j.rng,
                )
                if acts[s][1] == SALVAGE_ACTION:
                    TEL["salvage_action_samples"] += 1
                elif acts[s][1] == BUILD_TURRET_ACTION:
                    TEL["turret_action_samples"] += 1
                elif acts[s][1] == BUILD_ARRAY_ACTION:
                    TEL["array_action_samples"] += 1
                elif acts[s][1] == REPAIR_ACTION:
                    TEL["repair_action_samples"] += 1
                elif acts[s][1] == BUILD_BAY_ACTION:
                    TEL["bay_action_samples"] += 1
            acts.update(j.opponent_action(device))
            all_acts.append(acts)
        with timed("env_sec"):
            for j, acts in zip(step_jobs, all_acts, strict=True):
                j.worker.send_step(acts)
        for j in step_jobs:
            with timed("env_sec"):
                frame = j.worker.recv()
            for s, effects in frame.effects.items():
                if s not in j.learner_seats:
                    continue
                if effects.repair_unit_commands > 0:
                    j.repair_commanded.add(s)
                    TEL["repair_commands"] += effects.repair_unit_commands
                if effects.repair_unit_hp_restored > 0:
                    j.repaired.add(s)
                    TEL["repair_hp"] += effects.repair_unit_hp_restored
                if effects.unit_hp_restored > 0:
                    TEL["unit_hp_restored"] += effects.unit_hp_restored
                if effects.buildings_salvaged > 0:
                    j.salvaged.add(s)
                    TEL["buildings_salvaged"] += effects.buildings_salvaged
                completed = set(effects.buildings_completed)
                j.completed_structures[s].update(completed)
                if "repair_bay" in completed:
                    j.built_bay.add(s)
            if frame.done:
                # v5: the terminal frame carries observations for
                # living seats. Install it as the live frame BEFORE any
                # bonus reads a view, so tech and mix pay off the true
                # final position — a dead seat, absent from the
                # terminal seats, keeps its frozen last view.
                j.frame = frame
                # A tick cap is an artificial episode boundary, not an
                # absorbing game state. Price each living seat's final
                # observation with V(next) now; `done=True` below still
                # cuts GAE before the freshly reset episode. Eliminated
                # seats have no next observation and never bootstrap.
                truncation_values: dict[int, float] = {}
                truncated_seats = [
                    s for s in j.learner_seats if frame.truncated and s in frame.seats
                ]
                if truncated_seats:
                    terminal_views = [frame.seats[s] for s in truncated_seats]
                    terminal_obs = np.stack([view.obs for view in terminal_views])
                    terminal_mask = np.stack([view.mask for view in terminal_views])
                    with timed("policy_sec"), torch.no_grad():
                        _, terminal_value = policy(
                            torch.as_tensor(terminal_obs, device=device),
                            torch.as_tensor(terminal_mask, device=device),
                        )
                    truncation_values = dict(
                        zip(
                            truncated_seats,
                            terminal_value.cpu().numpy().tolist(),
                            strict=True,
                        )
                    )
                for s in j.learner_seats:
                    lane = lanes[(id(j), s)]
                    j.note_style(s, j.seat_view(s).raw)
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
                        # Same instrument as the tech bonus, but paid
                        # only after Rust reports a completed dismantle.
                        # Sampling the action, issuing a walking order,
                        # or partly stripping a structure earns nothing.
                        mut_bonus += salvage_bonus
                    if s in j.repair_commanded:
                        TEL["ep_repair_commanded"] += 1
                    # One flag seeds both v6 weld effects, each paying
                    # independently. A policy earns nothing for merely
                    # sampling an action, issuing a walking order, or
                    # leaving a Bay scaffold unfinished.
                    if s in j.repaired:
                        TEL["ep_repair"] += 1
                        mut_bonus += repair_bonus
                    if s in j.built_bay:
                        TEL["ep_bay"] += 1
                        mut_bonus += repair_bonus
                    if "reclaimer" in j.completed_structures[s]:
                        TEL["ep_build_reclaimer"] += 1
                        mut_bonus += reclaimer_bonus
                    completed_structures = j.completed_structures[s].intersection(
                        SEEDED_STRUCTURES
                    )
                    for kind in sorted(completed_structures):
                        TEL[f"ep_build_{kind}"] += 1
                        mut_bonus += structure_bonus
                    if mix_bonus > 0.0:
                        # Integrate every competitive decision rather
                        # than rewarding a lucky terminal snapshot. Use
                        # the weaker of value and body-count diversity so
                        # cheap-unit body spam cannot hide behind a few
                        # expensive units. Two bits earns the full bonus.
                        value_h, count_h = j.mix_entropies(s)
                        h = min(value_h, count_h)
                        TEL["mix_value_ent"] += value_h
                        TEL["mix_count_ent"] += count_h
                        TEL["mix_ent"] += h
                        mut_bonus += mix_bonus * min(h, 2.0) / 2.0
                    episode_style = style_bonus(
                        j.style_total[s],
                        j.style_steps[s],
                        style_coefficient,
                    )
                    mut_bonus += episode_style
                    TEL["style_bonus"] += episode_style
                    if s in truncation_values:
                        # Time limits retain both Phi(next) and V(next):
                        # only a true game end is an absorbing zero-value
                        # state. Injecting the value bootstrap into this
                        # transition lets the ordinary done mask cut GAE
                        # cleanly across the immediate environment reset.
                        next_pot = potential(frame.seats[s].raw)
                        shape = SHAPE_K * (SHAPE_GAMMA * next_pot - lane.last_pot)
                        bootstrap = TRAIN_GAMMA * truncation_values[s]
                    else:
                        # Canonical potential shaping assigns true terminal
                        # states Phi=0. Eliminated learners also settle
                        # against zero and receive no time-limit bootstrap.
                        shape = -SHAPE_K * lane.last_pot
                        bootstrap = 0.0
                    lane.rew.append(frame.reward(s) + mut_bonus + shape + bootstrap)
                    lane.done.append(True)
                    finished_rewards.append(frame.reward(s))
                if collection == "windows":
                    j.reset(next(seeds))
                    for s in j.learner_seats:
                        lanes[(id(j), s)].last_pot = potential(j.seat_view(s).raw)
                else:
                    active.remove(id(j))
            else:
                for s in j.learner_seats:
                    lane = lanes[(id(j), s)]
                    if s not in frame.seats:
                        # Died this step (or earlier): the lane pads at
                        # its frozen final view until the team's episode
                        # resolves. Keep applying the canonical discounted
                        # transition: the padding rows are update-masked,
                        # but GAE spans them and shaping must still telescope.
                        j.dead.add(s)
                        lane.rew.append(
                            SHAPE_K * (SHAPE_GAMMA * lane.last_pot - lane.last_pot)
                        )
                        lane.done.append(False)
                        continue
                    raw = frame.seats[s].raw
                    j.note_style(s, raw)
                    pot = potential(raw)
                    lane.rew.append(SHAPE_K * (SHAPE_GAMMA * pot - lane.last_pot))
                    lane.done.append(False)
                    lane.last_pot = pot
                j.frame = frame

    ordered = list(lanes.values())
    if collection == "episodes":
        if active:
            unfinished = [
                f"{job.kind}@tick{job.view.tick}" for job in jobs if id(job) in active
            ]
            raise RuntimeError(
                "episode collection exceeded "
                f"{episode_max_steps} decisions without completion: "
                + ", ".join(unfinished)
            )
        width = max(len(lane.obs) for lane in ordered)
        for lane in ordered:
            while len(lane.obs) < width:
                lane.obs.append(np.array(lane.obs[-1], copy=True))
                lane.mask.append(np.array(lane.mask[-1], copy=True))
                lane.act.append(np.array(lane.act[-1], copy=True))
                lane.logp.append(lane.logp[-1])
                lane.val.append(lane.val[-1])
                lane.rew.append(0.0)
                lane.done.append(True)
                lane.valid.append(False)
        last_val = np.zeros(len(ordered), dtype=np.float32)
    else:
        # Bootstrap values for unfinished fixed-window lanes.
        views = []
        for j in jobs:
            for s in j.learner_seats:
                views.append(j.seat_view(s))
        obs = np.stack([v.obs for v in views])
        mask = np.stack([v.mask for v in views])
        with torch.no_grad():
            _, last_value = policy(
                torch.as_tensor(obs, device=device),
                torch.as_tensor(mask, device=device),
            )
        last_val = last_value.cpu().numpy()

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
    """Greedy fixed suite across canonical profiles and both physical seats.

    ``opponent`` is ``overseer`` or ``rusher``. Each seed uses one
    Rust-authored named profile, cycling through the worker's catalog in
    handshake order; both seat assignments therefore test the same
    strategic policy vector under honest faction retinting.
    """
    seeds = range(1000, 1010) if seeds is None else seeds
    wins = games = 0
    profiles = workers[0].profile_catalog.profiles
    if not profiles:
        raise RuntimeError("evaluation requires Rust canonical profiles")
    jobs = [
        (seed, seat, profiles[seed_index % len(profiles)])
        for seed_index, seed in enumerate(seeds)
        for seat in (0, 1)
    ]
    for start in range(0, len(jobs), len(workers)):
        chunk = jobs[start : start + len(workers)]
        live = []
        for i, (seed, seat, profile) in enumerate(chunk):
            w = workers[i]
            catalog = w.profile_catalog
            straight = {
                s: catalog.condition(
                    profile.style,
                    profile.variant,
                    catalog.default_role,
                    "cupric" if faction_knob(s) == 1000 else "ferrous",
                )
                for s in (0, 1)
            }
            if opponent == "rusher":
                frame = w.reset(seed, control=(0, 1), conditions=straight)
            else:
                frame = w.reset(seed, control=(seat,), conditions=straight)
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
                action = factorized_greedy(logits).cpu().numpy()
            # Send-all, collect-in-order: the eval bracket's games are
            # independent, so the workers may as well all be simulating.
            sends = []
            for k, (i, seat, frame) in enumerate(live):
                plan: ActionPlan = (
                    int(action[k, 0]),
                    int(action[k, 1]),
                    int(action[k, 2]),
                    int(action[k, 3]),
                )
                acts: dict[int, ActionPlan] = {seat: plan}
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
    decisiveness, both value- and body-weighted mix readings, and unit
    AND building shares — read beside the rusher eval in the run log.
    Observed, never rewarded: nothing here may feed a loss term, or the
    probe stops being a measurement.

    Judgment reads every seat's competitive-lifetime combat fields like
    the fun gate does. The sampler retains losing seats before defeat
    but excludes resigned/foundry-less autonomous remnants; all-unit
    fields remain diagnostic."""
    overall = payload["overall"]
    health = cap_health(payload["matches"], 2_000)
    spread = overall.get("seat_combat_entropy")
    count_spread = overall.get("seat_combat_count_entropy")
    dominance = overall.get("seat_combat_count_dominance")
    return {
        "matches": overall["matches"],
        "decided": overall["decided"],
        "capped": overall["capped"],
        "competitive_seats": overall["combat_seats"],
        "active_caps": health["active_caps"],
        "unhealthy_caps": health["unhealthy_caps"],
        "resource_exhausted_caps": health["resource_exhausted_caps"],
        "entropy_bits": round(overall["combat_entropy_bits"], 2),
        "seat_p10": round(spread["p10"], 2) if spread else None,
        "count_entropy_bits": round(overall["combat_count_entropy_bits"], 2),
        "seat_count_p10": round(count_spread["p10"], 2) if count_spread else None,
        "count_dominance_p90": (round(dominance["p90"], 3) if dominance else None),
        "unit_share": {k: round(v, 3) for k, v in overall["mean_combat_share"].items()},
        "count_share": {
            k: round(v, 3) for k, v in overall["mean_combat_count_share"].items()
        },
        "building_share": {
            k: round(v, 3)
            for k, v in overall["competitive_seats_with_building"].items()
        },
    }


# Schema 8 drops the deleted scripted-tier dial from the probe payload
# while keeping the competitive-lifetime combat metrics beside the
# preserved all-unit diagnostics.
PROBE_SCHEMA = 9


def composition_probe(
    policy: nn.Module,
    arch: str,
    update: int,
    run_dir: pathlib.Path,
    driver: str,
    scenarios: str,
    level: str,
    seeds: int,
    lineage: dict[str, object] | None = None,
    critic_ready: bool = True,
) -> dict:
    """Snapshots the current policy and runs the enriched composition
    probe against the anchor slate (the shipped maps): checkpoint ->
    Q12 export -> `driver balance-probe --weights` — the fun gate's
    instrument played in-loop, so composition collapse shows up beside
    the rusher canary instead of after the campaign. The snapshot,
    artifact, and raw payload all land under runs/<name>/probe/ for
    post-hoc reading."""
    rng_state = torch.get_rng_state()
    try:
        probe_dir = run_dir / "probe"
        probe_dir.mkdir(parents=True, exist_ok=True)
        ckpt = probe_dir / f"ckpt-{update:05d}.pt"
        metadata: dict[str, object] = {
            "critic_ready": critic_ready,
            "gym_version": GYM_VERSION,
            "update": update,
        }
        if lineage is not None:
            metadata = checkpoint_metadata(lineage, metadata)
        save_policy(policy, arch, ckpt, metadata)
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
        if schema != PROBE_SCHEMA:
            raise RuntimeError(
                f"probe payload is schema {schema}, this loop reads {PROBE_SCHEMA} "
                "exactly — use the matching driver"
            )
        return probe_canary(payload)
    finally:
        torch.set_rng_state(rng_state)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--name",
        required=True,
        help="fresh write-once directory name under runs/",
    )
    ap.add_argument("--driver", default="../../target/release/oxide-driver")
    ap.add_argument(
        "--arch",
        default="mlp",
        help="mlp | wide (ignored with checkpoint initialization)",
    )
    ap.add_argument("--workers", type=int, default=8)
    ap.add_argument("--steps", type=int, default=384)
    ap.add_argument(
        "--collection",
        choices=("windows", "episodes"),
        default="windows",
        help="windows updates after --steps decisions; episodes freezes one "
        "policy through exactly one complete episode per worker",
    )
    ap.add_argument("--updates", type=int, default=2000)
    ap.add_argument("--lr", type=float, default=1e-4)
    ap.add_argument(
        "--gae-lambda",
        type=unit_interval,
        default=0.95,
        help="GAE trace decay in [0, 1]; 1.0 carries terminal outcomes "
        "through the full collected horizon",
    )
    add_entropy_arguments(ap)
    add_initialization_arguments(ap)
    ap.add_argument("--pool-every", type=int, default=25)
    ap.add_argument("--eval-every", type=int, default=25)
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
        "--style-coef",
        type=bounded_style_coefficient,
        default=0.0,
        help="bounded terminal personality bonus in 0..=0.1; named profiles "
        "align own-state economy, air, siege, support, and commitment with "
        "their Rust facets, while raw profiles retain aggression posture. "
        "The episode mean earns at most this amount; 0 disables (default).",
    )
    ap.add_argument(
        "--profile-columns-only",
        action="store_true",
        help="freeze the initialized policy except for the five widened "
        "profile input columns; requires the Rust named-profile curriculum",
    )
    ap.add_argument(
        "--profile-column",
        action="append",
        choices=PROFILE_CONDITION_NAMES,
        help="with --profile-columns-only, train only this named widened column; "
        "repeat to select several (default: all five)",
    )
    ap.add_argument(
        "--tech-bonus",
        type=float,
        default=0.0,
        help="terminal bonus paid when the seat still owns a completed "
        "Fabricator at episode end (own-state only, fog-safe); annealed "
        "linearly to zero across --tech-anneal updates. 0 disables.",
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
        help="terminal bonus paid after the seat completes a building "
        "salvage this episode (own-state effect telemetry, fog-safe); "
        "annealed on the --tech-anneal schedule. 0 disables.",
    )
    ap.add_argument(
        "--repair-bonus",
        type=float,
        default=0.0,
        help="terminal bonus paid per successful v6 weld effect this "
        "episode (field-welded hp and a completed Repair Bay each earn "
        "it once; own-state only, fog-safe); annealed on the "
        "--tech-anneal schedule. 0 disables.",
    )
    ap.add_argument(
        "--structure-bonus",
        type=bounded_structure_bonus,
        default=0.0,
        help="terminal bonus paid once per distinct completed tactical "
        "structure kind (Turret and Array); capped at 0.02 per kind "
        "and annealed on the "
        "--tech-anneal schedule. 0 disables.",
    )
    add_reclaimer_bonus_argument(ap)
    ap.add_argument(
        "--mix-bonus",
        type=float,
        default=0.0,
        help="terminal bonus scaled by the weaker of the seat's integrated "
        "combat-value and body-count entropy (fog-safe; 2 bits earns the "
        "full bonus); annealed on the same schedule as --tech-bonus. "
        "0 disables.",
    )
    ap.add_argument(
        "--maps",
        default="fixed",
        help="fixed | random (fresh map per episode) | grand (fresh map "
        "per episode, 1v1 lanes drawn from the large/vast classes only "
        "— the pacing curriculum)",
    )
    ap.add_argument(
        "--map-mix",
        type=parse_map_mix,
        default=None,
        help="per-duel-episode map weights, for example "
        "fixed=.25,random=.25,grand=.50; overrides --maps",
    )
    ap.add_argument(
        "--faction-mix",
        type=parse_faction_mix,
        default=None,
        help="per-episode ordered faction-pair weights, for example "
        "ff=.25,fc=.25,cf=.25,cc=.25; four-seat modes repeat the pair",
    )
    ap.add_argument(
        "--aggression-min",
        type=int,
        default=None,
        help="lowest aggression conditioning for a custom uniform exploration "
        "range (default consumes the Rust named-profile catalog)",
    )
    ap.add_argument(
        "--aggression-max",
        type=int,
        default=None,
        help="highest aggression conditioning for a custom uniform exploration "
        "range (default consumes the Rust named-profile catalog)",
    )
    ap.add_argument(
        "--aggression-mix",
        type=parse_aggression_mix,
        default=None,
        help="weighted inclusive aggression bands, for example "
        "0-249=.25,250-499=.25,500-749=.25,750-1000=.25; "
        "overrides --aggression-min/max",
    )
    ap.add_argument(
        "--mix",
        type=parse_opponent_mix,
        default=parse_opponent_mix("self=0.45,past=0.20,overseer=0.20,rusher=0.15"),
        help="opponent kind weights; overseer seats the scripted Overseer "
        "commander as the fixed anchor and rusher the scripted rush teacher",
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
    mix = args.mix
    try:
        map_mix = resolve_map_mix(args.maps, args.map_mix)
        faction_mix = resolve_faction_mix(args.faction_mix)
        aggression_range = (
            None
            if args.aggression_min is None and args.aggression_max is None
            else (
                0 if args.aggression_min is None else args.aggression_min,
                1000 if args.aggression_max is None else args.aggression_max,
            )
        )
        aggression_distribution = resolve_training_aggression_distribution(
            aggression_range,
            args.aggression_mix,
        )
        named_profile_curriculum = (
            aggression_range is None and args.aggression_mix is None
        )
        validate_profile_column_mode(
            args.profile_columns_only,
            named_profile_curriculum,
            args.style_coef,
            args.initialize_from is not None or args.resume is not None,
            args.profile_column,
        )
    except ValueError as err:
        ap.error(str(err))

    start_update = 0
    initialization_path = (
        args.initialize_from if args.initialize_from is not None else args.resume
    )
    if args.resume is not None:
        print(
            "warning: --resume is deprecated; use --initialize-from. "
            "Both start a new phase with fresh optimizer, RNG, and episodes.",
            file=sys.stderr,
        )
    initialization_blob = None
    if initialization_path is not None:
        policy, blob = load_policy(initialization_path, device)
        initialization_blob = blob
        arch = blob.get("arch", "mlp")
        # Carry the parent's absolute clock for pool numbering and artifact
        # provenance; optimizer, RNG, episodes, and annealing begin a new phase.
        start_update = int(blob.get("update", 0) or 0)
    else:
        arch = args.arch
        policy = make_policy(arch)
    initialize_q12_recovered = q12_initialization_provenance(initialization_blob)
    initialize_critic_ready = checkpoint_critic_ready(initialization_blob)
    value_warmup = resolved_value_warmup(
        args.value_warmup,
        initialized=initialization_path is not None,
        critic_ready=initialize_critic_ready,
    )
    optimizer_parameters = (
        profile_column_parameters(
            policy,
            None if args.profile_column is None else tuple(args.profile_column),
        )
        if args.profile_columns_only
        else list(policy.parameters())
    )
    opt = torch.optim.Adam(optimizer_parameters, lr=args.lr)
    anchor = None
    anchor_blob = None
    if args.anchor:
        anchor, anchor_blob = load_policy(args.anchor, device)
        anchor.eval()
    lineage_inputs = training_world_inputs(args.driver, map_mix, mix)
    if initialization_path is not None:
        lineage_inputs["initializer"] = input_identity(
            initialization_path,
            initialization_blob,
        )
    if args.anchor:
        lineage_inputs["anchor"] = input_identity(args.anchor, anchor_blob)
    run_lineage = build_lineage(
        phase="league",
        phase_start_update=start_update,
        hyperparameters={
            "anchor_coefficient": args.anchor_coef,
            "anchor_decay": args.anchor_decay,
            "arch": arch,
            "collection": args.collection,
            "entropy_coefficient": args.entropy_coef,
            "eval_every": args.eval_every,
            "execution_profiles": [
                {
                    "cadence": profile.cadence,
                    "hesitation_permille": profile.hesitation_permille,
                    "name": profile.name,
                }
                for profile in SHIPPED_EXECUTION_PROFILES
            ],
            "faction_mix": faction_mix,
            "gae_lambda": args.gae_lambda,
            "gym_version": GYM_VERSION,
            "learning_rate": args.lr,
            "map_mix": map_mix,
            "opponent_mix": mix,
            "optimizer_seed": 1,
            "pool_every": args.pool_every,
            "profile_curriculum": (
                "rust-named-factorial"
                if named_profile_curriculum
                else "custom-aggression"
            ),
            "trainable_scope": (
                "profile-columns-only" if args.profile_columns_only else "full-policy"
            ),
            "trainable_profile_columns": (
                list(PROFILE_CONDITION_NAMES)
                if args.profile_columns_only and args.profile_column is None
                else args.profile_column
                if args.profile_columns_only
                else []
            ),
            "ppo": {
                "clip": 0.2,
                "epochs": 4,
                "gradient_clip": 0.5,
                "kl_stop": 0.03,
                "minibatch": 1024,
                "value_loss_coefficient": 0.5,
            },
            "production_entropy_coefficient": args.production_entropy_coef,
            **reward_lineage_hyperparameters(args),
            "reward_gamma": TRAIN_GAMMA,
            "reward_anneal_clock": "actor-updates-after-critic-warmup",
            "rollout_seed_base": 50_000,
            "steps": args.steps,
            "shape_coefficient": SHAPE_K,
            "style_coefficient": args.style_coef,
            "tech_anneal": args.tech_anneal or args.updates,
            "torch_seed": 0,
            "updates": args.updates,
            "actor_updates": max(args.updates - value_warmup, 0),
            "critic_warmup_updates": value_warmup,
            "value_warmup": value_warmup,
            "workers": args.workers,
            "aggression_distribution": [
                {"min": lower, "max": upper, "weight": weight}
                for lower, upper, weight in aggression_distribution
            ],
        },
        inputs=lineage_inputs,
    )
    try:
        pool_dir = claim_fresh_run_directory(run_dir)
    except RuntimeError as err:
        ap.error(str(err))
    workers = [Worker(args.driver) for _ in range(args.workers)]
    if (args.repair_bonus or args.reclaimer_bonus or args.structure_bonus) and any(
        not worker.supports_effect_telemetry for worker in workers
    ):
        for worker in workers:
            worker.close()
        raise RuntimeError(
            "effect-seeded training requires a gym driver that advertises "
            "effect_telemetry"
        )
    rng = np.random.default_rng(0)
    optimizer_rng = np.random.default_rng(1)

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

    warm_families = generated_map_families(map_mix, mix)
    if warm_families:
        # Cold-cache map generation costs a driver subprocess per map
        # (~34% of an update when the cache is empty). A daemon warmer
        # stays a few seeds ahead of the cursor across every active
        # generated family, so the hot path only ever sees cache hits;
        # generate() is atomic-rename safe, so a foreground race is
        # harmless. Determinism is untouched: same seed, same file.
        def warm() -> None:
            warmed = 0
            while True:
                target = consumed[0] + 1 + 2 * args.workers
                while warmed < target:
                    warmed = max(warmed, consumed[0] + 1)
                    warm_generated_maps(warmed, warm_families, args.driver)
                    warmed += 1
                time.sleep(0.25)

        threading.Thread(target=warm, daemon=True, name="map-warmer").start()
    log = (run_dir / "log.jsonl").open("x")

    try:
        jobs = assign_roles(
            workers,
            mix,
            pool_dir,
            rng,
            device,
            args.maps,
            map_mix,
            faction_mix,
            aggression_range,
            args.aggression_mix,
            args.driver,
        )
        allocated_learner_row_mix = realized_learner_row_mix(jobs)
        for update in range(start_update + 1, start_update + args.updates + 1):
            t0 = time.time()
            phase_update = update - start_update
            TEL.clear()
            # The anneal runs on THIS run's clock, not the absolute one:
            # a resumed consolidation wants its exploration push at its
            # own start, wherever the parent's clock stands. Critic-only
            # warm-up holds the initial seed because the actor cannot yet
            # respond to it.
            reward_update = reward_anneal_index(
                update,
                start_update,
                value_warmup,
            )
            tb = tech_bonus_at(
                args.tech_bonus,
                reward_update,
                args.tech_anneal or args.updates,
            )
            mb = tech_bonus_at(
                args.mix_bonus,
                reward_update,
                args.tech_anneal or args.updates,
            )
            sb = tech_bonus_at(
                args.salvage_bonus,
                reward_update,
                args.tech_anneal or args.updates,
            )
            rb = tech_bonus_at(
                args.repair_bonus,
                reward_update,
                args.tech_anneal or args.updates,
            )
            eb = tech_bonus_at(
                args.reclaimer_bonus,
                reward_update,
                args.tech_anneal or args.updates,
            )
            ub = tech_bonus_at(
                args.structure_bonus,
                reward_update,
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
                reclaimer_bonus=eb,
                structure_bonus=ub,
                style_coefficient=args.style_coef,
                collection=args.collection,
            )
            rollout_sec = time.time() - t0
            obs_b, mask_b, act_b, logp_b, val_b, rew_b, done_b, valid_b = batch
            learner_row_mix = realized_learner_row_mix(jobs, valid_b)
            adv, ret = gae(
                rew_b,
                done_b,
                val_b,
                last_val,
                gamma=SHAPE_GAMMA,
                lam=args.gae_lambda,
            )
            # GAE ran over the full rectangle so a dead teammate's lane
            # still carries the episode's team payoff backward; the
            # frozen-view padding rows themselves train nothing.
            rows = valid_b.reshape(-1)
            flat = (
                obs_b.reshape(-1, NET_FEATURES)[rows],
                mask_b.reshape(-1, ACTIONS)[rows],
                act_b.reshape(-1, 3)[rows],
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
                ent_coef=args.entropy_coef,
                production_ent_coef=args.production_entropy_coef,
                value_only=value_warmup_active(
                    update,
                    start_update,
                    value_warmup,
                ),
                anchor=anchor,
                anchor_coef=anchor_coefficient_at(
                    args.anchor_coef,
                    args.anchor_decay,
                    update,
                    start_update,
                ),
                rng=optimizer_rng,
            )
            decisions = int(np.count_nonzero(valid_b))
            entry = {
                "update": update,
                "phase_update": phase_update,
                "lineage_id": run_lineage["lineage_id"],
                "kinds": sorted(j.kind for j in jobs),
                "collection": args.collection,
                "allocated_learner_row_mix": allocated_learner_row_mix,
                "learner_row_mix": learner_row_mix,
                "map_mix": map_mix,
                "faction_mix": faction_mix,
                "aggression_range": aggression_range,
                "aggression_distribution": [
                    {"min": lower, "max": upper, "weight": weight}
                    for lower, upper, weight in aggression_distribution
                ],
                "profile_curriculum": (
                    "rust-named-factorial"
                    if named_profile_curriculum
                    else "custom-aggression"
                ),
                "trainable_scope": (
                    "profile-columns-only"
                    if args.profile_columns_only
                    else "full-policy"
                ),
                "trainable_profile_columns": (
                    list(PROFILE_CONDITION_NAMES)
                    if args.profile_columns_only and args.profile_column is None
                    else args.profile_column
                    if args.profile_columns_only
                    else []
                ),
                "execution_profiles": [
                    {
                        "name": profile.name,
                        "hesitation_permille": profile.hesitation_permille,
                        "cadence": profile.cadence,
                    }
                    for profile in SHIPPED_EXECUTION_PROFILES
                ],
                "gae_lambda": args.gae_lambda,
                "entropy_coef": args.entropy_coef,
                "production_entropy_coef": args.production_entropy_coef,
                "effective_production_entropy_coef": (
                    effective_production_entropy_coefficient(
                        args.entropy_coef,
                        args.production_entropy_coef,
                    )
                ),
                "initialized": initialization_path is not None,
                "initialize_critic_ready": initialize_critic_ready,
                "initialize_q12_recovered": initialize_q12_recovered,
                "value_warmup": value_warmup,
                "phase_total_updates": args.updates,
                "critic_warmup_updates": value_warmup,
                "actor_updates": max(args.updates - value_warmup, 0),
                "optimization_mode": (
                    "critic-warmup"
                    if value_warmup_active(update, start_update, value_warmup)
                    else "actor-and-critic"
                ),
                "episodes": len(finals),
                "avg_final": round(float(np.mean(finals)), 3) if finals else None,
                "policy_loss": round(
                    stats["pi"] / max(stats["batches"], 1),
                    4,
                ),
                "value_loss": round(
                    stats["v"] / max(stats["batches"], 1),
                    4,
                ),
                "ent": round(stats["ent"] / max(stats["batches"], 1), 3),
                "production_ent": round(
                    stats["production_ent"] / max(stats["batches"], 1),
                    3,
                ),
                "kl": round(stats["kl"], 4),
                "optimizer_batches": int(stats["batches"]),
                "valid_decisions": decisions,
                "sec": round(time.time() - t0, 1),
                # The phase clocks: where an update's wall time actually
                # went, so optimization is measurement, not folklore.
                "rollout_sec": round(rollout_sec, 2),
                "update_sec": round(time.time() - t_update, 2),
                "decisions_s": round(decisions / max(rollout_sec, 1e-9)),
                **{
                    k: (int(v) if k == "resets" or k.startswith("ep_") else round(v, 2))
                    for k, v in sorted(TEL.items())
                },
            }
            if args.tech_bonus:
                entry["tech_bonus"] = round(tb, 4)
            if args.salvage_bonus:
                entry["salvage_bonus"] = round(sb, 4)
            if args.repair_bonus:
                entry["repair_bonus"] = round(rb, 4)
            if args.reclaimer_bonus:
                entry["reclaimer_bonus"] = round(eb, 4)
            if args.structure_bonus:
                entry["structure_bonus"] = round(ub, 4)
            if args.style_coef:
                entry["style_coef"] = args.style_coef
            final_update = phase_update == args.updates
            critic_ready = phase_update >= value_warmup
            if (
                phase_interval_due(update, start_update, args.pool_every)
                or final_update
            ):
                save_policy(
                    policy,
                    arch,
                    pool_dir / f"ckpt-{update:05d}.pt",
                    checkpoint_metadata(
                        run_lineage,
                        {
                            "critic_ready": critic_ready,
                            "gym_version": GYM_VERSION,
                            "update": update,
                        },
                    ),
                )
            if (
                phase_interval_due(update, start_update, args.eval_every)
                or final_update
            ):
                entry["eval"] = {
                    op: round(evaluate(policy, workers, device, op), 3)
                    for op in ("overseer", "rusher")
                }
                save_policy(
                    policy,
                    arch,
                    run_dir / "latest.pt",
                    checkpoint_metadata(
                        run_lineage,
                        {
                            "critic_ready": critic_ready,
                            "gym_version": GYM_VERSION,
                            "update": update,
                        },
                    ),
                )
                # Eval borrowed the workers; the standing episodes are
                # gone. Fresh ones start next rollout.
                for j in jobs:
                    j.frame = None
            if phase_interval_due(update, start_update, args.probe_every):
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
                        run_lineage,
                        critic_ready,
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
