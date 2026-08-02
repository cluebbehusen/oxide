import hashlib
import tempfile
import unittest
from itertools import pairwise
from pathlib import Path

from PIL import Image, ImageChops

from tools import gen_sprites as gen
from tools.production_sprite_sources import finalized


def _changed_pixels(left: Image.Image, right: Image.Image) -> int:
    difference = ImageChops.difference(left.convert("RGBA"), right.convert("RGBA"))
    return sum(pixel != (0, 0, 0, 0) for pixel in difference.get_flattened_data())


def _alpha_centroid_y(image: Image.Image, box: tuple[int, int, int, int]) -> float:
    alpha = image.getchannel("A")
    x0, y0, x1, y1 = box
    total = 0
    weighted = 0
    for y in range(y0, y1):
        for x in range(x0, x1):
            value = alpha.getpixel((x, y))
            total += value
            weighted += y * value
    if total == 0:
        raise AssertionError("centroid region is empty")
    return weighted / total


class ProductionSpriteSourceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.temp = tempfile.TemporaryDirectory(prefix="oxide-production-sprites-")
        cls.out = Path(cls.temp.name)
        cls.registry: dict[str, Image.Image] = {}
        old_out = gen.OUT
        old_registry = gen.REGISTRY
        try:
            gen.OUT = cls.out
            gen.REGISTRY = cls.registry
            for faction in gen.FACTIONS:
                gen.harvester(faction)
                gen.harvester(faction, dig=1)
                gen.harvester(faction, dig=2)
            finalized.install_finalized_sprites(cls.registry, cls.out)
        finally:
            gen.OUT = old_out
            gen.REGISTRY = old_registry

    @classmethod
    def tearDownClass(cls) -> None:
        cls.temp.cleanup()

    def test_installer_is_independent_of_review_files(self) -> None:
        root = Path(finalized.__file__).parent
        production_sources = "\n".join(
            path.read_text() for path in sorted(root.glob("*.py"))
        )
        self.assertNotIn("art-direction-review", production_sources)
        self.assertNotIn("from tools.batch", production_sources)
        self.assertNotIn("from tools import batch", production_sources)
        self.assertNotIn("gen._review", production_sources)
        self.assertNotIn("gen.REVIEW_ROUTE", production_sources)

    def test_metadata_counts_match_every_generated_action_row(self) -> None:
        for stem, frame_set in finalized.UNIT_ACTIONS.items():
            self.assertEqual(finalized.ACTION_COUNTS[stem], len(frame_set.suffixes))
            self.assertEqual(len(frame_set.suffixes), len(frame_set.events))
            self.assertEqual(len(frame_set.events), len(frame_set.durations_ms))
            for faction in gen.FACTIONS:
                for suffix in frame_set.suffixes:
                    self.assertIn(f"{stem}_{faction}{suffix}", self.registry)

        for stem, frame_set in finalized.BUILDING_WORK.items():
            self.assertEqual(finalized.ACTION_COUNTS[stem], len(frame_set.suffixes))
            self.assertEqual(len(frame_set.suffixes), len(frame_set.events))
            self.assertEqual(len(frame_set.events), len(frame_set.durations_ms))
            for faction in gen.FACTIONS:
                for suffix in frame_set.suffixes:
                    self.assertIn(f"{stem}_{faction}{suffix}", self.registry)

        for stem, frame_set in finalized.DEFENSE_ACTIONS.items():
            self.assertEqual(finalized.ACTION_COUNTS[stem], len(frame_set.suffixes))
            self.assertEqual(len(frame_set.suffixes), len(frame_set.events))
            self.assertEqual(len(frame_set.events), len(frame_set.durations_ms))
            for faction in gen.FACTIONS:
                for suffix in frame_set.suffixes:
                    self.assertIn(f"{stem}_{faction}{suffix}", self.registry)

        for stem, frame_set in finalized.DEFENSE_BASE_ACTIONS.items():
            self.assertEqual(finalized.ACTION_COUNTS[stem], len(frame_set.suffixes))
            for faction in gen.FACTIONS:
                for suffix in frame_set.suffixes:
                    self.assertIn(f"{stem}_{faction}{suffix}", self.registry)

        for frame_sets in (finalized.UNIT_ACTIONS, finalized.DEFENSE_ACTIONS):
            for stem, frame_set in frame_sets.items():
                with self.subTest(stem=stem, contract="damage-event"):
                    self.assertEqual(
                        sum("damage" in event for event in frame_set.events), 1
                    )

    def test_movement_metadata_matches_every_generated_row(self) -> None:
        self.assertEqual(set(finalized.UNIT_MOVEMENT), set(finalized.UNIT_ACTIONS))
        for stem, frame_set in finalized.UNIT_MOVEMENT.items():
            self.assertEqual(len(frame_set.suffixes), len(frame_set.events))
            self.assertEqual(len(frame_set.events), len(frame_set.durations_ms))
            for faction in gen.FACTIONS:
                base = self.registry[f"{stem}_{faction}"]
                for suffix in frame_set.suffixes:
                    frame = self.registry[f"{stem}_{faction}{suffix}"]
                    self.assertEqual(frame.size, base.size)
                    self.assertGreater(_changed_pixels(base, frame), 2)

    def test_unit_metadata_matches_source_sequences(self) -> None:
        for stem, builder in finalized._unit_sequences().items():
            sequence = builder()
            movement = finalized.UNIT_MOVEMENT[stem]
            actions = finalized.UNIT_ACTIONS[stem]
            self.assertEqual(
                tuple(frame.event for frame in sequence.frames[1:3]),
                movement.events,
            )
            self.assertEqual(
                tuple(frame.duration_ms for frame in sequence.frames[1:3]),
                movement.durations_ms,
            )
            self.assertEqual(
                tuple(frame.event for frame in sequence.frames[4:]),
                actions.events,
            )
            self.assertEqual(
                tuple(frame.duration_ms for frame in sequence.frames[4:]),
                actions.durations_ms,
            )

    def test_factions_share_dimensions_but_not_accent_pixels(self) -> None:
        stems = (
            "harvester",
            "sentinel",
            "scuttler",
            "lancer",
            "bombard",
            "flakhound",
            "stinger",
            "buzzard",
            "darter",
            "talon",
            "wisp",
            "foundry",
            "turret",
            "fabricator",
            "flak_turret",
            "bastion",
            "array",
            "reclaimer",
            "repair_bay",
        )
        for stem in stems:
            ferrous = self.registry[f"{stem}_ferrous"]
            cupric = self.registry[f"{stem}_cupric"]
            with self.subTest(stem=stem):
                self.assertEqual(ferrous.size, cupric.size)
                self.assertEqual(
                    ferrous.getchannel("A").tobytes(),
                    cupric.getchannel("A").tobytes(),
                )
                self.assertGreater(_changed_pixels(ferrous, cupric), 8)

    def test_harvester_preserves_every_cargo_motion_and_bite_combination(self) -> None:
        for faction in gen.FACTIONS:
            cargo_images = []
            for cargo in range(finalized.HARVESTER_CARGO_LEVELS):
                prefix = f"harvester_{faction}_cargo{cargo}"
                cargo_images.append(self.registry[prefix])
                for suffix in ("_tread1", "_tread2", "_scoop1", "_scoop2"):
                    self.assertIn(prefix + suffix, self.registry)
                self.assertEqual(
                    self.registry[prefix].crop((8, 0, 56, 14)).tobytes(),
                    self.registry[prefix + "_tread1"].crop((8, 0, 56, 14)).tobytes(),
                    "movement must retain the approved claw",
                )
                self.assertGreater(
                    _changed_pixels(
                        self.registry[prefix], self.registry[prefix + "_scoop2"]
                    ),
                    30,
                )
            for lower, higher in pairwise(cargo_images):
                self.assertGreater(
                    _changed_pixels(
                        lower.crop((23, 44, 41, 55)),
                        higher.crop((23, 44, 41, 55)),
                    ),
                    2,
                )

    def test_defense_foundations_and_mounts_are_separate_square_layers(self) -> None:
        pairs = (
            ("turret", "turret_barrel", 64),
            ("flak_turret", "flak_mount", 64),
            ("bastion", "bastion_mount", 128),
        )
        for faction in gen.FACTIONS:
            for base_stem, mount_stem, side in pairs:
                base = self.registry[f"{base_stem}_{faction}"]
                mount = self.registry[f"{mount_stem}_{faction}"]
                with self.subTest(faction=faction, mount=mount_stem):
                    self.assertEqual(base.size, (side, side))
                    self.assertEqual(mount.size, (side, side))
                    self.assertIsNotNone(base.getchannel("A").getbbox())
                    self.assertIsNotNone(mount.getchannel("A").getbbox())
                    self.assertGreater(_changed_pixels(base, mount), side)
                for suffix in finalized.DEFENSE_ACTIONS[mount_stem].suffixes:
                    action = self.registry[f"{mount_stem}_{faction}{suffix}"]
                    self.assertEqual(action.size, (side, side))
                    if mount_stem == "bastion_mount":
                        self.assertLess(
                            action.getchannel("A").getbbox()[3],
                            side,
                            f"{mount_stem}{suffix} must not clip at the canvas edge",
                        )

    def test_action_rows_contain_real_frame_changes(self) -> None:
        for faction in gen.FACTIONS:
            for stem, frame_set in finalized.UNIT_ACTIONS.items():
                base = self.registry[f"{stem}_{faction}"]
                changed = [
                    _changed_pixels(base, self.registry[f"{stem}_{faction}{suffix}"])
                    for suffix in frame_set.suffixes
                ]
                with self.subTest(faction=faction, stem=stem):
                    self.assertGreater(max(changed), 12)
            for stem, frame_set in finalized.BUILDING_WORK.items():
                base = self.registry[f"{stem}_{faction}"]
                changed = [
                    _changed_pixels(base, self.registry[f"{stem}_{faction}{suffix}"])
                    for suffix in frame_set.suffixes
                ]
                with self.subTest(faction=faction, stem=stem):
                    self.assertGreater(max(changed), 12)

    def test_reclaimer_is_the_exact_approved_open_works_sequence(self) -> None:
        expected = (
            "d5e1716c973f640419d30bc2291a5c5d4ef4d1e4c58cb29d8f1d408c7d151d81",
            "0c6c3d5294f510e63be162e9efbcee66166a7984587b0e6c4311805afd256504",
            "cbec01349a5b78465be3341c51ac472306df19df3738958da29b4df2589a65ce",
            "6a216a8cf3049dc76848be6e0b18436efe1460b40b0cb46c46ad695dc3a85790",
        )
        actual = tuple(
            hashlib.sha256(
                self.registry[f"reclaimer_ferrous{suffix}"].convert("RGBA").tobytes()
            ).hexdigest()
            for suffix in ("", "_work1", "_work2", "_work3")
        )
        self.assertEqual(actual, expected)

    def test_foundry_work_frames_only_pulse_the_centered_eye(self) -> None:
        frames = [
            self.registry[f"foundry_ferrous{suffix}"].convert("RGBA")
            for suffix in ("", "_work1", "_work2", "_work3", "_work4")
        ]
        outside_eye = Image.new("L", frames[0].size, 255)
        outside_eye.paste(0, (36, 44, 93, 101))
        for frame in frames[1:]:
            difference = ImageChops.difference(frames[0], frame)
            outside_difference = Image.new("RGBA", frames[0].size)
            outside_difference.paste(difference, mask=outside_eye)
            self.assertIsNone(
                outside_difference.getbbox(),
                "the Foundry gantry must remain fixed while its eye pulses",
            )

        center_values = [sum(frame.getpixel((64, 72))[:3]) for frame in frames]
        self.assertLess(center_values[0], center_values[1])
        self.assertLess(center_values[1], center_values[2])
        self.assertEqual(center_values[1], center_values[3])
        self.assertEqual(frames[0].tobytes(), frames[4].tobytes())

    def test_bastion_ready_and_reload_frames_have_physical_charge_cells(self) -> None:
        centers = [(19, 94 - index * 9) for index in range(5)]
        expected = {
            "": 5,
            "_action1": 1,
            "_action2": 2,
            "_action3": 3,
            "_action4": 4,
            "_action5": 5,
            "_action6": 5,
            "_action7": 0,
            "_action8": 0,
            "_action9": 0,
        }
        for faction in gen.FACTIONS:
            for suffix, count in expected.items():
                image = self.registry[f"bastion_{faction}{suffix}"]
                lit = sum(
                    image.getpixel(center)[:3] == gen.SCRAP_LIGHT for center in centers
                )
                with self.subTest(faction=faction, suffix=suffix):
                    self.assertEqual(lit, count)

    def test_bastion_recoils_on_report_then_returns_quickly(self) -> None:
        for faction in gen.FACTIONS:
            centers = [
                _alpha_centroid_y(
                    self.registry[f"bastion_mount_{faction}{suffix}"],
                    (31, 0, 98, 96),
                )
                for suffix in (
                    "_action5",
                    "_action6",
                    "_action7",
                    "_action8",
                    "_action9",
                )
            ]
            with self.subTest(faction=faction):
                self.assertGreater(centers[1] - centers[0], 4.0)
                self.assertGreater(centers[1], centers[2])
                self.assertGreater(centers[2], centers[3])
                self.assertGreater(centers[3], centers[4])

    def test_flakhound_ready_and_reload_frames_have_physical_charge_cells(self) -> None:
        centers = [(24 + index * 6, 53) for index in range(4)]
        expected = {
            "": 4,
            "_action1": 0,
            "_action2": 1,
            "_action3": 2,
            "_action4": 3,
            "_action5": 4,
            "_action6": 4,
            "_action7": 2,
            "_action8": 0,
            "_action9": 0,
        }
        for faction in gen.FACTIONS:
            for suffix, count in expected.items():
                image = self.registry[f"flakhound_{faction}{suffix}"]
                lit = sum(image.getpixel(center)[0] > 200 for center in centers)
                with self.subTest(faction=faction, suffix=suffix):
                    self.assertEqual(lit, count)


if __name__ == "__main__":
    unittest.main()
