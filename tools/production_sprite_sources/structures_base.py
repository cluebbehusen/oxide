"""Native production mechanisms for finalized structures."""

from __future__ import annotations

import math
from dataclasses import dataclass
from typing import Literal

from PIL import Image, ImageDraw

from tools.gen_sprites import (
    BONE,
    FACTIONS,
    IRON,
    IRON_DARK,
    IRON_LIGHT,
    SCRAP,
    SCRAP_LIGHT,
)

SS = 4
FERROUS = FACTIONS["ferrous"]
NATIVE_SIZES = {
    "fabricator": (128, 128),
    "repair_bay": (128, 128),
    "array": (64, 64),
    "turret": (64, 64),
}
FrameState = Literal["idle", "working", "relaying", "firing", "reloading"]


@dataclass(frozen=True)
class StructureFrame:
    image: Image.Image
    duration_ms: int
    event: str
    state: FrameState
    active: bool
    source: str
    logical_beat: str | None = None
    mechanism_anchor: tuple[int, int] | None = None


def _s(value: float) -> int:
    return round(value * SS)


def _box(values: tuple[float, float, float, float]) -> tuple[int, int, int, int]:
    x0, y0, x1, y1 = values
    return (_s(x0), _s(y0), _s(x1), _s(y1))


def _points(values: tuple[tuple[float, float], ...]) -> tuple[tuple[int, int], ...]:
    return tuple(((_s(x), _s(y)) for x, y in values))


def _new_sprite(size: tuple[int, int]) -> tuple[Image.Image, ImageDraw.ImageDraw]:
    image = Image.new("RGBA", (size[0] * SS, size[1] * SS), (0, 0, 0, 0))
    return (image, ImageDraw.Draw(image))


def _finish(image: Image.Image, size: tuple[int, int]) -> Image.Image:
    return image.resize(size, Image.Resampling.LANCZOS)


def _beveled_plate(
    draw: ImageDraw.ImageDraw,
    bounds: tuple[float, float, float, float],
    *,
    fill: tuple[int, int, int] = IRON,
    edge: tuple[int, int, int] = IRON_DARK,
    highlight: tuple[int, int, int] = IRON_LIGHT,
    radius: float = 3,
) -> None:
    x0, y0, x1, y1 = bounds
    draw.rounded_rectangle(_box(bounds), radius=_s(radius), fill=(*edge, 255))
    draw.rounded_rectangle(
        _box((x0 + 2, y0 + 2, x1 - 2, y1 - 2)),
        radius=_s(max(1, radius - 1)),
        fill=(*fill, 255),
    )
    draw.line(
        _points(((x0 + 4, y0 + 3), (x1 - 4, y0 + 3))),
        fill=(*highlight, 210),
        width=_s(1),
    )


def _bolt(draw: ImageDraw.ImageDraw, x: float, y: float, radius: float = 1.8) -> None:
    draw.ellipse(
        _box((x - radius, y - radius, x + radius, y + radius)), fill=(*IRON_DARK, 255)
    )
    draw.rectangle(_box((x - 0.7, y - 0.7, x + 0.7, y + 0.7)), fill=(*BONE, 230))


def _strut(
    draw: ImageDraw.ImageDraw,
    start: tuple[float, float],
    end: tuple[float, float],
    *,
    color: tuple[int, int, int] = IRON_LIGHT,
    width: float = 2,
) -> None:
    draw.line(_points((start, end)), fill=(*IRON_DARK, 255), width=_s(width + 2))
    draw.line(_points((start, end)), fill=(*color, 255), width=_s(width))


