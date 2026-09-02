"""Approved production frames for Gnat, Kestrel, and Airworks.

Gnat 454 preserves the Forktail Probe's animated sensor and tail mechanism.
Kestrel 456 keeps the Armored Kite airframe fixed while only its eye and
status lights sequence. Airworks 457 keeps its empty bay sealed during
ordinary production and reserves its opening doors for completion.
"""

from __future__ import annotations

import hashlib
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

Registry = dict[str, Image.Image]
Color = tuple[int, int, int]
Point = tuple[int, int]
Box = tuple[int, int, int, int]

UNIT_SIZE = 64
BUILDING_SIZE = 128

BLACK = (11, 11, 15)
IRON_DEEP = (26, 27, 33)
IRON_DARK = (41, 42, 49)
IRON = (63, 63, 72)
IRON_MID = (78, 77, 85)
IRON_LIGHT = (103, 101, 107)
BONE = (226, 220, 204)
GLASS_DARK = (70, 88, 91)
GLASS = (143, 177, 175)
SCRAP_DARK = (112, 76, 39)
SCRAP = (171, 111, 48)
WORK_LIGHT = (242, 190, 94)


@dataclass(frozen=True)
class Palette:
    """Faction paint used sparingly over the shared industrial chassis."""

    base: Color
    dark: Color
    light: Color


PALETTES = {
    "ferrous": Palette((176, 75, 52), (105, 43, 33), (217, 116, 86)),
    "cupric": Palette((48, 132, 113), (29, 79, 68), (101, 181, 157)),
}

APPROVED_SOURCE_RGBA_SHA256 = (
    "20ce5a6e972a1ea140f178b1477e0b9d4461c2d95569adab8bb9be39c492b2cd"
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
    points: Sequence[Point],
    color: Color,
    width: int,
) -> None:
    draw.line(points, fill=_rgba(color), width=width, joint="curve")


def _panel(
    draw: ImageDraw.ImageDraw,
    box: Box,
    palette: Palette,
    *,
    fill: Color = IRON,
    radius: int = 3,
) -> None:
    x0, y0, x1, y1 = box
    draw.rounded_rectangle(box, radius=radius + 1, fill=_rgba(BLACK))
    draw.rounded_rectangle(
        (x0 + 2, y0 + 2, x1 - 2, y1 - 2),
        radius=max(1, radius - 1),
        fill=_rgba(fill),
    )
    draw.line((x0 + 4, y0 + 3, x1 - 4, y0 + 3), fill=_rgba(palette.light))


def _bolt(draw: ImageDraw.ImageDraw, point: Point) -> None:
    x, y = point
    draw.rectangle((x - 1, y - 1, x + 1, y + 1), fill=_rgba(BONE))


def _vent(draw: ImageDraw.ImageDraw, box: Box, phase: int = 0) -> None:
    x0, y0, x1, y1 = box
    draw.rectangle(box, fill=_rgba(BLACK))
    for index, y in enumerate(range(y0 + 2, y1, 3)):
        color = IRON_LIGHT if index != phase % 3 else WORK_LIGHT
        draw.line((x0 + 2, y, x1 - 2, y), fill=_rgba(color))


def _truss(draw: ImageDraw.ImageDraw, start: Point, end: Point, width: int = 4) -> None:
    _line(draw, (start, end), BLACK, width + 2)
    _line(draw, (start, end), IRON_MID, width)
    x0, y0 = start
    x1, y1 = end
    for fraction in (0.25, 0.5, 0.75):
        x = round(x0 + (x1 - x0) * fraction)
        y = round(y0 + (y1 - y0) * fraction)
        draw.rectangle((x - 1, y - 1, x + 1, y + 1), fill=_rgba(IRON_LIGHT))


def _sensor_lens(
    draw: ImageDraw.ImageDraw,
    center: Point,
    radius: int,
    palette: Palette,
    phase: int,
) -> None:
    cx, cy = center
    draw.ellipse(
        (cx - radius - 2, cy - radius - 2, cx + radius + 2, cy + radius + 2),
        fill=_rgba(BLACK),
    )
    draw.ellipse(
        (cx - radius, cy - radius, cx + radius, cy + radius),
        fill=_rgba(GLASS_DARK),
    )
    draw.ellipse(
        (cx - radius + 2, cy - radius + 2, cx + radius - 2, cy + radius - 2),
        fill=_rgba(GLASS),
    )
    dx = (-1, 0, 1)[phase % 3]
    draw.ellipse(
        (cx + dx - 2, cy - 2, cx + dx + 2, cy + 2),
        fill=_rgba(palette.light),
    )
    draw.point((cx + dx - 1, cy - 1), fill=_rgba(BONE))


