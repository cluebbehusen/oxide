# /// script
# requires-python = ">=3.14"
# dependencies = ["pillow==12.3.0"]  # pinned: asset bytes must reproduce
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
PEAK = (66, 64, 82)
PEAK_DARK = (46, 44, 58)
PEAK_LIGHT = (118, 114, 138)
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
    if name.startswith(
        (
            "harvester",
            "sentinel",
            "scuttler",
            "lancer",
            "bombard",
            "flakhound",
            "stinger",
            "buzzard",
            "darter",
            "talon",
            "wisp",
        )
    ):
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


def _mix(a, b, t):
    return tuple(int(x + (y - x) * t) for x, y in zip(a, b))


# Skyline height where a ridge meets a connected tile side. Every
# connected edge uses this exact height, which is what lets adjacent
# ridge tiles join without a visible seam.
PEAK_EDGE_TOP = 40


def _peak_mass(d, rng, sky, caps):
    """Fill and facet a mountain mass under a left-to-right piecewise
    skyline. `caps` are indices into `sky` marking crest apexes; only
    the tallest earns the bright cap, and only when it stands high."""

    def sky_y(x):
        for (x0, y0), (x1, y1) in zip(sky, sky[1:]):
            if x0 <= x <= x1:
                t = 0.0 if x1 == x0 else (x - x0) / (x1 - x0)
                return y0 + (y1 - y0) * t
        return 64.0

    base = _mix(PEAK_DARK, PEAK, 0.4)
    poly = [(s(x), s(y)) for x, y in sky]
    poly += [(s(sky[-1][0]), s(64)), (s(sky[0][0]), s(64))]
    d.polygon(poly, fill=(*base, 255))
    tallest = min(caps, key=lambda i: sky[i][1])
    for i in caps:
        ax, ay = sky[i]
        # Lit west face sells the light direction; a thin lit ridge
        # line runs down the east shoulder.
        left = max(sky[0][0] + 1, ax - 16)
        fall = left + (ax - left) * 0.45
        d.polygon(
            [(s(left), s(64)), (s(ax), s(ay)), (s(fall), s(64))],
            fill=(*_mix(base, PEAK_LIGHT, 0.5), 255),
        )
        d.line(
            [(s(ax), s(ay)), (s(ax + 8), s(ay + (64 - ay) * 0.5))],
            fill=(*_mix(base, PEAK_LIGHT, 0.7), 255),
            width=SS,
        )
        if i == tallest and ay < 16:
            d.polygon(
                [(s(ax - 2), s(ay + 5)), (s(ax), s(ay - 1)), (s(ax + 2), s(ay + 5))],
                fill=(*_mix(PEAK_LIGHT, BONE, 0.45), 255),
            )
    # Scree inside the mass so the rock reads as rock, not paint.
    for _ in range(46):
        x, y = rng.randrange(0, 64), rng.randrange(0, 64)
        if y > sky_y(x) + 2:
            tone = _mix(PEAK_DARK, PEAK, rng.random() * 0.5)
            d.rectangle([s(x), s(y), s(x + 1), s(y + 1)], fill=(*tone, 255))


def peak_sky(w_conn: int, e_conn: int, variant: int) -> None:
    """The skyline row of a mountain range: crests against open sky over
    a full-width rock base (the base always spans the tile so a wall
    below joins cleanly). Connected sides meet the edge at
    PEAK_EDGE_TOP; open sides fall to a low toe at the corner."""
    px = 64
    img, d = canvas(px)
    rng = random.Random(509 + w_conn * 131 + e_conn * 47 + variant * 71)
    sky = []
    if w_conn:
        sky += [(0, PEAK_EDGE_TOP), (5, PEAK_EDGE_TOP - rng.randrange(0, 4))]
    else:
        sky += [(0, 59 + rng.randrange(0, 3)), (7, 48 + rng.randrange(0, 6))]
    first_cap = len(sky)
    if variant == 0:
        sky += [
            (17 + rng.randrange(-3, 4), 13 + rng.randrange(0, 6)),
            (31 + rng.randrange(-2, 3), 32 + rng.randrange(0, 5)),
            (45 + rng.randrange(-3, 4), 9 + rng.randrange(0, 6)),
        ]
    else:
        sky += [
            (23 + rng.randrange(-4, 5), 20 + rng.randrange(0, 5)),
            (35 + rng.randrange(-2, 3), 30 + rng.randrange(0, 4)),
            (46 + rng.randrange(-3, 4), 7 + rng.randrange(0, 5)),
        ]
    caps = [first_cap, first_cap + 2]
    if e_conn:
        sky += [(59, PEAK_EDGE_TOP - rng.randrange(0, 4)), (64, PEAK_EDGE_TOP)]
    else:
        sky += [(57, 48 + rng.randrange(0, 6)), (64, 59 + rng.randrange(0, 3))]
    _peak_mass(d, rng, sky, caps)
    finish(img, px, f"peak_sky_{w_conn}{e_conn}_{variant}")


