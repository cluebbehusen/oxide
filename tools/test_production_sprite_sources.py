import hashlib
import tempfile
import unittest
from itertools import pairwise
from pathlib import Path

from PIL import Image, ImageChops

from tools import gen_sprites as gen
from tools.production_sprite_sources import (
    air_final,
    air_support_final,
    airworks_scouts_final,
    construction_final,
    environment_final,
    excavator_final,
    finalized,
    installed_defenses_final,
    mechanical_final,
    quarry_final,
    skyhook_sapper_crucible_final,
    specialists_final,
    tier_one_combat_final,
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
            for phase in range(6):
                gen.ground(phase)
            cls.ground_controls = cls.registry.copy()
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

    def test_articulated_machine_bank_matches_approved_pixels(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            frames = {}
            mechanical_final.install_machines(frames, Path(directory))
        self.assertEqual(len(frames), 499)
        digest = hashlib.sha256()
        for key, image in sorted(frames.items()):
            self.assertEqual(self.registry[key].tobytes(), image.tobytes(), key)
            digest.update(key.encode())
            digest.update(image.tobytes())
        self.assertEqual(
            digest.hexdigest(),
            "2bbd5c9c9139a5a88d67ae0a6b4a3dfeefd00a6bf67a6ea73dbfb82b2d012440",
        )

    def test_promoted_quarry_matches_approved_pixels(self) -> None:
        digest = hashlib.sha256()
        for key, image in sorted(quarry_final.source_frames(self.ground_controls)):
            self.assertEqual(self.registry[key].tobytes(), image.tobytes(), key)
            digest.update(key.encode())
            digest.update(image.tobytes())
        self.assertEqual(
            digest.hexdigest(),
            "c4d211b58f896f010e6169f2e5730821430af0e8e02723f4cf0c530d46231874",
        )

    def test_promoted_specialists_match_approved_pixels(self) -> None:
        digest = hashlib.sha256()
        for key, image in sorted(specialists_final.source_frames()):
            self.assertEqual(self.registry[key].tobytes(), image.tobytes(), key)
            digest.update(key.encode())
            digest.update(image.tobytes())
        self.assertEqual(
            digest.hexdigest(),
            "a3887ffc13de6447106bbd757a3aa3f5e6ffa2d2b29954b6adb9de1e4f7400dd",
        )

    def test_promoted_defenses_match_approved_pixels(self) -> None:
        digest = hashlib.sha256()
        for key, image in sorted(installed_defenses_final.source_frames()):
            self.assertEqual(self.registry[key].tobytes(), image.tobytes(), key)
            digest.update(key.encode())
            digest.update(image.tobytes())
        self.assertEqual(
            digest.hexdigest(),
            "3711c93033f318f198d469c81c136a9eddbc31df1f40f83ce76a156441025d3f",
        )

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
            "b2a1438a5ee46f53243ba66666a2424b4972763113efa54d6b162a5ff27e92c9",
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
                self.assertTrue(
                    any(
                        _changed_pixels(
                            base, self.registry[f"{stem}_{faction}{suffix}"]
                        )
                        > 2
                        for suffix in frame_set.suffixes
                    ),
                    stem,
                )

    def test_air_support_detail_passes_preserve_the_approved_silhouettes(self) -> None:
        for faction in ("ferrous", "cupric"):
            with finalized._faction_palette(faction):
                for base_builder, approved_builder in (
                    (air_final.buzzard_sequence, air_support_final.buzzard_sequence),
                    (air_final.talon_sequence, air_support_final.talon_sequence),
                    (air_final.wisp_sequence, air_support_final.wisp_sequence),
                ):
                    base = base_builder()
                    approved = approved_builder()
                    self.assertEqual(len(base.frames), len(approved.frames))
                    for base_frame, approved_frame in zip(
                        base.frames, approved.frames, strict=True
                    ):
                        self.assertEqual(
                            base_frame.image.getchannel("A").tobytes(),
                            approved_frame.image.getchannel("A").tobytes(),
                        )

    def test_darter_action_frames_keep_transparent_canvas_margin(self) -> None:
        for faction in ("ferrous", "cupric"):
            for suffix in finalized.UNIT_ACTIONS["darter"].suffixes:
                image = self.registry[f"darter_{faction}{suffix}"]
                bbox = image.getchannel("A").getbbox()
                self.assertIsNotNone(bbox)
                assert bbox is not None
                self.assertGreater(bbox[0], 0)
                self.assertGreater(bbox[1], 0)
                self.assertLess(bbox[2], image.width)
                self.assertLess(bbox[3], image.height)

    def test_tier_one_combat_art_preserves_motion_and_attack_contracts(self) -> None:
        for builder in (
            tier_one_combat_final.lancer_sequence,
            tier_one_combat_final.bombard_sequence,
            tier_one_combat_final.flakhound_sequence,
            tier_one_combat_final.stinger_sequence,
        ):
            sequence = builder()
            idle, move1, move2 = (frame.image for frame in sequence.frames[:3])
            self.assertEqual(idle.size, (64, 64))
            self.assertNotEqual(idle.tobytes(), move1.tobytes())
            self.assertNotEqual(move1.tobytes(), move2.tobytes())
            damage_frames = [frame for frame in sequence.frames if frame.logical_damage]
            self.assertEqual(len(damage_frames), 1)
            self.assertGreaterEqual(damage_frames[0].report_count, 1)

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

    def test_crucible_opens_and_closes_its_segmented_lid(self) -> None:
        for faction in gen.FACTIONS:
            frames = [
                self.registry[f"crucible_{faction}" + (f"_work{i}" if i else "")]
                for i in range(4)
            ]
            self.assertEqual(frames[1].tobytes(), frames[3].tobytes())
            self.assertEqual(len({frame.tobytes() for frame in frames}), 3)
            for frame in frames[1:]:
                self.assertEqual(
                    frame.crop((0, 0, 128, 40)).tobytes(),
                    frames[0].crop((0, 0, 128, 40)).tobytes(),
                )
                self.assertGreater(
                    _changed_pixels(
                        frames[0].crop((45, 50, 85, 90)), frame.crop((45, 50, 85, 90))
                    ),
                    20,
                )

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

    def test_heavy_weapon_actions_preserve_distinct_launch_and_recovery_poses(
        self,
    ) -> None:
        for stem in ("breaker", "avalanche"):
            for faction in gen.FACTIONS:
                frames = [
                    self.registry[f"{stem}_{faction}_action{i}"] for i in range(1, 5)
                ]
                self.assertGreaterEqual(len({frame.tobytes() for frame in frames}), 3)
                self.assertNotEqual(frames[0].tobytes(), frames[1].tobytes())

    def test_tracked_workers_move_only_their_treads(self) -> None:
        for stem in ("tender", "excavator"):
            for faction in gen.FACTIONS:
                idle = self.registry[f"{stem}_{faction}"]
                for phase in (1, 2):
                    frame = self.registry[f"{stem}_{faction}_move{phase}"]
                    self.assertEqual(
                        idle.getchannel("A").tobytes(), frame.getchannel("A").tobytes()
                    )
                    self.assertEqual(
                        idle.crop((38, 0, 90, 128)).tobytes(),
                        frame.crop((38, 0, 90, 128)).tobytes(),
                    )
                    for tread in ((10, 36, 38, 117), (90, 36, 118, 117)):
                        self.assertGreater(
                            _changed_pixels(idle.crop(tread), frame.crop(tread)), 8
                        )

    def test_excavator_meter_matches_the_harvesters_five_load_levels(self) -> None:
        frames = [
            self.registry[f"excavator_cargo{level}"]
            for level in range(excavator_final.CARGO_LEVELS)
        ]
        self.assertEqual(len({frame.tobytes() for frame in frames}), 5)
        self.assertIsNone(frames[0].getchannel("A").getbbox())
        areas = [
            sum(value > 0 for value in frame.getchannel("A").get_flattened_data())
            for frame in frames
        ]
        self.assertTrue(all(left < right for left, right in pairwise(areas)))

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

    def test_moth_reloads_its_six_racks_in_pairs(self) -> None:
        centers = ((49, 45), (79, 45), (49, 59), (79, 59), (49, 73), (79, 73))
        for faction in gen.FACTIONS:
            idle = self.registry[f"moth_{faction}"]
            for action, expected in enumerate((0, 0, 0, 2, 4, 6), start=1):
                frame = self.registry[f"moth_{faction}_action{action}"]
                loaded = sum(
                    frame.getpixel(center) == idle.getpixel(center)
                    for center in centers
                )
                self.assertEqual(loaded, expected)

    def test_tender_adds_sparks_only_at_welding_contact(self) -> None:
        for faction in gen.FACTIONS:
            contact = self.registry[f"tender_{faction}_action2"]
            weld = self.registry[f"tender_{faction}_action3"]
            self.assertEqual(
                contact.crop((0, 32, 128, 128)).tobytes(),
                weld.crop((0, 32, 128, 128)).tobytes(),
            )
            self.assertGreater(
                _changed_pixels(
                    contact.crop((50, 0, 80, 25)), weld.crop((50, 0, 80, 25))
                ),
                5,
            )

    def test_condor_keeps_its_wing_fixed_while_the_nose_opens(self) -> None:
        for faction in gen.FACTIONS:
            idle = self.registry[f"condor_{faction}"]
            self.assertEqual(idle.size, (128, 128))
            for phase in (1, 2):
                self.assertEqual(
                    idle.tobytes(),
                    self.registry[f"condor_{faction}_move{phase}"].tobytes(),
                )
            for action in range(1, 5):
                frame = self.registry[f"condor_{faction}_action{action}"]
                self.assertEqual(
                    frame.crop((0, 40, 128, 128)).tobytes(),
                    idle.crop((0, 40, 128, 128)).tobytes(),
                )
            open_nose = self.registry[f"condor_{faction}_action2"]
            self.assertLess(open_nose.getpixel((64, 24))[3], idle.getpixel((64, 24))[3])

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

    def test_harvester_keeps_cargo_separate_from_tracks_and_grapple(self) -> None:
        for faction in gen.FACTIONS:
            loads = []
            for cargo in range(finalized.HARVESTER_CARGO_LEVELS):
                prefix = f"harvester_{faction}_cargo{cargo}"
                idle = self.registry[prefix]
                loads.append(idle.crop((44, 67, 84, 101)).tobytes())
                for suffix in ("_tread1", "_tread2", "_scoop1", "_scoop2"):
                    frame = self.registry[prefix + suffix]
                    self.assertEqual(
                        idle.crop((44, 67, 84, 101)).tobytes(),
                        frame.crop((44, 67, 84, 101)).tobytes(),
                    )
                    self.assertNotEqual(idle.tobytes(), frame.tobytes())
                for suffix in ("_tread1", "_tread2"):
                    self.assertEqual(
                        idle.crop((0, 0, 128, 32)).tobytes(),
                        self.registry[prefix + suffix].crop((0, 0, 128, 32)).tobytes(),
                    )
                for claw in ((30, 3, 64, 55), (64, 3, 98, 55)):
                    self.assertGreater(
                        _changed_pixels(
                            idle.crop(claw),
                            self.registry[prefix + "_scoop1"].crop(claw),
                        ),
                        20,
                    )
            self.assertEqual(len(set(loads)), 5)

    def test_defense_foundations_and_mounts_are_separate_square_layers(self) -> None:
        pairs = (
            ("turret", "turret_barrel", 128),
            ("turret_t1", "turret_barrel_t1", 128),
            ("turret_t2", "turret_barrel_t2", 128),
            ("flak_turret", "flak_mount", 128),
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

    def test_buzzard_matches_the_approved_armored_quad_fan_sequence(self) -> None:
        expected = (
            "1143a769ec064862af3d9fe9837b15434bd60326b45af2415fe267e3c80cb56d",
            "d5d7eb5d4f8a152bdc5a7ddabe52aa8093f1c99333469d9d841a5838021ba971",
            "018ba81c920b462f0a9f9f3fa009d6685c3886d8daf289d6e74155786c62b4d4",
            "1143a769ec064862af3d9fe9837b15434bd60326b45af2415fe267e3c80cb56d",
            "acc78432f812a96dea0e1551c251169d7f24f57174769e87e2a53162b09ed1b7",
            "f945d232516998e8518ba3320ad4e8556ada1131a39ffd072ecf0e28940a9fd7",
            "ed31b38adce84cd193df0e44fd047d95007cccf742b71c8b729c7f1dfea135b8",
            "1143a769ec064862af3d9fe9837b15434bd60326b45af2415fe267e3c80cb56d",
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

    def test_foundry_gantry_moves_over_a_fixed_foundation(self) -> None:
        for faction in gen.FACTIONS:
            frames = [
                self.registry[f"foundry_{faction}" + (f"_work{i}" if i else "")]
                for i in range(5)
            ]
            self.assertEqual(len({frame.tobytes() for frame in frames}), 5)
            for frame in frames[1:]:
                self.assertEqual(
                    frame.crop((0, 0, 128, 30)).tobytes(),
                    frames[0].crop((0, 0, 128, 30)).tobytes(),
                )
                self.assertGreater(
                    _changed_pixels(
                        frames[0].crop((30, 34, 100, 100)),
                        frame.crop((30, 34, 100, 100)),
                    ),
                    100,
                )

    def test_bastion_ready_and_reload_frames_have_physical_charge_cells(self) -> None:
        centers = [(25, 53 + index * 8) for index in range(5)]
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
                    sum(
                        (a - b) ** 2
                        for a, b in zip(
                            image.getpixel(center)[:3], installed_defenses_final.BRASS
                        )
                    )
                    < 100
                    for center in centers
                )
                with self.subTest(faction=faction, suffix=suffix):
                    self.assertEqual(lit, count)

    def test_bastion_recoils_after_report_then_returns_quickly(self) -> None:
        for faction in gen.FACTIONS:
            muzzle_tops = [
                self.registry[f"bastion_mount_{faction}{suffix}"]
                .crop((55, 0, 73, 40))
                .getchannel("A")
                .getbbox()[1]
                for suffix in ("_action5", "_action7", "_action8", "_action9")
            ]
            with self.subTest(faction=faction):
                ready, recoil, settling, returned = muzzle_tops
                self.assertEqual(recoil - ready, 7)
                self.assertEqual(settling - ready, 3)
                self.assertEqual(returned, ready)

    def test_flakhound_magazines_fill_during_reload(self) -> None:
        expected = {
            "": 4,
            "_tread1": 4,
            "_tread2": 4,
            "_action1": 0,
            "_action2": 1,
            "_action3": 2,
            "_action4": 3,
            "_action5": 4,
            "_action6": 1,
            "_action7": 1,
            "_action8": 1,
            "_action9": 1,
        }
        for faction in gen.FACTIONS:
            for suffix, count in expected.items():
                image = self.registry[f"flakhound_{faction}{suffix}"]
                for x in (34, 94):
                    loaded = 0
                    for index in range(4):
                        pixel = image.getpixel((x, 73 + index * 3))[:3]
                        brass = sum(
                            (a - b) ** 2 for a, b in zip(pixel, specialists_final.BRASS)
                        )
                        empty = sum(
                            (a - b) ** 2 for a, b in zip(pixel, specialists_final.DEEP)
                        )
                        loaded += brass < empty
                    self.assertEqual(loaded, count, (faction, suffix, x))

    def test_bombard_spades_and_muzzle_report_fit_their_canvases(self) -> None:
        for phase in range(5):
            image = self.registry[f"bombard_spades_{phase}"]
            left, top, right, bottom = image.getbbox()
            self.assertTrue(0 < left < right < image.width)
            self.assertTrue(0 < top < bottom < image.height)
        for faction in gen.FACTIONS:
            report = self.registry[f"bombard_{faction}_action4"]
            flashes = sum(
                pixel[0] > 200 and pixel[1] > 130 and pixel[3] > 128
                for pixel in report.crop((50, 8, 79, 35)).get_flattened_data()
            )
            self.assertGreater(flashes, 0)


if __name__ == "__main__":
    unittest.main()
