from __future__ import annotations

from typing import TYPE_CHECKING

import numpy as np
import torch

import tournament
from oxide_gym import (
    ACTIONS,
    FEATURES,
    NET_FEATURES,
    ActionPlan,
    CanonicalProfile,
    FactionName,
    Frame,
    ProfileCatalog,
    SeatView,
    condition_from_profile,
)

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


def profile_catalog() -> ProfileCatalog:
    roles = ("generalist", "vanguard")
    profile = CanonicalProfile("balanced", 0, "ground-combined", 500, roles)
    values: dict[tuple[str, int, str, FactionName], tuple[int, ...]] = {}
    for role, facets in (
        ("generalist", (500, 300, 400, 500, 500)),
        ("vanguard", (500, 300, 400, 450, 650)),
    ):
        for faction_name, faction in (("ferrous", 0), ("cupric", 1000)):
            condition = list(condition_from_profile(1000, 500, faction))
            condition[-5:] = facets
            values[("balanced", 0, role, faction_name)] = tuple(condition)
    return ProfileCatalog((profile,), values, "generalist")


class OneStepWorker:
    def __init__(self) -> None:
        self.cadence = 0
        self.actions: dict[int, ActionPlan] = {}
        self.conditions: dict[int, tuple[int, ...]] = {}
        self.profile_catalog = profile_catalog()

    def reset(
        self,
        seed: int,
        control: tuple[int, ...] = (0,),
        max_ticks: int = 40_000,
        cadence: int = 16,
        scenario: str | None = None,
        conditions: dict[int, tuple[int, ...]] | None = None,
        factions: str | Sequence[FactionName] | None = None,
    ) -> Frame:
        _ = (seed, control, max_ticks, scenario, factions)
        self.cadence = cadence
        self.conditions = {} if conditions is None else conditions
        view = SeatView(
            obs=np.zeros(NET_FEATURES, dtype=np.float32),
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
        "overseer",
        seed=7,
        seat=0,
        condition=(1000, 550),
        hesitation_permille=0,
        cadence=28,
    )

    assert won is True
    assert ticks == 1
    assert worker.cadence == 28
    assert worker.actions == {0: (2, 10, 42, 17)}


def test_tournament_applies_exact_hesitation_not_policy_skill() -> None:
    worker = OneStepWorker()
    tournament.play(
        FixedPolicy(),
        worker,
        "overseer",
        seed=7,
        seat=0,
        condition=(620, 300),
        hesitation_permille=1000,
        cadence=36,
    )

    assert worker.cadence == 36
    assert worker.actions == {0: (0, 24, 42, 25)}


def test_tournament_default_consumes_named_profile_and_specialist_role() -> None:
    worker = OneStepWorker()
    won, _ticks = tournament.play(
        FixedPolicy(),
        worker,
        "overseer",
        seed=7,
        seat=0,
        role="vanguard",
    )

    assert won is True
    assert worker.conditions[0][-5:] == (500, 300, 400, 450, 650)
    assert worker.conditions[1][-5:] == (500, 300, 400, 500, 500)
