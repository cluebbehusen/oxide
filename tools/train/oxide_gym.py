"""Wrapper over `oxide-driver gym` subprocesses (contract pinned by
``GYM_VERSION`` below and re-verified at every worker's hello).

Each worker is one driver process serving sequential episodes over
stdio. `control` picks the externally-driven seats: `(0,)` against the
scripted Overseer, or `(0, 1)` for self-play/league — each frame then
carries features and a mask per controlled seat. Features arrive as
raw integers (the Rust side is the source of truth for their meaning);
`normalize` scales them to roughly [-1, 1] for the network.

Named-profile resets send the five Rust-authored condition facets back to
the server so action masks and lowering use the same bounded doctrine as the
shipped bot. ``PROFILED_DOCTRINE_VERSION`` pins that rollout behavior without
changing the v9 tensor shape.
"""

import contextlib
import json
import subprocess
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Literal, cast

import numpy as np

if TYPE_CHECKING:
    from collections.abc import Sequence

type FactionName = Literal["ferrous", "cupric"]
type BuildingName = Literal[
    "foundry",
    "turret",
    "fabricator",
    "flak_turret",
    "bastion",
    "array",
    "reclaimer",
    "repair_bay",
    "extractor",
    "airworks",
    "crucible",
    "barricade",
    "scrap_depot",
    "scuttle_charge",
]
BUILDING_NAMES = frozenset(
    {
        "foundry",
        "turret",
        "fabricator",
        "flak_turret",
        "bastion",
        "array",
        "reclaimer",
        "repair_bay",
        "extractor",
        "airworks",
        "crucible",
        "barricade",
        "scrap_depot",
        "scuttle_charge",
    }
)

FEATURES = 107
ACTIONS = 43
GYM_VERSION = 9
# Wire capability for applying named-profile facets to the Rust executive.
# It is versioned independently because it changes rollout semantics without
# changing the actor tensor shape described by ``GYM_VERSION``.
PROFILED_DOCTRINE_VERSION = 1
# Each decision is one independent choice from each action head. The
# indices remain global flat-head rows so checkpoints and exported
# artifacts still carry one 43-row affine policy head. The partition is
# a wire contract with sim/src/bot/gym.rs `ACTION_HEADS`.
PRODUCTION_HEAD = (0, 1, 2, 3, 4, 5, 6, 7, 8, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35)
CONSTRUCTION_HEAD = (24, 9, 10, 11, 12, 13, 14, 15, 21, 22, 23, 36, 37, 38, 39)
UPGRADE_HEAD = (42, 40)
OPERATION_HEAD = (25, 16, 17, 18, 19, 20, 41)
ACTION_HEADS = (PRODUCTION_HEAD, CONSTRUCTION_HEAD, UPGRADE_HEAD, OPERATION_HEAD)
ACTION_PLAN_DIMS = len(ACTION_HEADS)
type ActionPlan = tuple[int, int, int, int]
# Conditioning dims appended to the gym features as network input:
# skill (0-1000; 1000 = full strength), aggression (0-1000; 500 =
# balanced), faction (0 = ferrous, 1000 = cupric), and a four-way
# strategy one-hot derived from aggression, followed by the five
# Rust-authored named-profile facets. Raw-aggression tools append zero
# facets; named training consumes complete vectors from the gym hello.
CONDITION_NAMES = (
    "skill",
    "aggression",
    "faction",
    "strategy_fortify",
    "strategy_industry",
    "strategy_combined",
    "strategy_pressure",
    "profile_economy",
    "profile_air",
    "profile_siege",
    "profile_support",
    "profile_commitment",
)
CONDITION_DIMS = len(CONDITION_NAMES)
BASE_CONDITION_DIMS = 7
PROFILE_CONDITION_NAMES = tuple(
    name for name in CONDITION_NAMES if name.startswith("profile_")
)
PROFILE_CONDITION_INDICES = tuple(
    CONDITION_NAMES.index(name) for name in PROFILE_CONDITION_NAMES
)
NET_FEATURES = FEATURES + CONDITION_DIMS

