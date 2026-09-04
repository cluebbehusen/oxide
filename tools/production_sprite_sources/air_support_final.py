"""Approved Darter, Buzzard, Talon, Wisp, and Fabricator art."""

from __future__ import annotations

import hashlib
from collections.abc import Callable
from dataclasses import replace

from PIL import Image, ImageChops, ImageDraw

from tools.gen_sprites import (
    FACTIONS,
    IRON,
    IRON_DARK,
    IRON_LIGHT,
    SCRAP,
    SCRAP_DARK,
    SCRAP_LIGHT,
    rim_light,
)
from tools.production_sprite_sources import air_final, structures_base
from tools.production_sprite_sources.ground_base import (
    GroundUnitFrame,
    GroundUnitSequence,
)

FERROUS = FACTIONS["ferrous"]
CUPRIC = FACTIONS["cupric"]
APPROVED_SOURCE_RGBA_SHA256 = (
    "636200c22ed9904836b411eb9a7435a8344a492616bd61120ef9649eaa29d7b7"
)
_EXPORTED_SEQUENCE_INDICES = (0, 1, 2, 4, 5, 6, 7)
_REVIEW_FRAME_INDICES = (0, 1, 2, 0, 3, 4, 5, 6)


def _rgba(color: tuple[int, int, int], alpha: int = 255) -> tuple[int, int, int, int]:
    return (*color, alpha)


def _clipped_overlay(
    source: Image.Image,
) -> tuple[Image.Image, ImageDraw.ImageDraw]:
    overlay = Image.new("RGBA", source.size, (0, 0, 0, 0))
    return overlay, ImageDraw.Draw(overlay)


def _finish_overlay(source: Image.Image, overlay: Image.Image) -> Image.Image:
    overlay.putalpha(
        ImageChops.multiply(overlay.getchannel("A"), source.getchannel("A"))
    )
    result = source.copy()
    result.alpha_composite(overlay)
    result.putalpha(source.getchannel("A"))
    return result


def _line(
    draw: ImageDraw.ImageDraw,
    points: tuple[tuple[int, int], ...],
    color: tuple[int, int, int],
    width: int = 1,
) -> None:
    draw.line(points, fill=_rgba(color), width=width, joint="curve")


def _rivet(
    draw: ImageDraw.ImageDraw,
    x: int,
    y: int,
    color: tuple[int, int, int] = IRON_LIGHT,
) -> None:
    draw.point((x, y), fill=_rgba(color))


def _state_for(index: int) -> str:
    return {3: "ready", 4: "attack", 5: "recover"}.get(index, "idle")


def _phase_for(index: int) -> int:
    return {1: 1, 2: 2}.get(index, 0)


def _state_light(
    state: str, accent: tuple[int, int, int]
) -> tuple[int, int, int]:
    return {
        "idle": accent,
        "ready": SCRAP,
        "attack": SCRAP_LIGHT,
        "recover": IRON_LIGHT,
    }[state]


def _armored_buzzard(
    source: Image.Image, review_index: int
) -> Image.Image:
    overlay, draw = _clipped_overlay(source)
    state = _state_for(review_index)
    accent = FERROUS["light"]
    for cx, cy in ((13, 22), (51, 22), (13, 48), (51, 48)):
        draw.arc(
            (cx - 7, cy - 7, cx + 7, cy + 7),
            210,
            330,
            fill=_rgba(accent),
            width=1,
        )
        _rivet(draw, cx - 6, cy)
        _rivet(draw, cx + 6, cy)
    draw.rounded_rectangle(
        (26, 19, 38, 31), radius=2, outline=_rgba(IRON_LIGHT), width=1
    )
    draw.rectangle((27, 35, 29, 49), fill=_rgba(FERROUS["dark"]))
    draw.rectangle((35, 35, 37, 49), fill=_rgba(FERROUS["dark"]))
    for y in (37, 42, 47):
        _rivet(draw, 28, y, IRON_LIGHT)
        _rivet(draw, 36, y, IRON_LIGHT)
    draw.rectangle(
        (30, 29, 34, 35),
        fill=_rgba(IRON_DARK),
        outline=_rgba(_state_light(state, accent)),
    )
    return _finish_overlay(source, overlay)


