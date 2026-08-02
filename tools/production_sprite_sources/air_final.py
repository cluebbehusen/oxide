"""Native production renderers for finalized aircraft."""

from __future__ import annotations

from collections.abc import Callable

from PIL import Image, ImageChops, ImageDraw

from tools.gen_sprites import (
    FACTIONS,
    IRON,
    IRON_DARK,
    IRON_LIGHT,
    SCRAP_DARK,
    SCRAP_LIGHT,
    rim_light,
)
from tools.production_sprite_sources import air_base as air_shapes
from tools.production_sprite_sources.ground_base import (
    GroundUnitFrame,
    GroundUnitSequence,
)

SIZE = 64
SS = 4
FERROUS = FACTIONS["ferrous"]
CUPRIC = FACTIONS["cupric"]
Palette = dict[str, tuple[int, int, int]]
Drawer = Callable[[int, str], Image.Image]
Box = tuple[int, int, int, int]
_LOCOMOTION_WINDOWS: dict[str, tuple[Box, ...]] = {
    "darter_shear_wing": ((19, 41, 24, 51), (40, 41, 45, 51))
}


def _rgba(color: tuple[int, int, int]) -> tuple[int, int, int, int]:
    return (*color, 255)


def _s(value: int) -> int:
    return value * SS


def _box(box: Box) -> Box:
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
    box: Box,
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
        draw.line(
            scaled, fill=_rgba(accent), width=_s(max(1, width - 2)), joint="curve"
        )


def _engine_pod(
    draw: ImageDraw.ImageDraw, box: Box, palette: Palette, phase: int
) -> None:
    _plate(draw, box, radius=3)
    x0, y0, x1, y1 = box
    vent_top = max(y0 + 4, y1 - 11)
    draw.rectangle(_box((x0 + 3, vent_top, x1 - 3, y1 - 3)), fill=_rgba(IRON_DARK))
    for index in range(3):
        y = vent_top + 2 + index * 3
        color = palette["light"] if index == phase % 3 else palette["dark"]
        draw.rectangle(_box((x0 + 3, y, x1 - 3, y + 1)), fill=_rgba(color))


def _internal_locomotion(idle: Image.Image, stem: str, phase: int) -> Image.Image:
    palette = FERROUS if stem.startswith(("buzzard_", "talon_")) else CUPRIC
    overlay = Image.new("RGBA", idle.size, (0, 0, 0, 0))
    draw = ImageDraw.Draw(overlay)
    for index, (x0, y0, x1, y1) in enumerate(_LOCOMOTION_WINDOWS[stem]):
        width = x1 - x0
        height = y1 - y0
        if height >= width:
            span = max(2, height - 3)
            travel = phase if span <= 3 else phase * 3
            y = y0 + 1 + (travel + index * 3) % span
            draw.rectangle(
                (x0 + 1, y, x1 - 1, min(y + 2, y1 - 1)), fill=_rgba(palette["light"])
            )
        else:
            span = max(2, width - 3)
            travel = phase if span <= 3 else phase * 3
            x = x0 + 1 + (travel + index * 3) % span
            draw.rectangle(
                (x, y0 + 1, min(x + 2, x1 - 1), y1 - 1), fill=_rgba(palette["light"])
            )
    original_alpha = idle.getchannel("A")
    overlay_alpha = ImageChops.multiply(overlay.getchannel("A"), original_alpha)
    overlay.putalpha(overlay_alpha)
    result = idle.copy()
    result.alpha_composite(overlay)
    result.putalpha(original_alpha)
    return result


def _carry(sequence: GroundUnitSequence) -> GroundUnitSequence:
    idle = sequence.frames[0]
    first = GroundUnitFrame(
        _internal_locomotion(idle.image, sequence.stem, 1),
        sequence.frames[1].duration_ms,
        "locomotion",
        "internal_propulsion_a",
    )
    second = GroundUnitFrame(
        _internal_locomotion(idle.image, sequence.stem, 2),
        sequence.frames[2].duration_ms,
        "locomotion",
        "internal_propulsion_b",
    )
    return GroundUnitSequence(
        stem=sequence.stem,
        title=sequence.title,
        mechanism=sequence.mechanism,
        mechanism_box=sequence.mechanism_box,
        attack_contract=sequence.attack_contract,
        frames=(idle, first, second, *sequence.frames[3:]),
    )


