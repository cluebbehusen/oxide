"""Native production renderer for the finalized dark-channel Lancer."""

from __future__ import annotations

from dataclasses import replace

from PIL import Image, ImageDraw

from tools.gen_sprites import BONE, IRON_DARK, IRON_LIGHT, SCRAP_DARK, SCRAP_LIGHT
from tools.production_sprite_sources import ground_final as ground_shapes
from tools.production_sprite_sources.ground_base import GroundUnitSequence


def _draw_center_channel(
    draw: ImageDraw.ImageDraw, *, charge_level: int, recoil: int, report: bool
) -> None:
    if report:
        draw.rectangle(
            ground_shapes._box((30, 5 + recoil, 34, 36 + recoil)),
            fill=ground_shapes._rgba(BONE),
        )
        draw.polygon(
            ground_shapes._points(
                (
                    (28, 5 + recoil),
                    (32, 2 + recoil),
                    (36, 5 + recoil),
                    (33, 9 + recoil),
                    (31, 9 + recoil),
                )
            ),
            fill=ground_shapes._rgba(SCRAP_LIGHT),
        )
    elif charge_level == 1:
        draw.rectangle(
            ground_shapes._box((31, 28, 33, 36)), fill=ground_shapes._rgba(SCRAP_DARK)
        )
    elif charge_level == 2:
        draw.rectangle(
            ground_shapes._box((31, 17, 33, 36)), fill=ground_shapes._rgba(SCRAP_LIGHT)
        )
    elif charge_level == 3:
        draw.rectangle(
            ground_shapes._box((30, 7, 34, 36)), fill=ground_shapes._rgba(SCRAP_LIGHT)
        )
    elif charge_level != 0:
        raise ValueError(f"invalid Lancer charge level: {charge_level}")


def _dark_channel_sprite(
    *,
    tread_phase: int = 0,
    recoil: int = 0,
    charge_level: int = 0,
    report: bool = False,
) -> Image.Image:
    image, draw = ground_shapes._canvas()
    ground_shapes._tracks(draw, ((8, 30, 18, 59), (46, 30, 56, 59)), tread_phase)
    draw.rectangle(
        ground_shapes._box((20, 37, 44, 57)),
        fill=ground_shapes._rgba(ground_shapes.FERROUS["dark"]),
    )
    draw.rectangle(ground_shapes._box((25, 42, 39, 53)), fill=(11, 11, 15, 255))
    rail_boxes = ((21, 6, 28, 44), (36, 6, 43, 44))
    draw.rectangle(
        ground_shapes._box((18, 31, 46, 38)), fill=ground_shapes._rgba(IRON_DARK)
    )
    for x0, y0, x1, y1 in rail_boxes:
        draw.rounded_rectangle(
            ground_shapes._box((x0, y0 + recoil, x1, y1 + recoil)),
            radius=ground_shapes._s(2),
            fill=ground_shapes._rgba(IRON_DARK),
        )
        draw.rectangle(
            ground_shapes._box((x0 + 2, y0 + 2 + recoil, x1 - 2, y1 - 3 + recoil)),
            fill=ground_shapes._rgba(IRON_LIGHT),
        )
    draw.rectangle(
        ground_shapes._box((27, 32 + recoil, 37, 43 + recoil)),
        fill=ground_shapes._rgba(IRON_DARK),
    )
    _draw_center_channel(draw, charge_level=charge_level, recoil=recoil, report=report)
    for index, x in enumerate((23, 31, 39)):
        draw.rounded_rectangle(
            ground_shapes._box((x, 51, x + 6, 59)),
            radius=ground_shapes._s(2),
            fill=ground_shapes._rgba(IRON_DARK),
        )
        color = SCRAP_LIGHT if index < charge_level else SCRAP_DARK
        draw.rectangle(
            ground_shapes._box((x + 2, 53, x + 4, 57)), fill=ground_shapes._rgba(color)
        )
    return ground_shapes._finish(image)


def _refined_sequence() -> GroundUnitSequence:
    source = ground_shapes.lancer_tuning_fork_sequence()
    images = (
        _dark_channel_sprite(),
        _dark_channel_sprite(tread_phase=1),
        _dark_channel_sprite(tread_phase=2),
        _dark_channel_sprite(),
        _dark_channel_sprite(charge_level=1),
        _dark_channel_sprite(charge_level=2),
        _dark_channel_sprite(charge_level=3),
        _dark_channel_sprite(recoil=5, report=True),
        _dark_channel_sprite(recoil=2),
        _dark_channel_sprite(),
    )
    return GroundUnitSequence(
        stem="lancer_tuning_fork_dark_channel",
        title="Lancer / Tuning-Fork Dark Channel",
        mechanism="open tuning-fork channel energized by three physical capacitor cells",
        mechanism_box=source.mechanism_box,
        attack_contract=source.attack_contract,
        frames=tuple(
            (
                replace(frame, image=image)
                for frame, image in zip(source.frames, images, strict=True)
            )
        ),
    )


def lancer_sequence() -> GroundUnitSequence:
    return _refined_sequence()
