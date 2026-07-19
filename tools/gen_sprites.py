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

import json
import math
import random
from pathlib import Path

from PIL import Image, ImageDraw

OUT = Path(__file__).resolve().parent.parent / "assets" / "sprites"
SS = 4  # supersample factor

# Every finished sprite lands here too, so main() can pack one atlas the
# shell renders from — one texture means one GPU batch for the whole world.
REGISTRY: dict[str, Image.Image] = {}

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
    if name.startswith(("harvester", "sentinel")):
        img = rim_light(img)
    img.save(OUT / f"{name}.png")
    REGISTRY[name] = img
    print(f"  {name}.png")


def rim_light(img: Image.Image) -> Image.Image:
    """A one-pixel warm edge along the top-left silhouette — keeps units
    readable at small zoom against dark ground."""
    from PIL import ImageChops, ImageFilter

    alpha = img.split()[3]
    grown = alpha.filter(ImageFilter.MaxFilter(3))
    edge = ImageChops.subtract(grown, alpha)
    shifted = ImageChops.subtract(edge, edge.transform(edge.size, Image.AFFINE, (1, 0, -1, 0, 1, -1)))
    rim = Image.new("RGBA", img.size, (255, 244, 224, 0))
    rim.putalpha(shifted.point(lambda v: min(v, 110)))
    out = img.copy()
    out.alpha_composite(rim)
    return out


def pack_atlas() -> None:
    """Shelf-packs every registered sprite into atlas.png + atlas.json.

    Deterministic: sprites are placed tallest-first, then by name. Each
    sprite gets a padded cell with its own edges extruded one pixel into the
    padding, so linear filtering at any zoom never bleeds a neighbor (or
    transparency) into the sample.
    """
    pad = 2
    atlas_w = 512
    entries = sorted(REGISTRY.items(), key=lambda kv: (-kv[1].height, kv[0]))
    placements: dict[str, tuple[int, int, int, int]] = {}
    x, y, shelf_h = pad, pad, 0
    for name, img in entries:
        w, h = img.width, img.height
        if x + w + pad > atlas_w:
            x = pad
            y += shelf_h + 2 * pad
            shelf_h = 0
        placements[name] = (x, y, w, h)
        shelf_h = max(shelf_h, h)
        x += w + 2 * pad
    atlas_h = y + shelf_h + pad
    atlas = Image.new("RGBA", (atlas_w, atlas_h), (0, 0, 0, 0))
    for name, (px_, py, w, h) in placements.items():
        img = REGISTRY[name]
        atlas.paste(img, (px_, py))
        # 1px edge extrusion into the padding.
        atlas.paste(img.crop((0, 0, w, 1)), (px_, py - 1))
        atlas.paste(img.crop((0, h - 1, w, h)), (px_, py + h))
        atlas.paste(img.crop((0, 0, 1, h)), (px_ - 1, py))
        atlas.paste(img.crop((w - 1, 0, w, h)), (px_ + w, py))
    atlas.save(OUT / "atlas.png")
    with open(OUT / "atlas.json", "w") as f:
        json.dump(
            {name: list(rect) for name, rect in sorted(placements.items())},
            f,
            indent=1,
            sort_keys=True,
        )
    print(f"  atlas.png ({atlas_w}x{atlas_h}, {len(placements)} sprites) + atlas.json")


def s(v: float) -> int:
    """Scale a sprite-space coordinate by the supersample factor."""
    return round(v * SS)


def ground(variant: int) -> None:
    px = 64
    # Variants sweep a subtle brightness range so the field reads as varied
    # terrain instead of a flat wash.
    lift = [-6, -3, 0, 2, 4, 7][variant % 6]
    base = tuple(max(0, c + lift) for c in GROUND)
    img, d = canvas(px, (*base, 255))
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