def peak_lone(variant: int) -> None:
    """A single standing peak, feet inset so the ground shows at the
    corners instead of a hard square cut."""
    px = 64
    img, d = canvas(px)
    rng = random.Random(823 + variant * 71)
    foot_l = 5 + rng.randrange(0, 4)
    foot_r = 57 + rng.randrange(0, 4)
    if variant == 0:
        sky = [
            (foot_l, 62),
            (24 + rng.randrange(-2, 3), 15 + rng.randrange(0, 5)),
            (36, 34 + rng.randrange(0, 4)),
            (46 + rng.randrange(-2, 3), 24 + rng.randrange(0, 4)),
            (foot_r, 62),
        ]
        caps = [1, 3]
    else:
        sky = [
            (foot_l, 62),
            (30 + rng.randrange(-3, 4), 12 + rng.randrange(0, 5)),
            (foot_r, 62),
        ]
        caps = [1]
    _peak_mass(d, rng, sky, caps)
    finish(img, px, f"peak_lone_{variant}")


def peak_body(variant: int) -> None:
    """Interior of a mountain wall: solid high rock filling the tile,
    textured but calm — the skyline row above carries the drama. Edges
    stay uniform so any two body tiles join seamlessly."""
    px = 64
    base = _mix(PEAK_DARK, PEAK, 0.35)
    img, d = canvas(px, color=(*base, 255))
    rng = random.Random(977 + variant * 71)
    for _ in range(64):
        x, y = rng.randrange(0, 64), rng.randrange(0, 64)
        tone = _mix(PEAK_DARK, PEAK, rng.random() * 0.55)
        d.rectangle([s(x), s(y), s(x + 1), s(y + 1)], fill=(*tone, 255))
    # Short ridge spines and shadow pockets, kept off the edges so the
    # texture never betrays the tile grid.
    for _ in range(2 + variant):
        x0 = rng.randrange(8, 56)
        y0 = rng.randrange(4, 18)
        y1 = rng.randrange(44, 58)
        drift = rng.randrange(-12, 13)
        d.line(
            [(s(x0), s(y0)), (s(x0 + drift), s(y1))],
            fill=(*_mix(base, PEAK_LIGHT, 0.3), 255),
            width=SS,
        )
    for _ in range(3):
        x, y = rng.randrange(4, 46), rng.randrange(4, 48)
        wd, ht = rng.randrange(6, 14), rng.randrange(5, 10)
        d.polygon(
            [(s(x), s(y + ht)), (s(x + wd * 0.5), s(y)), (s(x + wd), s(y + ht))],
            fill=(*_mix(base, PEAK_DARK, 0.55), 255),
        )
    finish(img, px, f"peak_body_{variant}")


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


def harvester(faction: str, dig: int = 0) -> None:
    """The hauler; `dig` (0-2) sinks the scoop for the working cycle —
    frame 0 is the travel pose and the atlas name every existing lookup
    uses, frames 1-2 land as `_scoop1`/`_scoop2`."""
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
    # Scoop out front (up = forward); digging drops and narrows it, as
    # if biting into the ground plane.
    dy = (0, 4, 7)[dig]
    pinch = (0, 1, 2)[dig]
    d.polygon(
        [
            (s(18 + pinch), s(16 + dy)),
            (s(46 - pinch), s(16 + dy)),
            (s(40 - pinch), s(6 + dy)),
            (s(24 + pinch), s(6 + dy)),
        ],
        fill=(*IRON_LIGHT, 255),
    )
    d.polygon(
        [
            (s(21 + pinch), s(15 + dy)),
            (s(43 - pinch), s(15 + dy)),
            (s(38 - pinch), s(8 + dy)),
            (s(26 + pinch), s(8 + dy)),
        ],
        fill=(*IRON_DARK, 255),
    )
    # Spoil spray beside a digging scoop.
    if dig:
        for i, (dx, sy) in enumerate([(-3, 10), (3, 8), (-2, 5), (4, 12)]):
            if dig == 1 and i % 2:
                continue
            cx = 32 + dx * 3
            d.ellipse(
                [s(cx - 1), s(sy - 1), s(cx + 1), s(sy + 1)],
                fill=(*SCRAP_DARK, 255),
            )
    # Cargo eye — the shell can read "carrying" at a glance someday.
    d.ellipse([s(28), s(34), s(36), s(42)], fill=(*SCRAP_DARK, 255))
    suffix = ("", "_scoop1", "_scoop2")[dig]
    finish(img, px, f"harvester_{faction}{suffix}")


