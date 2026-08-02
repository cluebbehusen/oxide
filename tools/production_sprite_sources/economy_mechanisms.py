"""Native production renderer for the finalized open-drum Reclaimer."""

from PIL import Image, ImageDraw

from tools.gen_sprites import (
    FACTIONS,
    IRON,
    IRON_DARK,
    IRON_LIGHT,
    canvas,
    rim_light,
    s,
)

SIZE = 64
PAL = FACTIONS["ferrous"]


def _native(img: Image.Image) -> Image.Image:
    return rim_light(img.resize((SIZE, SIZE), Image.Resampling.LANCZOS))


def _open_auger(d: ImageDraw.ImageDraw, phase: int) -> None:
    d.rounded_rectangle(
        (s(5), s(6), s(59), s(58)), radius=s(6), outline=(*IRON_DARK, 255), width=s(6)
    )
    for x in (12, 46):
        d.rounded_rectangle(
            (s(x), s(9), s(x + 7), s(55)), radius=s(2), fill=(*IRON, 255)
        )
        for index in range(5):
            y = 11 + (index * 9 + phase * 3) % 42
            d.rectangle(
                (s(x + 1), s(y), s(x + 6), s(min(54, y + 3))), fill=(*PAL["dark"], 255)
            )
    d.rounded_rectangle((s(21), s(6), s(43), s(58)), radius=s(8), fill=(9, 9, 11, 255))
    d.rectangle((s(29), s(8), s(35), s(52)), fill=(*IRON_DARK, 255))
    d.rectangle((s(31), s(8), s(33), s(52)), fill=(*IRON, 255))
    d.polygon([(s(25), s(49)), (s(39), s(49)), (s(32), s(60))], fill=(*IRON_LIGHT, 255))
    reach = (9, 4, -9, -4)[phase]
    for index, y in enumerate((15, 29, 43)):
        handed_reach = reach if index % 2 == 0 else -reach
        d.polygon(
            [
                (s(22), s(y + 2)),
                (s(32 + handed_reach), s(y - 3)),
                (s(42), s(y + 2)),
                (s(32 - handed_reach), s(y + 7)),
            ],
            fill=(*IRON_LIGHT, 255),
        )
        d.line(
            [(s(23), s(y + 3)), (s(32 + handed_reach), s(y - 2)), (s(41), s(y + 3))],
            fill=(*PAL["dark"], 255),
            width=s(2),
        )
    d.ellipse([s(27), s(5), s(37), s(15)], fill=(*IRON_DARK, 255))
    d.ellipse([s(30), s(8), s(34), s(12)], fill=(*PAL["light"], 255))


def render_reclaimer(phase: int) -> Image.Image:
    image, draw = canvas(SIZE)
    _open_auger(draw, phase % 4)
    return _native(image)