def _shrouded_wisp(
    source: Image.Image, review_index: int
) -> Image.Image:
    overlay, draw = _clipped_overlay(source)
    state = _state_for(review_index)
    phase = _phase_for(review_index)
    accent = CUPRIC["light"]
    for rotor_index, (cx, cy) in enumerate(
        ((20, 18), (44, 18), (20, 42), (44, 42))
    ):
        start = (phase * 75 + rotor_index * 90) % 360
        draw.arc(
            (cx - 5, cy - 5, cx + 5, cy + 5),
            start,
            start + 85,
            fill=_rgba(accent),
            width=1,
        )
        _rivet(draw, cx, cy, IRON_LIGHT)
    draw.rounded_rectangle(
        (27, 25, 37, 41), radius=2, outline=_rgba(IRON_LIGHT), width=1
    )
    draw.rectangle((29, 29, 35, 32), fill=_rgba(IRON_DARK))
    draw.rectangle(
        (31, 30, 33, 31), fill=_rgba(_state_light(state, accent))
    )
    return _finish_overlay(source, overlay)


def _armored_talon(
    source: Image.Image, review_index: int
) -> Image.Image:
    overlay, draw = _clipped_overlay(source)
    state = _state_for(review_index)
    phase = _phase_for(review_index)
    accent = FERROUS["light"]
    _line(draw, ((12, 31), (22, 27), (26, 35)), IRON_LIGHT)
    _line(draw, ((52, 31), (42, 27), (38, 35)), IRON_LIGHT)
    for x, y in ((13, 37), (20, 42), (51, 37), (44, 42)):
        _rivet(draw, x, y)
    draw.rounded_rectangle(
        (27, 24, 37, 38), radius=2, outline=_rgba(IRON_LIGHT), width=1
    )
    draw.rectangle(
        (29, 29, 35, 34),
        fill=_rgba(IRON_DARK),
        outline=_rgba(_state_light(state, accent)),
    )
    for y in (42, 46, 50):
        color = FERROUS["dark"] if y // 4 % 3 != phase else accent
        draw.rectangle((29, y, 35, y + 1), fill=_rgba(color))
    return _finish_overlay(source, overlay)


def _detail_sequence(
    sequence: GroundUnitSequence,
    renderer: Callable[[Image.Image, int], Image.Image],
    *,
    title: str,
    mechanism: str,
) -> GroundUnitSequence:
    frames = tuple(
        replace(frame, image=renderer(frame.image, review_index))
        for frame, review_index in zip(
            sequence.frames, _REVIEW_FRAME_INDICES, strict=True
        )
    )
    return replace(sequence, title=title, mechanism=mechanism, frames=frames)


def buzzard_sequence() -> GroundUnitSequence:
    return _detail_sequence(
        air_final.buzzard_sequence(),
        _armored_buzzard,
        title="Buzzard / Armored Quad-Fan Carriage",
        mechanism="four indexed lift fans braced around one armored forward gun",
    )


def wisp_sequence() -> GroundUnitSequence:
    return _detail_sequence(
        air_final.wisp_sequence(),
        _shrouded_wisp,
        title="Wisp / Shrouded Four-Rotor Relay",
        mechanism="four shrouded rotor pods feeding one protected pursuit striker",
    )


def talon_sequence() -> GroundUnitSequence:
    return _detail_sequence(
        air_final.talon_sequence(),
        _armored_talon,
        title="Talon / Armored Horizontal Interceptor",
        mechanism="broad engine wing bracing one armored anti-air cannon",
    )


