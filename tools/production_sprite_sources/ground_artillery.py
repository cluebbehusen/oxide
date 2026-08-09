"""Native production renderer for the finalized Bombard."""

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
from tools.production_sprite_sources.ground_base import (
    GroundUnitFrame,
    GroundUnitSequence,
)

SIZE = 64
SS = 4
FERROUS = FACTIONS["ferrous"]


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
    draw: ImageDraw.ImageDraw,
    *,
    left: tuple[int, int, int, int],
    right: tuple[int, int, int, int],
    phase: int,
) -> None:
    for x0, y0, x1, y1 in (left, right):
        draw.rounded_rectangle(
            _box((x0, y0, x1, y1)), radius=_s(4), fill=_rgba(IRON_DARK)
        )
        draw.rounded_rectangle(
            _box((x0 + 2, y0 + 2, x1 - 2, y1 - 2)), radius=_s(3), fill=(20, 20, 24, 255)
        )
        travel = y1 - y0 - 8
        offset = phase * 4
        for index in range(6):
            y = y0 + 3 + (index * 7 + offset) % travel
            if y + 3 >= y1 - 1:
                continue
            color = IRON_LIGHT if (index + phase) % 2 else IRON
            draw.rounded_rectangle(
                _box((x0 + 1, y, x1 - 1, y + 3)), radius=_s(1), fill=_rgba(color)
            )
        draw.line(
            ((_s(x0 + 2), _s(y0 + 5)), (_s(x0 + 2), _s(y1 - 5))),
            fill=_rgba(IRON_LIGHT),
            width=_s(1),
        )


def _muzzle_report(draw: ImageDraw.ImageDraw, *, center: int, y: int) -> None:
    draw.polygon(
        _points(
            (
                (center - 7, y + 5),
                (center, y),
                (center + 7, y + 5),
                (center + 3, y + 11),
                (center - 3, y + 11),
            )
        ),
        fill=_rgba(SCRAP_LIGHT),
    )
    draw.rectangle(_box((center - 2, y + 4, center + 2, y + 9)), fill=_rgba(BONE))


def _shell(
    draw: ImageDraw.ImageDraw, *, center_x: int, center_y: int, horizontal: bool = False
) -> None:
    if horizontal:
        draw.rounded_rectangle(
            _box((center_x - 5, center_y - 2, center_x + 5, center_y + 2)),
            radius=_s(1),
            fill=_rgba(SCRAP_DARK),
        )
        draw.polygon(
            _points(
                (
                    (center_x - 6, center_y),
                    (center_x - 3, center_y - 2),
                    (center_x - 3, center_y + 2),
                )
            ),
            fill=_rgba(SCRAP_LIGHT),
        )
    else:
        draw.rounded_rectangle(
            _box((center_x - 2, center_y - 5, center_x + 2, center_y + 5)),
            radius=_s(1),
            fill=_rgba(SCRAP_DARK),
        )
        draw.polygon(
            _points(
                (
                    (center_x, center_y - 6),
                    (center_x - 2, center_y - 3),
                    (center_x + 2, center_y - 3),
                )
            ),
            fill=_rgba(SCRAP_LIGHT),
        )


def _carriage_shell_cycle(
    draw: ImageDraw.ImageDraw, *, stage: int, recoil: int
) -> None:
    for y in (48, 55):
        _shell(draw, center_x=48, center_y=y, horizontal=True)
    if stage == 0:
        _shell(draw, center_x=48, center_y=41, horizontal=True)
    elif stage == 1:
        draw.line(_points(((47, 42), (40, 38))), fill=_rgba(IRON_LIGHT), width=_s(3))
        _shell(draw, center_x=41, center_y=38, horizontal=True)
    elif stage == 2:
        draw.line(_points(((41, 39), (34, 38))), fill=_rgba(IRON_LIGHT), width=_s(3))
        _shell(draw, center_x=34, center_y=38)
    elif stage >= 3 and recoil == 0:
        _shell(draw, center_x=32, center_y=31)


