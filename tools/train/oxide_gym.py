"""Wrapper over `oxide-driver gym` subprocesses (contract pinned by
``GYM_VERSION`` below and re-verified at every worker's hello).

Each worker is one driver process serving sequential episodes over
stdio. `control` picks the externally-driven seats: `(0,)` against a
scripted tier, or `(0, 1)` for self-play/league — each frame then
carries features and a mask per controlled seat. Features arrive as
raw integers (the Rust side is the source of truth for their meaning);
`normalize` scales them to roughly [-1, 1] for the network.
"""

import contextlib
import json
import subprocess
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Literal

import numpy as np

if TYPE_CHECKING:
    from collections.abc import Sequence

type FactionName = Literal["ferrous", "cupric"]

FEATURES = 65
ACTIONS = 24
GYM_VERSION = 6
# Conditioning dims appended to the gym features as network input:
# skill (0-1000; 1000 = full strength), aggression (0-1000; 500 =
# balanced), and faction (0 = ferrous, 1000 = cupric). The world
# features come from Rust; the knobs are the bot's own configuration,
# so the wrapper appends them.
CONDITION_DIMS = 3
NET_FEATURES = FEATURES + CONDITION_DIMS

DRAW_REWARD = -0.3
STEP_COST = 1e-4
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


def honest_condition(
    condition: tuple[int, ...], features: list[int]
) -> tuple[int, ...]:
    """Keeps skill/style knobs and replaces any caller-supplied faction
    with the faction Rust observed after scenario retinting."""
    if len(condition) != CONDITION_DIMS:
        raise ValueError(f"expected {CONDITION_DIMS} knobs, got {condition}")
    return (*condition[:-1], faction_knob_from_features(features))


def with_condition(obs: np.ndarray, condition: tuple[int, ...]) -> np.ndarray:
    """Appends normalized (skill, aggression, faction) knobs to a
    feature row. Faction rides as 0 (ferrous) or 1000 (cupric) so every
    knob shares the /1000 scale."""
    if len(condition) != CONDITION_DIMS:
        raise ValueError(f"expected {CONDITION_DIMS} knobs, got {condition}")
    knobs = np.asarray(condition, dtype=np.float32) / 1000.0
    return np.concatenate([obs, knobs])


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


@dataclass
class Frame:
    done: bool
    tick: int
    winner: int | None = None  # winning TEAM id, or None for a draw/cap
    winners: list[int] | None = None  # seats on the winning team
    alive: list[int] | None = None  # controlled seats still standing
    seats: dict[int, SeatView] = field(default_factory=dict)
    factions: list[FactionName] | None = None  # every scenario seat, in order

    def reward(self, seat: int) -> float:
        """Terminal reward for `seat` (call when done). A team win pays
        every seat on the team; elimination is a loss even if the game
        rages on (or the teammate later wins it)."""
        if self.winners is not None and len(self.winners) > 0:
            return 1.0 if seat in self.winners else -1.0
        if self.winner is not None:
            return 1.0 if self.winner == seat else -1.0
        if self.alive is not None and seat not in self.alive:
            return -1.0
        return DRAW_REWARD


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
        if hello.get("names") != FEATURE_NAMES:
            raise RuntimeError(
                "feature-name mismatch between Rust and Python — "
                f"rust: {hello.get('names')} vs python: {FEATURE_NAMES}"
            )
        self._supports_reset_factions = hello.get("reset_factions") is True

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
                winner=reply["winner"],
                winners=reply.get("winners"),
                alive=reply.get("alive"),
                factions=self.factions,
            )
            # v5: terminal frames carry observations for living
            # controlled seats — evidence for terminal shaping (the
            # tech bonus pays the LAST view's fab_built, and the
            # potential difference closes on the true final position).
            for s in reply.get("seats", []):
                seat, view = self._seat_view(s)
                frame.seats[seat] = view
            return frame
        frame = Frame(False, reply["tick"], factions=self.factions)
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
        tier: str = "veteran",
        max_ticks: int = 40_000,
        cadence: int = CADENCE,
        scenario: str | None = None,
        conditions: dict[int, tuple[int, ...]] | None = None,
        factions: str | Sequence[FactionName] | None = None,
    ) -> Frame:
        """Starts an episode. ``factions`` optionally names every
        scenario seat in order, either as a compact code such as ``fc``
        or as full names. The returned Rust observation is authoritative
        for the condition's faction knob."""
        self.control = control
        self.conditions = dict(conditions or {})
        self.factions = None
        self.requested_factions = None
        req: dict[str, object] = {
            "cmd": "reset",
            "seed": seed,
            "control": list(control),
            "tier": tier,
            "max_ticks": max_ticks,
            "cadence": cadence,
        }
        if scenario:
            req["scenario"] = scenario
        if factions is not None:
            if not self._supports_reset_factions:
                raise RuntimeError(
                    "gym driver does not advertise reset-time faction support"
                )
            self.requested_factions = normalize_factions(factions)
            req["factions"] = self.requested_factions
        return self._rpc(req)

    def step(self, actions: dict[int, int]) -> Frame:
        """Actions keyed by seat, in control order. Seats absent from
        the dict send nothing — the driver expects exactly one action
        per *living* controlled seat, and a dead teammate's seat has
        dropped out of the frame."""
        self.send_step(actions)
        return self.recv()

    def send_step(self, actions: dict[int, int]) -> None:
        """The write half of `step`, for pipelining across workers."""
        ordered = [int(actions[s]) for s in self.control if s in actions]
        self.send({"cmd": "step", "actions": ordered})

    def close(self) -> None:
        with contextlib.suppress(OSError, ValueError):
            self._stdin.write('{"cmd":"quit"}\n')
            self._stdin.flush()
        self.proc.terminate()
