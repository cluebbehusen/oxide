"""Production-native frames for approved core machine art.

This module promotes the approved Harvester, Sentinel, Scuttler, and Foundry
from their reviewed native frames. Movement changes only the locomotion
mechanism, weapon frames retain one report or bite, the Harvester deploys its
pincers before advancing its bucket, and the Foundry crane and work lights run
only through its production row.
"""

from __future__ import annotations

import hashlib
from collections.abc import Iterator
from pathlib import Path

from PIL import Image, ImageDraw

from tools import gen_sprites as gen

Registry = dict[str, Image.Image]
Palette = dict[str, tuple[int, int, int]]

SS = 4
IRON_DEEP = (17, 18, 23)
VOID = (9, 9, 12)
AMBER = (151, 93, 38)
AMBER_LIGHT = (218, 151, 65)

HARVESTER_CARGO_LEVELS = 5
HARVESTER_POSES = ("", "_tread1", "_tread2", "_scoop1", "_scoop2")
SENTINEL_POSES = (
    "",
    "_move1",
    "_move2",
    "_action1",
    "_action2",
    "_action3",
    "_action4",
)
SCUTTLER_POSES = SENTINEL_POSES
FOUNDRY_POSES = ("", "_work1", "_work2", "_work3", "_work4")

# Semantic RGBA digest of the four approved review candidates.
APPROVED_SOURCE_RGBA_SHA256 = (
    "0e9561f9d14ba071e7e3cb899053185b2b475f30276a79f026768b4e6c69b7e5"
)


def _s(value: float) -> int:
    return round(value * SS)


def _box(values: tuple[float, float, float, float]) -> tuple[int, int, int, int]:
    return tuple(_s(value) for value in values)  # type: ignore[return-value]


def _points(
    values: tuple[tuple[float, float], ...],
) -> tuple[tuple[int, int], ...]:
    return tuple((_s(x), _s(y)) for x, y in values)


def _rgba(color: tuple[int, int, int], alpha: int = 255) -> tuple[int, int, int, int]:
    return (*color, alpha)


def _new_sprite(size: int) -> tuple[Image.Image, ImageDraw.ImageDraw]:
    image = Image.new("RGBA", (size * SS, size * SS), (0, 0, 0, 0))
    return image, ImageDraw.Draw(image)


def _finish(image: Image.Image, size: int) -> Image.Image:
    native = image.resize((size, size), Image.Resampling.LANCZOS)
    return gen.rim_light(native)


def _plate(
    draw: ImageDraw.ImageDraw,
    bounds: tuple[float, float, float, float],
    *,
    fill: tuple[int, int, int] = gen.IRON,
    edge: tuple[int, int, int] = gen.IRON_DARK,
    highlight: tuple[int, int, int] = gen.IRON_LIGHT,
    radius: float = 3,
) -> None:
    x0, y0, x1, y1 = bounds
    draw.rounded_rectangle(_box(bounds), radius=_s(radius), fill=_rgba(edge))
    draw.rounded_rectangle(
        _box((x0 + 2, y0 + 2, x1 - 2, y1 - 2)),
        radius=_s(max(1, radius - 1)),
        fill=_rgba(fill),
    )
    draw.line(
        _points(((x0 + 3, y0 + 3), (x1 - 3, y0 + 3))),
        fill=_rgba(highlight, 190),
        width=_s(1),
    )


def _bolt(draw: ImageDraw.ImageDraw, x: float, y: float, radius: float = 1.4) -> None:
    draw.ellipse(
        _box((x - radius, y - radius, x + radius, y + radius)),
        fill=_rgba(gen.IRON_DARK),
    )
    draw.rectangle(
        _box((x - 0.5, y - 0.5, x + 0.5, y + 0.5)),
        fill=_rgba(gen.BONE, 220),
    )


def _strut(
    draw: ImageDraw.ImageDraw,
    start: tuple[float, float],
    end: tuple[float, float],
    *,
    color: tuple[int, int, int] = gen.IRON_LIGHT,
    width: float = 2,
) -> None:
    draw.line(_points((start, end)), fill=_rgba(gen.IRON_DARK), width=_s(width + 2))
    draw.line(_points((start, end)), fill=_rgba(color), width=_s(width))


