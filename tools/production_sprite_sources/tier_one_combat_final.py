"""Approved production frames for Lancer, Bombard, Flakhound, and Stinger.

The renderers preserve the selected review candidates byte-for-byte while the
sequence builders retain the established movement, attack, report, and damage
metadata used by the shell.
"""

from __future__ import annotations

import hashlib
from collections.abc import Iterator
from dataclasses import replace

from PIL import Image, ImageDraw

from tools import gen_sprites as gen
from tools.production_sprite_sources import (
    ground_artillery,
    ground_base,
    ground_final,
    lancer_final,
)

Palette = dict[str, tuple[int, int, int]]

SIZE = 64
SS = 4
FERROUS = gen.FACTIONS["ferrous"]
CUPRIC = gen.FACTIONS["cupric"]
BLACK = (10, 10, 14)

# Semantic RGBA digest of approved review candidates 528, 533, 538, and 540.
APPROVED_SOURCE_RGBA_SHA256 = (
    "4ab2e2bd9e3fcb0ed95229dd2bb44d57d1f48834da6c2d3ef04cd0f7f9868117"
)


def _s(value: float) -> int:
    return round(value * SS)


def _box(
    values: tuple[int | float, int | float, int | float, int | float],
) -> tuple[int, int, int, int]:
    return tuple(_s(value) for value in values)  # type: ignore[return-value]


def _points(
    values: tuple[tuple[int | float, int | float], ...],
) -> list[tuple[int, int]]:
    return [(_s(x), _s(y)) for x, y in values]


def _rgba(color: tuple[int, int, int], alpha: int = 255) -> tuple[int, int, int, int]:
    return (*color, alpha)


def _canvas() -> tuple[Image.Image, ImageDraw.ImageDraw]:
    image = Image.new("RGBA", (SIZE * SS, SIZE * SS), (0, 0, 0, 0))
    return image, ImageDraw.Draw(image)


def _finish(image: Image.Image) -> Image.Image:
    native = image.resize((SIZE, SIZE), Image.Resampling.LANCZOS)
    return gen.rim_light(native)


def _plate(
    draw: ImageDraw.ImageDraw,
    bounds: tuple[int, int, int, int],
    fill: tuple[int, int, int],
    *,
    radius: int = 3,
    inset: int = 2,
) -> None:
    draw.rounded_rectangle(_box(bounds), radius=_s(radius), fill=_rgba(gen.IRON_DARK))
    x0, y0, x1, y1 = bounds
    draw.rounded_rectangle(
        _box((x0 + inset, y0 + inset, x1 - inset, y1 - inset)),
        radius=_s(max(1, radius - 1)),
        fill=_rgba(fill),
    )
    draw.line(
        _points(((x0 + inset + 1, y0 + inset + 1), (x1 - inset - 1, y0 + inset + 1))),
        fill=_rgba(gen.IRON_LIGHT, 210),
        width=_s(1),
    )


def _tracks(
    draw: ImageDraw.ImageDraw,
    bounds: tuple[tuple[int, int, int, int], ...],
    phase: int,
    palette: Palette,
) -> None:
    for side, (x0, y0, x1, y1) in enumerate(bounds):
        draw.rounded_rectangle(
            _box((x0, y0, x1, y1)), radius=_s(4), fill=_rgba(gen.IRON_DARK)
        )
        draw.rounded_rectangle(
            _box((x0 + 2, y0 + 2, x1 - 2, y1 - 2)),
            radius=_s(3),
            fill=_rgba(BLACK),
        )
        travel = max(10, y1 - y0 - 8)
        for index in range(7):
            y = y0 + 3 + (index * 7 + phase * 3) % travel
            if y + 3 >= y1 - 1:
                continue
            color = (
                palette["dark"] if (index + side + phase) % 4 == 0 else gen.IRON_LIGHT
            )
            draw.rounded_rectangle(
                _box((x0 + 1, y, x1 - 1, y + 3)),
                radius=_s(1),
                fill=_rgba(color),
            )


