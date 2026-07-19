# /// script
# requires-python = ">=3.10"
# dependencies = ["pillow"]
# ///
"""Generates every sprite in assets/sprites/.

This script is the source of truth for the game's art: edit here, run
`uv run tools/gen_sprites.py`, and commit the PNGs together with the script.
Output is deterministic (fixed seeds, no timestamps in the pixels) so a
regenerated sprite only differs when the code does.

Style: flat top-down geometry, supersampled 4x for clean edges. Factions
share silhouettes and differ only in accent color — Ferrous rusts orange,
Cupric corrodes teal. Units face up; the shell rotates them toward their
heading.
"""

from __future__ import annotations

import random
from pathlib import Path

from PIL import Image, ImageDraw

OUT = Path(__file__).resolve().parent.parent / "assets" / "sprites"
SS = 4  # supersample factor

# The Oxide palette. Keep in sync with driver/src/render.rs and the shell.
GROUND = (35, 35, 41)
GROUND_DARK = (28, 28, 34)
GROUND_LIGHT = (44, 44, 52)
ROCK = (82, 82, 94)
ROCK_DARK = (58, 58, 68)
ROCK_LIGHT = (104, 104, 118)
SCRAP = (217, 164, 65)
SCRAP_DARK = (140, 106, 47)
SCRAP_LIGHT = (240, 200, 120)
IRON = (52, 52, 62)
IRON_DARK = (38, 38, 46)
IRON_LIGHT = (72, 72, 84)
BONE = (232, 228, 216)

FACTIONS = {
    "ferrous": {
        "base": (196, 87, 59),
        "dark": (126, 56, 38),
        "light": (232, 137, 107),
    },
    "cupric": {
        "base": (63, 148, 130),
        "dark": (39, 96, 79),
        "light": (119, 196, 176),
    },
}


def canvas(px: int, color=(0, 0, 0, 0)) -> tuple[Image.Image, ImageDraw.ImageDraw]:
    img = Image.new("RGBA", (px * SS, px * SS), color)
    return img, ImageDraw.Draw(img)


def finish(img: Image.Image, px: int, name: str) -> None:
    img = img.resize((px, px), Image.LANCZOS)
    img.save(OUT / f"{name}.png")
    print(f"  {name}.png")


def s(v: float) -> int:
    """Scale a sprite-space coordinate by the supersample factor."""
    return round(v * SS)


def ground(variant: int) -> None:
    px = 64
    img, d = canvas(px, (*GROUND, 255))
    rng = random.Random(1000 + variant)
    # Sparse grit: a few darker and lighter flecks, nothing that tiles loudly.
    for _ in range(26):
        x, y = rng.randrange(2, 62), rng.randrange(2, 62)
        w = rng.choice((1, 1, 2))
        color = rng.choice((GROUND_DARK, GROUND_DARK, GROUND_LIGHT))
        d.rectangle([s(x), s(y), s(x + w), s(y + w)], fill=(*color, 255))
    # One hairline crack.
    x0, y0 = rng.randrange(6, 30), rng.randrange(6, 58)
    points = [(x0, y0)]
    for _ in range(rng.randrange(3, 5)):
        x0 += rng.randrange(4, 12)
        y0 += rng.randrange(-6, 7)
        points.append((x0, y0))
    d.line([(s(x), s(y)) for x, y in points], fill=(*GROUND_DARK, 255), width=SS)
    finish(img, px, f"ground_{variant}")