def _fabricator_sprite(
    *, carriage_x: int, tool_drop: int, assembly_stage: int
) -> Image.Image:
    size = NATIVE_SIZES["fabricator"]
    image, draw = _new_sprite(size)
    draw.rounded_rectangle(
        _box((8, 16, 120, 120)), radius=_s(8), fill=(*IRON_DARK, 255)
    )
    draw.rounded_rectangle(
        _box((13, 22, 115, 115)), radius=_s(6), fill=(24, 23, 28, 255)
    )
    draw.rounded_rectangle(
        _box((27, 30, 101, 109)), radius=_s(4), fill=(12, 12, 16, 255)
    )
    for x in (36, 84):
        draw.rounded_rectangle(
            _box((x, 34, x + 8, 105)), radius=_s(2), fill=(*IRON_DARK, 255)
        )
        draw.rectangle(_box((x + 3, 36, x + 5, 103)), fill=(*IRON_LIGHT, 255))
        for y in range(40, 103, 13):
            draw.rectangle(_box((x + 1, y, x + 7, y + 3)), fill=(*FERROUS["dark"], 255))
    draw.rectangle(_box((46, 103, 82, 114)), fill=(*IRON_DARK, 255))
    for x in range(48, 82, 8):
        draw.polygon(
            _points(((x, 104), (x + 4, 104), (x + 8, 113), (x + 4, 113))),
            fill=(*FERROUS["dark"], 255),
        )
    for x0, x1 in ((12, 27), (101, 116)):
        _beveled_plate(draw, (x0, 25, x1, 108), radius=3)
        draw.rectangle(_box((x0 + 4, 35, x1 - 4, 98)), fill=(18, 18, 22, 255))
        _strut(draw, (x0 + 4, 40), (x1 - 4, 61), color=FERROUS["dark"])
        _strut(draw, (x1 - 4, 62), (x0 + 4, 83), color=FERROUS["dark"])
        _strut(draw, (x0 + 4, 84), (x1 - 4, 98), color=FERROUS["dark"])
        _bolt(draw, (x0 + x1) / 2, 31)
        _bolt(draw, (x0 + x1) / 2, 103)
    _beveled_plate(draw, (11, 11, 117, 31), radius=4)
    draw.rectangle(_box((19, 23, 109, 29)), fill=(*IRON_DARK, 255))
    draw.rectangle(_box((21, 24, 107, 26)), fill=(*IRON_LIGHT, 255))
    for x in (20, 108):
        _bolt(draw, x, 20, 2.2)
    _beveled_plate(
        draw,
        (carriage_x - 10, 17, carriage_x + 10, 34),
        fill=FERROUS["dark"],
        edge=IRON_DARK,
        highlight=FERROUS["light"],
        radius=3,
    )
    draw.rectangle(
        _box((carriage_x - 3, 31, carriage_x + 3, 42 + tool_drop)),
        fill=(*IRON_LIGHT, 255),
    )
    draw.rectangle(
        _box((carriage_x - 5, 40 + tool_drop, carriage_x + 5, 47 + tool_drop)),
        fill=(*IRON_DARK, 255),
    )
    draw.polygon(
        _points(
            (
                (carriage_x - 5, 47 + tool_drop),
                (carriage_x, 52 + tool_drop),
                (carriage_x + 5, 47 + tool_drop),
            )
        ),
        fill=(*FERROUS["light"], 255),
    )
    if assembly_stage:
        width = (18, 28, 36)[assembly_stage - 1]
        x0 = 64 - width // 2
        x1 = 64 + width // 2
        draw.rounded_rectangle(
            _box((x0, 73, x1, 98)), radius=_s(4), fill=(*IRON_DARK, 255)
        )
        draw.rounded_rectangle(
            _box((x0 + 3, 76, x1 - 3, 94)), radius=_s(3), fill=(*IRON, 255)
        )
        draw.rectangle(_box((58, 79, 70, 94)), fill=(*FERROUS["dark"], 255))
        if assembly_stage >= 2:
            for side in (-1, 1):
                draw.rounded_rectangle(
                    _box((64 + side * 20 - 5, 78, 64 + side * 20 + 5, 97)),
                    radius=_s(2),
                    fill=(*IRON_LIGHT, 255),
                )
        if assembly_stage >= 3:
            draw.rectangle(_box((59, 68, 69, 78)), fill=(*FERROUS["base"], 255))
            draw.rectangle(_box((62, 66, 66, 71)), fill=(*BONE, 255))
    return _finish(image, size)


def fabricator_frames() -> tuple[StructureFrame, ...]:
    source = "authored:open-works-fabricator"
    specs = (
        (28, 0, 0, 560, "idle", "idle", False, None),
        (38, 8, 1, 190, "carriage_left", "working", True, None),
        (64, 22, 2, 240, "tool_press", "working", True, "assembly_cycle"),
        (88, 10, 3, 190, "carriage_right", "working", True, None),
        (30, 0, 3, 260, "carriage_home", "working", True, None),
    )
    return tuple(
        (
            StructureFrame(
                _fabricator_sprite(
                    carriage_x=carriage_x, tool_drop=tool_drop, assembly_stage=stage
                ),
                duration,
                event,
                state,
                active,
                source,
                beat,
                (carriage_x, 45 + tool_drop),
            )
            for carriage_x, tool_drop, stage, duration, event, state, active, beat in specs
        )
    )