def _state_color(state: str, palette: Palette) -> tuple[int, int, int]:
    return {
        "idle": palette["dark"],
        "ready": SCRAP_DARK,
        "attack": SCRAP_LIGHT,
        "recover": palette["light"],
    }[state]


def _buzzard_compact_bomber(phase: int, state: str) -> Image.Image:
    image, draw = _canvas()
    draw.polygon(
        _points(
            (
                (19, 13),
                (27, 7),
                (37, 7),
                (45, 13),
                (53, 28),
                (49, 52),
                (15, 52),
                (11, 28),
            )
        ),
        fill=_rgba(IRON_DARK),
    )
    _engine_pod(draw, (7, 20, 19, 51), FERROUS, phase)
    _engine_pod(draw, (45, 20, 57, 51), FERROUS, phase + 1)
    _plate(draw, (18, 8, 46, 54), fill=FERROUS["base"], radius=6)
    draw.polygon(
        _points(((22, 12), (32, 5), (42, 12), (39, 22), (25, 22))), fill=_rgba(IRON)
    )
    draw.rectangle(_box((22, 24, 42, 48)), fill=_rgba(IRON_DARK))
    gap = {"idle": 1, "ready": 3, "attack": 6, "recover": 3}[state]
    draw.rounded_rectangle(
        _box((24, 26, 32 - gap, 46)), radius=_s(2), fill=_rgba(FERROUS["dark"])
    )
    draw.rounded_rectangle(
        _box((32 + gap, 26, 40, 46)), radius=_s(2), fill=_rgba(FERROUS["dark"])
    )
    payload_y = {"idle": 37, "ready": 33, "attack": 27, "recover": 32}[state]
    _plate(
        draw,
        (29, payload_y, 35, payload_y + 8),
        fill=_state_color(state, FERROUS),
        radius=2,
    )
    return _finish(image)


def _talon_compact_interceptor(phase: int, state: str) -> Image.Image:
    image, draw = _canvas()
    draw.polygon(
        _points(
            (
                (24, 17),
                (9, 24),
                (5, 42),
                (20, 47),
                (25, 40),
                (28, 54),
                (36, 54),
                (39, 40),
                (44, 47),
                (59, 42),
                (55, 24),
                (40, 17),
            )
        ),
        fill=_rgba(IRON_DARK),
    )
    draw.polygon(
        _points(((8, 28), (24, 20), (29, 32), (20, 42), (10, 39))),
        fill=_rgba(FERROUS["dark"]),
    )
    draw.polygon(
        _points(((56, 28), (40, 20), (35, 32), (44, 42), (54, 39))),
        fill=_rgba(FERROUS["dark"]),
    )
    _engine_pod(draw, (7, 28, 18, 46), FERROUS, phase)
    _engine_pod(draw, (46, 28, 57, 46), FERROUS, phase + 1)
    _plate(draw, (24, 19, 40, 53), fill=FERROUS["base"], radius=4)
    pinch = {"idle": 0, "ready": 1, "attack": 3, "recover": 1}[state]
    _rail(draw, ((26 + pinch, 30), (23 + pinch, 8)), accent=FERROUS["dark"], width=5)
    _rail(draw, ((38 - pinch, 30), (41 - pinch, 8)), accent=FERROUS["dark"], width=5)
    offset = {"idle": 0, "ready": -2, "attack": -8, "recover": -3}[state]
    _rail(draw, ((32, 32 + offset), (32, 16 + offset)), width=3)
    draw.rectangle(_box((28, 30, 36, 36)), fill=_rgba(_state_color(state, FERROUS)))
    return _finish(image)


def _rotor(
    draw: ImageDraw.ImageDraw, center: tuple[int, int], palette: Palette, phase: int
) -> None:
    cx, cy = center
    draw.ellipse(_box((cx - 5, cy - 5, cx + 5, cy + 5)), fill=_rgba(IRON_LIGHT))
    draw.ellipse(_box((cx - 3, cy - 3, cx + 3, cy + 3)), fill=_rgba(IRON_DARK))
    vectors = ((3, 0), (2, 2), (0, 3))
    dx, dy = vectors[phase % len(vectors)]
    draw.line(
        _points(((cx - dx, cy - dy), (cx + dx, cy + dy))),
        fill=_rgba(palette["light"]),
        width=_s(2),
    )
    draw.ellipse(_box((cx - 1, cy - 1, cx + 1, cy + 1)), fill=_rgba(IRON))


