"""Production-native frames for approved Flak 526 and Deep Array 520 art."""

from __future__ import annotations

import hashlib
import math
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw

from tools import gen_sprites as gen

FLAK_ACTION_SUFFIXES = tuple(f"_action{phase}" for phase in range(1, 9))
ARRAY_WORK_SUFFIXES = tuple(f"_work{phase}" for phase in range(1, 7))
ARRAY_HEADINGS = (225, 285, 345, 405, 465, 525, 585)

FLAK_APPROVED_VISIBLE_RGBA_SHA256 = (
    "0e9458d23ea7bf6c34a8d90fe5dab09bd9d2706c6bfd3f6878451e1012238793"
)
DEEP_ARRAY_APPROVED_VISIBLE_RGBA_SHA256 = (
    "56d4dd83c86f2526edfacb40218d95f35420faa5766aa6527d7533fbcbbb7a0f"
)


def _rgba(color: tuple[int, int, int], alpha: int = 255) -> tuple[int, int, int, int]:
    return (*color, alpha)


def _box(values: tuple[float, float, float, float]) -> tuple[int, int, int, int]:
    x0, y0, x1, y1 = values
    return (gen.s(x0), gen.s(y0), gen.s(x1), gen.s(y1))


def _points(values: list[tuple[float, float]]) -> list[tuple[int, int]]:
    return [(gen.s(x), gen.s(y)) for x, y in values]


def _rect(
    draw: ImageDraw.ImageDraw,
    xy: tuple[float, float, float, float],
    **kwargs: object,
) -> None:
    draw.rectangle(_box(xy), **kwargs)


def _rounded(
    draw: ImageDraw.ImageDraw,
    xy: tuple[float, float, float, float],
    radius: float,
    **kwargs: object,
) -> None:
    draw.rounded_rectangle(_box(xy), radius=gen.s(radius), **kwargs)


def _ellipse(
    draw: ImageDraw.ImageDraw,
    xy: tuple[float, float, float, float],
    **kwargs: object,
) -> None:
    draw.ellipse(_box(xy), **kwargs)


def _line(
    draw: ImageDraw.ImageDraw,
    xy: list[tuple[float, float]],
    width: float,
    **kwargs: object,
) -> None:
    draw.line(_points(xy), width=gen.s(width), **kwargs)


def _finish(image: Image.Image) -> Image.Image:
    result = image.resize((64, 64), Image.Resampling.LANCZOS)
    pixels = result.load()
    for y in range(result.height):
        for x in range(result.width):
            red, green, blue, alpha = pixels[x, y]
            if alpha <= 3:
                pixels[x, y] = (red, green, blue, 0)
    return result


def _beveled_plate(
    draw: ImageDraw.ImageDraw,
    bounds: tuple[float, float, float, float],
    fill: tuple[int, int, int],
    radius: float,
) -> None:
    x0, y0, x1, y1 = bounds
    _rounded(draw, bounds, radius, fill=_rgba(gen.IRON_DARK))
    _rounded(
        draw,
        (x0 + 2, y0 + 2, x1 - 2, y1 - 2),
        max(1, radius - 1),
        fill=_rgba(fill),
    )
    _line(
        draw,
        [(x0 + 4, y0 + 3), (x1 - 4, y0 + 3)],
        1,
        fill=_rgba(gen.IRON_LIGHT, 210),
    )


def _bolt(draw: ImageDraw.ImageDraw, x: float, y: float, radius: float = 1.5) -> None:
    _ellipse(
        draw,
        (x - radius, y - radius, x + radius, y + radius),
        fill=_rgba(gen.IRON_DARK),
    )
    _rect(draw, (x - 0.5, y - 0.5, x + 0.5, y + 0.5), fill=_rgba(gen.BONE, 230))


