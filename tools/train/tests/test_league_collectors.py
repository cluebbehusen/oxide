"""Sharded rollout collection: the seed partition, the fixed shard
order of batch assembly, and the job layout every collector rebuilds.

These tests exercise the pure machinery only — no collector subprocess
is spawned, so the suite stays fast and free of torch multiprocessing.
"""

import pathlib
from typing import TYPE_CHECKING, cast

import numpy as np
import pytest

from league import (
    assemble_shard_batches,
    assign_roles,
    collector_torch_seed,
    lane_kinds_for_layout,
    parse_opponent_mix,
    realized_learner_row_mix,
    realized_row_mix_from_lane_kinds,
    role_layout,
    shard_job_indices,
    shard_seed,
)

if TYPE_CHECKING:
    from oxide_gym import Worker


class TestShardJobIndices:
    def test_one_collector_owns_every_job_in_order(self) -> None:
        assert shard_job_indices(8, 1) == [tuple(range(8))]

    def test_shards_are_contiguous_balanced_and_cover_every_job(self) -> None:
        for workers in (1, 2, 3, 5, 8, 13):
            for collectors in range(1, workers + 1):
                shards = shard_job_indices(workers, collectors)
                assert len(shards) == collectors
                flat = [index for shard in shards for index in shard]
                assert flat == list(range(workers))
                sizes = [len(shard) for shard in shards]
                assert all(sizes)
                assert max(sizes) - min(sizes) <= 1

    @pytest.mark.parametrize(
        ("workers", "collectors"),
        [(4, 0), (4, -1), (4, 5), (0, 1)],
    )
    def test_rejects_impossible_partitions(
        self,
        workers: int,
        collectors: int,
    ) -> None:
        with pytest.raises(ValueError, match=r"collectors|worker"):
            shard_job_indices(workers, collectors)


class TestShardSeed:
    def test_a_single_shard_walks_the_original_sequence(self) -> None:
        assert [shard_seed(50_000, 0, 1, index) for index in range(6)] == [
            50_000 + index for index in range(6)
        ]

    def test_shards_partition_the_original_sequence_without_overlap(self) -> None:
        shards = 3
        drawn = sorted(
            shard_seed(50_000, shard, shards, index)
            for shard in range(shards)
            for index in range(7)
        )
        assert drawn == list(range(50_000, 50_000 + shards * 7))

    def test_rejects_a_shard_outside_the_partition(self) -> None:
        with pytest.raises(ValueError, match="shard"):
            shard_seed(50_000, 3, 3, 0)
        with pytest.raises(ValueError, match="index"):
            shard_seed(50_000, 0, 3, -1)


class TestCollectorTorchSeed:
    def test_deterministic_and_distinct_per_collector_and_update(self) -> None:
        seeds = {
            (shard, update): collector_torch_seed(0, shard, update)
            for shard in range(4)
            for update in range(1, 6)
        }
        assert len(set(seeds.values())) == len(seeds)
        assert seeds[2, 3] == collector_torch_seed(0, 2, 3)

    def test_rejects_negative_components(self) -> None:
        with pytest.raises(ValueError, match="non-negative"):
            collector_torch_seed(0, -1, 1)


def _shard(
    width: int,
    lanes: int,
    tag: float,
    finals: list[float],
) -> tuple[tuple[np.ndarray, ...], np.ndarray, list[float]]:
    """One fake collector shard whose every cell names its coordinates."""
    time_axis = np.arange(width, dtype=np.float32)[:, None]
    lane_axis = np.arange(lanes, dtype=np.float32)[None, :]
    cell = tag * 1000.0 + lane_axis * 100.0 + time_axis
    obs = np.repeat(cell[:, :, None], 2, axis=2).astype(np.float32)
    mask = np.ones((width, lanes, 4), dtype=bool)
    act = np.repeat(cell[:, :, None], 4, axis=2).astype(np.int64)
    logp = cell.astype(np.float32)
    val = (cell + 0.5).astype(np.float32)
    rew = (cell + 0.25).astype(np.float32)
    done = np.zeros((width, lanes), dtype=bool)
    done[-1] = True
    valid = np.ones((width, lanes), dtype=bool)
    last_val = np.full(lanes, tag, dtype=np.float32)
    return (obs, mask, act, logp, val, rew, done, valid), last_val, finals