def _recoil_spade_sprite(
    *,
    tread_phase: int = 0,
    load_stage: int = 0,
    recoil: int = 0,
    spades: bool = False,
    report: bool = False,
) -> Image.Image:
    image, draw = _canvas()
    _tracks(draw, left=(7, 23, 19, 52), right=(45, 23, 57, 52), phase=tread_phase)
    draw.polygon(
        _points(((16, 28), (23, 19), (41, 19), (48, 28), (43, 48), (21, 48))),
        fill=_rgba(IRON_DARK),
    )
    draw.polygon(
        _points(((21, 29), (26, 23), (38, 23), (43, 29), (39, 44), (25, 44))),
        fill=_rgba(FERROUS["dark"]),
    )
    for points in (
        ((24, 39), (31, 42), (24, 59), (15, 60), (20, 45)),
        ((40, 39), (33, 42), (40, 59), (49, 60), (44, 45)),
    ):
        draw.polygon(_points(points), fill=_rgba(IRON_DARK))
        draw.line(_points((points[0], points[2])), fill=_rgba(IRON_LIGHT), width=_s(3))
    draw.rectangle(_box((21, 36, 43, 42)), fill=(9, 9, 12, 255))
    y = recoil
    draw.rectangle(_box((24, 26 + y, 40, 40 + y)), fill=_rgba(IRON_DARK))
    draw.rectangle(_box((28, 29 + y, 36, 38 + y)), fill=(7, 7, 10, 255))
    draw.rounded_rectangle(
        _box((29, 3 + y, 35, 33 + y)), radius=_s(2), fill=_rgba(IRON_DARK)
    )
    draw.rectangle(_box((31, 4 + y, 33, 30 + y)), fill=_rgba(BONE))
    draw.rectangle(_box((25, 4 + y, 39, 8 + y)), fill=_rgba(IRON_DARK))
    draw.rectangle(_box((27, 5 + y, 37, 6 + y)), fill=_rgba(IRON_LIGHT))
    _carriage_shell_cycle(draw, stage=load_stage, recoil=recoil)
    if load_stage >= 3 and recoil == 0:
        draw.rectangle(_box((23, 34, 41, 39)), fill=_rgba(SCRAP_DARK))
    if spades:
        draw.polygon(
            _points(((15, 54), (5, 57), (4, 60), (18, 59))), fill=_rgba(IRON_LIGHT)
        )
        draw.polygon(
            _points(((49, 54), (59, 57), (60, 60), (46, 59))), fill=_rgba(IRON_LIGHT)
        )
        draw.rectangle(_box((7, 57, 16, 59)), fill=_rgba(IRON_DARK))
        draw.rectangle(_box((48, 57, 57, 59)), fill=_rgba(IRON_DARK))
    else:
        draw.rectangle(_box((16, 50, 20, 59)), fill=_rgba(IRON_LIGHT))
        draw.rectangle(_box((44, 50, 48, 59)), fill=_rgba(IRON_LIGHT))
    if report:
        _muzzle_report(draw, center=32, y=1 + recoil)
    return _finish(image)


def _recoil_spade_sequence() -> GroundUnitSequence:
    return GroundUnitSequence(
        stem="bombard_recoil_spade",
        title="Bombard / Recoil-Spade Carriage",
        mechanism="long open-breech gun carriage, side shell rack, split trails, and oversized recoil feet",
        mechanism_box=(1, 0, 63, 63),
        attack_contract="one side-rack loading cycle, one heavy launch report, and one damage event",
        frames=(
            GroundUnitFrame(_recoil_spade_sprite(), 420, "idle", "idle"),
            GroundUnitFrame(
                _recoil_spade_sprite(tread_phase=1), 170, "locomotion", "travel_1"
            ),
            GroundUnitFrame(
                _recoil_spade_sprite(tread_phase=2), 170, "locomotion", "travel_2"
            ),
            GroundUnitFrame(_recoil_spade_sprite(), 250, "settle", "travel_settle"),
            GroundUnitFrame(
                _recoil_spade_sprite(load_stage=1),
                230,
                "anticipation",
                "rack_shell_selected",
            ),
            GroundUnitFrame(
                _recoil_spade_sprite(load_stage=2),
                230,
                "anticipation",
                "shell_on_loading_tray",
            ),
            GroundUnitFrame(
                _recoil_spade_sprite(load_stage=3, spades=True),
                260,
                "anticipation",
                "shell_ram+breech_lock+spades_plant",
            ),
            GroundUnitFrame(
                _recoil_spade_sprite(load_stage=4, recoil=9, spades=True, report=True),
                160,
                "attack",
                "damage+artillery_launch",
                logical_damage=True,
                report_count=1,
                recoil_px=9,
            ),
            GroundUnitFrame(
                _recoil_spade_sprite(load_stage=4, recoil=4, spades=True),
                240,
                "recoil",
                "gun_return",
                recoil_px=4,
            ),
            GroundUnitFrame(_recoil_spade_sprite(), 540, "settle", "attack_settle"),
        ),
    )


def bombard_sequence() -> GroundUnitSequence:
    return _recoil_spade_sequence()