def _wisp_quadcopter(phase: int, state: str) -> Image.Image:
    image, draw = _canvas()
    centers = ((20, 18), (44, 18), (20, 42), (44, 42))
    for center in centers:
        _rail(draw, ((32, 31), center), accent=CUPRIC["dark"], width=2)
    for index, center in enumerate(centers):
        _rotor(draw, center, CUPRIC, phase + index % 2)
    _plate(draw, (26, 24, 38, 42), fill=CUPRIC["base"], radius=3)
    draw.rectangle(_box((29, 28, 35, 37)), fill=_rgba(IRON_DARK))
    draw.rectangle(_box((31, 29, 33, 36)), fill=_rgba(CUPRIC["light"]))
    offset = {"idle": 0, "ready": -2, "attack": -6, "recover": -3}[state]
    _rail(draw, ((32, 31 + offset), (32, 20 + offset)), width=2)
    draw.rectangle(_box((29, 36, 35, 40)), fill=_rgba(_state_color(state, CUPRIC)))
    return _finish(image)


def _sequence(
    *,
    stem: str,
    title: str,
    mechanism: str,
    mechanism_box: Box,
    anticipation_event: str,
    attack_event: str,
    recovery_event: str,
    recoil_px: int,
    drawer: Drawer,
    movement_events: tuple[str, str] = (
        "internal_propulsion_a",
        "internal_propulsion_b",
    ),
) -> GroundUnitSequence:
    return GroundUnitSequence(
        stem=stem,
        title=title,
        mechanism=mechanism,
        mechanism_box=mechanism_box,
        attack_contract=f"one physical attack and one logical damage event: {attack_event}",
        frames=(
            GroundUnitFrame(drawer(0, "idle"), 420, "idle", "idle"),
            GroundUnitFrame(drawer(1, "idle"), 150, "locomotion", movement_events[0]),
            GroundUnitFrame(drawer(2, "idle"), 150, "locomotion", movement_events[1]),
            GroundUnitFrame(drawer(0, "idle"), 220, "settle", "motion_settle"),
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


def buzzard_sequence() -> GroundUnitSequence:
    return _sequence(
        stem="buzzard_compact_bomber",
        title="Buzzard / Compact Ore-Drop Bomber",
        mechanism="armored belly hopper opening around one heavy payload ram",
        mechanism_box=(18, 21, 46, 49),
        anticipation_event="belly_hopper_opens",
        attack_event="damage+payload_ram_drop",
        recovery_event="belly_hopper_closes",
        recoil_px=6,
        drawer=_buzzard_compact_bomber,
    )


def darter_sequence() -> GroundUnitSequence:
    return _carry(air_shapes.darter_shear_wing_sequence())


def talon_sequence() -> GroundUnitSequence:
    return _sequence(
        stem="talon_compact_interceptor",
        title="Talon / Compact Fork Interceptor",
        mechanism="paired pursuit forks sighting one central anti-air cannon",
        mechanism_box=(19, 4, 45, 39),
        anticipation_event="pursuit_forks_converge",
        attack_event="damage+interceptor_cannon_report",
        recovery_event="pursuit_forks_release",
        recoil_px=5,
        drawer=_talon_compact_interceptor,
    )


def wisp_sequence() -> GroundUnitSequence:
    return _sequence(
        stem="wisp_quadcopter",
        title="Wisp / Four-Rotor Relay",
        mechanism="four indexed rotor pods feeding one tiny central pursuit striker",
        mechanism_box=(14, 12, 50, 45),
        anticipation_event="relay_striker_arms",
        attack_event="damage+relay_striker_snap",
        recovery_event="relay_striker_returns",
        recoil_px=2,
        drawer=_wisp_quadcopter,
        movement_events=("rotor_phase_a", "rotor_phase_b"),
    )
