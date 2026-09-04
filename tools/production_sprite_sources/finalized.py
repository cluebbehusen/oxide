"""Canonical native frames for Oxide's finalized machine art.

The public installer writes only gameplay-sized RGBA images into the sprite
generator's registry.  Review cards and their presentation framing are not
runtime inputs.  Every moving mechanism also carries explicit phase metadata
so the shell can select frames from simulation facts instead of free-running
the whole sprite as a decorative loop.
"""

from __future__ import annotations

from collections.abc import Callable, Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path

from PIL import Image, ImageDraw

from tools import gen_sprites as gen
from tools.production_sprite_sources import (
    air_support_final,
    airworks_scouts_final,
    core_unit_art_final,
    crucible_final,
    defense_mechanisms,
    economy_mechanisms,
    excavator_final,
    extractor_reclaimer_final,
    field_structures_final,
    flak_array_final,
    ground_base,
    ground_final,
    heavy_structures,
    moth_warden_final,
    shrike_sylph_final,
    skyhook_sapper_crucible_final,
    structures_base,
    tender_condor_final,
    tier_one_combat_final,
    turret_family,
)

Registry = dict[str, Image.Image]
SequenceBuilder = Callable[[], ground_base.GroundUnitSequence]


@dataclass(frozen=True)
class FrameSet:
    """One atlas row's suffixes, events, and authored frame durations."""

    suffixes: tuple[str, ...]
    events: tuple[str, ...]
    durations_ms: tuple[int, ...]


UNIT_MOVEMENT: dict[str, FrameSet] = {
    "sentinel": FrameSet(("_move1", "_move2"), ("travel_1", "travel_2"), (150, 150)),
    "scuttler": FrameSet(
        ("_move1", "_move2"), ("leg_step_a", "leg_step_b"), (130, 130)
    ),
    "lancer": FrameSet(("_move1", "_move2"), ("travel_1", "travel_2"), (150, 150)),
    "bombard": FrameSet(("_move1", "_move2"), ("travel_1", "travel_2"), (170, 170)),
    "flakhound": FrameSet(("_tread1", "_tread2"), ("travel_1", "travel_2"), (150, 150)),
    "stinger": FrameSet(
        ("_move1", "_move2"), ("wheel_step_a", "wheel_step_b"), (130, 130)
    ),
    "buzzard": FrameSet(
        ("_move1", "_move2"),
        ("rotor_phase_a", "rotor_phase_b"),
        (160, 160),
    ),
    "darter": FrameSet(
        ("_move1", "_move2"),
        ("internal_propulsion_a", "internal_propulsion_b"),
        (150, 150),
    ),
    "talon": FrameSet(
        ("_move1", "_move2"),
        ("internal_propulsion_a", "internal_propulsion_b"),
        (150, 150),
    ),
    "wisp": FrameSet(
        ("_move1", "_move2"), ("rotor_phase_a", "rotor_phase_b"), (150, 150)
    ),
}

