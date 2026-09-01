"""Production-native frames for approved Excavator candidate 423.

The milling-drum chassis animates locomotion and work independently. Its
recessed cargo meter is a separate aligned layer so the shell can display the
authoritative load fraction without duplicating every chassis pose.
"""

from __future__ import annotations

import hashlib
from pathlib import Path

from PIL import (  # ty: ignore[unresolved-import]
    Image,
    ImageChops,
    ImageDraw,
    ImageFilter,
)

Registry = dict[str, Image.Image]
Color = tuple[int, int, int]

SIZE = 128
ACTION_COUNT = 4
CARGO_LEVELS = 5

BLACK = (11, 11, 15)
IRON_DEEP = (27, 27, 34)
IRON_DARK = (42, 42, 50)
IRON = (62, 62, 72)
IRON_LIGHT = (92, 91, 99)
BONE = (226, 220, 204)
SCRAP_DARK = (116, 78, 38)
SCRAP = (170, 111, 48)
PALETTES = {
    "ferrous": ((176, 75, 52), (105, 43, 33)),
    "cupric": ((48, 132, 113), (29, 79, 68)),
}

APPROVED_SOURCE_RGBA_SHA256 = (
    "ee06a327d678858ae076d2fdf9d19839830d690db8d89c07ea65b28b80db736a"
)


def _rgba(color: Color, alpha: int = 255) -> tuple[int, int, int, int]:
    return (*color, alpha)


def _canvas() -> tuple[Image.Image, ImageDraw.ImageDraw]:
    image = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    return image, ImageDraw.Draw(image)


def _polygon(
    draw: ImageDraw.ImageDraw,
    points: tuple[tuple[int, int], ...],
    color: Color,
) -> None:
    draw.polygon(points, fill=_rgba(color))


def _finish(image: Image.Image) -> Image.Image:
    alpha = image.getchannel("A")
    grown = alpha.filter(ImageFilter.MaxFilter(3))
    edge = ImageChops.subtract(grown, alpha)
    shifted = ImageChops.subtract(
        edge,
        edge.transform(edge.size, Image.AFFINE, (1, 0, -1, 0, 1, -1)),
    )
    rim = Image.new("RGBA", image.size, (255, 244, 224, 0))
    rim.putalpha(shifted.point(lambda value: min(value, 105)))
    out = image.copy()
    out.alpha_composite(rim)
    return out


def _track(
    draw: ImageDraw.ImageDraw,
    box: tuple[int, int, int, int],
    phase: int,
    accent: Color,
) -> None:
    x0, y0, x1, y1 = box
    draw.rounded_rectangle(box, radius=6, fill=_rgba(BLACK))
    draw.rounded_rectangle(
        (x0 + 3, y0 + 3, x1 - 3, y1 - 3),
        radius=4,
        fill=_rgba(IRON_DEEP),
    )
    span = max(12, y1 - y0 - 13)
    for index in range(9):
        y = y0 + 5 + (index * 9 + phase * 4) % span
        if y + 3 >= y1 - 2:
            continue
        color = IRON_LIGHT if (index + phase) % 2 == 0 else IRON
        draw.rectangle((x0 + 2, y, x1 - 2, y + 3), fill=_rgba(color))
    draw.rectangle((x0 + 5, y0 + 15, x1 - 5, y0 + 21), fill=_rgba(accent))
    draw.line(
        (x0 + 4, y0 + 5, x0 + 4, y1 - 6),
        fill=_rgba(IRON_LIGHT),
        width=2,
    )


def _base(draw: ImageDraw.ImageDraw, phase: int, accent: Color) -> None:
    _track(draw, (8, 28, 30, 117), phase, accent)
    _track(draw, (98, 28, 120, 117), phase, accent)
    _polygon(
        draw,
        ((25, 34), (40, 22), (88, 22), (103, 34), (97, 110), (31, 110)),
        BLACK,
    )
    _polygon(
        draw,
        ((32, 38), (44, 30), (84, 30), (96, 38), (90, 102), (38, 102)),
        IRON_DARK,
    )


def _hopper(draw: ImageDraw.ImageDraw, accent: Color) -> None:
    _polygon(draw, ((37, 72), (91, 72), (86, 109), (42, 109)), BLACK)
    _polygon(draw, ((42, 77), (86, 77), (82, 104), (46, 104)), IRON_DEEP)
    draw.line((42, 78, 86, 78), fill=_rgba(accent), width=3)


def _cargo_bar_frame(draw: ImageDraw.ImageDraw) -> None:
    draw.rectangle((43, 91, 85, 104), fill=_rgba(BLACK))
    draw.rectangle((46, 94, 82, 101), fill=_rgba(IRON_DEEP))