def _repair_bay_sprite(
    *, arm_joint: tuple[int, int], torch: tuple[int, int], lift: int, welding: bool
) -> Image.Image:
    size = NATIVE_SIZES["repair_bay"]
    image, draw = _new_sprite(size)
    draw.rounded_rectangle(
        _box((8, 13, 120, 119)), radius=_s(8), fill=(*IRON_DARK, 255)
    )
    draw.rounded_rectangle(_box((14, 19, 114, 113)), radius=_s(6), fill=(*IRON, 255))
    draw.rounded_rectangle(
        _box((28, 28, 100, 111)), radius=_s(5), fill=(13, 13, 17, 255)
    )
    draw.rectangle(_box((42, 34, 86, 108)), fill=(8, 9, 12, 255))
    for x in (31, 91):
        draw.rounded_rectangle(
            _box((x - 5, 35, x + 5, 105)), radius=_s(2), fill=(*IRON_DARK, 255)
        )
        draw.rectangle(_box((x - 1, 38, x + 1, 102)), fill=(*IRON_LIGHT, 255))
        for y in range(42, 101, 14):
            draw.rectangle(_box((x - 3, y, x + 3, y + 3)), fill=(*FERROUS["dark"], 255))
    pad_inset = lift * 4
    for x0, x1 in ((34 + pad_inset, 48 + pad_inset), (80 - pad_inset, 94 - pad_inset)):
        _beveled_plate(
            draw,
            (x0, 66 - lift * 2, x1, 91 - lift * 2),
            fill=IRON,
            edge=IRON_DARK,
            highlight=FERROUS["light"],
            radius=3,
        )
    if lift:
        draw.rounded_rectangle(
            _box((49, 56, 79, 99)), radius=_s(6), fill=(*IRON_DARK, 255)
        )
        draw.rounded_rectangle(_box((54, 60, 74, 94)), radius=_s(4), fill=(*IRON, 255))
        draw.rectangle(_box((57, 70, 71, 86)), fill=(*FERROUS["dark"], 255))
        draw.rectangle(_box((60, 63, 68, 69)), fill=(*FERROUS["base"], 255))
    for x0, x1 in ((12, 27), (101, 116)):
        _beveled_plate(draw, (x0, 25, x1, 108), radius=3)
        for y in (38, 57, 96):
            draw.rectangle(
                _box((x0 + 4, y, x1 - 4, y + 9)), fill=(*FERROUS["dark"], 255)
            )
        _bolt(draw, (x0 + x1) / 2, 31)
        _bolt(draw, (x0 + x1) / 2, 103)
    shoulder = (105, 28)
    _strut(draw, shoulder, arm_joint, color=FERROUS["base"], width=7)
    _strut(draw, arm_joint, torch, color=FERROUS["light"], width=6)
    for joint_x, joint_y, radius in ((*shoulder, 7), (*arm_joint, 7), (*torch, 5)):
        draw.ellipse(
            _box(
                (joint_x - radius, joint_y - radius, joint_x + radius, joint_y + radius)
            ),
            fill=(*IRON_DARK, 255),
        )
        draw.ellipse(
            _box((joint_x - 3, joint_y - 3, joint_x + 3, joint_y + 3)),
            fill=(*FERROUS["base"], 255),
        )
        draw.rectangle(
            _box((joint_x - 1, joint_y - 1, joint_x + 1, joint_y + 1)),
            fill=(*BONE, 255),
        )
    draw.line(
        _points(((108, 33), (arm_joint[0] + 2, arm_joint[1] + 4), torch)),
        fill=(18, 17, 20, 255),
        width=_s(2),
    )
    draw.polygon(
        _points(
            (
                (torch[0] - 4, torch[1] + 2),
                (torch[0], torch[1] + 9),
                (torch[0] + 4, torch[1] + 2),
            )
        ),
        fill=(*IRON_LIGHT, 255),
    )
    if welding:
        contact = (torch[0], torch[1] + 10)
        draw.ellipse(
            _box((contact[0] - 4, contact[1] - 4, contact[0] + 4, contact[1] + 4)),
            fill=(*BONE, 255),
        )
        for dx, dy in ((-10, 2), (-7, 9), (8, 6), (11, -1)):
            draw.line(
                _points((contact, (contact[0] + dx, contact[1] + dy))),
                fill=(*SCRAP_LIGHT, 255),
                width=_s(2),
            )
    for x in range(34, 94, 12):
        draw.polygon(
            _points(((x, 109), (x + 6, 109), (x + 10, 116), (x + 4, 116))),
            fill=(*FERROUS["dark"], 255),
        )
    for x in (20, 108):
        draw.rectangle(_box((x - 2, 113, x + 2, 116)), fill=(*SCRAP, 255))
    return _finish(image, size)