UNIT_ACTIONS: dict[str, FrameSet] = {
    "sentinel": FrameSet(
        tuple(f"_action{i}" for i in range(1, 5)),
        ("breech_lock", "damage+barrel_report", "breech_return", "attack_settle"),
        (140, 100, 150, 460),
    ),
    "scuttler": FrameSet(
        tuple(f"_action{i}" for i in range(1, 5)),
        ("shear_open", "damage+shear_bite", "shear_release", "attack_settle"),
        (150, 120, 180, 440),
    ),
    "lancer": FrameSet(
        tuple(f"_action{i}" for i in range(1, 7)),
        (
            "charge_cell_1",
            "charge_cell_2",
            "charge_cell_3",
            "damage+rail_report",
            "rail_return",
            "attack_settle",
        ),
        (140, 140, 170, 100, 180, 480),
    ),
    "bombard": FrameSet(
        tuple(f"_action{i}" for i in range(1, 7)),
        (
            "rack_shell_selected",
            "shell_on_loading_tray",
            "shell_ram+breech_lock+spades_plant",
            "damage+artillery_launch",
            "gun_return",
            "attack_settle",
        ),
        (230, 230, 260, 160, 240, 540),
    ),
    "flakhound": FrameSet(
        tuple(f"_action{i}" for i in range(1, 10)),
        (
            "charge_0",
            "charge_1",
            "charge_2",
            "charge_3",
            "charge_4",
            "report_left_yoke",
            "damage+report_right_yoke",
            "paired_yokes_recover",
            "attack_settle",
        ),
        (120, 120, 120, 120, 180, 100, 110, 180, 500),
    ),
    "stinger": FrameSet(
        tuple(f"_action{i}" for i in range(1, 5)),
        (
            "paired_yoke_lock",
            "damage+paired_aa_burst",
            "paired_yoke_return",
            "attack_settle",
        ),
        (160, 100, 150, 460),
    ),
    "buzzard": FrameSet(
        tuple(f"_action{i}" for i in range(1, 5)),
        (
            "forward_gun_charges",
            "damage+forward_gun_report",
            "forward_gun_recovers",
            "attack_settle",
        ),
        (170, 90, 150, 520),
    ),
    "darter": FrameSet(
        tuple(f"_action{i}" for i in range(1, 5)),
        (
            "forward_needle_arms",
            "damage+forward_needle_report",
            "forward_needle_recovers",
            "attack_settle",
        ),
        (170, 100, 170, 480),
    ),
    "talon": FrameSet(
        tuple(f"_action{i}" for i in range(1, 5)),
        (
            "pursuit_forks_converge",
            "damage+interceptor_cannon_report",
            "pursuit_forks_release",
            "attack_settle",
        ),
        (170, 100, 170, 480),
    ),
    "wisp": FrameSet(
        tuple(f"_action{i}" for i in range(1, 5)),
        (
            "relay_striker_arms",
            "damage+relay_striker_snap",
            "relay_striker_returns",
            "attack_settle",
        ),
        (170, 100, 170, 480),
    ),
}

HARVESTER_ACTIONS = FrameSet(
    ("", "_scoop1", "_scoop2", "_scoop1", ""),
    (
        "ready",
        "pincers_deploy",
        "bucket_advance",
        "bucket_retract",
        "pincers_home",
    ),
    (360, 180, 220, 180, 260),
)

BUILDING_WORK: dict[str, FrameSet] = {
    "foundry": FrameSet(
        ("_work1", "_work2", "_work3", "_work4"),
        (
            "crane_left+light_1+eye_warm",
            "crane_center+light_2+eye_peak",
            "crane_right+light_3+eye_cool",
            "crane_home+light_4+eye_rest",
        ),
        (500, 500, 500, 500),
    ),
    "fabricator": FrameSet(
        ("_work1", "_work2", "_work3", "_work4"),
        ("carriage_left", "tool_press", "carriage_right", "carriage_home"),
        (190, 240, 190, 260),
    ),
    "array": FrameSet(
        tuple(f"_work{i}" for i in range(1, 7)),
        tuple(f"sweep_{i}" for i in range(1, 6)) + ("sweep_home",),
        (180, 180, 180, 180, 180, 260),
    ),
    "reclaimer": FrameSet(
        ("_work1", "_work2", "_work3"),
        ("drum_turn_1", "drum_turn_2", "drum_turn_3"),
        (180, 180, 180),
    ),
    "repair_bay": FrameSet(
        ("_work1", "_work2", "_work3", "_work4"),
        ("arm_unfold", "weld_contact", "arm_recover", "arm_home"),
        (210, 250, 190, 280),
    ),
    "airworks": FrameSet(
        ("_work1", "_work2", "_work3", "_work4"),
        ("systems_sequence", "exit_armed", "doors_opening", "doors_open"),
        (260, 260, 240, 520),
    ),
}

TURRET_ACTIONS = FrameSet(
    tuple(f"_action{i}" for i in range(1, 5)),
    ("damage+muzzle", "recoil", "reload", "ready"),
    (90, 130, 190, 270),
)

