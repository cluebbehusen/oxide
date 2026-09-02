"""Approved production frames for Extractor and the Reclaimer upgrade family.

Extractor 469 is the radial auger redesign. Reclaimer 476 preserves the
recognizable open-frame machine while adding a guarded feed. Refinery 471 is
the upgraded Reclaimer's twin-hammer redesign.
"""

from __future__ import annotations

import hashlib
import math
from pathlib import Path

from PIL import Image, ImageDraw

from tools import gen_sprites as gen

Registry = dict[str, Image.Image]

SS = 4
WORK_SUFFIXES = ("", "_work1", "_work2", "_work3")
IRON_DEEP = (17, 18, 23)
VOID = (10, 10, 13)
AMBER = (151, 93, 38)
AMBER_LIGHT = (218, 151, 65)

APPROVED_SOURCE_RGBA_SHA256 = (
    "2b2778b466cf24fe0d4a66b3b826d7db036a54ab2936137bcb1eb4fa1adff159"
)


def _s(value: float) -> int:
    return round(value * SS)


def _box(values: tuple[float, float, float, float]) -> tuple[int, int, int, int]:
    return tuple(_s(value) for value in values)  # type: ignore[return-value]


def _points(
    values: tuple[tuple[float, float], ...],
) -> tuple[tuple[int, int], ...]:
    return tuple((_s(x), _s(y)) for x, y in values)


def _new_sprite(size: int) -> tuple[Image.Image, ImageDraw.ImageDraw]:
    image = Image.new("RGBA", (size * SS, size * SS), (0, 0, 0, 0))
    return image, ImageDraw.Draw(image)


def _finish(image: Image.Image, size: int) -> Image.Image:
    native = image.resize((size, size), Image.Resampling.LANCZOS)
    return gen.rim_light(native)


def _plate(
    draw: ImageDraw.ImageDraw,
    bounds: tuple[float, float, float, float],
    *,
    fill: tuple[int, int, int] = gen.IRON,
    edge: tuple[int, int, int] = gen.IRON_DARK,
    highlight: tuple[int, int, int] = gen.IRON_LIGHT,
    radius: float = 4,
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
        fill=(*highlight, 190),
        width=_s(1),
    )


def _bolt(draw: ImageDraw.ImageDraw, x: float, y: float, radius: float = 1.7) -> None:
    draw.ellipse(
        _box((x - radius, y - radius, x + radius, y + radius)),
        fill=(*gen.IRON_DARK, 255),
    )
    draw.rectangle(_box((x - 0.6, y - 0.6, x + 0.6, y + 0.6)), fill=(*gen.BONE, 230))


def _strut(
    draw: ImageDraw.ImageDraw,
    start: tuple[float, float],
    end: tuple[float, float],
    *,
    color: tuple[int, int, int] = gen.IRON_LIGHT,
    width: float = 2,
) -> None:
    draw.line(_points((start, end)), fill=(*gen.IRON_DARK, 255), width=_s(width + 2))
    draw.line(_points((start, end)), fill=(*color, 255), width=_s(width))


def _foundation(draw: ImageDraw.ImageDraw, size: int, faction: str) -> None:
    palette = gen.FACTIONS[faction]
    inset = 7 if size == 128 else 4
    outer = size - inset
    draw.rounded_rectangle(
        _box((inset, inset + 2, outer, outer - 1)),
        radius=_s(8 if size == 128 else 6),
        fill=(*gen.IRON_DARK, 255),
    )
    draw.rounded_rectangle(
        _box((inset + 4, inset + 6, outer - 4, outer - 5)),
        radius=_s(6 if size == 128 else 4),
        fill=(*IRON_DEEP, 255),
    )
    stripe_y = outer - (10 if size == 128 else 7)
    draw.rectangle(
        _box((inset + 9, stripe_y, outer - 9, stripe_y + 3)),
        fill=(*palette["dark"], 255),
    )
    corners = (
        (inset + 7, inset + 9),
        (outer - 7, inset + 9),
        (inset + 7, outer - 8),
        (outer - 7, outer - 8),
    )
    for x, y in corners:
        _bolt(draw, x, y, 2 if size == 128 else 1.4)


