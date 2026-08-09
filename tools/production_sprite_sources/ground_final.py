"""Native production renderers for finalized ground-machine silhouettes."""

from __future__ import annotations

from PIL import Image, ImageDraw

from tools.gen_sprites import (
    BONE,
    FACTIONS,
    IRON,
    IRON_DARK,
    IRON_LIGHT,
    SCRAP_DARK,
    SCRAP_LIGHT,
    rim_light,
)
from tools.production_sprite_sources import ground_base as ground_ancestry
from tools.production_sprite_sources.ground_base import (
    GroundUnitFrame,
    GroundUnitSequence,
)

SIZE = 64
SS = 4
FERROUS = FACTIONS["ferrous"]
CUPRIC = FACTIONS["cupric"]


def _rgba(color: tuple[int, int, int]) -> tuple[int, int, int, int]:
    return (*color, 255)


def _s(value: int) -> int:
    return value * SS


def _box(box: tuple[int, int, int, int]) -> tuple[int, int, int, int]:
    x0, y0, x1, y1 = box
    return (_s(x0), _s(y0), _s(x1), _s(y1))


def _points(points: tuple[tuple[int, int], ...]) -> list[tuple[int, int]]:
    return [(_s(x), _s(y)) for x, y in points]


def _canvas() -> tuple[Image.Image, ImageDraw.ImageDraw]:
    image = Image.new("RGBA", (SIZE * SS, SIZE * SS), (0, 0, 0, 0))
    return (image, ImageDraw.Draw(image))


def _finish(image: Image.Image) -> Image.Image:
    return rim_light(image.resize((SIZE, SIZE), Image.Resampling.LANCZOS))


def _tracks(
    draw: ImageDraw.ImageDraw, boxes: tuple[tuple[int, int, int, int], ...], phase: int
) -> None:
    for x0, y0, x1, y1 in boxes:
        draw.rounded_rectangle(
            _box((x0, y0, x1, y1)), radius=_s(4), fill=_rgba(IRON_DARK)
        )
        draw.rounded_rectangle(
            _box((x0 + 2, y0 + 2, x1 - 2, y1 - 2)), radius=_s(3), fill=(18, 18, 23, 255)
        )
        travel = max(8, y1 - y0 - 7)
        for index in range(6):
            y = y0 + 3 + (index * 7 + phase * 4) % travel
            if y + 3 >= y1 - 1:
                continue
            color = IRON_LIGHT if (index + phase) % 2 else IRON
            draw.rounded_rectangle(
                _box((x0 + 1, y, x1 - 1, y + 3)), radius=_s(1), fill=_rgba(color)
            )


def _frame_with_image(source: GroundUnitFrame, image: Image.Image) -> GroundUnitFrame:
    return GroundUnitFrame(
        image=image,
        duration_ms=source.duration_ms,
        phase=source.phase,
        event=source.event,
        logical_damage=source.logical_damage,
        report_count=source.report_count,
        recoil_px=source.recoil_px,
    )