DEFENSE_ACTIONS: dict[str, FrameSet] = {
    "turret_barrel": TURRET_ACTIONS,
    "turret_barrel_t1": TURRET_ACTIONS,
    "turret_barrel_t2": TURRET_ACTIONS,
    "flak_mount": FrameSet(
        tuple(f"_action{i}" for i in range(1, 9)),
        (
            "charge_1",
            "charge_2",
            "charge_3",
            "charge_4",
            "muzzle_left",
            "damage+muzzle_right",
            "recovery",
            "ready",
        ),
        (260, 260, 260, 260, 100, 100, 180, 500),
    ),
    "bastion_mount": FrameSet(
        tuple(f"_action{i}" for i in range(1, 10)),
        (
            "charge_1",
            "charge_2",
            "charge_3",
            "charge_4",
            "charge_5",
            "damage+muzzle",
            "recoil",
            "breech_settle",
            "ready",
        ),
        (300, 300, 300, 300, 360, 50, 100, 100, 480),
    ),
}

DEFENSE_BASE_ACTIONS: dict[str, FrameSet] = {
    "bastion": DEFENSE_ACTIONS["bastion_mount"],
}

ACTION_COUNTS = {
    **{stem: len(frames.suffixes) for stem, frames in UNIT_ACTIONS.items()},
    "harvester": len(HARVESTER_ACTIONS.suffixes),
    **{stem: len(frames.suffixes) for stem, frames in BUILDING_WORK.items()},
    **{stem: len(frames.suffixes) for stem, frames in DEFENSE_ACTIONS.items()},
    **{stem: len(frames.suffixes) for stem, frames in DEFENSE_BASE_ACTIONS.items()},
}

WISP_CONTINUOUS_IDLE_SUFFIXES = ("_move1", "_move2")
HARVESTER_CARGO_LEVELS = 5


@contextmanager
def _faction_palette(faction: str) -> Iterator[None]:
    """Render role-colored sources in the requested allegiance palette."""
    if faction not in gen.FACTIONS:
        raise ValueError(f"unknown faction: {faction}")
    saved = {name: palette.copy() for name, palette in gen.FACTIONS.items()}
    chosen = saved[faction]
    try:
        for palette in gen.FACTIONS.values():
            palette.update(chosen)
        yield
    finally:
        for name, palette in gen.FACTIONS.items():
            palette.update(saved[name])


def _put(registry: Registry, out: Path, key: str, image: Image.Image) -> None:
    native = image.convert("RGBA")
    native.save(out / f"{key}.png")
    registry[key] = native


def _mix(
    start: tuple[int, int, int], end: tuple[int, int, int], amount: float
) -> tuple[int, int, int]:
    return tuple(int(left + (right - left) * amount) for left, right in zip(start, end))


def _harvester_tracks(
    draw: ImageDraw.ImageDraw, palette: dict[str, tuple[int, int, int]], phase: int
) -> None:
    for side, (x0, y0, x1, y1) in enumerate(((9, 14, 20, 57), (44, 14, 55, 57))):
        draw.rounded_rectangle(
            [gen.s(x0), gen.s(y0), gen.s(x1), gen.s(y1)],
            radius=gen.s(3),
            fill=(*gen.IRON_DARK, 255),
        )
        belt = _mix(gen.IRON_DARK, gen.IRON, 0.38 + 0.08 * side)
        draw.rounded_rectangle(
            [gen.s(x0 + 2), gen.s(y0 + 2), gen.s(x1 - 2), gen.s(y1 - 2)],
            radius=gen.s(2),
            fill=(*belt, 255),
        )
        span = max(1, round(y1 - y0 - 7))
        for cleat in range(5):
            cy = y0 + 3 + (cleat * 8 + phase * 3) % span
            color = (
                palette["dark"] if (cleat + phase + side) % 3 == 0 else gen.IRON_LIGHT
            )
            draw.rectangle(
                [gen.s(x0 + 2), gen.s(cy), gen.s(x1 - 2), gen.s(min(y1 - 2, cy + 3))],
                fill=(*color, 255),
            )