def repair_bay_frames() -> tuple[StructureFrame, ...]:
    source = "authored:open-works-repair-bay"
    specs = (
        ((94, 30), (89, 39), 0, False, 560, "idle", "idle", False, None),
        ((88, 39), (80, 51), 1, False, 210, "arm_unfold", "working", True, None),
        (
            (82, 49),
            (64, 59),
            2,
            True,
            250,
            "weld_contact",
            "working",
            True,
            "repair_pulse",
        ),
        ((88, 40), (78, 50), 1, False, 190, "arm_recover", "working", True, None),
        ((94, 30), (89, 39), 0, False, 280, "arm_home", "working", True, None),
    )
    return tuple(
        (
            StructureFrame(
                _repair_bay_sprite(
                    arm_joint=joint, torch=torch, lift=lift, welding=welding
                ),
                duration,
                event,
                state,
                active,
                source,
                beat,
                torch,
            )
            for joint, torch, lift, welding, duration, event, state, active, beat in specs
        )
    )


def _polar(
    center: tuple[float, float], radius: float, degrees: float
) -> tuple[float, float]:
    radians = math.radians(degrees)
    return (
        center[0] + radius * math.cos(radians),
        center[1] + radius * math.sin(radians),
    )


def _array_sprite(*, heading: int) -> Image.Image:
    size = NATIVE_SIZES["array"]
    image, draw = _new_sprite(size)
    center = (32.0, 32.0)
    for x, y in ((14, 18), (50, 18), (14, 50), (50, 50)):
        _beveled_plate(draw, (x - 5, y - 4, x + 5, y + 4), radius=2)
        _bolt(draw, x, y, 1.3)
    draw.arc(_box((10, 10, 54, 54)), 195, 345, fill=(*IRON_DARK, 255), width=_s(7))
    draw.arc(_box((10, 10, 54, 54)), 15, 165, fill=(*IRON_DARK, 255), width=_s(7))
    draw.arc(_box((12, 12, 52, 52)), 198, 342, fill=(*IRON_LIGHT, 255), width=_s(2))
    draw.arc(_box((12, 12, 52, 52)), 18, 162, fill=(*IRON_LIGHT, 255), width=_s(2))
    start = heading - 52
    end = heading + 52
    arc_points = tuple(_polar(center, 19, angle) for angle in range(start, end + 1, 13))
    draw.polygon(_points((center, *arc_points)), fill=(24, 24, 30, 255))
    draw.arc(
        _box((13, 13, 51, 51)), start, end, fill=(*FERROUS["light"], 255), width=_s(3)
    )
    for offset in (-36, -18, 0, 18, 36):
        draw.line(
            _points((center, _polar(center, 18, heading + offset))),
            fill=(*IRON_LIGHT, 255),
            width=_s(1.5),
        )
    for radius in (8, 13):
        draw.arc(
            _box((32 - radius, 32 - radius, 32 + radius, 32 + radius)),
            start,
            end,
            fill=(*IRON, 255),
            width=_s(1.5),
        )
    tip = _polar(center, 17, heading)
    draw.line(_points((center, tip)), fill=(*FERROUS["base"], 255), width=_s(3))
    draw.ellipse(
        _box((tip[0] - 3, tip[1] - 3, tip[0] + 3, tip[1] + 3)), fill=(*BONE, 255)
    )
    draw.ellipse(_box((25, 25, 39, 39)), fill=(*IRON_DARK, 255))
    draw.ellipse(_box((28, 28, 36, 36)), fill=(*FERROUS["base"], 255))
    draw.rectangle(_box((31, 30, 33, 34)), fill=(*SCRAP_LIGHT, 255))
    return _finish(image, size)
