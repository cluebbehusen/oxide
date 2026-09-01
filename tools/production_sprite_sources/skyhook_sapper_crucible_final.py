"""Approved production frames for Skyhook, Sapper, and Crucible.

Skyhook 427 is faithfully re-authored on a 128-pixel canvas so its large-
transport presentation stays crisp at Condor scale. Sapper 433 and Crucible
440 preserve their approved native review sizes and mechanisms.
"""

from __future__ import annotations

import hashlib
import math
from dataclasses import dataclass
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

Registry = dict[str, Image.Image]
Color = tuple[int, int, int]
Point = tuple[int, int]
Box = tuple[int, int, int, int]

SKYHOOK_SIZE = 128
SAPPER_SIZE = 64
CRUCIBLE_SIZE = 128

BLACK = (11, 11, 15)
IRON_DEEP = (27, 27, 34)
IRON_DARK = (42, 42, 50)
IRON = (62, 62, 72)
IRON_LIGHT = (92, 91, 99)
BONE = (226, 220, 204)
SCRAP_DARK = (116, 78, 38)
SCRAP = (170, 111, 48)
HEAT_DARK = (133, 49, 24)
HEAT = (222, 108, 42)
FLASH = (255, 220, 132)


@dataclass(frozen=True)
class Palette:
    """Faction paint over the shared industrial chassis."""

    base: Color
    dark: Color
    light: Color


PALETTES = {
    "ferrous": Palette((176, 75, 52), (105, 43, 33), (217, 116, 86)),
    "cupric": Palette((48, 132, 113), (29, 79, 68), (101, 181, 157)),
}

APPROVED_SOURCE_RGBA_SHA256 = (
    "3417752f20bf1badef6f7e52b6b756cc9131160fa93b756cad071c2c1643b0b8"
)


def _rgba(color: Color, alpha: int = 255) -> tuple[int, int, int, int]:
    return (*color, alpha)


def _canvas(size: int) -> tuple[Image.Image, ImageDraw.ImageDraw]:
    image = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    return image, ImageDraw.Draw(image)


def _finish(image: Image.Image) -> Image.Image:
    alpha = image.getchannel("A")
    grown = alpha.filter(ImageFilter.MaxFilter(3))
    edge = ImageChops.subtract(grown, alpha)
    rim = Image.new("RGBA", image.size, _rgba(BONE, 0))
    rim.putalpha(edge)
    rim.alpha_composite(image)
    return rim


def _line(
    draw: ImageDraw.ImageDraw,
    points: tuple[Point, ...],
    color: Color,
    width: int,
) -> None:
    draw.line(points, fill=_rgba(color), width=width, joint="curve")


def _bolt(draw: ImageDraw.ImageDraw, point: Point) -> None:
    x, y = point
    draw.rectangle((x - 1, y - 1, x + 1, y + 1), fill=_rgba(BONE))


def _vent(draw: ImageDraw.ImageDraw, box: Box) -> None:
    x0, y0, x1, y1 = box
    draw.rectangle(box, fill=_rgba(BLACK))
    for y in range(y0 + 2, y1, 3):
        draw.line((x0 + 2, y, x1 - 2, y), fill=_rgba(IRON_LIGHT))


def _truss(draw: ImageDraw.ImageDraw, start: Point, end: Point, width: int = 5) -> None:
    _line(draw, (start, end), BLACK, width + 2)
    _line(draw, (start, end), IRON, width)
    x0, y0 = start
    x1, y1 = end
    for fraction in (0.25, 0.5, 0.75):
        x = round(x0 + (x1 - x0) * fraction)
        y = round(y0 + (y1 - y0) * fraction)
        draw.rectangle((x - 1, y - 1, x + 1, y + 1), fill=_rgba(IRON_LIGHT))