def _hopper(
    draw: ImageDraw.ImageDraw,
    bounds: tuple[float, float, float, float],
    faction: str,
    *,
    split: bool = False,
) -> None:
    x0, y0, x1, y1 = bounds
    palette = gen.FACTIONS[faction]
    draw.polygon(
        _points(((x0, y0), (x1, y0), (x1 - 5, y1), (x0 + 5, y1))),
        fill=(*gen.IRON_DARK, 255),
    )
    draw.polygon(
        _points(
            (
                (x0 + 3, y0 + 3),
                (x1 - 3, y0 + 3),
                (x1 - 7, y1 - 3),
                (x0 + 7, y1 - 3),
            )
        ),
        fill=(*VOID, 255),
    )
    draw.line(
        _points(((x0 + 4, y0 + 2), (x1 - 4, y0 + 2))),
        fill=(*palette["light"], 235),
        width=_s(2),
    )
    if split:
        draw.line(
            _points((((x0 + x1) / 2, y0 + 3), ((x0 + x1) / 2, y1 - 3))),
            fill=(*gen.IRON_LIGHT, 255),
            width=_s(2),
        )


def _belt(
    draw: ImageDraw.ImageDraw,
    bounds: tuple[float, float, float, float],
    phase: int,
    *,
    faction: str,
) -> None:
    x0, y0, x1, y1 = bounds
    palette = gen.FACTIONS[faction]
    draw.rounded_rectangle(_box(bounds), radius=_s(2), fill=(*gen.IRON_DARK, 255))
    draw.rectangle(_box((x0 + 2, y0 + 2, x1 - 2, y1 - 2)), fill=(*VOID, 255))
    offset = phase * 3 % 8
    for y in range(round(y0) - 8, round(y1) + 8, 8):
        at = y + offset
        if y0 + 2 <= at <= y1 - 2:
            draw.line(
                _points(((x0 + 2, at), (x1 - 2, at))),
                fill=(*palette["dark"], 255),
                width=_s(2),
            )


def _cutter_ring(
    draw: ImageDraw.ImageDraw,
    center: tuple[float, float],
    radius: float,
    phase: int,
    faction: str,
    *,
    teeth: int,
) -> None:
    cx, cy = center
    palette = gen.FACTIONS[faction]
    draw.ellipse(
        _box((cx - radius, cy - radius, cx + radius, cy + radius)),
        fill=(*gen.IRON_DARK, 255),
    )
    draw.ellipse(
        _box((cx - radius + 5, cy - radius + 5, cx + radius - 5, cy + radius - 5)),
        fill=(*palette["base"], 255),
    )
    draw.ellipse(
        _box(
            (
                cx - radius + 12,
                cy - radius + 12,
                cx + radius - 12,
                cy + radius - 12,
            )
        ),
        fill=(*VOID, 255),
    )
    for index in range(teeth):
        angle = index * math.tau / teeth + phase * math.tau / (teeth * 4)
        x = cx + (radius - 3) * math.cos(angle)
        y = cy + (radius - 3) * math.sin(angle)
        draw.rounded_rectangle(
            _box((x - 3, y - 3, x + 3, y + 3)),
            radius=_s(1),
            fill=(*gen.IRON_LIGHT, 255),
        )
        draw.rectangle(_box((x - 1, y - 1, x + 1, y + 1)), fill=(*gen.SCRAP, 255))