class TestAssembleShardBatches:
    def test_window_lanes_concatenate_in_shard_order(self) -> None:
        first = _shard(4, 2, tag=1.0, finals=[1.0])
        second = _shard(4, 3, tag=2.0, finals=[-1.0, 0.0])
        batch, last_val, finals = assemble_shard_batches([first, second], "windows")
        for part in range(8):
            np.testing.assert_array_equal(
                batch[part],
                np.concatenate([first[0][part], second[0][part]], axis=1),
            )
        np.testing.assert_array_equal(last_val, np.concatenate([first[1], second[1]]))
        assert finals == [1.0, -1.0, 0.0]

    def test_shard_order_is_positional_not_content_derived(self) -> None:
        first = _shard(4, 2, tag=1.0, finals=[1.0])
        second = _shard(4, 3, tag=2.0, finals=[-1.0, 0.0])
        batch, last_val, finals = assemble_shard_batches([second, first], "windows")
        np.testing.assert_array_equal(batch[0][:, :3], second[0][0])
        np.testing.assert_array_equal(batch[0][:, 3:], first[0][0])
        np.testing.assert_array_equal(last_val, np.concatenate([second[1], first[1]]))
        assert finals == [-1.0, 0.0, 1.0]

    def test_fixed_windows_refuse_disagreeing_widths(self) -> None:
        with pytest.raises(ValueError, match="width"):
            assemble_shard_batches(
                [_shard(4, 2, 1.0, []), _shard(3, 2, 2.0, [])],
                "windows",
            )

    def test_episode_shards_pad_with_frozen_invalid_rows(self) -> None:
        long = _shard(5, 2, tag=1.0, finals=[1.0])
        short = _shard(3, 1, tag=2.0, finals=[0.0])
        batch, last_val, finals = assemble_shard_batches([long, short], "episodes")
        obs, _mask, act, logp, val, rew, done, valid = batch
        assert obs.shape[:2] == (5, 3)
        short_batch = short[0]
        for row in (3, 4):
            np.testing.assert_array_equal(obs[row, 2], short_batch[0][-1, 0])
            np.testing.assert_array_equal(act[row, 2], short_batch[2][-1, 0])
            assert logp[row, 2] == short_batch[3][-1, 0]
            assert val[row, 2] == short_batch[4][-1, 0]
            assert rew[row, 2] == 0.0
            assert bool(done[row, 2])
            assert not bool(valid[row, 2])
        for part in range(8):
            np.testing.assert_array_equal(batch[part][:, :2], long[0][part])
        np.testing.assert_array_equal(last_val, np.concatenate([long[1], short[1]]))
        assert finals == [1.0, 0.0]

    def test_requires_at_least_one_shard(self) -> None:
        with pytest.raises(ValueError, match="shard"):
            assemble_shard_batches([], "episodes")


class _LayoutWorker:
    """Job construction touches only the optional profile catalog."""

    profile_catalog = None


class TestRoleLayout:
    MIX = "self=0.25,past=0.10,overseer=0.30,rusher=0.10,ffa=0.10,team=0.15"

    def _jobs_and_layout(self) -> tuple[list, list[tuple[str, int]]]:
        mix = parse_opponent_mix(self.MIX)
        layout = role_layout(mix, 8)
        workers = [cast("Worker", _LayoutWorker()) for _ in range(8)]
        jobs = assign_roles(
            workers,
            mix,
            pathlib.Path("."),
            np.random.default_rng(0),
            "cpu",
        )
        return jobs, layout

    def test_layout_matches_the_jobs_assign_roles_builds(self) -> None:
        jobs, layout = self._jobs_and_layout()
        assert [job.kind for job in jobs] == [kind for kind, _seat in layout]
        for job, (kind, seat) in zip(jobs, layout, strict=True):
            if kind == "self":
                assert job.learner_seats == [0, 1]
            elif kind == "team":
                assert job.learner_seats == [0, 2]
            elif kind == "team2":
                assert job.learner_seats == [seat * 2]
            else:
                assert job.learner_seats == [seat]

    def test_lane_kinds_match_the_jobs_learner_lanes(self) -> None:
        jobs, layout = self._jobs_and_layout()
        assert lane_kinds_for_layout(layout) == [
            job.kind for job in jobs for _seat in job.learner_seats
        ]

    def test_lane_kind_row_mix_matches_the_job_reading(self) -> None:
        jobs, layout = self._jobs_and_layout()
        lane_kinds = lane_kinds_for_layout(layout)
        assert realized_row_mix_from_lane_kinds(lane_kinds) == (
            realized_learner_row_mix(jobs)
        )
        valid = np.zeros((4, len(lane_kinds)), dtype=bool)
        valid[:2, ::2] = True
        valid[1:, 1] = True
        assert realized_row_mix_from_lane_kinds(lane_kinds, valid) == (
            realized_learner_row_mix(jobs, valid)
        )

    def test_row_mix_refuses_a_mismatched_lane_count(self) -> None:
        with pytest.raises(ValueError, match="lane"):
            realized_row_mix_from_lane_kinds(
                ["self", "self"],
                np.ones((3, 3), dtype=bool),
            )