def scaffold(dense: bool) -> None:
    """Construction lattice drawn over a translucent rising site: dense
    early, sparse as the building nears completion."""
    px = 64
    img, d = canvas(px)
    beam = (*IRON_LIGHT, 255)
    dark = (*IRON_DARK, 255)
    for x in (4, 56):
        d.rectangle([s(x), s(4), s(x + 4), s(60)], fill=dark)
    for y in (4, 56):
        d.rectangle([s(4), s(y), s(60), s(y + 4)], fill=dark)
    step = 13 if dense else 26
    for y in range(8, 54, step):
        d.line([(s(6), s(y)), (s(58), s(y + 11))], fill=beam, width=s(2))
        d.line([(s(58), s(y)), (s(6), s(y + 11))], fill=beam, width=s(2))
    finish(img, px, "scaffold_dense" if dense else "scaffold_sparse")


def debris(variant: int) -> None:
    """A torn hull shard for death scatter."""
    px = 32
    img, d = canvas(px)
    shapes = [
        [(6, 10), (18, 4), (26, 14), (14, 24)],
        [(4, 18), (14, 6), (28, 10), (22, 26), (8, 28)],
        [(8, 8), (24, 6), (28, 20), (12, 26)],
    ]
    pts = [(s(x), s(y)) for x, y in shapes[variant]]
    d.polygon(pts, fill=(*IRON, 255))
    d.line([*pts[:2]], fill=(*IRON_LIGHT, 255), width=s(2))
    d.line([*pts[-2:]], fill=(*IRON_DARK, 255), width=s(2))
    finish(img, px, f"debris_{variant}")


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


def turret(faction: str) -> None:
    """1x1 static defense: a broad bolted base under a swivel gun. Reads
    as furniture, not a unit — no legs, no treads."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    # Foundation slab with corner bolts.
    d.rounded_rectangle([s(6), s(6), s(58), s(58)], radius=s(8), fill=(*IRON_DARK, 255))
    d.rounded_rectangle([s(10), s(10), s(54), s(54)], radius=s(6), fill=(*IRON, 255))
    for bx, by in ((13, 13), (51, 13), (13, 51), (51, 51)):
        d.ellipse([s(bx - 3), s(by - 3), s(bx + 3), s(by + 3)], fill=(*IRON_DARK, 255))
    # Rotor ring.
    d.ellipse([s(16), s(16), s(48), s(48)], fill=(*pal["dark"], 255))
    d.ellipse([s(20), s(20), s(44), s(44)], fill=(*pal["base"], 255))
    # The gun lives on a separate sprite so the mount can actually
    # track its victim; the base ships bare.
    finish(img, px, f"turret_{faction}")


def turret_barrel(faction: str) -> None:
    """The turret's gun, authored pointing up with its pivot at the
    canvas center — the renderer rotates it onto the last victim."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    d.rectangle([s(28), s(4), s(36), s(32)], fill=(*IRON_DARK, 255))
    d.rectangle([s(30), s(4), s(34), s(30)], fill=(*IRON_LIGHT, 255))
    d.ellipse([s(26), s(26), s(38), s(38)], fill=(*pal["light"], 255))
    d.ellipse([s(29), s(29), s(35), s(35)], fill=(*IRON_DARK, 255))
    finish(img, px, f"turret_barrel_{faction}")


def fabricator(faction: str) -> None:
    """2x2 second factory: an industrial gantry hall — long assembly bays
    instead of the Foundry's melt pool."""
    px = 128
    pal = FACTIONS[faction]
    img, d = canvas(px)
    d.rounded_rectangle([s(6), s(10), s(122), s(118)], radius=s(9), fill=(*IRON_DARK, 255))
    d.rounded_rectangle([s(12), s(16), s(116), s(112)], radius=s(7), fill=(*IRON, 255))
    # Sawtooth roof: four slanted skylight bands.
    for i in range(4):
        x0 = 16 + i * 25
        d.polygon(
            [(s(x0), s(24)), (s(x0 + 18), s(24)), (s(x0 + 18), s(44)), (s(x0), s(36))],
            fill=(*pal["dark"], 255),
        )
        d.polygon(
            [(s(x0), s(24)), (s(x0 + 18), s(24)), (s(x0 + 18), s(30)), (s(x0), s(28))],
            fill=(*pal["light"], 255),
        )
    # Twin assembly bays with door stripes at the bottom edge.
    for bx in (20, 68):
        d.rounded_rectangle([s(bx), s(56), s(bx + 40), s(104)], radius=s(4), fill=(*pal["base"], 255))
        d.rounded_rectangle([s(bx + 5), s(61), s(bx + 35), s(99)], radius=s(3), fill=(*IRON_DARK, 255))
        for stripe in range(3):
            d.rectangle(
                [s(bx + 7), s(92 - stripe * 9), s(bx + 33), s(95 - stripe * 9)],
                fill=(*IRON_LIGHT, 255),
            )
    # Gantry crane spanning the bays.
    d.rectangle([s(14), s(48), s(114), s(54)], fill=(*IRON_DARK, 255))
    d.rectangle([s(58), s(46), s(70), s(56)], fill=(*pal["light"], 255))
    finish(img, px, f"fabricator_{faction}")