def _sentinel_sprite(
    variant: str,
    *,
    tread_phase: int = 0,
    recoil: int = 0,
    locked: bool = False,
    report: bool = False,
) -> Image.Image:
    image, draw = _canvas()
    if variant == "forked":
        _tracks(draw, ((5, 17, 19, 59), (45, 17, 59, 59)), tread_phase)
        draw.polygon(
            _points(((18, 25), (23, 17), (29, 17), (29, 52), (19, 55))),
            fill=_rgba(IRON_DARK),
        )
        draw.polygon(
            _points(((46, 25), (41, 17), (35, 17), (35, 52), (45, 55))),
            fill=_rgba(IRON_DARK),
        )
        draw.rectangle(_box((21, 38, 43, 56)), fill=_rgba(FERROUS["dark"]))
        draw.rectangle(_box((26, 42, 38, 52)), fill=(12, 12, 16, 255))
        draw.rectangle(_box((20, 23, 27, 34)), fill=_rgba(IRON))
        draw.rectangle(_box((37, 23, 44, 34)), fill=_rgba(IRON))
    elif variant == "casemate":
        _tracks(draw, ((8, 25, 20, 59), (44, 25, 56, 59)), tread_phase)
        draw.polygon(
            _points(((13, 24), (20, 16), (44, 16), (51, 24), (47, 52), (17, 52))),
            fill=_rgba(IRON_DARK),
        )
        draw.polygon(
            _points(((18, 25), (23, 20), (41, 20), (46, 25), (43, 45), (21, 45))),
            fill=_rgba(IRON),
        )
        draw.rectangle(_box((15, 27, 49, 35)), fill=_rgba(FERROUS["dark"]))
        draw.rectangle(_box((22, 42, 42, 55)), fill=(15, 14, 17, 255))
    else:
        raise ValueError(f"unknown Sentinel variant: {variant}")
    y = recoil
    draw.rectangle(_box((26, 17 + y, 38, 38 + y)), fill=_rgba(IRON_DARK))
    draw.rectangle(_box((29, 18 + y, 35, 35 + y)), fill=_rgba(IRON_LIGHT))
    draw.rectangle(_box((30, 5 + y, 34, 24 + y)), fill=_rgba(IRON_DARK))
    draw.rectangle(_box((31, 5 + y, 33, 23 + y)), fill=_rgba(BONE))
    draw.rectangle(_box((27, 31 + y, 37, 39 + y)), fill=_rgba(FERROUS["dark"]))
    if locked:
        draw.rectangle(_box((25, 27, 39, 30)), fill=_rgba(SCRAP_DARK))
        draw.rectangle(_box((30, 27, 34, 29)), fill=_rgba(SCRAP_LIGHT))
    if report:
        draw.polygon(
            _points(((28, 5 + y), (32, 2 + y), (36, 5 + y), (33, 8 + y), (31, 8 + y))),
            fill=_rgba(SCRAP_LIGHT),
        )
        draw.rectangle(_box((31, 4 + y, 33, 7 + y)), fill=_rgba(BONE))
    return _finish(image)


def _sentinel_variant_sequence(
    variant: str, stem: str, title: str
) -> GroundUnitSequence:
    baseline = ground_ancestry.sentinel_sequence()
    images = (
        _sentinel_sprite(variant),
        _sentinel_sprite(variant, tread_phase=1),
        _sentinel_sprite(variant, tread_phase=2),
        _sentinel_sprite(variant),
        _sentinel_sprite(variant, locked=True),
        _sentinel_sprite(variant, recoil=4, report=True),
        _sentinel_sprite(variant, recoil=2),
        _sentinel_sprite(variant),
    )
    return GroundUnitSequence(
        stem=stem,
        title=title,
        mechanism="restrained Open Works chassis around the accepted breech-stamp gun",
        mechanism_box=(4, 1, 60, 61),
        attack_contract=baseline.attack_contract,
        frames=tuple(
            (
                _frame_with_image(frame, image)
                for frame, image in zip(baseline.frames, images, strict=True)
            )
        ),
    )


def _centipede_sprite(*, leg_pose: int = 0, bite: int = 0) -> Image.Image:
    image, draw = _canvas()
    offsets = (
        (0, 0, 0) if leg_pose == 0 else (-3, 2, -2) if leg_pose == 1 else (3, -2, 2)
    )
    for side in (-1, 1):
        for index, y in enumerate((24, 37, 50)):
            anchor_x = 26 if side < 0 else 38
            elbow_x = 16 if side < 0 else 48
            foot_x = 6 if side < 0 else 58
            shift = offsets[index] * side
            elbow = (elbow_x, y + shift)
            foot = (foot_x, y + shift + (-3 if index == 0 else 3 if index == 2 else 0))
            draw.line(
                _points(((anchor_x, y), elbow)), fill=_rgba(IRON_DARK), width=_s(6)
            )
            draw.line(
                _points(((anchor_x, y), elbow)), fill=_rgba(IRON_LIGHT), width=_s(2)
            )
            draw.line(_points((elbow, foot)), fill=_rgba(IRON_DARK), width=_s(5))
            draw.rounded_rectangle(
                _box((foot[0] - 3, foot[1] - 2, foot[0] + 3, foot[1] + 2)),
                radius=_s(1),
                fill=_rgba(IRON_LIGHT),
            )
    draw.rounded_rectangle(_box((24, 13, 40, 58)), radius=_s(6), fill=_rgba(IRON_DARK))
    draw.rectangle(_box((28, 18, 36, 53)), fill=_rgba(FERROUS["dark"]))
    for y in (23, 32, 41, 50):
        draw.rectangle(_box((27, y, 37, y + 4)), fill=_rgba(IRON))
    jaw_y = 8 - (2 if bite else 0)
    gap = 1 if bite == 2 else 5
    draw.line(_points(((28, 20), (22, jaw_y + 6))), fill=_rgba(IRON_LIGHT), width=_s(4))
    draw.line(_points(((36, 20), (42, jaw_y + 6))), fill=_rgba(IRON_LIGHT), width=_s(4))
    draw.polygon(
        _points(
            ((18, jaw_y + 2), (31 - gap, jaw_y + 3), (29, jaw_y + 12), (20, jaw_y + 9))
        ),
        fill=_rgba(IRON_DARK),
    )
    draw.polygon(
        _points(
            ((46, jaw_y + 2), (33 + gap, jaw_y + 3), (35, jaw_y + 12), (44, jaw_y + 9))
        ),
        fill=_rgba(IRON_DARK),
    )
    if bite == 2:
        draw.rectangle(_box((29, jaw_y + 5, 35, jaw_y + 9)), fill=_rgba(SCRAP_LIGHT))
    return _finish(image)


