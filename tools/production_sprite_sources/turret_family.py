"""Native production source for the circular, belt-fed Turret family."""

from __future__ import annotations

import hashlib

from PIL import Image, ImageChops, ImageDraw

from tools import gen_sprites as gen


def _finish_without_rim(image: Image.Image) -> Image.Image:
    return image.resize((64, 64), Image.Resampling.LANCZOS)


def _strip_pale_rim(image: Image.Image) -> Image.Image:
    result = image.copy()
    pixels = result.load()
    for y in range(result.height):
        for x in range(result.width):
            red, green, blue, alpha = pixels[x, y]
            if alpha < 190 and red > 210 and green > 195 and blue > 175:
                pixels[x, y] = (red, green, blue, 0)
    return result


def _sanitize_transparent_fringe(image: Image.Image) -> Image.Image:
    result = image.copy()
    pixels = result.load()
    for y in range(result.height):
        for x in range(result.width):
            red, green, blue, alpha = pixels[x, y]
            if alpha <= 3:
                pixels[x, y] = (red, green, blue, 0)
    return result


def turret_base(faction: str, tier: int) -> Image.Image:
    """Draw the circular foundation and increasingly armored traverse ring."""
    if tier not in range(3):
        raise ValueError(f"invalid Turret tier: {tier}")
    palette = gen.FACTIONS[faction]
    image, draw = gen.canvas(64)

    draw.ellipse(
        [gen.s(2), gen.s(2), gen.s(62), gen.s(62)], fill=(*gen.IRON_DARK, 255)
    )
    for points in (
        ((22, 3), (42, 3), (46, 12), (18, 12)),
        ((61, 22), (61, 42), (52, 46), (52, 18)),
        ((22, 61), (42, 61), (46, 52), (18, 52)),
        ((3, 22), (3, 42), (12, 46), (12, 18)),
    ):
        draw.polygon(
            [(gen.s(x), gen.s(y)) for x, y in points],
            fill=(*(gen.IRON if tier else gen.IRON_DARK), 255),
        )
    draw.ellipse(
        [gen.s(8), gen.s(8), gen.s(56), gen.s(56)], fill=(*gen.IRON, 255)
    )
    if tier >= 1:
        for x0, y0, x1, y1 in ((8, 18, 16, 46), (48, 18, 56, 46)):
            draw.rounded_rectangle(
                [gen.s(x0), gen.s(y0), gen.s(x1), gen.s(y1)],
                radius=gen.s(3),
                fill=(*palette["dark"], 255),
            )
    if tier == 2:
        draw.ellipse(
            [gen.s(12), gen.s(12), gen.s(52), gen.s(52)],
            outline=(*palette["dark"], 255),
            width=gen.s(6),
        )

    ring_outer = (14 - tier, 14 - tier, 50 + tier, 50 + tier)
    ring_inner = (19 - tier, 19 - tier, 45 + tier, 45 + tier)
    draw.ellipse(
        [gen.s(value) for value in ring_outer], fill=(*palette["dark"], 255)
    )
    draw.ellipse(
        [gen.s(value) for value in ring_inner], fill=(*palette["base"], 255)
    )
    draw.ellipse(
        [gen.s(26), gen.s(26), gen.s(38), gen.s(38)],
        fill=(*gen.IRON_DARK, 255),
    )

    outer_edge = ((6, 6, 58, 58), (3, 3, 61, 61), (1, 1, 63, 63))[tier]
    scaled_edge = tuple(gen.s(value) for value in outer_edge)
    draw.arc(
        scaled_edge,
        start=188,
        end=292,
        fill=(*gen.IRON_LIGHT, 255),
        width=gen.s(2),
    )
    draw.arc(
        scaled_edge,
        start=8,
        end=112,
        fill=(*gen.IRON, 255),
        width=gen.s(2),
    )
    return _finish_without_rim(image)