def _rotor(
    draw: ImageDraw.ImageDraw,
    center: Point,
    radius: int,
    phase: int,
    palette: Palette,
) -> None:
    cx, cy = center
    draw.ellipse((cx - radius, cy - radius, cx + radius, cy + radius), fill=_rgba(BLACK))
    draw.ellipse(
        (cx - radius + 2, cy - radius + 2, cx + radius - 2, cy + radius - 2),
        fill=_rgba(IRON_DEEP),
        outline=_rgba(IRON_LIGHT),
        width=1,
    )
    angle = phase * math.pi / 4
    for offset in (0.0, math.pi / 2):
        dx = round((radius - 3) * math.cos(angle + offset))
        dy = round((radius - 3) * math.sin(angle + offset))
        _line(draw, ((cx - dx, cy - dy), (cx + dx, cy + dy)), palette.dark, 2)
    draw.ellipse((cx - 2, cy - 2, cx + 2, cy + 2), fill=_rgba(palette.light))


def _clamp(
    draw: ImageDraw.ImageDraw,
    anchor: Point,
    inward: Point,
    action: int,
    palette: Palette,
) -> None:
    ax, ay = anchor
    ix, iy = inward
    travel = (0, -1, 2, 3, 1)[action]
    dx = 0 if ix == ax else (1 if ix > ax else -1)
    dy = 0 if iy == ay else (1 if iy > ay else -1)
    joint = (ax + dx * travel, ay + dy * travel)
    _line(draw, (anchor, joint, inward), BLACK, 4)
    _line(draw, (anchor, joint, inward), IRON_LIGHT, 2)
    draw.ellipse((ax - 3, ay - 3, ax + 3, ay + 3), fill=_rgba(BLACK))
    draw.rectangle((ax - 1, ay - 1, ax + 1, ay + 1), fill=_rgba(palette.light))
    spread = (3, 5, 6, 2, 1)[action]
    draw.line((ix - dy * spread, iy + dx * spread, ix, iy), fill=_rgba(SCRAP), width=2)
    draw.line((ix + dy * spread, iy - dx * spread, ix, iy), fill=_rgba(SCRAP), width=2)


def _cargo_vault(draw: ImageDraw.ImageDraw, box: Box, action: int, palette: Palette) -> None:
    x0, y0, x1, y1 = box
    draw.rounded_rectangle(box, radius=3, fill=_rgba(BLACK))
    draw.rectangle((x0 + 3, y0 + 3, x1 - 3, y1 - 3), fill=_rgba(IRON_DEEP))
    mid = (x0 + x1) // 2
    requested_gap = (0, 1, 2, 1, 0)[action]
    door_gap = min(requested_gap, max(0, mid - (x0 + 3)))
    draw.rectangle((x0 + 3, y0 + 3, mid - door_gap, y1 - 3), fill=_rgba(IRON))
    draw.rectangle((mid + door_gap, y0 + 3, x1 - 3, y1 - 3), fill=_rgba(IRON))
    if action == 4:
        for y in range(y0 + 7, y1 - 4, 7):
            draw.rectangle((mid - 2, y, mid + 2, y + 3), fill=_rgba(palette.light))
    else:
        draw.line((mid, y0 + 4, mid, y1 - 4), fill=_rgba(palette.dark), width=2)


def render_skyhook(faction: str, move_phase: int = 0, action: int = 0) -> Image.Image:
    """Render approved Clampwing Hauler art on its 128-pixel canvas."""
    palette = PALETTES[faction]
    if action not in range(5):
        raise ValueError(f"unknown Skyhook action: {action}")
    image, draw = _canvas(64)
    _rotor(draw, (15, 22), 10, move_phase % 4, palette)
    _rotor(draw, (49, 22), 10, move_phase % 4 + 1, palette)
    _truss(draw, (22, 23), (27, 30), 5)
    _truss(draw, (42, 23), (37, 30), 5)
    draw.rounded_rectangle((24, 17, 40, 55), radius=4, fill=_rgba(BLACK))
    draw.rounded_rectangle((26, 19, 38, 53), radius=2, fill=_rgba(IRON_DARK))
    draw.line((27, 20, 37, 20), fill=_rgba(palette.light))
    _cargo_vault(draw, (27, 23, 37, 51), action, palette)
    for anchor, inward in (
        ((10, 44), (23, 38)),
        ((54, 44), (41, 38)),
        ((14, 56), (25, 48)),
        ((50, 56), (39, 48)),
    ):
        _clamp(draw, anchor, inward, action, palette)
    _vent(draw, (26, 18, 31, 23))
    _vent(draw, (33, 18, 38, 23))
    enlarged = image.resize((SKYHOOK_SIZE, SKYHOOK_SIZE), Image.Resampling.NEAREST)
    return _finish(enlarged)


