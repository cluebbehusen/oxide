"""Tests for the ``oxide_gym`` feature contract: the gym hello carries
FEATURE_NAMES and the Worker asserts this list against it, so the name
table, the scale table, and the network input width must all agree."""

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
        assert oxide_gym.CONDITION_DIMS == 3


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
        assert oxide_gym.honest_condition((800, 250, 1000), ferrous) == (
            800,
            250,
            0,
        )
        assert oxide_gym.honest_condition((800, 250, 0), cupric) == (
            800,
            250,
            1000,
        )

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
        condition = oxide_gym.honest_condition((1000, 500, 0), raw)
        obs = oxide_gym.with_condition(oxide_gym.normalize(raw), condition)
        view = oxide_gym.SeatView(
            obs,
            np.ones(oxide_gym.ACTIONS, dtype=bool),
            raw,
        )
        assert view.faction == "cupric"
        assert view.faction_knob == 1000
        assert view.obs[-1] == view.faction_knob / 1000

    def test_an_invalid_rust_faction_feature_fails_loudly(self) -> None:
        raw = faction_features("ferrous")
        raw[oxide_gym.FACTION_FEATURE] = 2
        with pytest.raises(ValueError, match="must be 0 or 1"):
            oxide_gym.faction_name_from_features(raw)