def scuttler(faction: str) -> None:
    """Low, wide, and mean: a six-legged shredder that reads as vermin
    next to the Sentinel's arrowhead."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    # Legs splay from under the carapace, three per side.
    for side in (-1, 1):
        for i, ly in enumerate((24, 34, 44)):
            x0 = 32 + side * 12
            x1 = 32 + side * (24 + 2 * i)
            d.line([(s(x0), s(ly)), (s(x1), s(ly + 6))], fill=(*IRON_DARK, 255), width=s(3))
    # Carapace: a squat oval, wider than tall.
    d.ellipse([s(12), s(18), s(52), s(50)], fill=(*IRON, 255))
    d.ellipse([s(16), s(21), s(48), s(46)], fill=(*pal["base"], 255))
    d.ellipse([s(23), s(27), s(41), s(41)], fill=(*pal["dark"], 255))
    # Cutter jaws, forward.
    d.polygon([(s(24), s(20)), (s(30), s(8)), (s(32), s(18))], fill=(*IRON_LIGHT, 255))
    d.polygon([(s(40), s(20)), (s(34), s(8)), (s(32), s(18))], fill=(*IRON_LIGHT, 255))
    # A single hungry eye.
    d.ellipse([s(29), s(24), s(35), s(30)], fill=(*pal["light"], 255))
    finish(img, px, f"scuttler_{faction}")


def lancer(faction: str) -> None:
    """Artillery on legs: a narrow chassis dwarfed by its rail — the
    barrel is the silhouette."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    # Braced stance: four stubby stabilizer feet.
    for (fx, fy) in ((16, 34), (48, 34), (20, 52), (44, 52)):
        d.ellipse([s(fx - 4), s(fy - 4), s(fx + 4), s(fy + 4)], fill=(*IRON_DARK, 255))
    # Compact hull sitting low and back.
    d.rounded_rectangle([s(20), s(30), s(44), s(56)], radius=s(5), fill=(*IRON, 255))
    d.rounded_rectangle([s(23), s(33), s(41), s(53)], radius=s(4), fill=(*pal["base"], 255))
    d.rounded_rectangle([s(27), s(40), s(37), s(50)], radius=s(3), fill=(*pal["dark"], 255))
    # The rail: long, thin, unmistakable, reaching well past the hull.
    d.rectangle([s(28), s(0), s(36), s(34)], fill=(*IRON_DARK, 255))
    d.rectangle([s(30), s(0), s(34), s(32)], fill=(*IRON_LIGHT, 255))
    d.rectangle([s(31), s(0), s(33), s(30)], fill=(*pal["light"], 255))
    # Recoil shrouds flanking the rail base.
    d.rectangle([s(24), s(26), s(28), s(38)], fill=(*IRON_DARK, 255))
    d.rectangle([s(36), s(26), s(40), s(38)], fill=(*IRON_DARK, 255))
    finish(img, px, f"lancer_{faction}")


