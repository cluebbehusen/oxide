"""Native production renderers shared by finalized ground machines."""

from __future__ import annotations

from dataclasses import dataclass

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

SIZE = 64
SS = 4
FERROUS = FACTIONS["ferrous"]


@dataclass(frozen=True)
class GroundUnitFrame:
    image: Image.Image
    duration_ms: int
    phase: str
    event: str
    logical_damage: bool = False
    report_count: int = 0
    recoil_px: int = 0


@dataclass(frozen=True)
class GroundUnitSequence:
    stem: str
    title: str
    mechanism: str
    mechanism_box: tuple[int, int, int, int]
    attack_contract: str
    frames: tuple[GroundUnitFrame, ...]


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
    native = image.resize((SIZE, SIZE), Image.Resampling.LANCZOS)
    return rim_light(native)


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
            _box((x0 + 2, y0 + 2, x1 - 2, y1 - 2)), radius=_s(3), fill=(22, 22, 27, 255)
        )
        travel = max(8, y1 - y0 - 9)
        offset = phase * 4 % travel
        for index in range(6):
            y = y0 + 4 + (index * 8 + offset) % travel
            if y + 3 >= y1 - 1:
                continue
            color = IRON_LIGHT if (index + phase) % 2 == 0 else IRON
            draw.rounded_rectangle(
                _box((x0 + 1, y, x1 - 1, y + 3)), radius=_s(1), fill=_rgba(color)
            )
        pad_y = y0 + 8 + phase * 11 % max(10, y1 - y0 - 16)
        draw.rectangle(
            _box((x0 + 2, pad_y, x1 - 2, min(y1 - 3, pad_y + 5))),
            fill=_rgba(FERROUS["dark"]),
        )
        draw.line(
            ((_s(x0 + 2), _s(y0 + 4)), (_s(x0 + 2), _s(y1 - 5))),
            fill=_rgba(IRON_LIGHT),
            width=_s(1),
        )


def _sentinel_sprite(
    *, tread_phase: int = 0, recoil: int = 0, locked: bool = False, report: bool = False
) -> Image.Image:
    image, draw = _canvas()
    _tracks(draw, left=(7, 16, 20, 59), right=(44, 16, 57, 59), phase=tread_phase)
    draw.polygon(
        _points(((17, 20), (24, 14), (40, 14), (47, 20), (45, 56), (19, 56))),
        fill=_rgba(IRON_DARK),
    )
    draw.polygon(
        _points(((21, 22), (26, 18), (38, 18), (43, 22), (41, 51), (23, 51))),
        fill=_rgba(IRON),
    )
    draw.rectangle(_box((22, 42, 42, 55)), fill=_rgba(FERROUS["dark"]))
    draw.rectangle(_box((26, 46, 38, 51)), fill=(18, 17, 19, 255))
    draw.rectangle(_box((19, 30, 26, 38)), fill=_rgba(FERROUS["base"]))
    draw.rectangle(_box((38, 30, 45, 38)), fill=_rgba(FERROUS["base"]))
    draw.rectangle(_box((21, 25, 27, 29)), fill=_rgba(IRON_LIGHT))
    draw.rectangle(_box((37, 25, 43, 29)), fill=_rgba(IRON_LIGHT))
    y = recoil
    draw.rectangle(_box((26, 17 + y, 38, 38 + y)), fill=_rgba(IRON_DARK))
    draw.rectangle(_box((29, 18 + y, 35, 36 + y)), fill=_rgba(IRON_LIGHT))
    draw.rectangle(_box((30, 4 + y, 34, 24 + y)), fill=_rgba(IRON_DARK))
    draw.rectangle(_box((31, 4 + y, 33, 23 + y)), fill=_rgba(BONE))
    draw.rectangle(_box((27, 31 + y, 37, 39 + y)), fill=_rgba(FERROUS["dark"]))
    draw.rectangle(_box((29, 33 + y, 35, 37 + y)), fill=(12, 12, 15, 255))
    if locked:
        draw.rectangle(_box((25, 27, 39, 30)), fill=_rgba(SCRAP_DARK))
        draw.rectangle(_box((30, 27, 34, 29)), fill=_rgba(SCRAP_LIGHT))
    if report:
        draw.polygon(
            _points(((28, 4 + y), (32, 1 + y), (36, 4 + y), (33, 7 + y), (31, 7 + y))),
            fill=_rgba(SCRAP_LIGHT),
        )
        draw.rectangle(_box((31, 3 + y, 33, 6 + y)), fill=_rgba(BONE))
    return _finish(image)