def _tracks(
    draw: ImageDraw.ImageDraw,
    palette: Palette,
    phase: int,
    *,
    bounds: tuple[tuple[int, int, int, int], tuple[int, int, int, int]] = (
        (7, 14, 20, 59),
        (44, 14, 57, 59),
    ),
) -> None:
    for side, (x0, y0, x1, y1) in enumerate(bounds):
        draw.rounded_rectangle(
            _box((x0, y0, x1, y1)), radius=_s(4), fill=_rgba(gen.IRON_DARK)
        )
        draw.rounded_rectangle(
            _box((x0 + 2, y0 + 2, x1 - 2, y1 - 2)),
            radius=_s(2),
            fill=_rgba(IRON_DEEP),
        )
        travel = y1 - y0 - 7
        for index in range(6):
            y = y0 + 3 + (index * 7 + phase * 3) % travel
            if y + 3 >= y1 - 1:
                continue
            tone = (
                palette["dark"] if (index + phase + side) % 4 == 0 else gen.IRON_LIGHT
            )
            draw.rounded_rectangle(
                _box((x0 + 1, y, x1 - 1, y + 3)),
                radius=_s(1),
                fill=_rgba(tone),
            )


def _cargo_meter(draw: ImageDraw.ImageDraw, cargo: int) -> None:
    draw.rounded_rectangle(
        _box((23, 48, 41, 57)), radius=_s(2), fill=_rgba(gen.IRON_DARK)
    )
    draw.rectangle(_box((25, 50, 39, 55)), fill=_rgba(VOID))
    for index in range(4):
        x0 = 26 + index * 3
        color = gen.SCRAP_LIGHT if index < cargo else (33, 29, 24)
        draw.rectangle(_box((x0, 51, x0 + 2, 54)), fill=_rgba(color))


def _harvester_body(
    draw: ImageDraw.ImageDraw,
    faction: str,
    *,
    cargo: int,
    tread: int,
) -> Palette:
    palette = gen.FACTIONS[faction]
    _tracks(draw, palette, tread)
    _plate(draw, (18, 15, 46, 58), fill=gen.IRON, radius=5)
    draw.polygon(
        _points(((22, 20), (42, 20), (39, 47), (25, 47))),
        fill=_rgba(palette["dark"]),
    )
    draw.rectangle(_box((27, 22, 37, 45)), fill=_rgba(VOID))
    for y in (25, 31, 37, 43):
        draw.rectangle(_box((28, y, 36, y + 2)), fill=_rgba(gen.IRON_LIGHT))
    for side in (-1, 1):
        hinge_x = 32 + side * 18
        _strut(
            draw,
            (32 + side * 11, 30),
            (hinge_x, 16),
            color=palette["dark"],
            width=2,
        )
        draw.ellipse(_box((hinge_x - 3, 25, hinge_x + 3, 31)), fill=_rgba(AMBER))
    _cargo_meter(draw, cargo)
    return palette


