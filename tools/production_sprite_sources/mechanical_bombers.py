"""The Condor in shared aircraft materials, keeping its flying-wing silhouette."""

from PIL import Image, ImageDraw

from tools import gen_sprites as gen
from tools.production_sprite_sources.tender_condor_final import PALETTES

BLACK = (11, 11, 15)
IRON_DEEP = (24, 25, 31)
IRON_DARK = gen.IRON_DARK
IRON = gen.IRON
IRON_LIGHT = gen.IRON_LIGHT
CONDOR_SIZE = 128
CONDOR_STATES = ("idle", "crack", "open", "release", "recover")
SS = 4


class ScaledDraw:
    def __init__(self, image):
        self.draw = ImageDraw.Draw(image)

    def __getattr__(self, name):
        def paint(coords, **kwargs):
            if coords and isinstance(coords[0], (tuple, list)):
                scaled = [(round(x * SS), round(y * SS)) for x, y in coords]
            else:
                scaled = tuple(round(v * SS) for v in coords)
            for key in ("width", "radius"):
                if key in kwargs:
                    kwargs[key] = round(kwargs[key] * SS)
            return getattr(self.draw, name)(scaled, **kwargs)

        return paint


def _rgba(color, alpha=255):
    return (*color, alpha)


def _canvas(size=128):
    image = Image.new("RGBA", (size * SS, size * SS), (0, 0, 0, 0))
    return image, ScaledDraw(image)


def _finish(image):
    return gen.rim_light(image.resize((128, 128), Image.Resampling.LANCZOS))


def condor_panels(draw):
    for side in (-1, 1):
        draw.line(
            [(64 + side * 8, 26), (64 + side * 39, 58)],
            fill=_rgba((115, 114, 123)),
            width=1,
        )
        draw.line(
            [(64 + side * 31, 65), (64 + side * 20, 68), (64 + side * 13, 56)],
            fill=_rgba(IRON_DEEP),
            width=1,
        )
    for x in (43, 79):
        draw.rectangle((x, 49, x + 6, 67), fill=_rgba(BLACK))
        draw.line((x + 1, 50, x + 5, 50), fill=_rgba(IRON_LIGHT), width=1)
        draw.rectangle((x + 2, 53, x + 4, 64), fill=_rgba(IRON))


def nose_bay(
    image: Image.Image,
    draw: ImageDraw.ImageDraw,
    state: str,
    paint: tuple[int, int, int],
) -> None:
    # An uninterrupted dorsal keel keeps the back closed.
    draw.line((64, 43, 64, 70), fill=_rgba(IRON_DEEP), width=2)
    spread = {"idle": 0, "crack": 0, "open": 6, "release": 6, "recover": 3}[state]
    # The aperture reaches the leading edge, allowing the separate payload
    # underneath the airframe to emerge instead of appearing on its roof.
    draw.rectangle((56, 19, 72, 36), fill=_rgba(BLACK))
    draw.rectangle((60, 19, 68, 31), fill=(0, 0, 0, 0))
    for side in (-1, 1):
        points = [
            (64 + side * x + side * spread, y)
            for x, y in ((1, 19), (8, 19), (12, 32), (4, 36), (1, 29))
        ]
        draw.polygon(points, fill=_rgba(IRON_DARK))
        draw.line(
            [(64 + side * (x + spread), y) for x, y in ((7, 21), (10, 30))],
            fill=_rgba(IRON_LIGHT),
            width=1,
        )
        draw.line(
            (64 + side * (2 + spread), 21, 64 + side * (2 + spread), 28),
            fill=_rgba(paint),
            width=2,
        )
    if not spread:
        draw.line((64, 19, 64, 30), fill=_rgba(BLACK), width=1)


def render_condor(variant: str, state: str = "idle") -> Image.Image:
    """Preserve the flying wing and route its payload through split nose doors."""
    if variant not in PALETTES:
        raise ValueError(f"unknown variant: {variant}")
    if state not in CONDOR_STATES:
        raise ValueError(f"unknown Condor state: {state}")
    image, draw = _canvas(CONDOR_SIZE)
    palette = PALETTES[variant]
    outer = (
        (56, 19),
        (72, 19),
        (119, 62),
        (112, 82),
        (92, 77),
        (81, 94),
        (64, 83),
        (47, 94),
        (36, 77),
        (16, 82),
        (9, 62),
    )
    inner = (
        (58, 24),
        (70, 24),
        (112, 62),
        (107, 76),
        (89, 71),
        (78, 87),
        (64, 78),
        (50, 87),
        (39, 71),
        (21, 76),
        (16, 62),
    )
    draw.polygon(outer, fill=_rgba(BLACK))
    draw.polygon(inner, fill=_rgba(IRON_DEEP))
    draw.polygon(((60, 25), (24, 62), (43, 66), (58, 54)), fill=_rgba(IRON))
    draw.polygon(((68, 25), (104, 62), (85, 66), (70, 54)), fill=_rgba(IRON))
    draw.polygon(
        ((56, 30), (64, 22), (72, 30), (73, 72), (64, 80), (55, 72)),
        fill=_rgba(IRON_DARK),
    )
    for x in (43, 79):
        draw.rectangle((x, 49, x + 6, 67), fill=_rgba(BLACK))
        draw.rectangle((x + 2, 52, x + 4, 65), fill=_rgba(IRON_LIGHT))
    draw.rectangle((58, 31, 61, 54), fill=_rgba(palette.dark))
    draw.rectangle((67, 31, 70, 54), fill=_rgba(palette.dark))
    condor_panels(draw)
    nose_bay(image, draw, state, palette.dark)
    return _finish(image)