def _walker_foot(
    draw: ImageDraw.ImageDraw,
    box: Box,
    move_phase: int,
    index: int,
    palette: Palette,
) -> None:
    x0, y0, x1, y1 = box
    stride = (move_phase + index) % 3
    toe = (0, 2, 1)[stride]
    if toe:
        draw.rectangle((x0 + 1, y0 - toe, x1 - 1, y0 + 2), fill=_rgba(BLACK))
    draw.rounded_rectangle(box, radius=2, fill=_rgba(BLACK))
    draw.rectangle((x0 + 2, y0 + 2, x1 - 2, y1 - 2), fill=_rgba(IRON_DARK))
    travel = max(3, y1 - y0 - 7)
    offset = round((0.15, 0.5, 0.82)[stride] * travel)
    center_x = (x0 + x1) // 2
    draw.line(
        (center_x, y0 + 3, center_x, y0 + 3 + offset),
        fill=_rgba(IRON_LIGHT),
        width=2,
    )
    draw.rectangle(
        (x0 + 2, y0 + 2 + offset, x1 - 2, min(y1 - 2, y0 + 5 + offset)),
        fill=_rgba(palette.dark if stride == 1 else BONE),
    )


def _arm_lamp(draw: ImageDraw.ImageDraw, point: Point, action: int, palette: Palette) -> None:
    x, y = point
    color = (palette.dark, SCRAP_DARK, SCRAP, FLASH, IRON_LIGHT)[action]
    draw.ellipse((x - 3, y - 3, x + 3, y + 3), fill=_rgba(BLACK))
    draw.rectangle((x - 1, y - 1, x + 1, y + 1), fill=_rgba(color))


def _detonation_flash(draw: ImageDraw.ImageDraw, center: Point, action: int) -> None:
    if action != 3:
        return
    x, y = center
    draw.polygon(
        (
            (x, y - 8),
            (x + 3, y - 3),
            (x + 9, y),
            (x + 3, y + 3),
            (x, y + 8),
            (x - 3, y + 3),
            (x - 9, y),
            (x - 3, y - 3),
        ),
        fill=_rgba(FLASH),
    )
    draw.rectangle((x - 2, y - 2, x + 2, y + 2), fill=_rgba(BONE))


def render_sapper(faction: str, move_phase: int = 0, action: int = 0) -> Image.Image:
    """Render approved Blast Beetle art and its demolition states."""
    palette = PALETTES[faction]
    if action not in range(5):
        raise ValueError(f"unknown Sapper action: {action}")
    image, draw = _canvas(SAPPER_SIZE)
    if action == 4:
        return image
    boxes = ((8, 18, 16, 36), (48, 18, 56, 36), (10, 40, 18, 57), (46, 40, 54, 57))
    for index, box in enumerate(boxes):
        _walker_foot(draw, box, move_phase, index, palette)
    draw.ellipse((15, 11, 49, 57), fill=_rgba(BLACK))
    draw.ellipse((18, 14, 46, 54), fill=_rgba(IRON_DARK))
    draw.polygon(((22, 17), (42, 17), (45, 38), (19, 38)), fill=_rgba(palette.dark))
    jaw = (1, 5, 7, 2, 3)[action]
    draw.polygon(((22, 14), (28, 4), (31, 17), (26, 23)), fill=_rgba(IRON_LIGHT))
    draw.polygon(((42, 14), (36, 4), (33, 17), (38, 23)), fill=_rgba(IRON_LIGHT))
    draw.line((28 - jaw, 8, 32, 17), fill=_rgba(SCRAP), width=2)
    draw.line((36 + jaw, 8, 32, 17), fill=_rgba(SCRAP), width=2)
    _arm_lamp(draw, (32, 45), action, palette)
    _detonation_flash(draw, (32, 5), action)
    return _finish(image)