def _wheel(
    draw: ImageDraw.ImageDraw,
    center: tuple[int, int],
    phase: int,
    palette: Palette,
    radius: int = 5,
) -> None:
    cx, cy = center
    radius_x = max(3, radius - 2)
    draw.rounded_rectangle(
        _box((cx - radius_x, cy - radius, cx + radius_x, cy + radius)),
        radius=_s(radius_x),
        fill=_rgba(gen.IRON_DARK),
    )
    draw.ellipse(
        _box(
            (
                cx - radius_x + 1,
                cy - radius + 2,
                cx + radius_x - 1,
                cy + radius - 2,
            )
        ),
        fill=_rgba(gen.IRON),
    )
    spoke = (
        ((cx, cy - radius + 2), (cx, cy + radius - 2)),
        ((cx - radius_x + 1, cy - 2), (cx + radius_x - 1, cy + 2)),
        ((cx - radius_x + 1, cy), (cx + radius_x - 1, cy)),
    )[phase % 3]
    draw.line(_points(spoke), fill=_rgba(palette["light"]), width=_s(1))
    draw.ellipse(_box((cx - 1, cy - 1, cx + 1, cy + 1)), fill=_rgba(BLACK))


def _bolt(draw: ImageDraw.ImageDraw, x: int, y: int) -> None:
    draw.ellipse(_box((x - 1, y - 1, x + 1, y + 1)), fill=_rgba(gen.IRON_LIGHT))


def _shell(draw: ImageDraw.ImageDraw, x: int, y: int) -> None:
    draw.rounded_rectangle(
        _box((x - 2, y - 5, x + 2, y + 5)),
        radius=_s(1),
        fill=_rgba(gen.SCRAP_DARK),
    )
    draw.polygon(
        _points(((x, y - 6), (x - 2, y - 3), (x + 2, y - 3))),
        fill=_rgba(gen.SCRAP_LIGHT),
    )


def _muzzle_flash(draw: ImageDraw.ImageDraw, x: int, y: int, *, width: int = 7) -> None:
    draw.polygon(
        _points(
            (
                (x, y),
                (x - width, y + 5),
                (x - 3, y + 7),
                (x - 1, y + 12),
                (x + 1, y + 12),
                (x + 3, y + 7),
                (x + width, y + 5),
            )
        ),
        fill=_rgba(gen.SCRAP_LIGHT),
    )
    draw.rectangle(_box((x - 2, y + 4, x + 2, y + 10)), fill=_rgba(gen.BONE))


def _bore(draw: ImageDraw.ImageDraw, x: int, y: int, *, radius: int = 4) -> None:
    draw.ellipse(
        _box((x - radius, y - radius + 1, x + radius, y + radius - 1)),
        fill=_rgba(gen.IRON_LIGHT),
    )
    draw.ellipse(
        _box((x - radius + 2, y - radius + 2, x + radius - 2, y + radius - 2)),
        fill=_rgba(BLACK),
    )
    draw.arc(
        _box((x - radius, y - radius + 1, x + radius, y + radius - 1)),
        195,
        335,
        fill=_rgba(gen.BONE),
        width=_s(1),
    )


def _lancer_sprite(
    *, tread_phase: int = 0, charge: int = 0, recoil: int = 0, report: bool = False
) -> Image.Image:
    image, draw = _canvas()
    palette = FERROUS
    _tracks(draw, ((5, 25, 17, 61), (47, 25, 59, 61)), tread_phase, palette)
    draw.polygon(
        _points(((15, 31), (21, 22), (43, 22), (49, 31), (45, 58), (19, 58))),
        fill=_rgba(gen.IRON_DARK),
    )
    draw.polygon(
        _points(((20, 33), (24, 27), (40, 27), (44, 33), (41, 54), (23, 54))),
        fill=_rgba(palette["dark"]),
    )
    for cx in (24, 32, 40):
        _plate(draw, (cx - 3, 46, cx + 3, 57), gen.IRON, radius=2, inset=1)
    draw.line(
        _points(((18, 40), (22, 38), (25, 43))),
        fill=_rgba(palette["light"]),
        width=_s(2),
    )
    draw.line(
        _points(((46, 40), (42, 38), (39, 43))),
        fill=_rgba(palette["light"]),
        width=_s(2),
    )
    draw.rectangle(_box((18, 32, 46, 40)), fill=_rgba(gen.IRON_DARK))
    for x0, y0, x1, y1 in ((20, 5, 28, 40), (36, 5, 44, 40)):
        draw.rounded_rectangle(
            _box((x0, y0 + recoil, x1, y1 + recoil)),
            radius=_s(2),
            fill=_rgba(gen.IRON_DARK),
        )
        draw.rectangle(
            _box((x0 + 2, y0 + 2 + recoil, x1 - 2, y1 - 2 + recoil)),
            fill=_rgba(gen.IRON_LIGHT),
        )
        draw.line(
            _points(((x0 + 2, y0 + 4 + recoil), (x1 - 2, y0 + 4 + recoil))),
            fill=_rgba(gen.BONE),
            width=_s(1),
        )
    channel_top = 34 - charge * 8
    draw.rectangle(
        _box((30, max(7, channel_top) + recoil, 34, 38 + recoil)),
        fill=_rgba(gen.SCRAP_LIGHT if charge else BLACK),
    )
    for index, cx in enumerate((24, 32, 40)):
        color = gen.SCRAP_LIGHT if index < charge else gen.SCRAP_DARK
        draw.rectangle(_box((cx - 2, 50, cx + 2, 55)), fill=_rgba(color))
    for x, y in ((20, 40), (44, 40), (22, 56), (42, 56)):
        _bolt(draw, x, y)
    if report:
        _muzzle_flash(draw, 32, 0, width=6)
    return _finish(image)