def _mix(
    start: tuple[int, int, int],
    end: tuple[int, int, int],
    numerator: int,
    denominator: int = 4,
) -> tuple[int, int, int]:
    return tuple(
        (left * (denominator - numerator) + right * numerator) // denominator
        for left, right in zip(start, end, strict=True)
    )


def _polygon(
    draw: ImageDraw.ImageDraw,
    points: list[tuple[int, int]],
    fill: tuple[int, int, int],
    edge: tuple[int, int, int] = IRON_DARK,
    width: int = 2,
) -> None:
    draw.polygon(points, fill=_rgba(fill), outline=_rgba(edge), width=width)


def _plate(
    draw: ImageDraw.ImageDraw,
    box: tuple[int, int, int, int],
    fill: tuple[int, int, int] = IRON,
    edge: tuple[int, int, int] = IRON_DARK,
    radius: int = 3,
) -> None:
    draw.rounded_rectangle(box, radius=radius, fill=_rgba(edge))
    x0, y0, x1, y1 = box
    draw.rounded_rectangle(
        (x0 + 2, y0 + 2, x1 - 2, y1 - 2),
        radius=max(1, radius - 1),
        fill=_rgba(fill),
    )


def _light(
    draw: ImageDraw.ImageDraw,
    position: tuple[int, int],
    color: tuple[int, int, int],
    hot: bool = False,
) -> None:
    x, y = position
    draw.rectangle((x - 2, y - 2, x + 2, y + 2), fill=_rgba(IRON_DARK))
    draw.rectangle(
        (x - 1, y - 1, x + 1, y + 1),
        fill=_rgba(SCRAP_LIGHT if hot else color),
    )


def _muzzle(draw: ImageDraw.ImageDraw, x: int, y: int, state: str) -> None:
    if state != "fire":
        return
    _polygon(
        draw,
        [(x, y - 4), (x - 3, y - 1), (x, y), (x + 3, y - 1)],
        SCRAP_LIGHT,
        SCRAP_DARK,
        1,
    )
    draw.rectangle((x - 1, y - 3, x + 1, y), fill=(255, 244, 188, 255))


def _weapon(
    draw: ImageDraw.ImageDraw,
    x: int,
    y0: int,
    y1: int,
    state: str,
    accent: tuple[int, int, int],
) -> None:
    recoil = 3 if state == "fire" else 1 if state == "recover" else 0
    hot = state in {"ready", "fire"}
    draw.rectangle((x - 2, y0 + recoil, x + 2, y1 + recoil), fill=_rgba(IRON_DARK))
    draw.rectangle(
        (x - 1, y0 + recoil, x + 1, y1 + recoil),
        fill=_rgba(IRON_LIGHT if not hot else SCRAP_LIGHT),
    )
    _muzzle(draw, x, y0 + recoil, state)
    _light(draw, (x, y1 + 3), accent, hot)