def _foundation(draw: ImageDraw.ImageDraw, palette: Palette) -> None:
    draw.polygon(
        ((12, 4), (116, 4), (124, 12), (124, 116), (116, 124), (12, 124), (4, 116), (4, 12)),
        fill=_rgba(BLACK),
    )
    draw.polygon(
        ((16, 9), (112, 9), (119, 16), (119, 112), (112, 119), (16, 119), (9, 112), (9, 16)),
        fill=_rgba(IRON_DEEP),
    )
    for point in ((15, 15), (113, 15), (15, 113), (113, 113)):
        draw.ellipse((point[0] - 4, point[1] - 4, point[0] + 4, point[1] + 4), fill=_rgba(IRON))
        _bolt(draw, point)
    draw.line((18, 10, 110, 10), fill=_rgba(palette.dark), width=3)


def _hopper(
    draw: ImageDraw.ImageDraw,
    box: Box,
    palette: Palette,
    work: int,
    index: int,
) -> None:
    x0, y0, x1, y1 = box
    draw.rounded_rectangle(box, radius=4, fill=_rgba(BLACK))
    draw.polygon(
        ((x0 + 3, y0 + 3), (x1 - 3, y0 + 3), (x1 - 7, y1 - 3), (x0 + 7, y1 - 3)),
        fill=_rgba(IRON),
    )
    fill_height = 4 + ((work + index) % 3) * 3
    draw.rectangle((x0 + 7, y1 - 4 - fill_height, x1 - 7, y1 - 4), fill=_rgba(SCRAP_DARK))
    draw.line((x0 + 8, y1 - 5 - fill_height, x1 - 8, y1 - 5 - fill_height), fill=_rgba(SCRAP))
    draw.rectangle((x0 + 4, y0 + 4, x1 - 4, y0 + 7), fill=_rgba(palette.dark))


def _rail(draw: ImageDraw.ImageDraw, x0: int, y0: int, x1: int, y1: int, work: int) -> None:
    draw.rectangle((x0, y0, x1, y1), fill=_rgba(BLACK))
    if y1 - y0 > x1 - x0:
        for y in range(y0 + 4, y1 - 2, 8):
            shifted = y0 + 4 + ((y - y0 - 4 + work * 3) % max(8, y1 - y0 - 7))
            draw.rectangle((x0 + 2, shifted, x1 - 2, min(y1 - 2, shifted + 3)), fill=_rgba(IRON_LIGHT))
    else:
        for x in range(x0 + 4, x1 - 2, 8):
            shifted = x0 + 4 + ((x - x0 - 4 + work * 3) % max(8, x1 - x0 - 7))
            draw.rectangle((shifted, y0 + 2, min(x1 - 2, shifted + 3), y1 - 2), fill=_rgba(IRON_LIGHT))


def _furnace(
    draw: ImageDraw.ImageDraw,
    center: Point,
    radius: int,
    work: int,
    palette: Palette,
) -> None:
    cx, cy = center
    draw.ellipse((cx - radius, cy - radius, cx + radius, cy + radius), fill=_rgba(BLACK))
    draw.ellipse(
        (cx - radius + 4, cy - radius + 4, cx + radius - 4, cy + radius - 4),
        fill=_rgba(IRON),
    )
    draw.ellipse(
        (cx - radius + 9, cy - radius + 9, cx + radius - 9, cy + radius - 9),
        fill=_rgba(IRON_DEEP),
    )
    draw.ellipse((cx - 8, cy - 8, cx + 8, cy + 8), fill=_rgba((BLACK, HEAT_DARK, HEAT, FLASH)[work]))
    draw.arc(
        (cx - radius + 2, cy - radius + 2, cx + radius - 2, cy + radius - 2),
        205,
        335,
        fill=_rgba(palette.dark),
        width=4,
    )
    for angle in range(0, 360, 45):
        x = round(cx + (radius - 3) * math.cos(math.radians(angle)))
        y = round(cy + (radius - 3) * math.sin(math.radians(angle)))
        _bolt(draw, (x, y))


