from __future__ import annotations

from typing import TYPE_CHECKING

import numpy as np
import torch

import tournament
from oxide_gym import ACTIONS, FEATURES, ActionPlan, FactionName, Frame, SeatView

if TYPE_CHECKING:
    from collections.abc import Sequence


class FixedPolicy(torch.nn.Module):
    def forward(
        self,
        obs: torch.Tensor,
        _mask: torch.Tensor,
    ) -> tuple[torch.Tensor, torch.Tensor]:
        logits = torch.zeros((obs.shape[0], ACTIONS), dtype=torch.float32)
        logits[:, 2] = 3.0
        logits[:, 10] = 3.0
        logits[:, 17] = 3.0
        return logits, torch.zeros(obs.shape[0], dtype=torch.float32)


class OneStepWorker:
    def __init__(self) -> None:
        self.cadence = 0
        self.actions: dict[int, ActionPlan] = {}

    def reset(
        self,
        seed: int,
        control: tuple[int, ...] = (0,),
        tier: str = "veteran",
        max_ticks: int = 40_000,
        cadence: int = 16,
        scenario: str | None = None,
        conditions: dict[int, tuple[int, ...]] | None = None,
        factions: str | Sequence[FactionName] | None = None,
    ) -> Frame:
        _ = (seed, control, tier, max_ticks, scenario, conditions, factions)
        self.cadence = cadence
        view = SeatView(
            obs=np.zeros(FEATURES + 7, dtype=np.float32),
            mask=np.ones(ACTIONS, dtype=np.bool_),
            raw=[0] * FEATURES,
        )
        return Frame(done=False, tick=0, seats={0: view}, alive=[0])

    def step(self, actions: dict[int, ActionPlan]) -> Frame:
        self.actions = actions
        return Frame(done=True, tick=1, winners=[0], alive=[0])


def test_tournament_keeps_policy_condition_separate_from_execution() -> None:
    worker = OneStepWorker()
    won, ticks = tournament.play(
        FixedPolicy(),
        worker,
        "scrapheap",
        seed=7,
        seat=0,
        condition=(1000, 550),
        hesitation_permille=0,
        cadence=28,
    )

    assert won is True
    assert ticks == 1
    assert worker.cadence == 28
    assert worker.actions == {0: (2, 10, 17)}


def test_tournament_applies_exact_hesitation_not_policy_skill() -> None:
    worker = OneStepWorker()
    tournament.play(
        FixedPolicy(),
        worker,
        "scrapheap",
        seed=7,
        seat=0,
        condition=(620, 300),
        hesitation_permille=1000,
        cadence=36,
    )

    assert worker.cadence == 36
    assert worker.actions == {0: (0, 24, 25)}
