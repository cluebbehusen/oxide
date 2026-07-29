"""Tests for ``league``: the faction convention, the per-episode condition
sampler, and the frozen-view padding path in rollout.

When one learner seat dies mid-episode while its teammate plays on, the
dead seat's lane keeps producing rows (so the batch stays rectangular for
GAE) but those rows must be flagged invalid and dropped before the PPO
update learns from them. The rollout tests drive the real Job/Lane/
seat_view machinery and a real (tiny) policy; only the subprocess Worker
is scripted."""

import itertools
import json
import pathlib
import sys
from typing import TYPE_CHECKING, cast

import numpy as np
import pytest
import torch

from league import (
    BUILD_BAY_ACTION,
    FAB_BUILT,
    REPAIR_ACTION,
    SHAPE_K,
    TEL,
    F,
    Job,
    comp_entropy,
    composition_probe,
    faction_knob,
    probe_canary,
    rollout,
    sample_condition,
    tech_bonus_at,
)
from models import make_policy
from oxide_gym import ACTIONS, FEATURE_NAMES, FEATURES, NET_FEATURES, Frame, SeatView

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


class TestTechBonusSchedule:
    def test_full_at_the_runs_first_update(self) -> None:
        assert tech_bonus_at(0.08, 0, 1000) == pytest.approx(0.08)

    def test_halfway_through_pays_half(self) -> None:
        assert tech_bonus_at(0.08, 500, 1000) == pytest.approx(0.04)

    def test_zero_at_and_past_the_span(self) -> None:
        # The anneal must actually reach zero — a floor would let the
        # shaped incentive be farmed forever instead of handing the
        # argument back to winning.
        assert tech_bonus_at(0.08, 1000, 1000) == 0.0
        assert tech_bonus_at(0.08, 1500, 1000) == 0.0

    def test_disabled_base_or_span_pays_nothing(self) -> None:
        assert tech_bonus_at(0.0, 0, 1000) == 0.0
        assert tech_bonus_at(0.08, 0, 0) == 0.0


def _tech_view(fab: int) -> SeatView:
    view = _view(0.5)
    view.raw[FAB_BUILT] = fab
    return view


class _ResettingWorker(_ScriptedWorker):
    """A scripted worker whose reset serves a fresh two-seat frame — for
    rollouts that cross an episode boundary."""

    def __init__(self, frames: list[Frame], reset_frame: Frame) -> None:
        super().__init__(frames)
        self._reset_frame = reset_frame

    def reset(self, *_args: object, **_kwargs: object) -> Frame:
        return self._reset_frame


class TestOwnTechShaping:
    """The round-6 shaping: a terminal bonus for having stood a
    Fabricator this episode. Own-state only (fog-safe by construction),
    paid on top of the game outcome, invisible to telemetry finals."""

    def _rollout_two_episodes(self) -> tuple[np.ndarray, list[float]]:
        # Episode 1: seat 0 stands a fab mid-episode and LOSES; seat 1
        # never techs and wins. Episode 2: nobody techs, seat 0 wins.
        frames = [
            Frame(False, 16, seats={0: _tech_view(1), 1: _tech_view(0)}),
            Frame(True, 32, winners=[1]),
            Frame(False, 64, seats={0: _tech_view(0), 1: _tech_view(0)}),
            Frame(True, 80, winners=[0]),
        ]
        fresh = Frame(False, 48, seats={0: _tech_view(0), 1: _tech_view(0)})
        worker = cast("Worker", _ResettingWorker(frames, fresh))
        job = Job(worker, "self", 0, pathlib.Path("."), np.random.default_rng(0), "cpu")
        job.frame = Frame(False, 0, seats={0: _tech_view(0), 1: _tech_view(0)})
        job.conditions = {0: (1000, 500, 0), 1: (1000, 500, 1000)}
        torch.manual_seed(0)
        policy = make_policy("mlp")
        policy.eval()
        TEL.clear()
        batch, _last_val, finals = rollout(
            policy, [job], itertools.repeat(0), 4, "cpu", tech_bonus=0.05
        )
        rew = batch[5]
        return rew, finals

    def test_the_bonus_pays_the_teched_seat_even_in_defeat(self) -> None:
        rew, _finals = self._rollout_two_episodes()
        # rew is [step, lane]; lanes follow learner_seats order, so
        # column 0 = seat 0. Episode 1's terminal lands at step 1.
        assert rew[1][0] == pytest.approx(-1.0 + 0.05), "teched loser"
        assert rew[1][1] == pytest.approx(1.0), "untech'd winner gets no bonus"

    def test_the_flag_resets_at_the_episode_boundary(self) -> None:
        rew, _finals = self._rollout_two_episodes()
        # Episode 2's winner is the seat that teched in episode 1; its
        # terminal must be the bare outcome — stale flags would let one
        # early fab pay rent forever.
        assert rew[3][0] == pytest.approx(1.0)
        assert rew[3][1] == pytest.approx(-1.0)

    def test_telemetry_finals_stay_the_pure_outcome(self) -> None:
        _rew, finals = self._rollout_two_episodes()
        assert sorted(finals) == [-1.0, -1.0, 1.0, 1.0], (
            "avg_final must compare across runs with and without shaping"
        )
        assert TEL["ep_teched"] == 1, "exactly one teched episode-seat"


