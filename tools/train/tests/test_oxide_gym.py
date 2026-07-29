"""Tests for the ``oxide_gym`` feature contract: the gym hello carries
FEATURE_NAMES and the Worker asserts this list against it, so the name
table, the scale table, and the network input width must all agree."""

import io
import json

import numpy as np
import pytest

import oxide_gym


class TestFeatureContract:
    def test_names_cover_every_feature(self) -> None:
        assert len(oxide_gym.FEATURE_NAMES) == oxide_gym.FEATURES

    def test_scales_cover_every_feature(self) -> None:
        assert oxide_gym.SCALES.shape == (oxide_gym.FEATURES,)

    def test_net_input_is_features_plus_conditioning(self) -> None:
        assert oxide_gym.NET_FEATURES == oxide_gym.FEATURES + oxide_gym.CONDITION_DIMS
        assert oxide_gym.CONDITION_DIMS == 7

    def test_v7_appends_the_exact_context_features(self) -> None:
        expected = {
            "known_salvage_value": 2_000,
            "near_home_salvage_value": 1_000,
            "nearest_salvage_distance": 200,
            "idle_harvesters": 8,
            "carried_scrap": 200,
            "queued_unit_value": 1_000,
            "construction_site_value": 1_000,
            "my_unit_health_value": 2_000,
            "my_building_health_value": 1_000,
            "my_bastions_built": 2,
            "my_repair_bays_built": 1,
            "my_construction_sites": 4,
            "home_enemy_pressure": 500,
            "nearest_enemy_distance": 200,
            "construction_plan": 7,
            "construction_reserve": 250,
        }
        assert oxide_gym.FEATURES == 81
        assert oxide_gym.FEATURE_NAMES[-len(expected) :] == list(expected)
        assert {name: oxide_gym.SCALE_BY_NAME[name] for name in expected} == expected


class _FakeProcess:
    def __init__(self, hello: object, replies: list[object] | None = None) -> None:
        self.stdin = io.StringIO()
        messages = [hello, *(replies or [])]
        self.stdout = io.StringIO(
            "".join(f"{json.dumps(message)}\n" for message in messages)
        )

    def terminate(self) -> None:
        pass


class TestFactorizedContract:
    def test_the_heads_are_global_indices_and_preserve_old_rows(self) -> None:
        assert oxide_gym.GYM_VERSION == 7
        assert oxide_gym.ACTIONS == 26
        assert oxide_gym.ACTION_HEADS == (
            (0, 1, 2, 3, 4, 5, 6, 7, 8),
            (24, 9, 10, 11, 12, 13, 14, 15, 21, 22, 23),
            (25, 16, 17, 18, 19, 20),
        )

    def test_action_plans_validate_global_head_membership(self) -> None:
        assert oxide_gym.validate_action_plan([8, 24, 20]) == (8, 24, 20)
        with pytest.raises(ValueError, match="head 1"):
            oxide_gym.validate_action_plan((8, 8, 20))
        with pytest.raises(ValueError, match="must contain 3"):
            oxide_gym.validate_action_plan((8, 24))

    def test_worker_requires_exact_heads_and_writes_nested_plans(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        hello = {
            "ready": True,
            "version": oxide_gym.GYM_VERSION,
            "features": oxide_gym.FEATURES,
            "actions": oxide_gym.ACTIONS,
            "action_heads": [list(head) for head in oxide_gym.ACTION_HEADS],
            "names": oxide_gym.FEATURE_NAMES,
        }
        proc = _FakeProcess(hello)

        def fake_popen(*_args: object, **_kwargs: object) -> _FakeProcess:
            return proc

        monkeypatch.setattr(oxide_gym.subprocess, "Popen", fake_popen)
        worker = oxide_gym.Worker("fake-driver")
        worker.control = (1, 0)
        worker.send_step({0: (0, 24, 25), 1: (8, 23, 20)})

        request = json.loads(proc.stdin.getvalue().splitlines()[-1])
        assert request == {
            "cmd": "step",
            "actions": [[8, 23, 20], [0, 24, 25]],
        }

    def test_worker_rejects_a_different_action_partition(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        hello = {
            "ready": True,
            "version": oxide_gym.GYM_VERSION,
            "features": oxide_gym.FEATURES,
            "actions": oxide_gym.ACTIONS,
            "action_heads": [list(range(9)), [24, 9], [25, 16]],
            "names": oxide_gym.FEATURE_NAMES,
        }

        def fake_popen(*_args: object, **_kwargs: object) -> _FakeProcess:
            return _FakeProcess(hello)

        monkeypatch.setattr(oxide_gym.subprocess, "Popen", fake_popen)
        with pytest.raises(RuntimeError, match="action-head mismatch"):
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
        hello = {
            "ready": True,
            "version": oxide_gym.GYM_VERSION,
            "features": oxide_gym.FEATURES,
            "actions": oxide_gym.ACTIONS,
            "action_heads": [list(head) for head in oxide_gym.ACTION_HEADS],
            "names": oxide_gym.FEATURE_NAMES,
        }
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
        np.testing.assert_array_equal(view.obs[-4:], [0, 0, 1, 0])

    def test_an_invalid_rust_faction_feature_fails_loudly(self) -> None:
        raw = faction_features("ferrous")
        raw[oxide_gym.FACTION_FEATURE] = 2
        with pytest.raises(ValueError, match="must be 0 or 1"):
            oxide_gym.faction_name_from_features(raw)