def _engine_pod(
    draw: ImageDraw.ImageDraw,
    box: Box,
    palette: Palette,
    phase: int,
) -> None:
    _panel(draw, box, palette, fill=IRON_DARK, radius=3)
    x0, _, x1, y1 = box
    _vent(draw, (x0 + 3, y1 - 11, x1 - 3, y1 - 3), phase)


def _wing_panel(
    draw: ImageDraw.ImageDraw,
    points: Sequence[Point],
    palette: Palette,
) -> None:
    draw.polygon(points, fill=_rgba(BLACK))
    center_x = sum(x for x, _ in points) // len(points)
    center_y = sum(y for _, y in points) // len(points)
    inner = tuple(
        (
            round(x + (center_x - x) * 0.12),
            round(y + (center_y - y) * 0.12),
        )
        for x, y in points
    )
    draw.polygon(inner, fill=_rgba(IRON_MID))
    draw.line((*inner[0], *inner[1]), fill=_rgba(palette.light), width=1)


def _building_deck(
    draw: ImageDraw.ImageDraw, palette: Palette, *, cut_front: bool = False
) -> None:
    outline = (
        (7, 13),
        (121, 13),
        (124, 20),
        (124, 116),
        (114, 122),
        (14, 122),
        (4, 116),
        (4, 20),
    )
    if cut_front:
        outline = (
            (7, 13),
            (121, 13),
            (124, 20),
            (124, 116),
            (84, 122),
            (44, 122),
            (4, 116),
            (4, 20),
        )
    draw.polygon(outline, fill=_rgba(BLACK))
    inner = tuple(
        (x + (2 if x < 64 else -2), y + (2 if y < 64 else -2))
        for x, y in outline
    )
    draw.polygon(inner, fill=_rgba(IRON_DARK))
    draw.line((12, 18, 116, 18), fill=_rgba(palette.light), width=2)
    for point in ((12, 22), (116, 22), (10, 109), (118, 109)):
        _bolt(draw, point)


def render_gnat(faction: str, phase: int) -> Image.Image:
    """Render approved Forktail Probe candidate 454."""
    palette = PALETTES[faction]
    image, draw = _canvas(UNIT_SIZE)
    spread = (-1, 0, 1)[phase % 3]
    _panel(draw, (25, 10, 39, 44), palette, fill=IRON_DARK, radius=5)
    _sensor_lens(draw, (32, 18), 7, palette, phase)
    _truss(draw, (28, 36), (17 - spread, 57), 4)
    _truss(draw, (36, 36), (47 + spread, 57), 4)
    _engine_pod(draw, (11 - spread, 42, 22 - spread, 58), palette, phase)
    _engine_pod(draw, (42 + spread, 42, 53 + spread, 58), palette, phase + 1)
    _line(draw, ((32, 10), (32, 1)), IRON_LIGHT, 2)
    draw.polygon(
        ((29, 10), (32, 4), (35, 10)),
        fill=_rgba(palette.dark),
        outline=_rgba(BLACK),
    )
    return _finish(image)


def render_kestrel(faction: str, phase: int) -> Image.Image:
    """Render approved candidate 456 with a fixed airframe and sequenced lights."""
    palette = PALETTES[faction]
    image, draw = _canvas(UNIT_SIZE)
    _wing_panel(
        draw,
        ((32, 3), (59, 26), (52, 55), (32, 47), (12, 55), (5, 26)),
        palette,
    )
    draw.polygon(
        ((15, 28), (26, 15), (28, 40), (17, 46)), fill=_rgba(palette.dark)
    )
    draw.polygon(
        ((49, 28), (38, 15), (36, 40), (47, 46)), fill=_rgba(palette.dark)
    )
    _engine_pod(draw, (12, 29, 23, 48), palette, phase)
    _engine_pod(draw, (41, 29, 52, 48), palette, phase + 1)
    _panel(draw, (25, 7, 39, 57), palette, fill=IRON, radius=5)
    _sensor_lens(draw, (32, 20), 6, palette, phase)
    draw.rectangle((29, 39, 35, 50), fill=_rgba(palette.base))
    return _finish(image)


def _sequencer(draw: ImageDraw.ImageDraw, phase: int, y: int) -> None:
    for index, x in enumerate((48, 56, 64, 72, 80)):
        color = WORK_LIGHT if index == phase % 5 else IRON_MID
        draw.rectangle((x - 2, y, x + 2, y + 3), fill=_rgba(color))


