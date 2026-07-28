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

from oxide_gym import ACTIONS, FEATURES, Worker

pytestmark = pytest.mark.skipif(
    "OXIDE_DRIVER_BIN" not in os.environ,
    reason="OXIDE_DRIVER_BIN not set (CI wires it to a built oxide-driver)",
)


def test_the_handshake_and_one_masked_step() -> None:
    worker = Worker(os.environ["OXIDE_DRIVER_BIN"])
    try:
        frame = worker.reset(seed=11, max_ticks=200)
        assert not frame.done
        assert frame.tick == 0
        (seat,) = worker.control
        view = frame.seats[seat]
        assert len(view.raw) == FEATURES
        assert view.mask.shape == (ACTIONS,)
        assert view.mask.any(), "at least one action must be legal at tick 0"

        legal = int(view.mask.argmax())
        after = worker.step({seat: legal})
        assert after.tick > frame.tick
        assert after.done or seat in after.seats
    finally:
        worker.close()
