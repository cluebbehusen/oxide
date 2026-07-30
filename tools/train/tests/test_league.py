"""Tests for ``league``: the faction convention, the per-episode condition
sampler, and the frozen-view padding path in rollout.

When one learner seat dies mid-episode while its teammate plays on, the
dead seat's lane keeps producing rows (so the batch stays rectangular for
GAE) but those rows must be flagged invalid and dropped before the PPO
update learns from them. The rollout tests drive the real Job/Lane/
seat_view machinery and a real (tiny) policy; only the subprocess Worker
is scripted."""

import argparse
import itertools
import json
import pathlib
import sys
from collections import Counter
from typing import TYPE_CHECKING, cast

import numpy as np
import pytest
import torch

import league
import oxide_gym
from league import (
    DEFAULT_VALUE_WARMUP,
    FAB_BUILT,
    MAX_STYLE_BONUS,
    SHAPE_GAMMA,
    SHAPE_K,
    SHIPPED_AGGRESSION_DISTRIBUTION,
    SHIPPED_EXECUTION_PROFILES,
    TEL,
    TRAIN_GAMMA,
    EpisodeDials,
    ExecutionProfile,
    F,
    Job,
    ProfileCurriculum,
    add_entropy_arguments,
    add_initialization_arguments,
    allocate_role_counts,
    anchor_coefficient_at,
    assign_roles,
    bounded_entropy_coefficient,
    bounded_structure_bonus,
    bounded_style_coefficient,
    claim_fresh_run_directory,
    comp_entropy,
    composition_probe,
    effective_production_entropy_coefficient,
    expand_faction_pair,
    faction_knob,
    generated_map_families,
    legacy_incumbent_plan,
    load_incumbent_policy,
    maybe_blunder,
    parse_aggression_mix,
    parse_faction_mix,
    parse_map_mix,
    parse_opponent_mix,
    phase_interval_due,
    policy_skill_for_aggression,
    probe_canary,
    q12_initialization_provenance,
    realized_learner_row_mix,
    resolve_aggression_distribution,
    resolve_faction_mix,
    resolve_map_mix,
    resolve_training_aggression_distribution,
    resolved_value_warmup,
    rollout,
    sample_aggression,
    sample_condition,
    sample_faction_pair,
    sample_map_family,
    style_alignment,
    style_bonus,
    tech_bonus_at,
    training_world_inputs,
    unit_interval,
    value_warmup_active,
    warm_generated_maps,
)
from models import checkpoint_critic_ready, make_policy, save_policy
from oxide_gym import (
    ACTION_HEADS,
    ACTIONS,
    FEATURE_NAMES,
    FEATURES,
    GYM_VERSION,
    NET_FEATURES,
    ActionPlan,
    Frame,
    SeatEffects,
    SeatView,
    condition_from_profile,
)

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

    def step(self, actions: dict[int, ActionPlan]) -> Frame:
        self.send_step(actions)
        return self.recv()

    def send_step(self, _actions: dict[int, ActionPlan]) -> None:
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
    job.conditions = {
        0: condition_from_profile(1000, 500, 0),
        1: condition_from_profile(1000, 500, 1000),
    }
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
    def test_default_condition_uses_the_shipped_style_skill(self) -> None:
        rng = np.random.default_rng(7)
        for _ in range(500):
            condition = sample_condition(rng, 0)
            assert condition[0] == policy_skill_for_aggression(condition[1])

    def test_default_aggression_uses_the_shipped_bands(self) -> None:
        rng = np.random.default_rng(7)
        for _ in range(500):
            aggression = sample_condition(rng, 0)[1]
            assert 250 <= aggression <= 399 or 500 <= aggression <= 600

    def test_aggression_curriculum_stays_inside_its_inclusive_range(self) -> None:
        rng = np.random.default_rng(7)
        seen = {sample_condition(rng, 0, (750, 751))[1] for _ in range(500)}
        assert seen == {750, 751}

    @pytest.mark.parametrize("bounds", [(-1, 100), (200, 100), (0, 1001)])
    def test_rejects_invalid_aggression_curriculum(
        self,
        bounds: tuple[int, int],
    ) -> None:
        with pytest.raises(ValueError, match="aggression range"):
            sample_condition(np.random.default_rng(0), 0, bounds)

    def test_faction_is_the_seats_honest_side_never_sampled(self) -> None:
        rng = np.random.default_rng(7)
        for faction in (0, 1000):
            for _ in range(100):
                actual = sample_condition(rng, faction)[2]
                assert actual == faction

    def test_the_condition_includes_the_matching_strategy_one_hot(self) -> None:
        cond = sample_condition(np.random.default_rng(0), 0)
        assert isinstance(cond, tuple)
        assert len(cond) == 7
        assert cond[3:] == oxide_gym.strategy_one_hot(cond[1])

    def test_rejects_a_condition_that_lies_about_the_roster(self) -> None:
        with pytest.raises(ValueError, match="0 or 1000"):
            sample_condition(np.random.default_rng(0), 1)


class TestAggressionMix:
    def test_parser_normalizes_bands_in_canonical_order(self) -> None:
        assert parse_aggression_mix("500-600=.40, 250-399=.60") == (
            (250, 399, 0.6),
            (500, 600, 0.4),
        )

    @pytest.mark.parametrize(
        "text",
        [
            "",
            "250-399",
            "250=.5",
            "low-high=1",
            "-1-100=1",
            "0-1001=1",
            "400-300=1",
            "250-399=one",
            "250-399=0",
            "250-399=-1",
            "250-399=nan",
            "250-399=1,250-399=1",
            "250-399=1,399-500=1",
        ],
    )
    def test_parser_rejects_invalid_or_overlapping_bands(self, text: str) -> None:
        with pytest.raises(argparse.ArgumentTypeError):
            parse_aggression_mix(text)

    def test_weighted_draws_are_seeded_and_match_the_requested_share(self) -> None:
        mix = parse_aggression_mix("250-399=.60,500-600=.40")
        left = np.random.default_rng(81)
        right = np.random.default_rng(81)
        left_draws = [sample_aggression(left, mix) for _ in range(10_000)]
        right_draws = [sample_aggression(right, mix) for _ in range(10_000)]

        assert left_draws == right_draws
        assert all(
            250 <= aggression <= 399 or 500 <= aggression <= 600
            for aggression in left_draws
        )
        industry_share = sum(aggression <= 399 for aggression in left_draws) / len(
            left_draws
        )
        assert 0.58 <= industry_share <= 0.62

    def test_custom_range_spends_only_the_aggression_draw(self) -> None:
        actual = np.random.default_rng(9)
        control = np.random.default_rng(9)
        condition = sample_condition(actual, 0, (750, 751))
        expected_aggression = int(control.integers(750, 752))

        assert condition == condition_from_profile(
            1000,
            expected_aggression,
            0,
        )
        assert actual.integers(1_000_000) == control.integers(1_000_000)
        assert resolve_aggression_distribution((750, 751), None) == ((750, 751, 1.0),)

    def test_no_custom_range_resolves_to_the_shipped_mix(self) -> None:
        assert (
            resolve_training_aggression_distribution(None, None)
            == SHIPPED_AGGRESSION_DISTRIBUTION
        )


class TestProfileCurriculum:
    @staticmethod
    def _default_draws(seed: int) -> list[EpisodeDials]:
        curriculum = ProfileCurriculum(
            np.random.default_rng(seed),
            SHIPPED_AGGRESSION_DISTRIBUTION,
        )
        return [curriculum.sample({0: 0})[0] for _ in range(20)]

    def test_default_block_is_the_exact_shipped_factorial(self) -> None:
        draws = self._default_draws(71)
        cells = Counter((draw.policy_skill, draw.execution.name) for draw in draws)

        assert cells == Counter(
            {(620, profile.name): 3 for profile in SHIPPED_EXECUTION_PROFILES}
            | {(1000, profile.name): 2 for profile in SHIPPED_EXECUTION_PROFILES}
        )
        assert all(
            (
                (draw.policy_skill == 620 and 250 <= draw.aggression <= 399)
                or (draw.policy_skill == 1000 and 500 <= draw.aggression <= 600)
            )
            for draw in draws
        )

    def test_default_factorial_is_deterministic_but_seeded(self) -> None:
        assert self._default_draws(71) == self._default_draws(71)
        assert self._default_draws(71) != self._default_draws(72)

    def test_custom_aggression_keeps_an_independent_execution_cycle(self) -> None:
        distribution = resolve_aggression_distribution((750, 751), None)
        curriculum = ProfileCurriculum(np.random.default_rng(9), distribution)
        draws = [curriculum.sample({0: 0})[0] for _ in range(4)]

        assert {draw.aggression for draw in draws} == {750, 751}
        assert {draw.policy_skill for draw in draws} == {1000}
        assert {draw.execution for draw in draws} == set(SHIPPED_EXECUTION_PROFILES)

    def test_named_execution_values_match_deployment(self) -> None:
        assert [
            (profile.hesitation_permille, profile.cadence)
            for profile in SHIPPED_EXECUTION_PROFILES
        ] == [(350, 56), (190, 36), (5, 28), (0, 28)]