def rock() -> None:
    px = 64
    img, d = canvas(px)
    rng = random.Random(7)

    def boulder(cx, cy, r, tone, dark, light):
        pts = []
        n = 9
        for i in range(n):
            angle = i / n * 6.28318
            wobble = r * (0.78 + 0.22 * rng.random())
            pts.append((cx + wobble * __import__("math").cos(angle),
                        cy + wobble * __import__("math").sin(angle)))
        d.polygon([(s(x), s(y)) for x, y in pts], fill=(*tone, 255))
        # Top-left facet catches light; bottom-right falls into shade.
        facet = [(s(x * 0.62 + cx * 0.38 - r * 0.12), s(y * 0.62 + cy * 0.38 - r * 0.12))
                 for x, y in pts[:5]]
        d.polygon(facet, fill=(*light, 255))
        shade = [(s(x * 0.72 + cx * 0.28 + r * 0.10), s(y * 0.72 + cy * 0.28 + r * 0.10))
                 for x, y in pts[4:9]]
        d.polygon(shade, fill=(*dark, 255))

    boulder(30, 34, 24, ROCK, ROCK_DARK, ROCK_LIGHT)
    boulder(46, 44, 12, ROCK, ROCK_DARK, ROCK_LIGHT)
    boulder(16, 48, 8, ROCK_DARK, ROCK_DARK, ROCK)
    finish(img, px, "rock")


def scrap(stage: str, fullness: float) -> None:
    px = 64
    img, d = canvas(px)
    rng = random.Random(11)
    pieces = int(16 * fullness) + 3
    spread = 20 * fullness + 6
    for _ in range(pieces):
        cx = 32 + rng.uniform(-spread, spread)
        cy = 32 + rng.uniform(-spread, spread)
        w, h = rng.uniform(5, 12), rng.uniform(4, 9)
        angle_steps = rng.choice((0, 1, 2, 3))
        tone = rng.choice((SCRAP, SCRAP, SCRAP_DARK, SCRAP_LIGHT))
        box = [
            (cx - w / 2, cy - h / 2), (cx + w / 2, cy - h / 2),
            (cx + w / 2, cy + h / 2), (cx - w / 2, cy + h / 2),
        ]
        if angle_steps:  # crude rotation by shearing corners — looks like junk, which it is
            dx = rng.uniform(-2.5, 2.5)
            box = [(x + (dx if i % 2 else -dx), y) for i, (x, y) in enumerate(box)]
        d.polygon([(s(x), s(y)) for x, y in box], fill=(*tone, 255))
    # A glinting bolt or two on top.
    for _ in range(max(1, int(3 * fullness))):
        cx, cy = 32 + rng.uniform(-10, 10), 32 + rng.uniform(-10, 10)
        d.ellipse([s(cx - 2), s(cy - 2), s(cx + 2), s(cy + 2)], fill=(*BONE, 255))
    finish(img, px, f"scrap_{stage}")


def foundry(faction: str) -> None:
    px = 128
    pal = FACTIONS[faction]
    img, d = canvas(px)
    # Baseplate with a bevel.
    d.rounded_rectangle([s(6), s(6), s(122), s(122)], radius=s(10), fill=(*IRON_DARK, 255))
    d.rounded_rectangle([s(12), s(12), s(116), s(116)], radius=s(8), fill=(*IRON, 255))
    # Faction roof panels, chevroned toward the center.
    d.polygon([(s(12), s(12)), (s(64), s(12)), (s(12), s(64))], fill=(*pal["dark"], 255))
    d.polygon([(s(116), s(116)), (s(64), s(116)), (s(116), s(64))], fill=(*pal["dark"], 255))
    d.rectangle([s(20), s(20), s(108), s(108)], fill=(*IRON, 255))
    d.rectangle([s(26), s(26), s(102), s(102)], fill=(*pal["base"], 255))
    d.rectangle([s(34), s(34), s(94), s(94)], fill=(*IRON_DARK, 255))
    # The melt pool — the glowing heart of the works.
    d.ellipse([s(44), s(44), s(84), s(84)], fill=(*pal["base"], 255))
    d.ellipse([s(50), s(50), s(78), s(78)], fill=(*pal["light"], 255))
    d.ellipse([s(57), s(57), s(71), s(71)], fill=(*BONE, 255))
    # Chimney stack, top-right, with a dark throat.
    d.ellipse([s(88), s(16), s(112), s(40)], fill=(*IRON_LIGHT, 255))
    d.ellipse([s(93), s(21), s(107), s(35)], fill=(*IRON_DARK, 255))
    # Rivets on the corners that have no chimney.
    for cx, cy in ((22, 22), (22, 106), (106, 106)):
        d.ellipse([s(cx - 3), s(cy - 3), s(cx + 3), s(cy + 3)], fill=(*IRON_LIGHT, 255))
    finish(img, px, f"foundry_{faction}")


