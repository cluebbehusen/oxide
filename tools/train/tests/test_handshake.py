"""The Rust/Python gym contract, exercised for real: spawn the driver,
shake hands, reset one episode, take one legal masked action, step.

``Worker.__init__`` is the contract assertion — gym version, tensor
shapes, ordered feature and condition names, and the Rust-authored
profile catalog all verify at hello, so a column drift dies here instead
of in a silently mistrained run. The rest of the test proves the loop
actually turns.

Skipped unless ``OXIDE_DRIVER_BIN`` points at a built ``oxide-driver``:
a local ``uv run pytest`` must not require a Rust build. CI builds the
driver (no macroquad, so no display or system libraries) and sets the
variable.
"""

import os

import pytest

from oxide_gym import (
    ACTION_HEADS,
    ACTIONS,
    CONDITION_DIMS,
    FEATURES,
    Worker,
    condition_from_profile,
    normalize_factions,
)

pytestmark = pytest.mark.skipif(
    "OXIDE_DRIVER_BIN" not in os.environ,
    reason="OXIDE_DRIVER_BIN not set (CI wires it to a built oxide-driver)",
)


def test_the_handshake_and_one_masked_step() -> None:
    worker = Worker(os.environ["OXIDE_DRIVER_BIN"])
    try:
        assert worker.supports_effect_telemetry
        assert len(worker.profile_catalog.profiles) == 9
        profile = worker.profile_catalog.profiles[0]
        named = worker.named_condition(
            profile.style,
            profile.variant,
            worker.profile_catalog.default_role,
            "ferrous",
        )
        assert len(named) == CONDITION_DIMS
        assert named[1] == profile.aggression
        assert named[2] == 0
        frame = worker.reset(seed=11, max_ticks=200, conditions={0: named})
        assert not frame.done
        assert frame.tick == 0
        (seat,) = worker.control
        assert frame.effects[seat].unit_hp_restored == 0
        assert frame.effects[seat].buildings_salvaged == 0
        assert frame.effects[seat].buildings_completed == ()
        view = frame.seats[seat]
        assert len(view.raw) == FEATURES
        assert len(view.obs) == FEATURES + CONDITION_DIMS
        assert view.mask.shape == (ACTIONS,)
        assert view.mask.any(), "at least one action must be legal at tick 0"

        plan = (
            next(action for action in ACTION_HEADS[0] if view.mask[action]),
            next(action for action in ACTION_HEADS[1] if view.mask[action]),
            next(action for action in ACTION_HEADS[2] if view.mask[action]),
            next(action for action in ACTION_HEADS[3] if view.mask[action]),
        )
        after = worker.step({seat: plan})
        assert after.tick > frame.tick
        assert after.done or seat in after.seats
        assert seat in after.effects
    finally:
        worker.close()


def test_reset_retints_every_faction_pair_and_conditions_follow_rust() -> None:
    worker = Worker(os.environ["OXIDE_DRIVER_BIN"])
    try:
        for i, code in enumerate(("ff", "fc", "cf", "cc")):
            expected = normalize_factions(code)
            frame = worker.reset(
                seed=100 + i,
                control=(0, 1),
                max_ticks=200,
                factions=code,
                # Deliberately lie in both directions. The wrapper must
                # replace the faction knob from Rust's observation.
                conditions={
                    0: condition_from_profile(1000, 500, 1000),
                    1: condition_from_profile(1000, 500, 0),
                },
            )
            assert frame.factions == expected
            for seat, faction in enumerate(expected):
                view = frame.seats[seat]
                knob = 1000 if faction == "cupric" else 0
                assert view.faction == faction
                assert view.faction_knob == knob
                assert worker.conditions[seat][2] == knob
                assert view.obs[FEATURES + 2] == knob / 1000
    finally:
        worker.close()