def _gun(faction: str, tier: int, phase: int) -> Image.Image:
    palette = gen.FACTIONS[faction]
    image, draw = gen.canvas(64)
    report = phase == 1
    recoil = {2: 5, 3: 2}.get(phase, 0)
    barrel_width = (5, 8, 11)[tier]
    barrel_top = (5, 2, 1)[tier] + recoil
    breech_top = (27, 23, 19)[tier] + recoil
    breech_bottom = (47, 49, 51)[tier] + recoil

    draw.rounded_rectangle(
        [
            gen.s(32 - barrel_width / 2 - 2),
            gen.s(barrel_top),
            gen.s(32 + barrel_width / 2 + 2),
            gen.s(34 + recoil),
        ],
        radius=gen.s(2),
        fill=(*gen.IRON_DARK, 255),
    )
    draw.rectangle(
        [
            gen.s(32 - barrel_width / 2),
            gen.s(barrel_top),
            gen.s(32 + barrel_width / 2),
            gen.s(34 + recoil),
        ],
        fill=(*gen.IRON_LIGHT, 255),
    )
    if tier >= 1:
        draw.rectangle(
            [
                gen.s(25 - tier),
                gen.s(barrel_top),
                gen.s(39 + tier),
                gen.s(barrel_top + 5 + tier),
            ],
            fill=(*gen.IRON_DARK, 255),
        )
        if tier == 2:
            for x0, x1 in ((24, 28), (36, 40)):
                draw.rectangle(
                    [
                        gen.s(x0),
                        gen.s(barrel_top + 2),
                        gen.s(x1),
                        gen.s(barrel_top + 8),
                    ],
                    fill=(8, 8, 10, 255),
                )
    draw.rounded_rectangle(
        [
            gen.s(22 - tier * 2),
            gen.s(breech_top),
            gen.s(42 + tier * 2),
            gen.s(breech_bottom),
        ],
        radius=gen.s(3),
        fill=(*palette["dark"], 255),
    )
    draw.rounded_rectangle(
        [
            gen.s(26 - tier),
            gen.s(breech_top + 4),
            gen.s(38 + tier),
            gen.s(breech_bottom - 4),
        ],
        radius=gen.s(2),
        fill=(9, 9, 12, 255),
    )
    if tier >= 1:
        for x in (17 - tier, 43):
            draw.rounded_rectangle(
                [
                    gen.s(x),
                    gen.s(28 + recoil),
                    gen.s(x + 7 + tier),
                    gen.s(45 + recoil),
                ],
                radius=gen.s(2),
                fill=(*gen.IRON_DARK, 255),
            )
            draw.rectangle(
                [
                    gen.s(x + 2),
                    gen.s(31 + recoil),
                    gen.s(x + 5 + tier),
                    gen.s(41 + recoil),
                ],
                fill=(*gen.SCRAP_DARK, 255),
            )
    draw.ellipse(
        [
            gen.s(28),
            gen.s(37 + recoil),
            gen.s(36),
            gen.s(45 + recoil),
        ],
        fill=(*palette["base"], 255),
    )
    draw.ellipse(
        [
            gen.s(30),
            gen.s(39 + recoil),
            gen.s(34),
            gen.s(43 + recoil),
        ],
        fill=(*palette["light"], 255),
    )
    if report:
        size = 7 + tier * 3
        draw.polygon(
            [
                (gen.s(32), gen.s(0)),
                (gen.s(32 - size), gen.s(8)),
                (gen.s(32), gen.s(5)),
                (gen.s(32 + size), gen.s(8)),
            ],
            fill=(*gen.SCRAP_LIGHT, 255),
        )
        draw.polygon(
            [
                (gen.s(32), gen.s(0)),
                (gen.s(29), gen.s(6)),
                (gen.s(32), gen.s(4)),
                (gen.s(35), gen.s(6)),
            ],
            fill=(*gen.BONE, 255),
        )
    return _strip_pale_rim(
        gen.rim_light(image.resize((64, 64), Image.Resampling.LANCZOS))
    )


def _feed(tier: int, phase: int) -> Image.Image:
    image, draw = gen.canvas(64)
    recoil = {2: 5, 3: 2}.get(phase, 0)
    shell_count = 3 + tier
    sides = ("right",) if tier == 0 else ("left", "right")
    for side in sides:
        left = 8 if side == "left" else 47
        right = left + 9
        draw.rounded_rectangle(
            [
                gen.s(left),
                gen.s(29 + recoil),
                gen.s(right),
                gen.s(51 + recoil),
            ],
            radius=gen.s(3),
            fill=(*gen.IRON_DARK, 255),
        )
        for index in range(shell_count):
            y = 32 + recoil + index * 4
            draw.rectangle(
                [gen.s(left + 2), gen.s(y), gen.s(right - 2), gen.s(y + 2)],
                fill=(*gen.SCRAP, 255),
            )
        belt_start = right if side == "left" else left
        belt_end = 23 - tier if side == "left" else 41 + tier
        draw.line(
            [
                (gen.s(belt_start), gen.s(37 + recoil)),
                (gen.s(belt_end), gen.s(37 + recoil)),
            ],
            fill=(*gen.SCRAP_DARK, 255),
            width=gen.s(3),
        )
    return _finish_without_rim(image)


def turret_mount(faction: str, tier: int, phase: int) -> Image.Image:
    """Draw one tier-specific firing pose, including its visible shell feeds."""
    if tier not in range(3):
        raise ValueError(f"invalid Turret tier: {tier}")
    if phase not in range(5):
        raise ValueError(f"invalid Turret action phase: {phase}")
    image = _gun(faction, tier, phase)
    image.alpha_composite(_feed(tier, phase))
    if faction != "ferrous":
        silhouette = _gun("ferrous", tier, phase)
        silhouette.alpha_composite(_feed(tier, phase))
        image.putalpha(silhouette.getchannel("A"))
    return image


def turret_frame(faction: str, tier: int, phase: int) -> Image.Image:
    """Compose a native review-equivalent frame for stability tests."""
    image = turret_base(faction, tier)
    image.alpha_composite(turret_mount(faction, tier, phase))
    return _sanitize_transparent_fringe(image)


TURRET_APPROVED_VISIBLE_RGBA_SHA256 = (
    "9180f207de386758147599607eba761dafe8757c874779d0af10b7d8f878fff4"
)


def _visible_rgba_bytes(image: Image.Image) -> bytes:
    data = bytearray(image.convert("RGBA").tobytes())
    for alpha in range(3, len(data), 4):
        if data[alpha] <= 3:
            data[alpha - 3 : alpha] = b"\0\0\0"
            data[alpha] = 0
    return bytes(data)


def turret_source_visible_digest() -> str:
    """Hash the approved segmented twin-feed sequence without invisible RGB."""
    digest = hashlib.sha256()
    for faction in ("ferrous", "cupric"):
        for tier in range(3):
            for phase in range(5):
                digest.update(f"turret/{faction}/{tier}/{phase}".encode())
                digest.update(_visible_rgba_bytes(turret_frame(faction, tier, phase)))
    return digest.hexdigest()


def factions_share_silhouette(tier: int, phase: int) -> bool:
    """Return whether both allegiance variants occupy the same pixels."""
    ferrous = turret_frame("ferrous", tier, phase).getchannel("A")
    cupric = turret_frame("cupric", tier, phase).getchannel("A")
    return ImageChops.difference(ferrous, cupric).getbbox() is None
