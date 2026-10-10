"""Production-native frames for the tier-two line brawler.

The Warden keeps a fixed four-pod hull while its tread cleats travel and its
protected fork cannon recoils.
"""

from __future__ import annotations

import hashlib
from pathlib import Path

from PIL import (  # ty: ignore[unresolved-import]
    Image,
    ImageChops,
    ImageDraw,
    ImageFilter,
)

from tools.gen_sprites import save_sprite, variant_tag

Registry = dict[str, Image.Image]
Color = tuple[int, int, int]

SIZE = 128
WARDEN_ACTION_COUNT = 4

BLACK = (11, 11, 15)
IRON_DEEP = (27, 27, 34)
IRON_DARK = (42, 42, 50)
IRON = (62, 62, 72)
IRON_LIGHT = (92, 91, 99)
BONE = (226, 220, 204)
FLASH = (255, 220, 132)
PALETTES = {
    "base": ((176, 75, 52), (105, 43, 33)),
    "probe": ((48, 132, 113), (29, 79, 68)),
}

APPROVED_SOURCE_RGBA_SHA256 = (
    "686dfe4eedcb3a2c4403c543ce19235c73e8cc8013a637a8071baa61bc08d32c"
)


def _rgba(color: Color, alpha: int = 255) -> tuple[int, int, int, int]:
    return (*color, alpha)


def _canvas() -> tuple[Image.Image, ImageDraw.ImageDraw]:
    image = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    return image, ImageDraw.Draw(image)


def _polygon(
    draw: ImageDraw.ImageDraw,
    points: tuple[tuple[int, int], ...],
    color: Color,
) -> None:
    draw.polygon(points, fill=_rgba(color))


def _finish(image: Image.Image) -> Image.Image:
    alpha = image.getchannel("A")
    grown = alpha.filter(ImageFilter.MaxFilter(3))
    edge = ImageChops.subtract(grown, alpha)
    shifted = ImageChops.subtract(
        edge,
        edge.transform(edge.size, Image.AFFINE, (1, 0, -1, 0, 1, -1)),
    )
    rim = Image.new("RGBA", image.size, (255, 244, 224, 0))
    rim.putalpha(shifted.point(lambda value: min(value, 105)))
    out = image.copy()
    out.alpha_composite(rim)
    return out


def _track(
    draw: ImageDraw.ImageDraw,
    box: tuple[int, int, int, int],
    phase: int,
    accent: Color,
) -> None:
    x0, y0, x1, y1 = box
    draw.rounded_rectangle(box, radius=6, fill=_rgba(BLACK))
    draw.rounded_rectangle(
        (x0 + 3, y0 + 3, x1 - 3, y1 - 3),
        radius=4,
        fill=_rgba(IRON_DEEP),
    )
    span = max(12, y1 - y0 - 13)
    for index in range(9):
        y = y0 + 5 + (index * 9 + phase * 4) % span
        if y + 3 >= y1 - 2:
            continue
        color = IRON_LIGHT if (index + phase) % 2 == 0 else IRON
        draw.rectangle((x0 + 2, y, x1 - 2, y + 3), fill=_rgba(color))
    draw.rectangle((x0 + 5, y0 + 15, x1 - 5, y0 + 21), fill=_rgba(accent))
    draw.line(
        (x0 + 4, y0 + 5, x0 + 4, y1 - 6),
        fill=_rgba(IRON_LIGHT),
        width=2,
    )


