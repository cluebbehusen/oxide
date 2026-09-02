"""Approved production frames for Shrike and Sylph interceptors."""

from __future__ import annotations

import hashlib
from collections.abc import Callable
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

Registry = dict[str, Image.Image]
Renderer = Callable[[str, str], Image.Image]
Color = tuple[int, int, int]
Box = tuple[int, int, int, int]
Point = tuple[int, int]

SIZE = 64
STATES = ("idle", "move1", "move2", "ready", "report", "recover1", "recover2")
ACTION_STATES = ("ready", "report", "recover1", "recover2")

BLACK: Color = (13, 13, 17)
IRON_DEEP: Color = (25, 25, 31)
IRON_DARK: Color = (38, 38, 46)
IRON: Color = (52, 52, 62)
IRON_LIGHT: Color = (72, 72, 84)
BONE: Color = (232, 228, 216)
SCRAP: Color = (217, 164, 65)
SCRAP_DARK: Color = (140, 106, 47)
SCRAP_LIGHT: Color = (240, 200, 120)

PALETTES: dict[str, dict[str, Color]] = {
    "ferrous": {
        "base": (196, 87, 59),
        "dark": (126, 56, 38),
        "light": (232, 137, 107),
    },
    "cupric": {
        "base": (63, 148, 130),
        "dark": (39, 96, 79),
        "light": (119, 196, 176),
    },
}

APPROVED_SOURCE_RGBA_SHA256 = (
    "666091c5346eb48450ae10516eb285ff7f562e4a5b466270e72d655ae7757025"
)


def _rgba(color: Color, alpha: int = 255) -> tuple[int, int, int, int]:
    return (*color, alpha)


def _canvas() -> tuple[Image.Image, ImageDraw.ImageDraw]:
    image = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    return image, ImageDraw.Draw(image)


def _plate(
    draw: ImageDraw.ImageDraw,
    box: Box,
    fill: Color,
    *,
    radius: int = 2,
    inset: int = 2,
) -> None:
    draw.rounded_rectangle(box, radius=radius + 1, fill=_rgba(BLACK))
    x0, y0, x1, y1 = box
    draw.rounded_rectangle(
        (x0 + inset, y0 + inset, x1 - inset, y1 - inset),
        radius=max(1, radius),
        fill=_rgba(fill),
    )


def _rail(
    draw: ImageDraw.ImageDraw,
    points: tuple[Point, ...],
    color: Color,
    width: int,
) -> None:
    draw.line(points, fill=_rgba(BLACK), width=width + 2, joint="curve")
    draw.line(points, fill=_rgba(color), width=width, joint="curve")


def _bolt(draw: ImageDraw.ImageDraw, point: Point) -> None:
    x, y = point
    draw.rectangle((x - 1, y - 1, x + 1, y + 1), fill=_rgba(BLACK))
    draw.point((x, y), fill=_rgba(IRON_LIGHT))


def _engine_pod(
    draw: ImageDraw.ImageDraw,
    box: Box,
    palette: dict[str, Color],
    phase: int,
    *,
    armored: bool,
) -> None:
    x0, y0, x1, y1 = box
    _plate(draw, box, IRON_DARK if armored else IRON, radius=3)
    intake_h = 5 if armored else 4
    draw.rectangle((x0 + 3, y0 + 3, x1 - 3, y0 + intake_h), fill=_rgba(BLACK))
    draw.line(
        (x0 + 4, y0 + intake_h - 1, x1 - 4, y0 + intake_h - 1),
        fill=_rgba(IRON_LIGHT),
        width=1,
    )
    vent_top = y1 - (8 if armored else 7)
    draw.rectangle((x0 + 3, vent_top, x1 - 3, y1 - 3), fill=_rgba(IRON_DEEP))
    for index in range(3):
        y = vent_top + 1 + index * 2
        color = palette["light"] if index == phase % 3 else palette["dark"]
        draw.line((x0 + 4, y, x1 - 4, y), fill=_rgba(color), width=1)


def _sequence_lights(
    draw: ImageDraw.ImageDraw,
    points: tuple[Point, ...],
    palette: dict[str, Color],
    phase: int,
) -> None:
    for index, (x, y) in enumerate(points):
        active = index == phase % len(points)
        draw.rectangle((x - 1, y - 1, x + 1, y + 1), fill=_rgba(BLACK))
        draw.point((x, y), fill=_rgba(palette["light"] if active else palette["dark"]))