def _elevated_barrel(
    draw: ImageDraw.ImageDraw,
    center_x: float,
    tip_y: float,
    breech_y: float,
    width: float,
) -> None:
    draw.polygon(
        _points(
            [
                (center_x - width * 0.62, tip_y + 1),
                (center_x + width * 0.62, tip_y + 1),
                (center_x + width, breech_y),
                (center_x - width, breech_y),
            ]
        ),
        fill=_rgba(gen.IRON_DARK),
    )
    draw.polygon(
        _points(
            [
                (center_x - width * 0.28, tip_y + 2),
                (center_x + width * 0.28, tip_y + 2),
                (center_x + width * 0.42, breech_y - 1),
                (center_x - width * 0.42, breech_y - 1),
            ]
        ),
        fill=_rgba(gen.IRON_LIGHT),
    )
    _ellipse(
        draw,
        (
            center_x - width * 0.78,
            tip_y - 1,
            center_x + width * 0.78,
            tip_y + 2.2,
        ),
        fill=_rgba(gen.IRON_LIGHT),
    )
    _ellipse(
        draw,
        (
            center_x - width * 0.42,
            tip_y - 0.2,
            center_x + width * 0.42,
            tip_y + 1.5,
        ),
        fill=(8, 8, 11, 255),
    )


def flak_base(faction: str, tier: int) -> Image.Image:
    """Draw the approved compact base or broad Burst Flak foundation."""
    if tier not in range(2):
        raise ValueError(f"invalid Flak tier: {tier}")
    palette = gen.FACTIONS[faction]
    image, draw = gen.canvas(64)
    if tier == 0:
        _ellipse(draw, (8, 8, 56, 58), fill=_rgba(gen.IRON_DARK))
        _ellipse(draw, (13, 13, 51, 53), fill=_rgba(gen.IRON))
        for x, y in ((12, 15), (52, 15), (12, 50), (52, 50)):
            _bolt(draw, x, y, 1.3)
    else:
        draw.polygon(
            _points(
                [
                    (15, 3),
                    (49, 3),
                    (61, 15),
                    (61, 49),
                    (49, 61),
                    (15, 61),
                    (3, 49),
                    (3, 15),
                ]
            ),
            fill=_rgba(gen.IRON_DARK),
        )
        draw.polygon(
            _points(
                [
                    (17, 8),
                    (47, 8),
                    (56, 17),
                    (56, 47),
                    (47, 56),
                    (17, 56),
                    (8, 47),
                    (8, 17),
                ]
            ),
            fill=_rgba(gen.IRON),
        )
        for x in (3, 51):
            _beveled_plate(draw, (x, 22, x + 10, 45), palette["dark"], 2)
            for y in (27, 34, 41):
                _rect(draw, (x + 3, y, x + 7, y + 3), fill=_rgba(gen.SCRAP_DARK))

    _ellipse(draw, (23, 24, 41, 42), fill=(14, 14, 19, 255))
    _ellipse(draw, (28, 29, 36, 37), fill=_rgba(palette["base"]))
    return _finish(image)