def _warden_barrel(draw: ImageDraw.ImageDraw, recoil: int) -> None:
    width = 13
    draw.rounded_rectangle(
        (64 - width // 2 - 3, 8 + recoil, 64 + width // 2 + 3, 68 + recoil),
        radius=4,
        fill=_rgba(BLACK),
    )
    draw.rectangle(
        (64 - width // 2, 11 + recoil, 64 + width // 2, 64 + recoil),
        fill=_rgba(IRON_LIGHT),
    )
    draw.rectangle((61, 12 + recoil, 67, 64 + recoil), fill=_rgba(IRON))
    draw.rectangle((52, 8 + recoil, 58, 29 + recoil), fill=_rgba(BLACK))
    draw.rectangle((70, 8 + recoil, 76, 29 + recoil), fill=_rgba(BLACK))
    draw.rectangle((54, 10 + recoil, 57, 27 + recoil), fill=_rgba(BONE))
    draw.rectangle((71, 10 + recoil, 74, 27 + recoil), fill=_rgba(BONE))


def _muzzle_flash(draw: ImageDraw.ImageDraw, x: int, y: int) -> None:
    _polygon(
        draw,
        (
            (x, y - 10),
            (x + 5, y - 3),
            (x + 12, y),
            (x + 5, y + 4),
            (x, y + 10),
            (x - 5, y + 4),
            (x - 12, y),
            (x - 5, y - 3),
        ),
        FLASH,
    )
    draw.rectangle((x - 3, y - 4, x + 3, y + 4), fill=_rgba(BONE))


def render_warden(
    variant: str,
    move_phase: int = 0,
    action: int = 0,
) -> Image.Image:
    """Render one approved four-pod Warden frame."""
    if variant not in PALETTES:
        raise ValueError(f"unknown variant: {variant}")
    if action not in range(WARDEN_ACTION_COUNT + 1):
        raise ValueError(f"unknown Warden action: {action}")
    image, draw = _canvas()
    primary, dark = PALETTES[variant]
    recoil = (0, 1, 0, 10, 4)[action]
    for box in (
        (7, 18, 31, 61),
        (7, 72, 31, 116),
        (97, 18, 121, 61),
        (97, 72, 121, 116),
    ):
        _track(draw, box, move_phase % 3, dark)
    _polygon(
        draw,
        (
            (27, 29),
            (44, 16),
            (84, 16),
            (101, 29),
            (97, 102),
            (82, 112),
            (46, 112),
            (31, 102),
        ),
        BLACK,
    )
    _polygon(
        draw,
        (
            (35, 33),
            (48, 24),
            (80, 24),
            (93, 33),
            (89, 96),
            (78, 104),
            (50, 104),
            (39, 96),
        ),
        IRON_DARK,
    )
    draw.rectangle((38, 82, 90, 101), fill=_rgba(primary))
    draw.ellipse((42, 30 + recoil, 86, 76 + recoil), fill=_rgba(BLACK))
    draw.ellipse((49, 37 + recoil, 79, 69 + recoil), fill=_rgba(IRON))
    _warden_barrel(draw, recoil)
    if action == 2:
        _muzzle_flash(draw, 64, 4)
    return _finish(image)


def source_rgba_digest() -> str:
    """Digest every production frame in the installed source order."""
    digest = hashlib.sha256()
    states = (
        ("idle", 0, 0),
        ("move1", 1, 0),
        ("move2", 2, 0),
        *(
            (f"action{action}", 0, action)
            for action in range(1, WARDEN_ACTION_COUNT + 1)
        ),
    )
    for variant in ("base", "probe"):
        for label, move_phase, action in states:
            digest.update(f"warden/{variant}/{label}".encode())
            digest.update(render_warden(variant, move_phase, action).tobytes())
    return digest.hexdigest()


def _put(registry: Registry, out: Path, key: str, image: Image.Image) -> None:
    native = image.convert("RGBA")
    save_sprite(native, out, key)
    registry[key] = native


def install_warden(registry: Registry, out: Path) -> None:
    """Install the approved Warden rows into the production bank."""
    out.mkdir(parents=True, exist_ok=True)
    for variant in ("base", "probe"):
        tag = variant_tag(variant)
        _put(registry, out, f"warden{tag}", render_warden(variant, 0, 0))
        for move_phase in (1, 2):
            _put(
                registry,
                out,
                f"warden{tag}_move{move_phase}",
                render_warden(variant, move_phase, 0),
            )
        for action in range(1, WARDEN_ACTION_COUNT + 1):
            _put(
                registry,
                out,
                f"warden{tag}_action{action}",
                render_warden(variant, 0, action),
            )