def _conveyor(draw: ImageDraw.ImageDraw, work_phase: int) -> None:
    draw.rounded_rectangle((53, 37, 75, 79), radius=3, fill=_rgba(BLACK))
    draw.rectangle((56, 40, 72, 76), fill=_rgba(IRON_DEEP))
    for y in range(43, 76, 9):
        offset = (work_phase * 3 if work_phase else 0) % 6
        draw.line((57, y + offset, 71, y + offset), fill=_rgba(IRON_LIGHT), width=2)


def render_excavator(
    faction: str,
    move_phase: int = 0,
    work_phase: int = 0,
) -> Image.Image:
    """Render one approved milling-drum chassis frame."""
    if faction not in PALETTES:
        raise ValueError(f"unknown faction: {faction}")
    if work_phase not in range(ACTION_COUNT + 1):
        raise ValueError(f"unknown Excavator work phase: {work_phase}")
    image, draw = _canvas()
    primary, dark = PALETTES[faction]
    _base(draw, move_phase % 3, dark)
    _hopper(draw, dark)
    _cargo_bar_frame(draw)
    _conveyor(draw, work_phase)
    drum_y = 18 + (3 if work_phase in (1, 2) else 0)
    draw.rounded_rectangle(
        (18, drum_y - 8, 110, drum_y + 17), radius=8, fill=_rgba(BLACK)
    )
    draw.rounded_rectangle(
        (24, drum_y - 3, 104, drum_y + 12), radius=6, fill=_rgba(IRON)
    )
    draw.rectangle((30, drum_y + 1, 40, drum_y + 8), fill=_rgba(primary))
    draw.rectangle((88, drum_y + 1, 98, drum_y + 8), fill=_rgba(primary))
    for x in range(26, 104, 10):
        tooth = 5 if (x // 10 + work_phase) % 2 == 0 else 2
        _polygon(
            draw,
            ((x, drum_y - 3), (x + 5, drum_y - 3 - tooth), (x + 8, drum_y - 3)),
            BONE,
        )
    draw.line((28, 37, 43, 58), fill=_rgba(IRON_LIGHT), width=6)
    draw.line((100, 37, 85, 58), fill=_rgba(IRON_LIGHT), width=6)
    if work_phase == 2:
        for x, y in ((49, 26), (64, 20), (79, 28), (58, 34), (72, 37)):
            draw.rectangle((x, y, x + 4, y + 3), fill=_rgba(SCRAP))
    elif work_phase == 3:
        for x, y in ((59, 48), (66, 59), (61, 69)):
            draw.rectangle((x, y, x + 4, y + 3), fill=_rgba(SCRAP))
    return _finish(image)


def render_cargo_meter(level: int) -> Image.Image:
    """Render one aligned load-fraction layer, matching the Harvester."""
    if level not in range(CARGO_LEVELS):
        raise ValueError(f"unknown Excavator cargo level: {level}")
    image, draw = _canvas()
    fill_width = round(36 * level / (CARGO_LEVELS - 1))
    if fill_width:
        draw.rectangle((46, 94, 46 + fill_width, 101), fill=_rgba(SCRAP_DARK))
        draw.line(
            (47, 95, max(47, 45 + fill_width), 95),
            fill=_rgba(SCRAP),
            width=2,
        )
    return image


def source_rgba_digest() -> str:
    """Digest the approved chassis frames in installation order."""
    digest = hashlib.sha256()
    for faction in ("ferrous", "cupric"):
        states = (
            ("idle", 0, 0),
            ("move1", 1, 0),
            ("move2", 2, 0),
            *((f"action{action}", 0, action) for action in range(1, ACTION_COUNT + 1)),
        )
        for label, move_phase, work_phase in states:
            digest.update(f"excavator/{faction}/{label}".encode())
            digest.update(render_excavator(faction, move_phase, work_phase).tobytes())
    return digest.hexdigest()


def _put(registry: Registry, out: Path, key: str, image: Image.Image) -> None:
    native = image.convert("RGBA")
    native.save(out / f"{key}.png")
    registry[key] = native


def install_excavator(registry: Registry, out: Path) -> None:
    """Install candidate 423 and its exact cargo layers into production."""
    out.mkdir(parents=True, exist_ok=True)
    for faction in ("ferrous", "cupric"):
        _put(registry, out, f"excavator_{faction}", render_excavator(faction))
        for move_phase in (1, 2):
            _put(
                registry,
                out,
                f"excavator_{faction}_move{move_phase}",
                render_excavator(faction, move_phase=move_phase),
            )
        for work_phase in range(1, ACTION_COUNT + 1):
            _put(
                registry,
                out,
                f"excavator_{faction}_action{work_phase}",
                render_excavator(faction, work_phase=work_phase),
            )
    for level in range(CARGO_LEVELS):
        _put(registry, out, f"excavator_cargo{level}", render_cargo_meter(level))
