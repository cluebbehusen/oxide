"""Tests for ``league``: the faction convention, the per-episode condition
sampler, and the frozen-view padding path in rollout.

When one learner seat dies mid-episode while its teammate plays on, the
dead seat's lane keeps producing rows (so the batch stays rectangular for
GAE) but those rows must be flagged invalid and dropped before the PPO
update learns from them. The rollout tests drive the real Job/Lane/
seat_view machinery and a real (tiny) policy; only the subprocess Worker
is scripted."""

import itertools
import pathlib
from typing import TYPE_CHECKING, cast

import numpy as np
import pytest
import torch

from league import Job, faction_knob, rollout, sample_condition
from models import make_policy
from oxide_gym import ACTIONS, FEATURES, NET_FEATURES, Frame, SeatView

if TYPE_CHECKING:
    from collections.abc import Callable

    from oxide_gym import Worker


class _ScriptedWorker:
    """Returns a fixed sequence of frames; the Job's frame is preset so
    reset() is never reached. Implements the pipelined send/recv split
    the way the real Worker does, and journals every call into `log`
    (shared across workers when provided) so tests can prove ordering."""

    def __init__(
        self, frames: list[Frame], name: str = "w", log: list[str] | None = None
    ) -> None:
        self._frames = frames
        self._i = 0
        self._pending = False
        self.name = name
        self.log = log if log is not None else []

    def step(self, actions: dict[int, int]) -> Frame:
        self.send_step(actions)
        return self.recv()

    def send_step(self, _actions: dict[int, int]) -> None:
        assert not self._pending, "one request in flight per worker"
        self._pending = True
        self.log.append(f"send:{self.name}")

    def recv(self) -> Frame:
        assert self._pending, "recv without a send"
        self._pending = False
        self.log.append(f"recv:{self.name}")
        frame = self._frames[self._i]
        self._i += 1
        return frame

    def reset(self, *_args: object, **_kwargs: object) -> Frame:
        raise AssertionError("reset must not run: the Job's frame is preset")


def _view(fill: float) -> SeatView:
    obs = np.full(NET_FEATURES, fill, dtype=np.float32)
    mask = np.ones(ACTIONS, dtype=bool)
    return SeatView(obs, mask, [0] * FEATURES)


@pytest.fixture
def view() -> Callable[[float], SeatView]:
    """A SeatView factory: a constant-filled observation with every action
    legal, distinguishable by its fill value."""

    def make(fill: float) -> SeatView:
        obs = np.full(NET_FEATURES, fill, dtype=np.float32)
        mask = np.ones(ACTIONS, dtype=bool)
        return SeatView(obs, mask, [0] * FEATURES)

    return make


@pytest.fixture
def job_with_death(view: Callable[[float], SeatView]) -> Job:
    """A self-play Job (seats 0 and 1) whose seat 1 vanishes after the
    second step and never returns."""
    initial = Frame(False, 0, seats={0: view(1.0), 1: view(2.0)})
    step_frames = [
        Frame(False, 16, seats={0: view(1.1), 1: view(2.1)}),  # both alive
        Frame(False, 32, seats={0: view(1.2)}),  # seat 1 died here
        Frame(False, 48, seats={0: view(1.3)}),  # seat 1 padding
        Frame(False, 64, seats={0: view(1.4)}),  # seat 1 padding
    ]
    worker = cast("Worker", _ScriptedWorker(step_frames))
    job = Job(worker, "self", 0, pathlib.Path("."), np.random.default_rng(0), "cpu")
    job.frame = initial
    # sample_condition normally sets these; preset with skill 1000 so
    # maybe_blunder is a no-op and the rows equal the policy's intent.
    job.conditions = {0: (1000, 500, 0), 1: (1000, 500, 1000)}
    return job


@pytest.fixture
def death_batch(job_with_death: Job) -> tuple[np.ndarray, ...]:
    """The batch from a four-step rollout over the dying-teammate job."""
    torch.manual_seed(0)
    policy = make_policy("mlp")
    policy.eval()
    batch, _last_val, _finals = rollout(
        policy,
        [job_with_death],
        itertools.repeat(0),
        4,
        "cpu",
    )
    return batch


class TestFactionKnob:
    def test_even_seats_run_ferrous(self) -> None:
        assert faction_knob(0) == 0
        assert faction_knob(2) == 0

    def test_odd_seats_run_cupric(self) -> None:
        assert faction_knob(1) == 1000
        assert faction_knob(3) == 1000


class TestSampleCondition:
    def test_skill_comes_from_the_favor_the_strong_menu(self) -> None:
        rng = np.random.default_rng(7)
        for _ in range(500):
            skill, _, _ = sample_condition(rng, 0)
            assert skill in {400, 550, 700, 850, 1000}

    def test_aggression_spans_the_inclusive_knob_range(self) -> None:
        rng = np.random.default_rng(7)
        for _ in range(500):
            _, aggression, _ = sample_condition(rng, 0)
            assert 0 <= aggression <= 1000

    def test_faction_is_the_seats_honest_side_never_sampled(self) -> None:
        rng = np.random.default_rng(7)
        for seat in (0, 1, 2, 3):
            for _ in range(100):
                _, _, faction = sample_condition(rng, seat)
                assert faction == faction_knob(seat)

    def test_the_condition_is_exactly_a_three_tuple(self) -> None:
        cond = sample_condition(np.random.default_rng(0), 0)
        assert isinstance(cond, tuple)
        assert len(cond) == 3