def flak_mount(faction: str, tier: int, phase: int) -> Image.Image:
    """Draw one approved four-stage charge, report, or recovery pose."""
    if tier not in range(2):
        raise ValueError(f"invalid Flak tier: {tier}")
    if phase not in range(9):
        raise ValueError(f"invalid Flak action phase: {phase}")
    palette = gen.FACTIONS[faction]
    image, draw = gen.canvas(64)
    report_recoil = 5 if tier == 0 else 3
    left_recoil = report_recoil if phase == 5 else 2 if phase == 7 else 0
    right_recoil = report_recoil if phase == 6 else 2 if phase == 7 else 0
    barrels_per_bank = 2 if tier == 0 else 3
    bank_centers = (22, 42) if tier == 0 else (20, 44)

    _rounded(
        draw,
        (14 if tier == 0 else 8, 28, 50 if tier == 0 else 56, 49),
        5,
        fill=_rgba(gen.IRON_DARK),
    )
    _rounded(
        draw,
        (19 if tier == 0 else 13, 32, 45 if tier == 0 else 51, 45),
        3,
        fill=_rgba(palette["dark"]),
    )

    spacing = 4 if tier == 0 else 4.5
    width = 2.1 if tier == 0 else 2.35
    for bank_index, (center_x, recoil) in enumerate(
        zip(bank_centers, (left_recoil, right_recoil), strict=True)
    ):
        pod_half = 7 if tier == 0 else 9
        _rounded(
            draw,
            (center_x - pod_half, 20 + recoil, center_x + pod_half, 37 + recoil),
            3,
            fill=_rgba(gen.IRON_DARK),
        )
        _rounded(
            draw,
            (
                center_x - pod_half + 3,
                23 + recoil,
                center_x + pod_half - 3,
                34 + recoil,
            ),
            2,
            fill=_rgba(gen.IRON),
        )
        for barrel_index in range(barrels_per_bank):
            x = center_x + (barrel_index - (barrels_per_bank - 1) / 2) * spacing
            _elevated_barrel(draw, x, 5 + recoil, 26 + recoil, width)

        if tier:
            cassette_x = 6 if bank_index == 0 else 50
            _rounded(
                draw,
                (cassette_x, 29, cassette_x + 8, 47),
                2,
                fill=_rgba(gen.IRON_DARK),
            )
            for y in (32, 37, 42):
                _rect(
                    draw,
                    (cassette_x + 2, y, cassette_x + 6, y + 2),
                    fill=_rgba(gen.SCRAP_DARK),
                )

    if tier:
        _rounded(draw, (29, 18, 35, 41), 2, fill=_rgba(gen.IRON_DARK))
        _rect(draw, (31, 21, 33, 37), fill=_rgba(gen.IRON_LIGHT))
        _bolt(draw, 32, 20, 1.2)
        for x in (14, 50):
            _line(
                draw,
                [(x, 35), (18 if x < 32 else 46, 31)],
                2,
                fill=_rgba(gen.IRON_LIGHT),
            )

    if 1 <= phase <= 4:
        filled = phase
    elif phase in (5, 6):
        filled = 2 if phase == 5 else 0
    else:
        filled = 0
    for index, x in enumerate((24.5, 29.5, 34.5, 39.5)):
        color = gen.SCRAP_LIGHT if index < filled else gen.SCRAP_DARK
        _rounded(draw, (x - 1.8, 49, x + 1.8, 53), 1, fill=_rgba(color))

    if phase in (5, 6):
        bank_index = 0 if phase == 5 else 1
        center_x = bank_centers[bank_index]
        recoil = left_recoil if bank_index == 0 else right_recoil
        flare_width = 7 if tier == 0 else 9
        draw.polygon(
            _points(
                [
                    (center_x, recoil - 1),
                    (center_x - flare_width, recoil + 5),
                    (center_x - 2, recoil + 9),
                    (center_x, recoil + 6),
                    (center_x + 2, recoil + 9),
                    (center_x + flare_width, recoil + 5),
                ]
            ),
            fill=_rgba(gen.SCRAP_LIGHT),
        )
        _ellipse(
            draw,
            (center_x - 2.5, recoil + 2, center_x + 2.5, recoil + 7),
            fill=_rgba(gen.BONE),
        )
    return _finish(image)


def flak_frame(faction: str, tier: int, phase: int) -> Image.Image:
    """Compose a native review-equivalent Flak frame for stability tests."""
    image = flak_base(faction, tier)
    image.alpha_composite(flak_mount(faction, tier, phase))
    return image


def _polar(
    center: tuple[float, float], radius: float, degrees: float
) -> tuple[float, float]:
    angle = math.radians(degrees)
    return (
        center[0] + radius * math.cos(angle),
        center[1] + radius * math.sin(angle),
    )


def deep_array_frame(faction: str, phase: int) -> Image.Image:
    """Draw one approved concentric-gimbal Deep Array sweep pose."""
    if phase not in range(7):
        raise ValueError(f"invalid Deep Array work phase: {phase}")
    palette = gen.FACTIONS[faction]
    image, draw = gen.canvas(64)
    center = (32.0, 32.0)
    heading = ARRAY_HEADINGS[phase]

    _ellipse(draw, (3, 3, 61, 61), fill=_rgba(gen.IRON_DARK))
    _ellipse(draw, (8, 8, 56, 56), fill=_rgba(gen.IRON))
    for x, y in ((9, 12), (55, 12), (9, 52), (55, 52)):
        _beveled_plate(draw, (x - 5, y - 4, x + 5, y + 4), palette["dark"], 2)
    _ellipse(draw, (20, 20, 44, 44), fill=(15, 15, 20, 255))

    start = heading - 50
    end = heading + 50
    fan = [center] + [_polar(center, 17, angle) for angle in range(start, end + 1, 12)]
    draw.polygon(_points(fan), fill=(24, 24, 30, 255))
    draw.arc(
        _box((14, 14, 50, 50)),
        start=start,
        end=end,
        fill=_rgba(palette["light"]),
        width=gen.s(3),
    )
    draw.arc(
        _box((7, 7, 57, 57)),
        start=heading + 65,
        end=heading + 295,
        fill=_rgba(gen.IRON_DARK),
        width=gen.s(7),
    )
    draw.arc(
        _box((9, 9, 55, 55)),
        start=heading + 68,
        end=heading + 292,
        fill=_rgba(gen.IRON_LIGHT),
        width=gen.s(2),
    )
    for offset in (-38, 0, 38):
        _line(
            draw,
            [center, _polar(center, 23, heading + offset)],
            2,
            fill=_rgba(gen.IRON),
        )
    tip = _polar(center, 17, heading)
    _line(draw, [center, tip], 3, fill=_rgba(palette["base"]))
    _ellipse(
        draw,
        (tip[0] - 3, tip[1] - 3, tip[0] + 3, tip[1] + 3),
        fill=_rgba(gen.BONE),
    )

    _ellipse(draw, (25, 25, 39, 39), fill=_rgba(gen.IRON_DARK))
    _ellipse(draw, (28, 28, 36, 36), fill=_rgba(palette["base"]))
    _rect(draw, (31, 30, 33, 34), fill=_rgba(gen.SCRAP_LIGHT))
    return _finish(image)


