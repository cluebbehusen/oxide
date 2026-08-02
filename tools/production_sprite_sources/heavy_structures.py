"""Native production source for the finalized Foundry and Bastion."""

from __future__ import annotations

from PIL import Image

from tools.gen_sprites import (
    BONE,
    FACTIONS,
    GROUND_DARK,
    IRON,
    IRON_DARK,
    IRON_LIGHT,
    SCRAP,
    SCRAP_DARK,
    SCRAP_LIGHT,
    canvas,
    rim_light,
    s,
)


def _finish(image: Image.Image, side: int) -> Image.Image:
    return rim_light(image.resize((side, side), Image.Resampling.LANCZOS))


def foundry_frame(faction: str, work: int) -> Image.Image:
    """Draw one exposed-gantry production pose at the 2x2 native size."""
    if work not in range(4):
        raise ValueError(f"invalid Foundry work frame: {work}")
    palette = FACTIONS[faction]
    image, draw = canvas(128)
    draw.rounded_rectangle(
        [s(8), s(8), s(120), s(120)],
        radius=s(8),
        fill=(*IRON_DARK, 255),
    )
    draw.rectangle([s(16), s(16), s(112), s(112)], fill=(*GROUND_DARK, 255))
    for x in (20, 102):
        draw.rectangle([s(x), s(16), s(x + 7), s(111)], fill=(*IRON, 255))
        for y in range(22, 108, 14):
            draw.rectangle(
                [s(x - 2), s(y), s(x + 9), s(y + 4)],
                fill=(*IRON_LIGHT, 255),
            )
    draw.ellipse([s(35), s(55), s(83), s(103)], fill=(*IRON_DARK, 255))
    draw.ellipse([s(43), s(63), s(75), s(95)], fill=(*SCRAP_DARK, 255))
    if work:
        draw.ellipse([s(50), s(70), s(68), s(88)], fill=(*SCRAP_LIGHT, 255))
    carriage_y = (26, 42, 63, 42)[work]
    draw.rectangle(
        [s(22), s(carriage_y), s(106), s(carriage_y + 9)],
        fill=(*IRON_DARK, 255),
    )
    draw.rectangle(
        [s(29), s(carriage_y + 2), s(99), s(carriage_y + 6)],
        fill=(*palette["dark"], 255),
    )
    draw.rounded_rectangle(
        [s(54), s(carriage_y - 4), s(74), s(carriage_y + 15)],
        radius=s(4),
        fill=(*IRON, 255),
    )
    hook_y = carriage_y + (22 if work in (1, 3) else 13)
    draw.line(
        [(s(64), s(carriage_y + 11)), (s(64), s(hook_y))],
        fill=(*BONE, 255),
        width=s(3),
    )
    draw.rectangle([s(51), s(105), s(91), s(118)], fill=(*palette["dark"], 255))
    return _finish(image, 128)


def bastion_base(faction: str) -> Image.Image:
    """Draw the open circular carriage and visible shell rack."""
    palette = FACTIONS[faction]
    image, draw = canvas(128)
    draw.ellipse(
        [s(7), s(7), s(121), s(121)],
        outline=(*IRON_DARK, 255),
        width=s(12),
    )
    draw.ellipse(
        [s(25), s(25), s(103), s(103)],
        outline=(*palette["dark"], 255),
        width=s(9),
    )
    for x in (38, 82):
        draw.rectangle([s(x - 5), s(15), s(x + 5), s(108)], fill=(*IRON, 255))
        for y in range(21, 103, 16):
            draw.rectangle(
                [s(x - 8), s(y), s(x + 8), s(y + 4)],
                fill=(*IRON_LIGHT, 255),
            )
    draw.ellipse([s(36), s(36), s(92), s(92)], fill=(*IRON_DARK, 255))
    draw.ellipse([s(45), s(45), s(83), s(83)], fill=(*palette["dark"], 255))
    draw.rectangle([s(8), s(48), s(29), s(103)], fill=(*IRON_DARK, 255))
    for y in range(55, 96, 10):
        draw.rounded_rectangle(
            [s(14), s(y), s(24), s(y + 6)],
            radius=s(2),
            fill=(*SCRAP, 255),
        )
    return _finish(image, 128)


def bastion_mount(faction: str) -> Image.Image:
    """Draw the centered open breech and shell rammer on a square pivot."""
    palette = FACTIONS[faction]
    image, draw = canvas(128)
    barrel = (48, 1, 80, 72)
    inner = (55, 2, 73, 68)
    draw.rounded_rectangle(
        [s(value) for value in barrel],
        radius=s(5),
        fill=(*IRON_DARK, 255),
    )
    draw.rectangle([s(value) for value in inner], fill=(*IRON_LIGHT, 255))
    for x in (38, 90):
        draw.line([(s(x), s(34)), (s(x), s(104))], fill=(*IRON, 255), width=s(10))
    draw.rectangle([s(31), s(53), s(97), s(85)], fill=(*palette["dark"], 255))
    draw.rectangle([s(41), s(61), s(87), s(78)], fill=(8, 8, 10, 255))
    draw.rectangle([s(54), s(82), s(74), s(116)], fill=(*BONE, 255))
    draw.rectangle([s(46), s(104), s(82), s(121)], fill=(*IRON_DARK, 255))
    return _finish(image, 128)