class TestRollout:
    def test_the_batch_stays_rectangular_past_a_death(
        self, death_batch: tuple[np.ndarray, ...]
    ) -> None:
        # Four steps x two lanes, no lane truncated by the death — column 0
        # (seat 0) is the surviving teammate, column 1 (seat 1) died.
        obs_b, *_rest, valid_b = death_batch
        assert obs_b.shape == (4, 2, NET_FEATURES)
        assert valid_b.shape == (4, 2)

    def test_rows_collected_while_dead_flag_invalid(
        self, death_batch: tuple[np.ndarray, ...]
    ) -> None:
        # Seat 1 dies as the result of the second step; its first two
        # observations were live, the frozen-view padding after is not.
        # The surviving teammate is valid throughout.
        valid_b = death_batch[-1]
        np.testing.assert_array_equal(valid_b[:, 1], [True, True, False, False])
        np.testing.assert_array_equal(valid_b[:, 0], [True, True, True, True])

    def test_the_flat_filter_drops_exactly_the_padded_rows(
        self, death_batch: tuple[np.ndarray, ...]
    ) -> None:
        # The update path flattens row-major and keeps only valid rows; seat
        # 1 is column 1, so its padded rows sit at flat indices t*2 + 1 for
        # t in (2, 3).
        obs_b, *_rest, valid_b = death_batch
        rows = valid_b.reshape(-1)
        np.testing.assert_array_equal(np.where(~rows)[0], [5, 7])
        kept = obs_b.reshape(-1, NET_FEATURES)[rows]
        assert kept.shape[0] == 4 * 2 - 2

    def test_padding_freezes_the_last_view_at_zero_reward(
        self, death_batch: tuple[np.ndarray, ...]
    ) -> None:
        obs_b, _mask_b, _act_b, _logp_b, _val_b, rew_b, done_b, _valid_b = death_batch
        np.testing.assert_array_equal(obs_b[2, 1], obs_b[3, 1])
        assert rew_b[2, 1] == 0.0
        assert rew_b[3, 1] == 0.0
        assert not done_b[2, 1]


class TestJobSeatView:
    def test_a_live_view_is_returned_and_remembered(
        self, view: Callable[[float], SeatView]
    ) -> None:
        live = view(1.0)
        job = Job(
            cast("Worker", _ScriptedWorker([])),
            "self",
            0,
            pathlib.Path("."),
            np.random.default_rng(0),
            "cpu",
        )
        job.frame = Frame(False, 0, seats={0: live})
        assert job.seat_view(0) is live

    def test_a_dead_seat_serves_its_frozen_last_view(
        self, view: Callable[[float], SeatView]
    ) -> None:
        live = view(1.0)
        job = Job(
            cast("Worker", _ScriptedWorker([])),
            "self",
            0,
            pathlib.Path("."),
            np.random.default_rng(0),
            "cpu",
        )
        job.frame = Frame(False, 0, seats={0: live})
        job.seat_view(0)  # remembered while alive
        job.frame = Frame(False, 16, seats={})  # seat 0 no longer reported
        assert job.seat_view(0) is live  # frozen, not a KeyError


class TestRolloutPipelining:
    def test_every_send_lands_before_any_recv_within_a_step(self) -> None:
        # Two jobs sharing one journal: the pipelined loop must write
        # both workers' steps before it blocks on either reply, every
        # step — that concurrency is the entire point of the split.
        log: list[str] = []
        torch.manual_seed(0)
        policy = make_policy("mlp")
        policy.eval()

        def scripted(name: str) -> Job:
            frames = [
                Frame(False, 16 * (t + 1), seats={0: _view(1.0), 1: _view(2.0)})
                for t in range(3)
            ]
            worker = cast("Worker", _ScriptedWorker(frames, name, log))
            job = Job(
                worker, "self", 0, pathlib.Path("."), np.random.default_rng(0), "cpu"
            )
            job.frame = Frame(False, 0, seats={0: _view(1.0), 1: _view(2.0)})
            job.conditions = {0: (1000, 500, 0), 1: (1000, 500, 1000)}
            return job

        jobs = [scripted("a"), scripted("b")]
        rollout(policy, jobs, itertools.repeat(0), 3, "cpu")

        steps = [log[i : i + 4] for i in range(0, len(log), 4)]
        assert len(steps) == 3
        for chunk in steps:
            assert chunk == ["send:a", "send:b", "recv:a", "recv:b"], (
                f"pipelining broke: {chunk}"
            )
