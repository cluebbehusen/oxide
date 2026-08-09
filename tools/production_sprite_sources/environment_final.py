"""Finalized quarry environment sprites for the production atlas.

This module is deliberately self-contained: review cards and exploratory
generators are not production inputs.  The public installer writes the
approved field debris, ground blockers, and rock library, then replaces the
old hazard-striped Peak tiles with connected quarry mesas.
"""

from __future__ import annotations

import math
import random
from dataclasses import dataclass
from pathlib import Path

from PIL import Image, ImageDraw

from tools import gen_sprites as gen

Registry = dict[str, Image.Image]
Color = tuple[int, int, int]
Boulder = tuple[float, float, float, bool]

TILE = 64
SS = 4

FIELD_DEBRIS_KEYS = (
    "field_debris_severed_cable",
    "field_debris_bent_service_rail",
    "field_debris_abandoned_canisters",
    "field_debris_braided_cable",
    "field_debris_fastener_spill",
    "field_debris_canister_cluster",
    "field_debris_sunken_cooling_fan",
    "field_debris_inspection_hatch",
    "field_debris_cable_tray",
    "field_debris_motor_casing",
)

GROUND_BLOCKER_KEYS = (
    "ground_blocker_cooling_fan",
    "ground_blocker_compressor_skid",
    "ground_blocker_transformer_bank",
    "ground_blocker_exposed_gearbox",
    "ground_blocker_conveyor_drive",
    "ground_blocker_crusher_motor",
    "ground_blocker_track_assembly",
    "ground_blocker_vent_blower",
    "ground_blocker_generator_pallet",
)

GROUND_BLOCKER_FOOTPRINTS = (
    (2, 2),
    (2, 1),
    (3, 2),
    (2, 2),
    (3, 2),
    (3, 2),
    (3, 1),
    (2, 2),
    (2, 2),
)

ROCK_KEYS = (
    "rock_0",
    "rock_1",
    "rock_2",
    "rock_3",
    "rock_jagged_crown",
    "rock_split_anvil",
    "rock_broken_tooth",
    "rock_hooked_shelf",
    "rock_three_spall",
    "rock_quarry_bite",
    "rock_fallen_fang",
    "rock_split_face",
    "rock_low_scree",
    "rock_hollow_crook",
    "rock_broken_rampart",
    "rock_twin_outcrop",
    "rock_quarry_fall",
    "rock_shattered_crown",
    "rock_split_gully",
    "rock_collapsed_cut",
    "rock_jagged_field",
    "rock_broken_bench",
    "rock_quarry_scatter",
)

ROCK_FOOTPRINTS = (
    *((1, 1),) * 14,
    *((2, 1),) * 5,
    *((3, 1),) * 4,
)


def _rgba(color: Color, alpha: int = 255) -> tuple[int, int, int, int]:
    return (*color, alpha)


def _s(value: float) -> int:
    return round(value * SS)


def _box(
    box: tuple[int | float, int | float, int | float, int | float],
) -> tuple[int, int, int, int]:
    return tuple(_s(value) for value in box)  # type: ignore[return-value]


def _points(
    points: tuple[tuple[int | float, int | float], ...],
) -> list[tuple[int, int]]:
    return [(_s(x), _s(y)) for x, y in points]


def _canvas(width: int, height: int) -> tuple[Image.Image, ImageDraw.ImageDraw]:
    image = Image.new("RGBA", (width * SS, height * SS), (0, 0, 0, 0))
    return image, ImageDraw.Draw(image)


def _finish(image: Image.Image, width: int, height: int) -> Image.Image:
    return image.resize((width, height), Image.Resampling.LANCZOS)


def _put(registry: Registry, out: Path, key: str, image: Image.Image) -> None:
    image = image.convert("RGBA")
    image.save(out / f"{key}.png")
    registry[key] = image
    print(f"  {key}.png")


def _flat_shadow(
    draw: ImageDraw.ImageDraw, box: tuple[int, int, int, int], alpha: int = 80
) -> None:
    x0, y0, x1, y1 = box
    draw.rounded_rectangle(
        _box((x0 + 1, y0 + 3, x1 + 2, y1 + 4)),
        radius=_s(3),
        fill=(9, 9, 12, alpha),
    )