class TestCompEntropy:
    def test_a_single_kind_army_scores_zero_bits(self) -> None:
        raw = [0] * FEATURES
        raw[FEATURE_NAMES.index("my_sentinels")] = 12
        assert comp_entropy(raw) == 0.0

    def test_two_equal_value_kinds_score_one_bit(self) -> None:
        # 11 sentinels (990) vs 9 lancers (990): a perfect two-way split
        # by cost weight.
        raw = [0] * FEATURES
        raw[FEATURE_NAMES.index("my_sentinels")] = 11
        raw[FEATURE_NAMES.index("my_lancers")] = 9
        assert abs(comp_entropy(raw) - 1.0) < 1e-9

    def test_an_empty_army_scores_zero_not_nan(self) -> None:
        assert comp_entropy([0] * FEATURES) == 0.0


class TestMixBonus:
    def test_the_terminal_pays_by_own_mix_entropy(self) -> None:
        # Seat 0 ends with a perfect two-way mix (1 bit -> half the
        # bonus); seat 1 ends with a sentinel monoculture (nothing).
        mixed = _view(0.5)
        mixed.raw[FEATURE_NAMES.index("my_sentinels")] = 11
        mixed.raw[FEATURE_NAMES.index("my_lancers")] = 9
        mono = _view(0.5)
        mono.raw[FEATURE_NAMES.index("my_sentinels")] = 20
        frames = [
            Frame(False, 16, seats={0: mixed, 1: mono}),
            Frame(True, 32, winners=[0]),
        ]
        fresh = Frame(False, 48, seats={0: _view(0.1), 1: _view(0.1)})
        worker = cast("Worker", _ResettingWorker(frames, fresh))
        job = Job(worker, "self", 0, pathlib.Path("."), np.random.default_rng(0), "cpu")
        job.frame = Frame(False, 0, seats={0: _view(0.1), 1: _view(0.1)})
        job.conditions = {0: (1000, 500, 0), 1: (1000, 500, 1000)}
        torch.manual_seed(0)
        policy = make_policy("mlp")
        policy.eval()
        batch, _last_val, finals = rollout(
            policy, [job], itertools.repeat(0), 2, "cpu", mix_bonus=0.1
        )
        rew = batch[5]
        assert rew[1][0] == pytest.approx(1.0 + 0.05), "1 bit earns half of 0.1"
        assert rew[1][1] == pytest.approx(-1.0), "a monoculture earns nothing"
        assert sorted(finals) == [-1.0, 1.0], "telemetry finals stay pure"


def _forced_view(action: int) -> SeatView:
    """A view whose mask permits exactly one action — the policy has no
    choice, so the rollout's executed action is the test's choice."""
    view = _view(0.5)
    view.mask = np.zeros(ACTIONS, dtype=bool)
    view.mask[action] = True
    return view


