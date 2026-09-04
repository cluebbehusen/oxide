"""Approved production renderers for the Barricade and Scuttle Charge."""

from __future__ import annotations

import hashlib
from collections.abc import Callable, Iterator
from pathlib import Path

from PIL import Image, ImageDraw

from tools import gen_sprites as gen

Registry = dict[str, Image.Image]

SS = 4
BARRICADE_SOURCE_RGBA_SHA256 = (
    "a90a9c6d4239fb80d770daccbd0e520d937d38dbd03b733f9222a4e91f3b6653"
)
SCUTTLE_CHARGE_SOURCE_RGBA_SHA256 = (
    "06e4784d5d38a8104fb413bcc60e4a7f9efd3cd5cc39abfe1b2000bdde76a4ea"
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


def _new_sprite() -> tuple[Image.Image, ImageDraw.ImageDraw]:
    image = Image.new("RGBA", (64 * SS, 64 * SS), (0, 0, 0, 0))
    return image, ImageDraw.Draw(image)


def _finish(image: Image.Image) -> Image.Image:
    return image.resize((64, 64), Image.Resampling.LANCZOS)


def _plate(
    draw: ImageDraw.ImageDraw,
    bounds: tuple[float, float, float, float],
    *,
    fill: tuple[int, int, int] = gen.IRON,
    edge: tuple[int, int, int] = gen.IRON_DARK,
    highlight: tuple[int, int, int] = gen.IRON_LIGHT,
    radius: float = 2,
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


def _bolt(draw: ImageDraw.ImageDraw, x: float, y: float) -> None:
    draw.ellipse(
        _box((x - 1.5, y - 1.5, x + 1.5, y + 1.5)),
        fill=_rgba(gen.IRON_DARK),
    )
    draw.rectangle(
        _box((x - 0.5, y - 0.5, x + 0.5, y + 0.5)),
        fill=_rgba(gen.BONE, 215),
    )


def render_barricade(faction: str) -> Image.Image:
    """Render the connected bulkhead wall."""
    palette = gen.FACTIONS[faction]
    image, draw = _new_sprite()
    draw.rounded_rectangle(
        _box((1, 44, 63, 59)), radius=_s(3), fill=_rgba(gen.IRON_DARK)
    )
    draw.rectangle(_box((4, 48, 60, 56)), fill=_rgba((17, 18, 23)))
    for x in (7, 31, 55):
        draw.polygon(
            _points(((x - 6, 58), (x - 3, 45), (x + 3, 45), (x + 6, 58))),
            fill=_rgba(gen.IRON),
        )
        _bolt(draw, x, 52)
    _plate(draw, (0, 12, 64, 48), fill=(48, 48, 57), radius=2)
    for x in (21, 43):
        draw.rectangle(_box((x - 3, 13, x + 3, 48)), fill=_rgba(gen.IRON_DARK))
        draw.rectangle(_box((x - 1, 16, x + 1, 45)), fill=_rgba(gen.IRON_LIGHT))
        _bolt(draw, x, 20)
        _bolt(draw, x, 40)
    draw.rectangle(_box((0, 9, 64, 16)), fill=_rgba(gen.IRON_DARK))
    draw.rectangle(_box((2, 11, 62, 13)), fill=_rgba(gen.IRON_LIGHT))
    for x0, x1 in ((5, 17), (27, 37), (47, 59)):
        draw.rectangle(_box((x0, 28, x1, 38)), fill=_rgba(palette["dark"]))
        draw.rectangle(_box((x0 + 2, 29, x1 - 2, 31)), fill=_rgba(palette["base"]))
    return _finish(image)


def _anchor(draw: ImageDraw.ImageDraw, x: float, y: float, orientation: str) -> None:
    bounds = (
        (x - 5, y - 3, x + 5, y + 3)
        if orientation == "h"
        else (
            x - 3,
            y - 5,
            x + 3,
            y + 5,
        )
    )
    draw.rounded_rectangle(_box(bounds), radius=_s(1), fill=_rgba(gen.IRON_DARK))
    _bolt(draw, x, y)


def render_scuttle_charge(faction: str) -> Image.Image:
    """Render the recessed shaped-charge iris."""
    palette = gen.FACTIONS[faction]
    image, draw = _new_sprite()
    draw.ellipse(_box((10, 11, 54, 55)), fill=_rgba((14, 15, 19), 220))
    for x, y, orientation in (
        (32, 10, "h"),
        (32, 56, "h"),
        (9, 33, "v"),
        (55, 33, "v"),
    ):
        _anchor(draw, x, y, orientation)
    draw.ellipse(_box((14, 14, 50, 50)), fill=_rgba(gen.IRON_DARK))
    draw.ellipse(_box((18, 18, 46, 46)), fill=_rgba(gen.IRON))
    draw.ellipse(_box((22, 22, 42, 42)), fill=_rgba((9, 9, 12)))
    petals = (
        ((32, 23), (39, 25), (35, 31)),
        ((41, 28), (40, 36), (34, 33)),
        ((37, 40), (30, 42), (31, 35)),
        ((27, 41), (22, 35), (29, 33)),
        ((23, 30), (27, 24), (31, 30)),
    )
    for petal in petals:
        draw.polygon(_points(petal), fill=_rgba(palette["dark"]))
    draw.ellipse(_box((29, 29, 35, 35)), fill=_rgba(gen.IRON_DARK))
    draw.rectangle(_box((28, 47, 36, 51)), fill=_rgba(gen.IRON_DARK))
    draw.rectangle(_box((30, 48, 34, 49)), fill=_rgba(palette["light"]))
    return _finish(image)


def _source_digest(renderer: Callable[[str], Image.Image]) -> str:
    digest = hashlib.sha256()
    for faction in sorted(gen.FACTIONS):
        image = renderer(faction)
        digest.update(faction.encode())
        digest.update(image.mode.encode())
        digest.update(bytes(image.size))
        digest.update(image.tobytes())
    return digest.hexdigest()


def barricade_source_rgba_digest() -> str:
    return _source_digest(render_barricade)


def scuttle_charge_source_rgba_digest() -> str:
    return _source_digest(render_scuttle_charge)


def source_frames() -> Iterator[tuple[str, Image.Image]]:
    for faction in ("ferrous", "cupric"):
        yield f"barricade_{faction}", render_barricade(faction)
        yield f"scuttle_charge_{faction}", render_scuttle_charge(faction)


def install_field_structures(registry: Registry, out: Path) -> None:
    """Install both approved field structures into the production bank."""
    out.mkdir(parents=True, exist_ok=True)
    for key, image in source_frames():
        image.save(out / f"{key}.png")
        registry[key] = image