def _bombard_sprite(
    *,
    tread_phase: int = 0,
    stage: int = 0,
    recoil: int = 0,
    spades: bool = False,
    report: bool = False,
) -> Image.Image:
    image, draw = _canvas()
    palette = FERROUS
    _tracks(draw, ((6, 24, 17, 56), (47, 24, 58, 56)), tread_phase, palette)
    draw.polygon(
        _points(((13, 28), (21, 18), (43, 18), (51, 28), (47, 57), (17, 57))),
        fill=_rgba(gen.IRON_DARK),
    )
    draw.polygon(
        _points(((19, 30), (25, 23), (39, 23), (45, 30), (41, 53), (23, 53))),
        fill=_rgba(palette["dark"]),
    )
    _plate(draw, (20, 38, 44, 57), gen.IRON, radius=4)
    for index, cx in enumerate((25, 32, 39)):
        if index == 1 and stage >= 1:
            continue
        _shell(draw, cx, 48)
    draw.ellipse(_box((21, 8 + recoil, 43, 32 + recoil)), fill=_rgba(gen.IRON_DARK))
    draw.ellipse(_box((25, 12 + recoil, 39, 27 + recoil)), fill=_rgba(BLACK))
    draw.arc(
        _box((23, 10 + recoil, 41, 29 + recoil)),
        195,
        340,
        fill=_rgba(gen.BONE),
        width=_s(2),
    )
    draw.rectangle(_box((29, 29, 35, 43)), fill=_rgba(gen.IRON_DARK))
    if stage == 1:
        _shell(draw, 32, 41)
    elif stage == 2:
        _shell(draw, 32, 34)
    elif stage >= 3 and recoil == 0:
        draw.rectangle(_box((30, 31, 34, 38)), fill=_rgba(gen.SCRAP_DARK))
    for arm, _, foot in (
        ((20, 48), (13, 60), (4, 60)),
        ((44, 48), (51, 60), (60, 60)),
    ):
        if spades:
            draw.line(_points((arm, foot)), fill=_rgba(gen.IRON_LIGHT), width=_s(3))
            fx, fy = foot
            draw.polygon(
                _points(
                    (
                        (fx - 5, fy - 3),
                        (fx + 5, fy - 3),
                        (fx + 7, fy + 1),
                        (fx - 7, fy + 1),
                    )
                ),
                fill=_rgba(gen.IRON_DARK),
            )
    if report:
        _muzzle_flash(draw, 32, 0, width=8)
    return _finish(image)


def _aa_barrel(
    draw: ImageDraw.ImageDraw,
    x: int,
    *,
    y: int,
    recoil: int,
    palette: Palette,
    report: bool,
    narrow: bool = False,
) -> None:
    width = 6 if narrow else 8
    draw.rounded_rectangle(
        _box((x - width / 2, y + recoil, x + width / 2, 31 + recoil)),
        radius=_s(2),
        fill=_rgba(gen.IRON_DARK),
    )
    draw.rectangle(
        _box((x - width / 2 + 2, y + 3 + recoil, x + width / 2 - 2, 28 + recoil)),
        fill=_rgba(palette["dark"]),
    )
    _bore(draw, x, y + 2 + recoil, radius=3 if narrow else 4)
    if report:
        _muzzle_flash(draw, x, max(0, y - 4), width=5 if narrow else 6)


