"""Native production ancestry for the finalized Darter."""

from __future__ import annotations

from collections.abc import Callable

from PIL import Image, ImageDraw

from tools.gen_sprites import (
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
CUPRIC = FACTIONS["cupric"]
Palette = dict[str, tuple[int, int, int]]
Drawer = Callable[[int, str], Image.Image]


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


def _plate(
    draw: ImageDraw.ImageDraw,
    box: tuple[int, int, int, int],
    *,
    fill: tuple[int, int, int] = IRON,
    edge: tuple[int, int, int] = IRON_DARK,
    radius: int = 3,
) -> None:
    draw.rounded_rectangle(_box(box), radius=_s(radius), fill=_rgba(edge))
    x0, y0, x1, y1 = box
    draw.rounded_rectangle(
        _box((x0 + 2, y0 + 2, x1 - 2, y1 - 2)),
        radius=_s(max(1, radius - 1)),
        fill=_rgba(fill),
    )


def _rail(
    draw: ImageDraw.ImageDraw,
    points: tuple[tuple[int, int], ...],
    *,
    accent: tuple[int, int, int] | None = None,
    width: int = 4,
) -> None:
    scaled = _points(points)
    draw.line(scaled, fill=_rgba(IRON_DARK), width=_s(width + 2), joint="curve")
    draw.line(scaled, fill=_rgba(IRON_LIGHT), width=_s(width), joint="curve")
    if accent is not None:
        draw.line(scaled, fill=_rgba(accent), width=_s(max(1, width - 2)))


def _engine_pod(
    draw: ImageDraw.ImageDraw,
    box: tuple[int, int, int, int],
    palette: Palette,
    phase: int,
) -> None:
    _plate(draw, box, radius=3)
    x0, _y0, x1, y1 = box
    vent_top = y1 - 12
    draw.rectangle(_box((x0 + 3, vent_top, x1 - 3, y1 - 3)), fill=_rgba(IRON_DARK))
    louver_inset = 3 if x1 - x0 >= 9 else 2
    for index in range(3):
        y = vent_top + 2 + index * 3
        color = palette["light"] if index == phase % 3 else palette["dark"]
        draw.rectangle(
            _box((x0 + louver_inset, y, x1 - louver_inset, y + 1)), fill=_rgba(color)
        )


def _state_offset(state: str, *, attack: int) -> int:
    return {"idle": 0, "ready": -2, "attack": attack, "recover": -3}[state]


def _state_color(state: str, palette: Palette) -> tuple[int, int, int]:
    return {
        "idle": palette["dark"],
        "ready": SCRAP_DARK,
        "attack": SCRAP_LIGHT,
        "recover": palette["light"],
    }[state]


def _darter_shear_wing(hover: int, state: str) -> Image.Image:
    image, draw = _canvas()
    spread = {"idle": 0, "ready": -2, "attack": -5, "recover": -2}[state]
    flutter = (0, -2, 2)[hover % 3]
    _rail(draw, ((32, 4), (32, 59)), accent=CUPRIC["base"], width=5)
    _rail(
        draw,
        ((30, 26), (14 - spread, 43 + flutter), (20 - spread, 50 + flutter)),
        accent=CUPRIC["dark"],
        width=4,
    )
    _rail(
        draw,
        ((34, 26), (50 + spread, 43 - flutter), (44 + spread, 50 - flutter)),
        accent=CUPRIC["dark"],
        width=4,
    )
    _engine_pod(
        draw, (18 - spread, 39 + flutter, 25 - spread, 53 + flutter), CUPRIC, hover
    )
    _engine_pod(
        draw, (39 + spread, 39 - flutter, 46 + spread, 53 - flutter), CUPRIC, hover + 1
    )
    offset = _state_offset(state, attack=-7)
    _plate(draw, (28, 24 + offset, 36, 36 + offset), fill=_state_color(state, CUPRIC))
    return _finish(image)


def _sequence(
    *,
    stem: str,
    title: str,
    mechanism: str,
    mechanism_box: tuple[int, int, int, int],
    attack_event: str,
    anticipation_event: str,
    recovery_event: str,
    recoil_px: int,
    drawer: Drawer,
) -> GroundUnitSequence:
    return GroundUnitSequence(
        stem=stem,
        title=title,
        mechanism=mechanism,
        mechanism_box=mechanism_box,
        attack_contract=f"one physical attack and one logical damage event: {attack_event}",
        frames=(
            GroundUnitFrame(drawer(0, "idle"), 420, "idle", "idle"),
            GroundUnitFrame(drawer(1, "idle"), 150, "locomotion", "hover_rise"),
            GroundUnitFrame(drawer(2, "idle"), 150, "locomotion", "hover_fall"),
            GroundUnitFrame(drawer(0, "idle"), 220, "settle", "hover_settle"),
            GroundUnitFrame(
                drawer(0, "ready"), 170, "anticipation", anticipation_event
            ),
            GroundUnitFrame(
                drawer(0, "attack"),
                100,
                "attack",
                attack_event,
                logical_damage=True,
                report_count=1,
                recoil_px=recoil_px,
            ),
            GroundUnitFrame(
                drawer(0, "recover"),
                170,
                "recovery",
                recovery_event,
                recoil_px=max(1, recoil_px // 2),
            ),
            GroundUnitFrame(drawer(0, "idle"), 480, "settle", "attack_settle"),
        ),
    )


def darter_shear_wing_sequence() -> GroundUnitSequence:
    return _sequence(
        stem="darter_shear_wing",
        title="Darter / Open Works Shear-Wing Skiff",
        mechanism="two narrow shear arms closing around a central striker",
        mechanism_box=(10, 2, 54, 54),
        anticipation_event="shear_wings_close",
        attack_event="damage+shear_strike",
        recovery_event="shear_wings_reopen",
        recoil_px=3,
        drawer=_darter_shear_wing,
    )