def render_extractor(faction: str, phase: int) -> Image.Image:
    """Render approved radial-auger Extractor candidate 469."""
    image, draw = _new_sprite(128)
    palette = gen.FACTIONS[faction]
    _foundation(draw, 128, faction)
    center = (63.0, 58.0)
    for angle in (0, math.pi / 2, math.pi, 3 * math.pi / 2):
        dx, dy = math.cos(angle), math.sin(angle)
        px, py = -dy, dx
        start = (center[0] + dx * 18, center[1] + dy * 18)
        end = (center[0] + dx * 46, center[1] + dy * 46)
        polygon = (
            (start[0] + px * 5, start[1] + py * 5),
            (end[0] + px * 5, end[1] + py * 5),
            (end[0] - px * 5, end[1] - py * 5),
            (start[0] - px * 5, start[1] - py * 5),
        )
        draw.polygon(_points(polygon), fill=(*gen.IRON_DARK, 255))
        draw.line(_points((start, end)), fill=(*palette["dark"], 255), width=_s(5))
        for slat in range(3):
            distance = 22 + slat * 10
            x = center[0] + dx * distance
            y = center[1] + dy * distance
            draw.line(
                _points(((x - px * 4, y - py * 4), (x + px * 4, y + py * 4))),
                fill=(*gen.IRON_LIGHT, 230),
                width=_s(1),
            )
        for step in range(3):
            distance = 23 + ((step * 10 + phase * 4) % 27)
            x = center[0] + dx * distance
            y = center[1] + dy * distance
            draw.rectangle(
                _box((x - 2, y - 2, x + 2, y + 2)),
                fill=(*gen.SCRAP_LIGHT, 255),
            )
    draw.ellipse(_box((37, 32, 89, 84)), fill=(*gen.IRON_DARK, 255))
    draw.ellipse(_box((43, 38, 83, 78)), fill=(*palette["base"], 255))
    for arm in range(8):
        angle = arm * math.tau / 8 + phase * math.pi / 16
        inner = (63 + 7 * math.cos(angle), 58 + 7 * math.sin(angle))
        outer = (63 + 18 * math.cos(angle), 58 + 18 * math.sin(angle))
        _strut(draw, inner, outer, color=gen.BONE, width=3)
    draw.ellipse(_box((54, 49, 72, 67)), fill=(*VOID, 255))
    draw.arc(
        _box((55, 40, 71, 75)),
        250 + phase * 25,
        440 + phase * 25,
        fill=(*AMBER_LIGHT, 255),
        width=_s(3),
    )
    for bounds in (
        (14, 14, 41, 36),
        (87, 14, 114, 36),
        (14, 88, 41, 110),
        (87, 88, 114, 110),
    ):
        _hopper(draw, bounds, faction)
        x0, _y0, x1, y1 = bounds
        draw.rectangle(
            _box((x0 + 7, y1 - 8, x1 - 7, y1 - 4)),
            fill=(*gen.SCRAP_DARK, 255),
        )
        draw.rectangle(
            _box((x0 + 10, y1 - 10, x0 + 14, y1 - 7)),
            fill=(*gen.SCRAP_LIGHT, 255),
        )
    for x, y in ((28, 61), (98, 61), (63, 100)):
        draw.ellipse(_box((x - 5, y - 5, x + 5, y + 5)), fill=(*palette["dark"], 255))
        _bolt(draw, x, y, 2)
    return _finish(image, 128)


def render_refinery(faction: str, phase: int) -> Image.Image:
    """Render approved twin-hammer Refinery candidate 471."""
    image, draw = _new_sprite(64)
    palette = gen.FACTIONS[faction]
    _foundation(draw, 64, faction)
    _hopper(draw, (7, 8, 31, 25), faction)
    _hopper(draw, (33, 8, 57, 25), faction)
    _plate(draw, (8, 24, 56, 48), fill=IRON_DEEP, radius=3)
    for cx, direction in ((25, 1), (39, -1)):
        draw.ellipse(_box((cx - 10, 27, cx + 10, 45)), fill=(*palette["base"], 255))
        for tooth in range(6):
            angle = tooth * math.tau / 6 + direction * phase * math.pi / 12
            x = cx + 7 * math.cos(angle)
            y = 36 + 6 * math.sin(angle)
            draw.rectangle(
                _box((x - 2, y - 2, x + 2, y + 2)),
                fill=(*gen.IRON_LIGHT, 255),
            )
    draw.rectangle(_box((28, 31, 36, 41)), fill=(*VOID, 255))
    draw.rectangle(_box((22, 48, 42, 57)), fill=(*gen.IRON_DARK, 255))
    draw.rectangle(_box((26, 50, 38, 55)), fill=(*gen.SCRAP_DARK, 255))
    _plate(draw, (5, 28, 15, 46), fill=gen.IRON, radius=2)
    drive_color = AMBER if phase % 2 else palette["dark"]
    draw.ellipse(_box((7, 33, 13, 39)), fill=(*drive_color, 255))
    return _finish(image, 64)