def render_crucible(faction: str, work: int = 0) -> Image.Image:
    """Render approved Hammer Mill art and one production phase."""
    palette = PALETTES[faction]
    if work not in range(4):
        raise ValueError(f"unknown Crucible work phase: {work}")
    image, draw = _canvas(CRUCIBLE_SIZE)
    _foundation(draw, palette)
    _furnace(draw, (64, 29), 22, work, palette)
    _hopper(draw, (12, 17, 34, 46), palette, work, 0)
    _hopper(draw, (94, 17, 116, 46), palette, work, 1)
    _rail(draw, 54, 45, 74, 117, work)
    draw.rectangle((28, 49, 100, 108), fill=_rgba(BLACK))
    draw.rectangle((34, 55, 94, 102), fill=_rgba(IRON_DARK))
    hammer_y = (58, 66, 80, 62)[work]
    for x in (43, 85):
        draw.rectangle((x - 8, 45, x + 8, hammer_y), fill=_rgba(BLACK))
        draw.rectangle((x - 4, 48, x + 4, hammer_y - 4), fill=_rgba(IRON_LIGHT))
        draw.rectangle((x - 11, hammer_y - 4, x + 11, hammer_y + 5), fill=_rgba(palette.dark))
    draw.rectangle((49, 75, 79, 98), fill=_rgba(IRON_DEEP))
    if work >= 2:
        draw.rectangle((55, 80, 73, 93), fill=_rgba(HEAT if work == 2 else SCRAP))
    _vent(draw, (13, 58, 25, 103))
    _vent(draw, (103, 58, 115, 103))
    return _finish(image)


def source_rgba_digest() -> str:
    """Digest every approved source frame in stable order."""
    digest = hashlib.sha256()
    for faction in ("ferrous", "cupric"):
        for stem, renderer, size in (
            ("skyhook", render_skyhook, SKYHOOK_SIZE),
            ("sapper", render_sapper, SAPPER_SIZE),
        ):
            for label, move_phase, action in (
                ("idle", 0, 0),
                ("move1", 1, 0),
                ("move2", 2, 0),
                ("action1", 0, 1),
                ("action2", 1, 2),
                ("action3", 2, 3),
                ("action4", 3, 4),
            ):
                image = renderer(faction, move_phase, action)
                if image.size != (size, size):
                    raise AssertionError(f"{stem} produced {image.size}")
                digest.update(f"{stem}/{faction}/{label}".encode())
                digest.update(image.tobytes())
        for work in range(4):
            image = render_crucible(faction, work)
            digest.update(f"crucible/{faction}/work{work}".encode())
            digest.update(image.tobytes())
    return digest.hexdigest()


def _put(registry: Registry, out: Path, key: str, image: Image.Image) -> None:
    native = image.convert("RGBA")
    native.save(out / f"{key}.png")
    registry[key] = native


def install_skyhook_sapper_crucible(registry: Registry, out: Path) -> None:
    """Install the three approved rows into the production sprite bank."""
    out.mkdir(parents=True, exist_ok=True)
    for faction in ("ferrous", "cupric"):
        for stem, renderer, action_count in (
            ("skyhook", render_skyhook, 4),
            ("sapper", render_sapper, 3),
        ):
            _put(registry, out, f"{stem}_{faction}", renderer(faction, 0, 0))
            for move_phase in (1, 2):
                _put(
                    registry,
                    out,
                    f"{stem}_{faction}_move{move_phase}",
                    renderer(faction, move_phase, 0),
                )
            for action in range(1, action_count + 1):
                _put(
                    registry,
                    out,
                    f"{stem}_{faction}_action{action}",
                    renderer(faction, action - 1, action),
                )
        _put(registry, out, f"crucible_{faction}", render_crucible(faction, 0))
        for work in range(1, 4):
            _put(
                registry,
                out,
                f"crucible_{faction}_work{work}",
                render_crucible(faction, work),
            )