def _raised_plinth(
    draw: ImageDraw.ImageDraw,
    box: tuple[int, int, int, int],
    *,
    accent: Color,
) -> None:
    x0, y0, x1, y1 = box
    draw.rounded_rectangle(
        _box((x0 + 3, y0 + 6, x1 + 5, y1 + 8)),
        radius=_s(5),
        fill=(7, 7, 10, 180),
    )
    draw.rounded_rectangle(
        _box((x0, y0, x1, y1)), radius=_s(5), fill=_rgba(gen.IRON_DARK)
    )
    draw.rounded_rectangle(
        _box((x0 + 3, y0 + 3, x1 - 3, y1 - 5)),
        radius=_s(3),
        fill=_rgba(gen.IRON),
    )
    draw.rectangle(_box((x0 + 4, y1 - 7, x1 - 4, y1 - 2)), fill=_rgba((22, 22, 27)))
    draw.line(
        _points(((x0 + 5, y0 + 4), (x1 - 5, y0 + 4))),
        fill=_rgba(gen.IRON_LIGHT),
        width=_s(1),
    )
    draw.line(
        _points(((x0 + 7, y1 - 5), (x1 - 7, y1 - 5))),
        fill=_rgba(accent),
        width=_s(2),
    )
    for x, y in (
        (x0 + 7, y0 + 7),
        (x1 - 7, y0 + 7),
        (x0 + 7, y1 - 9),
        (x1 - 7, y1 - 9),
    ):
        draw.ellipse(
            _box((x - 1.5, y - 1.5, x + 1.5, y + 1.5)),
            fill=_rgba(gen.BONE, 180),
        )


def _raised_skids(
    draw: ImageDraw.ImageDraw,
    box: tuple[int, int, int, int],
    *,
    accent: Color,
) -> None:
    x0, y0, x1, y1 = box
    draw.rounded_rectangle(
        _box((x0 + 3, y0 + 7, x1 + 5, y1 + 9)),
        radius=_s(7),
        fill=(7, 7, 10, 175),
    )
    for y in (y0 + 4, y1 - 10):
        draw.rounded_rectangle(
            _box((x0, y, x1, y + 8)), radius=_s(3), fill=_rgba(gen.IRON_DARK)
        )
        draw.line(
            _points(((x0 + 5, y + 2), (x1 - 5, y + 2))),
            fill=_rgba(gen.IRON_LIGHT),
            width=_s(1),
        )
        draw.line(
            _points(((x0 + 8, y + 6), (x1 - 8, y + 6))),
            fill=_rgba(accent),
            width=_s(2),
        )
    for x in (x0 + 10, x1 - 10):
        draw.rectangle(_box((x - 3, y0 + 9, x + 3, y1 - 8)), fill=_rgba(gen.IRON_DARK))


def _raised_wreck_frame(
    draw: ImageDraw.ImageDraw,
    box: tuple[int, int, int, int],
    *,
    accent: Color,
) -> None:
    x0, y0, x1, y1 = box
    shadow = (
        (x0 + 4, y0 + 11),
        (x1 - 8, y0 + 5),
        (x1 + 4, y1 - 4),
        (x0 + 12, y1 + 9),
    )
    draw.polygon(_points(shadow), fill=(7, 7, 10, 180))
    frame = (
        (x0, y0 + 7),
        (x1 - 11, y0),
        (x1, y1 - 12),
        (x0 + 8, y1),
    )
    draw.line(
        _points(frame + (frame[0],)),
        fill=_rgba(gen.IRON_DARK),
        width=_s(9),
        joint="curve",
    )
    draw.line(
        _points(frame + (frame[0],)),
        fill=_rgba(gen.IRON_LIGHT),
        width=_s(2),
        joint="curve",
    )
    draw.line(
        _points(((x0 + 10, y1 - 6), (x1 - 14, y1 - 13))),
        fill=_rgba(accent),
        width=_s(3),
    )


def _beveled_panel(
    draw: ImageDraw.ImageDraw,
    box: tuple[int, int, int, int],
    *,
    fill: Color = gen.IRON,
    accent: Color | None = None,
) -> None:
    x0, y0, x1, y1 = box
    draw.rounded_rectangle(_box(box), radius=_s(3), fill=_rgba(gen.IRON_DARK))
    draw.rounded_rectangle(
        _box((x0 + 2, y0 + 2, x1 - 2, y1 - 2)),
        radius=_s(2),
        fill=_rgba(fill),
    )
    draw.line(
        _points(((x0 + 3, y0 + 3), (x1 - 3, y0 + 3))),
        fill=_rgba(gen.IRON_LIGHT),
        width=_s(1),
    )
    if accent is not None:
        draw.rectangle(_box((x0 + 4, y1 - 6, x1 - 4, y1 - 3)), fill=_rgba(accent))


def _fan(
    draw: ImageDraw.ImageDraw,
    center: tuple[int, int],
    radius: int,
    *,
    raised: bool,
) -> None:
    cx, cy = center
    if raised:
        draw.ellipse(
            _box((cx - radius - 3, cy - radius - 1, cx + radius + 3, cy + radius + 5)),
            fill=_rgba(gen.IRON_DARK),
        )
    draw.ellipse(
        _box((cx - radius, cy - radius, cx + radius, cy + radius)),
        fill=_rgba((17, 17, 22)),
        outline=_rgba(gen.IRON_LIGHT),
        width=_s(2),
    )
    blades = (
        ((2, -2), (radius - 4, -7), (radius - 2, 2), (4, 4)),
        ((2, 2), (7, radius - 4), (-2, radius - 2), (-4, 4)),
        ((-2, 2), (-radius + 4, 7), (-radius + 2, -2), (-4, -4)),
        ((-2, -2), (-7, -radius + 4), (2, -radius + 2), (4, -4)),
    )
    for blade in blades:
        points = tuple((cx + x, cy + y) for x, y in blade)
        draw.polygon(_points(points), fill=_rgba(gen.FACTIONS["ferrous"]["dark"]))
        draw.line(
            _points((points[0], points[1])),
            fill=_rgba(gen.FACTIONS["ferrous"]["light"]),
            width=_s(1),
        )
    draw.ellipse(_box((cx - 4, cy - 4, cx + 4, cy + 4)), fill=_rgba(gen.IRON_LIGHT))
    draw.ellipse(_box((cx - 2, cy - 2, cx + 2, cy + 2)), fill=_rgba(gen.SCRAP_DARK))