def bombard(faction: str) -> None:
    """Heavy siege mortar: a broad braced platform under one fat, short
    tube — the anti-silhouette of the Lancer's needle rail."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    # Recoil spades splayed at the rear corners.
    for sx in (14, 50):
        d.polygon(
            [(s(sx), s(46)), (s(sx - 6 if sx < 32 else sx + 6), s(58)), (s(sx + 4 if sx < 32 else sx - 4), s(56))],
            fill=(*IRON_DARK, 255),
        )
    # Wide low hull.
    d.rounded_rectangle([s(14), s(26), s(50), s(56)], radius=s(6), fill=(*IRON, 255))
    d.rounded_rectangle([s(18), s(30), s(46), s(52)], radius=s(5), fill=(*pal["base"], 255))
    # Base ring for the tube.
    d.ellipse([s(20), s(18), s(44), s(42)], fill=(*IRON_DARK, 255))
    d.ellipse([s(24), s(22), s(40), s(38)], fill=(*pal["dark"], 255))
    # The mortar tube: short, fat, forward, with a gaping muzzle.
    d.rectangle([s(26), s(6), s(38), s(30)], fill=(*IRON_DARK, 255))
    d.rectangle([s(28), s(6), s(36), s(28)], fill=(*IRON_LIGHT, 255))
    d.ellipse([s(25), s(2), s(39), s(14)], fill=(*IRON_DARK, 255))
    d.ellipse([s(28), s(5), s(36), s(11)], fill=(*pal["light"], 255))
    finish(img, px, f"bombard_{faction}")


def flakhound(faction: str) -> None:
    """Ferrous-pattern anti-air crawler: a fat tracked slab carrying a
    quad flak battery — four skyward muzzles read as four rings."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    # Broad treads.
    d.rounded_rectangle([s(10), s(12), s(22), s(54)], radius=s(4), fill=(*IRON_DARK, 255))
    d.rounded_rectangle([s(42), s(12), s(54), s(54)], radius=s(4), fill=(*IRON_DARK, 255))
    for y in range(16, 52, 6):
        d.rectangle([s(11), s(y), s(21), s(y + 2)], fill=(*IRON, 255))
        d.rectangle([s(43), s(y), s(53), s(y + 2)], fill=(*IRON, 255))
    # Armored hull.
    d.rounded_rectangle([s(18), s(14), s(46), s(54)], radius=s(6), fill=(*IRON, 255))
    d.rounded_rectangle([s(21), s(17), s(43), s(51)], radius=s(5), fill=(*pal["base"], 255))
    # Quad flak battery: four upward muzzles.
    for cx, cy in ((26, 26), (38, 26), (26, 40), (38, 40)):
        d.ellipse([s(cx - 5), s(cy - 5), s(cx + 5), s(cy + 5)], fill=(*IRON_DARK, 255))
        d.ellipse([s(cx - 3), s(cy - 3), s(cx + 3), s(cy + 3)], fill=(*IRON_LIGHT, 255))
        d.ellipse([s(cx - 1), s(cy - 1), s(cx + 1), s(cy + 1)], fill=(*pal["light"], 255))
    finish(img, px, f"flakhound_{faction}")


def stinger(faction: str) -> None:
    """Cupric-pattern anti-air skiff: light chassis under a three-rocket
    rack — cheap, quick, and pointing at the sky."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    # Three splayed wheel-legs.
    for (x0, y0, x1, y1) in ((24, 40, 14, 52), (40, 40, 50, 52), (32, 44, 32, 58)):
        d.line([(s(x0), s(y0)), (s(x1), s(y1))], fill=(*IRON_DARK, 255), width=s(3))
    # Slim triangular chassis.
    d.polygon([(s(32), s(14)), (s(46), s(46)), (s(18), s(46))], fill=(*IRON, 255))
    d.polygon([(s(32), s(20)), (s(42), s(43)), (s(22), s(43))], fill=(*pal["base"], 255))
    # Rocket rack: three tubes seen end-on, stacked forward.
    for i, cy in enumerate((22, 30, 38)):
        tip = pal["light"] if i == 0 else IRON_LIGHT
        d.ellipse([s(28), s(cy - 3), s(36), s(cy + 5)], fill=(*IRON_DARK, 255))
        d.ellipse([s(30), s(cy - 1), s(34), s(cy + 3)], fill=(*tip, 255))
    finish(img, px, f"stinger_{faction}")


def buzzard(faction: str) -> None:
    """Ferrous-pattern ground-attack flyer: a heavy delta wing with twin
    engine pods — slow, blunt, loaded."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    # Delta wing.
    d.polygon([(s(32), s(4)), (s(58), s(50)), (s(6), s(50))], fill=(*IRON, 255))
    d.polygon([(s(32), s(12)), (s(52), s(46)), (s(12), s(46))], fill=(*pal["base"], 255))
    d.polygon([(s(32), s(24)), (s(44), s(43)), (s(20), s(43))], fill=(*pal["dark"], 255))
    # Engine pods at the trailing corners.
    for cx in (16, 48):
        d.rounded_rectangle([s(cx - 5), s(40), s(cx + 5), s(58)], radius=s(4), fill=(*IRON_DARK, 255))
        d.ellipse([s(cx - 3), s(52), s(cx + 3), s(58)], fill=(*pal["light"], 255))
    # Chin cannon along the nose.
    d.rectangle([s(30), s(8), s(34), s(30)], fill=(*IRON_DARK, 255))
    d.ellipse([s(29), s(26), s(35), s(32)], fill=(*IRON_LIGHT, 255))
    finish(img, px, f"buzzard_{faction}")