def _unit_sequences() -> dict[str, SequenceBuilder]:
    return {
        "sentinel": ground_final.sentinel_sequence,
        "scuttler": ground_final.scuttler_sequence,
        "lancer": tier_one_combat_final.lancer_sequence,
        "bombard": tier_one_combat_final.bombard_sequence,
        "flakhound": tier_one_combat_final.flakhound_sequence,
        "stinger": tier_one_combat_final.stinger_sequence,
        "buzzard": air_support_final.buzzard_sequence,
        "darter": air_support_final.darter_sequence,
        "talon": air_support_final.talon_sequence,
        "wisp": air_support_final.wisp_sequence,
    }


def _install_units(registry: Registry, out: Path, faction: str) -> None:
    with _faction_palette(faction):
        for stem, builder in _unit_sequences().items():
            sequence = builder()
            _put(registry, out, f"{stem}_{faction}", sequence.frames[0].image)
            for suffix, frame in zip(
                UNIT_MOVEMENT[stem].suffixes, sequence.frames[1:3], strict=True
            ):
                _put(registry, out, f"{stem}_{faction}{suffix}", frame.image)
            action_frames = sequence.frames[4:]
            for suffix, frame in zip(
                UNIT_ACTIONS[stem].suffixes, action_frames, strict=True
            ):
                _put(registry, out, f"{stem}_{faction}{suffix}", frame.image)


def _harvester_body(faction: str, tread_phase: int, cargo_level: int) -> Image.Image:
    palette = gen.FACTIONS[faction]
    image, draw = gen.canvas(64)
    _harvester_tracks(draw, palette, tread_phase % 3)
    draw.rectangle(
        [gen.s(20), gen.s(13), gen.s(44), gen.s(56)], fill=(*gen.IRON_DARK, 255)
    )
    draw.rectangle([gen.s(24), gen.s(17), gen.s(40), gen.s(52)], fill=(*gen.IRON, 255))
    belt_phase = (0, 5, 10)[tread_phase % 3]
    for y in range(20, 51, 8):
        cy = 20 + (y - 20 + belt_phase) % 31
        draw.rectangle(
            [gen.s(27), gen.s(cy), gen.s(37), gen.s(min(51, cy + 4))],
            fill=(*palette["dark"], 255),
        )
    draw.rounded_rectangle(
        [gen.s(21), gen.s(42), gen.s(43), gen.s(57)],
        radius=gen.s(3),
        fill=(*gen.IRON_DARK, 255),
    )
    draw.rectangle([gen.s(25), gen.s(46), gen.s(39), gen.s(53)], fill=(11, 10, 11, 255))
    fill_width = round(14 * max(0, min(4, cargo_level)) / 4)
    if fill_width:
        draw.rectangle(
            [gen.s(25), gen.s(46), gen.s(25 + fill_width), gen.s(53)],
            fill=(*gen.SCRAP_DARK, 255),
        )
        draw.line(
            [gen.s(25), gen.s(47), gen.s(25 + fill_width), gen.s(47)],
            fill=(*gen.SCRAP_LIGHT, 255),
            width=gen.s(2),
        )
    return gen.rim_light(image.resize((64, 64), Image.Resampling.LANCZOS))


def _harvester_tool(source: Image.Image) -> Image.Image:
    selection = Image.new("L", source.size, 0)
    draw = ImageDraw.Draw(selection)
    draw.polygon(((13, 0), (51, 0), (54, 21), (10, 21)), fill=255)
    draw.rectangle((5, 12, 21, 28), fill=255)
    draw.rectangle((43, 12, 59, 28), fill=255)
    tool = Image.new("RGBA", source.size, (0, 0, 0, 0))
    tool.paste(source, (0, 0), selection)
    return tool


