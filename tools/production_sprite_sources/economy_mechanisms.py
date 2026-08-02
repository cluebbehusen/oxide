"""Native production renderer for the finalized Open Works Reclaimer."""

import math

from PIL import Image, ImageDraw

from tools.gen_sprites import (
    FACTIONS,
    IRON,
    IRON_DARK,
    SCRAP_DARK,
    canvas,
    rim_light,
    s,
)

SIZE = 64
PAL = FACTIONS["ferrous"]


def _native(img: Image.Image) -> Image.Image:
    return rim_light(img.resize((SIZE, SIZE), Image.Resampling.LANCZOS))


def _open_conveyor_drum(d: ImageDraw.ImageDraw, phase: int) -> None:
    d.rounded_rectangle(
        (s(5), s(7), s(59), s(57)),
        radius=s(6),
        outline=(*IRON_DARK, 255),
        width=s(6),
    )
    d.rectangle((s(12), s(10), s(22), s(53)), fill=(*IRON, 255))
    d.rectangle((s(42), s(10), s(52), s(53)), fill=(*IRON, 255))
    belt_shift = (0, 5, 10, 5)[phase]
    for y in range(12, 52, 8):
        cy = 12 + (y - 12 + belt_shift) % 40
        d.rectangle(
            (s(25), s(cy), s(39), s(min(52, cy + 4))),
            fill=(*SCRAP_DARK, 255),
        )
    d.ellipse((s(18), s(24), s(46), s(52)), fill=(*IRON_DARK, 255))
    for tooth in range(7):
        angle = tooth * math.tau / 7 + phase * 0.42
        x = 32 + 11 * math.cos(angle)
        y = 38 + 11 * math.sin(angle)
        d.rectangle(
            (s(x - 3), s(y - 3), s(x + 3), s(y + 3)),
            fill=(*PAL["dark"], 255),
        )
    d.ellipse((s(27), s(33), s(37), s(43)), fill=(10, 10, 12, 255))


def render_reclaimer(phase: int) -> Image.Image:
    image, draw = canvas(SIZE)
    _open_conveyor_drum(draw, phase % 4)
    return _native(image)
