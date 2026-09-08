"""Shared aircraft materials with preserved flying-wing and split-wing silhouettes."""

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
MOTH_ACTION_COUNT = 6
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


def _polygon(draw, points, color):
    draw.polygon(points, fill=_rgba(color))


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


def _engine(draw, bounds, phase, accent):
    x0, y0, x1, y1 = bounds
    draw.rounded_rectangle(bounds, radius=5, fill=_rgba(BLACK))
    draw.rounded_rectangle(
        (x0 + 2, y0 + 2, x1 - 2, y1 - 2), radius=3, fill=_rgba(IRON_DARK)
    )
    draw.rectangle((x0 + 5, y0 + 10, x1 - 5, y1 - 10), fill=_rgba(accent))
    draw.line((x0 + 4, y0 + 3, x1 - 4, y0 + 3), fill=_rgba(IRON_LIGHT), width=1)
    draw.rectangle((x0 + 6, y0 + 5, x1 - 6, y0 + 8), fill=_rgba(IRON_DEEP))
    for offset in (-4, 0, 4):
        x = (x0 + x1) / 2 + offset
        draw.line(
            (x, y1 - 8, x, y1 - 4),
            fill=_rgba(IRON_LIGHT if (offset // 4 + phase) % 2 else IRON),
            width=1,
        )


def capsule(draw, x, y):
    draw.rounded_rectangle((x - 4, y - 7, x + 4, y + 7), radius=3, fill=_rgba(BLACK))
    draw.rounded_rectangle(
        (x - 2, y - 5, x + 2, y + 5), radius=2, fill=_rgba((116, 112, 103))
    )
    draw.line((x - 1, y - 3, x - 1, y + 3), fill=_rgba((157, 149, 133)), width=1)
    draw.line((x - 3, y + 5, x + 3, y + 5), fill=_rgba(IRON_LIGHT), width=1)


def moth_racks(draw, action):
    loaded = {0: 6, 1: 0, 2: 0, 3: 0, 4: 2, 5: 4, 6: 6}[action]
    for x in (49, 79):
        draw.rounded_rectangle((x - 8, 34, x + 8, 85), radius=3, fill=_rgba(BLACK))
        draw.rectangle((x - 4, 38, x + 4, 81), fill=_rgba(IRON_DEEP))
        draw.line((x - 6, 38, x - 6, 81), fill=_rgba(IRON), width=1)
        draw.line((x + 6, 38, x + 6, 81), fill=_rgba(IRON), width=1)
    positions = [(49, 45), (79, 45), (49, 59), (79, 59), (49, 73), (79, 73)]
    for slot, (x, y) in enumerate(positions):
        if slot < loaded:
            capsule(draw, x, y)
        elif action == 1:
            draw.rectangle((x - 3, y - 6, x + 3, y + 6), fill=(0, 0, 0, 0))
        else:
            draw.line((x - 3, y, x + 3, y), fill=_rgba(IRON_DARK), width=1)
    if action in (1, 2):
        for x in (39, 89):
            draw.line((x, 40, x, 78), fill=_rgba(IRON_LIGHT), width=1)


def nose_bay(
    image: Image.Image,
    draw: ImageDraw.ImageDraw,
    state: str,
    paint: tuple[int, int, int],
) -> None:
    # An uninterrupted dorsal keel replaces the old opening on the back.
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


def render_condor(faction: str, state: str = "idle") -> Image.Image:
    """Preserve the flying wing and route its payload through split nose doors."""
    if faction not in PALETTES:
        raise ValueError(f"unknown faction: {faction}")
    if state not in CONDOR_STATES:
        raise ValueError(f"unknown Condor state: {state}")
    image, draw = _canvas(CONDOR_SIZE)
    palette = PALETTES[faction]
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


def render_moth(
    faction: str,
    move_phase: int = 0,
    action: int = 0,
) -> Image.Image:
    """Render one approved split-bay Moth frame."""
    if faction not in PALETTES:
        raise ValueError(f"unknown faction: {faction}")
    if action not in range(MOTH_ACTION_COUNT + 1):
        raise ValueError(f"unknown Moth action: {action}")
    image, draw = _canvas()
    primary, dark = PALETTES[faction].base, PALETTES[faction].dark
    _polygon(
        draw,
        (
            (8, 42),
            (37, 20),
            (53, 18),
            (64, 34),
            (75, 18),
            (91, 20),
            (120, 42),
            (111, 87),
            (83, 70),
            (76, 108),
            (64, 99),
            (52, 108),
            (45, 70),
            (17, 87),
        ),
        BLACK,
    )
    _polygon(
        draw,
        (
            (17, 44),
            (41, 28),
            (50, 28),
            (64, 45),
            (78, 28),
            (87, 28),
            (111, 44),
            (104, 76),
            (79, 61),
            (70, 94),
            (64, 88),
            (58, 94),
            (49, 61),
            (24, 76),
        ),
        IRON_DARK,
    )
    _engine(draw, (15, 40, 35, 83), move_phase % 3, dark)
    _engine(draw, (93, 40, 113, 83), move_phase % 3, dark)
    draw.rectangle((36, 31, 52, 39), fill=_rgba(primary))
    draw.rectangle((76, 31, 92, 39), fill=_rgba(primary))
    moth_racks(draw, action)
    for side in (-1, 1):
        _polygon(
            draw,
            tuple(
                (64 + side * x, y) for x, y in ((23, 25), (36, 34), (30, 38), (19, 32))
            ),
            IRON,
        )
        draw.line(
            [(64 + side * 24, 25), (64 + side * 36, 34)],
            fill=_rgba(IRON_LIGHT),
            width=1,
        )
    return _finish(image)
