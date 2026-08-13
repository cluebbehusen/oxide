"""The autopilot's style-signature constraint: parsing and fitness.

The gate itself is the Rust `candidate_profile_behavior_gates` test;
these cover the autopilot's reading of its output and the promise that
a style failure can never outrank a clean pass, whatever the cup says.
"""

from autopilot import fitness, style_failures

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
