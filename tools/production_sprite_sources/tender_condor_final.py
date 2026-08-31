"""Production native frames for the armored welder and strategic bomber.

The Condor preserves its reviewed code-native geometry exactly. The Tender
keeps the approved armored-toolbox silhouette and repair sequence, with its
idle treatment restrained to the established ground-machine family. Keep both
renderers independent of the review workspace so a clean checkout can
reproduce every promoted frame.
"""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

Registry = dict[str, Image.Image]
Color = tuple[int, int, int]
Point = tuple[int, int]
Box = tuple[int, int, int, int]

TENDER_SIZE = 64
CONDOR_SIZE = 128

BLACK = (11, 11, 15)
IRON_DEEP = (28, 28, 35)
IRON_DARK = (42, 42, 50)
IRON = (62, 62, 72)
IRON_LIGHT = (92, 91, 99)
BONE = (226, 220, 204)
SCRAP = (191, 139, 61)
SCRAP_DARK = (116, 78, 38)
WELD = (255, 220, 132)


@dataclass(frozen=True)
class Palette:
    base: Color
    dark: Color


PALETTES = {
    "ferrous": Palette((176, 75, 52), (105, 43, 33)),
    "cupric": Palette((48, 132, 113), (29, 79, 68)),
}

TENDER_STATES = ("idle", "deploy", "contact", "weld", "recover")
CONDOR_STATES = ("idle", "crack", "open", "release", "recover")

# Semantic RGBA digest of the original approved Tender 301 frames for both
# factions and Condor 305's Ferrous frames, retained as the production control.
ORIGINAL_APPROVED_SOURCE_RGBA_SHA256 = (
    "16cecca7a8851ba4aaf55a587fcb19e8fde3fbfc4d3f3e08932c24a24134cdeb"
)

# Semantic RGBA digest of the current production Tender frames for both
# factions and Condor 305's Ferrous frames, in the state order declared above.
PRODUCTION_SOURCE_RGBA_SHA256 = (
    "6bf645624c18b4c531872cd4bb5d04640987bd19e99760639f1de3e1b673b421"
)


def _rgba(color: Color, alpha: int = 255) -> tuple[int, int, int, int]:
    return (*color, alpha)


def _canvas(size: int) -> tuple[Image.Image, ImageDraw.ImageDraw]:
    image = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    return image, ImageDraw.Draw(image)


def _finish_condor(image: Image.Image) -> Image.Image:
    alpha = image.getchannel("A")
    grown = alpha.filter(ImageFilter.MaxFilter(3))
    edge = ImageChops.subtract(grown, alpha)
    rim = Image.new("RGBA", image.size, _rgba(BONE, 0))
    rim.putalpha(edge)
    rim.alpha_composite(image)
    return rim


def _finish_ground_unit(image: Image.Image) -> Image.Image:
    """Use the finalized ground roster's restrained top-left rim."""
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


def _line(
    draw: ImageDraw.ImageDraw,
    points: Sequence[Point],
    color: Color,
    width: int,
) -> None:
    draw.line(points, fill=_rgba(color), width=width, joint="curve")


def _track(
    draw: ImageDraw.ImageDraw,
    box: Box,
    move_phase: int,
) -> None:
    x0, y0, x1, y1 = box
    draw.rounded_rectangle(box, radius=3, fill=_rgba(BLACK))
    draw.rounded_rectangle(
        (x0 + 2, y0 + 2, x1 - 2, y1 - 2), radius=2, fill=_rgba(IRON_DEEP)
    )
    span = max(4, y1 - y0 - 8)
    for index in range(5):
        y = y0 + 3 + (index * 8 + move_phase * 3) % span
        color = IRON_LIGHT if (index + move_phase) % 2 == 0 else IRON
        draw.rectangle((x0 + 2, y, x1 - 2, min(y + 3, y1 - 2)), fill=_rgba(color))


def _service_reel(
    draw: ImageDraw.ImageDraw,
    point: Point,
    *,
    active: bool,
    radius: int = 6,
) -> None:
    x, y = point
    draw.ellipse((x - radius, y - radius, x + radius, y + radius), fill=_rgba(BLACK))
    draw.ellipse(
        (x - radius + 2, y - radius + 2, x + radius - 2, y + radius - 2),
        fill=_rgba(IRON_DEEP),
    )
    draw.rectangle(
        (x - radius + 2, y - 1, x + radius - 2, y + 1),
        fill=_rgba(SCRAP_DARK if active else IRON_DARK),
    )
    draw.rectangle(
        (x - 1, y - 1, x + 1, y + 1),
        fill=_rgba(IRON_LIGHT if active else IRON_DARK),
    )


def _vent(draw: ImageDraw.ImageDraw, box: Box) -> None:
    x0, y0, x1, y1 = box
    draw.rectangle(box, fill=_rgba(IRON_DEEP))
    center_y = (y0 + y1) // 2
    draw.rectangle((x0 + 2, center_y, x1 - 2, center_y + 1), fill=_rgba(IRON_DARK))


def _joint(draw: ImageDraw.ImageDraw, point: Point, palette: Palette) -> None:
    x, y = point
    draw.ellipse((x - 3, y - 3, x + 3, y + 3), fill=_rgba(BLACK))
    draw.rectangle((x - 1, y - 1, x + 1, y + 1), fill=_rgba(palette.dark))