def _state_phase(state: str) -> int:
    return {"idle": 0, "move1": 1, "move2": 2}.get(state, 0)


def _weapon_state(state: str) -> tuple[int, Color, bool]:
    return {
        "idle": (0, IRON_DARK, False),
        "move1": (0, IRON_DARK, False),
        "move2": (0, IRON_DARK, False),
        "ready": (-2, SCRAP_DARK, False),
        "report": (4, SCRAP_LIGHT, True),
        "recover1": (2, SCRAP, False),
        "recover2": (0, IRON_DARK, False),
    }[state]


def _muzzle_flash(draw: ImageDraw.ImageDraw, x: int, y: int, *, wide: bool) -> None:
    reach = 6 if wide else 4
    draw.polygon(
        (
            (x, y - reach),
            (x + 2, y - 2),
            (x + reach, y),
            (x + 2, y + 1),
            (x, y + 4),
            (x - 2, y + 1),
            (x - reach, y),
            (x - 2, y - 2),
        ),
        fill=_rgba(SCRAP_LIGHT, 235),
    )
    draw.rectangle((x - 1, y - 2, x + 1, y + 1), fill=_rgba(BONE))


def _finish(image: Image.Image) -> Image.Image:
    alpha = image.getchannel("A")
    grown = alpha.filter(ImageFilter.MaxFilter(3))
    edge = ImageChops.subtract(grown, alpha)
    shifted = ImageChops.subtract(
        edge,
        edge.transform(edge.size, Image.AFFINE, (1, 0, -1, 0, 1, -1)),
    )
    rim = Image.new("RGBA", image.size, (255, 244, 224, 0))
    rim.putalpha(shifted.point(lambda value: min(value, 110)))
    out = image.copy()
    out.alpha_composite(rim)
    return out


def render_shrike(faction: str, state: str = "idle") -> Image.Image:
    """Render the fixed-wing hammerhead Shrike and its authored states."""
    if faction not in PALETTES:
        raise ValueError(f"unknown faction: {faction}")
    if state not in STATES:
        raise ValueError(f"unknown Shrike state: {state}")
    image, draw = _canvas()
    palette = PALETTES[faction]
    phase = _state_phase(state)
    recoil, feed, report = _weapon_state(state)

    draw.polygon(
        (
            (29, 4),
            (35, 4),
            (40, 15),
            (59, 22),
            (57, 36),
            (46, 35),
            (52, 52),
            (42, 57),
            (32, 49),
            (22, 57),
            (12, 52),
            (18, 35),
            (7, 36),
            (5, 22),
            (24, 15),
        ),
        fill=_rgba(BLACK),
    )
    draw.polygon(
        ((8, 24), (26, 17), (29, 30), (17, 33), (9, 31)),
        fill=_rgba(palette["dark"]),
    )
    draw.polygon(
        ((56, 24), (38, 17), (35, 30), (47, 33), (55, 31)),
        fill=_rgba(palette["dark"]),
    )
    draw.rectangle((9, 24, 55, 31), fill=_rgba(IRON_DARK))
    draw.rectangle((13, 26, 51, 29), fill=_rgba(palette["base"]))
    for x in (13, 19, 45, 51):
        _bolt(draw, (x, 27))
    _engine_pod(draw, (14, 31, 27, 55), palette, phase, armored=True)
    _engine_pod(draw, (37, 31, 50, 55), palette, phase + 1, armored=True)
    _plate(draw, (25, 12, 39, 50), palette["base"], radius=3)
    draw.ellipse((26, 22, 38, 34), fill=_rgba(BLACK))
    draw.ellipse((29, 25, 35, 31), fill=_rgba(feed))
    for x0, x1 in ((18, 28), (36, 46)):
        _rail(draw, ((x0, 28), (x1, 28)), SCRAP_DARK, 2)
    barrel_top = 4 + recoil
    _rail(draw, ((32, 29 + recoil), (32, barrel_top)), IRON_LIGHT, 4)
    draw.rectangle((29, 33 + recoil, 35, 39 + recoil), fill=_rgba(BLACK))
    draw.rectangle((31, 34 + recoil, 33, 37 + recoil), fill=_rgba(feed))
    _sequence_lights(draw, ((16, 27), (22, 27), (28, 27)), palette, phase)
    _sequence_lights(draw, ((36, 27), (42, 27), (48, 27)), palette, phase + 1)
    if report:
        _muzzle_flash(draw, 32, max(4, barrel_top), wide=True)
    return _finish(image)