DRAW_REWARD = -0.3
# Decision stride in sim ticks. 16 halves the credit-assignment horizon
# relative to the bots' own 8 — macro decisions don't need finer.
CADENCE = 16

# Hand-set scales keyed by the Rust-side feature NAME — the gym hello
# carries the authoritative name list, and the Worker asserts ours
# matches it index for index. A wrong count or a shifted column fails
# at handshake instead of silently training on garbage.
SCALE_BY_NAME: dict[str, float] = {
    "tick": 40_000,
    "scrap": 500,
    "my_harvesters": 8,
    "my_sentinels": 20,
    "my_scuttlers": 10,
    "my_lancers": 10,
    "my_turrets_built": 4,
    "fab_built": 1,
    "max_foundry_hp": 800,
    "idle_ground_fighters": 20,
    "armies": 4,
    "staging_army_size": 20,
    "army_state": 4,
    "enemy_harvesters": 8,
    "enemy_sentinels": 20,
    "enemy_scuttlers": 10,
    "enemy_lancers": 10,
    "enemy_buildings": 8,
    "enemy_turrets_built": 4,
    "enemy_foundry_known": 1,
    "my_strength": 500,
    "army_strength": 500,
    "enemy_strength": 500,
    "home_x": 1000,
    "home_y": 1000,
    "enemy_site_x": 1000,
    "enemy_site_y": 1000,
    "intel_age": 10_000,
    "seen_strength": 500,
    "seen_age": 10_000,
    "seen_x": 1000,
    "seen_y": 1000,
    "my_bombards": 6,
    "my_antiair": 8,
    "my_airground": 8,
    "my_airair": 8,
    "enemy_bombards": 6,
    "enemy_antiair": 8,
    "enemy_airground": 8,
    "enemy_airair": 8,
    "my_flak_built": 3,
    "my_arrays_built": 2,
    "my_reclaimers_built": 3,
    "my_aa_strength": 300,
    "enemy_aa_strength": 300,
    "blip_count": 10,
    "nearest_blip_x": 1000,
    "nearest_blip_y": 1000,
    "wreck_count": 20,
    "wreck_value": 500,
    "nearest_wreck_x": 1000,
    "nearest_wreck_y": 1000,
    "damaged_buildings": 5,
    "repair_deficit": 1_000,
    "ally_units": 30,
    "ally_strength": 500,
    "ally_foundry_hp": 800,
    "ally_distress": 1,
    "faction": 1,
    # v4: relative 0-1000 coordinates above, plus map dims and shell
    # observability. Dims scale by the largest shipped field.
    "map_w": 100,
    "map_h": 100,
    "incoming_shells": 6,
    "my_shells_in_flight": 8,
    # v5: own cost-weighted standing buildings — what Salvage can
    # liquidate, and the potential term that stops selling a Bastion
    # from reading as free dense reward.
    "my_building_value": 500,
    # v6: scrap locked in own ground wounds — what RepairUnit recovers
    # and what a Repair Bay amortizes against; the potential term that
    # prices welding by the wound it heals.
    "damaged_unit_value": 500,
    # v7: economy, commitment, and survivability context for the
    # factorized production/construction/operation decision.
    "known_salvage_value": 2_000,
    "near_home_salvage_value": 1_000,
    "nearest_salvage_distance": 200,
    "idle_harvesters": 8,
    "carried_scrap": 200,
    "queued_unit_value": 1_000,
    "construction_site_value": 1_000,
    "my_unit_health_value": 2_000,
    "my_building_health_value": 1_000,
    "my_bastions_built": 2,
    "my_repair_bays_built": 1,
    "my_construction_sites": 4,
    "home_enemy_pressure": 500,
    "nearest_enemy_distance": 200,
    "construction_plan": 7,
    "construction_reserve": 250,
    # v9: the 0.15 roster, tree state, and frame intel. Counts scale like
    # the established roster columns; frame coordinates are raw tile
    # positions (map dims cap near 100), unlike the 0-1000 relative
    # coordinate columns above.
    "my_wardens": 6,
    "my_tenders": 4,
    "my_excavators": 4,
    "my_scout_flyers": 4,
    "my_interceptors": 8,
    "my_bombers": 6,
    "my_transports": 4,
    "my_sappers": 6,
    "my_breakers": 4,
    "my_avalanches": 2,
    "enemy_interceptors": 8,
    "enemy_bombers": 6,
    "enemy_heavies": 6,
    "airworks_built": 1,
    "crucible_built": 1,
    "my_foundries_built": 3,
    "my_extractors_built": 3,
    "known_frames": 8,
    "nearest_frame_x": 100,
    "nearest_frame_y": 100,
    "nearest_frame_distance": 100,
    "my_upgraded_works": 3,
    "upgrade_candidates": 4,
    "tech_tier": 3,
    "transport_cargo": 8,
    "enemy_foundries_known": 3,
}
FEATURE_NAMES = list(SCALE_BY_NAME.keys())
SCALES = np.array([SCALE_BY_NAME[n] for n in FEATURE_NAMES], dtype=np.float32)
if SCALES.shape != (FEATURES,):
    raise RuntimeError("SCALES must cover every gym feature")