def _centipede_sequence() -> GroundUnitSequence:
    return GroundUnitSequence(
        stem="scuttler_centipede_shear",
        title="Scuttler / Centipede Shear",
        mechanism="long six-leg body ending in one paired scrap shear",
        mechanism_box=(3, 2, 61, 61),
        attack_contract="one forward shear bite and one logical damage report",
        frames=(
            GroundUnitFrame(_centipede_sprite(), 380, "idle", "idle"),
            GroundUnitFrame(
                _centipede_sprite(leg_pose=1), 130, "locomotion", "leg_step_a"
            ),
            GroundUnitFrame(
                _centipede_sprite(leg_pose=2), 130, "locomotion", "leg_step_b"
            ),
            GroundUnitFrame(_centipede_sprite(), 220, "settle", "travel_settle"),
            GroundUnitFrame(
                _centipede_sprite(bite=1), 150, "anticipation", "shear_open"
            ),
            GroundUnitFrame(
                _centipede_sprite(bite=2),
                120,
                "attack",
                "damage+shear_bite",
                logical_damage=True,
                report_count=1,
            ),
            GroundUnitFrame(
                _centipede_sprite(bite=1), 180, "recovery", "shear_release"
            ),
            GroundUnitFrame(_centipede_sprite(), 440, "settle", "attack_settle"),
        ),
    )


def _lancer_sprite(
    variant: str,
    *,
    tread_phase: int = 0,
    recoil: int = 0,
    charge_level: int = 0,
    report: bool = False,
) -> Image.Image:
    image, draw = _canvas()
    if variant == "fork":
        _tracks(draw, ((8, 30, 18, 59), (46, 30, 56, 59)), tread_phase)
        draw.rectangle(_box((20, 37, 44, 57)), fill=_rgba(FERROUS["dark"]))
        draw.rectangle(_box((25, 42, 39, 53)), fill=(11, 11, 15, 255))
        rail_boxes = ((21, 6, 28, 44), (36, 6, 43, 44))
        draw.rectangle(_box((18, 31, 46, 38)), fill=_rgba(IRON_DARK))
    elif variant == "triangle":
        _tracks(draw, ((10, 35, 18, 58), (46, 35, 54, 58)), tread_phase)
        draw.polygon(
            _points(((32, 12), (52, 47), (43, 59), (21, 59), (12, 47))),
            fill=_rgba(IRON_DARK),
        )
        draw.polygon(
            _points(((32, 18), (46, 46), (39, 54), (25, 54), (18, 46))),
            fill=_rgba(FERROUS["dark"]),
        )
        rail_boxes = ((25, 5, 30, 43), (34, 5, 39, 43))
    else:
        raise ValueError(f"unknown Lancer variant: {variant}")
    y = recoil
    for x0, y0, x1, y1 in rail_boxes:
        draw.rounded_rectangle(
            _box((x0, y0 + y, x1, y1 + y)), radius=_s(2), fill=_rgba(IRON_DARK)
        )
        draw.rectangle(
            _box((x0 + 2, y0 + 2 + y, x1 - 2, y1 - 3 + y)), fill=_rgba(IRON_LIGHT)
        )
    draw.rectangle(_box((27, 32 + y, 37, 43 + y)), fill=_rgba(IRON_DARK))
    draw.rectangle(_box((30, 5 + y, 34, 36 + y)), fill=_rgba(BONE))
    if report:
        draw.polygon(
            _points(((28, 5 + y), (32, 2 + y), (36, 5 + y), (33, 9 + y), (31, 9 + y))),
            fill=_rgba(SCRAP_LIGHT),
        )
    for index, x in enumerate((23, 31, 39)):
        draw.rounded_rectangle(
            _box((x, 51, x + 6, 59)), radius=_s(2), fill=_rgba(IRON_DARK)
        )
        color = SCRAP_LIGHT if index < charge_level else SCRAP_DARK
        draw.rectangle(_box((x + 2, 53, x + 4, 57)), fill=_rgba(color))
    return _finish(image)


