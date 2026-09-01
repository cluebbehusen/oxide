import hashlib
import tempfile
import unittest
from itertools import pairwise
from pathlib import Path

from PIL import Image, ImageChops

from tools import gen_sprites as gen
from tools.production_sprite_sources import (
    construction_final,
    crucible_final,
    environment_final,
    excavator_final,
    finalized,
    moth_warden_final,
    tender_condor_final,
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
