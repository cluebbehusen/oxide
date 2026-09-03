"""Native production source for the finalized Foundry and Bastion."""

from __future__ import annotations

import hashlib

from PIL import Image, ImageDraw

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
    """Draw the Foundry with a fixed gantry and centered production eye."""
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
    eye_x, eye_y = 64, 72
    draw.ellipse([s(40), s(48), s(88), s(96)], fill=(*IRON_DARK, 255))
    draw.ellipse([s(47), s(55), s(81), s(89)], fill=(9, 9, 12, 255))
    pulse_radius = (6, 9, 12, 9)[work]
    pulse_color = (SCRAP_DARK, SCRAP, SCRAP_LIGHT, SCRAP)[work]
    draw.ellipse(
        [
            s(eye_x - pulse_radius),
            s(eye_y - pulse_radius),
            s(eye_x + pulse_radius),
            s(eye_y + pulse_radius),
        ],
        fill=(*pulse_color, 255),
    )
    carriage_y = 26
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
    draw.line(
        [(s(64), s(carriage_y + 11)), (s(64), s(45))],
        fill=(*BONE, 255),
        width=s(3),
    )
    draw.rectangle([s(44), s(105), s(84), s(118)], fill=(*palette["dark"], 255))
    return _finish(image, 128)


def bastion_base(faction: str, charge: int) -> Image.Image:
    """Draw the open service carriage and its single five-cell charge rack."""
    if charge not in range(6):
        raise ValueError(f"invalid Bastion charge: {charge}")
    palette = FACTIONS[faction]
    image, draw = canvas(128)
    draw.ellipse([s(6), s(6), s(122), s(122)], fill=(*IRON_DARK, 255))
    draw.ellipse([s(12), s(12), s(116), s(116)], fill=(*IRON, 255))
    draw.ellipse([s(18), s(18), s(110), s(110)], fill=(*GROUND_DARK, 255))
    for start, end in ((23, 49), (79, 105)):
        draw.line(
            [(s(start), s(20)), (s(end), s(20))],
            fill=(*palette["dark"], 255),
            width=s(5),
        )
        draw.line(
            [(s(start), s(108)), (s(end), s(108))],
            fill=(*palette["dark"], 255),
            width=s(5),
        )
    for x, y in ((18, 18), (110, 18), (18, 110), (110, 110)):
        draw.ellipse(
            [s(x - 5), s(y - 5), s(x + 5), s(y + 5)],
            fill=(*IRON_DARK, 255),
        )
        draw.ellipse(
            [s(x - 2), s(y - 2), s(x + 2), s(y + 2)], fill=(*BONE, 255)
        )
    for x in (40, 88):
        draw.rounded_rectangle(
            [s(x - 5), s(23), s(x + 5), s(106)],
            radius=s(3),
            fill=(*IRON, 255),
        )
        for y in range(29, 103, 15):
            draw.rectangle(
                [s(x - 8), s(y), s(x + 8), s(y + 3)],
                fill=(*IRON_LIGHT, 255),
            )
    draw.ellipse([s(34), s(34), s(94), s(94)], fill=(*IRON_DARK, 255))
    draw.ellipse([s(42), s(42), s(86), s(86)], fill=(*palette["dark"], 255))
    draw.rounded_rectangle(
        [s(8), s(43), s(31), s(109)], radius=s(4), fill=(*IRON_DARK, 255)
    )
    draw.rectangle([s(11), s(47), s(27), s(103)], fill=(12, 12, 16, 255))
    for index in range(5):
        y = 51 + index * 10
        color = SCRAP_LIGHT if index < charge else SCRAP_DARK
        draw.rounded_rectangle(
            [s(15), s(y), s(24), s(y + 6)],
            radius=s(2),
            fill=(*color, 255),
        )
    draw.rectangle([s(13), s(104), s(26), s(108)], fill=(*palette["light"], 255))
    return _finish(image, 128)


def bastion_mount(faction: str, phase: int) -> Image.Image:
    """Draw the service-deck breech through charge, report, and recoil."""
    if phase not in range(10):
        raise ValueError(f"invalid Bastion action phase: {phase}")
    palette = FACTIONS[faction]
    image, draw = canvas(128)
    report = phase == 6
    recoil = {7: 10, 8: 4}.get(phase, 0)
    draw.rounded_rectangle(
        [s(43), s(2 + recoil), s(85), s(74 + recoil)],
        radius=s(6),
        fill=(*IRON_DARK, 255),
    )
    draw.rectangle(
        [s(51), s(3 + recoil), s(77), s(69 + recoil)],
        fill=(*IRON_LIGHT, 255),
    )
    draw.rectangle(
        [s(57), s(3 + recoil), s(71), s(69 + recoil)], fill=(*IRON, 255)
    )
    for x in (35, 87):
        draw.rounded_rectangle(
            [s(x), s(29 + recoil), s(x + 8), s(101)],
            radius=s(3),
            fill=(*IRON_DARK, 255),
        )
        draw.rectangle(
            [s(x + 2), s(35 + recoil), s(x + 6), s(88)], fill=(*BONE, 255)
        )
    draw.rounded_rectangle(
        [s(28), s(49 + recoil), s(100), s(88 + recoil)],
        radius=s(5),
        fill=(*palette["dark"], 255),
    )
    draw.rectangle(
        [s(39), s(58 + recoil), s(89), s(78 + recoil)], fill=(8, 8, 11, 255)
    )
    draw.rounded_rectangle(
        [s(50), s(84 + recoil), s(78), s(116)],
        radius=s(4),
        fill=(*IRON, 255),
    )
    draw.rectangle([s(44), s(105), s(84), s(121)], fill=(*IRON_DARK, 255))
    if report:
        draw.polygon(
            [(s(64), s(0)), (s(56), s(12)), (s(64), s(8)), (s(72), s(12))],
            fill=(*SCRAP_LIGHT, 255),
        )
        draw.polygon(
            [(s(64), s(0)), (s(61), s(8)), (s(64), s(6)), (s(67), s(8))],
            fill=(*BONE, 255),
        )
    result = _finish(image, 128)
    final_draw = ImageDraw.Draw(result)
    final_draw.rounded_rectangle(
        (47, 94, 81, 102), radius=2, fill=(*IRON_DARK, 255)
    )
    final_draw.line((53, 97, 75, 97), fill=(*IRON, 255), width=2)
    for x in (52, 76):
        final_draw.ellipse((x, 96, x + 2, 98), fill=(*IRON_LIGHT, 255))
    return result


BASTION_APPROVED_VISIBLE_RGBA_SHA256 = (
    "5cc15839228f11455710e46803c5fea9a77ac867b2e7ea8a8ee5e856b10c159e"
)


def _visible_rgba_bytes(image: Image.Image) -> bytes:
    data = bytearray(image.convert("RGBA").tobytes())
    for alpha in range(3, len(data), 4):
        if data[alpha] == 0:
            data[alpha - 3 : alpha] = b"\0\0\0"
    return bytes(data)


def bastion_source_visible_digest() -> str:
    """Hash the approved single-gauge review sequence without invisible RGB."""
    digest = hashlib.sha256()
    for faction in ("ferrous", "cupric"):
        for phase in range(10):
            base = bastion_base(faction, min(phase, 5))
            frame = base.copy()
            frame.alpha_composite(bastion_mount(faction, phase))
            frame.alpha_composite(base.crop((7, 40, 35, 113)), (7, 40))
            digest.update(f"bastion/{faction}/{phase}".encode())
            digest.update(_visible_rgba_bytes(frame))
    return digest.hexdigest()