def rock(variant: int) -> None:
    px = 64
    img, d = canvas(px)
    rng = random.Random(7 + variant * 31)

    def boulder(cx, cy, r, tone, dark, light):
        pts = []
        n = 9
        for i in range(n):
            angle = i / n * 6.28318
            wobble = r * (0.78 + 0.22 * rng.random())
            pts.append((cx + wobble * math.cos(angle), cy + wobble * math.sin(angle)))
        d.polygon([(s(x), s(y)) for x, y in pts], fill=(*tone, 255))
        # Top-left facet catches light; bottom-right falls into shade.
        facet = [(s(x * 0.62 + cx * 0.38 - r * 0.12), s(y * 0.62 + cy * 0.38 - r * 0.12))
                 for x, y in pts[:5]]
        d.polygon(facet, fill=(*light, 255))
        shade = [(s(x * 0.72 + cx * 0.28 + r * 0.10), s(y * 0.72 + cy * 0.28 + r * 0.10))
                 for x, y in pts[4:9]]
        d.polygon(shade, fill=(*dark, 255))

    layouts = [
        [(30, 34, 24), (46, 44, 12), (16, 48, 8)],
        [(34, 28, 22), (18, 40, 14), (46, 50, 9)],
        [(26, 40, 26), (48, 24, 11)],
        [(32, 32, 18), (14, 26, 10), (50, 46, 12), (20, 52, 7)],
    ]
    for i, (cx, cy, r) in enumerate(layouts[variant % 4]):
        tone = ROCK if i < 2 else ROCK_DARK
        boulder(cx, cy, r, tone, ROCK_DARK, ROCK_LIGHT if i < 2 else ROCK)
    finish(img, px, f"rock_{variant}")