def _legacy_debris(index: int) -> Image.Image:
    image = Image.new("RGBA", (32, 32), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    rust = gen.FACTIONS["ferrous"]["dark"]
    if index == 0:
        draw.arc((4, 7, 27, 28), 170, 345, fill=_rgba(gen.IRON_LIGHT), width=2)
    elif index == 1:
        draw.line((5, 20, 10, 10, 22, 9, 27, 18), fill=_rgba(gen.IRON), width=3)
    elif index == 5:
        for x in (8, 16, 24):
            draw.rectangle((x - 3, 10, x + 2, 24), fill=_rgba(gen.IRON_DARK))
            draw.line((x - 2, 12, x + 1, 12), fill=_rgba(rust))
    else:
        raise ValueError(f"unsupported finalized legacy debris {index}")
    return image


def _refined_debris(index: int) -> Image.Image:
    image, draw = _canvas(64, 64)
    rust = gen.FACTIONS["ferrous"]["dark"]
    patina = gen.FACTIONS["cupric"]["dark"]
    if index == 0:
        _flat_shadow(draw, (7, 15, 57, 48), 36)
        draw.arc(
            _box((8, 11, 55, 53)), 165, 350, fill=_rgba(gen.IRON_DARK), width=_s(6)
        )
        draw.arc(
            _box((8, 11, 55, 53)), 165, 350, fill=_rgba(gen.IRON_LIGHT), width=_s(2)
        )
        for x, y, color in (
            (9, 33, gen.SCRAP),
            (11, 36, patina),
            (53, 25, rust),
            (55, 28, gen.SCRAP_DARK),
        ):
            draw.line(
                _points(((x, y), (x - 5 if x < 20 else x + 5, y + 5))),
                fill=_rgba(color),
                width=_s(2),
            )
    elif index == 2:
        for x, y, angle in (
            (13, 17, 0),
            (27, 13, 1),
            (43, 20, 0),
            (19, 38, 1),
            (39, 42, 0),
            (50, 35, 1),
        ):
            draw.ellipse(_box((x - 4, y - 4, x + 4, y + 4)), fill=_rgba(gen.IRON_DARK))
            draw.ellipse(_box((x - 2, y - 2, x + 2, y + 2)), fill=_rgba(gen.IRON_LIGHT))
            line = (
                ((x - 1, y - 2), (x + 1, y + 2)) if angle else ((x - 2, y), (x + 2, y))
            )
            draw.line(_points(line), fill=_rgba(rust), width=_s(1))
    elif index == 3:
        for x, y, tilt in ((16, 30, -2), (32, 27, 1), (47, 34, 0)):
            draw.rounded_rectangle(
                _box((x - 7 + tilt, y - 14, x + 6 + tilt, y + 15)),
                radius=_s(3),
                fill=_rgba(gen.IRON_DARK),
            )
            draw.rectangle(
                _box((x - 5 + tilt, y - 10, x + 4 + tilt, y + 10)),
                fill=_rgba(gen.IRON),
            )
            draw.line(
                _points(((x - 4 + tilt, y - 5), (x + 3 + tilt, y - 5))),
                fill=_rgba(rust if x != 32 else patina),
                width=_s(2),
            )
            draw.rectangle(
                _box((x - 3 + tilt, y - 16, x + 2 + tilt, y - 12)),
                fill=_rgba(gen.IRON_LIGHT),
            )
    else:
        raise ValueError(f"unsupported finalized refined debris {index}")
    return _finish(image, 64, 64)


def _new_debris(index: int) -> Image.Image:
    image, draw = _canvas(64, 64)
    rust = gen.FACTIONS["ferrous"]["dark"]
    patina = gen.FACTIONS["cupric"]["dark"]
    if index == 0:
        draw.rounded_rectangle(
            _box((7, 8, 57, 56)), radius=_s(5), fill=_rgba(gen.GROUND_DARK)
        )
        draw.line(_points(((12, 12), (52, 12))), fill=_rgba(gen.IRON_DARK), width=_s(2))
        _fan(draw, (32, 32), 18, raised=False)
        for x, y in ((11, 12), (53, 12), (11, 52), (53, 52)):
            draw.ellipse(_box((x - 2, y - 2, x + 2, y + 2)), fill=_rgba(gen.IRON_LIGHT))
    elif index == 1:
        draw.polygon(
            _points(((16, 8), (48, 8), (57, 19), (53, 51), (14, 55), (7, 43), (9, 18))),
            fill=_rgba(gen.IRON_DARK),
        )
        draw.polygon(
            _points(
                ((18, 12), (46, 12), (52, 21), (49, 47), (17, 50), (12, 40), (13, 21))
            ),
            fill=_rgba(gen.IRON),
        )
        draw.line(_points(((18, 17), (46, 44))), fill=_rgba(patina), width=_s(3))
        for x, y in ((18, 17), (46, 17), (18, 45), (46, 45)):
            draw.ellipse(_box((x - 2, y - 2, x + 2, y + 2)), fill=_rgba(gen.BONE, 160))
    elif index == 3:
        draw.polygon(
            _points(((5, 20), (54, 12), (59, 40), (11, 49))),
            fill=_rgba(gen.IRON_DARK),
        )
        draw.polygon(
            _points(((9, 22), (51, 16), (54, 37), (13, 45))),
            fill=_rgba(gen.GROUND_DARK),
        )
        for offset in range(0, 41, 7):
            draw.line(
                _points(
                    ((12 + offset, 20 - offset // 7), (15 + offset, 44 - offset // 7))
                ),
                fill=_rgba(gen.IRON_LIGHT),
                width=_s(1),
            )
        draw.arc(_box((9, 16, 53, 47)), 165, 350, fill=_rgba(rust), width=_s(3))
        draw.arc(_box((12, 15, 56, 44)), 170, 345, fill=_rgba(patina), width=_s(2))
    elif index == 5:
        draw.rounded_rectangle(
            _box((8, 17, 55, 48)), radius=_s(7), fill=_rgba(gen.IRON_DARK)
        )
        draw.rounded_rectangle(
            _box((13, 20, 50, 44)), radius=_s(5), fill=_rgba(gen.IRON)
        )
        for x in range(17, 49, 6):
            draw.line(
                _points(((x, 22), (x - 2, 42))),
                fill=_rgba(gen.GROUND_DARK),
                width=_s(2),
            )
        draw.ellipse(_box((40, 25, 55, 40)), fill=_rgba(gen.IRON_DARK))
        draw.ellipse(_box((44, 29, 51, 36)), fill=_rgba(patina))
        draw.line(_points(((15, 24), (23, 18))), fill=_rgba(rust), width=_s(2))
    else:
        raise ValueError(f"unsupported finalized new debris {index}")
    return _finish(image, 64, 64)


def _field_debris() -> tuple[Image.Image, ...]:
    return (
        _legacy_debris(0),
        _legacy_debris(1),
        _legacy_debris(5),
        _refined_debris(0),
        _refined_debris(2),
        _refined_debris(3),
        _new_debris(0),
        _new_debris(1),
        _new_debris(3),
        _new_debris(5),
    )


def _ground_blocker(original_index: int) -> Image.Image:
    footprints = {
        0: (2, 2),
        1: (2, 1),
        2: (3, 2),
        5: (2, 2),
        6: (3, 2),
        9: (3, 2),
        11: (3, 1),
        13: (2, 2),
        15: (2, 2),
    }
    footprint = footprints[original_index]
    width = footprint[0] * TILE
    height = footprint[1] * TILE
    image, draw = _canvas(width, height)
    x0, y0, x1, y1 = 7, 7, width - 8, height - 10
    ferrous = gen.FACTIONS["ferrous"]
    cupric = gen.FACTIONS["cupric"]
    accent = ferrous["dark"] if original_index % 2 == 0 else cupric["dark"]
    if original_index in (0, 2, 5, 9, 13, 15):
        _raised_plinth(draw, (x0, y0, x1, y1), accent=accent)
    elif original_index in (1, 6):
        _raised_skids(draw, (x0, y0, x1, y1), accent=accent)
    else:
        _raised_wreck_frame(draw, (x0, y0, x1, y1), accent=accent)
    cx, cy = width // 2, height // 2 - 2

    if original_index == 0:
        _beveled_panel(
            draw,
            (18, 16, width - 18, height - 24),
            fill=(43, 44, 53),
            accent=cupric["dark"],
        )
        _fan(draw, (cx, cy - 2), min(width, height) // 3, raised=True)
        for x, y in (
            (22, 20),
            (width - 22, 20),
            (22, height - 28),
            (width - 22, height - 28),
        ):
            draw.rectangle(
                _box((x - 4, y - 4, x + 4, y + 4)), fill=_rgba(gen.IRON_DARK)
            )
    elif original_index == 1:
        _beveled_panel(
            draw,
            (16, 18, width - 16, height - 22),
            fill=gen.IRON,
            accent=ferrous["dark"],
        )
        draw.rounded_rectangle(
            _box((25, 20, 74, 46)), radius=_s(11), fill=_rgba(gen.IRON_DARK)
        )
        draw.rounded_rectangle(
            _box((29, 23, 70, 42)), radius=_s(8), fill=_rgba(ferrous["dark"])
        )
        for x in (79, 91, 103):
            draw.rectangle(_box((x, 22, x + 7, 45)), fill=_rgba(gen.IRON_LIGHT))
            draw.line(
                _points(((x + 2, 25), (x + 2, 42))),
                fill=_rgba(gen.GROUND_DARK),
                width=_s(2),
            )
    elif original_index == 2:
        for offset, panel_accent in (
            (17, ferrous["dark"]),
            (75, cupric["dark"]),
            (133, ferrous["dark"]),
        ):
            _beveled_panel(
                draw,
                (offset, 22, offset + 44, height - 29),
                fill=(46, 46, 55),
                accent=panel_accent,
            )
            for y in range(31, height - 38, 10):
                draw.line(
                    _points(((offset + 6, y), (offset + 38, y))),
                    fill=_rgba(gen.IRON_DARK),
                    width=_s(3),
                )
            for x in (offset + 13, offset + 31):
                draw.ellipse(_box((x - 4, 13, x + 4, 24)), fill=_rgba(gen.IRON_LIGHT))
    elif original_index == 5:
        _beveled_panel(draw, (18, 19, width - 18, height - 25), fill=(44, 44, 53))
        for gx, gy, radius in ((43, 52, 21), (83, 47, 17), (69, 82, 15)):
            draw.ellipse(
                _box((gx - radius, gy - radius, gx + radius, gy + radius)),
                fill=_rgba(gen.IRON_DARK),
            )
            for tooth in range(8):
                tx = gx + (
                    -radius - 3 if tooth == 0 else radius + 3 if tooth == 4 else 0
                )
                ty = gy + (
                    -radius - 3 if tooth == 2 else radius + 3 if tooth == 6 else 0
                )
                draw.rectangle(
                    _box((tx - 3, ty - 3, tx + 3, ty + 3)),
                    fill=_rgba(gen.IRON_LIGHT),
                )
            draw.ellipse(
                _box(
                    (gx - radius + 6, gy - radius + 6, gx + radius - 6, gy + radius - 6)
                ),
                fill=_rgba(ferrous["dark"]),
            )
            draw.ellipse(
                _box((gx - 4, gy - 4, gx + 4, gy + 4)), fill=_rgba(gen.SCRAP_DARK)
            )
    elif original_index == 6:
        _beveled_panel(draw, (17, 25, width - 17, height - 31), fill=(42, 42, 50))
        draw.rounded_rectangle(
            _box((27, 34, width - 76, height - 41)),
            radius=_s(10),
            fill=_rgba(gen.IRON_DARK),
        )
        for x in range(36, width - 83, 16):
            draw.line(
                _points(((x, 38), (x + 11, height - 45))),
                fill=_rgba(ferrous["dark"]),
                width=_s(4),
            )
        _fan(draw, (width - 48, cy), 25, raised=True)
    elif original_index == 9:
        _beveled_panel(
            draw,
            (20, 22, width - 20, height - 30),
            fill=gen.IRON,
            accent=ferrous["dark"],
        )
        draw.rounded_rectangle(
            _box((31, 29, width - 77, height - 39)),
            radius=_s(14),
            fill=_rgba(gen.IRON_DARK),
        )
        draw.rounded_rectangle(
            _box((38, 35, width - 84, height - 45)),
            radius=_s(10),
            fill=_rgba((52, 52, 61)),
        )
        for x in range(46, width - 91, 11):
            draw.line(
                _points(((x, 37), (x - 3, height - 47))),
                fill=_rgba(gen.GROUND_DARK),
                width=_s(3),
            )
        draw.ellipse(
            _box((width - 76, 32, width - 25, height - 38)),
            fill=_rgba(gen.IRON_DARK),
        )
        draw.ellipse(
            _box((width - 62, 45, width - 39, height - 51)),
            fill=_rgba(gen.SCRAP_DARK),
        )
    elif original_index == 11:
        draw.rounded_rectangle(
            _box((15, 17, width - 15, height - 17)),
            radius=_s(18),
            fill=_rgba(gen.IRON_DARK),
        )
        draw.rounded_rectangle(
            _box((24, 24, width - 24, height - 24)),
            radius=_s(12),
            fill=_rgba((20, 20, 25)),
        )
        for x in range(20, width - 20, 14):
            draw.rectangle(_box((x, 18, x + 9, height - 19)), fill=_rgba(gen.IRON))
            draw.line(
                _points(((x + 2, 21), (x + 7, height - 22))),
                fill=_rgba(gen.IRON_LIGHT),
                width=_s(2),
            )
        for x in (43, width - 43):
            draw.ellipse(
                _box((x - 14, 25, x + 14, height - 25)),
                fill=_rgba(gen.IRON_DARK),
            )
            draw.ellipse(
                _box((x - 6, cy - 6, x + 6, cy + 6)),
                fill=_rgba(ferrous["dark"]),
            )
    elif original_index == 13:
        _beveled_panel(
            draw,
            (17, 18, width - 17, height - 27),
            fill=(45, 45, 53),
            accent=cupric["dark"],
        )
        _fan(draw, (cx, cy - 3), 31, raised=True)
        draw.line(
            _points(((18, height - 34), (width - 18, height - 34))),
            fill=_rgba(ferrous["dark"]),
            width=_s(4),
        )
    elif original_index == 15:
        _beveled_panel(
            draw,
            (19, 18, width - 19, height - 27),
            fill=gen.IRON,
            accent=ferrous["dark"],
        )
        for x in (30, 64, 98):
            draw.rounded_rectangle(
                _box((x - 12, 26, x + 12, height - 37)),
                radius=_s(5),
                fill=_rgba(gen.IRON_DARK),
            )
            draw.rectangle(
                _box((x - 8, 31, x + 8, height - 43)),
                fill=_rgba(ferrous["dark"] if x != 64 else cupric["dark"]),
            )
            for y in range(35, height - 47, 9):
                draw.line(
                    _points(((x - 6, y), (x + 6, y))),
                    fill=_rgba(gen.IRON_LIGHT),
                    width=_s(2),
                )
    else:
        raise ValueError(f"unsupported finalized ground blocker {original_index}")

    return _finish(image, width, height)


def _ground_blockers() -> tuple[Image.Image, ...]:
    return tuple(_ground_blocker(index) for index in (0, 1, 2, 5, 6, 9, 11, 13, 15))


@dataclass(frozen=True)
class _RockSource:
    key: str
    footprint: tuple[int, int]
    palette: str
    seed: int
    boulders: tuple[Boulder, ...]


_ROCK_PALETTES: dict[str, tuple[Color, Color, Color]] = {
    "cool-slate": ((77, 80, 94), (53, 55, 68), (105, 109, 125)),
    "dusty-quarry": ((91, 86, 84), (63, 59, 60), (121, 114, 109)),
    "iron-stained": ((92, 75, 70), (62, 51, 53), (126, 102, 89)),
    "charcoal-slag": ((61, 62, 71), (40, 41, 50), (88, 90, 101)),
    "bleached-mineral": ((104, 101, 98), (71, 69, 72), (138, 134, 126)),
}


_ROCK_SOURCES = (
    _RockSource(
        "rock_jagged_crown",
        (1, 1),
        "cool-slate",
        151,
        (
            (30, 35, 23, False),
            (17, 47, 10, True),
            (47, 45, 12, False),
            (39, 20, 9, True),
        ),
    ),
    _RockSource(
        "rock_split_anvil",
        (1, 1),
        "dusty-quarry",
        179,
        ((23, 36, 21, False), (44, 34, 18, False), (33, 51, 9, True), (8, 46, 7, True)),
    ),
    _RockSource(
        "rock_broken_tooth",
        (1, 1),
        "iron-stained",
        211,
        ((31, 31, 25, False), (49, 47, 9, True), (15, 48, 8, True)),
    ),
    _RockSource(
        "rock_hooked_shelf",
        (1, 1),
        "charcoal-slag",
        257,
        (
            (26, 37, 22, False),
            (45, 25, 13, False),
            (50, 48, 8, True),
            (10, 50, 7, True),
        ),
    ),
    _RockSource(
        "rock_three_spall",
        (1, 1),
        "bleached-mineral",
        293,
        (
            (19, 40, 17, False),
            (39, 28, 20, False),
            (47, 49, 11, True),
            (10, 25, 7, True),
        ),
    ),
    _RockSource(
        "rock_quarry_bite",
        (1, 1),
        "dusty-quarry",
        337,
        (
            (20, 38, 19, False),
            (44, 40, 19, False),
            (32, 51, 10, True),
            (29, 20, 8, True),
        ),
    ),
    _RockSource(
        "rock_fallen_fang",
        (1, 1),
        "iron-stained",
        379,
        ((33, 35, 24, False), (16, 48, 9, True), (50, 22, 9, False)),
    ),
    _RockSource(
        "rock_split_face",
        (1, 1),
        "cool-slate",
        419,
        (
            (22, 32, 20, False),
            (43, 38, 19, False),
            (19, 51, 8, True),
            (48, 19, 7, True),
        ),
    ),
    _RockSource(
        "rock_low_scree",
        (1, 1),
        "charcoal-slag",
        461,
        (
            (22, 43, 18, False),
            (42, 40, 17, False),
            (31, 26, 14, False),
            (52, 51, 7, True),
            (9, 48, 6, True),
        ),
    ),
    _RockSource(
        "rock_hollow_crook",
        (1, 1),
        "bleached-mineral",
        503,
        (
            (25, 35, 22, False),
            (45, 24, 13, False),
            (47, 49, 10, True),
            (13, 52, 7, True),
        ),
    ),
    _RockSource(
        "rock_broken_rampart",
        (2, 1),
        "iron-stained",
        607,
        (
            (23, 39, 21, False),
            (47, 28, 17, False),
            (69, 43, 24, False),
            (101, 34, 19, False),
            (116, 50, 9, True),
            (9, 50, 8, True),
        ),
    ),
    _RockSource(
        "rock_twin_outcrop",
        (2, 1),
        "cool-slate",
        653,
        (
            (27, 33, 24, False),
            (49, 49, 12, True),
            (67, 45, 10, True),
            (94, 37, 22, False),
            (116, 25, 10, False),
            (112, 52, 8, True),
        ),
    ),
    _RockSource(
        "rock_quarry_fall",
        (2, 1),
        "dusty-quarry",
        701,
        (
            (19, 45, 17, True),
            (40, 32, 25, False),
            (69, 43, 18, False),
            (91, 29, 15, False),
            (109, 43, 18, False),
            (121, 54, 6, True),
        ),
    ),
    _RockSource(
        "rock_shattered_crown",
        (2, 1),
        "charcoal-slag",
        743,
        (
            (18, 42, 18, False),
            (39, 27, 21, False),
            (58, 45, 14, True),
            (80, 31, 23, False),
            (108, 44, 17, False),
            (120, 31, 8, True),
        ),
    ),
    _RockSource(
        "rock_split_gully",
        (2, 1),
        "bleached-mineral",
        797,
        (
            (24, 35, 24, False),
            (48, 48, 12, True),
            (64, 51, 9, True),
            (82, 48, 11, True),
            (104, 34, 23, False),
            (120, 51, 7, True),
        ),
    ),
    _RockSource(
        "rock_collapsed_cut",
        (3, 1),
        "dusty-quarry",
        859,
        (
            (18, 43, 18, True),
            (38, 30, 24, False),
            (61, 47, 15, True),
            (82, 36, 20, False),
            (105, 49, 14, True),
            (126, 31, 23, False),
            (151, 43, 19, False),
            (173, 28, 13, False),
            (184, 50, 8, True),
        ),
    ),
    _RockSource(
        "rock_jagged_field",
        (3, 1),
        "cool-slate",
        907,
        (
            (16, 46, 15, True),
            (35, 28, 22, False),
            (62, 42, 19, False),
            (81, 51, 9, True),
            (101, 31, 24, False),
            (129, 48, 15, True),
            (151, 36, 21, False),
            (177, 27, 11, False),
            (183, 51, 8, True),
        ),
    ),
    _RockSource(
        "rock_broken_bench",
        (3, 1),
        "iron-stained",
        953,
        (
            (22, 34, 23, False),
            (47, 49, 14, True),
            (68, 39, 18, False),
            (90, 26, 20, False),
            (110, 42, 14, True),
            (132, 43, 18, False),
            (155, 29, 22, False),
            (174, 43, 10, True),
        ),
    ),
    _RockSource(
        "rock_quarry_scatter",
        (3, 1),
        "charcoal-slag",
        1013,
        (
            (15, 50, 11, True),
            (33, 38, 21, False),
            (57, 27, 16, False),
            (72, 44, 14, True),
            (95, 40, 20, False),
            (116, 27, 15, False),
            (136, 44, 15, True),
            (158, 37, 22, False),
            (181, 46, 10, True),
        ),
    ),
)


def _mix_color(left: Color, right: Color, amount: int) -> Color:
    return tuple(
        (left[channel] * (100 - amount) + right[channel] * amount) // 100
        for channel in range(3)
    )  # type: ignore[return-value]


def _stone_colors(
    palette: tuple[Color, Color, Color], index: int, dark_body: bool
) -> tuple[Color, Color, Color]:
    body_color, dark_color, light_color = palette
    shift = (0, 8, -6, 4, -3)[index % 5]

    def adjusted(color: Color) -> Color:
        target = light_color if shift > 0 else dark_color
        return _mix_color(color, target, abs(shift))

    if dark_body:
        return (
            adjusted(dark_color),
            adjusted(body_color),
            _mix_color(adjusted(dark_color), (28, 28, 34), 10),
        )
    return adjusted(body_color), adjusted(light_color), adjusted(dark_color)


def _rock(source: _RockSource) -> Image.Image:
    width = source.footprint[0] * TILE
    height = source.footprint[1] * TILE
    image = Image.new("RGBA", (width * SS, height * SS), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    rng = random.Random(source.seed)
    palette = _ROCK_PALETTES[source.palette]
    for boulder_index, (cx, cy, radius, dark_body) in enumerate(source.boulders):
        points = []
        for index in range(9):
            angle = index / 9 * math.tau
            wobble = radius * (0.78 + 0.22 * rng.random())
            points.append(
                (cx + wobble * math.cos(angle), cy + wobble * math.sin(angle))
            )
        body, highlight, shade_color = _stone_colors(palette, boulder_index, dark_body)
        scaled = [(_s(x), _s(y)) for x, y in points]
        draw.polygon(scaled, fill=_rgba(body))
        facet = [
            (
                _s(x * 0.62 + cx * 0.38 - radius * 0.12),
                _s(y * 0.62 + cy * 0.38 - radius * 0.12),
            )
            for x, y in points[:5]
        ]
        draw.polygon(facet, fill=_rgba(highlight))
        shade = [
            (
                _s(x * 0.72 + cx * 0.28 + radius * 0.10),
                _s(y * 0.72 + cy * 0.28 + radius * 0.10),
            )
            for x, y in points[4:9]
        ]
        draw.polygon(shade, fill=_rgba(shade_color))
    return _finish(image, width, height)


def _peak_barrier(mask: int, variant: int) -> Image.Image:
    """Render one connected, uncut quarry mesa tile.

    The cardinal mask is fog-honest at the call site. Connected sides carry
    continuous stone to the edge; exposed sides reveal a tall cut face with
    sparse abandoned retaining hardware.
    """

    image, draw = _canvas(TILE, TILE)
    seed = 7_001 + mask * 97 + variant * 431
    rng = random.Random(seed)
    top = (51 + variant * 3, 50 + variant * 2, 59 + variant * 3)
    top_light = (70 + variant * 2, 67 + variant * 2, 76 + variant * 2)
    seam = (34, 34, 42)
    face = (29 + variant * 2, 29 + variant * 2, 36 + variant * 2)
    deep = (18, 19, 24)
    rust = (111, 62, 49)

    # The full-tile top prevents a Peak from reading as a passable object.
    draw.rectangle(_box((0, 0, 64, 64)), fill=_rgba(top))
    for band_y, shift in ((10, 3), (29, -3), (45, 2)):
        draw.polygon(
            _points(
                (
                    (0, band_y + 2),
                    (16, band_y - 1 + shift),
                    (35, band_y + 2),
                    (64, band_y - 2 - shift),
                    (64, band_y + 5),
                    (36, band_y + 8),
                    (17, band_y + 5),
                    (0, band_y + 8),
                )
            ),
            fill=_rgba(_mix_color(top, seam, 18 if shift < 0 else 10)),
        )

    # A south-facing cut is tallest in a top-down quarry. Side cuts are
    # narrower, while the north edge retains a bright cap and short drop.
    if not mask & 4:
        draw.polygon(
            _points(((0, 46), (17, 43), (37, 47), (64, 42), (64, 64), (0, 64))),
            fill=_rgba(face),
        )
        draw.line(
            _points(((0, 46), (17, 43), (37, 47), (64, 42))),
            fill=_rgba(top_light),
            width=_s(2),
        )
        for y, color in ((52, seam), (58, deep)):
            draw.line(_points(((2, y), (62, y - 3))), fill=_rgba(color), width=_s(2))
    if not mask & 1:
        draw.polygon(
            _points(((0, 0), (64, 0), (61, 8), (42, 6), (21, 9), (3, 7))),
            fill=_rgba(face),
        )
        draw.line(
            _points(((3, 7), (21, 9), (42, 6), (61, 8))),
            fill=_rgba(top_light),
            width=_s(2),
        )
    if not mask & 8:
        draw.polygon(
            _points(((0, 0), (8, 4), (6, 23), (10, 43), (7, 64), (0, 64))),
            fill=_rgba(deep),
        )
        draw.line(
            _points(((8, 4), (6, 23), (10, 43), (7, 64))),
            fill=_rgba(top_light),
            width=_s(2),
        )
    if not mask & 2:
        draw.polygon(
            _points(((56, 4), (64, 0), (64, 64), (57, 60), (59, 42), (55, 22))),
            fill=_rgba(face),
        )
        draw.line(
            _points(((56, 4), (55, 22), (59, 42), (57, 60))),
            fill=_rgba(top_light),
            width=_s(2),
        )

    # Large fractures and rare retaining remnants keep connected fields from
    # becoming a repeated checkerboard without reverting to hazard stripes.
    start_x = rng.randrange(14, 34)
    start_y = rng.randrange(13, 31)
    crack = (
        (start_x, start_y),
        (start_x + rng.randrange(5, 11), start_y + rng.randrange(4, 9)),
        (start_x + rng.randrange(10, 18), start_y + rng.randrange(10, 17)),
    )
    draw.line(_points(crack), fill=_rgba(seam), width=_s(1))
    if variant == 1:
        draw.line(_points(((13, 53), (25, 51))), fill=_rgba(deep), width=_s(3))
        draw.line(_points(((15, 52), (26, 50))), fill=_rgba(rust), width=_s(1))
        draw.line(_points(((39, 55), (49, 54))), fill=_rgba(deep), width=_s(3))
        draw.line(_points(((40, 54), (50, 53))), fill=_rgba(rust), width=_s(1))

    return _finish(image, TILE, TILE)


def install_finalized_environment(registry: Registry, out: Path) -> None:
    """Install every approved environment sprite into the production bank."""

    for key, image in zip(FIELD_DEBRIS_KEYS, _field_debris(), strict=True):
        _put(registry, out, key, image)
    for key, image in zip(GROUND_BLOCKER_KEYS, _ground_blockers(), strict=True):
        _put(registry, out, key, image)
    for source in _ROCK_SOURCES:
        _put(registry, out, source.key, _rock(source))
    for mask in range(16):
        for variant in range(2):
            key = f"peak_barrier_{mask:02x}_{variant}"
            _put(registry, out, key, _peak_barrier(mask, variant))