FACTION_FEATURE = FEATURE_NAMES.index("faction")

FACTION_CODES: dict[str, FactionName] = {
    "f": "ferrous",
    "c": "cupric",
}


def _faction_name(value: str) -> FactionName:
    match value.lower():
        case "ferrous":
            return "ferrous"
        case "cupric":
            return "cupric"
        case _:
            raise ValueError(f"unknown faction {value!r}; expected ferrous or cupric")


def normalize_factions(factions: str | Sequence[FactionName]) -> list[FactionName]:
    """Expands a compact seat-order code (``ff``, ``fc``, ``cf``,
    ``cc``) or validates a full-name sequence."""
    if isinstance(factions, str):
        if not factions:
            raise ValueError("factions must name at least one seat")
        normalized: list[FactionName] = []
        for code in factions.lower():
            try:
                normalized.append(FACTION_CODES[code])
            except KeyError as err:
                raise ValueError(
                    f"unknown faction code {code!r}; expected only f or c"
                ) from err
        return normalized
    if not factions:
        raise ValueError("factions must name at least one seat")
    return [_faction_name(faction) for faction in factions]


def validate_reported_factions(
    reported: Sequence[FactionName] | None,
    requested: list[FactionName] | None,
) -> list[FactionName] | None:
    """Checks the optional reset extension instead of trusting that an
    older same-version driver understood an unknown request field."""
    actual = normalize_factions(reported) if reported is not None else None
    if requested is not None and actual != requested:
        raise RuntimeError(
            f"gym reset faction mismatch: requested {requested}, Rust reported {actual}"
        )
    return actual


def normalize(features: list[int]) -> np.ndarray:
    return np.asarray(features, dtype=np.float32) / SCALES


def faction_name_from_features(features: list[int]) -> FactionName:
    """Reads the seat's actual Rust-side faction observation."""
    faction = features[FACTION_FEATURE]
    if faction == 0:
        return "ferrous"
    if faction == 1:
        return "cupric"
    raise ValueError(f"Rust faction feature must be 0 or 1, got {faction}")


def faction_knob_from_features(features: list[int]) -> int:
    """Returns the network's 0/1000 faction condition from Rust facts."""
    return 1000 if faction_name_from_features(features) == "cupric" else 0