def render_sylph(faction: str, state: str = "idle") -> Image.Image:
    """Render the fixed-wing needle Sylph and its authored states."""
    if faction not in PALETTES:
        raise ValueError(f"unknown faction: {faction}")
    if state not in STATES:
        raise ValueError(f"unknown Sylph state: {state}")
    image, draw = _canvas()
    palette = PALETTES[faction]
    phase = _state_phase(state)
    recoil, feed, report = _weapon_state(state)

    draw.polygon(
        (
            (31, 2),
            (33, 2),
            (38, 19),
            (58, 45),
            (48, 48),
            (38, 39),
            (42, 58),
            (34, 55),
            (32, 62),
            (30, 55),
            (22, 58),
            (26, 39),
            (16, 48),
            (6, 45),
            (26, 19),
        ),
        fill=_rgba(BLACK),
    )
    draw.polygon(((8, 44), (27, 21), (28, 36), (17, 45)), fill=_rgba(palette["dark"]))
    draw.polygon(((56, 44), (37, 21), (36, 36), (47, 45)), fill=_rgba(palette["dark"]))
    draw.line((12, 43, 26, 28), fill=_rgba(palette["base"]), width=2)
    draw.line((52, 43, 38, 28), fill=_rgba(palette["base"]), width=2)
    _engine_pod(draw, (22, 37, 30, 57), palette, phase, armored=False)
    _engine_pod(draw, (34, 37, 42, 57), palette, phase + 1, armored=False)
    _plate(draw, (27, 13, 37, 51), palette["base"], radius=2)
    draw.rectangle((29, 19, 35, 33), fill=_rgba(IRON_DEEP))
    draw.rectangle((31, 21, 33, 31), fill=_rgba(feed))
    _rail(draw, ((32, 34 + recoil), (32, 3 + recoil)), IRON_LIGHT, 2)
    draw.rectangle((29, 34 + recoil, 35, 39 + recoil), fill=_rgba(BLACK))
    draw.rectangle((31, 35 + recoil, 33, 37 + recoil), fill=_rgba(feed))
    _sequence_lights(draw, ((32, 43), (32, 47), (32, 51)), palette, phase)
    if report:
        _muzzle_flash(draw, 32, max(3, 3 + recoil), wide=False)
    return _finish(image)


def source_rgba_digest() -> str:
    """Digest every approved production frame in installation order."""
    digest = hashlib.sha256()
    for faction in ("ferrous", "cupric"):
        for stem, renderer in (("shrike", render_shrike), ("sylph", render_sylph)):
            for state in STATES:
                digest.update(f"{stem}/{faction}/{state}".encode())
                digest.update(renderer(faction, state).tobytes())
    return digest.hexdigest()


def _put(registry: Registry, out: Path, key: str, image: Image.Image) -> None:
    native = image.convert("RGBA")
    native.save(out / f"{key}.png")
    registry[key] = native


def _install_unit(
    registry: Registry,
    out: Path,
    faction: str,
    stem: str,
    renderer: Renderer,
) -> None:
    _put(registry, out, f"{stem}_{faction}", renderer(faction, "idle"))
    for phase in (1, 2):
        _put(
            registry,
            out,
            f"{stem}_{faction}_move{phase}",
            renderer(faction, f"move{phase}"),
        )
    for index, state in enumerate(ACTION_STATES, start=1):
        _put(
            registry,
            out,
            f"{stem}_{faction}_action{index}",
            renderer(faction, state),
        )


def install_shrike_sylph(registry: Registry, out: Path) -> None:
    """Install both approved interceptor rows into the production bank."""
    out.mkdir(parents=True, exist_ok=True)
    for faction in PALETTES:
        _install_unit(registry, out, faction, "shrike", render_shrike)
        _install_unit(registry, out, faction, "sylph", render_sylph)