class TestRepairBonus:
    """The v6 seeding: one --repair-bonus flag pays each weld verb the
    seat picked this episode, independently — tracked like Salvage,
    annealed on the same schedule, invisible to telemetry finals."""

    def _rollout(
        self, initial: Frame, frames: list[Frame], steps: int
    ) -> tuple[np.ndarray, list[float]]:
        fresh = Frame(False, 99, seats={0: _forced_view(0), 1: _forced_view(0)})
        worker = cast("Worker", _ResettingWorker(frames, fresh))
        job = Job(worker, "self", 0, pathlib.Path("."), np.random.default_rng(0), "cpu")
        job.frame = initial
        job.conditions = {0: (1000, 500, 0), 1: (1000, 500, 1000)}
        torch.manual_seed(0)
        policy = make_policy("mlp")
        policy.eval()
        TEL.clear()
        batch, _last_val, finals = rollout(
            policy, [job], itertools.repeat(0), steps, "cpu", repair_bonus=0.05
        )
        return batch[5], finals

    def test_each_weld_verb_pays_the_bonus_once(self) -> None:
        initial = Frame(
            False,
            0,
            seats={
                0: _forced_view(REPAIR_ACTION),
                1: _forced_view(BUILD_BAY_ACTION),
            },
        )
        rew, finals = self._rollout(initial, [Frame(True, 16, winners=[0])], 1)
        assert rew[0][0] == pytest.approx(1.0 + 0.05), "the field weld pays"
        assert rew[0][1] == pytest.approx(-1.0 + 0.05), "the Bay pays, even in defeat"
        assert sorted(finals) == [-1.0, 1.0], "telemetry finals stay pure"
        assert TEL["ep_repair"] == 1
        assert TEL["ep_bay"] == 1

    def test_a_seat_that_picked_both_verbs_earns_both(self) -> None:
        initial = Frame(
            False, 0, seats={0: _forced_view(REPAIR_ACTION), 1: _forced_view(0)}
        )
        frames = [
            Frame(
                False,
                16,
                seats={0: _forced_view(BUILD_BAY_ACTION), 1: _forced_view(0)},
            ),
            Frame(True, 32, winners=[0]),
        ]
        rew, _finals = self._rollout(initial, frames, 2)
        assert rew[1][0] == pytest.approx(1.0 + 0.10), "both verbs, both bonuses"
        assert rew[1][1] == pytest.approx(-1.0), "an idle seat earns nothing"

    def test_the_flags_reset_at_the_episode_boundary(self) -> None:
        initial = Frame(
            False,
            0,
            seats={0: _forced_view(REPAIR_ACTION), 1: _forced_view(REPAIR_ACTION)},
        )
        frames = [Frame(True, 16, winners=[0]), Frame(True, 32, winners=[0])]
        rew, _finals = self._rollout(initial, frames, 2)
        assert rew[0][0] == pytest.approx(1.0 + 0.05)
        # Episode 2 picked nothing (the fresh frame forces Idle); a
        # stale flag would let one early weld pay rent forever.
        assert rew[1][0] == pytest.approx(1.0)
        assert rew[1][1] == pytest.approx(-1.0)


_PROBE_PAYLOAD = {
    "schema": 2,
    "overall": {"matches": 50, "decided": 41, "capped": 9},
    "decided": {
        "seats": 82,
        "entropy_bits": 2.134,
        "seat_entropy": {"mean": 1.9, "p10": 1.42, "p25": 1.7, "median": 2.0},
        "mean_share": {"lancer": 0.599, "sentinel": 0.401},
        "mean_buildings": {"fabricator": 1.2},
        "seats_with_building": {"fabricator": 0.85, "repairbay": 0.125},
    },
}


class TestProbeCanary:
    def test_the_row_reads_decisiveness_mix_and_both_share_tables(self) -> None:
        assert probe_canary(_PROBE_PAYLOAD) == {
            "matches": 50,
            "decided": 41,
            "capped": 9,
            "entropy_bits": 2.13,
            "seat_p10": 1.42,
            "unit_share": {"lancer": 0.599, "sentinel": 0.401},
            "building_share": {"fabricator": 0.85, "repairbay": 0.125},
        }

    def test_an_empty_cohort_reports_no_p10_instead_of_crashing(self) -> None:
        payload = json.loads(json.dumps(_PROBE_PAYLOAD))
        payload["decided"]["seat_entropy"] = None
        assert probe_canary(payload)["seat_p10"] is None


class TestCompositionProbe:
    """The in-loop canary's plumbing: snapshot -> Q12 export -> a
    balance-probe subprocess -> the canary row. The driver is a stub
    that records its argv and writes a canned payload, so the test
    proves the wiring without a Rust build."""

    def _fake_driver(self, tmp_path: pathlib.Path, payload: dict) -> pathlib.Path:
        script = tmp_path / "fake-driver"
        script.write_text(
            f"#!{sys.executable}\n"
            "import json, sys\n"
            "opts = dict(zip(sys.argv[2::2], sys.argv[3::2]))\n"
            "with open(opts['--out'] + '.argv', 'w') as f:\n"
            "    json.dump(sys.argv[1:], f)\n"
            "with open(opts['--out'], 'w') as f:\n"
            f"    json.dump({payload!r}, f)\n"
        )
        script.chmod(0o755)
        return script

    def test_the_probe_snapshots_exports_and_reads_the_canary(
        self, tmp_path: pathlib.Path
    ) -> None:
        driver = self._fake_driver(tmp_path, _PROBE_PAYLOAD)
        torch.manual_seed(0)
        policy = make_policy("mlp")
        row = composition_probe(
            policy, "mlp", 100, tmp_path, str(driver), "some/scenarios", "medium", 2
        )
        assert row["decided"] == 41
        assert row["seat_p10"] == 1.42
        probe_dir = tmp_path / "probe"
        assert (probe_dir / "ckpt-00100.pt").exists(), "the snapshot persists"
        weights = json.loads((probe_dir / "weights-00100.json").read_text())
        assert weights["actions"] == ACTIONS, "the export speaks the live contract"
        argv = json.loads((probe_dir / "probe-00100.json.argv").read_text())
        assert argv[0] == "balance-probe"
        opts = dict(zip(argv[1::2], argv[2::2], strict=True))
        assert opts["--dir"] == "some/scenarios"
        assert opts["--level"] == "medium"
        assert opts["--seeds"] == "2"
        assert opts["--weights"] == str(probe_dir / "weights-00100.json")

    def test_an_old_probe_schema_is_refused(self, tmp_path: pathlib.Path) -> None:
        stale = json.loads(json.dumps(_PROBE_PAYLOAD))
        stale["schema"] = 1
        driver = self._fake_driver(tmp_path, stale)
        torch.manual_seed(0)
        policy = make_policy("mlp")
        with pytest.raises(RuntimeError, match="schema 1"):
            composition_probe(policy, "mlp", 5, tmp_path, str(driver), "s", "medium", 1)