def _lancer_variant_sequence(variant: str, stem: str, title: str) -> GroundUnitSequence:
    baseline = ground_ancestry.lancer_sequence()
    images = (
        _lancer_sprite(variant),
        _lancer_sprite(variant, tread_phase=1),
        _lancer_sprite(variant, tread_phase=2),
        _lancer_sprite(variant),
        _lancer_sprite(variant, charge_level=1),
        _lancer_sprite(variant, charge_level=2),
        _lancer_sprite(variant, charge_level=3),
        _lancer_sprite(variant, recoil=5, report=True),
        _lancer_sprite(variant, recoil=2),
        _lancer_sprite(variant),
    )
    return GroundUnitSequence(
        stem=stem,
        title=title,
        mechanism="approved three-cell lance sequence on a distinct open carriage",
        mechanism_box=(6, 1, 58, 62),
        attack_contract=baseline.attack_contract,
        frames=tuple(
            (
                _frame_with_image(frame, image)
                for frame, image in zip(baseline.frames, images, strict=True)
            )
        ),
    )


def _wheel(
    draw: ImageDraw.ImageDraw, x: int, y: int, *, phase: int, horizontal: bool = False
) -> None:
    if horizontal:
        draw.rounded_rectangle(
            _box((x - 5, y - 3, x + 5, y + 3)), radius=_s(2), fill=_rgba(IRON_DARK)
        )
        draw.rectangle(
            _box((x - 2 + phase, y - 2, x + phase, y + 2)), fill=_rgba(IRON_LIGHT)
        )
    else:
        draw.rounded_rectangle(
            _box((x - 3, y - 5, x + 3, y + 5)), radius=_s(2), fill=_rgba(IRON_DARK)
        )
        draw.rectangle(
            _box((x - 2, y - 2 + phase, x + 2, y + phase)), fill=_rgba(IRON_LIGHT)
        )