def _install_harvester(registry: Registry, out: Path, faction: str) -> None:
    source = {
        "": _harvester_tool(registry[f"harvester_{faction}"].copy()),
        "_scoop1": _harvester_tool(registry[f"harvester_{faction}_scoop1"].copy()),
        "_scoop2": _harvester_tool(registry[f"harvester_{faction}_scoop2"].copy()),
    }
    for cargo in range(HARVESTER_CARGO_LEVELS):
        cargo_prefix = f"harvester_{faction}_cargo{cargo}"
        idle = Image.alpha_composite(_harvester_body(faction, 0, cargo), source[""])
        _put(registry, out, cargo_prefix, idle)
        for phase in (1, 2):
            moving = Image.alpha_composite(
                _harvester_body(faction, phase, cargo), source[""]
            )
            _put(registry, out, f"{cargo_prefix}_tread{phase}", moving)
        for phase in (1, 2):
            working = Image.alpha_composite(
                _harvester_body(faction, 0, cargo), source[f"_scoop{phase}"]
            )
            _put(registry, out, f"{cargo_prefix}_scoop{phase}", working)
    aliases = {
        f"harvester_{faction}": f"harvester_{faction}_cargo0",
        f"harvester_{faction}_tread1": f"harvester_{faction}_cargo0_tread1",
        f"harvester_{faction}_tread2": f"harvester_{faction}_cargo0_tread2",
        f"harvester_{faction}_scoop1": f"harvester_{faction}_cargo0_scoop1",
        f"harvester_{faction}_scoop2": f"harvester_{faction}_cargo0_scoop2",
    }
    for alias, source_key in aliases.items():
        _put(registry, out, alias, registry[source_key])


def _family_tuned_foundry(source: Image.Image, faction: str) -> Image.Image:
    palette = gen.FACTIONS[faction]
    image = Image.new("RGBA", (128, 128), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)

    def octagon(inset: int, fill: tuple[int, int, int, int]) -> None:
        draw.polygon(
            (
                (inset + 12, inset),
                (128 - inset - 12, inset),
                (128 - inset, inset + 12),
                (128 - inset, 128 - inset - 12),
                (128 - inset - 12, 128 - inset),
                (inset + 12, 128 - inset),
                (inset, 128 - inset - 12),
                (inset, inset + 12),
            ),
            fill=fill,
        )

    octagon(4, (*gen.IRON_DARK, 255))
    octagon(10, (18, 18, 23, 255))
    image.alpha_composite(source)
    draw = ImageDraw.Draw(image)
    for x0, x1, mirror in ((9, 29, False), (99, 119, True)):
        draw.rounded_rectangle((x0, 18, x1, 111), radius=4, fill=(*gen.IRON_DARK, 255))
        draw.rectangle((x0 + 5, 24, x1 - 5, 104), fill=(10, 10, 14, 255))
        if mirror:
            braces = (((x1 - 5, 29), (x0 + 5, 53)), ((x0 + 5, 57), (x1 - 5, 81)))
        else:
            braces = (((x0 + 5, 29), (x1 - 5, 53)), ((x1 - 5, 57), (x0 + 5, 81)))
        for start, end in braces:
            draw.line((start, end), fill=(*palette["dark"], 255), width=4)
        for y in (88, 98):
            draw.rectangle((x0 + 4, y, x1 - 4, y + 6), fill=(*gen.IRON, 255))
    for x, y in ((17, 17), (111, 17), (17, 111), (111, 111)):
        draw.ellipse((x - 3, y - 3, x + 3, y + 3), fill=(*gen.IRON_LIGHT, 255))
        draw.rectangle((x - 1, y - 1, x + 1, y + 1), fill=(*gen.BONE, 220))
    return image


def _octagonal_repair_frame(source: Image.Image, faction: str) -> Image.Image:
    palette = gen.FACTIONS[faction]
    mask = Image.new("L", source.size, 0)
    ImageDraw.Draw(mask).polygon(
        (
            (22, 7),
            (106, 7),
            (121, 22),
            (121, 106),
            (107, 121),
            (21, 121),
            (7, 107),
            (7, 22),
        ),
        fill=255,
    )
    clipped = source.copy()
    clipped.putalpha(
        Image.composite(source.getchannel("A"), Image.new("L", source.size, 0), mask)
    )
    draw = ImageDraw.Draw(clipped)
    edge = (
        (22, 8),
        (106, 8),
        (120, 22),
        (120, 106),
        (106, 120),
        (22, 120),
        (8, 106),
        (8, 22),
        (22, 8),
    )
    draw.line(edge, fill=(*gen.IRON_DARK, 255), width=6, joint="curve")
    for start, end in (
        ((22, 10), (10, 22)),
        ((106, 10), (118, 22)),
        ((118, 106), (106, 118)),
        ((22, 118), (10, 106)),
    ):
        draw.line((start, end), fill=(*palette["dark"], 255), width=4)
    return clipped