class TestTerminalObservations:
    """v5: done frames carry observations for living seats, and the
    tech bonus pays the TERMINAL frame's fab_built — a Fabricator lost
    (or sold) before the end earns nothing, however long it stood."""

    def _run(self, frames: list[Frame], seats: int = 2, steps: int = 2) -> np.ndarray:
        fresh = Frame(False, 99, seats={s: _tech_view(0) for s in range(seats)})
        worker = cast("Worker", _ResettingWorker(frames, fresh))
        job = Job(worker, "self", 0, pathlib.Path("."), np.random.default_rng(0), "cpu")
        job.frame = Frame(False, 0, seats={s: _tech_view(0) for s in range(seats)})
        job.conditions = {s: (1000, 500, faction_knob(s)) for s in range(seats)}
        torch.manual_seed(0)
        policy = make_policy("mlp")
        policy.eval()
        TEL.clear()
        batch, _last_val, _finals = rollout(
            policy, [job], itertools.repeat(0), steps, "cpu", tech_bonus=0.05
        )
        return batch[5]

    def test_terminal_evidence_outranks_the_run_of_play(self) -> None:
        # Seat 0 stood a fab mid-episode but the TERMINAL frame says
        # it's gone (killed or salvaged): no bonus. Seat 1 shows the
        # reverse: no fab all game, one standing at the end: bonus.
        frames = [
            Frame(False, 16, seats={0: _tech_view(1), 1: _tech_view(0)}),
            Frame(
                True,
                32,
                winners=[1],
                seats={0: _tech_view(0), 1: _tech_view(1)},
            ),
        ]
        rew = self._run(frames)
        assert rew[1][0] == pytest.approx(-1.0), "a lost fab earns nothing"
        assert rew[1][1] == pytest.approx(1.0 + 0.05), "a standing fab pays"

    def test_the_terminal_step_still_prices_the_potential(self) -> None:
        # A salvage (or an army loss) landing on the terminal cadence
        # must not escape the building-value shaping: seat 0 carries
        # standing value mid-episode that is GONE on the final frame,
        # and the terminal reward carries the negative delta the
        # nonterminal branch would have priced.
        rich = _tech_view(0)
        rich.raw[F["my_building_value"]] = 300
        frames = [
            Frame(False, 16, seats={0: rich, 1: _tech_view(0)}),
            Frame(
                True,
                32,
                winners=[1],
                seats={0: _tech_view(0), 1: _tech_view(0)},
            ),
        ]
        rew = self._run(frames)
        drop = SHAPE_K * (0.0 - (300 / 3.0) / 500.0)
        assert rew[1][0] == pytest.approx(-1.0 + drop), (
            "the final step's lost value is priced into the terminal reward"
        )
        assert rew[1][1] == pytest.approx(1.0), "a flat seat sees no delta"

    def test_a_dead_seat_settles_on_its_frozen_last_view(self) -> None:
        # Seat 1 dies mid-episode (drops from the frame) after standing
        # a fab; the terminal frame only carries the living seat 0. The
        # dead seat's bonus reads its frozen last view — fab standing
        # when it died — and nothing crashes on the missing terminal row.
        frames = [
            Frame(False, 16, seats={0: _tech_view(0), 1: _tech_view(1)}),
            Frame(False, 32, seats={0: _tech_view(0)}),  # seat 1 died
            Frame(True, 48, winners=[0], seats={0: _tech_view(0)}),
        ]
        rew = self._run(frames, steps=3)
        assert rew[2][0] == pytest.approx(1.0), "living seat: terminal says no fab"
        assert rew[2][1] == pytest.approx(-1.0 + 0.05), (
            "dead seat: the frozen last view carried its fab"
        )