def _visible_rgba_bytes(image: Image.Image) -> bytes:
    data = bytearray(image.convert("RGBA").tobytes())
    for alpha in range(3, len(data), 4):
        if data[alpha] <= 3:
            data[alpha - 3 : alpha] = b"\0\0\0"
            data[alpha] = 0
    return bytes(data)


def flak_source_visible_digest() -> str:
    """Hash every approved Flak family frame without invisible RGB."""
    digest = hashlib.sha256()
    for faction in ("ferrous", "cupric"):
        for tier in range(2):
            for phase in range(9):
                digest.update(f"flak/{faction}/{tier}/{phase}".encode())
                digest.update(_visible_rgba_bytes(flak_frame(faction, tier, phase)))
    return digest.hexdigest()


def deep_array_source_visible_digest() -> str:
    """Hash every approved Deep Array sweep frame without invisible RGB."""
    digest = hashlib.sha256()
    for faction in ("ferrous", "cupric"):
        for phase in range(7):
            digest.update(f"array/{faction}/{phase}".encode())
            digest.update(_visible_rgba_bytes(deep_array_frame(faction, phase)))
    return digest.hexdigest()


def factions_share_silhouette(image: str, tier: int, phase: int) -> bool:
    """Return whether the two allegiance variants occupy identical pixels."""
    if image == "flak":
        ferrous = flak_frame("ferrous", tier, phase)
        cupric = flak_frame("cupric", tier, phase)
    elif image == "array":
        ferrous = deep_array_frame("ferrous", phase)
        cupric = deep_array_frame("cupric", phase)
    else:
        raise ValueError(f"unknown sprite family: {image}")
    return (
        ImageChops.difference(ferrous.getchannel("A"), cupric.getchannel("A")).getbbox()
        is None
    )


def _put(
    registry: dict[str, Image.Image], out: Path, key: str, image: Image.Image
) -> None:
    native = image.convert("RGBA")
    native.save(out / f"{key}.png")
    registry[key] = native


def install_flak_array(registry: dict[str, Image.Image], out: Path) -> None:
    """Install approved tier-specific Flak and Deep Array production rows."""
    out.mkdir(parents=True, exist_ok=True)
    for faction in ("ferrous", "cupric"):
        for tier, (base_stem, mount_stem) in enumerate(
            (("flak_turret", "flak_mount"), ("flak_turret_t1", "flak_mount_t1"))
        ):
            _put(registry, out, f"{base_stem}_{faction}", flak_base(faction, tier))
            _put(registry, out, f"{mount_stem}_{faction}", flak_mount(faction, tier, 0))
            for phase, suffix in enumerate(FLAK_ACTION_SUFFIXES, start=1):
                _put(
                    registry,
                    out,
                    f"{mount_stem}_{faction}{suffix}",
                    flak_mount(faction, tier, phase),
                )

        _put(registry, out, f"array_t1_{faction}", deep_array_frame(faction, 0))
        for phase, suffix in enumerate(ARRAY_WORK_SUFFIXES, start=1):
            _put(
                registry,
                out,
                f"array_t1_{faction}{suffix}",
                deep_array_frame(faction, phase),
            )