def _install_working_buildings(registry: Registry, out: Path, faction: str) -> None:
    with _faction_palette(faction):
        fabricator = air_support_final.fabricator_frames()
        _put(registry, out, f"fabricator_{faction}", fabricator[0].image)
        for suffix, frame in zip(
            BUILDING_WORK["fabricator"].suffixes, fabricator[1:], strict=True
        ):
            _put(registry, out, f"fabricator_{faction}{suffix}", frame.image)

        repair = tuple(
            _octagonal_repair_frame(frame.image, faction)
            for frame in structures_base.repair_bay_frames()
        )
        _put(registry, out, f"repair_bay_{faction}", repair[0])
        for suffix, image in zip(
            BUILDING_WORK["repair_bay"].suffixes, repair[1:], strict=True
        ):
            _put(registry, out, f"repair_bay_{faction}{suffix}", image)

        headings = (225, 285, 345, 405, 465, 525, 585)
        array_frames = tuple(
            structures_base._array_sprite(heading=heading) for heading in headings
        )
        _put(registry, out, f"array_{faction}", array_frames[0])
        for suffix, image in zip(
            BUILDING_WORK["array"].suffixes, array_frames[1:], strict=True
        ):
            _put(registry, out, f"array_{faction}{suffix}", image)

        reclaimer_frames = tuple(
            economy_mechanisms.render_reclaimer(phase) for phase in range(4)
        )
        _put(registry, out, f"reclaimer_{faction}", reclaimer_frames[0])
        for suffix, image in zip(
            BUILDING_WORK["reclaimer"].suffixes, reclaimer_frames[1:], strict=True
        ):
            _put(registry, out, f"reclaimer_{faction}{suffix}", image)

    sources = [heavy_structures.foundry_frame(faction, work) for work in range(4)]
    sources.append(sources[0])
    tuned = tuple(_family_tuned_foundry(source, faction) for source in sources)
    _put(registry, out, f"foundry_{faction}", tuned[0])
    for suffix, image in zip(BUILDING_WORK["foundry"].suffixes, tuned[1:], strict=True):
        _put(registry, out, f"foundry_{faction}{suffix}", image)


def _flak_base_and_mount(
    faction: str, *, phase: int, charge: int
) -> tuple[Image.Image, Image.Image]:
    palette = gen.FACTIONS[faction]
    base, draw = gen.canvas(64)
    draw.ellipse(
        [gen.s(10), gen.s(10), gen.s(54), gen.s(54)],
        outline=(*gen.IRON_DARK, 255),
        width=gen.s(7),
    )
    draw.ellipse(
        [gen.s(19), gen.s(19), gen.s(45), gen.s(45)], fill=(*palette["dark"], 255)
    )
    mount, mount_draw = gen.canvas(64)
    for x0, x1 in ((7, 30), (34, 57)):
        mount_draw.rounded_rectangle(
            [gen.s(x0), gen.s(23), gen.s(x1), gen.s(45)],
            radius=gen.s(5),
            fill=(*gen.IRON_DARK, 255),
        )
        mount_draw.rectangle(
            [gen.s(x0 + 4), gen.s(28), gen.s(x1 - 4), gen.s(39)],
            fill=(*palette["dark"], 255),
        )
    left_recoil, right_recoil, left_flash, right_flash = defense_mechanisms._flak_cycle(
        phase
    )
    defense_mechanisms._flak_barrel_pair(
        mount_draw, xs=(17, 25), recoil=left_recoil, flash=left_flash
    )
    defense_mechanisms._flak_barrel_pair(
        mount_draw, xs=(39, 47), recoil=right_recoil, flash=right_flash
    )
    defense_mechanisms._four_cell_feed(mount_draw, charge, horizontal=True)
    return (
        gen.rim_light(base.resize((64, 64), Image.Resampling.LANCZOS)),
        gen.rim_light(mount.resize((64, 64), Image.Resampling.LANCZOS)),
    )