def strategy_one_hot(aggression: int) -> tuple[int, int, int, int]:
    """Maps aggression quartiles to fortify/industry/combined/pressure."""
    if aggression < 0 or aggression > 1000:
        raise ValueError(f"aggression must be in 0..1000, got {aggression}")
    bucket = min(aggression // 250, 3)
    return (
        1000 if bucket == 0 else 0,
        1000 if bucket == 1 else 0,
        1000 if bucket == 2 else 0,
        1000 if bucket == 3 else 0,
    )


def condition_from_profile(
    skill: int,
    aggression: int,
    faction: int,
) -> tuple[int, ...]:
    """Builds a raw-aggression v9 condition with no named profile lean."""
    if skill < 0 or skill > 1000:
        raise ValueError(f"skill must be in 0..1000, got {skill}")
    if faction not in (0, 1000):
        raise ValueError(f"faction must be 0 or 1000, got {faction}")
    return (
        skill,
        aggression,
        faction,
        *strategy_one_hot(aggression),
        *(0 for _ in range(CONDITION_DIMS - BASE_CONDITION_DIMS)),
    )


def policy_skill_for_aggression(aggression: int) -> int:
    """Returns the learned skill condition used by the shipped ladder wrapper."""
    if aggression < 0 or aggression > 1000:
        raise ValueError(f"aggression must be in 0..1000, got {aggression}")
    return 620 if 250 <= aggression <= 499 else 1000


def honest_condition(
    condition: tuple[int, ...], features: list[int]
) -> tuple[int, ...]:
    """Keeps skill/style knobs and replaces any caller-supplied faction
    with the faction Rust observed after scenario retinting."""
    if len(condition) != CONDITION_DIMS:
        raise ValueError(f"expected {CONDITION_DIMS} knobs, got {condition}")
    corrected = list(condition)
    corrected[CONDITION_NAMES.index("faction")] = faction_knob_from_features(features)
    return tuple(corrected)


def with_condition(obs: np.ndarray, condition: tuple[int, ...]) -> np.ndarray:
    """Appends the normalized v9 policy condition."""
    if len(condition) != CONDITION_DIMS:
        raise ValueError(f"expected {CONDITION_DIMS} knobs, got {condition}")
    knobs = np.asarray(condition, dtype=np.float32) / 1000.0
    return np.concatenate([obs, knobs])


def doctrine_facets(condition: tuple[int, ...]) -> tuple[int, ...]:
    """Extracts Rust-authored profile facets from one policy condition.

    Named-profile values originate in the gym hello catalog. Raw-aggression
    conditions carry zeroes here, which selects the historical gym doctrine.
    """
    if len(condition) != CONDITION_DIMS:
        raise ValueError(f"expected {CONDITION_DIMS} knobs, got {condition}")
    facets = tuple(condition[index] for index in PROFILE_CONDITION_INDICES)
    if any(not isinstance(value, int) or not 0 <= value <= 1000 for value in facets):
        raise ValueError(f"profile facets must be integers in 0..1000, got {facets}")
    return facets


@dataclass(frozen=True)
class CanonicalProfile:
    """One Rust-authored named style variant from the gym handshake."""

    style: str
    variant: int
    name: str
    aggression: int
    roles: tuple[str, ...]


class ProfileCatalog:
    """Validated named-profile vectors published by the Rust gym server."""

    def __init__(
        self,
        profiles: tuple[CanonicalProfile, ...],
        values: dict[tuple[str, int, str, FactionName], tuple[int, ...]],
        default_role: str,
    ) -> None:
        self.profiles = profiles
        self._values = values
        self.default_role = default_role
        role_sets = {profile.roles for profile in profiles}
        if len(role_sets) != 1:
            raise RuntimeError(
                "Rust canonical profiles disagree on their team-role set"
            )
        (self.team_roles,) = role_sets
        if default_role not in self.team_roles:
            raise RuntimeError(
                f"Rust default team role {default_role!r} is absent from its "
                "profile catalog"
            )

    @classmethod
    def from_hello(cls, hello: dict) -> ProfileCatalog:
        """Parses the canonical contract instead of rebuilding it in Python."""
        names = hello.get("condition_names")
        if names != list(CONDITION_NAMES):
            raise RuntimeError(
                "condition-name mismatch between Rust and Python — "
                f"rust: {names} vs python: {list(CONDITION_NAMES)}"
            )
        if hello.get("conditioning") != CONDITION_DIMS:
            raise RuntimeError(
                "conditioning-width mismatch between Rust and Python — "
                f"rust: {hello.get('conditioning')} vs python: {CONDITION_DIMS}"
            )
        default_role = hello.get("default_team_role")
        if not isinstance(default_role, str) or not default_role:
            raise RuntimeError("Rust gym hello lacks a default_team_role")
        raw_profiles = hello.get("canonical_profiles")
        if not isinstance(raw_profiles, list) or not raw_profiles:
            raise RuntimeError("Rust gym hello lacks canonical named profiles")

        aggression_index = CONDITION_NAMES.index("aggression")
        faction_index = CONDITION_NAMES.index("faction")
        profiles: list[CanonicalProfile] = []
        values: dict[tuple[str, int, str, FactionName], tuple[int, ...]] = {}
        profile_keys: set[tuple[str, int]] = set()
        profile_names: set[str] = set()
        for raw_profile in raw_profiles:
            if not isinstance(raw_profile, dict):
                raise TypeError("canonical profile rows must be objects")
            style = raw_profile.get("style")
            variant = raw_profile.get("variant")
            name = raw_profile.get("name")
            aggression = raw_profile.get("aggression")
            if (
                not isinstance(style, str)
                or not style
                or not isinstance(variant, int)
                or variant < 0
                or not isinstance(name, str)
                or not name
                or not isinstance(aggression, int)
                or not 0 <= aggression <= 1000
            ):
                raise RuntimeError(f"invalid canonical profile metadata: {raw_profile}")
            profile_key = (style, variant)
            if profile_key in profile_keys or name in profile_names:
                raise RuntimeError(
                    f"duplicate canonical profile {style}/{variant}/{name}"
                )
            profile_keys.add(profile_key)
            profile_names.add(name)

            raw_roles = raw_profile.get("roles")
            if not isinstance(raw_roles, list) or not raw_roles:
                raise RuntimeError(f"canonical profile {name!r} has no team roles")
            roles: list[str] = []
            for raw_role in raw_roles:
                if not isinstance(raw_role, dict):
                    raise TypeError(f"canonical profile {name!r} has a non-object role")
                role = raw_role.get("role")
                if not isinstance(role, str) or not role or role in roles:
                    raise RuntimeError(
                        f"canonical profile {name!r} has invalid role {role!r}"
                    )
                roles.append(role)
                raw_conditions = raw_role.get("conditions")
                if not isinstance(raw_conditions, dict) or set(raw_conditions) != {
                    "ferrous",
                    "cupric",
                }:
                    raise RuntimeError(
                        f"canonical profile {name!r}/{role} must publish both factions"
                    )
                for faction_name in ("ferrous", "cupric"):
                    raw_values = raw_conditions[faction_name]
                    if (
                        not isinstance(raw_values, list)
                        or len(raw_values) != CONDITION_DIMS
                        or any(
                            not isinstance(value, int) or not 0 <= value <= 1000
                            for value in raw_values
                        )
                    ):
                        raise RuntimeError(
                            f"invalid canonical condition for "
                            f"{name!r}/{role}/{faction_name}"
                        )
                    expected_faction = 1000 if faction_name == "cupric" else 0
                    if (
                        raw_values[aggression_index] != aggression
                        or raw_values[faction_index] != expected_faction
                    ):
                        raise RuntimeError(
                            f"canonical condition metadata disagrees for "
                            f"{name!r}/{role}/{faction_name}"
                        )
                    key = (style, variant, role, faction_name)
                    if key in values:
                        raise RuntimeError(f"duplicate canonical condition {key}")
                    values[key] = tuple(raw_values)
            profiles.append(
                CanonicalProfile(style, variant, name, aggression, tuple(roles))
            )
        return cls(tuple(profiles), values, default_role)

    def condition(
        self,
        style: str,
        variant: int,
        role: str,
        faction: FactionName,
    ) -> tuple[int, ...]:
        """Returns one complete Rust-authored named condition."""
        key = (style, variant, role, faction)
        try:
            return self._values[key]
        except KeyError as err:
            raise ValueError(f"unknown canonical profile condition {key}") from err


def validate_action_plan(plan: tuple[int, ...] | list[int]) -> ActionPlan:
    """Validates one plan of global indices against its four heads."""
    if len(plan) != ACTION_PLAN_DIMS:
        raise ValueError(
            f"action plan must contain {ACTION_PLAN_DIMS} global indices, got {plan}"
        )
    normalized = tuple(int(action) for action in plan)
    for head_index, (action, head) in enumerate(
        zip(normalized, ACTION_HEADS, strict=True)
    ):
        if action not in head:
            raise ValueError(
                f"action {action} does not belong to action head {head_index}: {head}"
            )
    return cast("ActionPlan", normalized)


@dataclass
class SeatView:
    obs: np.ndarray
    mask: np.ndarray
    raw: list[int]

    @property
    def faction(self) -> FactionName:
        """The actual roster reported by the Rust observation."""
        return faction_name_from_features(self.raw)

    @property
    def faction_knob(self) -> int:
        """The same roster in the network condition's 0/1000 scale."""
        return faction_knob_from_features(self.raw)


@dataclass(frozen=True)
class SeatEffects:
    """Own-state effects observed during one Rust decision interval.

    These are training telemetry, not policy inputs. In particular, the
    repair and salvage bonuses consume completed work rather than rewarding
    a sampled action that lowering or the sim may have rejected.
    """

    repair_unit_commands: int = 0
    unit_hp_restored: int = 0
    repair_unit_hp_restored: int = 0
    buildings_salvaged: int = 0
    buildings_completed: tuple[BuildingName, ...] = ()


@dataclass
class Frame:
    done: bool
    tick: int
    # True only when the gym's tick budget ended a still-live match.
    # This is an artificial MDP boundary: living seats receive a neutral
    # outcome and the trainer bootstraps their terminal observations.
    truncated: bool = False
    winner: int | None = None  # winning TEAM id, or None for a draw/cap
    winners: list[int] | None = None  # seats on the winning team
    alive: list[int] | None = None  # controlled seats still standing
    seats: dict[int, SeatView] = field(default_factory=dict)
    factions: list[FactionName] | None = None  # every scenario seat, in order
    effects: dict[int, SeatEffects] = field(default_factory=dict)

    def reward(self, seat: int) -> float:
        """Terminal reward for `seat` (call when done). A team win pays
        every seat on the team; elimination is a loss even if the game
        rages on (or the teammate later wins it). A living seat at an
        artificial tick boundary is neutral, not a penalized draw."""
        if self.winners is not None and len(self.winners) > 0:
            return 1.0 if seat in self.winners else -1.0
        if self.winner is not None:
            return 1.0 if self.winner == seat else -1.0
        if self.alive is not None and seat not in self.alive:
            return -1.0
        if self.truncated:
            return 0.0
        return DRAW_REWARD


def parse_effects(reply: dict) -> dict[int, SeatEffects]:
    """Parses the optional v6 effect-telemetry extension."""
    effects: dict[int, SeatEffects] = {}
    for raw in reply.get("effects", []):
        seat = int(raw["seat"])
        if seat in effects:
            raise RuntimeError(f"duplicate effect telemetry for seat {seat}")
        completed = tuple(raw.get("buildings_completed", ()))
        unknown = set(completed).difference(BUILDING_NAMES)
        if unknown:
            raise RuntimeError(
                f"unknown completed building kinds from Rust: {sorted(unknown)}"
            )
        typed_completed = cast("tuple[BuildingName, ...]", completed)
        effects[seat] = SeatEffects(
            repair_unit_commands=int(raw.get("repair_unit_commands", 0)),
            unit_hp_restored=int(raw.get("unit_hp_restored", 0)),
            repair_unit_hp_restored=int(raw.get("repair_unit_hp_restored", 0)),
            buildings_salvaged=int(raw.get("buildings_salvaged", 0)),
            buildings_completed=typed_completed,
        )
    return effects


class Worker:
    """One driver process, one live episode at a time."""

    def __init__(self, driver_bin: str) -> None:
        self.conditions: dict[int, tuple[int, ...]] = {}
        self.factions: list[FactionName] | None = None
        self.requested_factions: list[FactionName] | None = None
        self.proc = subprocess.Popen(
            [driver_bin, "gym"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        if self.proc.stdin is None or self.proc.stdout is None:
            raise RuntimeError("gym driver started without pipes")
        self._stdin = self.proc.stdin
        self._stdout = self.proc.stdout
        hello = json.loads(self._stdout.readline())
        if not hello.get("ready"):
            raise RuntimeError(f"gym server failed to start: {hello}")
        if hello["version"] != GYM_VERSION or hello["features"] != FEATURES:
            raise RuntimeError(f"gym contract mismatch: {hello}")
        if hello.get("actions") != ACTIONS:
            got = hello.get("actions")
            raise RuntimeError(f"action-count mismatch: rust {got} vs python {ACTIONS}")
        expected_heads = [list(head) for head in ACTION_HEADS]
        if hello.get("action_heads") != expected_heads:
            raise RuntimeError(
                "action-head mismatch between Rust and Python — "
                f"rust: {hello.get('action_heads')} vs python: {expected_heads}"
            )
        if hello.get("names") != FEATURE_NAMES:
            raise RuntimeError(
                "feature-name mismatch between Rust and Python — "
                f"rust: {hello.get('names')} vs python: {FEATURE_NAMES}"
            )
        if hello.get("profiled_doctrine") != PROFILED_DOCTRINE_VERSION:
            raise RuntimeError(
                "profiled-doctrine mismatch between Rust and Python — "
                f"rust: {hello.get('profiled_doctrine')} vs "
                f"python: {PROFILED_DOCTRINE_VERSION}"
            )
        self.profile_catalog = ProfileCatalog.from_hello(hello)
        self._supports_reset_factions = hello.get("reset_factions") is True
        self._supports_effect_telemetry = hello.get("effect_telemetry") is True

    def named_condition(
        self,
        style: str,
        variant: int,
        role: str,
        faction: FactionName,
    ) -> tuple[int, ...]:
        """Returns a complete condition from the connected Rust contract."""
        return self.profile_catalog.condition(style, variant, role, faction)

    @property
    def supports_effect_telemetry(self) -> bool:
        """Whether replies carry the optional successful-effect sideband."""
        return self._supports_effect_telemetry

    def _rpc(self, request: dict) -> Frame:
        self.send(request)
        return self.recv()

    def send(self, request: dict) -> None:
        """Writes a request without waiting for its reply. Exactly one
        request may be in flight per worker — the driver is a strict
        read-compute-reply loop — but *across* workers, sending to all
        before collecting from any turns N blocking round-trips into N
        concurrent simulations. The pipelined loops in league.py exist
        because of this split."""
        self._stdin.write(json.dumps(request) + "\n")

    def recv(self) -> Frame:
        """Blocks for the reply to the outstanding request."""
        reply = json.loads(self._stdout.readline())
        if "error" in reply:
            raise RuntimeError(reply["error"])
        self.factions = validate_reported_factions(
            reply.get("factions"),
            self.requested_factions,
        )
        if reply["done"]:
            frame = Frame(
                done=True,
                tick=reply["tick"],
                truncated=bool(reply["truncated"]),
                winner=reply["winner"],
                winners=reply.get("winners"),
                alive=reply.get("alive"),
                factions=self.factions,
                effects=parse_effects(reply),
            )
            # v5: terminal frames carry observations for living
            # controlled seats — evidence for terminal shaping (the
            # tech bonus pays the LAST view's fab_built, and the
            # potential difference closes on the true final position).
            for s in reply.get("seats", []):
                seat, view = self._seat_view(s)
                frame.seats[seat] = view
            return frame
        frame = Frame(
            False,
            reply["tick"],
            factions=self.factions,
            effects=parse_effects(reply),
        )
        for s in reply["seats"]:
            seat, view = self._seat_view(s)
            frame.seats[seat] = view
        return frame

    def _seat_view(self, reply: dict) -> tuple[int, SeatView]:
        """Builds one seat row and makes its condition faction honest."""
        seat = reply["seat"]
        raw = reply["features"]
        obs = normalize(raw)
        condition = self.conditions.get(seat)
        if condition is not None:
            condition = honest_condition(condition, raw)
            self.conditions[seat] = condition
            obs = with_condition(obs, condition)
        view = SeatView(
            obs,
            np.asarray(reply["mask"], dtype=bool),
            raw,
        )
        if self.factions is not None:
            try:
                reported = self.factions[seat]
            except IndexError as err:
                raise RuntimeError(
                    f"Rust faction list has no entry for controlled seat {seat}"
                ) from err
            if view.faction != reported:
                raise RuntimeError(
                    "gym faction observation mismatch: "
                    f"seat {seat} list says {reported}, feature says {view.faction}"
                )
        return seat, view

    def reset(
        self,
        seed: int,
        control: tuple[int, ...] = (0,),
        max_ticks: int = 40_000,
        cadence: int = CADENCE,
        scenario: str | None = None,
        conditions: dict[int, tuple[int, ...]] | None = None,
        factions: str | Sequence[FactionName] | None = None,
    ) -> Frame:
        """Starts an episode. Every uncontrolled seat is driven by the
        Rust-side Overseer. ``factions`` optionally names every
        scenario seat in order, either as a compact code such as ``fc``
        or as full names. The returned Rust observation is authoritative
        for the condition's faction knob. Profile facets are extracted by
        their advertised condition names and sent to the Rust executive;
        raw zero-facet conditions retain its historical doctrine."""
        if conditions is not None and set(conditions) != set(control):
            raise ValueError(
                "conditions must name exactly the controlled seats: "
                f"control={list(control)}, conditions={sorted(conditions)}"
            )
        self.control = control
        self.conditions = dict(conditions or {})
        self.factions = None
        self.requested_factions = None
        req: dict[str, object] = {
            "cmd": "reset",
            "seed": seed,
            "control": list(control),
            "max_ticks": max_ticks,
            "cadence": cadence,
        }
        if scenario:
            req["scenario"] = scenario
        if conditions is not None:
            req["profile_facets"] = [
                list(doctrine_facets(self.conditions[seat])) for seat in control
            ]
        if factions is not None:
            if not self._supports_reset_factions:
                raise RuntimeError(
                    "gym driver does not advertise reset-time faction support"
                )
            self.requested_factions = normalize_factions(factions)
            req["factions"] = self.requested_factions
        return self._rpc(req)

    def step(self, actions: dict[int, ActionPlan]) -> Frame:
        """Action plans keyed by seat, in control order. Seats absent from
        the dict send nothing — the driver expects exactly one action
        plan per *living* controlled seat, and a dead teammate's seat has
        dropped out of the frame."""
        self.send_step(actions)
        return self.recv()

    def send_step(self, actions: dict[int, ActionPlan]) -> None:
        """The write half of `step`, for pipelining across workers."""
        ordered = [
            list(validate_action_plan(actions[s])) for s in self.control if s in actions
        ]
        self.send({"cmd": "step", "actions": ordered})

    def close(self) -> None:
        with contextlib.suppress(OSError, ValueError):
            self._stdin.write('{"cmd":"quit"}\n')
            self._stdin.flush()
        self.proc.terminate()