def _reclaimer_shell(draw: ImageDraw.ImageDraw, faction: str) -> None:
    palette = gen.FACTIONS[faction]
    draw.rounded_rectangle(
        _box((6, 7, 58, 57)),
        radius=_s(6),
        outline=(*gen.IRON_DARK, 255),
        width=_s(6),
    )
    draw.line(_points(((10, 10), (54, 10))), fill=(*gen.IRON_LIGHT, 230), width=_s(2))
    draw.rectangle(_box((11, 11, 21, 53)), fill=(*gen.IRON, 255))
    draw.rectangle(_box((43, 11, 53, 53)), fill=(*gen.IRON, 255))
    for x in (16, 48):
        for y in (17, 30, 46):
            _bolt(draw, x, y, 1.3)
    draw.rectangle(_box((27, 9, 37, 13)), fill=(*palette["dark"], 255))


def render_reclaimer(faction: str, phase: int) -> Image.Image:
    """Render approved guarded-feed Reclaimer candidate 476."""
    image, draw = _new_sprite(64)
    palette = gen.FACTIONS[faction]
    _reclaimer_shell(draw, faction)
    _hopper(draw, (21, 11, 43, 24), faction, split=True)
    _belt(draw, (25, 20, 39, 45), phase, faction=faction)
    _cutter_ring(draw, (32, 40), 13, phase, faction, teeth=8)
    draw.arc(
        _box((17, 24, 47, 55)),
        195,
        345,
        fill=(*gen.IRON_LIGHT, 255),
        width=_s(2),
    )
    for x in (21, 27, 37, 43):
        draw.line(
            _points(((x, 28), (x, 48))),
            fill=(*gen.IRON_DARK, 210),
            width=_s(1),
        )
    _plate(draw, (7, 20, 18, 34), fill=gen.IRON, radius=2)
    motor_color = AMBER if phase in (1, 2) else palette["dark"]
    draw.rectangle(_box((10, 23, 15, 31)), fill=(*motor_color, 255))
    draw.rectangle(_box((25, 51, 39, 58)), fill=(*gen.SCRAP_DARK, 255))
    return _finish(image, 64)


def source_rgba_digest() -> str:
    """Hash every approved native frame in stable production-key order."""
    digest = hashlib.sha256()
    for faction in ("ferrous", "cupric"):
        for stem, renderer in (
            ("extractor", render_extractor),
            ("reclaimer_t1", render_refinery),
            ("reclaimer", render_reclaimer),
        ):
            for phase in range(4):
                digest.update(f"{stem}/{faction}/{phase}".encode())
                digest.update(renderer(faction, phase).tobytes())
    return digest.hexdigest()


def _put(registry: Registry, out: Path, key: str, image: Image.Image) -> None:
    native = image.convert("RGBA")
    native.save(out / f"{key}.png")
    registry[key] = native


def install_extractor_reclaimer(registry: Registry, out: Path) -> None:
    """Install the approved building frames into the generator bank."""
    out.mkdir(parents=True, exist_ok=True)
    for faction in ("ferrous", "cupric"):
        for stem, renderer in (
            ("extractor", render_extractor),
            ("reclaimer_t1", render_refinery),
            ("reclaimer", render_reclaimer),
        ):
            for phase, suffix in enumerate(WORK_SUFFIXES):
                _put(
                    registry, out, f"{stem}_{faction}{suffix}", renderer(faction, phase)
                )
