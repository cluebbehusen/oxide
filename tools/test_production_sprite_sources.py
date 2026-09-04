import hashlib
import tempfile
import unittest
from itertools import pairwise
from pathlib import Path

from PIL import Image, ImageChops

from tools import gen_sprites as gen
from tools.production_sprite_sources import (
    airworks_scouts_final,
    construction_final,
    core_unit_art_final,
    crucible_final,
    environment_final,
    excavator_final,
    extractor_reclaimer_final,
    finalized,
    flak_array_final,
    heavy_structures,
    moth_warden_final,
    shrike_sylph_final,
    skyhook_sapper_crucible_final,
    tender_condor_final,
    turret_family,
)


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
                gen.barricade(faction)
                gen.scuttle_charge(faction)
            finalized.install_finalized_sprites(cls.registry, cls.out)
            construction_final.install_finalized_construction(cls.registry, cls.out)
            environment_final.install_finalized_environment(cls.registry, cls.out)
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

    def test_construction_bank_covers_every_building(self) -> None:
        self.assertEqual(
            construction_final.BUILDING_STEMS,
            (
                "foundry",
                "turret",
                "fabricator",
                "flak_turret",
                "bastion",
                "array",
                "reclaimer",
                "repair_bay",
                "extractor",
                "airworks",
                "crucible",
                "barricade",
                "scuttle_charge",
            ),
        )

    def test_finalized_environment_bank_is_complete_and_pixel_stable(self) -> None:
        self.assertEqual(len(environment_final.FIELD_DEBRIS_KEYS), 10)
        self.assertEqual(len(environment_final.GROUND_BLOCKER_KEYS), 9)
        self.assertEqual(len(environment_final.ROCK_KEYS), 23)
        self.assertEqual(len(environment_final.ROCK_FOOTPRINTS), 23)

        digest = hashlib.sha256()
        for key in environment_final.FIELD_DEBRIS_KEYS:
            image = self.registry[key]
            self.assertIn(image.size, ((32, 32), (64, 64)))
            digest.update(key.encode())
            digest.update(image.tobytes())
        for key, footprint in zip(
            environment_final.GROUND_BLOCKER_KEYS,
            environment_final.GROUND_BLOCKER_FOOTPRINTS,
            strict=True,
        ):
            image = self.registry[key]
            self.assertEqual(image.size, tuple(side * 64 for side in footprint))
            digest.update(key.encode())
            digest.update(image.tobytes())
        for key, footprint in zip(
            environment_final.ROCK_KEYS[4:],
            environment_final.ROCK_FOOTPRINTS[4:],
            strict=True,
        ):
            image = self.registry[key]
            self.assertEqual(image.size, tuple(side * 64 for side in footprint))
            digest.update(key.encode())
            digest.update(image.tobytes())
        self.assertEqual(
            digest.hexdigest(),
            "a80200d8332879e8fc53a81c5469861d1512a03b13de9835925276c3ad0920ee",
        )

    def test_peak_bank_covers_every_fog_honest_connectivity_mask(self) -> None:
        images = []
        for mask in range(16):
            for variant in range(2):
                key = f"peak_barrier_{mask:02x}_{variant}"
                image = self.registry[key]
                self.assertEqual(image.size, (64, 64))
                self.assertEqual(image.getchannel("A").getbbox(), (0, 0, 64, 64))
                images.append(image.tobytes())
        self.assertEqual(len(set(images)), 32)

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

    def test_promoted_tender_and_condor_match_the_production_rgba_source(self) -> None:
        digest = hashlib.sha256()
        for faction in ("ferrous", "cupric"):
            for state in tender_condor_final.TENDER_STATES:
                key = f"tender/{faction}/{state}"
                digest.update(key.encode())
                digest.update(
                    tender_condor_final.render_tender(faction, state).tobytes()
                )
            for phase in (1, 2):
                key = f"tender/{faction}/move{phase}"
                digest.update(key.encode())
                digest.update(
                    tender_condor_final.render_tender(
                        faction, move_phase=phase
                    ).tobytes()
                )
        for state in tender_condor_final.CONDOR_STATES:
            key = f"condor/ferrous/{state}"
            digest.update(key.encode())
            digest.update(tender_condor_final.render_condor("ferrous", state).tobytes())
        self.assertEqual(
            digest.hexdigest(), tender_condor_final.PRODUCTION_SOURCE_RGBA_SHA256
        )

        tender_states = ("idle", "deploy", "contact", "weld", "recover")
        tender_suffixes = ("", "_action1", "_action2", "_action3", "_action4")
        condor_states = ("idle", "crack", "open", "release", "recover")
        condor_suffixes = ("", "_action1", "_action2", "_action3", "_action4")
        for faction in ("ferrous", "cupric"):
            for state, suffix in zip(tender_states, tender_suffixes, strict=True):
                self.assertEqual(
                    self.registry[f"tender_{faction}{suffix}"].tobytes(),
                    tender_condor_final.render_tender(faction, state).tobytes(),
                )
            for state, suffix in zip(condor_states, condor_suffixes, strict=True):
                self.assertEqual(
                    self.registry[f"condor_{faction}{suffix}"].tobytes(),
                    tender_condor_final.render_condor(faction, state).tobytes(),
                )

    def test_promoted_airworks_scouts_match_the_approved_rgba_source(self) -> None:
        self.assertEqual(
            airworks_scouts_final.source_rgba_digest(),
            airworks_scouts_final.APPROVED_SOURCE_RGBA_SHA256,
        )
        unit_states = (("", 1), ("_move1", 0), ("_move2", 2))
        for faction in ("ferrous", "cupric"):
            for suffix, phase in unit_states:
                for stem, renderer in (
                    ("gnat", airworks_scouts_final.render_gnat),
                    ("kestrel", airworks_scouts_final.render_kestrel),
                ):
                    image = renderer(faction, phase)
                    self.assertEqual(image.size, (64, 64))
                    self.assertEqual(
                        self.registry[f"{stem}_{faction}{suffix}"].tobytes(),
                        image.tobytes(),
                    )
            for stage in range(5):
                suffix = "" if stage == 0 else f"_work{stage}"
                image = airworks_scouts_final.render_airworks(faction, stage)
                self.assertEqual(image.size, (128, 128))
                self.assertEqual(
                    self.registry[f"airworks_{faction}{suffix}"].tobytes(),
                    image.tobytes(),
                )

    def test_promoted_core_unit_art_matches_the_approved_rgba_source(self) -> None:
        self.assertEqual(
            core_unit_art_final.source_rgba_digest(),
            core_unit_art_final.APPROVED_SOURCE_RGBA_SHA256,
        )
        for key, image in core_unit_art_final.source_frames():
            self.assertEqual(self.registry[key].tobytes(), image.tobytes(), key)

    def test_promoted_extractor_reclaimer_family_matches_approved_source(self) -> None:
        self.assertEqual(
            extractor_reclaimer_final.source_rgba_digest(),
            extractor_reclaimer_final.APPROVED_SOURCE_RGBA_SHA256,
        )
        for faction in ("ferrous", "cupric"):
            for phase, suffix in enumerate(extractor_reclaimer_final.WORK_SUFFIXES):
                for stem, renderer in (
                    ("extractor", extractor_reclaimer_final.render_extractor),
                    ("reclaimer", extractor_reclaimer_final.render_reclaimer),
                    ("reclaimer_t1", extractor_reclaimer_final.render_refinery),
                ):
                    image = renderer(faction, phase)
                    expected_size = (128, 128) if stem == "extractor" else (64, 64)
                    self.assertEqual(image.size, expected_size)
                    self.assertEqual(
                        self.registry[f"{stem}_{faction}{suffix}"].tobytes(),
                        image.tobytes(),
                    )

    def test_kestrel_sequence_keeps_its_airframe_fixed(self) -> None:
        for faction in ("ferrous", "cupric"):
            frames = [
                airworks_scouts_final.render_kestrel(faction, phase)
                for phase in range(3)
            ]
            alpha = frames[0].getchannel("A").tobytes()
            self.assertTrue(
                all(frame.getchannel("A").tobytes() == alpha for frame in frames)
            )
            self.assertTrue(
                all(
                    _changed_pixels(left, right) > 2 for left, right in pairwise(frames)
                )
            )

    def test_airworks_queue_frames_keep_the_doors_closed(self) -> None:
        for faction in ("ferrous", "cupric"):
            frames = [
                airworks_scouts_final.render_airworks(faction, stage)
                for stage in range(5)
            ]
            door_box = (29, 44, 100, 106)
            closed = frames[0].crop(door_box).tobytes()
            self.assertEqual(frames[1].crop(door_box).tobytes(), closed)
            self.assertEqual(frames[2].crop(door_box).tobytes(), closed)
            self.assertNotEqual(frames[3].crop(door_box).tobytes(), closed)
            self.assertNotEqual(frames[4].crop(door_box).tobytes(), closed)

    def test_promoted_crucible_units_match_the_approved_rgba_source(self) -> None:
        digest = hashlib.sha256()
        renderers = (
            ("breaker", crucible_final.render_breaker),
            ("avalanche", crucible_final.render_avalanche),
        )
        states = (
            ("idle", 0, 0, ""),
            ("move1", 1, 0, "_move1"),
            ("move2", 2, 0, "_move2"),
            ("action1", 0, 1, "_action1"),
            ("action2", 0, 2, "_action2"),
            ("action3", 0, 3, "_action3"),
            ("action4", 0, 4, "_action4"),
        )
        for faction in ("ferrous", "cupric"):
            for stem, renderer in renderers:
                for label, move_phase, action, suffix in states:
                    key = f"{stem}/{faction}/{label}"
                    image = renderer(faction, move_phase, action)
                    digest.update(key.encode())
                    digest.update(image.tobytes())
                    self.assertEqual(image.size, (128, 128))
                    self.assertEqual(
                        self.registry[f"{stem}_{faction}{suffix}"].tobytes(),
                        image.tobytes(),
                    )
        self.assertEqual(digest.hexdigest(), crucible_final.APPROVED_SOURCE_RGBA_SHA256)

    def test_promoted_skyhook_sapper_and_crucible_match_the_approved_source(
        self,
    ) -> None:
        source = skyhook_sapper_crucible_final
        self.assertEqual(
            source.source_rgba_digest(), source.APPROVED_SOURCE_RGBA_SHA256
        )
        for faction in ("ferrous", "cupric"):
            for stem, renderer, size, action_count in (
                ("skyhook", source.render_skyhook, 128, 4),
                ("sapper", source.render_sapper, 64, 3),
            ):
                states = [
                    ("", 0, 0),
                    ("_move1", 1, 0),
                    ("_move2", 2, 0),
                    *(
                        (f"_action{action}", action - 1, action)
                        for action in range(1, action_count + 1)
                    ),
                ]
                for suffix, move_phase, action in states:
                    image = renderer(faction, move_phase, action)
                    self.assertEqual(image.size, (size, size))
                    self.assertEqual(
                        self.registry[f"{stem}_{faction}{suffix}"].tobytes(),
                        image.tobytes(),
                    )
            for work in range(4):
                suffix = "" if work == 0 else f"_work{work}"
                image = source.render_crucible(faction, work)
                self.assertEqual(image.size, (128, 128))
                self.assertEqual(
                    self.registry[f"crucible_{faction}{suffix}"].tobytes(),
                    image.tobytes(),
                )

    def test_skyhook_rotors_and_sapper_legs_have_real_movement(self) -> None:
        for faction in ("ferrous", "cupric"):
            for stem in ("skyhook", "sapper"):
                idle = self.registry[f"{stem}_{faction}"]
                move1 = self.registry[f"{stem}_{faction}_move1"]
                move2 = self.registry[f"{stem}_{faction}_move2"]
                self.assertNotEqual(idle.tobytes(), move1.tobytes())
                self.assertNotEqual(move1.tobytes(), move2.tobytes())
            self.assertIsNone(
                skyhook_sapper_crucible_final.render_sapper(faction, action=4)
                .getchannel("A")
                .getbbox()
            )

    def test_crucible_work_row_moves_the_hammers_and_heat(self) -> None:
        for faction in ("ferrous", "cupric"):
            frames = [
                self.registry[f"crucible_{faction}"],
                *(
                    self.registry[f"crucible_{faction}_work{work}"]
                    for work in range(1, 4)
                ),
            ]
            self.assertEqual(len({frame.tobytes() for frame in frames}), 4)

    def test_crucible_units_animate_treads_without_wobbling_the_hull(self) -> None:
        for stem in ("breaker", "avalanche"):
            for faction in ("ferrous", "cupric"):
                idle = self.registry[f"{stem}_{faction}"]
                move1 = self.registry[f"{stem}_{faction}_move1"]
                move2 = self.registry[f"{stem}_{faction}_move2"]
                self.assertEqual(
                    idle.getchannel("A").tobytes(), move1.getchannel("A").tobytes()
                )
                self.assertEqual(
                    idle.getchannel("A").tobytes(), move2.getchannel("A").tobytes()
                )
                self.assertNotEqual(idle.tobytes(), move1.tobytes())
                self.assertNotEqual(move1.tobytes(), move2.tobytes())

    def test_promoted_interceptors_match_the_approved_rgba_source(self) -> None:
        self.assertEqual(
            shrike_sylph_final.source_rgba_digest(),
            shrike_sylph_final.APPROVED_SOURCE_RGBA_SHA256,
        )
        renderers = (
            ("shrike", shrike_sylph_final.render_shrike),
            ("sylph", shrike_sylph_final.render_sylph),
        )
        suffixes = (
            "",
            "_move1",
            "_move2",
            "_action1",
            "_action2",
            "_action3",
            "_action4",
        )
        for faction in ("ferrous", "cupric"):
            for stem, renderer in renderers:
                frames = tuple(
                    renderer(faction, state) for state in shrike_sylph_final.STATES
                )
                for suffix, frame in zip(suffixes, frames, strict=True):
                    self.assertEqual(frame.size, (64, 64))
                    self.assertEqual(
                        self.registry[f"{stem}_{faction}{suffix}"].tobytes(),
                        frame.tobytes(),
                    )
                idle, move1, move2 = frames[:3]
                self.assertEqual(
                    idle.getchannel("A").tobytes(), move1.getchannel("A").tobytes()
                )
                self.assertEqual(
                    idle.getchannel("A").tobytes(), move2.getchannel("A").tobytes()
                )
                self.assertNotEqual(idle.tobytes(), move1.tobytes())
                self.assertNotEqual(move1.tobytes(), move2.tobytes())

    def test_crucible_unit_actions_preserve_one_decisive_report(self) -> None:
        for stem in ("breaker", "avalanche"):
            for faction in ("ferrous", "cupric"):
                frames = [
                    self.registry[f"{stem}_{faction}_action{action}"]
                    for action in range(1, 5)
                ]
                self.assertGreaterEqual(len({frame.tobytes() for frame in frames}), 3)
                self.assertIn(
                    (*crucible_final.FLASH, 255),
                    set(frames[1].get_flattened_data()),
                )

    def test_tender_treads_move_without_shifting_the_chassis(self) -> None:
        for faction in ("ferrous", "cupric"):
            idle = self.registry[f"tender_{faction}"]
            phases = [
                self.registry[f"tender_{faction}_move{phase}"] for phase in (1, 2)
            ]
            self.assertEqual(idle.size, (64, 64))
            for frame in phases:
                self.assertEqual(
                    idle.getchannel("A").tobytes(), frame.getchannel("A").tobytes()
                )
                changed = ImageChops.difference(idle, frame)
                changed_points = [
                    (index % idle.width, index // idle.width)
                    for index, pixel in enumerate(changed.get_flattened_data())
                    if pixel != (0, 0, 0, 0)
                ]
                self.assertGreater(len(changed_points), 8)
                self.assertTrue(
                    all(
                        (7 <= x <= 19 or 45 <= x <= 57) and 17 <= y <= 58
                        for x, y in changed_points
                    ),
                    "Tender locomotion must move its tread cleats, not wobble the hull",
                )
            self.assertNotEqual(phases[0].tobytes(), phases[1].tobytes())

    def test_promoted_moth_and_warden_match_the_approved_rgba_source(self) -> None:
        self.assertEqual(
            moth_warden_final.source_rgba_digest(),
            moth_warden_final.APPROVED_SOURCE_RGBA_SHA256,
        )
        for faction in ("ferrous", "cupric"):
            for stem, renderer, action_count in (
                ("moth", moth_warden_final.render_moth, 6),
                ("warden", moth_warden_final.render_warden, 4),
            ):
                states = (
                    ("", 0, 0),
                    ("_move1", 1, 0),
                    ("_move2", 2, 0),
                    *(
                        (f"_action{action}", 0, action)
                        for action in range(1, action_count + 1)
                    ),
                )
                for suffix, move_phase, action in states:
                    image = renderer(faction, move_phase, action)
                    self.assertEqual(image.size, (128, 128))
                    self.assertEqual(
                        self.registry[f"{stem}_{faction}{suffix}"].tobytes(),
                        image.tobytes(),
                    )

    def test_promoted_excavator_matches_candidate_423_and_keeps_channels_independent(
        self,
    ) -> None:
        self.assertEqual(
            excavator_final.source_rgba_digest(),
            excavator_final.APPROVED_SOURCE_RGBA_SHA256,
        )
        for faction in ("ferrous", "cupric"):
            states = (
                ("", 0, 0),
                ("_move1", 1, 0),
                ("_move2", 2, 0),
                *((f"_action{phase}", 0, phase) for phase in range(1, 5)),
            )
            for suffix, move_phase, work_phase in states:
                image = excavator_final.render_excavator(
                    faction, move_phase, work_phase
                )
                self.assertEqual(image.size, (128, 128))
                self.assertEqual(
                    self.registry[f"excavator_{faction}{suffix}"].tobytes(),
                    image.tobytes(),
                )
            idle = self.registry[f"excavator_{faction}"]
            for phase in (1, 2):
                moving = self.registry[f"excavator_{faction}_move{phase}"]
                self.assertEqual(
                    idle.getchannel("A").tobytes(),
                    moving.getchannel("A").tobytes(),
                )
                self.assertNotEqual(idle.tobytes(), moving.tobytes())

    def test_excavator_meter_matches_the_harvesters_five_load_levels(self) -> None:
        frames = [
            self.registry[f"excavator_cargo{level}"]
            for level in range(excavator_final.CARGO_LEVELS)
        ]
        self.assertEqual(len({frame.tobytes() for frame in frames}), 5)
        self.assertIsNone(frames[0].getchannel("A").getbbox())
        widths = []
        for frame in frames:
            bbox = frame.getchannel("A").getbbox()
            widths.append(0 if bbox is None else bbox[2] - bbox[0])
        self.assertTrue(all(left <= right for left, right in pairwise(widths)))
        self.assertGreater(widths[-1], widths[1])

    def test_moth_and_warden_move_without_wobbling_the_hull(self) -> None:
        for stem in ("moth", "warden"):
            for faction in ("ferrous", "cupric"):
                idle = self.registry[f"{stem}_{faction}"]
                phases = [
                    self.registry[f"{stem}_{faction}_move{phase}"] for phase in (1, 2)
                ]
                for frame in phases:
                    self.assertEqual(
                        idle.getchannel("A").tobytes(),
                        frame.getchannel("A").tobytes(),
                    )
                self.assertTrue(
                    any(idle.tobytes() != frame.tobytes() for frame in phases)
                )
                self.assertNotEqual(phases[0].tobytes(), phases[1].tobytes())

    def test_moth_empties_all_six_bays_and_warden_reports_once(self) -> None:
        bomb_centers = ((49, 45), (79, 45), (49, 59), (79, 59), (49, 73), (79, 73))
        for faction in ("ferrous", "cupric"):
            for action in range(1, 7):
                frame = self.registry[f"moth_{faction}_action{action}"]
                remaining = sum(
                    frame.getpixel(center) == (*moth_warden_final.BONE, 255)
                    for center in bomb_centers
                )
                self.assertEqual(remaining, 6 - action)
            for action in range(1, 5):
                pixels = set(
                    self.registry[
                        f"warden_{faction}_action{action}"
                    ].get_flattened_data()
                )
                self.assertEqual(
                    (*moth_warden_final.FLASH, 255) in pixels,
                    action == 2,
                )

    def test_tender_reserves_its_bright_tool_color_for_welding(self) -> None:
        bright_tool = (*tender_condor_final.WELD, 255)
        bright_scrap = (*tender_condor_final.SCRAP, 255)
        for faction in ("ferrous", "cupric"):
            idle_pixels = set(self.registry[f"tender_{faction}"].get_flattened_data())
            self.assertNotIn(bright_tool, idle_pixels)
            self.assertNotIn(bright_scrap, idle_pixels)
            self.assertIn(
                bright_tool,
                set(self.registry[f"tender_{faction}_action3"].get_flattened_data()),
            )

    def test_condor_uses_a_fixed_large_silhouette_without_move_wobble(self) -> None:
        for faction in ("ferrous", "cupric"):
            idle = self.registry[f"condor_{faction}"]
            self.assertEqual(idle.size, (128, 128))
            self.assertEqual(idle.getbbox(), (8, 18, 121, 96))
            for phase in (1, 2):
                self.assertEqual(
                    idle.tobytes(),
                    self.registry[f"condor_{faction}_move{phase}"].tobytes(),
                )
            for action in range(1, 5):
                frame = self.registry[f"condor_{faction}_action{action}"]
                self.assertEqual(frame.size, idle.size)
                self.assertEqual(
                    frame.getchannel("A").tobytes(), idle.getchannel("A").tobytes()
                )

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

    def test_promoted_bastion_and_turret_family_match_approved_sources(self) -> None:
        self.assertEqual(
            heavy_structures.bastion_source_visible_digest(),
            heavy_structures.BASTION_APPROVED_VISIBLE_RGBA_SHA256,
        )
        self.assertEqual(
            turret_family.turret_source_visible_digest(),
            turret_family.TURRET_APPROVED_VISIBLE_RGBA_SHA256,
        )
        for faction in gen.FACTIONS:
            for tier, (base_stem, mount_stem) in enumerate(
                (
                    ("turret", "turret_barrel"),
                    ("turret_t1", "turret_barrel_t1"),
                    ("turret_t2", "turret_barrel_t2"),
                )
            ):
                for phase in range(5):
                    suffix = "" if phase == 0 else f"_action{phase}"
                    frame = self.registry[f"{base_stem}_{faction}"].copy()
                    frame.alpha_composite(
                        self.registry[f"{mount_stem}_{faction}{suffix}"]
                    )
                    self.assertEqual(
                        turret_family._visible_rgba_bytes(frame),
                        turret_family._visible_rgba_bytes(
                            turret_family.turret_frame(faction, tier, phase)
                        ),
                        f"{mount_stem}_{faction}{suffix}",
                    )

    def test_promoted_flak_and_deep_array_match_approved_sources(self) -> None:
        self.assertEqual(
            flak_array_final.flak_source_visible_digest(),
            flak_array_final.FLAK_APPROVED_VISIBLE_RGBA_SHA256,
        )
        self.assertEqual(
            flak_array_final.deep_array_source_visible_digest(),
            flak_array_final.DEEP_ARRAY_APPROVED_VISIBLE_RGBA_SHA256,
        )
        for faction in gen.FACTIONS:
            for tier, (base_stem, mount_stem) in enumerate(
                (("flak_turret", "flak_mount"), ("flak_turret_t1", "flak_mount_t1"))
            ):
                for phase in range(9):
                    suffix = "" if phase == 0 else f"_action{phase}"
                    frame = self.registry[f"{base_stem}_{faction}"].copy()
                    frame.alpha_composite(
                        self.registry[f"{mount_stem}_{faction}{suffix}"]
                    )
                    self.assertEqual(
                        flak_array_final._visible_rgba_bytes(frame),
                        flak_array_final._visible_rgba_bytes(
                            flak_array_final.flak_frame(faction, tier, phase)
                        ),
                        f"{mount_stem}_{faction}{suffix}",
                    )
                    self.assertTrue(
                        flak_array_final.factions_share_silhouette("flak", tier, phase)
                    )

            for phase in range(7):
                suffix = "" if phase == 0 else f"_work{phase}"
                self.assertEqual(
                    flak_array_final._visible_rgba_bytes(
                        self.registry[f"array_t1_{faction}{suffix}"]
                    ),
                    flak_array_final._visible_rgba_bytes(
                        flak_array_final.deep_array_frame(faction, phase)
                    ),
                    f"array_t1_{faction}{suffix}",
                )
                self.assertTrue(
                    flak_array_final.factions_share_silhouette("array", 1, phase)
                )

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
                    "movement must retain the approved tool",
                )
                self.assertEqual(
                    self.registry[prefix].crop((25, 7, 40, 16)).tobytes(),
                    self.registry[prefix + "_scoop1"].crop((25, 7, 40, 16)).tobytes(),
                    "the bucket stays tucked while the pincers deploy",
                )
                for pincer_box in ((5, 5, 23, 32), (41, 5, 59, 32)):
                    self.assertGreater(
                        _changed_pixels(
                            self.registry[prefix].crop(pincer_box),
                            self.registry[prefix + "_scoop1"].crop(pincer_box),
                        ),
                        100,
                        "both pincers must deploy before the bucket advances",
                    )
                self.assertGreater(
                    _changed_pixels(
                        self.registry[prefix + "_scoop1"].crop((23, 5, 42, 25)),
                        self.registry[prefix + "_scoop2"].crop((23, 5, 42, 25)),
                    ),
                    20,
                    "the bucket advances only after the pincers deploy",
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
            ("turret_t1", "turret_barrel_t1", 64),
            ("turret_t2", "turret_barrel_t2", 64),
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

    def test_construction_keeps_the_complete_hull_visible_from_stage_zero(self) -> None:
        for faction in gen.FACTIONS:
            for stem in construction_final.BUILDING_STEMS:
                hull = construction_final.complete_hull(self.registry, stem, faction)
                occupied = [
                    index
                    for index, value in enumerate(
                        hull.getchannel("A").get_flattened_data()
                    )
                    if value
                ]
                self.assertTrue(occupied)
                for stage in range(3):
                    frame_alpha = self.registry[
                        f"{stem}_{faction}_site{stage}_0"
                    ].getchannel("A")
                    with self.subTest(faction=faction, stem=stem, stage=stage):
                        self.assertTrue(
                            all(
                                frame_alpha.getpixel(
                                    (index % hull.width, index // hull.width)
                                )
                                for index in occupied
                            )
                        )

    def test_construction_hull_energy_increases_without_a_reveal_wipe(self) -> None:
        for faction in gen.FACTIONS:
            for stem in construction_final.BUILDING_STEMS:
                hull = construction_final.complete_hull(self.registry, stem, faction)
                alpha_totals = [
                    sum(
                        construction_final.dimmed_hull(hull, stage)
                        .getchannel("A")
                        .get_flattened_data()
                    )
                    for stage in range(3)
                ]
                with self.subTest(faction=faction, stem=stem):
                    self.assertLess(alpha_totals[0], alpha_totals[1])
                    self.assertLess(alpha_totals[1], alpha_totals[2])

    def test_construction_cage_is_fixed_and_active_delta_is_local(self) -> None:
        for faction in gen.FACTIONS:
            for stem in construction_final.BUILDING_STEMS:
                first = self.registry[f"{stem}_{faction}_site0_0"]
                scale = first.width / 64
                fixed_points = (
                    (round(6 * scale), round(16 * scale)),
                    (first.width - round(6 * scale), round(16 * scale)),
                    (round(20 * scale), round(9 * scale)),
                    (round(20 * scale), round(32 * scale)),
                    (round(20 * scale), first.height - round(7 * scale)),
                )
                fixed_colors = tuple(first.getpixel(point) for point in fixed_points)
                for stage in range(3):
                    still = self.registry[f"{stem}_{faction}_site{stage}_0"]
                    active = self.registry[f"{stem}_{faction}_site{stage}_1"]
                    difference = ImageChops.difference(still, active)
                    bbox = difference.getbbox()
                    with self.subTest(faction=faction, stem=stem, stage=stage):
                        self.assertEqual(
                            tuple(still.getpixel(point) for point in fixed_points),
                            fixed_colors,
                        )
                        self.assertIsNotNone(bbox)
                        self.assertLessEqual(bbox[2] - bbox[0], round(9 * scale))
                        self.assertLessEqual(bbox[3] - bbox[1], round(9 * scale))

    def test_defense_sites_include_their_recognizable_mounts(self) -> None:
        for faction in gen.FACTIONS:
            for stem, mount_stem in construction_final.DEFENSE_MOUNTS.items():
                mount = self.registry[f"{mount_stem}_{faction}"].getchannel("A")
                site = self.registry[f"{stem}_{faction}_site0_0"].getchannel("A")
                mount_pixels = [
                    index
                    for index, value in enumerate(mount.get_flattened_data())
                    if value
                ]
                with self.subTest(faction=faction, stem=stem):
                    self.assertTrue(mount_pixels)
                    self.assertTrue(
                        all(
                            site.getpixel((index % site.width, index // site.width))
                            for index in mount_pixels
                        )
                    )

    def test_reclaimer_is_the_exact_approved_guarded_feed_sequence(self) -> None:
        expected = (
            "13b9f75a7086f79676776d7a28e9542bd86ef29ce14af013348c5ec7554581b5",
            "5ac8b3f9486beeacfe959a5e0590813bd91c88d48349e93221c41ea5cc6bc84d",
            "ac6ea8df80b1d7a2183b081beee261060ea82c7cf2b99d1889219e3f4645170f",
            "879f3ae9ac5a271e03aabe77f41d50e2427f328aadb4a46d557c2189166c5c42",
        )
        actual = tuple(
            hashlib.sha256(
                self.registry[f"reclaimer_ferrous{suffix}"].convert("RGBA").tobytes()
            ).hexdigest()
            for suffix in ("", "_work1", "_work2", "_work3")
        )
        self.assertEqual(actual, expected)

    def test_buzzard_matches_the_approved_quad_fan_sequence(self) -> None:
        expected = (
            "782c0994ca8601fe27e196a4328d8df3f8144ca5b13664cc96a462e596a5f973",
            "a2b581226a2042792865fcc4740b7650ebcdba8d1b10f19beec21ad30d434f78",
            "1230b5ca5f5a39a14216309b0cc994ba31fb2d2221d31da15265a89fe95f0005",
            "782c0994ca8601fe27e196a4328d8df3f8144ca5b13664cc96a462e596a5f973",
            "11f7861da155eafb98ff0feca6ad0be9496ae99f9d80a96b7e9b0a65fcdcd352",
            "42e3e652114d3332058f87182b4745ea67e6cfd9e9cfdba0b377193bb5d6873f",
            "8d2af0abdedc5c06f2822d6a4a8b523f1cae50d6926ba3100a0b880fd374eb0d",
            "782c0994ca8601fe27e196a4328d8df3f8144ca5b13664cc96a462e596a5f973",
        )
        sequence = finalized._unit_sequences()["buzzard"]()
        actual = tuple(
            hashlib.sha256(frame.image.convert("RGBA").tobytes()).hexdigest()
            for frame in sequence.frames
        )
        self.assertEqual(actual, expected)

    def test_buzzard_attack_flare_does_not_touch_the_canvas_edge(self) -> None:
        for faction in gen.FACTIONS:
            frame = self.registry[f"buzzard_{faction}_action2"].getchannel("A")
            edge = list(frame.crop((0, 0, frame.width, 1)).get_flattened_data())
            self.assertFalse(any(edge), f"{faction} muzzle flare is clipped")

    def test_foundry_work_frames_move_the_crane_and_sequence_the_lights(self) -> None:
        frames = [
            self.registry[f"foundry_ferrous{suffix}"].convert("RGBA")
            for suffix in ("", "_work1", "_work2", "_work3", "_work4")
        ]
        self.assertEqual(len({frame.tobytes() for frame in frames}), len(frames))
        for frame in frames[1:]:
            self.assertGreater(
                _changed_pixels(
                    frames[0].crop((20, 12, 108, 58)), frame.crop((20, 12, 108, 58))
                ),
                20,
                "the Foundry crane must travel while production advances",
            )

        center_values = [sum(frame.getpixel((64, 72))[:3]) for frame in frames]
        self.assertLess(center_values[0], center_values[1])
        self.assertLess(center_values[1], center_values[2])
        self.assertEqual(center_values[1], center_values[3])
        self.assertEqual(center_values[0], center_values[4])

        lit_positions = ((19, 27), (19, 45), (19, 63), (19, 81))
        for work, frame in enumerate(frames[1:], start=1):
            values = [sum(frame.getpixel(position)[:3]) for position in lit_positions]
            self.assertEqual(values.index(max(values)), work - 1)

    def test_bastion_ready_and_reload_frames_have_physical_charge_cells(self) -> None:
        centers = [(19, 54 + index * 10) for index in range(5)]
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

    def test_bastion_recoils_after_report_then_returns_quickly(self) -> None:
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
                self.assertAlmostEqual(centers[0], centers[1], delta=1.0)
                self.assertGreater(centers[2] - centers[1], 4.0)
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