def darter(faction: str) -> None:
    """Cupric-pattern strafer: a slim swept dart, all speed and spite."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    # Needle fuselage.
    d.polygon([(s(32), s(2)), (s(38), s(34)), (s(32), s(56)), (s(26), s(34))], fill=(*IRON, 255))
    d.polygon([(s(32), s(8)), (s(36), s(33)), (s(32), s(50)), (s(28), s(33))], fill=(*pal["base"], 255))
    # Swept blades.
    d.polygon([(s(30), s(26)), (s(8), s(44)), (s(28), s(38))], fill=(*pal["dark"], 255))
    d.polygon([(s(34), s(26)), (s(56), s(44)), (s(36), s(38))], fill=(*pal["dark"], 255))
    # Tail vanes.
    d.polygon([(s(30), s(48)), (s(20), s(60)), (s(31), s(54))], fill=(*IRON_DARK, 255))
    d.polygon([(s(34), s(48)), (s(44), s(60)), (s(33), s(54))], fill=(*IRON_DARK, 255))
    # Cockpit eye.
    d.ellipse([s(29), s(16), s(35), s(24)], fill=(*pal["light"], 255))
    finish(img, px, f"darter_{faction}")


def talon(faction: str) -> None:
    """Ferrous-pattern air-superiority fighter: cruciform, canarded, a
    hunter of other wings."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    # Main wings, straight and wide.
    d.polygon([(s(32), s(20)), (s(60), s(36)), (s(56), s(42)), (s(32), s(34))], fill=(*pal["dark"], 255))
    d.polygon([(s(32), s(20)), (s(4), s(36)), (s(8), s(42)), (s(32), s(34))], fill=(*pal["dark"], 255))
    # Canards near the nose.
    d.polygon([(s(32), s(10)), (s(46), s(18)), (s(32), s(20))], fill=(*IRON, 255))
    d.polygon([(s(32), s(10)), (s(18), s(18)), (s(32), s(20))], fill=(*IRON, 255))
    # Fuselage.
    d.polygon([(s(32), s(2)), (s(37), s(30)), (s(35), s(58)), (s(29), s(58)), (s(27), s(30))], fill=(*IRON, 255))
    d.polygon([(s(32), s(8)), (s(35), s(30)), (s(34), s(52)), (s(30), s(52)), (s(29), s(30))], fill=(*pal["base"], 255))
    # Twin tail.
    d.polygon([(s(29), s(50)), (s(20), s(62)), (s(30), s(56))], fill=(*pal["dark"], 255))
    d.polygon([(s(35), s(50)), (s(44), s(62)), (s(34), s(56))], fill=(*pal["dark"], 255))
    d.ellipse([s(29), s(14), s(35), s(22)], fill=(*pal["light"], 255))
    finish(img, px, f"talon_{faction}")


def wisp(faction: str) -> None:
    """Cupric-pattern swarm wing: a tiny pod on stub wings — one is a
    joke, a dozen are a problem."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    # Stub wings.
    d.polygon([(s(30), s(28)), (s(12), s(38)), (s(28), s(40))], fill=(*pal["dark"], 255))
    d.polygon([(s(34), s(28)), (s(52), s(38)), (s(36), s(40))], fill=(*pal["dark"], 255))
    # Round pod body.
    d.ellipse([s(22), s(16), s(42), s(44)], fill=(*IRON, 255))
    d.ellipse([s(25), s(19), s(39), s(41)], fill=(*pal["base"], 255))
    # Single rotor ring hint on top.
    d.ellipse([s(27), s(21), s(37), s(31)], fill=(*pal["light"], 255))
    d.ellipse([s(30), s(24), s(34), s(28)], fill=(*IRON_DARK, 255))
    # Tail needle.
    d.rectangle([s(31), s(42), s(33), s(54)], fill=(*IRON_DARK, 255))
    finish(img, px, f"wisp_{faction}")


def flak_turret(faction: str) -> None:
    """1x1 anti-air emplacement: the ground turret's slab, but the gun is
    a quad battery aimed at the ceiling — no forward barrel at all."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    d.rounded_rectangle([s(6), s(6), s(58), s(58)], radius=s(8), fill=(*IRON_DARK, 255))
    d.rounded_rectangle([s(10), s(10), s(54), s(54)], radius=s(6), fill=(*IRON, 255))
    for bx, by in ((13, 13), (51, 13), (13, 51), (51, 51)):
        d.ellipse([s(bx - 3), s(by - 3), s(bx + 3), s(by + 3)], fill=(*IRON_DARK, 255))
    d.ellipse([s(14), s(14), s(50), s(50)], fill=(*pal["dark"], 255))
    d.ellipse([s(18), s(18), s(46), s(46)], fill=(*pal["base"], 255))
    # Quad skyward muzzles.
    for cx, cy in ((25, 25), (39, 25), (25, 39), (39, 39)):
        d.ellipse([s(cx - 6), s(cy - 6), s(cx + 6), s(cy + 6)], fill=(*IRON_DARK, 255))
        d.ellipse([s(cx - 4), s(cy - 4), s(cx + 4), s(cy + 4)], fill=(*IRON_LIGHT, 255))
        d.ellipse([s(cx - 1.5), s(cy - 1.5), s(cx + 1.5), s(cy + 1.5)], fill=(*pal["light"], 255))
    finish(img, px, f"flak_turret_{faction}")


