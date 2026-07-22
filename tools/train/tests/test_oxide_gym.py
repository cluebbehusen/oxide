"""Tests for the ``oxide_gym`` feature contract: the gym hello carries
FEATURE_NAMES and the Worker asserts this list against it, so the name
table, the scale table, and the network input width must all agree."""

import oxide_gym


class TestFeatureContract:
    def test_names_cover_every_feature(self) -> None:
        assert len(oxide_gym.FEATURE_NAMES) == oxide_gym.FEATURES

    def test_scales_cover_every_feature(self) -> None:
        assert oxide_gym.SCALES.shape == (oxide_gym.FEATURES,)

    def test_net_input_is_features_plus_conditioning(self) -> None:
        assert oxide_gym.NET_FEATURES == oxide_gym.FEATURES + oxide_gym.CONDITION_DIMS
        assert oxide_gym.CONDITION_DIMS == 3