def _charge_lamps(
    draw: ImageDraw.ImageDraw, charge: int, palette: Palette, *, y: int = 52
) -> None:
    for index, x in enumerate((23, 29, 35, 41)):
        draw.ellipse(_box((x - 2, y - 2, x + 2, y + 2)), fill=_rgba(gen.IRON_DARK))
        color = palette["light"] if index < charge else palette["dark"]
        draw.ellipse(_box((x - 1, y - 1, x + 1, y + 1)), fill=_rgba(color))


def _flakhound_sprite(
    *,
    tread_phase: int = 0,
    charge: int = 0,
    report_side: str | None = None,
    recover: bool = False,
) -> Image.Image:
    image, draw = _canvas()
    palette = FERROUS
    _tracks(draw, ((5, 31, 17, 61), (47, 31, 59, 61)), tread_phase, palette)
    for center in ((12, 24), (52, 24)):
        _wheel(draw, center, tread_phase, palette, 6)
    draw.polygon(
        _points(((11, 32), (18, 18), (46, 18), (53, 32), (47, 59), (17, 59))),
        fill=_rgba(gen.IRON_DARK),
    )
    draw.polygon(
        _points(((18, 33), (23, 23), (41, 23), (46, 33), (41, 54), (23, 54))),
        fill=_rgba(palette["dark"]),
    )
    draw.rectangle(_box((17, 37, 47, 45)), fill=_rgba(gen.IRON_DARK))
    for x in (22, 42):
        draw.line(_points(((x, 42), (32, 48))), fill=_rgba(gen.SCRAP_DARK), width=_s(4))
    xs = (22, 29, 35, 42)
    left_recoil = 4 if report_side == "left" else (2 if recover else 0)
    right_recoil = 4 if report_side == "right" else (2 if recover else 0)
    for index, x in enumerate(xs):
        side = "left" if index < 2 else "right"
        recoil = left_recoil if side == "left" else right_recoil
        report = report_side == side and index % 2 == 0
        _aa_barrel(draw, x, y=5, recoil=recoil, palette=palette, report=report)
        draw.line(
            _points(((x, 29 + recoil), (32, 39))),
            fill=_rgba(gen.IRON_LIGHT),
            width=_s(2),
        )
    draw.rectangle(_box((18, 38, 46, 43)), fill=_rgba(gen.IRON_DARK))
    _charge_lamps(draw, charge, palette)
    for x, y in ((18, 29), (46, 29), (20, 56), (44, 56)):
        _bolt(draw, x, y)
    return _finish(image)


def _stinger_sprite(
    *, move_phase: int = 0, ready: bool = False, recoil: int = 0, report: bool = False
) -> Image.Image:
    image, draw = _canvas()
    palette = CUPRIC
    for center in ((12, 43), (52, 43), (32, 57)):
        _wheel(draw, center, move_phase, palette, 6 if center[1] == 43 else 5)
    draw.line(_points(((20, 40), (12, 43))), fill=_rgba(gen.IRON_DARK), width=_s(4))
    draw.line(_points(((44, 40), (52, 43))), fill=_rgba(gen.IRON_DARK), width=_s(4))
    draw.line(_points(((32, 48), (32, 57))), fill=_rgba(gen.IRON_DARK), width=_s(4))
    draw.polygon(
        _points(((32, 19), (47, 39), (40, 52), (24, 52), (17, 39))),
        fill=_rgba(gen.IRON_DARK),
    )
    draw.polygon(
        _points(((32, 24), (42, 39), (37, 48), (27, 48), (22, 39))),
        fill=_rgba(palette["base"]),
    )
    _plate(draw, (25, 36, 39, 49), palette["dark"], radius=3)
    yoke_shift = -1 if ready else 0
    for index, x in enumerate((26, 38)):
        shifted_x = x + (yoke_shift if index == 0 else -yoke_shift)
        _aa_barrel(
            draw,
            shifted_x,
            y=7,
            recoil=recoil,
            palette=palette,
            report=report,
            narrow=True,
        )
        draw.ellipse(
            _box((shifted_x - 4, 27 + recoil, shifted_x + 4, 35 + recoil)),
            fill=_rgba(gen.IRON_DARK),
        )
        draw.ellipse(
            _box((shifted_x - 2, 29 + recoil, shifted_x + 2, 33 + recoil)),
            fill=_rgba(palette["light"]),
        )
    draw.rectangle(_box((23, 34, 41, 39)), fill=_rgba(gen.IRON_DARK))
    for x, y in ((24, 44), (40, 44)):
        _bolt(draw, x, y)
    return _finish(image)