def _arm(
    draw: ImageDraw.ImageDraw,
    points: Sequence[Point],
    palette: Palette,
    *,
    active: bool,
    welding: bool,
) -> None:
    _line(draw, points, BLACK, 6)
    _line(draw, points, IRON_LIGHT if active else IRON, 3)
    for point in points[:-1]:
        _joint(draw, point, palette)
    tip_x, tip_y = points[-1]
    draw.rectangle((tip_x - 3, tip_y - 2, tip_x + 3, tip_y + 2), fill=_rgba(BLACK))
    draw.rectangle((tip_x - 1, tip_y - 3, tip_x + 1, tip_y), fill=_rgba(SCRAP_DARK))
    if welding:
        for x0, y0, x1, y1 in (
            (tip_x - 5, tip_y - 5, tip_x + 5, tip_y + 5),
            (tip_x + 5, tip_y - 5, tip_x - 5, tip_y + 5),
            (tip_x, tip_y - 7, tip_x, tip_y + 4),
        ):
            draw.line((x0, y0, x1, y1), fill=_rgba(WELD))
        draw.rectangle((tip_x - 1, tip_y - 1, tip_x + 1, tip_y + 1), fill=_rgba(BONE))


def _hose(
    draw: ImageDraw.ImageDraw,
    start: Point,
    arm_points: Sequence[Point],
) -> None:
    elbow = arm_points[-2]
    tip = arm_points[-1]
    points = (start, (start[0] + 2, start[1] - 8), elbow, tip)
    _line(draw, points, BLACK, 3)
    _line(draw, points, SCRAP_DARK, 1)


def render_tender(
    faction: str,
    state: str = "idle",
    move_phase: int = 0,
) -> Image.Image:
    """Render one approved 64x64 Tender frame."""
    if faction not in PALETTES:
        raise ValueError(f"unknown faction: {faction}")
    if state not in TENDER_STATES:
        raise ValueError(f"unknown Tender state: {state}")
    image, draw = _canvas(TENDER_SIZE)
    palette = PALETTES[faction]
    active = state != "idle"
    _track(draw, (7, 17, 19, 58), move_phase % 3)
    _track(draw, (45, 17, 57, 58), move_phase % 3)
    draw.rounded_rectangle((13, 15, 51, 56), radius=6, fill=_rgba(BLACK))
    draw.polygon(
        (
            (17, 21),
            (21, 18),
            (43, 18),
            (47, 21),
            (47, 51),
            (43, 54),
            (21, 54),
            (17, 51),
        ),
        fill=_rgba(IRON_DARK),
    )
    draw.polygon(((20, 22), (44, 22), (45, 36), (19, 36)), fill=_rgba(IRON))
    draw.rectangle((20, 25, 44, 33), fill=_rgba(palette.dark))
    draw.rectangle((23, 27, 41, 31), fill=_rgba(palette.base))
    _service_reel(draw, (25, 45), active=active, radius=6)
    _vent(draw, (35, 41, 44, 50))
    paths = (
        ((40, 24), (46, 20), (39, 16)),
        ((40, 24), (42, 15), (35, 9)),
        ((40, 24), (39, 12), (32, 4)),
        ((40, 24), (39, 11), (32, 3)),
        ((40, 24), (42, 15), (35, 9)),
    )
    arm = paths[TENDER_STATES.index(state)]
    if active:
        _hose(draw, (30, 44), arm)
    _arm(draw, arm, palette, active=active, welding=state == "weld")
    return _finish_ground_unit(image)


def _bay(
    draw: ImageDraw.ImageDraw,
    center_x: int,
    top: int,
    bottom: int,
    state: str,
    *,
    width: int,
) -> None:
    if state == "idle":
        draw.rectangle((center_x - 1, top, center_x + 1, bottom), fill=_rgba(IRON_DEEP))
        return
    opening = 2 if state in {"crack", "recover"} else width
    draw.rectangle(
        (center_x - opening, top, center_x + opening, bottom), fill=_rgba(BLACK)
    )
    draw.line(
        (center_x - opening - 2, top, center_x - opening - 2, bottom),
        fill=_rgba(IRON_LIGHT),
        width=2,
    )
    draw.line(
        (center_x + opening + 2, top, center_x + opening + 2, bottom),
        fill=_rgba(IRON_LIGHT),
        width=2,
    )
    if state == "open":
        draw.rounded_rectangle(
            (center_x - 3, top + 4, center_x + 3, top + 13),
            radius=2,
            fill=_rgba(SCRAP_DARK),
        )
        draw.rectangle(
            (center_x - 1, top + 2, center_x + 1, top + 4), fill=_rgba(SCRAP)
        )


def render_condor(faction: str, state: str = "idle") -> Image.Image:
    """Render one approved 128x128 Condor frame."""
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
    _bay(draw, 64, 47, 72, state, width=6)
    return _finish_condor(image)


def _put(registry: Registry, out: Path, key: str, image: Image.Image) -> None:
    image.save(out / f"{key}.png")
    registry[key] = image


def install_tender_condor(registry: Registry, out: Path) -> None:
    """Install the approved idle, locomotion, and action rows."""
    tender_actions = ("deploy", "contact", "weld", "recover")
    condor_actions = ("crack", "open", "release", "recover")
    for faction in PALETTES:
        _put(registry, out, f"tender_{faction}", render_tender(faction))
        for phase in (1, 2):
            _put(
                registry,
                out,
                f"tender_{faction}_move{phase}",
                render_tender(faction, move_phase=phase),
            )
        for index, state in enumerate(tender_actions, start=1):
            _put(
                registry,
                out,
                f"tender_{faction}_action{index}",
                render_tender(faction, state),
            )

        idle = render_condor(faction)
        _put(registry, out, f"condor_{faction}", idle)
        for phase in (1, 2):
            _put(registry, out, f"condor_{faction}_move{phase}", idle.copy())
        for index, state in enumerate(condor_actions, start=1):
            _put(
                registry,
                out,
                f"condor_{faction}_action{index}",
                render_condor(faction, state),
            )
