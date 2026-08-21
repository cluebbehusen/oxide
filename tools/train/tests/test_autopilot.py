"""The autopilot's style-signature constraint: parsing and fitness.

The gate itself is the Rust `candidate_profile_behavior_gates` test;
these cover the autopilot's reading of its output and the promise that
a style failure can never outrank a clean pass, whatever the cup says.
"""

import json
import subprocess
from typing import TYPE_CHECKING

import pytest

if TYPE_CHECKING:
    import pathlib

from autopilot import fitness, phase_checkpoint, run_battery, style_failures

SUMMARY_OK = (
    "Turtle: 3 unique vectors [...]\n"
    "style-family signatures / 7 seeds: "
    "development 7, fortification 7, force 7, mobile pressure 7\n"
)
SUMMARY_ERODED = (
    "style-family signatures / 7 seeds: "
    "development 0, fortification 7, force 0, mobile pressure 7\n"
)
EARLY_PANIC = (
    "thread 'candidate_profile_behavior_gates' panicked at bot_profiles.rs:\n"
    "Turtle profiles must diverge in at least 4/7 seeds\n"
)


def test_clean_signatures_yield_no_failures() -> None:
    assert style_failures(SUMMARY_OK) == []


def test_eroded_families_are_each_named() -> None:
    failures = style_failures(SUMMARY_ERODED)
    assert len(failures) == 2
    assert any("development" in f for f in failures)
    assert any("force" in f for f in failures)
    assert all("0/7" in f for f in failures)


def test_multiword_family_names_survive_the_parse() -> None:
    report = "style-family signatures / 7 seeds: mobile pressure 2\n"
    assert style_failures(report) == ["STYLE GATE FAIL: mobile pressure held 2/7 seeds"]


def test_a_run_that_dies_before_the_summary_still_reports() -> None:
    assert len(style_failures(EARLY_PANIC)) == 1
    assert style_failures("") == [
        "STYLE GATE FAIL: gate did not reach the signature summary"
    ]


def test_style_failure_can_never_outrank_a_clean_pass() -> None:
    strong_but_eroded = {
        "fun_gate_pass": True,
        "style_gate_pass": False,
        "style_gate_failures": ["STYLE GATE FAIL: force held 0/7 seeds"],
        "overseer_wins": 60,
        "rusher_wins": 60,
    }
    weak_but_clean = {
        "fun_gate_pass": True,
        "style_gate_pass": True,
        "style_gate_failures": [],
        "overseer_wins": 1,
        "rusher_wins": 0,
    }
    assert fitness(weak_but_clean) > fitness(strong_but_eroded)


def test_both_gates_are_required() -> None:
    fun_only = {"fun_gate_pass": True, "style_gate_pass": False}
    style_only = {"fun_gate_pass": False, "style_gate_pass": True}
    both = {"fun_gate_pass": True, "style_gate_pass": True}
    assert not fitness(fun_only)[0]
    assert not fitness(style_only)[0]
    assert fitness(both)[0]


class TestCrashResume:
    """The two resume shortcuts the audit measured at zero execution.

    A wrong reuse silently trains the next generation from the wrong
    parent (or skips a battery that should re-run), so each witness
    condition gets a direct row.
    """

    _seq = 0

    @classmethod
    def _run_dir(
        cls, tmp_path: pathlib.Path, last_row: str | None, checkpoints: int = 1
    ) -> pathlib.Path:
        cls._seq += 1
        run_dir = tmp_path / f"g0m{cls._seq}"
        (run_dir / "pool").mkdir(parents=True)
        for index in range(checkpoints):
            (run_dir / "pool" / f"ckpt-{index:06}.pt").write_bytes(b"weights")
        if last_row is not None:
            (run_dir / "log.jsonl").write_text(last_row)
        return run_dir

    def test_a_completed_phase_reuses_its_final_checkpoint(
        self, tmp_path: pathlib.Path
    ) -> None:
        run_dir = self._run_dir(
            tmp_path, '{"phase_update": 59}\n{"phase_update": 60}\n', checkpoints=3
        )
        assert phase_checkpoint(run_dir, 60) == run_dir / "pool" / "ckpt-000002.pt"

    def test_an_incomplete_phase_retrains(self, tmp_path: pathlib.Path) -> None:
        run_dir = self._run_dir(tmp_path, '{"phase_update": 59}\n')
        assert phase_checkpoint(run_dir, 60) is None

    def test_a_corrupt_or_missing_log_retrains(self, tmp_path: pathlib.Path) -> None:
        assert phase_checkpoint(self._run_dir(tmp_path, "not json\n"), 60) is None
        assert phase_checkpoint(self._run_dir(tmp_path, None), 60) is None
        assert phase_checkpoint(self._run_dir(tmp_path, ""), 60) is None

    def test_missing_checkpoints_retrain_even_when_the_log_says_done(
        self, tmp_path: pathlib.Path
    ) -> None:
        run_dir = self._run_dir(tmp_path, '{"phase_update": 60}\n', checkpoints=0)
        assert phase_checkpoint(run_dir, 60) is None


class TestBatteryMemo:
    def test_a_matching_memo_is_reused_verbatim(
        self, tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        candidate = tmp_path / "ckpt-000060.pt"
        candidate.write_bytes(b"weights")
        memo = {
            "candidate": "x.json",
            "cup_seeds": 30,
            "fun_gate_pass": True,
            "fun_gate_failures": [],
            "style_gate_pass": True,
            "style_gate_failures": [],
        }
        candidate.with_suffix(".scores.json").write_text(json.dumps(memo))
        # Any subprocess call would mean the memo was NOT trusted.
        monkeypatch.setattr(
            subprocess, "run", lambda *_args, **_kwargs: pytest.fail("battery re-ran")
        )
        assert run_battery(candidate, "driver", 30) == memo

    @pytest.mark.parametrize(
        "stale",
        [
            {"cup_seeds": 10},  # different slate
            {"drop": "style_gate_pass"},  # pre-style-gate memo
        ],
    )
    def test_a_stale_memo_is_not_trusted(
        self, tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch, stale: dict
    ) -> None:
        candidate = tmp_path / "ckpt-000060.pt"
        candidate.write_bytes(b"weights")
        memo = {
            "candidate": "x.json",
            "cup_seeds": 30,
            "fun_gate_pass": True,
            "fun_gate_failures": [],
            "style_gate_pass": True,
            "style_gate_failures": [],
        }
        if "drop" in stale:
            memo.pop(stale["drop"])
        else:
            memo.update(stale)
        candidate.with_suffix(".scores.json").write_text(json.dumps(memo))
        calls = []

        def fake_run(*args: object, **_kwargs: object) -> None:
            calls.append(args)
            raise RuntimeError("battery correctly re-running; stop here")

        monkeypatch.setattr(subprocess, "run", fake_run)
        with pytest.raises(RuntimeError, match="correctly re-running"):
            run_battery(candidate, "driver", 30)
        assert calls, "the stale memo must trigger a fresh battery"
