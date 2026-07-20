"""Wrapper over `oxide-driver gym` subprocesses (protocol v2).

Each worker is one driver process serving sequential episodes over
stdio. `control` picks the externally-driven seats: `(0,)` against a
scripted tier, or `(0, 1)` for self-play/league — each frame then
carries features and a mask per controlled seat. Features arrive as
raw integers (the Rust side is the source of truth for their meaning);
`normalize` scales them to roughly [-1, 1] for the network.
"""

from __future__ import annotations

import json
import subprocess
from dataclasses import dataclass, field

import numpy as np

FEATURES = 32
ACTIONS = 11
GYM_VERSION = 2

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
        40_000, 500, 8, 20, 10, 10, 4, 1, 800, 20, 4, 20, 4,
        8, 20, 10, 10, 8, 4, 1, 500, 500, 500, 48, 32, 48, 32, 10_000,
        500, 10_000, 48, 32,
    ],
    dtype=np.float32,
)
assert SCALES.shape == (FEATURES,)


def normalize(features: list[int]) -> np.ndarray:
    return np.asarray(features, dtype=np.float32) / SCALES


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
    seats: dict[int, SeatView] = field(default_factory=dict)

    def reward(self, seat: int) -> float:
        """Terminal reward for `seat` (call when done)."""
        if self.winner is None:
            return DRAW_REWARD
        return 1.0 if self.winner == seat else -1.0


class Worker:
    """One driver process, one live episode at a time."""

    def __init__(self, driver_bin: str):
        self.proc = subprocess.Popen(
            [driver_bin, "gym"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        hello = json.loads(self.proc.stdout.readline())
        if not hello.get("ready"):
            raise RuntimeError(f"gym server failed to start: {hello}")
        if hello["version"] != GYM_VERSION or hello["features"] != FEATURES:
            raise RuntimeError(f"gym contract mismatch: {hello}")

    def _rpc(self, request: dict) -> Frame:
        self.proc.stdin.write(json.dumps(request) + "\n")
        reply = json.loads(self.proc.stdout.readline())
        if "error" in reply:
            raise RuntimeError(reply["error"])
        if reply["done"]:
            return Frame(True, reply["tick"], reply["winner"])
        frame = Frame(False, reply["tick"])
        for s in reply["seats"]:
            frame.seats[s["seat"]] = SeatView(
                normalize(s["features"]),
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
    ) -> Frame:
        self.control = control
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

    def close(self):
        try:
            self.proc.stdin.write('{"cmd":"quit"}\n')
            self.proc.stdin.flush()
        except Exception:
            pass
        self.proc.terminate()
