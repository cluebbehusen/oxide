"""Vectorized wrapper over `oxide-driver gym` subprocesses.

Each worker is one driver process serving sequential episodes over
stdio. Features arrive as raw integers (the Rust side is the source of
truth for their meaning); `normalize` scales them to roughly [-1, 1]
for the network. Rewards are terminal: +1 win, -1 loss, DRAW_REWARD at
the tick cap — plus a tiny per-decision cost so faster wins score
better.
"""

from __future__ import annotations

import json
import subprocess
from dataclasses import dataclass

import numpy as np

FEATURES = 28
ACTIONS = 11
GYM_VERSION = 1

DRAW_REWARD = -0.3
STEP_COST = 1e-4

# Hand-set scales per feature index (see sim/src/bot/gym.rs for the
# layout). Order: tick, scrap, my H/S/Sc/L, turrets, fab, foundry hp,
# idle fighters, armies, staging size, army state, enemy H/S/Sc/L,
# enemy buildings, enemy turrets, enemy foundry known, my strength,
# army strength, enemy strength, home x/y, enemy x/y, intel age.
SCALES = np.array(
    [
        40_000, 500, 8, 20, 10, 10, 4, 1, 800, 20, 4, 20, 4,
        8, 20, 10, 10, 8, 4, 1, 500, 500, 500, 48, 32, 48, 32, 10_000,
    ],
    dtype=np.float32,
)
assert SCALES.shape == (FEATURES,)


def normalize(features: list[int]) -> np.ndarray:
    return np.asarray(features, dtype=np.float32) / SCALES


@dataclass
class StepResult:
    obs: np.ndarray | None
    mask: np.ndarray | None
    reward: float
    done: bool
    win: bool | None
    ticks: int
    raw: list[int] | None = None


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

    def _rpc(self, request: dict) -> dict:
        self.proc.stdin.write(json.dumps(request) + "\n")
        reply = json.loads(self.proc.stdout.readline())
        if "error" in reply:
            raise RuntimeError(reply["error"])
        return reply

    def reset(self, seed: int, seat: int, tier: str, max_ticks: int = 40_000) -> StepResult:
        reply = self._rpc(
            {"cmd": "reset", "seed": seed, "seat": seat, "tier": tier, "max_ticks": max_ticks}
        )
        return self._wrap(reply)

    def step(self, action: int) -> StepResult:
        return self._wrap(self._rpc({"cmd": "step", "action": int(action)}))

    @staticmethod
    def _wrap(reply: dict) -> StepResult:
        if reply["done"]:
            win = reply["win"]
            reward = 1.0 if win is True else -1.0 if win is False else DRAW_REWARD
            return StepResult(None, None, reward, True, win, reply["tick"])
        return StepResult(
            normalize(reply["features"]),
            np.asarray(reply["mask"], dtype=bool),
            -STEP_COST,
            False,
            None,
            reply["tick"],
            raw=reply["features"],
        )

    def close(self):
        try:
            self.proc.stdin.write('{"cmd":"quit"}\n')
            self.proc.stdin.flush()
        except Exception:
            pass
        self.proc.terminate()