def bastion(faction: str) -> None:
    """2x2 artillery emplacement: a fortress ring around one enormous
    mortar throat — the gun that shells what it cannot see."""
    px = 128
    pal = FACTIONS[faction]
    img, d = canvas(px)
    # Octagonal rampart.
    oct_pts = [(40, 8), (88, 8), (120, 40), (120, 88), (88, 120), (40, 120), (8, 88), (8, 40)]
    d.polygon([(s(x), s(y)) for x, y in oct_pts], fill=(*IRON_DARK, 255))
    inner = [(46, 16), (82, 16), (112, 46), (112, 82), (82, 112), (46, 112), (16, 82), (16, 46)]
    d.polygon([(s(x), s(y)) for x, y in inner], fill=(*IRON, 255))
    # Faction rampart wedges.
    d.polygon([(s(46), s(16)), (s(82), s(16)), (s(64), s(40))], fill=(*pal["dark"], 255))
    d.polygon([(s(46), s(112)), (s(82), s(112)), (s(64), s(88))], fill=(*pal["dark"], 255))
    # The pit and the throat.
    d.ellipse([s(28), s(28), s(100), s(100)], fill=(*pal["base"], 255))
    d.ellipse([s(38), s(38), s(90), s(90)], fill=(*IRON_DARK, 255))
    d.ellipse([s(46), s(46), s(82), s(82)], fill=(*IRON_LIGHT, 255))
    d.ellipse([s(54), s(54), s(74), s(74)], fill=(*pal["dark"], 255))
    d.ellipse([s(60), s(60), s(68), s(68)], fill=(12, 10, 10, 255))
    # Shell racks on two corners.
    for cx, cy in ((26, 100), (100, 26)):
        for i in range(3):
            d.ellipse(
                [s(cx - 8 + i * 7), s(cy - 3), s(cx - 2 + i * 7), s(cy + 3)],
                fill=(*SCRAP_DARK, 255),
            )
    finish(img, px, f"bastion_{faction}")


def array(faction: str) -> None:
    """1x1 radar mast: a lattice tower under a wide dish — the eyes that
    make long guns matter."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    d.rounded_rectangle([s(10), s(10), s(54), s(54)], radius=s(7), fill=(*IRON_DARK, 255))
    d.rounded_rectangle([s(14), s(14), s(50), s(50)], radius=s(5), fill=(*IRON, 255))
    # Lattice cross-braces.
    for (x0, y0, x1, y1) in ((18, 18, 46, 46), (46, 18, 18, 46)):
        d.line([(s(x0), s(y0)), (s(x1), s(y1))], fill=(*IRON_DARK, 255), width=s(2))
    # The dish, slightly off-center as if mid-sweep.
    d.ellipse([s(16), s(14), s(52), s(50)], fill=(*pal["dark"], 255))
    d.ellipse([s(20), s(18), s(48), s(46)], fill=(*pal["base"], 255))
    d.ellipse([s(24), s(22), s(44), s(42)], fill=(*pal["dark"], 255))
    d.arc([s(20), s(18), s(48), s(46)], 300, 60, fill=(*pal["light"], 255), width=s(2))
    # Feed horn and its shadow.
    d.line([(s(34), s(32)), (s(44), s(22))], fill=(*IRON_LIGHT, 255), width=s(2))
    d.ellipse([s(31), s(29), s(37), s(35)], fill=(*BONE, 255))
    finish(img, px, f"array_{faction}")


def reclaimer(faction: str) -> None:
    """1x1 debris grinder: hopper, drum, and a chute stained amber by
    everything it has ever eaten."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    d.rounded_rectangle([s(6), s(8), s(58), s(56)], radius=s(7), fill=(*IRON_DARK, 255))
    d.rounded_rectangle([s(10), s(12), s(54), s(52)], radius=s(5), fill=(*IRON, 255))
    # Intake hopper: a funnel mouth at the top.
    d.polygon([(s(14), s(12)), (s(50), s(12)), (s(42), s(28)), (s(22), s(28))], fill=(*pal["dark"], 255))
    d.polygon([(s(18), s(14)), (s(46), s(14)), (s(40), s(24)), (s(24), s(24))], fill=(12, 10, 10, 255))
    # Grinder drum with teeth.
    d.ellipse([s(18), s(26), s(46), s(48)], fill=(*pal["base"], 255))
    for i in range(6):
        ang = i / 6 * 6.28318
        cx, cy = 32 + 10 * math.cos(ang), 37 + 9 * math.sin(ang)
        d.ellipse([s(cx - 2), s(cy - 2), s(cx + 2), s(cy + 2)], fill=(*IRON_LIGHT, 255))
    d.ellipse([s(27), s(32), s(37), s(42)], fill=(*pal["dark"], 255))
    # Output chute, amber-stained.
    d.rectangle([s(24), s(48), s(40), s(58)], fill=(*IRON_DARK, 255))
    d.rectangle([s(27), s(50), s(37), s(56)], fill=(*SCRAP_DARK, 255))
    d.rectangle([s(30), s(52), s(34), s(56)], fill=(*SCRAP, 255))
    finish(img, px, f"reclaimer_{faction}")