class TestMaybeBlunder:
    def test_uses_the_exact_hesitation_permille(self) -> None:
        action = (3, 24, 25)
        logits = np.zeros(ACTIONS)
        mask = np.ones(ACTIONS, dtype=bool)
        actual = np.random.default_rng(17)
        control = np.random.default_rng(17)

        outcomes = [
            maybe_blunder(action, logits, mask, 190, actual) for _ in range(1000)
        ]
        expected_hesitations = sum(
            int(control.integers(1000)) < 190 for _ in range(1000)
        )
        assert outcomes.count((0, 24, 25)) == expected_hesitations

    def test_zero_hesitation_spends_no_rng_draw(self) -> None:
        action = (3, 24, 25)
        actual = np.random.default_rng(17)
        control = np.random.default_rng(17)

        assert (
            maybe_blunder(
                action,
                np.zeros(ACTIONS),
                np.ones(ACTIONS, dtype=bool),
                0,
                actual,
            )
            == action
        )
        assert actual.integers(1_000_000) == control.integers(1_000_000)

    @pytest.mark.parametrize("hesitation", [-1, 1001])
    def test_rejects_invalid_hesitation(self, hesitation: int) -> None:
        with pytest.raises(ValueError, match="hesitation"):
            maybe_blunder(
                (3, 24, 25),
                np.zeros(ACTIONS),
                np.ones(ACTIONS, dtype=bool),
                hesitation,
                np.random.default_rng(0),
            )

    def test_rollout_uses_execution_hesitation_not_policy_skill(
        self,
        monkeypatch: pytest.MonkeyPatch,
        job_with_death: Job,
    ) -> None:
        seen: list[int] = []

        def record(
            action: ActionPlan,
            _logits: np.ndarray,
            _mask: np.ndarray,
            hesitation_permille: int,
            _rng: np.random.Generator,
        ) -> ActionPlan:
            seen.append(hesitation_permille)
            return action

        monkeypatch.setattr(league, "maybe_blunder", record)
        job_with_death.episode_dials = {
            0: EpisodeDials(1000, 500, ExecutionProfile("a", 350, 28)),
            1: EpisodeDials(1000, 500, ExecutionProfile("b", 190, 28)),
        }
        policy = make_policy("mlp")
        policy.eval()

        rollout(
            policy,
            [job_with_death],
            itertools.repeat(0),
            1,
            "cpu",
        )

        assert seen == [350, 190]