def _with_images(
    source: ground_base.GroundUnitSequence,
    *,
    stem: str,
    title: str,
    mechanism: str,
    images: tuple[Image.Image, ...],
) -> ground_base.GroundUnitSequence:
    return replace(
        source,
        stem=stem,
        title=title,
        mechanism=mechanism,
        frames=tuple(
            replace(frame, image=image)
            for frame, image in zip(source.frames, images, strict=True)
        ),
    )


def lancer_sequence() -> ground_base.GroundUnitSequence:
    images = (
        _lancer_sprite(),
        _lancer_sprite(tread_phase=1),
        _lancer_sprite(tread_phase=2),
        _lancer_sprite(),
        _lancer_sprite(charge=1),
        _lancer_sprite(charge=2),
        _lancer_sprite(charge=3),
        _lancer_sprite(charge=3, recoil=5, report=True),
        _lancer_sprite(charge=2, recoil=2),
        _lancer_sprite(),
    )
    return _with_images(
        lancer_final.lancer_sequence(),
        stem="lancer_capacitor_sled",
        title="Lancer / Capacitor Sled",
        mechanism="broad tracked sled with three armored capacitors feeding twin rails",
        images=images,
    )


def bombard_sequence() -> ground_base.GroundUnitSequence:
    images = (
        _bombard_sprite(),
        _bombard_sprite(tread_phase=1),
        _bombard_sprite(tread_phase=2),
        _bombard_sprite(),
        _bombard_sprite(stage=1),
        _bombard_sprite(stage=2),
        _bombard_sprite(stage=3, spades=True),
        _bombard_sprite(stage=4, recoil=8, spades=True, report=True),
        _bombard_sprite(stage=4, recoil=3, spades=True),
        _bombard_sprite(),
    )
    return _with_images(
        ground_artillery.bombard_sequence(),
        stem="bombard_drum_mortar",
        title="Bombard / Drum Mortar",
        mechanism="armored vertical mortar fed from a visible three-shell drum",
        images=images,
    )


def flakhound_sequence() -> ground_base.GroundUnitSequence:
    images = (
        _flakhound_sprite(),
        _flakhound_sprite(tread_phase=1),
        _flakhound_sprite(tread_phase=2),
        _flakhound_sprite(),
        *(_flakhound_sprite(charge=charge) for charge in range(5)),
        _flakhound_sprite(charge=4, report_side="left"),
        _flakhound_sprite(charge=4, report_side="right"),
        _flakhound_sprite(charge=2, recover=True),
        _flakhound_sprite(),
    )
    return _with_images(
        ground_base.flakhound_sequence(),
        stem="flakhound_split_feed_halftrack",
        title="Flakhound / Split-Feed Halftrack",
        mechanism="two front wheels and rear tracks carrying split-fed paired AA yokes",
        images=images,
    )


def stinger_sequence() -> ground_base.GroundUnitSequence:
    images = (
        _stinger_sprite(),
        _stinger_sprite(move_phase=1),
        _stinger_sprite(move_phase=2),
        _stinger_sprite(),
        _stinger_sprite(ready=True),
        _stinger_sprite(ready=True, recoil=4, report=True),
        _stinger_sprite(ready=True, recoil=2),
        _stinger_sprite(),
    )
    return _with_images(
        ground_final.stinger_sequence(),
        stem="stinger_armored_trike",
        title="Stinger / Armored Trike",
        mechanism="three-wheel armored scout carriage with a paired AA yoke",
        images=images,
    )


def source_frames() -> Iterator[tuple[str, Image.Image]]:
    for review_id, sequence in (
        (528, lancer_sequence()),
        (533, bombard_sequence()),
        (538, flakhound_sequence()),
        (540, stinger_sequence()),
    ):
        for index, frame in enumerate(sequence.frames):
            yield f"{review_id}/{index}", frame.image


def source_rgba_digest() -> str:
    digest = hashlib.sha256()
    for key, image in source_frames():
        digest.update(key.encode())
        digest.update(image.tobytes())
    return digest.hexdigest()