def _lancer_sprite(
    *,
    tread_phase: int = 0,
    recoil: int = 0,
    charge_level: int = 0,
    report: bool = False,
) -> Image.Image:
    image, draw = _canvas()
    _tracks(draw, left=(7, 15, 19, 59), right=(45, 15, 57, 59), phase=tread_phase)
    draw.polygon(
        _points(((17, 20), (23, 14), (41, 14), (47, 20), (45, 56), (19, 56))),
        fill=_rgba(IRON_DARK),
    )
    draw.rectangle(_box((20, 22, 44, 54)), fill=_rgba(IRON))
    draw.rectangle(_box((21, 42, 43, 55)), fill=_rgba(FERROUS["dark"]))
    draw.rectangle(_box((25, 45, 39, 51)), fill=(13, 13, 16, 255))
    y = recoil
    for x0, x1 in ((25, 29), (35, 39)):
        draw.rectangle(_box((x0, 6 + y, x1, 38 + y)), fill=_rgba(IRON_DARK))
        draw.rectangle(_box((x0 + 1, 7 + y, x1 - 1, 35 + y)), fill=_rgba(IRON_LIGHT))
    draw.rectangle(_box((28, 8 + y, 36, 11 + y)), fill=_rgba(FERROUS["dark"]))
    draw.rectangle(_box((28, 31 + y, 36, 34 + y)), fill=_rgba(FERROUS["dark"]))
    draw.rectangle(_box((24, 34 + y, 40, 43 + y)), fill=_rgba(IRON_DARK))
    draw.rectangle(_box((27, 36 + y, 37, 41 + y)), fill=(9, 9, 12, 255))
    draw.rectangle(_box((30, 4 + y, 34, 9 + y)), fill=_rgba(BONE))
    if report:
        draw.polygon(
            _points(((29, 5 + y), (32, 1 + y), (35, 5 + y), (33, 8 + y), (31, 8 + y))),
            fill=_rgba(SCRAP_LIGHT),
        )
    for index, x in enumerate((23, 31, 39)):
        draw.rounded_rectangle(
            _box((x, 50, x + 6, 57)), radius=_s(2), fill=_rgba(IRON_DARK)
        )
        fill = SCRAP_LIGHT if index < charge_level else (24, 23, 25)
        draw.rectangle(_box((x + 2, 52, x + 4, 55)), fill=_rgba(fill))
    return _finish(image)


def _flakhound_sprite(*, tread_phase: int = 0, state: str = "idle") -> Image.Image:
    image, draw = _canvas()
    _tracks(draw, left=(6, 15, 18, 59), right=(46, 15, 58, 59), phase=tread_phase)
    draw.polygon(
        _points(((15, 20), (22, 14), (42, 14), (49, 20), (47, 56), (17, 56))),
        fill=_rgba(IRON_DARK),
    )
    draw.rectangle(_box((19, 23, 45, 54)), fill=_rgba(IRON))
    draw.ellipse(_box((17, 18, 47, 49)), fill=_rgba(IRON_DARK))
    draw.ellipse(_box((22, 23, 42, 43)), fill=_rgba(FERROUS["dark"]))
    left_recoil = 3 if state == "report_left" else 1 if state == "recover" else 0
    right_recoil = 3 if state == "report_right" else 1 if state == "recover" else 0
    for center, recoil in ((24, left_recoil), (40, right_recoil)):
        draw.arc(
            _box((center - 9, 18, center + 9, 42)),
            start=180,
            end=360,
            fill=_rgba(FERROUS["base"]),
            width=_s(5),
        )
        draw.rectangle(
            _box((center - 9, 28, center - 5, 43)), fill=_rgba(FERROUS["dark"])
        )
        draw.rectangle(
            _box((center + 5, 28, center + 9, 43)), fill=_rgba(FERROUS["dark"])
        )
        for barrel_x in (center - 4, center + 4):
            draw.rounded_rectangle(
                _box((barrel_x - 2, 5 + recoil, barrel_x + 2, 32 + recoil)),
                radius=_s(1),
                fill=_rgba(IRON_DARK),
            )
            draw.rectangle(
                _box((barrel_x - 1, 7 + recoil, barrel_x + 1, 29 + recoil)),
                fill=_rgba(IRON_LIGHT),
            )
            draw.ellipse(
                _box((barrel_x - 2, 3 + recoil, barrel_x + 2, 8 + recoil)),
                fill=(7, 7, 10, 255),
            )
    draw.rectangle(_box((30, 28, 34, 50)), fill=_rgba(FERROUS["base"]))
    draw.rectangle(_box((27, 43, 37, 49)), fill=_rgba(IRON_DARK))
    filled = (
        4 if state in {"ready", "report_left"} else 2 if state == "report_right" else 0
    )
    draw.rectangle(_box((20, 50, 44, 57)), fill=_rgba(IRON_DARK))
    for index, x in enumerate((22, 28, 34, 40)):
        color = SCRAP_LIGHT if index < filled else SCRAP_DARK
        draw.rectangle(_box((x, 52, x + 3, 55)), fill=_rgba(color))
    if state == "report_left":
        draw.rectangle(
            _box((17, 6 + left_recoil, 31, 8 + left_recoil)), fill=_rgba(SCRAP_LIGHT)
        )
    elif state == "report_right":
        draw.rectangle(
            _box((33, 6 + right_recoil, 47, 8 + right_recoil)), fill=_rgba(SCRAP_LIGHT)
        )
    return _finish(image)