class TestMapMix:
    def test_parser_normalizes_in_canonical_order(self) -> None:
        assert parse_map_mix("grand=.50, fixed=.25, random=.25") == {
            "fixed": 0.25,
            "random": 0.25,
            "grand": 0.5,
        }

    @pytest.mark.parametrize(
        "text",
        [
            "",
            "fixed",
            "small=1",
            "fixed=one",
            "fixed=-1",
            "fixed=nan",
            "fixed=0,random=0",
            "fixed=1,fixed=2",
        ],
    )
    def test_parser_rejects_ambiguous_or_non_positive_mixes(self, text: str) -> None:
        with pytest.raises(argparse.ArgumentTypeError):
            parse_map_mix(text)

    def test_maps_compatibility_becomes_a_single_family_without_spending_rng(
        self,
    ) -> None:
        assert resolve_map_mix("grand", None) == {"grand": 1.0}
        actual = np.random.default_rng(9)
        control = np.random.default_rng(9)
        assert sample_map_family(actual, {"grand": 1.0}) == "grand"
        assert actual.integers(1_000_000) == control.integers(1_000_000)

    def test_mixed_draws_are_seeded_and_visit_each_family(self) -> None:
        mix = parse_map_mix("fixed=.25,random=.25,grand=.50")
        left = np.random.default_rng(81)
        right = np.random.default_rng(81)
        left_draws = [sample_map_family(left, mix) for _ in range(100)]
        right_draws = [sample_map_family(right, mix) for _ in range(100)]
        assert left_draws == right_draws
        assert set(left_draws) == {"fixed", "random", "grand"}

    def test_warmer_covers_every_active_generated_family(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        calls: list[tuple[int, str, int, bool, str, str | None]] = []

        def fake_generate(
            seed: int,
            out_dir: str,
            players: int = 2,
            teams: bool = False,
            driver: str = "default-driver",
            pace: str | None = None,
        ) -> str:
            calls.append((seed, out_dir, players, teams, driver, pace))
            return "unused"

        monkeypatch.setattr(league, "_generate", fake_generate)
        monkeypatch.setattr(league, "cache_dir", lambda name: name)
        families = generated_map_families(
            parse_map_mix("fixed=.25,random=.25,grand=.50"),
            {"self": 0.5, "ffa": 0.2, "team2": 0.3},
        )
        assert families == ("random", "grand", "ffa", "team")

        warm_generated_maps(100_007, families, "candidate-driver")

        assert calls == [
            (7, "oxide-maps-train", 2, False, "candidate-driver", None),
            (7, "oxide-maps-train-grand", 2, False, "candidate-driver", "grand"),
            (7, "oxide-maps-train4", 4, False, "candidate-driver", None),
            (7, "oxide-maps-train2v2", 4, True, "candidate-driver", None),
        ]

    def test_lineage_covers_every_consumed_world_source(
        self, tmp_path: pathlib.Path
    ) -> None:
        driver = tmp_path / "oxide-driver"
        fixed = tmp_path / "skirmish.json"
        generator = tmp_path / "mapgen.py"
        environment = tmp_path / "uv.lock"
        driver.write_bytes(b"gym contract")
        fixed.write_bytes(b"fixed scenario")
        generator.write_bytes(b"generator v1")
        environment.write_bytes(b"numpy version")

        fixed_inputs = training_world_inputs(
            driver,
            {"fixed": 1.0},
            {"tier": 1.0},
            fixed_scenario=fixed,
            map_generator=generator,
            environment_lock=environment,
        )
        code_inputs = {
            "gym_client",
            "gym_driver",
            "model_code",
            "ppo_code",
            "trainer",
        }
        assert set(fixed_inputs) == code_inputs | {"fixed_scenario"}

        generated_inputs = training_world_inputs(
            driver,
            {"random": 1.0},
            {"ffa": 1.0},
            fixed_scenario=fixed,
            map_generator=generator,
            environment_lock=environment,
        )
        assert set(generated_inputs) == code_inputs | {
            "map_generator",
            "map_environment",
        }
        original_generator = generated_inputs["map_generator"]
        generator.write_bytes(b"generator v2")
        changed_inputs = training_world_inputs(
            driver,
            {"random": 1.0},
            {"ffa": 1.0},
            fixed_scenario=fixed,
            map_generator=generator,
            environment_lock=environment,
        )
        assert changed_inputs["map_generator"] != original_generator


class TestFactionMix:
    def test_parser_normalizes_in_canonical_order(self) -> None:
        assert parse_faction_mix("CC=.50, ff=.25, fc=.125, cf=.125") == {
            "ff": 0.25,
            "fc": 0.125,
            "cf": 0.125,
            "cc": 0.5,
        }

    @pytest.mark.parametrize(
        "text",
        [
            "",
            "fc",
            "ferrous-cupric=1",
            "fx=1",
            "fc=one",
            "fc=-1",
            "fc=nan",
            "ff=0,fc=0",
            "fc=1,FC=2",
        ],
    )
    def test_parser_rejects_ambiguous_or_non_positive_mixes(self, text: str) -> None:
        with pytest.raises(argparse.ArgumentTypeError):
            parse_faction_mix(text)

    def test_default_preserves_the_authored_pair_without_spending_rng(self) -> None:
        mix = resolve_faction_mix(None)
        assert mix == {"fc": 1.0}
        actual = np.random.default_rng(9)
        control = np.random.default_rng(9)
        assert sample_faction_pair(actual, mix) == "fc"
        assert actual.integers(1_000_000) == control.integers(1_000_000)

    def test_mixed_draws_are_seeded_and_visit_every_pair(self) -> None:
        mix = parse_faction_mix("ff=.25,fc=.25,cf=.25,cc=.25")
        left = np.random.default_rng(81)
        right = np.random.default_rng(81)
        left_draws = [sample_faction_pair(left, mix) for _ in range(100)]
        right_draws = [sample_faction_pair(right, mix) for _ in range(100)]
        assert left_draws == right_draws
        assert set(left_draws) == {"ff", "fc", "cf", "cc"}

    @pytest.mark.parametrize(
        ("pair", "seats", "expected"),
        [
            ("fc", 2, "fc"),
            ("cf", 4, "cfcf"),
        ],
    )
    def test_pair_expands_to_full_seat_order(
        self, pair: str, seats: int, expected: str
    ) -> None:
        assert expand_faction_pair(pair, seats) == expected


class _ResetRecorder:
    def __init__(self) -> None:
        self.scenarios: list[str | None] = []
        self.resets: list[dict[str, object]] = []

    def reset(self, *_args: object, **kwargs: object) -> Frame:
        self.scenarios.append(cast("str | None", kwargs.get("scenario")))
        self.resets.append(dict(kwargs))
        return Frame(False, 0, seats={0: _view(0.1), 1: _view(0.2)})


class TestPerEpisodeAggressionSampling:
    def test_each_learner_seat_uses_the_jobs_seeded_disjoint_mix(self) -> None:
        mix = parse_aggression_mix("250-399=.60,500-600=.40")

        def make_job() -> Job:
            return Job(
                cast("Worker", _ResetRecorder()),
                "self",
                0,
                pathlib.Path("."),
                np.random.default_rng(23),
                "cpu",
                aggression_mix=mix,
            )

        left, right = make_job(), make_job()
        left_draws: list[tuple[int, int]] = []
        right_draws: list[tuple[int, int]] = []
        for seed in range(50_000, 50_100):
            left.reset(seed)
            right.reset(seed)
            left_draws.append((left.conditions[0][1], left.conditions[1][1]))
            right_draws.append((right.conditions[0][1], right.conditions[1][1]))

        assert left_draws == right_draws
        assert all(
            250 <= aggression <= 399 or 500 <= aggression <= 600
            for draw in left_draws
            for aggression in draw
        )
        assert any(west != east for west, east in left_draws)

    def test_job_passes_the_sampled_cadence_to_worker_reset(self) -> None:
        worker = _ResetRecorder()
        job = Job(
            cast("Worker", worker),
            "tier",
            0,
            pathlib.Path("."),
            np.random.default_rng(31),
            "cpu",
        )

        seen: set[ExecutionProfile] = set()
        for seed in range(50_000, 50_020):
            job.reset(seed)
            dials = job.episode_dials[0]
            seen.add(dials.execution)
            assert worker.resets[-1]["cadence"] == dials.execution.cadence
            assert job.conditions[0][0] == dials.policy_skill

        assert seen == set(SHIPPED_EXECUTION_PROFILES)


class TestPerEpisodeMapSampling:
    def test_duel_jobs_draw_the_same_seeded_family_sequence(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        def fake_generate(
            _seed: int,
            _out_dir: str,
            players: int = 2,
            teams: bool = False,
            driver: str = "default-driver",
            pace: str | None = None,
        ) -> str:
            assert players == 2
            assert not teams
            assert driver == "candidate-driver"
            return "grand" if pace == "grand" else "random"

        monkeypatch.setattr(league, "generate", fake_generate)
        mix = parse_map_mix("fixed=.25,random=.25,grand=.50")

        def make_job() -> Job:
            return Job(
                cast("Worker", _ResetRecorder()),
                "self",
                0,
                pathlib.Path("."),
                np.random.default_rng(19),
                "cpu",
                map_mix=mix,
                map_driver="candidate-driver",
            )

        left = make_job()
        right = make_job()
        left_families = []
        right_families = []
        for seed in range(50_000, 50_080):
            left.reset(seed)
            right.reset(seed)
            left_families.append(left.map_family)
            right_families.append(right.map_family)

        assert left_families == right_families
        assert set(left_families) == {"fixed", "random", "grand"}


class TestFixedTierRoles:
    def test_prime_role_keeps_the_exact_tier_and_requested_seat(self) -> None:
        worker = _ResetRecorder()
        job = Job(
            cast("Worker", worker),
            "prime",
            1,
            pathlib.Path("."),
            np.random.default_rng(19),
            "cpu",
            faction_mix={"fc": 1.0},
        )

        for seed in (50_000, 50_001):
            job.reset(seed)

        assert job.learner_seats == [1]
        assert job.tier == "prime"
        assert job.map_family == "fixed"
        assert [
            (reset["control"], reset["tier"], reset["scenario"])
            for reset in worker.resets
        ] == [
            ((1,), "prime", None),
            ((1,), "prime", None),
        ]
        assert all(reset["factions"] == "fc" for reset in worker.resets)


class TestPerEpisodeFactionSampling:
    @pytest.mark.parametrize(
        ("kind", "seat", "pair", "expected_code", "expected_conditions"),
        [
            ("self", 0, "cf", "cf", {0: 1000, 1: 0}),
            ("past", 0, "cc", "cc", {0: 1000, 1: 1000}),
            ("team", 0, "cf", "cfcf", {0: 1000, 2: 1000}),
            ("team2", 1, "fc", "fcfc", {2: 0}),
            ("ffa", 3, "fc", "fcfc", {3: 1000}),
            ("prime", 1, "fc", "fc", {1: 1000}),
        ],
    )
    def test_reset_passes_full_roster_and_faction_correct_conditions(
        self,
        monkeypatch: pytest.MonkeyPatch,
        kind: str,
        seat: int,
        pair: str,
        expected_code: str,
        expected_conditions: dict[int, int],
    ) -> None:
        monkeypatch.setattr(league, "generate", lambda *_args, **_kwargs: "generated")
        worker = _ResetRecorder()
        job = Job(
            cast("Worker", worker),
            kind,
            seat,
            pathlib.Path("."),
            np.random.default_rng(19),
            "cpu",
            faction_mix={pair: 1.0},
        )
        job.reset(50_000)

        reset = worker.resets[-1]
        assert reset["factions"] == expected_code
        conditions = cast("dict[int, tuple[int, ...]]", reset["conditions"])
        assert {s: condition[2] for s, condition in conditions.items()} == (
            expected_conditions
        )
        assert {s: condition[2] for s, condition in job.conditions.items()} == {
            s: faction
            for s, faction in expected_conditions.items()
            if s in job.learner_seats
        }

    def test_episode_sequence_is_deterministic_and_not_tied_to_seat_parity(
        self,
    ) -> None:
        mix = parse_faction_mix("ff=.25,fc=.25,cf=.25,cc=.25")

        def make_job() -> tuple[Job, _ResetRecorder]:
            worker = _ResetRecorder()
            return (
                Job(
                    cast("Worker", worker),
                    "self",
                    0,
                    pathlib.Path("."),
                    np.random.default_rng(23),
                    "cpu",
                    faction_mix=mix,
                ),
                worker,
            )

        left, left_worker = make_job()
        right, right_worker = make_job()
        for seed in range(50_000, 50_040):
            left.reset(seed)
            right.reset(seed)

        left_codes = [cast("str", reset["factions"]) for reset in left_worker.resets]
        right_codes = [cast("str", reset["factions"]) for reset in right_worker.resets]
        assert left_codes == right_codes
        assert set(left_codes) == {"ff", "fc", "cf", "cc"}


class TestRoleAllocation:
    def test_equivalent_cli_mix_order_has_one_role_and_rng_order(self) -> None:
        first = parse_opponent_mix("tier=2,self=1,rusher=1")
        second = parse_opponent_mix("rusher=.5,self=.5,tier=1")

        assert first == second == {"self": 0.25, "tier": 0.5, "rusher": 0.25}
        assert list(first) == ["self", "tier", "rusher"]

    @pytest.mark.parametrize(
        "text",
        [
            "unknown=1",
            "self",
            "self=1,self=2",
            "self=-1",
            "self=nan",
            "self=0,tier=0",
        ],
    )
    def test_invalid_cli_mix_is_refused(self, text: str) -> None:
        with pytest.raises(argparse.ArgumentTypeError):
            parse_opponent_mix(text)

    def test_requested_mix_is_allocated_by_learner_rows_not_jobs(self) -> None:
        mix = {"self": 0.25, "team": 0.25, "tier": 0.5}

        counts = allocate_role_counts(mix, 12)

        assert counts == {"self": 2, "team": 2, "tier": 8}

    def test_realized_mix_counts_both_learner_lanes(self) -> None:
        workers = [cast("Worker", object()) for _ in range(12)]
        jobs = assign_roles(
            workers,
            {"self": 0.25, "team": 0.25, "tier": 0.5},
            pathlib.Path("."),
            np.random.default_rng(0),
            "cpu",
        )

        assert realized_learner_row_mix(jobs) == {
            "self": 0.25,
            "team": 0.25,
            "tier": 0.5,
        }

    def test_realized_mix_can_measure_actual_valid_training_rows(self) -> None:
        workers = [cast("Worker", object()), cast("Worker", object())]
        self_job = Job(
            workers[0],
            "self",
            0,
            pathlib.Path("."),
            np.random.default_rng(0),
            "cpu",
        )
        tier_job = Job(
            workers[1],
            "tier",
            0,
            pathlib.Path("."),
            np.random.default_rng(1),
            "cpu",
        )
        valid = np.asarray(
            [
                [True, True, True],
                [True, False, True],
                [False, False, True],
            ]
        )

        assert realized_learner_row_mix([self_job, tier_job], valid) == {
            "self": 0.5,
            "tier": 0.5,
        }

    @pytest.mark.parametrize(
        "mix",
        [
            {"self": -0.1, "tier": 1.1},
            {"self": float("nan")},
            {"self": 0.0},
        ],
    )
    def test_invalid_role_weights_fail_before_job_construction(
        self, mix: dict[str, float]
    ) -> None:
        with pytest.raises(ValueError, match="finite non-negative"):
            allocate_role_counts(mix, 8)


class TestRollout:
    def test_the_batch_stays_rectangular_past_a_death(
        self, death_batch: tuple[np.ndarray, ...]
    ) -> None:
        # Four steps x two lanes, no lane truncated by the death — column 0
        # (seat 0) is the surviving teammate, column 1 (seat 1) died.
        obs_b, _mask_b, act_b, *_rest, valid_b = death_batch
        assert obs_b.shape == (4, 2, NET_FEATURES)
        assert act_b.shape == (4, 2, 3)
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


class TestIncumbentOpponent:
    @pytest.mark.parametrize(
        ("winner", "expected"),
        [
            (7, (7, 24, 25)),
            (13, (0, 13, 25)),
            (20, (0, 24, 20)),
        ],
    )
    def test_legacy_arbitration_activates_only_the_winning_head(
        self,
        winner: int,
        expected: tuple[int, int, int],
    ) -> None:
        logits = torch.zeros(1, ACTIONS)
        logits[0, winner] = 5.0
        mask = torch.ones(1, ACTIONS, dtype=torch.bool)

        assert tuple(legacy_incumbent_plan(logits, mask)[0].tolist()) == expected

    def test_legacy_arbitration_uses_the_mask_and_lowest_index_tie_break(self) -> None:
        logits = torch.zeros(1, ACTIONS)
        logits[0, 5] = 4.0
        logits[0, 13] = 9.0
        logits[0, 20] = 4.0
        logits[0, 24:] = 100.0
        mask = torch.ones(1, ACTIONS, dtype=torch.bool)
        mask[0, 13] = False

        plan = legacy_incumbent_plan(logits, mask)

        assert tuple(plan[0].tolist()) == (5, 24, 25)

    def test_legacy_arbitration_rejects_an_empty_inherited_mask(self) -> None:
        logits = torch.zeros(1, ACTIONS)
        mask = torch.zeros(1, ACTIONS, dtype=torch.bool)
        mask[0, 24:] = True

        with pytest.raises(ValueError, match="no legal inherited"):
            legacy_incumbent_plan(logits, mask)

    def test_active_lane_requires_an_exact_recovered_checkpoint(
        self, tmp_path: pathlib.Path
    ) -> None:
        with pytest.raises(ValueError, match="requires --incumbent"):
            load_incumbent_policy({"incumbent": 0.25}, None, "cpu")

        policy = make_policy("mlp")
        ordinary = tmp_path / "ordinary.pt"
        save_policy(
            policy,
            "mlp",
            ordinary,
            {"gym_version": GYM_VERSION, "update": 10},
        )
        with pytest.raises(ValueError, match="exact Q12-recovered"):
            load_incumbent_policy({"incumbent": 0.25}, str(ordinary), "cpu")

        unfloored = tmp_path / "unfloored.pt"
        save_policy(
            policy,
            "mlp",
            unfloored,
            {
                "gym_version": GYM_VERSION,
                "update": 10,
                "q12_recovered": True,
                "unfloored_actions": [24, 25],
            },
        )
        with pytest.raises(ValueError, match="no unfloored actions"):
            load_incumbent_policy({"incumbent": 0.25}, str(unfloored), "cpu")

        recovered = tmp_path / "recovered.pt"
        save_policy(
            policy,
            "mlp",
            recovered,
            {
                "gym_version": GYM_VERSION,
                "update": 10,
                "q12_recovered": True,
                "unfloored_actions": [],
            },
        )
        loaded = load_incumbent_policy(
            {"incumbent": 0.25},
            str(recovered),
            "cpu",
        )
        assert loaded is not None
        assert not loaded.training

    def test_incumbent_plays_the_frozen_actor_greedily(self) -> None:
        policy = make_policy("mlp")
        with torch.no_grad():
            for parameter in policy.parameters():
                parameter.zero_()
            policy.pi.bias[7] = 3.0
        policy.eval()
        job = Job(
            cast("Worker", _ScriptedWorker([])),
            "incumbent",
            0,
            pathlib.Path("."),
            np.random.default_rng(0),
            "cpu",
            incumbent=policy,
        )
        job.frame = Frame(False, 0, seats={0: _view(0.1), 1: _view(0.2)})

        for seed in range(20):
            torch.manual_seed(seed)
            assert job.opponent_action("cpu") == {1: (7, 24, 25)}

    def test_past_lane_still_samples_its_checkpoint(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        policy = make_policy("mlp")
        policy.eval()
        job = Job(
            cast("Worker", _ScriptedWorker([])),
            "past",
            0,
            pathlib.Path("."),
            np.random.default_rng(0),
            "cpu",
        )
        job.past = policy
        job.frame = Frame(False, 0, seats={0: _view(0.1), 1: _view(0.2)})
        sampled = []

        def fake_sample(_distribution: object) -> torch.Tensor:
            sampled.append(True)
            return torch.tensor([5])

        monkeypatch.setattr(torch.distributions.Categorical, "sample", fake_sample)
        assert job.opponent_action("cpu") == {1: (5, 13, 20)}
        assert sampled == [True, True, True]

    def test_initialization_provenance_distinguishes_reconstructed_critics(
        self,
    ) -> None:
        assert q12_initialization_provenance({"q12_recovered": True})
        assert not q12_initialization_provenance({"q12_recovered": False})
        assert not q12_initialization_provenance({})
        assert not q12_initialization_provenance(None)


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
            job.conditions = {
                0: condition_from_profile(1000, 500, 0),
                1: condition_from_profile(1000, 500, 1000),
            }
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


class TestValueWarmupSchedule:
    def test_an_explicit_warmup_runs_relative_to_the_parent_update(self) -> None:
        assert value_warmup_active(1301, 1300, 15)
        assert value_warmup_active(1315, 1300, 15)
        assert not value_warmup_active(1316, 1300, 15)

    def test_zero_disables_warmup(self) -> None:
        assert not value_warmup_active(1, 0, 0)

    def test_from_scratch_keeps_the_historical_default(self) -> None:
        assert resolved_value_warmup(None, initialized=False) == 15

    def test_initialized_checkpoint_defaults_to_no_actor_freeze(self) -> None:
        assert resolved_value_warmup(None, initialized=True) == 0

    def test_critic_readiness_is_explicit_with_legacy_producer_fallbacks(
        self,
    ) -> None:
        assert checkpoint_critic_ready({"critic_ready": True})
        assert not checkpoint_critic_ready({"critic_ready": False})
        assert not checkpoint_critic_ready({"q12_recovered": True})
        assert not checkpoint_critic_ready({"bc_epoch": 4})
        assert not checkpoint_critic_ready({"revival": {}})
        assert checkpoint_critic_ready({"update": 10})
        with pytest.raises(TypeError, match="must be a boolean"):
            checkpoint_critic_ready({"critic_ready": 1})

    def test_unready_initialized_critic_gets_warmup(self) -> None:
        assert (
            resolved_value_warmup(
                None,
                initialized=True,
                critic_ready=False,
            )
            == DEFAULT_VALUE_WARMUP
        )

    def test_explicit_warmup_overrides_either_default(self) -> None:
        assert resolved_value_warmup(7, initialized=False) == 7
        assert resolved_value_warmup(7, initialized=True) == 7
        assert resolved_value_warmup(0, initialized=True, critic_ready=False) == 0

    def test_checkpoint_options_are_mutually_exclusive(self) -> None:
        parser = argparse.ArgumentParser()
        add_initialization_arguments(parser)

        initialized = parser.parse_args(["--initialize-from", "parent.pt"])
        assert initialized.initialize_from == "parent.pt"
        assert initialized.resume is None

        compatibility = parser.parse_args(["--resume", "parent.pt"])
        assert compatibility.initialize_from is None
        assert compatibility.resume == "parent.pt"

        with pytest.raises(SystemExit):
            parser.parse_args(
                [
                    "--initialize-from",
                    "parent.pt",
                    "--resume",
                    "other.pt",
                ]
            )

    def test_compatibility_alias_is_marked_deprecated_in_help(self) -> None:
        parser = argparse.ArgumentParser()
        add_initialization_arguments(parser)
        help_text = parser.format_help()
        assert "--initialize-from" in help_text
        assert "DEPRECATED alias" in help_text


class TestRunDirectoryIsolation:
    def test_a_new_run_claims_its_own_empty_pool(self, tmp_path: pathlib.Path) -> None:
        run_dir = tmp_path / "runs" / "new-phase"

        pool_dir = claim_fresh_run_directory(run_dir)

        assert pool_dir == run_dir / "pool"
        assert pool_dir.is_dir()
        assert list(run_dir.iterdir()) == [pool_dir]

    def test_an_empty_precreated_directory_is_a_valid_destination(
        self, tmp_path: pathlib.Path
    ) -> None:
        run_dir = tmp_path / "runs" / "new-phase"
        run_dir.mkdir(parents=True)

        assert claim_fresh_run_directory(run_dir) == run_dir / "pool"

    @pytest.mark.parametrize(
        ("stale_path", "contents"),
        [
            ("log.jsonl", b'{"update": 1}\n'),
            ("pool/ckpt-00025.pt", b"old pool"),
            ("probe/probe-00100.json", b"old probe"),
            ("latest.pt", b"old latest"),
            ("unexpected.txt", b"unknown"),
        ],
    )
    def test_any_existing_run_content_is_refused_without_mutation(
        self,
        tmp_path: pathlib.Path,
        stale_path: str,
        contents: bytes,
    ) -> None:
        run_dir = tmp_path / "runs" / "reused-name"
        target = run_dir / stale_path
        target.parent.mkdir(parents=True)
        target.write_bytes(contents)
        before = [
            (
                path.relative_to(run_dir).as_posix(),
                path.is_dir(),
                None if path.is_dir() else path.read_bytes(),
            )
            for path in sorted(run_dir.rglob("*"))
        ]

        with pytest.raises(RuntimeError, match="run directory is not empty"):
            claim_fresh_run_directory(run_dir)

        after = [
            (
                path.relative_to(run_dir).as_posix(),
                path.is_dir(),
                None if path.is_dir() else path.read_bytes(),
            )
            for path in sorted(run_dir.rglob("*"))
        ]
        assert after == before

    def test_a_second_claim_is_refused_without_changing_the_first(
        self, tmp_path: pathlib.Path
    ) -> None:
        run_dir = tmp_path / "runs" / "owned"
        pool_dir = claim_fresh_run_directory(run_dir)

        with pytest.raises(RuntimeError, match="run directory is not empty"):
            claim_fresh_run_directory(run_dir)

        assert list(run_dir.iterdir()) == [pool_dir]
        assert not list(pool_dir.iterdir())

    def test_main_refuses_a_stale_phase_before_launch_or_file_write(
        self,
        tmp_path: pathlib.Path,
        monkeypatch: pytest.MonkeyPatch,
    ) -> None:
        run_dir = tmp_path / "runs" / "reused-name"
        stale_log = run_dir / "log.jsonl"
        stale_log.parent.mkdir(parents=True)
        stale_log.write_text('{"lineage": "old"}\n')
        before = stale_log.read_bytes()

        def unexpected_worker(_driver: str) -> None:
            pytest.fail("workers must not launch for a stale run directory")

        monkeypatch.chdir(tmp_path)
        monkeypatch.setattr(league, "Worker", unexpected_worker)
        monkeypatch.setattr(league, "training_world_inputs", lambda *_: {})
        monkeypatch.setattr(
            sys,
            "argv",
            [
                "league.py",
                "--name",
                "reused-name",
                "--anchor",
                "",
            ],
        )

        with pytest.raises(SystemExit) as stopped:
            league.main()

        assert stopped.value.code == 2
        assert stale_log.read_bytes() == before
        assert [path.relative_to(run_dir) for path in run_dir.rglob("*")] == [
            pathlib.Path("log.jsonl")
        ]


class TestAnchorSchedule:
    def test_decay_uses_the_current_phase_clock(self) -> None:
        assert anchor_coefficient_at(0.1, 0.5, 1301, 1300) == pytest.approx(0.1)
        assert anchor_coefficient_at(0.1, 0.5, 1302, 1300) == pytest.approx(0.05)

    def test_holding_the_anchor_ignores_the_parent_update(self) -> None:
        assert anchor_coefficient_at(0.1, 1.0, 1451, 1450) == pytest.approx(0.1)

    def test_pre_phase_updates_are_rejected(self) -> None:
        with pytest.raises(ValueError, match="after phase start"):
            anchor_coefficient_at(0.1, 0.995, 1450, 1450)


class TestPhaseIntervals:
    def test_intervals_start_from_the_current_phase(self) -> None:
        assert not phase_interval_due(96, 95, 25)
        assert phase_interval_due(120, 95, 25)
        assert not phase_interval_due(100, 95, 25)

    def test_zero_disables_an_optional_interval(self) -> None:
        assert not phase_interval_due(96, 95, 0)

    def test_pre_phase_updates_are_rejected(self) -> None:
        with pytest.raises(ValueError, match="after phase start"):
            phase_interval_due(95, 95, 25)


class TestBoundedStyleBonus:
    def test_alignment_matches_the_requested_posture(self) -> None:
        pushing = [0] * FEATURES
        pushing[F["army_state"]] = 2
        defending = [0] * FEATURES
        defending[F["army_state"]] = 0

        assert style_alignment(pushing, 1000) == 1.0
        assert style_alignment(defending, 0) == 1.0
        assert style_alignment(pushing, 0) == -1.0
        assert style_alignment(defending, 1000) == -1.0

    def test_episode_length_cannot_increase_the_bonus(self) -> None:
        assert style_bonus(1.0, 1, MAX_STYLE_BONUS) == MAX_STYLE_BONUS
        assert style_bonus(40_000.0, 40_000, MAX_STYLE_BONUS) == MAX_STYLE_BONUS
        assert style_bonus(-40_000.0, 40_000, MAX_STYLE_BONUS) == -MAX_STYLE_BONUS

    def test_zero_disables_the_bonus(self) -> None:
        assert style_bonus(1.0, 1, 0.0) == 0.0
        assert style_bonus(0.0, 0, MAX_STYLE_BONUS) == 0.0

    def test_cli_coefficient_is_finite_and_bounded(self) -> None:
        assert bounded_style_coefficient("0.1") == MAX_STYLE_BONUS
        for value in ("-0.001", "0.1001", "nan", "inf"):
            with pytest.raises(argparse.ArgumentTypeError):
                bounded_style_coefficient(value)


class TestCreditAndEntropyControls:
    def test_gae_lambda_accepts_only_the_unit_interval(self) -> None:
        assert unit_interval("0") == 0.0
        assert unit_interval("1") == 1.0
        for value in ("-0.001", "1.001", "nan", "inf"):
            with pytest.raises(argparse.ArgumentTypeError):
                unit_interval(value)

    def test_entropy_coefficient_is_finite_and_conservative(self) -> None:
        assert bounded_entropy_coefficient("0") == 0.0
        assert bounded_entropy_coefficient("0.1") == 0.1
        for value in ("-0.001", "0.101", "nan", "inf"):
            with pytest.raises(argparse.ArgumentTypeError):
                bounded_entropy_coefficient(value)

    def test_production_entropy_is_additional_and_disabled_by_default(self) -> None:
        parser = argparse.ArgumentParser()
        add_entropy_arguments(parser)

        defaults = parser.parse_args([])
        assert defaults.entropy_coef == 0.002
        assert defaults.production_entropy_coef == 0.0

        configured = parser.parse_args(
            [
                "--entropy-coef",
                "0.006",
                "--production-entropy-coef",
                "0.04",
            ]
        )
        assert effective_production_entropy_coefficient(
            configured.entropy_coef,
            configured.production_entropy_coef,
        ) == pytest.approx(0.042)

    def test_production_entropy_help_explains_the_effective_weight(self) -> None:
        parser = argparse.ArgumentParser()
        add_entropy_arguments(parser)

        help_text = parser.format_help()
        assert "--production-entropy-coef" in help_text
        assert "effective head weight" in help_text
        assert "third of --entropy-coef" in help_text


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


class _EpisodeWorker(_ResettingWorker):
    """Records episode-boundary resets for aligned-collection tests."""

    def __init__(
        self,
        frames: list[Frame],
        reset_frame: Frame,
        name: str,
        log: list[str] | None = None,
    ) -> None:
        super().__init__(frames, reset_frame)
        self.name = name
        self.log = log if log is not None else []
        self.reset_count = 0

    def reset(self, *_args: object, **_kwargs: object) -> Frame:
        self.reset_count += 1
        self.log.append(f"reset:{self.name}")
        return self._reset_frame


class _ObservationValuePolicy(torch.nn.Module):
    """A policy whose critic exposes the first observation component."""

    def forward(
        self,
        obs: torch.Tensor,
        mask: torch.Tensor,
    ) -> tuple[torch.Tensor, torch.Tensor]:
        logits = torch.zeros(
            (obs.shape[0], ACTIONS),
            dtype=obs.dtype,
            device=obs.device,
        ).masked_fill(~mask, float("-inf"))
        return logits, obs[:, 0]


class TestEpisodeAlignedCollection:
    @staticmethod
    def _job(
        length: int,
        name: str,
        log: list[str] | None = None,
    ) -> tuple[Job, _EpisodeWorker]:
        initial = Frame(False, 0, seats={0: _view(0.1), 1: _view(0.2)})
        frames = [
            Frame(
                step == length,
                step * 16,
                winners=[0] if step == length else None,
                seats={0: _view(0.1 + step), 1: _view(0.2 + step)},
            )
            for step in range(1, length + 1)
        ]
        worker = _EpisodeWorker(frames, initial, name, log)
        job = Job(
            cast("Worker", worker),
            "self",
            0,
            pathlib.Path("."),
            np.random.default_rng(7),
            "cpu",
        )
        return job, worker

    def test_collects_one_complete_variable_length_episode_per_job(self) -> None:
        log: list[str] = []
        short, short_worker = self._job(2, "short", log)
        long, long_worker = self._job(4, "long", log)
        torch.manual_seed(3)
        policy = make_policy("mlp")
        policy.eval()

        batch, last_val, finals = rollout(
            policy,
            [short, long],
            iter((100, 101)),
            1,
            "cpu",
            collection="episodes",
            episode_max_steps=4,
        )

        valid = batch[7]
        assert valid.shape == (4, 4)
        np.testing.assert_array_equal(valid[:, 0], [True, True, False, False])
        np.testing.assert_array_equal(valid[:, 1], [True, True, False, False])
        np.testing.assert_array_equal(valid[:, 2], [True, True, True, True])
        np.testing.assert_array_equal(valid[:, 3], [True, True, True, True])
        np.testing.assert_array_equal(last_val, np.zeros(4, dtype=np.float32))
        assert sorted(finals) == [-1.0, -1.0, 1.0, 1.0]
        assert short_worker.reset_count == 1
        assert long_worker.reset_count == 1
        assert log.count("send:short") == 2
        assert log.count("send:long") == 4

    def test_ignores_window_steps_and_never_updates_mid_episode(self) -> None:
        job, worker = self._job(3, "only")
        torch.manual_seed(5)
        policy = make_policy("mlp")
        before = {
            name: parameter.detach().clone()
            for name, parameter in policy.named_parameters()
        }

        batch, _last_val, _finals = rollout(
            policy,
            [job],
            iter((100,)),
            1,
            "cpu",
            collection="episodes",
            episode_max_steps=3,
        )

        assert batch[0].shape[0] == 3
        assert worker.reset_count == 1
        for name, parameter in policy.named_parameters():
            assert torch.equal(parameter, before[name]), name

    def test_seeded_episode_batches_are_deterministic(self) -> None:
        def collect() -> tuple[np.ndarray, ...]:
            left, _ = self._job(2, "left")
            right, _ = self._job(3, "right")
            torch.manual_seed(11)
            policy = make_policy("mlp")
            policy.eval()
            torch.manual_seed(29)
            batch, _last_val, _finals = rollout(
                policy,
                [left, right],
                iter((100, 101)),
                1,
                "cpu",
                collection="episodes",
                episode_max_steps=3,
            )
            return batch

        first = collect()
        second = collect()
        for left, right in zip(first, second, strict=True):
            np.testing.assert_array_equal(left, right)

    def test_team_dead_lane_padding_survives_episode_alignment(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        monkeypatch.setattr(league, "generate", lambda *_args, **_kwargs: "team")
        initial = Frame(False, 0, seats={0: _view(0.1), 2: _view(0.2)})
        frames = [
            Frame(False, 16, seats={0: _view(1.1)}),
            Frame(False, 32, seats={0: _view(1.2)}),
            Frame(True, 48, winners=[0], seats={0: _view(1.3)}),
        ]
        worker = _EpisodeWorker(frames, initial, "team")
        job = Job(
            cast("Worker", worker),
            "team",
            0,
            pathlib.Path("."),
            np.random.default_rng(7),
            "cpu",
        )
        policy = make_policy("mlp")
        policy.eval()

        batch, _last_val, _finals = rollout(
            policy,
            [job],
            iter((100,)),
            1,
            "cpu",
            collection="episodes",
            episode_max_steps=3,
        )

        np.testing.assert_array_equal(batch[7][:, 0], [True, True, True])
        np.testing.assert_array_equal(batch[7][:, 1], [True, False, False])
        assert batch[6][-1, 1]
        assert batch[5][-1, 1] == pytest.approx(-1.0)

    def test_fails_loudly_when_a_job_exceeds_the_episode_limit(self) -> None:
        job, _worker = self._job(3, "slow")
        policy = make_policy("mlp")
        policy.eval()

        with pytest.raises(RuntimeError, match=r"exceeded 2 decisions.*self@tick32"):
            rollout(
                policy,
                [job],
                iter((100,)),
                1,
                "cpu",
                collection="episodes",
                episode_max_steps=2,
            )


def test_time_limit_bootstraps_only_living_lanes_and_cuts_the_reset() -> None:
    initial_live = _view(1.0)
    initial_live.raw[F["scrap"]] = 500
    initial_doomed = _view(2.0)
    initial_doomed.raw[F["scrap"]] = 500
    at_limit = _view(3.0)
    at_limit.raw[F["scrap"]] = 1_000
    after_reset_0 = _view(17.0)
    after_reset_1 = _view(19.0)

    terminal = Frame(
        True,
        40_000,
        truncated=True,
        alive=[0],
        seats={0: at_limit},
    )
    fresh = Frame(
        False,
        0,
        seats={0: after_reset_0, 1: after_reset_1},
    )
    worker = cast("Worker", _ResettingWorker([terminal], fresh))
    job = Job(worker, "self", 0, pathlib.Path("."), np.random.default_rng(0), "cpu")
    job.frame = Frame(
        False,
        39_992,
        seats={0: initial_live, 1: initial_doomed},
    )
    job.conditions = {
        0: condition_from_profile(1000, 500, 0),
        1: condition_from_profile(1000, 500, 1000),
    }
    policy = _ObservationValuePolicy()
    policy.eval()

    batch, last_val, finals = rollout(
        policy,
        [job],
        itertools.repeat(0),
        1,
        "cpu",
    )
    rew = batch[5]
    done = batch[6]
    expected_live = SHAPE_K * (SHAPE_GAMMA * 2.0 - 1.0) + TRAIN_GAMMA * 3.0
    expected_dead = -1.0 - SHAPE_K
    np.testing.assert_allclose(rew[0], [expected_live, expected_dead])
    np.testing.assert_array_equal(done[0], [True, True])
    np.testing.assert_allclose(last_val, [17.0, 19.0])
    assert finals == [0.0, -1.0]

    _adv, returns = league.gae(batch[5], done, batch[4], last_val)
    np.testing.assert_allclose(
        returns[0],
        [expected_live, expected_dead],
        err_msg="done must cut GAE before the reset observation",
    )


def _style_view(army_state: int) -> SeatView:
    view = _view(0.5)
    view.raw[F["army_state"]] = army_state
    return view


class TestStyleShaping:
    def test_style_is_absent_per_step_and_paid_once_at_the_terminal(self) -> None:
        pushing = _style_view(2)
        defending = _style_view(0)
        frames = [
            Frame(False, 16, seats={0: pushing, 1: defending}),
            Frame(True, 32, winners=[0], seats={0: pushing, 1: defending}),
        ]
        fresh = Frame(False, 48, seats={0: _view(0.1), 1: _view(0.1)})
        worker = cast("Worker", _ResettingWorker(frames, fresh))
        job = Job(worker, "self", 0, pathlib.Path("."), np.random.default_rng(0), "cpu")
        job.frame = Frame(False, 0, seats={0: pushing, 1: defending})
        job.conditions = {
            0: condition_from_profile(1000, 1000, 0),
            1: condition_from_profile(1000, 0, 1000),
        }
        torch.manual_seed(0)
        policy = make_policy("mlp")
        policy.eval()
        TEL.clear()

        batch, _last_val, finals = rollout(
            policy,
            [job],
            itertools.repeat(0),
            2,
            "cpu",
            style_coefficient=MAX_STYLE_BONUS,
        )
        rewards = batch[5]

        assert rewards[0][0] == pytest.approx(0.0)
        assert rewards[0][1] == pytest.approx(0.0)
        assert rewards[1][0] == pytest.approx(1.0 + MAX_STYLE_BONUS)
        assert rewards[1][1] == pytest.approx(-1.0 + MAX_STYLE_BONUS)
        assert sorted(finals) == [-1.0, 1.0], "telemetry finals stay pure"
        assert TEL["style_bonus"] == pytest.approx(2 * MAX_STYLE_BONUS)


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
        job.conditions = {
            0: condition_from_profile(1000, 500, 0),
            1: condition_from_profile(1000, 500, 1000),
        }
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

    def test_harvesters_do_not_count_as_army_diversity(self) -> None:
        raw = [0] * FEATURES
        raw[FEATURE_NAMES.index("my_harvesters")] = 100
        raw[FEATURE_NAMES.index("my_sentinels")] = 1
        assert comp_entropy(raw) == 0.0

    def test_an_empty_army_scores_zero_not_nan(self) -> None:
        assert comp_entropy([0] * FEATURES) == 0.0


class TestMixBonus:
    def test_the_terminal_pays_by_own_mix_entropy(self) -> None:
        # Seat 0 ends with a perfect two-way mix (1 bit -> half the
        # bonus); seat 1 ends with a sentinel monoculture (nothing).
        mixed = _view(0.5)
        mixed.raw[FEATURE_NAMES.index("my_sentinels")] = 11
        mixed.raw[FEATURE_NAMES.index("my_airair")] = 11
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
        job.conditions = {
            0: condition_from_profile(1000, 500, 0),
            1: condition_from_profile(1000, 500, 1000),
        }
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

    def test_value_diversity_cannot_hide_body_spam(self) -> None:
        # 50 Scuttlers and 10 Bombards split purchase value evenly, but
        # five bodies in six are still Scuttlers. The count lens must be
        # the limiting one.
        spammed = _view(0.5)
        spammed.raw[FEATURE_NAMES.index("my_scuttlers")] = 50
        spammed.raw[FEATURE_NAMES.index("my_bombards")] = 10
        mono = _view(0.5)
        mono.raw[FEATURE_NAMES.index("my_sentinels")] = 20
        frames = [
            Frame(False, 16, seats={0: spammed, 1: mono}),
            Frame(True, 32, winners=[0]),
        ]
        fresh = Frame(False, 48, seats={0: _view(0.1), 1: _view(0.1)})
        worker = cast("Worker", _ResettingWorker(frames, fresh))
        job = Job(worker, "self", 0, pathlib.Path("."), np.random.default_rng(0), "cpu")
        job.frame = Frame(False, 0, seats={0: _view(0.1), 1: _view(0.1)})
        job.conditions = {
            0: condition_from_profile(1000, 500, 0),
            1: condition_from_profile(1000, 500, 1000),
        }
        torch.manual_seed(0)
        policy = make_policy("mlp")
        policy.eval()
        batch, _last_val, _finals = rollout(
            policy, [job], itertools.repeat(0), 2, "cpu", mix_bonus=0.1
        )
        count_entropy = -(
            (5.0 / 6.0) * np.log2(5.0 / 6.0) + (1.0 / 6.0) * np.log2(1.0 / 6.0)
        )
        reward = batch[5][1][0]
        assert reward == pytest.approx(1.0 + 0.1 * count_entropy / 2.0)
        assert reward < 1.0 + 0.05, (
            "equal purchase value must not earn the full two-kind credit"
        )

    def test_the_bonus_integrates_the_episode_not_only_the_final_snapshot(
        self,
    ) -> None:
        mixed = _view(0.5)
        mixed.raw[FEATURE_NAMES.index("my_sentinels")] = 11
        mixed.raw[FEATURE_NAMES.index("my_lancers")] = 9
        mono = _view(0.5)
        mono.raw[FEATURE_NAMES.index("my_sentinels")] = 20
        frames = [
            Frame(False, 16, seats={0: mixed, 1: mono}),
            Frame(True, 32, seats={0: mono, 1: mono}, winners=[0]),
        ]
        fresh = Frame(False, 48, seats={0: _view(0.1), 1: _view(0.1)})
        worker = cast("Worker", _ResettingWorker(frames, fresh))
        job = Job(worker, "self", 0, pathlib.Path("."), np.random.default_rng(0), "cpu")
        job.frame = Frame(False, 0, seats={0: _view(0.1), 1: _view(0.1)})
        job.conditions = {
            0: condition_from_profile(1000, 500, 0),
            1: condition_from_profile(1000, 500, 1000),
        }
        torch.manual_seed(0)
        policy = make_policy("mlp")
        policy.eval()
        batch, _last_val, _finals = rollout(
            policy, [job], itertools.repeat(0), 2, "cpu", mix_bonus=0.1
        )
        assert batch[5][1][0] > 1.0, (
            "the earlier mixed army must still earn credit after a "
            "sentinel-only terminal snapshot"
        )


def _forced_view(action: int | ActionPlan) -> SeatView:
    """A view whose mask permits exactly one action in each head."""
    if isinstance(action, int):
        selected = [0, 24, 25]
        for head_index, head in enumerate(ACTION_HEADS):
            if action in head:
                selected[head_index] = action
                break
        else:
            raise ValueError(f"unknown action {action}")
        plan = tuple(selected)
    else:
        plan = action
    view = _view(0.5)
    view.mask = np.zeros(ACTIONS, dtype=bool)
    view.mask[list(plan)] = True
    return view


class TestSalvageBonus:
    """Salvage seeding pays completed dismantles, never selected intent."""

    def _rollout(
        self, initial: Frame, frames: list[Frame], steps: int
    ) -> tuple[np.ndarray, list[float]]:
        fresh = Frame(False, 99, seats={0: _forced_view(0), 1: _forced_view(0)})
        worker = cast("Worker", _ResettingWorker(frames, fresh))
        job = Job(worker, "self", 0, pathlib.Path("."), np.random.default_rng(0), "cpu")
        job.frame = initial
        job.conditions = {
            0: condition_from_profile(1000, 500, 0),
            1: condition_from_profile(1000, 500, 1000),
        }
        torch.manual_seed(0)
        policy = make_policy("mlp")
        policy.eval()
        TEL.clear()
        batch, _last_val, finals = rollout(
            policy, [job], itertools.repeat(0), steps, "cpu", salvage_bonus=0.05
        )
        return batch[5], finals

    def test_a_completed_dismantle_pays_once_per_episode(self) -> None:
        initial = Frame(False, 0, seats={0: _forced_view(0), 1: _forced_view(0)})
        frames = [
            Frame(
                False,
                16,
                seats={0: _forced_view(0), 1: _forced_view(0)},
                effects={0: SeatEffects(buildings_salvaged=1)},
            ),
            Frame(
                True,
                32,
                winners=[0],
                effects={0: SeatEffects(buildings_salvaged=2)},
            ),
        ]
        rew, finals = self._rollout(initial, frames, 2)
        assert rew[1][0] == pytest.approx(1.0 + 0.05)
        assert rew[1][1] == pytest.approx(-1.0)
        assert sorted(finals) == [-1.0, 1.0], "telemetry finals stay pure"
        assert TEL["buildings_salvaged"] == 3
        assert TEL["ep_salvage"] == 1

    def test_a_sampled_verb_without_a_completed_dismantle_earns_nothing(
        self,
    ) -> None:
        initial = Frame(
            False,
            0,
            seats={0: _forced_view(league.SALVAGE_ACTION), 1: _forced_view(0)},
        )
        terminal = Frame(True, 16, winners=[0])
        rew, _finals = self._rollout(initial, [terminal], 1)
        assert rew[0][0] == pytest.approx(1.0)
        assert TEL["salvage_action_samples"] == 1
        assert TEL["ep_salvage"] == 0

    def test_the_effect_flag_resets_at_the_episode_boundary(self) -> None:
        initial = Frame(False, 0, seats={0: _forced_view(0), 1: _forced_view(0)})
        frames = [
            Frame(
                True,
                16,
                winners=[0],
                effects={0: SeatEffects(buildings_salvaged=1)},
            ),
            Frame(True, 32, winners=[0]),
        ]
        rew, _finals = self._rollout(initial, frames, 2)
        assert rew[0][0] == pytest.approx(1.0 + 0.05)
        assert rew[1][0] == pytest.approx(1.0)
        assert TEL["ep_salvage"] == 1


class TestRepairBonus:
    """The v6 seeding pays successful work, never sampled intent."""

    def _rollout(
        self, initial: Frame, frames: list[Frame], steps: int
    ) -> tuple[np.ndarray, list[float]]:
        fresh = Frame(False, 99, seats={0: _forced_view(0), 1: _forced_view(0)})
        worker = cast("Worker", _ResettingWorker(frames, fresh))
        job = Job(worker, "self", 0, pathlib.Path("."), np.random.default_rng(0), "cpu")
        job.frame = initial
        job.conditions = {
            0: condition_from_profile(1000, 500, 0),
            1: condition_from_profile(1000, 500, 1000),
        }
        torch.manual_seed(0)
        policy = make_policy("mlp")
        policy.eval()
        TEL.clear()
        batch, _last_val, finals = rollout(
            policy, [job], itertools.repeat(0), steps, "cpu", repair_bonus=0.05
        )
        return batch[5], finals

    def test_each_successful_weld_effect_pays_once(self) -> None:
        initial = Frame(False, 0, seats={0: _forced_view(0), 1: _forced_view(0)})
        terminal = Frame(
            True,
            16,
            winners=[0],
            effects={
                0: SeatEffects(
                    repair_unit_commands=1,
                    repair_unit_hp_restored=12,
                    unit_hp_restored=12,
                ),
                1: SeatEffects(buildings_completed=("repair_bay",)),
            },
        )
        rew, finals = self._rollout(initial, [terminal], 1)
        assert rew[0][0] == pytest.approx(1.0 + 0.05), "the field weld pays"
        assert rew[0][1] == pytest.approx(-1.0 + 0.05), "the Bay pays, even in defeat"
        assert sorted(finals) == [-1.0, 1.0], "telemetry finals stay pure"
        assert TEL["ep_repair"] == 1
        assert TEL["ep_bay"] == 1

    def test_a_sampled_or_lowered_verb_without_an_effect_earns_nothing(self) -> None:
        initial = Frame(False, 0, seats={0: _forced_view(22), 1: _forced_view(0)})
        terminal = Frame(
            True,
            16,
            winners=[0],
            effects={0: SeatEffects(repair_unit_commands=1)},
        )
        rew, _finals = self._rollout(initial, [terminal], 1)
        assert rew[0][0] == pytest.approx(1.0)
        assert TEL["ep_repair_commanded"] == 1
        assert TEL["ep_repair"] == 0

    def test_a_seat_that_completes_both_effects_earns_both(self) -> None:
        initial = Frame(False, 0, seats={0: _forced_view(0), 1: _forced_view(0)})
        frames = [
            Frame(
                False,
                16,
                seats={0: _forced_view(0), 1: _forced_view(0)},
                effects={
                    0: SeatEffects(
                        repair_unit_commands=1,
                        repair_unit_hp_restored=8,
                        unit_hp_restored=8,
                    )
                },
            ),
            Frame(
                True,
                32,
                winners=[0],
                effects={0: SeatEffects(buildings_completed=("repair_bay",))},
            ),
        ]
        rew, _finals = self._rollout(initial, frames, 2)
        assert rew[1][0] == pytest.approx(1.0 + 0.10), "both verbs, both bonuses"
        assert rew[1][1] == pytest.approx(-1.0), "an idle seat earns nothing"

    def test_the_flags_reset_at_the_episode_boundary(self) -> None:
        initial = Frame(False, 0, seats={0: _forced_view(0), 1: _forced_view(0)})
        frames = [
            Frame(
                True,
                16,
                winners=[0],
                effects={
                    0: SeatEffects(
                        repair_unit_commands=1,
                        repair_unit_hp_restored=8,
                        unit_hp_restored=8,
                    ),
                    1: SeatEffects(
                        repair_unit_commands=1,
                        repair_unit_hp_restored=8,
                        unit_hp_restored=8,
                    ),
                },
            ),
            Frame(True, 32, winners=[0]),
        ]
        rew, _finals = self._rollout(initial, frames, 2)
        assert rew[0][0] == pytest.approx(1.0 + 0.05)
        # Episode 2 picked nothing (the fresh frame forces Idle); a
        # stale flag would let one early weld pay rent forever.
        assert rew[1][0] == pytest.approx(1.0)
        assert rew[1][1] == pytest.approx(-1.0)


class TestStructureBonus:
    def test_distinct_completed_turret_and_array_each_pay_once(self) -> None:
        initial = Frame(False, 0, seats={0: _forced_view(0), 1: _forced_view(0)})
        frames = [
            Frame(
                False,
                16,
                seats={0: _forced_view(0), 1: _forced_view(0)},
                effects={0: SeatEffects(buildings_completed=("turret",))},
            ),
            Frame(
                True,
                32,
                winners=[0],
                effects={
                    0: SeatEffects(buildings_completed=("turret", "array")),
                    1: SeatEffects(buildings_completed=("repair_bay",)),
                },
            ),
        ]
        fresh = Frame(False, 99, seats={0: _forced_view(0), 1: _forced_view(0)})
        worker = cast("Worker", _ResettingWorker(frames, fresh))
        job = Job(worker, "self", 0, pathlib.Path("."), np.random.default_rng(0), "cpu")
        job.frame = initial
        job.conditions = {
            0: condition_from_profile(1000, 500, 0),
            1: condition_from_profile(1000, 500, 1000),
        }
        policy = make_policy("mlp")
        policy.eval()
        TEL.clear()

        batch, _last_val, _finals = rollout(
            policy,
            [job],
            itertools.repeat(0),
            2,
            "cpu",
            structure_bonus=0.02,
        )
        rewards = batch[5]
        assert rewards[1][0] == pytest.approx(1.04)
        assert rewards[1][1] == pytest.approx(-1.0)
        assert TEL["ep_build_turret"] == 1
        assert TEL["ep_build_array"] == 1

    def test_cli_bonus_is_finite_and_capped_per_kind(self) -> None:
        assert bounded_structure_bonus("0.02") == 0.02
        for value in ("-0.001", "0.0201", "nan", "inf"):
            with pytest.raises(argparse.ArgumentTypeError):
                bounded_structure_bonus(value)


_ACTIVE_CAP = {
    "capped": True,
    "ticks": 20_000,
    "last_progress_tick": 19_000,
    "activity": {"last_combat_tick": 19_900, "last_economy_tick": 19_500},
    "final_economy": {
        "remaining_map_salvage": 500,
        "seats": [
            {
                "completed_reclaimers": 1,
                "living_foundries": 1,
                "resigned": False,
                "recovery_income_active": True,
            }
        ],
    },
}
_INACTIVE_CAP = {
    "capped": True,
    "ticks": 20_000,
    "last_progress_tick": 5_000,
    "activity": {"last_combat_tick": 5_000, "last_economy_tick": 5_000},
    "final_economy": {
        "remaining_map_salvage": 0,
        "seats": [
            {
                "completed_reclaimers": 0,
                "living_foundries": 1,
                "resigned": False,
                "recovery_income_active": False,
            }
        ],
    },
}
_PROBE_COHORT = {
    "seats": 82,
    "combat_seats": 82,
    "entropy_bits": 2.134,
    "seat_entropy": {"mean": 1.9, "p10": 1.42, "p25": 1.7, "median": 2.0},
    "mean_share": {"lancer": 0.599, "sentinel": 0.401},
    "count_entropy_bits": 1.967,
    "seat_count_entropy": {
        "mean": 1.7,
        "p10": 1.37,
        "p25": 1.5,
        "median": 1.8,
    },
    "seat_count_dominance": {"mean": 0.52, "p90": 0.641, "max": 0.82},
    "mean_count_share": {"lancer": 0.301, "scuttler": 0.447},
    "combat_entropy_bits": 2.134,
    "seat_combat_entropy": {
        "mean": 1.9,
        "p10": 1.42,
        "p25": 1.7,
        "median": 2.0,
    },
    "mean_combat_share": {"lancer": 0.599, "sentinel": 0.401},
    "combat_count_entropy_bits": 1.967,
    "seat_combat_count_entropy": {
        "mean": 1.7,
        "p10": 1.37,
        "p25": 1.5,
        "median": 1.8,
    },
    "seat_combat_count_dominance": {
        "mean": 0.52,
        "p90": 0.641,
        "max": 0.82,
    },
    "mean_combat_count_share": {"lancer": 0.301, "scuttler": 0.447},
    "mean_buildings": {"fabricator": 1.2},
    "seats_with_building": {"fabricator": 0.85, "repairbay": 0.125},
    "competitive_mean_buildings": {"fabricator": 1.2},
    "competitive_seats_with_building": {
        "fabricator": 0.85,
        "repairbay": 0.125,
    },
}
_PROBE_PAYLOAD = {
    "schema": 6,
    "overall": {
        "matches": 50,
        "decided": 41,
        "capped": 9,
        **_PROBE_COHORT,
        # Deliberately unlike the competitive combat fields: preserved
        # unprefixed composition stays diagnostic.
        "entropy_bits": 9.0,
        "seat_entropy": {"p10": 8.0},
        "mean_share": {"sentinel": 1.0},
        "count_entropy_bits": 7.0,
        "seat_count_entropy": {"p10": 6.0},
        "seat_count_dominance": {"p90": 0.99},
        "mean_count_share": {"scuttler": 1.0},
    },
    "matches": [
        *[{"capped": False} for _ in range(41)],
        *[dict(_ACTIVE_CAP) for _ in range(8)],
        _INACTIVE_CAP,
    ],
    "decided": {
        "decided": 41,
        **_PROBE_COHORT,
    },
}


class TestProbeCanary:
    def test_the_row_reads_decisiveness_mix_and_both_share_tables(self) -> None:
        assert probe_canary(_PROBE_PAYLOAD) == {
            "matches": 50,
            "decided": 41,
            "capped": 9,
            "competitive_seats": 82,
            "active_caps": 8,
            "unhealthy_caps": 1,
            "resource_exhausted_caps": 1,
            "entropy_bits": 2.13,
            "seat_p10": 1.42,
            "count_entropy_bits": 1.97,
            "seat_count_p10": 1.37,
            "count_dominance_p90": 0.641,
            "unit_share": {"lancer": 0.599, "sentinel": 0.401},
            "count_share": {"lancer": 0.301, "scuttler": 0.447},
            "building_share": {"fabricator": 0.85, "repairbay": 0.125},
        }

    def test_missing_survivor_spreads_report_none_instead_of_crashing(self) -> None:
        payload = json.loads(json.dumps(_PROBE_PAYLOAD))
        payload["overall"]["seat_combat_entropy"] = None
        payload["overall"]["seat_combat_count_entropy"] = None
        payload["overall"]["seat_combat_count_dominance"] = None
        row = probe_canary(payload)
        assert row["seat_p10"] is None
        assert row["seat_count_p10"] is None
        assert row["count_dominance_p90"] is None


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
        rng_before = torch.get_rng_state().clone()
        row = composition_probe(
            policy, "mlp", 100, tmp_path, str(driver), "some/scenarios", "medium", 2
        )
        assert torch.equal(torch.get_rng_state(), rng_before)
        assert row["decided"] == 41
        assert row["competitive_seats"] == 82
        assert row["seat_p10"] == 1.42
        assert row["seat_count_p10"] == 1.37
        assert row["count_dominance_p90"] == 0.641
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
        stale["schema"] = 5
        driver = self._fake_driver(tmp_path, stale)
        torch.manual_seed(0)
        policy = make_policy("mlp")
        rng_before = torch.get_rng_state().clone()
        with pytest.raises(RuntimeError, match="schema 5"):
            composition_probe(policy, "mlp", 5, tmp_path, str(driver), "s", "medium", 1)
        assert torch.equal(torch.get_rng_state(), rng_before)

    def test_a_future_probe_schema_is_refused(self, tmp_path: pathlib.Path) -> None:
        future = json.loads(json.dumps(_PROBE_PAYLOAD))
        future["schema"] = 7
        driver = self._fake_driver(tmp_path, future)
        torch.manual_seed(0)
        policy = make_policy("mlp")
        with pytest.raises(RuntimeError, match="schema 7"):
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
        job.conditions = {
            s: condition_from_profile(1000, 500, faction_knob(s)) for s in range(seats)
        }
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
        # A destroyed asset landing on the terminal cadence must not
        # escape shaping: terminal Phi is zero, so the previous owned
        # health value is fully settled into the terminal reward.
        rich = _tech_view(0)
        rich.raw[F["my_building_health_value"]] = 300
        terminal_rich = _tech_view(0)
        terminal_rich.raw[F["my_building_health_value"]] = 900
        frames = [
            Frame(False, 16, seats={0: rich, 1: _tech_view(0)}),
            Frame(
                True,
                32,
                winners=[1],
                seats={0: terminal_rich, 1: _tech_view(0)},
            ),
        ]
        rew = self._run(frames)
        drop = -SHAPE_K * (300 / 500.0)
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


def test_the_potential_is_neutral_across_owned_value_forms() -> None:
    """Bank, cargo, queues, sites, and healthy assets are one conserved lens."""
    base = [0] * oxide_gym.FEATURES
    forms = (
        "scrap",
        "carried_scrap",
        "queued_unit_value",
        "construction_site_value",
        "my_unit_health_value",
        "my_building_health_value",
    )
    readings = []
    for name in forms:
        raw = list(base)
        raw[league.F[name]] = 300
        readings.append(league.potential(raw))
    assert readings == pytest.approx([0.6] * len(forms))


def test_the_potential_ignores_old_composition_proxies() -> None:
    raw = [0] * oxide_gym.FEATURES
    for name in (
        "my_strength",
        "my_building_value",
        "damaged_unit_value",
        "my_harvesters",
    ):
        raw[league.F[name]] = 500
    assert league.potential(raw) == 0.0


def test_unchanged_owned_value_gets_only_the_canonical_discount_correction() -> None:
    owned = _tech_view(0)
    owned.raw[F["scrap"]] = 500
    worker = cast(
        "Worker",
        _ScriptedWorker([Frame(False, 16, seats={0: owned, 1: owned})]),
    )
    job = Job(worker, "self", 0, pathlib.Path("."), np.random.default_rng(0), "cpu")
    job.frame = Frame(False, 0, seats={0: owned, 1: owned})
    job.conditions = {
        0: condition_from_profile(1000, 500, 0),
        1: condition_from_profile(1000, 500, 1000),
    }
    policy = make_policy("mlp")
    policy.eval()

    batch, _last_val, _finals = rollout(
        policy,
        [job],
        itertools.repeat(0),
        1,
        "cpu",
    )
    correction = SHAPE_K * (SHAPE_GAMMA - 1.0)
    np.testing.assert_allclose(batch[5][0], [correction, correction])


def test_dead_padding_keeps_the_canonical_frozen_potential_transition() -> None:
    owned = _tech_view(0)
    owned.raw[F["scrap"]] = 500
    live = _tech_view(0)
    frames = [
        Frame(False, 16, seats={0: live}),
        Frame(False, 32, seats={0: live}),
    ]
    worker = cast("Worker", _ScriptedWorker(frames))
    job = Job(worker, "self", 0, pathlib.Path("."), np.random.default_rng(0), "cpu")
    job.frame = Frame(False, 0, seats={0: live, 1: owned})
    job.conditions = {
        0: condition_from_profile(1000, 500, 0),
        1: condition_from_profile(1000, 500, 1000),
    }
    policy = make_policy("mlp")
    policy.eval()

    batch, _last_val, _finals = rollout(
        policy,
        [job],
        itertools.repeat(0),
        2,
        "cpu",
    )
    correction = SHAPE_K * (SHAPE_GAMMA - 1.0)
    np.testing.assert_allclose(batch[5][:, 1], [correction, correction])
    np.testing.assert_array_equal(batch[7][:, 1], [True, False])
