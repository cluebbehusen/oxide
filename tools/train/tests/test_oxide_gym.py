"""Tests for the ``oxide_gym`` contract: the gym hello carries feature
and condition names plus canonical profile values, and the Worker asserts
the complete shape before any rollout can begin."""

from __future__ import annotations

import io
import json
from typing import TYPE_CHECKING

import numpy as np
import pytest

import oxide_gym

if TYPE_CHECKING:
    from collections.abc import Callable


class TestFeatureContract:
    def test_names_cover_every_feature(self) -> None:
        assert len(oxide_gym.FEATURE_NAMES) == oxide_gym.FEATURES

    def test_scales_cover_every_feature(self) -> None:
        assert oxide_gym.SCALES.shape == (oxide_gym.FEATURES,)

    def test_net_input_is_features_plus_conditioning(self) -> None:
        assert oxide_gym.NET_FEATURES == oxide_gym.FEATURES + oxide_gym.CONDITION_DIMS
        assert oxide_gym.CONDITION_DIMS == 12
        assert oxide_gym.CONDITION_NAMES[-5:] == (
            "profile_economy",
            "profile_air",
            "profile_siege",
            "profile_support",
            "profile_commitment",
        )

    def test_v9_appends_the_exact_roster_tree_and_frame_features(self) -> None:
        expected = [
            "my_wardens",
            "my_tenders",
            "my_excavators",
            "my_scout_flyers",
            "my_interceptors",
            "my_bombers",
            "my_transports",
            "my_sappers",
            "my_breakers",
            "my_avalanches",
            "enemy_interceptors",
            "enemy_bombers",
            "enemy_heavies",
            "airworks_built",
            "crucible_built",
            "my_foundries_built",
            "my_extractors_built",
            "known_frames",
            "nearest_frame_x",
            "nearest_frame_y",
            "nearest_frame_distance",
            "my_upgraded_works",
            "upgrade_candidates",
            "tech_tier",
            "transport_cargo",
            "enemy_foundries_known",
        ]
        assert oxide_gym.FEATURES == 107
        assert oxide_gym.FEATURE_NAMES[-len(expected) :] == expected
        assert oxide_gym.FEATURE_NAMES[80] == "construction_reserve"
        assert all(oxide_gym.SCALE_BY_NAME[name] >= 1 for name in expected)


class _FakeProcess:
    def __init__(self, hello: object, replies: list[object] | None = None) -> None:
        self.stdin = io.StringIO()
        messages = [hello, *(replies or [])]
        self.stdout = io.StringIO(
            "".join(f"{json.dumps(message)}\n" for message in messages)
        )

    def terminate(self) -> None:
        pass


def contract_hello() -> dict:
    ferrous = list(oxide_gym.condition_from_profile(1000, 500, 0))
    cupric = list(oxide_gym.condition_from_profile(1000, 500, 1000))
    ferrous[-5:] = [600, 300, 400, 500, 700]
    cupric[-5:] = [600, 300, 400, 500, 700]
    return {
        "ready": True,
        "version": oxide_gym.GYM_VERSION,
        "features": oxide_gym.FEATURES,
        "actions": oxide_gym.ACTIONS,
        "action_heads": [list(head) for head in oxide_gym.ACTION_HEADS],
        "names": oxide_gym.FEATURE_NAMES,
        "conditioning": oxide_gym.CONDITION_DIMS,
        "condition_names": list(oxide_gym.CONDITION_NAMES),
        "profiled_doctrine": oxide_gym.PROFILED_DOCTRINE_VERSION,
        "default_team_role": "generalist",
        "canonical_profiles": [
            {
                "style": "balanced",
                "variant": 0,
                "name": "ground-combined",
                "aggression": 500,
                "roles": [
                    {
                        "role": "generalist",
                        "conditions": {
                            "ferrous": ferrous,
                            "cupric": cupric,
                        },
                    }
                ],
            }
        ],
    }