def _install_defenses(registry: Registry, out: Path, faction: str) -> None:
    for tier, (base_stem, mount_stem) in enumerate(
        (
            ("turret", "turret_barrel"),
            ("turret_t1", "turret_barrel_t1"),
            ("turret_t2", "turret_barrel_t2"),
        )
    ):
        _put(
            registry,
            out,
            f"{base_stem}_{faction}",
            turret_family.turret_base(faction, tier),
        )
        mounts = tuple(
            turret_family.turret_mount(faction, tier, phase) for phase in range(5)
        )
        _put(registry, out, f"{mount_stem}_{faction}", mounts[0])
        for suffix, mount in zip(
            DEFENSE_ACTIONS[mount_stem].suffixes, mounts[1:], strict=True
        ):
            _put(registry, out, f"{mount_stem}_{faction}{suffix}", mount)

    flak_specs = (
        (0, 0),
        (0, 1),
        (0, 2),
        (0, 3),
        (0, 4),
        (1, 0),
        (2, 0),
        (3, 0),
        (0, 0),
    )
    flak_frames = tuple(
        _flak_base_and_mount(faction, phase=phase, charge=charge)
        for phase, charge in flak_specs
    )
    _put(registry, out, f"flak_turret_{faction}", flak_frames[0][0])
    _put(registry, out, f"flak_mount_{faction}", flak_frames[0][1])
    for suffix, (_, mount) in zip(
        DEFENSE_ACTIONS["flak_mount"].suffixes, flak_frames[1:], strict=True
    ):
        _put(registry, out, f"flak_mount_{faction}{suffix}", mount)

    # The base frame is a ready weapon, so its rack is full. Report holds
    # full through the muzzle frame; recovery empties it before the next
    # cooldown advances action1..action5 from one cell back to five.
    charges = (5, 1, 2, 3, 4, 5, 5, 0, 0, 0)
    bastion_frames = tuple(
        (
            heavy_structures.bastion_base(faction, charge),
            heavy_structures.bastion_mount(faction, index),
        )
        for index, charge in enumerate(charges)
    )
    _put(registry, out, f"bastion_{faction}", bastion_frames[0][0])
    _put(registry, out, f"bastion_mount_{faction}", bastion_frames[0][1])
    for suffix, (base, mount) in zip(
        DEFENSE_ACTIONS["bastion_mount"].suffixes, bastion_frames[1:], strict=True
    ):
        _put(registry, out, f"bastion_{faction}{suffix}", base)
        _put(registry, out, f"bastion_mount_{faction}{suffix}", mount)


def install_finalized_sprites(registry: Registry, out: Path) -> None:
    """Replace machine rows with the finalized native art and action frames.

    Call this after the legacy base generator has populated ``registry`` and
    before construction frames, allegiance masks, and atlas packing. Earlier
    family passes may use legacy pixels as ancestry, while later focused banks
    replace their complete rows. No external presentation asset is read.
    """
    out.mkdir(parents=True, exist_ok=True)
    for faction in ("ferrous", "cupric"):
        _install_harvester(registry, out, faction)
        _install_units(registry, out, faction)
        _install_working_buildings(registry, out, faction)
        _install_defenses(registry, out, faction)
    tender_condor_final.install_tender_condor(registry, out)
    crucible_final.install_crucible_units(registry, out)
    moth_warden_final.install_moth_warden(registry, out)
    shrike_sylph_final.install_shrike_sylph(registry, out)
    excavator_final.install_excavator(registry, out)
    skyhook_sapper_crucible_final.install_skyhook_sapper_crucible(registry, out)
    airworks_scouts_final.install_airworks_scouts(registry, out)
    extractor_reclaimer_final.install_extractor_reclaimer(registry, out)
    core_unit_art_final.install_core_unit_art(registry, out)
    flak_array_final.install_flak_array(registry, out)
    field_structures_final.install_field_structures(registry, out)