def scrap_pile(name: str, seed: int, pieces: int, spread: float, lift: float) -> None:
    """A mounded salvage heap: shadow base, center-biased shards piled so
    they overlap into one mass, glints on the crown. `lift` raises the
    crown for taller piles."""
    px = 64
    img, d = canvas(px)
    rng = random.Random(seed)
    base_r = spread + 5
    d.ellipse(
        [s(32 - base_r), s(36 - base_r * 0.6), s(32 + base_r), s(36 + base_r * 0.6)],
        fill=(24, 20, 16, 120),
    )
    # Far pieces first so central ones stack on top.
    placed = []
    for _ in range(pieces):
        angle = rng.uniform(0, 6.28318)
        dist = spread * rng.random() ** 0.6
        cx = 32 + dist * math.cos(angle)
        cy = 34 + dist * math.sin(angle) * 0.65 - lift * (1 - dist / spread)
        w = rng.uniform(6, 12) * (1.15 - 0.4 * dist / spread)
        h = rng.uniform(4, 9) * (1.15 - 0.4 * dist / spread)
        placed.append((dist, cx, cy, w, h))
    placed.sort(key=lambda p: -p[0])
    for dist, cx, cy, w, h in placed:
        # Central pieces catch light; the fringe sits in shade.
        tone = (
            rng.choice((SCRAP, SCRAP_LIGHT))
            if dist < spread * 0.45
            else rng.choice((SCRAP, SCRAP_DARK, SCRAP_DARK))
        )
        dx = rng.uniform(-2.5, 2.5)
        box = [
            (cx - w / 2 - dx, cy - h / 2),
            (cx + w / 2, cy - h / 2 + dx / 2),
            (cx + w / 2 + dx, cy + h / 2),
            (cx - w / 2, cy + h / 2 - dx / 2),
        ]
        d.polygon([(s(x), s(y)) for x, y in box], fill=(*tone, 255))
    for _ in range(max(1, pieces // 6)):
        cx, cy = 32 + rng.uniform(-6, 6), 32 - lift * 0.7 + rng.uniform(-4, 4)
        d.ellipse([s(cx - 2), s(cy - 2), s(cx + 2), s(cy + 2)], fill=(*BONE, 255))
    finish(img, px, name)


def scrap(stage: str, fullness: float) -> None:
    scrap_pile(
        f"scrap_{stage}",
        seed=11,
        pieces=int(14 * fullness) + 4,
        spread=10 * fullness + 6,
        lift=3 * fullness,
    )


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


def rock_skirt() -> None:
    """A soft shadow cast from the top edge; rotated at draw time toward
    whichever neighbor holds the rock."""
    px = 64
    img, d = canvas(px)
    for row in range(20):
        alpha = int(70 * (1.0 - row / 20) ** 1.6)
        d.rectangle([0, s(row), s(64), s(row + 1)], fill=(10, 10, 14, alpha))
    finish(img, px, "rock_skirt")


def decal(name: str, seed: int, style: str) -> None:
    px = 64
    img, d = canvas(px)
    rng = random.Random(seed)
    if style == "crack":
        x, y = rng.randrange(8, 24), rng.randrange(12, 52)
        pts = [(x, y)]
        for _ in range(rng.randrange(4, 6)):
            x += rng.randrange(4, 12)
            y += rng.randrange(-8, 9)
            pts.append((x, y))
        d.line([(s(a), s(b)) for a, b in pts], fill=(16, 16, 20, 160), width=SS)
        for a, b in pts[1:-1]:
            if rng.random() < 0.6:
                d.line(
                    [(s(a), s(b)), (s(a + rng.randrange(-6, 7)), s(b + rng.randrange(3, 9)))],
                    fill=(16, 16, 20, 120),
                    width=SS,
                )
    elif style == "plate":
        x, y = rng.randrange(10, 26), rng.randrange(10, 26)
        w, h = rng.randrange(18, 30), rng.randrange(14, 24)
        d.rectangle([s(x), s(y), s(x + w), s(y + h)], outline=(70, 70, 82, 110), width=SS)
        for cx, cy in [(x + 3, y + 3), (x + w - 3, y + 3), (x + 3, y + h - 3), (x + w - 3, y + h - 3)]:
            d.ellipse([s(cx - 1), s(cy - 1), s(cx + 1), s(cy + 1)], fill=(70, 70, 82, 140))
    elif style == "stain":
        for _ in range(rng.randrange(3, 5)):
            cx, cy = rng.randrange(16, 48), rng.randrange(16, 48)
            r = rng.randrange(6, 14)
            d.ellipse([s(cx - r), s(cy - r), s(cx + r), s(cy + r)], fill=(24, 20, 16, 60))
    elif style == "wreck":
        for _ in range(rng.randrange(4, 7)):
            cx, cy = rng.randrange(12, 52), rng.randrange(12, 52)
            w, h = rng.randrange(4, 10), rng.randrange(3, 7)
            tone = rng.choice([(58, 58, 68), (44, 44, 52), (86, 64, 40)])
            d.polygon(
                [
                    (s(cx), s(cy)),
                    (s(cx + w), s(cy + rng.randrange(-2, 3))),
                    (s(cx + w - rng.randrange(0, 3)), s(cy + h)),
                    (s(cx - rng.randrange(0, 3)), s(cy + h - 1)),
                ],
                fill=(*tone, 150),
            )
    finish(img, px, name)


def scrap_rich() -> None:
    """A dense, tall heap — the 'S' legend's double-value node."""
    scrap_pile("scrap_rich", seed=23, pieces=30, spread=19, lift=7)


def muzzle_flash() -> None:
    px = 32
    img, d = canvas(px)
    for r, alpha in [(11, 90), (7, 170), (4, 255)]:
        d.ellipse([s(16 - r), s(16 - r), s(16 + r), s(16 + r)], fill=(255, 240, 200, alpha))
    d.polygon([(s(16), s(2)), (s(19), s(13)), (s(13), s(13))], fill=(255, 240, 200, 220))
    d.polygon([(s(16), s(30)), (s(19), s(19)), (s(13), s(19))], fill=(255, 240, 200, 220))
    finish(img, px, "muzzle_flash")


def scorch() -> None:
    px = 128
    img, d = canvas(px)
    rng = random.Random(77)
    for r, alpha in [(56, 60), (44, 90), (30, 120)]:
        d.ellipse([s(64 - r), s(64 - r), s(64 + r), s(64 + r)], fill=(12, 10, 10, alpha))
    for _ in range(14):
        ang = rng.uniform(0, 6.28318)
        import math as _m
        fr = rng.uniform(30, 60)
        x, y = 64 + fr * _m.cos(ang), 64 + fr * _m.sin(ang)
        d.ellipse([s(x - 3), s(y - 3), s(x + 3), s(y + 3)], fill=(16, 14, 12, 90))
    finish(img, px, "scorch")


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    print(f"writing {OUT}")
    for i in range(6):
        ground(i)
    for i in range(4):
        rock(i)
    rock_skirt()
    decal("decal_crack", 41, "crack")
    decal("decal_plate", 42, "plate")
    decal("decal_stain", 43, "stain")
    decal("decal_wreck", 44, "wreck")
    scrap_rich()
    muzzle_flash()
    scorch()
    scrap("full", 1.0)
    scrap("mid", 0.55)
    scrap("low", 0.25)
    for faction in FACTIONS:
        foundry(faction)
        harvester(faction)
        sentinel(faction)
    pack_atlas()
    print("done")


if __name__ == "__main__":
    main()