class TestFactorizedContract:
    def test_the_heads_are_global_indices_and_preserve_old_rows(self) -> None:
        assert oxide_gym.GYM_VERSION == 9
        assert oxide_gym.ACTIONS == 43
        assert oxide_gym.ACTION_HEADS == (
            (0, 1, 2, 3, 4, 5, 6, 7, 8, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35),
            (24, 9, 10, 11, 12, 13, 14, 15, 21, 22, 23, 36, 37, 38, 39),
            (42, 40),
            (25, 16, 17, 18, 19, 20, 41),
        )

    def test_action_plans_validate_global_head_membership(self) -> None:
        assert oxide_gym.validate_action_plan([8, 24, 42, 20]) == (8, 24, 42, 20)
        with pytest.raises(ValueError, match="head 1"):
            oxide_gym.validate_action_plan((8, 8, 42, 20))
        with pytest.raises(ValueError, match="head 2"):
            oxide_gym.validate_action_plan((8, 24, 41, 20))
        with pytest.raises(ValueError, match="must contain 4"):
            oxide_gym.validate_action_plan((8, 24, 20))

    def test_worker_requires_exact_heads_and_writes_nested_plans(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        hello = contract_hello()
        proc = _FakeProcess(hello)

        def fake_popen(*_args: object, **_kwargs: object) -> _FakeProcess:
            return proc

        monkeypatch.setattr(oxide_gym.subprocess, "Popen", fake_popen)
        worker = oxide_gym.Worker("fake-driver")
        worker.control = (1, 0)
        worker.send_step({0: (0, 24, 42, 25), 1: (8, 23, 40, 20)})

        request = json.loads(proc.stdin.getvalue().splitlines()[-1])
        assert request == {
            "cmd": "step",
            "actions": [[8, 23, 40, 20], [0, 24, 42, 25]],
        }

    def test_worker_rejects_a_different_action_partition(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        hello = contract_hello()
        hello["action_heads"] = [list(range(9)), [24, 9], [25, 16]]

        def fake_popen(*_args: object, **_kwargs: object) -> _FakeProcess:
            return _FakeProcess(hello)

        monkeypatch.setattr(oxide_gym.subprocess, "Popen", fake_popen)
        with pytest.raises(RuntimeError, match="action-head mismatch"):
            oxide_gym.Worker("fake-driver")


class TestCanonicalProfileContract:
    def test_worker_consumes_the_complete_rust_named_vector(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        hello = contract_hello()

        def fake_popen(*_args: object, **_kwargs: object) -> _FakeProcess:
            return _FakeProcess(hello)

        monkeypatch.setattr(oxide_gym.subprocess, "Popen", fake_popen)
        worker = oxide_gym.Worker("fake-driver")
        condition = worker.named_condition(
            "balanced",
            0,
            "generalist",
            "cupric",
        )
        assert condition == tuple(
            hello["canonical_profiles"][0]["roles"][0]["conditions"]["cupric"]
        )
        assert condition[-5:] == (600, 300, 400, 500, 700)

    def test_worker_requires_the_profiled_doctrine_capability(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        hello = contract_hello()
        hello.pop("profiled_doctrine")

        def fake_popen(*_args: object, **_kwargs: object) -> _FakeProcess:
            return _FakeProcess(hello)

        monkeypatch.setattr(oxide_gym.subprocess, "Popen", fake_popen)
        with pytest.raises(RuntimeError, match="profiled-doctrine mismatch"):
            oxide_gym.Worker("fake-driver")

    def test_named_reset_passes_rust_authored_facets_in_control_order(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        hello = contract_hello()
        proc = _FakeProcess(
            hello,
            [
                {
                    "done": False,
                    "tick": 0,
                    "seats": [],
                    "factions": ["ferrous", "cupric"],
                    "effects": [],
                }
            ],
        )

        def fake_popen(*_args: object, **_kwargs: object) -> _FakeProcess:
            return proc

        monkeypatch.setattr(oxide_gym.subprocess, "Popen", fake_popen)
        worker = oxide_gym.Worker("fake-driver")
        named = worker.named_condition("balanced", 0, "generalist", "ferrous")
        raw = oxide_gym.condition_from_profile(1000, 500, 1000)
        worker.reset(seed=3, control=(1, 0), conditions={0: named, 1: raw})

        request = json.loads(proc.stdin.getvalue().splitlines()[-1])
        assert request["control"] == [1, 0]
        assert request["profile_facets"] == [
            [0, 0, 0, 0, 0],
            [600, 300, 400, 500, 700],
        ]

    def test_raw_condition_sends_the_exact_zero_facet_sentinel(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        hello = contract_hello()
        proc = _FakeProcess(
            hello,
            [
                {
                    "done": False,
                    "tick": 0,
                    "seats": [],
                    "factions": ["ferrous", "cupric"],
                    "effects": [],
                }
            ],
        )

        def fake_popen(*_args: object, **_kwargs: object) -> _FakeProcess:
            return proc

        monkeypatch.setattr(oxide_gym.subprocess, "Popen", fake_popen)
        worker = oxide_gym.Worker("fake-driver")
        worker.reset(
            seed=4,
            conditions={0: oxide_gym.condition_from_profile(620, 300, 0)},
        )

        request = json.loads(proc.stdin.getvalue().splitlines()[-1])
        assert request["profile_facets"] == [[0, 0, 0, 0, 0]]

    def test_profile_facet_values_are_validated_before_reset(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        hello = contract_hello()

        def fake_popen(*_args: object, **_kwargs: object) -> _FakeProcess:
            return _FakeProcess(hello)

        monkeypatch.setattr(oxide_gym.subprocess, "Popen", fake_popen)
        worker = oxide_gym.Worker("fake-driver")
        invalid = list(oxide_gym.condition_from_profile(1000, 500, 0))
        invalid[-1] = 1001
        with pytest.raises(ValueError, match="profile facets"):
            worker.reset(seed=5, conditions={0: tuple(invalid)})

    @pytest.mark.parametrize(
        "conditions",
        [
            {0: oxide_gym.condition_from_profile(1000, 500, 0)},
            {
                0: oxide_gym.condition_from_profile(1000, 500, 0),
                1: oxide_gym.condition_from_profile(1000, 500, 1000),
                2: oxide_gym.condition_from_profile(1000, 500, 0),
            },
        ],
    )
    def test_profiled_reset_requires_one_condition_per_controlled_seat(
        self,
        monkeypatch: pytest.MonkeyPatch,
        conditions: dict[int, tuple[int, ...]],
    ) -> None:
        hello = contract_hello()
        proc = _FakeProcess(hello)

        def fake_popen(*_args: object, **_kwargs: object) -> _FakeProcess:
            return proc

        monkeypatch.setattr(oxide_gym.subprocess, "Popen", fake_popen)
        worker = oxide_gym.Worker("fake-driver")

        with pytest.raises(ValueError, match="exactly the controlled seats"):
            worker.reset(seed=6, control=(0, 1), conditions=conditions)
        assert proc.stdin.getvalue() == ""
        assert worker.conditions == {}

    @pytest.mark.parametrize(
        ("mutate", "message"),
        [
            (
                lambda hello: hello["condition_names"].reverse(),
                "condition-name mismatch",
            ),
            (
                lambda hello: hello.pop("canonical_profiles"),
                "lacks canonical named profiles",
            ),
            (
                lambda hello: hello["canonical_profiles"][0]["roles"][0]["conditions"][
                    "ferrous"
                ].pop(),
                "invalid canonical condition",
            ),
            (
                lambda hello: hello["canonical_profiles"][0]["roles"][0][
                    "conditions"
                ].pop("cupric"),
                "must publish both factions",
            ),
        ],
    )
    def test_missing_malformed_or_reordered_condition_metadata_is_rejected(
        self,
        monkeypatch: pytest.MonkeyPatch,
        mutate: Callable[[dict], object],
        message: str,
    ) -> None:
        hello = contract_hello()
        mutate(hello)

        def fake_popen(*_args: object, **_kwargs: object) -> _FakeProcess:
            return _FakeProcess(hello)

        monkeypatch.setattr(oxide_gym.subprocess, "Popen", fake_popen)
        with pytest.raises(RuntimeError, match=message):
            oxide_gym.Worker("fake-driver")


class TestTerminalSemantics:
    def test_a_living_tick_cap_is_neutral_but_elimination_is_a_loss(self) -> None:
        capped = oxide_gym.Frame(
            True,
            40_000,
            truncated=True,
            alive=[0],
        )
        assert capped.reward(0) == 0.0
        assert capped.reward(1) == -1.0

        real_draw = oxide_gym.Frame(True, 12_000, alive=[0, 1])
        assert real_draw.reward(0) == oxide_gym.DRAW_REWARD

    def test_worker_parses_the_terminal_truncation_marker(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        hello = contract_hello()
        proc = _FakeProcess(
            hello,
            [
                {
                    "done": True,
                    "truncated": True,
                    "tick": 40_000,
                    "winner": None,
                    "winners": [],
                    "alive": [0],
                    "seats": [],
                }
            ],
        )

        def fake_popen(*_args: object, **_kwargs: object) -> _FakeProcess:
            return proc

        monkeypatch.setattr(oxide_gym.subprocess, "Popen", fake_popen)
        worker = oxide_gym.Worker("fake-driver")

        frame = worker.recv()
        assert frame.done
        assert frame.truncated
        assert frame.reward(0) == 0.0


class TestEffectTelemetry:
    def test_successful_effects_are_typed(self) -> None:
        effects = oxide_gym.parse_effects(
            {
                "effects": [
                    {
                        "seat": 1,
                        "repair_unit_commands": 2,
                        "unit_hp_restored": 13,
                        "repair_unit_hp_restored": 8,
                        "buildings_salvaged": 3,
                        "buildings_completed": ["turret", "repair_bay"],
                    }
                ]
            }
        )
        assert effects[1] == oxide_gym.SeatEffects(
            repair_unit_commands=2,
            unit_hp_restored=13,
            repair_unit_hp_restored=8,
            buildings_salvaged=3,
            buildings_completed=("turret", "repair_bay"),
        )

    def test_unknown_or_duplicate_effect_rows_fail_loudly(self) -> None:
        with pytest.raises(RuntimeError, match="unknown completed"):
            oxide_gym.parse_effects(
                {"effects": [{"seat": 0, "buildings_completed": ["moon_base"]}]}
            )
        with pytest.raises(RuntimeError, match="duplicate"):
            oxide_gym.parse_effects({"effects": [{"seat": 0}, {"seat": 0}]})


def faction_features(name: oxide_gym.FactionName) -> list[int]:
    features = [0] * oxide_gym.FEATURES
    features[oxide_gym.FACTION_FEATURE] = int(name == "cupric")
    return features


class TestFactionContract:
    def test_compact_codes_cover_every_two_seat_pair(self) -> None:
        assert oxide_gym.normalize_factions("FF") == ["ferrous", "ferrous"]
        assert oxide_gym.normalize_factions("FC") == ["ferrous", "cupric"]
        assert oxide_gym.normalize_factions("CF") == ["cupric", "ferrous"]
        assert oxide_gym.normalize_factions("CC") == ["cupric", "cupric"]

    def test_full_names_are_validated(self) -> None:
        assert oxide_gym.normalize_factions(("cupric", "ferrous")) == [
            "cupric",
            "ferrous",
        ]
        with pytest.raises(ValueError, match="expected only f or c"):
            oxide_gym.normalize_factions("cx")

    def test_condition_faction_is_derived_from_rust_not_the_caller(self) -> None:
        ferrous = faction_features("ferrous")
        cupric = faction_features("cupric")
        pressure = oxide_gym.condition_from_profile(800, 900, 1000)
        assert oxide_gym.honest_condition(pressure, ferrous) == (
            800,
            900,
            0,
            0,
            0,
            0,
            1000,
            0,
            0,
            0,
            0,
            0,
        )
        fortify = oxide_gym.condition_from_profile(800, 249, 0)
        assert oxide_gym.honest_condition(fortify, cupric) == (
            800,
            249,
            1000,
            1000,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        )

    def test_strategy_one_hot_uses_exact_quartile_boundaries(self) -> None:
        assert oxide_gym.strategy_one_hot(0) == (1000, 0, 0, 0)
        assert oxide_gym.strategy_one_hot(249) == (1000, 0, 0, 0)
        assert oxide_gym.strategy_one_hot(250) == (0, 1000, 0, 0)
        assert oxide_gym.strategy_one_hot(499) == (0, 1000, 0, 0)
        assert oxide_gym.strategy_one_hot(500) == (0, 0, 1000, 0)
        assert oxide_gym.strategy_one_hot(749) == (0, 0, 1000, 0)
        assert oxide_gym.strategy_one_hot(750) == (0, 0, 0, 1000)
        assert oxide_gym.strategy_one_hot(1000) == (0, 0, 0, 1000)
        with pytest.raises(ValueError, match="aggression"):
            oxide_gym.strategy_one_hot(1001)

    @pytest.mark.parametrize(
        ("aggression", "skill"),
        [
            (0, 1000),
            (249, 1000),
            (250, 620),
            (499, 620),
            (500, 1000),
            (1000, 1000),
        ],
    )
    def test_policy_skill_matches_the_shipped_strategy_profiles(
        self,
        aggression: int,
        skill: int,
    ) -> None:
        assert oxide_gym.policy_skill_for_aggression(aggression) == skill

    @pytest.mark.parametrize("aggression", [-1, 1001])
    def test_policy_skill_rejects_out_of_range_aggression(
        self,
        aggression: int,
    ) -> None:
        with pytest.raises(ValueError, match=r"0\.\.1000"):
            oxide_gym.policy_skill_for_aggression(aggression)

    def test_a_missing_or_mismatched_rust_roster_is_rejected(self) -> None:
        requested = oxide_gym.normalize_factions("ff")
        with pytest.raises(RuntimeError, match="Rust reported None"):
            oxide_gym.validate_reported_factions(None, requested)
        with pytest.raises(RuntimeError, match=r"Rust reported.*cupric"):
            oxide_gym.validate_reported_factions(
                oxide_gym.normalize_factions("fc"),
                requested,
            )

    def test_seat_view_and_appended_condition_cannot_disagree(self) -> None:
        raw = faction_features("cupric")
        condition = oxide_gym.honest_condition(
            oxide_gym.condition_from_profile(1000, 500, 0), raw
        )
        obs = oxide_gym.with_condition(oxide_gym.normalize(raw), condition)
        view = oxide_gym.SeatView(
            obs,
            np.ones(oxide_gym.ACTIONS, dtype=bool),
            raw,
        )
        assert view.faction == "cupric"
        assert view.faction_knob == 1000
        assert view.obs[oxide_gym.FEATURES + 2] == view.faction_knob / 1000
        np.testing.assert_array_equal(
            view.obs[oxide_gym.FEATURES + 3 : oxide_gym.FEATURES + 7],
            [0, 0, 1, 0],
        )
        np.testing.assert_array_equal(view.obs[-5:], [0, 0, 0, 0, 0])

    def test_an_invalid_rust_faction_feature_fails_loudly(self) -> None:
        raw = faction_features("ferrous")
        raw[oxide_gym.FACTION_FEATURE] = 2
        with pytest.raises(ValueError, match="must be 0 or 1"):
            oxide_gym.faction_name_from_features(raw)