def harvester(faction: str) -> None:
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    # Treads flanking the hull.
    d.rounded_rectangle([s(12), s(14), s(22), s(54)], radius=s(4), fill=(*IRON_DARK, 255))
    d.rounded_rectangle([s(42), s(14), s(52), s(54)], radius=s(4), fill=(*IRON_DARK, 255))
    for y in range(18, 52, 6):
        d.rectangle([s(13), s(y), s(21), s(y + 2)], fill=(*IRON, 255))
        d.rectangle([s(43), s(y), s(51), s(y + 2)], fill=(*IRON, 255))
    # Hull.
    d.rounded_rectangle([s(20), s(18), s(44), s(52)], radius=s(6), fill=(*IRON, 255))
    d.rounded_rectangle([s(23), s(21), s(41), s(49)], radius=s(5), fill=(*pal["base"], 255))
    d.rounded_rectangle([s(26), s(30), s(38), s(46)], radius=s(3), fill=(*pal["dark"], 255))
    # Scoop out front (up = forward).
    d.polygon(
        [(s(18), s(16)), (s(46), s(16)), (s(40), s(6)), (s(24), s(6))],
        fill=(*IRON_LIGHT, 255),
    )
    d.polygon(
        [(s(21), s(15)), (s(43), s(15)), (s(38), s(8)), (s(26), s(8))],
        fill=(*IRON_DARK, 255),
    )
    # Cargo eye — the shell can read "carrying" at a glance someday.
    d.ellipse([s(28), s(34), s(36), s(42)], fill=(*SCRAP_DARK, 255))
    finish(img, px, f"harvester_{faction}")


def sentinel(faction: str) -> None:
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    # Angular chassis: a blunt arrowhead pointing up.
    hull = [(32, 6), (50, 30), (46, 54), (18, 54), (14, 30)]
    d.polygon([(s(x), s(y)) for x, y in hull], fill=(*IRON, 255))
    inner = [(32, 12), (45, 31), (42, 49), (22, 49), (19, 31)]
    d.polygon([(s(x), s(y)) for x, y in inner], fill=(*pal["base"], 255))
    core = [(32, 22), (39, 33), (37, 44), (27, 44), (25, 33)]
    d.polygon([(s(x), s(y)) for x, y in core], fill=(*pal["dark"], 255))
    # Weapon pods.
    d.ellipse([s(15), s(30), s(25), s(40)], fill=(*IRON_DARK, 255))
    d.ellipse([s(39), s(30), s(49), s(40)], fill=(*IRON_DARK, 255))
    # Barrel, forward.
    d.rectangle([s(29), s(2), s(35), s(20)], fill=(*IRON_DARK, 255))
    d.rectangle([s(30.5), s(2), s(33.5), s(18)], fill=(*IRON_LIGHT, 255))
    # Sight.
    d.ellipse([s(29), s(24), s(35), s(30)], fill=(*pal["light"], 255))
    finish(img, px, f"sentinel_{faction}")


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    print(f"writing {OUT}")
    for i in range(3):
        ground(i)
    rock()
    scrap("full", 1.0)
    scrap("mid", 0.55)
    scrap("low", 0.25)
    for faction in FACTIONS:
        foundry(faction)
        harvester(faction)
        sentinel(faction)
    print("done")


if __name__ == "__main__":
    main()