def _darter_needle(phase: int, state: str) -> Image.Image:
    image = Image.new("RGBA", (64, 64), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    accent = CUPRIC
    _polygon(
        draw,
        [
            (32, 5),
            (39, 22),
            (56, 43),
            (41, 40),
            (37, 57),
            (27, 57),
            (23, 40),
            (8, 43),
            (25, 22),
        ],
        _mix(accent["base"], IRON, 2),
    )
    _polygon(
        draw,
        [(27, 15), (32, 8), (37, 15), (36, 48), (28, 48)],
        accent["base"],
    )
    _plate(draw, (16, 37, 25, 53), accent["dark"])
    _plate(draw, (39, 37, 48, 53), accent["dark"])
    for x, offset in ((20, 0), (44, 1)):
        for vent in range(3):
            color = (
                accent["light"]
                if vent == (phase + offset) % 3
                else accent["dark"]
            )
            draw.rectangle(
                (x - 2, 42 + vent * 3, x + 2, 43 + vent * 3),
                fill=_rgba(color),
            )
    _weapon(draw, 32, 3, 26, state, accent["light"])
    return rim_light(image)


def darter_sequence() -> GroundUnitSequence:
    frames = (
        GroundUnitFrame(_darter_needle(0, "idle"), 420, "idle", "idle"),
        GroundUnitFrame(
            _darter_needle(1, "idle"),
            150,
            "locomotion",
            "internal_propulsion_a",
        ),
        GroundUnitFrame(
            _darter_needle(2, "idle"),
            150,
            "locomotion",
            "internal_propulsion_b",
        ),
        GroundUnitFrame(
            _darter_needle(0, "idle"), 220, "settle", "motion_settle"
        ),
        GroundUnitFrame(
            _darter_needle(0, "ready"),
            170,
            "anticipation",
            "forward_needle_arms",
        ),
        GroundUnitFrame(
            _darter_needle(0, "fire"),
            100,
            "attack",
            "damage+forward_needle_report",
            logical_damage=True,
            report_count=1,
            recoil_px=3,
        ),
        GroundUnitFrame(
            _darter_needle(0, "recover"),
            170,
            "recovery",
            "forward_needle_recovers",
            recoil_px=1,
        ),
        GroundUnitFrame(
            _darter_needle(0, "idle"), 480, "settle", "attack_settle"
        ),
    )
    return GroundUnitSequence(
        stem="darter_needle_strike_skiff",
        title="Darter / Needle Strafe Skiff",
        mechanism="sequenced engine pods driving one narrow forward striker",
        mechanism_box=(8, 3, 56, 57),
        attack_contract="one physical forward-needle report and one logical damage event",
        frames=frames,
    )


def _native_plate(
    draw: ImageDraw.ImageDraw,
    box: tuple[int, int, int, int],
    fill: tuple[int, int, int],
    radius: int = 2,
) -> None:
    draw.rounded_rectangle(box, radius=radius, fill=_rgba(IRON_DARK))
    x0, y0, x1, y1 = box
    draw.rounded_rectangle(
        (x0 + 2, y0 + 2, x1 - 2, y1 - 2),
        radius=max(1, radius - 1),
        fill=_rgba(fill),
    )


def _fabricator_detail(source: Image.Image, frame: int) -> Image.Image:
    image = source.copy()
    draw = ImageDraw.Draw(image)
    accent = FERROUS
    phase = max(0, frame - 1)
    for x in range(34, 99, 10):
        color = IRON_LIGHT if x // 10 % 4 == phase else IRON_DARK
        draw.rectangle((x, 22, x + 2, 24), fill=_rgba(color))
    _native_plate(draw, (16, 43, 27, 66), _mix(accent["base"], IRON, 2))
    _native_plate(draw, (101, 43, 112, 66), _mix(accent["base"], IRON, 2))
    draw.ellipse(
        (18, 48, 25, 55), fill=_rgba(SCRAP_DARK), outline=_rgba(accent["light"])
    )
    _line(draw, ((22, 54), (31, 62), (31, 83)), accent["light"], 2)
    for x in (20, 108):
        _light(
            draw,
            (x, 31),
            accent["light"],
            frame > 0 and phase % 2 == (0 if x < 50 else 1),
        )
    return image


def fabricator_frames() -> tuple[structures_base.StructureFrame, ...]:
    return tuple(
        replace(
            frame,
            image=_fabricator_detail(frame.image, index),
            source="approved:riveted-rail-fabricator",
        )
        for index, frame in enumerate(structures_base.fabricator_frames())
    )


def source_rgba_digest() -> str:
    digest = hashlib.sha256()
    for stem, sequence in (
        ("buzzard", buzzard_sequence()),
        ("darter", darter_sequence()),
        ("talon", talon_sequence()),
        ("wisp", wisp_sequence()),
    ):
        for index in _EXPORTED_SEQUENCE_INDICES:
            digest.update(stem.encode())
            digest.update(sequence.frames[index].image.convert("RGBA").tobytes())
    for frame in fabricator_frames():
        digest.update(b"fabricator")
        digest.update(frame.image.convert("RGBA").tobytes())
    return digest.hexdigest()
