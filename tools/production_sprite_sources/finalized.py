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
    air_final,
    defense_mechanisms,
    economy_mechanisms,
    ground_artillery,
    ground_base,
    ground_final,
    heavy_structures,
    lancer_final,
    structures_base,
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
        ("internal_propulsion_a", "internal_propulsion_b"),
        (150, 150),
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
        tuple(f"_action{i}" for i in range(1, 6)),
        (
            "charge_bar_ready",
            "report_left_yoke",
            "damage+report_right_yoke",
            "paired_yokes_recover",
            "attack_settle",
        ),
        (180, 100, 110, 180, 500),
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
            "belly_hopper_opens",
            "damage+payload_ram_drop",
            "belly_hopper_closes",
            "attack_settle",
        ),
        (170, 100, 170, 480),
    ),
    "darter": FrameSet(
        tuple(f"_action{i}" for i in range(1, 5)),
        (
            "shear_wings_close",
            "damage+shear_strike",
            "shear_wings_reopen",
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
    ("ready", "bite_lower", "bite", "pull", "scoop_complete"),
    (360, 180, 220, 180, 260),
)

BUILDING_WORK: dict[str, FrameSet] = {
    "foundry": FrameSet(
        ("_work1", "_work2", "_work3", "_work4"),
        ("gantry_lower", "transfer_contact", "gantry_raise", "gantry_home"),
        (190, 240, 190, 260),
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
}

DEFENSE_ACTIONS: dict[str, FrameSet] = {
    "turret_barrel": FrameSet(
        tuple(f"_action{i}" for i in range(1, 5)),
        ("damage+muzzle", "recoil", "reload", "ready"),
        (90, 130, 190, 270),
    ),
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
        (300, 300, 300, 300, 360, 100, 170, 220, 480),
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
        "lancer": lancer_final.lancer_sequence,
        "bombard": ground_artillery.bombard_sequence,
        "flakhound": ground_base.flakhound_sequence,
        "stinger": ground_final.stinger_sequence,
        "buzzard": air_final.buzzard_sequence,
        "darter": air_final.darter_sequence,
        "talon": air_final.talon_sequence,
        "wisp": air_final.wisp_sequence,
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
        fabricator = structures_base.fabricator_frames()
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


def _turret_base_and_mount(
    faction: str, *, recoil: int, muzzle: bool, breech_open: bool, feed_phase: int
) -> tuple[Image.Image, Image.Image]:
    palette = gen.FACTIONS[faction]
    base, base_draw = structures_base._new_sprite((64, 64))
    for x, y in ((12, 25), (52, 25), (12, 51), (52, 51)):
        structures_base._beveled_plate(
            base_draw, (x - 5, y - 4, x + 5, y + 4), radius=2
        )
    base_draw.ellipse(structures_base._box((8, 13, 56, 61)), fill=(*gen.IRON_DARK, 255))
    base_draw.ellipse(
        structures_base._box((13, 18, 51, 56)), fill=(*gen.IRON_LIGHT, 255)
    )
    base_draw.ellipse(structures_base._box((18, 23, 46, 51)), fill=(0, 0, 0, 0))
    base_draw.arc(
        structures_base._box((11, 16, 53, 58)),
        205,
        335,
        fill=(*palette["base"], 255),
        width=structures_base._s(4),
    )
    for x, y in ((16, 24), (48, 24), (18, 50), (46, 50)):
        structures_base._bolt(base_draw, x, y, 1.5)

    mount, draw = structures_base._new_sprite((64, 64))
    structures_base._beveled_plate(
        draw,
        (22, 28 + recoil, 42, 48 + recoil),
        fill=palette["dark"],
        edge=gen.IRON_DARK,
        highlight=palette["light"],
        radius=4,
    )
    draw.rectangle(
        structures_base._box((27, 23 + recoil, 37, 37 + recoil)),
        fill=(*gen.IRON_DARK, 255),
    )
    draw.rectangle(
        structures_base._box((29, 24 + recoil, 35, 35 + recoil)),
        fill=(*(palette["base"] if breech_open else gen.IRON_LIGHT), 255),
    )
    draw.rectangle(
        structures_base._box((27, 31 + recoil, 37, 35 + recoil)),
        fill=(*gen.IRON_DARK, 255),
    )
    draw.rounded_rectangle(
        structures_base._box((27, 5 + recoil, 37, 30 + recoil)),
        radius=structures_base._s(2),
        fill=(*gen.IRON_DARK, 255),
    )
    draw.rectangle(
        structures_base._box((30, 6 + recoil, 34, 27 + recoil)),
        fill=(*gen.IRON_LIGHT, 255),
    )
    draw.rectangle(
        structures_base._box((27, 5 + recoil, 37, 10 + recoil)),
        fill=(*palette["dark"], 255),
    )
    draw.rectangle(
        structures_base._box((29, 5 + recoil, 35, 7 + recoil)), fill=(12, 12, 15, 255)
    )
    cartridges = ((45, 43), (48, 38), (49, 32), (46, 27))
    draw.line(
        structures_base._points(((40, 46), (46, 43), (49, 37), (49, 31), (44, 26))),
        fill=(*gen.IRON_DARK, 255),
        width=structures_base._s(5),
    )
    for index, (x, y) in enumerate(cartridges):
        color = (
            gen.SCRAP_LIGHT
            if (index + feed_phase) % len(cartridges) == 0
            else gen.SCRAP_DARK
        )
        draw.rounded_rectangle(
            structures_base._box((x - 3, y - 2, x + 3, y + 2)),
            radius=structures_base._s(1),
            fill=(*color, 255),
        )
        draw.rectangle(
            structures_base._box((x + 1, y - 2, x + 3, y + 2)), fill=(*gen.BONE, 210)
        )
    if muzzle:
        draw.polygon(
            structures_base._points(
                (
                    (32, 2),
                    (36, 5),
                    (41, 7),
                    (36, 9),
                    (32, 13),
                    (28, 9),
                    (23, 7),
                    (28, 5),
                )
            ),
            fill=(*gen.SCRAP_LIGHT, 255),
        )
        draw.rectangle(structures_base._box((29, 4, 35, 9)), fill=(*gen.BONE, 255))
    return (
        structures_base._finish(base, (64, 64)),
        structures_base._finish(mount, (64, 64)),
    )


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


def _bastion_base(source: Image.Image, faction: str, charge: int) -> Image.Image:
    palette = gen.FACTIONS[faction]
    image = Image.new("RGBA", (128, 128), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    draw.ellipse((5, 5, 123, 123), fill=(17, 17, 22, 255))
    for x, y in ((13, 28), (115, 28), (13, 100), (115, 100)):
        draw.rounded_rectangle(
            (x - 8, y - 5, x + 8, y + 5), radius=2, fill=(*gen.IRON_DARK, 255)
        )
        draw.line((x - 4, y, x + 4, y), fill=(*palette["dark"], 255), width=2)
    for polygon in (
        ((27, 35), (8, 11), (2, 20), (23, 45)),
        ((101, 35), (120, 11), (126, 20), (105, 45)),
        ((27, 93), (8, 117), (2, 108), (23, 83)),
        ((101, 93), (120, 117), (126, 108), (105, 83)),
    ):
        draw.polygon(polygon, fill=(*gen.IRON_DARK, 255))
    image.alpha_composite(source)
    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle((11, 51, 27, 105), radius=3, fill=(*gen.IRON_DARK, 255))
    draw.rectangle((14, 54, 24, 102), fill=(9, 9, 12, 255))
    for index in range(5):
        y0 = 91 - index * 9
        color = gen.SCRAP_LIGHT if index < charge else gen.SCRAP_DARK
        draw.rounded_rectangle((16, y0, 22, y0 + 6), radius=1, fill=(*color, 255))
        if index < charge:
            draw.line((17, y0 + 1, 21, y0 + 1), fill=(*gen.SCRAP, 255), width=1)
    return image


def _shifted_mount(source: Image.Image, recoil: int, muzzle: bool) -> Image.Image:
    moving_mask = Image.new("L", source.size, 0)
    moving_draw = ImageDraw.Draw(moving_mask)
    moving_draw.rectangle((48, 0, 80, 72), fill=255)
    moving_draw.rectangle((31, 53, 97, 85), fill=255)

    moving = Image.new("RGBA", source.size, (0, 0, 0, 0))
    moving.paste(source, (0, 0), moving_mask)
    fixed = source.copy()
    fixed.paste((0, 0, 0, 0), (0, 0, source.width, source.height), moving_mask)

    image = fixed
    image.alpha_composite(moving, (0, recoil))
    if muzzle:
        draw = ImageDraw.Draw(image)
        draw.polygon(
            (
                (64, 2 + recoil),
                (69, 7 + recoil),
                (76, 10 + recoil),
                (69, 13 + recoil),
                (64, 20 + recoil),
                (59, 13 + recoil),
                (52, 10 + recoil),
                (59, 7 + recoil),
            ),
            fill=(*gen.SCRAP_LIGHT, 255),
        )
        draw.rectangle((61, 7 + recoil, 67, 13 + recoil), fill=(*gen.BONE, 255))
    return image


def _install_defenses(registry: Registry, out: Path, faction: str) -> None:
    turret_specs = (
        (0, False, False, 0),
        (2, True, False, 0),
        (5, False, True, 0),
        (3, False, True, 1),
        (0, False, False, 2),
    )
    turret_frames = tuple(
        _turret_base_and_mount(
            faction, recoil=recoil, muzzle=muzzle, breech_open=breech, feed_phase=feed
        )
        for recoil, muzzle, breech, feed in turret_specs
    )
    _put(registry, out, f"turret_{faction}", turret_frames[0][0])
    _put(registry, out, f"turret_barrel_{faction}", turret_frames[0][1])
    for suffix, (_, mount) in zip(
        DEFENSE_ACTIONS["turret_barrel"].suffixes, turret_frames[1:], strict=True
    ):
        _put(registry, out, f"turret_barrel_{faction}{suffix}", mount)

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

    base_source = heavy_structures.bastion_base(faction)
    mount_source = heavy_structures.bastion_mount(faction)
    charges = (0, 1, 2, 3, 4, 5, 5, 0, 0, 0)
    recoils = (0, 0, 0, 0, 0, 0, 2, 8, 4, 0)
    bastion_frames = tuple(
        (
            _bastion_base(base_source, faction, charge),
            _shifted_mount(mount_source, recoil, index == 6),
        )
        for index, (charge, recoil) in enumerate(zip(charges, recoils, strict=True))
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
    before construction frames, allegiance masks, and atlas packing.  The
    Harvester deliberately reads the existing approved claw pixels before
    replacing its chassis; no external presentation asset is read.
    """
    out.mkdir(parents=True, exist_ok=True)
    for faction in ("ferrous", "cupric"):
        _install_harvester(registry, out, faction)
        _install_units(registry, out, faction)
        _install_working_buildings(registry, out, faction)
        _install_defenses(registry, out, faction)