def _harvester_pincers(draw: ImageDraw.ImageDraw, palette: Palette, scoop: int) -> None:
    extension = (0, 6, 7)[scoop]
    spread = (0, 2, 1)[scoop]
    for side in (-1, 1):
        shoulder = (32 + side * 16, 10)
        elbow = (32 + side * (18 + spread), 16 + extension // 2)
        tip = (32 + side * 12, 19 + extension)
        _strut(draw, shoulder, elbow, color=gen.IRON_DARK, width=4)
        _strut(draw, elbow, tip, color=palette["dark"], width=3)
        draw.line(
            _points((tip, (tip[0] - side * 5, tip[1] + 2))),
            fill=_rgba(gen.IRON_LIGHT),
            width=_s(2),
        )
        _bolt(draw, elbow[0], elbow[1], 2)


def _harvester_bucket(draw: ImageDraw.ImageDraw, palette: Palette, scoop: int) -> None:
    drop = (0, 0, 7)[scoop]
    narrow = 1 if scoop == 2 else 0
    draw.rounded_rectangle(
        _box((23 + narrow, 6 + drop, 41 - narrow, 17 + drop)),
        radius=_s(3),
        fill=_rgba(gen.IRON_DARK),
    )
    draw.polygon(
        _points(
            (
                (26 + narrow, 8 + drop),
                (38 - narrow, 8 + drop),
                (36 - narrow, 14 + drop),
                (28 + narrow, 14 + drop),
            )
        ),
        fill=_rgba(VOID),
    )
    draw.rectangle(
        _box((28 + narrow, 14 + drop, 36 - narrow, 17 + drop)),
        fill=_rgba(palette["dark"]),
    )


def render_harvester(
    faction: str, *, cargo: int, tread: int = 0, scoop: int = 0
) -> Image.Image:
    """Render the approved Harvester and its independent state channels."""
    if faction not in gen.FACTIONS:
        raise ValueError(f"unknown faction: {faction}")
    if cargo not in range(HARVESTER_CARGO_LEVELS):
        raise ValueError(f"invalid Harvester cargo level: {cargo}")
    if tread not in range(3) or scoop not in range(3) or (tread and scoop):
        raise ValueError("Harvester tread and scoop states must be separate 0..2 rows")
    image, draw = _new_sprite(64)
    palette = _harvester_body(draw, faction, cargo=cargo, tread=tread)
    _harvester_pincers(draw, palette, scoop)
    _harvester_bucket(draw, palette, scoop)
    return _finish(image, 64)


def render_sentinel(faction: str, *, move: int = 0, action: int = 0) -> Image.Image:
    """Render the approved riveted-casemate Sentinel."""
    if faction not in gen.FACTIONS:
        raise ValueError(f"unknown faction: {faction}")
    if move not in range(3) or action not in range(5) or (move and action):
        raise ValueError("Sentinel movement and action states are separate")
    palette = gen.FACTIONS[faction]
    image, draw = _new_sprite(64)
    _tracks(draw, palette, move, bounds=((7, 20, 20, 59), (44, 20, 57, 59)))
    draw.polygon(
        _points(((14, 25), (21, 15), (43, 15), (50, 25), (47, 55), (17, 55))),
        fill=_rgba(gen.IRON_DARK),
    )
    draw.polygon(
        _points(((19, 26), (24, 20), (40, 20), (45, 26), (42, 48), (22, 48))),
        fill=_rgba(gen.IRON),
    )
    draw.rectangle(_box((17, 29, 47, 36)), fill=_rgba(palette["dark"]))
    draw.rectangle(_box((22, 40, 42, 53)), fill=_rgba(VOID))
    for x, y in ((18, 27), (46, 27), (20, 50), (44, 50)):
        _bolt(draw, x, y)

    recoil = (0, 0, 4, 2, 0)[action]
    draw.rounded_rectangle(
        _box((25, 19 + recoil, 39, 41 + recoil)),
        radius=_s(3),
        fill=_rgba(gen.IRON_DARK),
    )
    draw.rectangle(_box((28, 21 + recoil, 36, 37 + recoil)), fill=_rgba(gen.IRON_LIGHT))
    draw.rectangle(_box((29, 4 + recoil, 35, 27 + recoil)), fill=_rgba(gen.IRON_DARK))
    draw.rectangle(_box((31, 4 + recoil, 33, 25 + recoil)), fill=_rgba(gen.BONE))
    draw.rectangle(
        _box((27, 34 + recoil, 37, 42 + recoil)), fill=_rgba(palette["dark"])
    )
    if action == 1:
        draw.rectangle(_box((25, 29, 39, 32)), fill=_rgba(AMBER))
        draw.rectangle(_box((30, 29, 34, 31)), fill=_rgba(AMBER_LIGHT))
    _plate(draw, (41, 38, 47, 52), fill=IRON_DEEP, radius=1)
    for y in (40, 44, 48):
        draw.rectangle(_box((43, y, 45, y + 2)), fill=_rgba(AMBER))
    draw.ellipse(_box((39, 23, 43, 27)), fill=_rgba(palette["light"]))
    if action == 2:
        draw.polygon(
            _points(
                (
                    (28, 5 + recoil),
                    (32, 1 + recoil),
                    (36, 5 + recoil),
                    (33, 9 + recoil),
                    (31, 9 + recoil),
                )
            ),
            fill=_rgba(gen.SCRAP_LIGHT),
        )
        draw.rectangle(_box((31, 4 + recoil, 33, 7 + recoil)), fill=_rgba(gen.BONE))
    return _finish(image, 64)


def _scuttler_legs(draw: ImageDraw.ImageDraw, gait: int) -> None:
    offsets = ((0, 0, 0), (-3, 2, -2), (3, -2, 2))[gait]
    for side in (-1, 1):
        for index, y in enumerate((25, 37, 49)):
            anchor_x = 25 if side < 0 else 39
            elbow_x = 16 if side < 0 else 48
            foot_x = 6 if side < 0 else 58
            shift = offsets[index] * side
            elbow = (elbow_x, y + shift)
            foot = (
                foot_x,
                y + shift + (-2 if index == 0 else 2 if index == 2 else 0),
            )
            _strut(draw, (anchor_x, y), elbow, color=gen.IRON, width=2)
            _strut(draw, elbow, foot, color=gen.IRON_LIGHT, width=1.5)
            draw.rounded_rectangle(
                _box((foot[0] - 3, foot[1] - 2, foot[0] + 3, foot[1] + 2)),
                radius=_s(1),
                fill=_rgba(gen.IRON_LIGHT),
            )
            _bolt(draw, elbow[0], elbow[1], 1.2)


def render_scuttler(faction: str, *, move: int = 0, action: int = 0) -> Image.Image:
    """Render the approved armored-centipede Scuttler."""
    if faction not in gen.FACTIONS:
        raise ValueError(f"unknown faction: {faction}")
    if move not in range(3) or action not in range(5) or (move and action):
        raise ValueError("Scuttler movement and action states are separate")
    palette = gen.FACTIONS[faction]
    image, draw = _new_sprite(64)
    _scuttler_legs(draw, move)
    _plate(draw, (22, 14, 42, 59), fill=gen.IRON, radius=6)
    draw.rectangle(_box((26, 19, 38, 54)), fill=_rgba(palette["dark"]))
    for y in (22, 31, 40, 49):
        draw.rectangle(_box((25, y, 39, y + 4)), fill=_rgba(gen.IRON_DARK))
        draw.line(
            _points(((28, y + 1), (36, y + 1))),
            fill=_rgba(gen.IRON_LIGHT),
            width=_s(1),
        )
    gap = (4, 8, 0, 6, 4)[action]
    jaw_y = 8 - (2 if action == 2 else 0)
    draw.line(
        _points(((27, 21), (19, jaw_y + 8))),
        fill=_rgba(gen.IRON_LIGHT),
        width=_s(4),
    )
    draw.line(
        _points(((37, 21), (45, jaw_y + 8))),
        fill=_rgba(gen.IRON_LIGHT),
        width=_s(4),
    )
    draw.polygon(
        _points(
            (
                (15, jaw_y + 2),
                (30 - gap, jaw_y + 3),
                (28, jaw_y + 13),
                (18, jaw_y + 10),
            )
        ),
        fill=_rgba(gen.IRON_DARK),
    )
    draw.polygon(
        _points(
            (
                (49, jaw_y + 2),
                (34 + gap, jaw_y + 3),
                (36, jaw_y + 13),
                (46, jaw_y + 10),
            )
        ),
        fill=_rgba(gen.IRON_DARK),
    )
    for x0, x1 in ((18, 27), (37, 46)):
        draw.line(
            _points(((x0, jaw_y + 5), (x1, jaw_y + 6))),
            fill=_rgba(gen.BONE),
            width=_s(2),
        )
    draw.ellipse(_box((29, 20, 35, 26)), fill=_rgba(palette["light"]))
    return _finish(image, 64)


def _octagon(
    draw: ImageDraw.ImageDraw, inset: int, color: tuple[int, int, int]
) -> None:
    draw.polygon(
        _points(
            (
                (inset + 11, inset),
                (128 - inset - 11, inset),
                (128 - inset, inset + 11),
                (128 - inset, 128 - inset - 11),
                (128 - inset - 11, 128 - inset),
                (inset + 11, 128 - inset),
                (inset, 128 - inset - 11),
                (inset, inset + 11),
            )
        ),
        fill=_rgba(color),
    )


def render_foundry(faction: str, *, work: int = 0) -> Image.Image:
    """Render the approved heavy-bridge Foundry."""
    if faction not in gen.FACTIONS:
        raise ValueError(f"unknown faction: {faction}")
    if work not in range(5):
        raise ValueError(f"invalid Foundry work frame: {work}")
    palette = gen.FACTIONS[faction]
    image, draw = _new_sprite(128)
    _octagon(draw, 4, gen.IRON_DARK)
    _octagon(draw, 10, IRON_DEEP)
    draw.rectangle(_box((17, 17, 111, 111)), fill=_rgba((13, 13, 17)))

    for x0, x1 in ((9, 29), (99, 119)):
        _plate(draw, (x0, 16, x1, 113), fill=gen.IRON, radius=4)
        for index, y in enumerate((25, 43, 61, 79)):
            draw.rectangle(_box((x0 + 4, y, x1 - 4, y + 8)), fill=_rgba(VOID))
            signal_on = work and index == (work - 1) % 4
            draw.line(
                _points(((x0 + 5, y + 2), (x1 - 5, y + 2))),
                fill=_rgba(palette["light"] if signal_on else gen.IRON_LIGHT),
                width=_s(1),
            )
    for side in (-1, 1):
        x = 64 + side * 34
        draw.polygon(
            _points(
                (
                    (x - 8, 79),
                    (x + 8, 79),
                    (x + side * 20, 68),
                    (x + side * 20, 77),
                )
            ),
            fill=_rgba(gen.IRON_DARK),
        )
        draw.line(
            _points(((x - 5, 80), (64 + side * 17, 72), (64 + side * 15, 66))),
            fill=_rgba(palette["dark"]),
            width=_s(4),
        )
        _plate(draw, (x - 8, 88, x + 8, 106), fill=IRON_DEEP, radius=2)

    carriage_x = (64, 44, 64, 84, 64)[work]
    draw.rectangle(_box((22, 19, 106, 34)), fill=_rgba(gen.IRON_DARK))
    draw.rectangle(_box((28, 22, 100, 27)), fill=_rgba(palette["dark"]))
    for x in (31, 97):
        _bolt(draw, x, 25, 2)
    _plate(
        draw,
        (carriage_x - 12, 16, carriage_x + 12, 41),
        fill=gen.IRON,
        radius=3,
    )
    draw.rectangle(
        _box((carriage_x - 5, 20, carriage_x + 5, 25)),
        fill=_rgba(palette["light"] if work else AMBER),
    )
    draw.line(
        _points(((carriage_x, 38), (carriage_x, 51))),
        fill=_rgba(gen.BONE),
        width=_s(2),
    )
    draw.polygon(
        _points(
            (
                (carriage_x - 4, 50),
                (carriage_x + 4, 50),
                (carriage_x + 2, 56),
                (carriage_x - 2, 56),
            )
        ),
        fill=_rgba(gen.IRON_LIGHT),
    )

    draw.ellipse(_box((39, 43, 89, 95)), fill=_rgba(gen.IRON_DARK))
    draw.ellipse(_box((45, 49, 83, 89)), fill=_rgba(gen.IRON))
    draw.ellipse(_box((50, 54, 78, 84)), fill=_rgba(VOID))
    radius = (6, 9, 13, 9, 6)[work]
    color = (AMBER, AMBER_LIGHT, gen.SCRAP_LIGHT, AMBER_LIGHT, AMBER)[work]
    draw.ellipse(
        _box((64 - radius, 69 - radius, 64 + radius, 69 + radius)),
        fill=_rgba(color),
    )
    for angle_box in (
        (42, 48, 48, 54),
        (80, 48, 86, 54),
        (42, 84, 48, 90),
        (80, 84, 86, 90),
    ):
        draw.rectangle(_box(angle_box), fill=_rgba(gen.IRON_LIGHT))
    for index, x in enumerate((35, 47, 81, 93)):
        tone = palette["light"] if work and index == work - 1 else gen.IRON_DARK
        draw.ellipse(_box((x - 2, 116, x + 2, 120)), fill=_rgba(tone))
    draw.rectangle(_box((43, 105, 85, 120)), fill=_rgba(palette["dark"]))
    draw.rectangle(_box((49, 108, 79, 115)), fill=_rgba(VOID))
    for x, y in ((16, 16), (112, 16), (16, 112), (112, 112)):
        _bolt(draw, x, y, 2)
    return _finish(image, 128)


def source_frames() -> Iterator[tuple[str, Image.Image]]:
    """Yield every approved frame in stable production-key order."""
    for faction in ("ferrous", "cupric"):
        for cargo in range(HARVESTER_CARGO_LEVELS):
            prefix = f"harvester_{faction}_cargo{cargo}"
            yield prefix, render_harvester(faction, cargo=cargo)
            yield f"{prefix}_tread1", render_harvester(faction, cargo=cargo, tread=1)
            yield f"{prefix}_tread2", render_harvester(faction, cargo=cargo, tread=2)
            yield f"{prefix}_scoop1", render_harvester(faction, cargo=cargo, scoop=1)
            yield f"{prefix}_scoop2", render_harvester(faction, cargo=cargo, scoop=2)
        yield f"sentinel_{faction}", render_sentinel(faction)
        yield f"sentinel_{faction}_move1", render_sentinel(faction, move=1)
        yield f"sentinel_{faction}_move2", render_sentinel(faction, move=2)
        for action in range(1, 5):
            yield (
                f"sentinel_{faction}_action{action}",
                render_sentinel(faction, action=action),
            )
        yield f"scuttler_{faction}", render_scuttler(faction)
        yield f"scuttler_{faction}_move1", render_scuttler(faction, move=1)
        yield f"scuttler_{faction}_move2", render_scuttler(faction, move=2)
        for action in range(1, 5):
            yield (
                f"scuttler_{faction}_action{action}",
                render_scuttler(faction, action=action),
            )
        yield f"foundry_{faction}", render_foundry(faction)
        for work in range(1, 5):
            yield f"foundry_{faction}_work{work}", render_foundry(faction, work=work)


def source_rgba_digest() -> str:
    """Digest every approved native frame in installation order."""
    digest = hashlib.sha256()
    for key, image in source_frames():
        digest.update(key.encode())
        digest.update(image.convert("RGBA").tobytes())
    return digest.hexdigest()


def _put(registry: Registry, out: Path, key: str, image: Image.Image) -> None:
    native = image.convert("RGBA")
    native.save(out / f"{key}.png")
    registry[key] = native


def install_core_unit_art(registry: Registry, out: Path) -> None:
    """Install all four approved families into the live sprite bank."""
    out.mkdir(parents=True, exist_ok=True)
    for key, image in source_frames():
        _put(registry, out, key, image)
    for faction in ("ferrous", "cupric"):
        aliases = {
            f"harvester_{faction}": f"harvester_{faction}_cargo0",
            f"harvester_{faction}_tread1": f"harvester_{faction}_cargo0_tread1",
            f"harvester_{faction}_tread2": f"harvester_{faction}_cargo0_tread2",
            f"harvester_{faction}_scoop1": f"harvester_{faction}_cargo0_scoop1",
            f"harvester_{faction}_scoop2": f"harvester_{faction}_cargo0_scoop2",
        }
        for alias, source_key in aliases.items():
            _put(registry, out, alias, registry[source_key])
