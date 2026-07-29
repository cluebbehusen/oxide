"""The Rust/Python gym contract, exercised for real: spawn the driver,
shake hands, reset one episode, take one legal masked action, step.

``Worker.__init__`` is the contract assertion — gym version, feature
count, action count, and the complete feature-name list all verify at
hello, so a column drift dies here instead of in a silently mistrained
run. The rest of the test proves the loop actually turns.

Skipped unless ``OXIDE_DRIVER_BIN`` points at a built ``oxide-driver``:
a local ``uv run pytest`` must not require a Rust build. CI builds the
driver (no macroquad, so no display or system libraries) and sets the
variable.
"""

import os

import pytest

from oxide_gym import ACTIONS, FEATURES, Worker, normalize_factions

pytestmark = pytest.mark.skipif(
    "OXIDE_DRIVER_BIN" not in os.environ,
    reason="OXIDE_DRIVER_BIN not set (CI wires it to a built oxide-driver)",
)


def test_the_handshake_and_one_masked_step() -> None:
    worker = Worker(os.environ["OXIDE_DRIVER_BIN"])
    try:
        assert worker.supports_effect_telemetry
        frame = worker.reset(seed=11, max_ticks=200)
        assert not frame.done
        assert frame.tick == 0
        (seat,) = worker.control
        assert frame.effects[seat].unit_hp_restored == 0
        assert frame.effects[seat].buildings_completed == ()
        view = frame.seats[seat]
        assert len(view.raw) == FEATURES
        assert view.mask.shape == (ACTIONS,)
        assert view.mask.any(), "at least one action must be legal at tick 0"

        legal = int(view.mask.argmax())
        after = worker.step({seat: legal})
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
                # replace this final knob from Rust's observation.
                conditions={
                    0: (1000, 500, 1000),
                    1: (1000, 500, 0),
                },
            )
            assert frame.factions == expected
            for seat, faction in enumerate(expected):
                view = frame.seats[seat]
                knob = 1000 if faction == "cupric" else 0
                assert view.faction == faction
                assert view.faction_knob == knob
                assert worker.conditions[seat][-1] == knob
                assert view.obs[-1] == knob / 1000
    finally:
        worker.close()