def _door_panel(
    draw: ImageDraw.ImageDraw,
    points: tuple[Point, ...],
    palette: Palette,
    hinge: Point,
) -> None:
    draw.polygon(points, fill=_rgba(BLACK))
    center_x = sum(x for x, _ in points) // len(points)
    center_y = sum(y for _, y in points) // len(points)
    inner = tuple(
        (
            round(x + (center_x - x) * 0.08),
            round(y + (center_y - y) * 0.08),
        )
        for x, y in points
    )
    draw.polygon(inner, fill=_rgba(IRON))
    draw.line((*inner[0], *inner[1]), fill=_rgba(palette.light), width=2)
    hinge_x, hinge_y = hinge
    draw.rectangle(
        (hinge_x - 2, hinge_y - 4, hinge_x + 2, hinge_y + 4),
        fill=_rgba(IRON_LIGHT),
    )


def render_airworks(faction: str, stage: int) -> Image.Image:
    """Render approved Clampwell Airworks candidate 457."""
    if stage not in range(5):
        raise ValueError(f"unknown Airworks stage: {stage}")
    palette = PALETTES[faction]
    image, draw = _canvas(BUILDING_SIZE)
    _building_deck(draw, palette, cut_front=True)
    _panel(draw, (39, 17, 89, 38), palette, fill=IRON)
    for index, x in enumerate((45, 61, 77)):
        stock = SCRAP if index == min(stage, 2) else SCRAP_DARK
        draw.rectangle((x, 22, x + 8, 33), fill=_rgba(stock))

    well = (
        (50, 36),
        (78, 36),
        (78, 48),
        (99, 60),
        (99, 88),
        (79, 99),
        (79, 119),
        (49, 119),
        (49, 99),
        (29, 88),
        (29, 60),
        (50, 48),
    )
    inner = (
        (53, 41),
        (75, 41),
        (75, 52),
        (94, 63),
        (94, 85),
        (75, 95),
        (75, 116),
        (53, 116),
        (53, 95),
        (34, 85),
        (34, 63),
        (53, 52),
    )
    draw.polygon(well, fill=_rgba(BLACK))
    draw.polygon(inner, fill=_rgba(IRON_DEEP))

    open_stage = max(0, stage - 2)
    if open_stage == 0:
        left = ((34, 61), (62, 44), (62, 105), (34, 86))
        right = ((94, 61), (66, 44), (66, 105), (94, 86))
    elif open_stage == 1:
        left = ((31, 60), (51, 49), (51, 99), (31, 87))
        right = ((97, 60), (77, 49), (77, 99), (97, 87))
    else:
        left = ((29, 58), (42, 53), (42, 95), (29, 89))
        right = ((99, 58), (86, 53), (86, 95), (99, 89))
    _door_panel(draw, left, palette, (34, 74))
    _door_panel(draw, right, palette, (94, 74))
    _sequencer(draw, stage + 1, 109)
    _truss(draw, (55, 101), (55, 120), 3)
    _truss(draw, (73, 101), (73, 120), 3)
    return _finish(image)


def source_rgba_digest() -> str:
    """Hash every approved native frame in stable production-key order."""
    digest = hashlib.sha256()
    for faction in PALETTES:
        for phase in range(3):
            for stem, renderer in (("gnat", render_gnat), ("kestrel", render_kestrel)):
                digest.update(f"{stem}/{faction}/{phase}".encode())
                digest.update(renderer(faction, phase).tobytes())
        for stage in range(5):
            digest.update(f"airworks/{faction}/{stage}".encode())
            digest.update(render_airworks(faction, stage).tobytes())
    return digest.hexdigest()


def _put(registry: Registry, out: Path, key: str, image: Image.Image) -> None:
    native = image.convert("RGBA")
    native.save(out / f"{key}.png")
    registry[key] = native


def install_airworks_scouts(registry: Registry, out: Path) -> None:
    """Install the approved native gameplay frames into the generator bank."""
    out.mkdir(parents=True, exist_ok=True)
    for faction in PALETTES:
        unit_states = (("", 1), ("_move1", 0), ("_move2", 2))
        for suffix, phase in unit_states:
            _put(registry, out, f"gnat_{faction}{suffix}", render_gnat(faction, phase))
            _put(
                registry,
                out,
                f"kestrel_{faction}{suffix}",
                render_kestrel(faction, phase),
            )
        for stage in range(5):
            suffix = "" if stage == 0 else f"_work{stage}"
            _put(
                registry,
                out,
                f"airworks_{faction}{suffix}",
                render_airworks(faction, stage),
            )