def wreck_pile() -> None:
    """Battlefield salvage on open ground: like a scrap heap but in dead
    machine tones with only a few amber glints — walkable junk, not a
    node."""
    px = 64
    img, d = canvas(px)
    rng = random.Random(53)
    d.ellipse([s(14), s(28), s(50), s(46)], fill=(24, 20, 16, 110))
    for _ in range(11):
        angle = rng.uniform(0, 6.28318)
        dist = 11 * rng.random() ** 0.6
        cx = 32 + dist * math.cos(angle)
        cy = 35 + dist * math.sin(angle) * 0.6 - 2 * (1 - dist / 11)
        w = rng.uniform(5, 10)
        h = rng.uniform(4, 7)
        tone = rng.choice([(58, 58, 68), (44, 44, 52), (86, 64, 40), (72, 72, 84)])
        dx = rng.uniform(-2, 2)
        d.polygon(
            [
                (s(cx - w / 2 - dx), s(cy - h / 2)),
                (s(cx + w / 2), s(cy - h / 2 + dx / 2)),
                (s(cx + w / 2 + dx), s(cy + h / 2)),
                (s(cx - w / 2), s(cy + h / 2 - dx / 2)),
            ],
            fill=(*tone, 255),
        )
    for _ in range(3):
        cx, cy = 32 + rng.uniform(-6, 6), 34 + rng.uniform(-4, 4)
        d.ellipse([s(cx - 1.5), s(cy - 1.5), s(cx + 1.5), s(cy + 1.5)], fill=(*SCRAP, 255))
    finish(img, px, "wreck_pile")


def air_shadow() -> None:
    """The soft blob a flyer casts on the ground — drawn separately and
    offset by the shell so altitude reads at a glance."""
    px = 64
    img, d = canvas(px)
    for r, alpha in [(22, 40), (16, 60), (10, 80)]:
        d.ellipse([s(32 - r), s(36 - r * 0.6), s(32 + r), s(36 + r * 0.6)], fill=(8, 8, 12, alpha))
    finish(img, px, "air_shadow")


def burst() -> None:
    """One frame of splash detonation: bright core, hot ring — the shell
    scales and fades it over the effect's life."""
    px = 64
    img, d = canvas(px)
    d.ellipse([s(10), s(10), s(54), s(54)], fill=(255, 200, 120, 60))
    d.ellipse([s(16), s(16), s(48), s(48)], outline=(255, 220, 150, 200), width=s(3))
    d.ellipse([s(24), s(24), s(40), s(40)], fill=(255, 240, 200, 200))
    d.ellipse([s(28), s(28), s(36), s(36)], fill=(255, 255, 240, 255))
    finish(img, px, "burst")


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
    for v in range(2):
        peak_body(v)
        peak_lone(v)
        for w in (0, 1):
            for e in (0, 1):
                peak_sky(w, e, v)
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
    wreck_pile()
    air_shadow()
    burst()
    scaffold(dense=True)
    scaffold(dense=False)
    for variant in range(3):
        debris(variant)
    for faction in FACTIONS:
        foundry(faction)
        harvester(faction)
        harvester(faction, dig=1)
        harvester(faction, dig=2)
        sentinel(faction)
        scuttler(faction)
        lancer(faction)
        bombard(faction)
        flakhound(faction)
        stinger(faction)
        buzzard(faction)
        darter(faction)
        talon(faction)
        wisp(faction)
        turret(faction)
        turret_barrel(faction)
        fabricator(faction)
        flak_turret(faction)
        bastion(faction)
        array(faction)
        reclaimer(faction)
    pack_atlas()
    print("done")


if __name__ == "__main__":
    main()