def _sentinel_sequence() -> GroundUnitSequence:
    return GroundUnitSequence(
        stem="sentinel",
        title="Sentinel / Open Works Line Tractor",
        mechanism="tracked line tractor with an exposed breech-stamp gun carriage",
        mechanism_box=(23, 0, 41, 46),
        attack_contract="one barrel report and one damage event on the 4px recoil frame",
        frames=(
            GroundUnitFrame(_sentinel_sprite(), 420, "idle", "idle"),
            GroundUnitFrame(
                _sentinel_sprite(tread_phase=1), 150, "locomotion", "travel_1"
            ),
            GroundUnitFrame(
                _sentinel_sprite(tread_phase=2), 150, "locomotion", "travel_2"
            ),
            GroundUnitFrame(_sentinel_sprite(), 220, "settle", "travel_settle"),
            GroundUnitFrame(
                _sentinel_sprite(locked=True), 140, "anticipation", "breech_lock"
            ),
            GroundUnitFrame(
                _sentinel_sprite(recoil=4, report=True),
                100,
                "attack",
                "damage+barrel_report",
                logical_damage=True,
                report_count=1,
                recoil_px=4,
            ),
            GroundUnitFrame(
                _sentinel_sprite(recoil=2), 150, "recoil", "breech_return", recoil_px=2
            ),
            GroundUnitFrame(_sentinel_sprite(), 460, "settle", "attack_settle"),
        ),
    )


def _lancer_sequence() -> GroundUnitSequence:
    return GroundUnitSequence(
        stem="lancer",
        title="Lancer / Open Works Rail Sled",
        mechanism="open twin rail, rear breech, and three physical capacitor cans",
        mechanism_box=(20, 0, 44, 59),
        attack_contract="one three-cell rail charge, one report, and one damage event",
        frames=(
            GroundUnitFrame(_lancer_sprite(), 420, "idle", "idle"),
            GroundUnitFrame(
                _lancer_sprite(tread_phase=1), 150, "locomotion", "travel_1"
            ),
            GroundUnitFrame(
                _lancer_sprite(tread_phase=2), 150, "locomotion", "travel_2"
            ),
            GroundUnitFrame(_lancer_sprite(), 240, "settle", "travel_settle"),
            GroundUnitFrame(
                _lancer_sprite(charge_level=1), 140, "anticipation", "charge_cell_1"
            ),
            GroundUnitFrame(
                _lancer_sprite(charge_level=2), 140, "anticipation", "charge_cell_2"
            ),
            GroundUnitFrame(
                _lancer_sprite(charge_level=3), 170, "anticipation", "charge_cell_3"
            ),
            GroundUnitFrame(
                _lancer_sprite(recoil=5, report=True),
                100,
                "attack",
                "damage+rail_report",
                logical_damage=True,
                report_count=1,
                recoil_px=5,
            ),
            GroundUnitFrame(
                _lancer_sprite(recoil=2), 180, "recoil", "rail_return", recoil_px=2
            ),
            GroundUnitFrame(_lancer_sprite(), 480, "settle", "attack_settle"),
        ),
    )


def _flakhound_sequence() -> GroundUnitSequence:
    return GroundUnitSequence(
        stem="flakhound",
        title="Flakhound / Open Works Paired-Yoke Carrier",
        mechanism="two exposed paired-barrel yokes with a physical charge bar",
        mechanism_box=(14, 2, 50, 58),
        attack_contract="left yoke reports, then right yoke reports with the cycle's one damage event",
        frames=(
            GroundUnitFrame(_flakhound_sprite(), 420, "idle", "idle"),
            GroundUnitFrame(
                _flakhound_sprite(tread_phase=1), 150, "locomotion", "travel_1"
            ),
            GroundUnitFrame(
                _flakhound_sprite(tread_phase=2), 150, "locomotion", "travel_2"
            ),
            GroundUnitFrame(_flakhound_sprite(), 240, "settle", "travel_settle"),
            GroundUnitFrame(
                _flakhound_sprite(state="ready"),
                180,
                "anticipation",
                "charge_bar_ready",
            ),
            GroundUnitFrame(
                _flakhound_sprite(state="report_left"),
                100,
                "attack",
                "report_left_yoke",
                report_count=1,
                recoil_px=3,
            ),
            GroundUnitFrame(
                _flakhound_sprite(state="report_right"),
                110,
                "attack",
                "damage+report_right_yoke",
                logical_damage=True,
                report_count=1,
                recoil_px=3,
            ),
            GroundUnitFrame(
                _flakhound_sprite(state="recover"),
                180,
                "recovery",
                "paired_yokes_recover",
            ),
            GroundUnitFrame(_flakhound_sprite(), 500, "settle", "attack_settle"),
        ),
    )


def sentinel_sequence() -> GroundUnitSequence:
    return _sentinel_sequence()


def lancer_sequence() -> GroundUnitSequence:
    return _lancer_sequence()


def flakhound_sequence() -> GroundUnitSequence:
    return _flakhound_sequence()