def _stinger_sprite(
    variant: str,
    *,
    move_phase: int = 0,
    ready: bool = False,
    recoil: int = 0,
    report: bool = False,
) -> Image.Image:
    image, draw = _canvas()
    if variant == "trike":
        for x, y in ((10, 19), (54, 19), (32, 56)):
            _wheel(draw, x, y, phase=move_phase)
        draw.line(_points(((28, 34), (12, 20))), fill=_rgba(IRON_DARK), width=_s(6))
        draw.line(_points(((36, 34), (52, 20))), fill=_rgba(IRON_DARK), width=_s(6))
        draw.line(_points(((32, 39), (32, 54))), fill=_rgba(IRON_DARK), width=_s(7))
        draw.rounded_rectangle(
            _box((23, 25, 41, 48)), radius=_s(5), fill=_rgba(IRON_DARK)
        )
        draw.rectangle(_box((27, 30, 37, 43)), fill=_rgba(CUPRIC["dark"]))
        centers = (26, 38)
    elif variant == "reel":
        _wheel(draw, 10, 34, phase=move_phase)
        _wheel(draw, 54, 34, phase=move_phase)
        draw.rectangle(_box((16, 27, 48, 42)), fill=_rgba(IRON_DARK))
        draw.ellipse(_box((20, 21, 44, 45)), fill=_rgba(CUPRIC["dark"]))
        draw.ellipse(_box((25, 26, 39, 40)), fill=(13, 13, 17, 255))
        for radius in (5, 9):
            draw.arc(
                _box((32 - radius, 33 - radius, 32 + radius, 33 + radius)),
                20,
                340,
                fill=_rgba(IRON_LIGHT),
                width=_s(2),
            )
        draw.rectangle(_box((27, 40, 37, 59)), fill=_rgba(IRON_DARK))
        _wheel(draw, 32, 56, phase=move_phase, horizontal=True)
        centers = (26, 38)
    elif variant == "mast":
        for x, y in ((18, 45), (46, 45), (18, 56), (46, 56)):
            _wheel(draw, x, y, phase=move_phase)
        draw.polygon(
            _points(((24, 56), (25, 22), (39, 22), (40, 56))), fill=_rgba(IRON_DARK)
        )
        draw.rectangle(_box((29, 27, 35, 53)), fill=_rgba(CUPRIC["dark"]))
        centers = (28, 36)
        if ready:
            draw.rectangle(_box((24, 17, 40, 24)), fill=_rgba(IRON_LIGHT))
    else:
        raise ValueError(f"unknown Stinger variant: {variant}")
    lift = -2 if ready else 0
    for center in centers:
        draw.rounded_rectangle(
            _box((center - 2, 7 + lift + recoil, center + 2, 31 + lift + recoil)),
            radius=_s(1),
            fill=_rgba(IRON_DARK),
        )
        draw.rectangle(
            _box((center - 1, 9 + lift + recoil, center + 1, 28 + lift + recoil)),
            fill=_rgba(BONE),
        )
    draw.rectangle(
        _box((centers[0] - 4, 27 + lift, centers[1] + 4, 34 + lift)),
        fill=_rgba(CUPRIC["dark"]),
    )
    if report:
        draw.rectangle(
            _box((centers[0] - 4, 5 + recoil, centers[1] + 4, 8 + recoil)),
            fill=_rgba(SCRAP_LIGHT),
        )
        draw.rectangle(_box((30, 4 + recoil, 34, 9 + recoil)), fill=_rgba(BONE))
    return _finish(image)


def _stinger_sequence(
    variant: str, stem: str, title: str, mechanism: str
) -> GroundUnitSequence:
    return GroundUnitSequence(
        stem=stem,
        title=title,
        mechanism=mechanism,
        mechanism_box=(4, 2, 60, 62),
        attack_contract="one aggregate paired-barrel burst and one logical damage event",
        frames=(
            GroundUnitFrame(_stinger_sprite(variant), 400, "idle", "idle"),
            GroundUnitFrame(
                _stinger_sprite(variant, move_phase=1),
                130,
                "locomotion",
                "wheel_step_a",
            ),
            GroundUnitFrame(
                _stinger_sprite(variant, move_phase=2),
                130,
                "locomotion",
                "wheel_step_b",
            ),
            GroundUnitFrame(_stinger_sprite(variant), 220, "settle", "travel_settle"),
            GroundUnitFrame(
                _stinger_sprite(variant, ready=True),
                160,
                "anticipation",
                "paired_yoke_lock",
            ),
            GroundUnitFrame(
                _stinger_sprite(variant, ready=True, recoil=4, report=True),
                100,
                "attack",
                "damage+paired_aa_burst",
                logical_damage=True,
                report_count=1,
                recoil_px=4,
            ),
            GroundUnitFrame(
                _stinger_sprite(variant, ready=True, recoil=2),
                150,
                "recoil",
                "paired_yoke_return",
                recoil_px=2,
            ),
            GroundUnitFrame(_stinger_sprite(variant), 460, "settle", "attack_settle"),
        ),
    )


def sentinel_sequence() -> GroundUnitSequence:
    return _sentinel_variant_sequence(
        "casemate", "sentinel_low_casemate", "Sentinel / Low Casemate"
    )


def scuttler_sequence() -> GroundUnitSequence:
    return _centipede_sequence()


def lancer_tuning_fork_sequence() -> GroundUnitSequence:
    return _lancer_variant_sequence(
        "fork", "lancer_tuning_fork", "Lancer / Tuning-Fork Accelerator"
    )


def stinger_sequence() -> GroundUnitSequence:
    return _stinger_sequence(
        "trike",
        "stinger_inspection_trike",
        "Stinger / Forked Inspection Trike",
        "three-wheel inspection cart with a small paired AA yoke",
    )
