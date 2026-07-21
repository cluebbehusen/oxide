"""Wrapper over `oxide-driver gym` subprocesses (protocol v2).

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

import numpy as np

FEATURES = 32
ACTIONS = 11
GYM_VERSION = 2
# Conditioning dims appended to the gym features as network input:
# skill (0-1000; 1000 = full strength) and aggression (0-1000; 500 =
# balanced). The world features come from Rust; the knobs are the
# bot's own configuration, so the wrapper appends them.
CONDITION_DIMS = 2
NET_FEATURES = FEATURES + CONDITION_DIMS

DRAW_REWARD = -0.3
STEP_COST = 1e-4
# Decision stride in sim ticks. 16 halves the credit-assignment horizon
# relative to the bots' own 8 — macro decisions don't need finer.
CADENCE = 16

# Hand-set scales per feature index (see sim/src/bot/gym.rs for the
# layout). Order: tick, scrap, my H/S/Sc/L, turrets, fab, foundry hp,
# idle fighters, armies, staging size, army state, enemy H/S/Sc/L,
# enemy buildings, enemy turrets, enemy foundry known, my strength,
# army strength, enemy strength, home x/y, enemy x/y, intel age,
# remembered enemy strength, ticks since enemy seen, last seen x/y.
SCALES = np.array(
    [
        40_000,
        500,
        8,
        20,
        10,
        10,
        4,
        1,
        800,
        20,
        4,
        20,
        4,
        8,
        20,
        10,
        10,
        8,
        4,
        1,
        500,
        500,
        500,
        48,
        32,
        48,
        32,
        10_000,
        500,
        10_000,
        48,
        32,
    ],
    dtype=np.float32,
)
if SCALES.shape != (FEATURES,):
    raise RuntimeError("SCALES must cover every gym feature")


def normalize(features: list[int]) -> np.ndarray:
    return np.asarray(features, dtype=np.float32) / SCALES


def with_condition(obs: np.ndarray, condition: tuple[int, int]) -> np.ndarray:
    """Appends normalized (skill, aggression) knobs to a feature row."""
    knobs = np.asarray(condition, dtype=np.float32) / 1000.0
    return np.concatenate([obs, knobs])


@dataclass
class SeatView:
    obs: np.ndarray
    mask: np.ndarray
    raw: list[int]


@dataclass
class Frame:
    done: bool
    tick: int
    winner: int | None = None  # seat number, or None for a draw/cap
    alive: list[int] | None = None  # controlled seats still standing
    seats: dict[int, SeatView] = field(default_factory=dict)

    def reward(self, seat: int) -> float:
        """Terminal reward for `seat` (call when done). Elimination in a
        multiplayer game is a loss even if the game rages on."""
        if self.winner is not None:
            return 1.0 if self.winner == seat else -1.0
        if self.alive is not None and seat not in self.alive:
            return -1.0
        return DRAW_REWARD


class Worker:
    """One driver process, one live episode at a time."""

    def __init__(self, driver_bin: str) -> None:
        self.conditions: dict[int, tuple[int, int]] = {}
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

    def _rpc(self, request: dict) -> Frame:
        self._stdin.write(json.dumps(request) + "\n")
        reply = json.loads(self._stdout.readline())
        if "error" in reply:
            raise RuntimeError(reply["error"])
        if reply["done"]:
            return Frame(True, reply["tick"], reply["winner"], reply.get("alive"))
        frame = Frame(False, reply["tick"])
        for s in reply["seats"]:
            seat = s["seat"]
            obs = normalize(s["features"])
            cond = self.conditions.get(seat)
            if cond is not None:
                obs = with_condition(obs, cond)
            frame.seats[seat] = SeatView(
                obs,
                np.asarray(s["mask"], dtype=bool),
                s["features"],
            )
        return frame

    def reset(
        self,
        seed: int,
        control: tuple[int, ...] = (0,),
        tier: str = "veteran",
        max_ticks: int = 40_000,
        cadence: int = CADENCE,
        scenario: str | None = None,
        conditions: dict[int, tuple[int, int]] | None = None,
    ) -> Frame:
        self.control = control
        self.conditions = conditions or {}
        req = {
            "cmd": "reset",
            "seed": seed,
            "control": list(control),
            "tier": tier,
            "max_ticks": max_ticks,
            "cadence": cadence,
        }
        if scenario:
            req["scenario"] = scenario
        return self._rpc(req)

    def step(self, actions: dict[int, int]) -> Frame:
        """Actions keyed by seat (must cover every controlled seat)."""
        ordered = [int(actions[s]) for s in self.control]
        return self._rpc({"cmd": "step", "actions": ordered})

    def close(self) -> None:
        with contextlib.suppress(OSError, ValueError):
            self._stdin.write('{"cmd":"quit"}\n')
            self._stdin.flush()
        self.proc.terminate()
