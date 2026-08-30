"""Approved production frames for the Crucible's advanced ground units."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

Registry = dict[str, Image.Image]
Renderer = Callable[[str, int, int], Image.Image]
Color = tuple[int, int, int]

SIZE = 128

BLACK = (11, 11, 15)
IRON_DEEP = (27, 27, 34)
IRON_DARK = (42, 42, 50)
IRON = (62, 62, 72)
IRON_LIGHT = (92, 91, 99)
BONE = (226, 220, 204)
FLASH = (255, 220, 132)
SMOKE = (151, 145, 137)
PALETTES = {
    "ferrous": ((176, 75, 52), (105, 43, 33)),
    "cupric": ((48, 132, 113), (29, 79, 68)),
}

APPROVED_SOURCE_RGBA_SHA256 = (
    "ec628fce0c214eb50a438c44e2de991af76a54d128c80ce07e803ca766dcb1ed"
)


def _rgba(color: Color, alpha: int = 255) -> tuple[int, int, int, int]:
    return (*color, alpha)


def _canvas() -> tuple[Image.Image, ImageDraw.ImageDraw]:
    image = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    return image, ImageDraw.Draw(image)


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


def _polygon(
    draw: ImageDraw.ImageDraw,
    points: tuple[tuple[int, int], ...],
    color: Color,
) -> None:
    draw.polygon(points, fill=_rgba(color))


def _track(
    draw: ImageDraw.ImageDraw,
    box: tuple[int, int, int, int],
    phase: int,
    accent: Color,
) -> None:
    x0, y0, x1, y1 = box
    draw.rounded_rectangle(box, radius=7, fill=_rgba(BLACK))
    draw.rounded_rectangle(
        (x0 + 3, y0 + 3, x1 - 3, y1 - 3), radius=5, fill=_rgba(IRON_DEEP)
    )
    span = max(12, y1 - y0 - 13)
    for index in range(8):
        y = y0 + 5 + (index * 10 + phase * 4) % span
        if y + 4 >= y1 - 2:
            continue
        color = IRON_LIGHT if (index + phase) % 2 == 0 else IRON
        draw.rectangle((x0 + 2, y, x1 - 2, y + 4), fill=_rgba(color))
    draw.rectangle((x0 + 5, y0 + 15, x1 - 5, y0 + 23), fill=_rgba(accent))
    draw.line((x0 + 4, y0 + 5, x0 + 4, y1 - 6), fill=_rgba(IRON_LIGHT), width=2)


def _barrel(
    draw: ImageDraw.ImageDraw,
    *,
    x: int,
    front: int,
    rear: int,
    recoil: int,
    width: int,
) -> None:
    y0 = front + recoil
    y1 = rear + recoil
    draw.rounded_rectangle(
        (x - width // 2 - 2, y0, x + width // 2 + 2, y1),
        radius=3,
        fill=_rgba(BLACK),
    )
    draw.rectangle(
        (x - width // 2, y0 + 2, x + width // 2, y1 - 1),
        fill=_rgba(IRON_LIGHT),
    )
    draw.rectangle(
        (x - width // 2 + 3, y0 + 2, x + width // 2 - 3, y1 - 1),
        fill=_rgba(IRON),
    )
    draw.ellipse(
        (x - width // 2 - 2, y0 - 3, x + width // 2 + 2, y0 + 5),
        fill=_rgba(BLACK),
    )


def _muzzle_flash(draw: ImageDraw.ImageDraw, x: int, y: int) -> None:
    _polygon(
        draw,
        (
            (x, y - 10),
            (x + 5, y - 3),
            (x + 12, y),
            (x + 5, y + 4),
            (x, y + 10),
            (x - 5, y + 4),
            (x - 12, y),
            (x - 5, y - 3),
        ),
        FLASH,
    )
    draw.rectangle((x - 3, y - 4, x + 3, y + 4), fill=_rgba(BONE))


def _rocket(
    draw: ImageDraw.ImageDraw,
    *,
    x: int,
    y0: int,
    y1: int,
    width: int,
    flame: int = 0,
) -> None:
    half = width // 2
    _polygon(
        draw,
        (
            (x, y0 - 10),
            (x + half, y0 + 3),
            (x + half, y1 - 6),
            (x - half, y1 - 6),
            (x - half, y0 + 3),
        ),
        IRON_LIGHT,
    )
    draw.rectangle((x - half + 3, y0 + 5, x + half - 3, y1 - 8), fill=_rgba(IRON))
    draw.rectangle((x - 2, y0 + 5, x + 2, y1 - 8), fill=_rgba(BONE))
    _polygon(
        draw,
        ((x - half, y1 - 18), (x - half - 7, y1 - 5), (x - half, y1 - 7)),
        IRON_DARK,
    )
    _polygon(
        draw,
        ((x + half, y1 - 18), (x + half + 7, y1 - 5), (x + half, y1 - 7)),
        IRON_DARK,
    )
    if flame:
        _polygon(
            draw,
            ((x - 6, y1 - 5), (x, y1 + flame), (x + 6, y1 - 5)),
            FLASH,
        )
        _polygon(
            draw,
            (
                (x - 3, y1 - 4),
                (x, y1 + max(3, flame - 7)),
                (x + 3, y1 - 4),
            ),
            BONE,
        )


def render_breaker(faction: str, move_phase: int = 0, action: int = 0) -> Image.Image:
    """Render the twin-casemate Breaker and its authored motion states."""
    if faction not in PALETTES:
        raise ValueError(f"unknown faction: {faction}")
    if action not in range(5):
        raise ValueError(f"unknown Breaker action: {action}")
    image, draw = _canvas()
    palette = PALETTES[faction]
    _track(draw, (7, 18, 34, 116), move_phase % 3, palette[1])
    _track(draw, (94, 18, 121, 116), move_phase % 3, palette[1])
    _polygon(
        draw,
        ((28, 27), (43, 15), (85, 15), (100, 27), (94, 109), (34, 109)),
        BLACK,
    )
    _polygon(
        draw,
        ((34, 31), (46, 21), (82, 21), (94, 31), (88, 102), (40, 102)),
        IRON_DARK,
    )
    _polygon(
        draw,
        ((35, 31), (64, 13), (93, 31), (84, 46), (44, 46)),
        palette[1],
    )
    draw.rectangle((42, 76, 86, 103), fill=_rgba(palette[1]))
    draw.rectangle((50, 83, 78, 96), fill=_rgba(BLACK))
    recoil = (0, 1, 0, 10, 4)[action]
    draw.rounded_rectangle(
        (46, 34 + recoil, 82, 82 + recoil), radius=8, fill=_rgba(BLACK)
    )
    draw.rectangle((52, 39 + recoil, 76, 76 + recoil), fill=_rgba(IRON))
    draw.rectangle((44, 55 + recoil, 84, 64 + recoil), fill=_rgba(IRON_LIGHT))
    _barrel(draw, x=64, front=3, rear=59, recoil=recoil, width=16)
    if action == 2:
        _muzzle_flash(draw, 64, 3)
    return _finish(image)


def _artillery_base(
    faction: str, move_phase: int, *, outriggers: bool
) -> tuple[Image.Image, ImageDraw.ImageDraw]:
    image, draw = _canvas()
    palette = PALETTES[faction]
    for box in (
        (8, 27, 29, 66),
        (99, 27, 120, 66),
        (8, 79, 29, 117),
        (99, 79, 120, 117),
    ):
        _track(draw, box, move_phase % 3, palette[1])
    _polygon(
        draw,
        ((23, 31), (39, 20), (89, 20), (105, 31), (102, 109), (26, 109)),
        BLACK,
    )
    _polygon(
        draw,
        ((29, 35), (43, 27), (85, 27), (99, 35), (95, 102), (33, 102)),
        IRON_DARK,
    )
    draw.rectangle((37, 85, 91, 103), fill=_rgba(palette[1]))
    draw.rectangle((45, 91, 83, 98), fill=_rgba(BLACK))
    if outriggers:
        for x in (7, 121):
            draw.line(
                (31 if x < 64 else 97, 72, x, 72), fill=_rgba(IRON_LIGHT), width=5
            )
            draw.rectangle((max(1, x - 5), 66, min(126, x + 5), 78), fill=_rgba(BLACK))
    return image, draw


def render_avalanche(faction: str, move_phase: int = 0, action: int = 0) -> Image.Image:
    """Render the single-payload Avalanche and its authored motion states."""
    if faction not in PALETTES:
        raise ValueError(f"unknown faction: {faction}")
    if action not in range(5):
        raise ValueError(f"unknown Avalanche action: {action}")
    image, draw = _artillery_base(faction, move_phase, outriggers=action > 0)
    palette = PALETTES[faction]
    draw.ellipse((38, 39, 90, 91), fill=_rgba(BLACK))
    draw.ellipse((45, 46, 83, 84), fill=_rgba(IRON))
    rail_shift = 0 if action < 2 else -3 if action == 2 else 0
    draw.rounded_rectangle((42, 18, 86, 96), radius=7, fill=_rgba(BLACK))
    draw.rectangle((47, 23, 81, 91), fill=_rgba(IRON_DEEP))
    draw.line((48, 77, 38, 101), fill=_rgba(IRON_LIGHT), width=6)
    draw.line((80, 77, 90, 101), fill=_rgba(IRON_LIGHT), width=6)
    draw.rectangle((47, 76, 81, 84), fill=_rgba(palette[1]))
    if action != 3:
        _rocket(
            draw,
            x=64,
            y0=7 + rail_shift,
            y1=88 + rail_shift,
            width=20,
            flame=19 if action == 2 else 0,
        )
    else:
        _polygon(draw, ((58, 18), (64, 2), (70, 18)), SMOKE)
    return _finish(image)


def _put(registry: Registry, out: Path, key: str, image: Image.Image) -> None:
    image.save(out / f"{key}.png")
    registry[key] = image


def _install_unit(
    registry: Registry,
    out: Path,
    faction: str,
    stem: str,
    renderer: Renderer,
) -> None:
    _put(registry, out, f"{stem}_{faction}", renderer(faction, 0, 0))
    for phase in (1, 2):
        _put(
            registry,
            out,
            f"{stem}_{faction}_move{phase}",
            renderer(faction, phase, 0),
        )
    for action in range(1, 5):
        _put(
            registry,
            out,
            f"{stem}_{faction}_action{action}",
            renderer(faction, 0, action),
        )


def install_crucible_units(registry: Registry, out: Path) -> None:
    """Install both advanced ground-unit sprite rows."""
    for faction in PALETTES:
        _install_unit(registry, out, faction, "breaker", render_breaker)
        _install_unit(registry, out, faction, "avalanche", render_avalanche)
