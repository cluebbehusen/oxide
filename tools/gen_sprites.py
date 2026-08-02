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
import tempfile
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw

OUT = Path(__file__).resolve().parent.parent / "assets" / "sprites"
SS = 4  # supersample factor

# Every finished sprite lands here too, so main() can pack one atlas the
# shell renders from — one texture means one GPU batch for the whole world.
REGISTRY: dict[str, Image.Image] = {}

# The Oxide palette. Keep in sync with kit/src/render.rs and the shell.
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
    shifted = ImageChops.subtract(
        edge, edge.transform(edge.size, Image.AFFINE, (1, 0, -1, 0, 1, -1))
    )
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
    # Animation rows add many complete 2x2 frames. A wider shelf keeps the
    # deterministic atlas comfortably below common 8192px texture limits.
    atlas_w = 2048
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


def _install_finalized_sprite_bank() -> None:
    """Installs approved frames into this generator's live registry."""
    import sys

    repo_root = str(Path(__file__).resolve().parent.parent)
    if repo_root not in sys.path:
        sys.path.insert(0, repo_root)
    sys.modules["tools.gen_sprites"] = sys.modules[__name__]

    from tools.production_sprite_sources.finalized import install_finalized_sprites

    install_finalized_sprites(REGISTRY, OUT)


def _install_finalized_construction_bank() -> None:
    """Installs the approved full-hull site frames into the live registry."""
    from tools.production_sprite_sources.construction_final import (
        install_finalized_construction,
    )

    install_finalized_construction(REGISTRY, OUT)


def accent_masks() -> None:
    """Derives one allegiance-accent mask per faction-varied sprite.

    Factions share silhouettes and differ only in accent color, so the
    pixels where a sprite's two faction variants differ ARE the
    faction-colored regions — exactly the region an owner tint should
    cover (the RTS team-color mask, derived instead of hand-painted).
    The mask is luminance-preserving grayscale: the shell multiplies it
    by an allegiance hue, keeping the original shading. Rim light and
    chassis grays cancel in the diff and stay untinted.
    """
    from PIL import ImageChops

    for name in [n for n in sorted(REGISTRY) if "_ferrous" in n]:
        fer = REGISTRY[name]
        cup = REGISTRY[name.replace("_ferrous", "_cupric")]
        diff = ImageChops.difference(fer.convert("RGB"), cup.convert("RGB"))
        r, g, b = diff.split()
        weight = ImageChops.lighter(ImageChops.lighter(r, g), b).point(
            lambda v: min(255, v * 3)
        )
        alpha = ImageChops.multiply(fer.split()[3], weight)
        # Ferrous base luminance is ~116/255; x1.7 restores full-tint
        # brightness to the palette midpoint without clipping "light".
        lum = fer.convert("L").point(lambda v: min(255, round(v * 1.7)))
        mask = Image.merge("RGBA", (lum, lum, lum, alpha))
        out_name = name.replace("_ferrous", "_accent")
        mask.save(OUT / f"{out_name}.png")
        REGISTRY[out_name] = mask
        print(f"  {out_name}.png")


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
        facet = [
            (s(x * 0.62 + cx * 0.38 - r * 0.12), s(y * 0.62 + cy * 0.38 - r * 0.12))
            for x, y in pts[:5]
        ]
        d.polygon(facet, fill=(*light, 255))
        shade = [
            (s(x * 0.72 + cx * 0.28 + r * 0.10), s(y * 0.72 + cy * 0.28 + r * 0.10))
            for x, y in pts[4:9]
        ]
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


def peak_barrier(mask: int, variant: int) -> None:
    """A full-tile plated exclusion barrier.

    `mask` is north/east/south/west in the low four bits. Connected edges
    open their bevel and carry the central hazard band through; exposed
    edges keep a hard bright rim. The result reads as one authored wall
    instead of a row of mountain icons, while remaining flat enough not to
    imply that units can pass behind it.
    """
    px = 64
    base = _mix(PEAK_DARK, (30, 31, 40), 0.28 + variant * 0.07)
    img, d = canvas(px, color=(*base, 255))
    rng = random.Random(977 + mask * 71 + variant * 313)

    # Broad inset plates create a mechanical mass wholly unlike the loose,
    # round rock sprites. Their shared center seam is intentionally quiet so
    # the crossed hazard bands stay dominant at play zoom.
    d.rectangle([s(4), s(4), s(60), s(60)], fill=(*_mix(base, PEAK, 0.28), 255))
    d.polygon(
        [(s(5), s(5)), (s(31), s(5)), (s(26), s(31)), (s(5), s(36))],
        fill=(*_mix(base, PEAK_LIGHT, 0.12), 255),
    )
    d.polygon(
        [(s(59), s(59)), (s(33), s(59)), (s(38), s(33)), (s(59), s(28))],
        fill=(*_mix(base, PEAK_DARK, 0.62), 255),
    )

    # Crossed industrial hatching is the terrain's gameplay signifier:
    # yellow-bone strokes over a black backing, clipped to the tile.
    hazard_dark = (25, 24, 22, 255)
    hazard = _mix(SCRAP, BONE, 0.16 + variant * 0.08)
    for offset in range(-56, 72, 16):
        d.line(
            [(s(offset), s(64)), (s(offset + 64), s(0))],
            fill=hazard_dark,
            width=s(8),
        )
        d.line(
            [(s(offset + 2), s(64)), (s(offset + 66), s(0))],
            fill=(*hazard, 238),
            width=s(3),
        )
    for offset in range(-48, 80, 24):
        d.line(
            [(s(offset), s(0)), (s(offset + 64), s(64))],
            fill=(20, 20, 24, 205),
            width=s(5),
        )
        d.line(
            [(s(offset), s(0)), (s(offset + 64), s(64))],
            fill=(*_mix(PEAK_DARK, PEAK_LIGHT, 0.48), 205),
            width=s(2),
        )

    # Exposed sides are capped; connected sides instead expose two dark
    # bridge rails so adjacent tiles join as one continuous barrier.
    edges = (
        (1, [(0, 0), (64, 0)], [(19, 0), (45, 0)]),
        (2, [(64, 0), (64, 64)], [(64, 19), (64, 45)]),
        (4, [(64, 64), (0, 64)], [(45, 64), (19, 64)]),
        (8, [(0, 64), (0, 0)], [(0, 45), (0, 19)]),
    )
    for bit, rim, bridge in edges:
        if mask & bit:
            d.line(
                [(s(x), s(y)) for x, y in bridge], fill=(17, 18, 24, 255), width=s(8)
            )
            d.line(
                [(s(x), s(y)) for x, y in bridge],
                fill=(*_mix(PEAK, PEAK_LIGHT, 0.35), 255),
                width=s(2),
            )
        else:
            d.line([(s(x), s(y)) for x, y in rim], fill=(*PEAK_DARK, 255), width=s(6))
            inset = [(min(62, max(2, x)), min(62, max(2, y))) for x, y in rim]
            d.line(
                [(s(x), s(y)) for x, y in inset], fill=(*PEAK_LIGHT, 235), width=s(2)
            )

    # Rivets and a few scuffs keep large fields from reading as one flat
    # procedural texture. Their seeded positions stay clear of the edges.
    for x, y in ((8, 8), (56, 8), (56, 56), (8, 56)):
        d.ellipse([s(x - 1.5), s(y - 1.5), s(x + 1.5), s(y + 1.5)], fill=(*BONE, 215))
    for _ in range(7):
        x, y = rng.randrange(10, 54), rng.randrange(10, 54)
        d.line(
            [(s(x), s(y)), (s(x + rng.randrange(2, 7)), s(y + rng.randrange(-2, 3)))],
            fill=(*PEAK_LIGHT, 105),
            width=SS,
        )
    finish(img, px, f"peak_barrier_{mask:02x}_{variant}")


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


def foundry(faction: str, work: int = 0) -> None:
    px = 128
    pal = FACTIONS[faction]
    img, d = canvas(px)
    # Baseplate with a bevel.
    d.rounded_rectangle(
        [s(6), s(6), s(122), s(122)], radius=s(10), fill=(*IRON_DARK, 255)
    )
    d.rounded_rectangle([s(12), s(12), s(116), s(116)], radius=s(8), fill=(*IRON, 255))
    # Faction roof panels, chevroned toward the center.
    d.polygon(
        [(s(12), s(12)), (s(64), s(12)), (s(12), s(64))], fill=(*pal["dark"], 255)
    )
    d.polygon(
        [(s(116), s(116)), (s(64), s(116)), (s(116), s(64))], fill=(*pal["dark"], 255)
    )
    d.rectangle([s(20), s(20), s(108), s(108)], fill=(*IRON, 255))
    d.rectangle([s(26), s(26), s(102), s(102)], fill=(*pal["base"], 255))
    d.rectangle([s(34), s(34), s(94), s(94)], fill=(*IRON_DARK, 255))
    # The melt pool's rings circulate through authored frames. The base
    # frame remains the quiet pose used by reduced motion and static UI.
    pool_shift = (0, 2, 0, -2)[work]
    core_shift = (0, -2, 2, 0)[work]
    d.ellipse([s(44), s(44), s(84), s(84)], fill=(*pal["base"], 255))
    d.ellipse(
        [s(50 + pool_shift), s(50), s(78 + pool_shift), s(78)],
        fill=(*pal["light"], 255),
    )
    d.ellipse(
        [
            s(57 + core_shift),
            s(57 - core_shift),
            s(71 + core_shift),
            s(71 - core_shift),
        ],
        fill=(*BONE, 255),
    )
    # Chimney stack, top-right, with a dark throat.
    d.ellipse([s(88), s(16), s(112), s(40)], fill=(*IRON_LIGHT, 255))
    d.ellipse([s(93), s(21), s(107), s(35)], fill=(*IRON_DARK, 255))
    # Rivets on the corners that have no chimney.
    for cx, cy in ((22, 22), (22, 106), (106, 106)):
        d.ellipse([s(cx - 3), s(cy - 3), s(cx + 3), s(cy + 3)], fill=(*IRON_LIGHT, 255))
    suffix = "" if work == 0 else f"_work{work}"
    finish(img, px, f"foundry_{faction}{suffix}")


def harvester(faction: str, dig: int = 0, tread: int = 0) -> None:
    """The hauler; `dig` (0-2) sinks the scoop for the working cycle —
    frame 0 is the travel pose and the atlas name every existing lookup
    uses, frames 1-2 land as `_scoop1`/`_scoop2`."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    if dig and tread:
        raise ValueError("dig and tread frames are separate animation rows")
    # Treads flanking the hull. Their cleats advance through three authored
    # phases instead of moving the whole chassis or painting dust under it.
    d.rounded_rectangle(
        [s(12), s(14), s(22), s(54)], radius=s(4), fill=(*IRON_DARK, 255)
    )
    d.rounded_rectangle(
        [s(42), s(14), s(52), s(54)], radius=s(4), fill=(*IRON_DARK, 255)
    )
    tread_phase = tread % 3
    for slot in range(5):
        y = 16 + (slot * 8 + tread_phase * 3) % 40
        cleat = IRON_LIGHT if (slot + tread_phase) % 2 == 0 else IRON
        d.rectangle([s(13), s(y), s(21), s(y + 3)], fill=(*cleat, 255))
        d.rectangle([s(43), s(y), s(51), s(y + 3)], fill=(*cleat, 255))
    # A broad colored drive pad makes the phase survive the ordinary
    # battlefield downscale; it advances front-to-rear with the cleats.
    drive_y = (17, 30, 43)[tread_phase]
    for x0, x1 in ((13, 21), (43, 51)):
        d.rounded_rectangle(
            [s(x0), s(drive_y), s(x1), s(drive_y + 6)],
            radius=s(2),
            fill=(*pal["light"], 255),
        )
    # Hull.
    d.rounded_rectangle([s(20), s(18), s(44), s(52)], radius=s(6), fill=(*IRON, 255))
    d.rounded_rectangle(
        [s(23), s(21), s(41), s(49)], radius=s(5), fill=(*pal["base"], 255)
    )
    d.rounded_rectangle(
        [s(26), s(30), s(38), s(46)], radius=s(3), fill=(*pal["dark"], 255)
    )
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
    if tread:
        suffix = f"_tread{tread}"
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


def sentinel(faction: str, move: int = 0) -> None:
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    phase = move % 3
    body_dy = (0, -1, 1)[phase]
    # Armored runner pads stay under the hull while their lit suspension
    # blocks trade load front-to-rear. The body settles against them in the
    # two travel phases, giving the otherwise enclosed chassis visible weight.
    runner_y = ((36, 36), (29, 42), (42, 29))[phase]
    for x, load_y in zip((14, 46), runner_y, strict=True):
        d.rounded_rectangle(
            [s(x), s(25), s(x + 5), s(51)], radius=s(2), fill=(*IRON_DARK, 255)
        )
        d.rounded_rectangle(
            [s(x + 1), s(load_y), s(x + 4), s(load_y + 7)],
            radius=s(1),
            fill=(*IRON_LIGHT, 255),
        )
    # Angular chassis: a blunt arrowhead pointing up.
    hull = [
        (32, 6 + body_dy),
        (50, 30 + body_dy),
        (46, 54 + body_dy),
        (18, 54 + body_dy),
        (14, 30 + body_dy),
    ]
    d.polygon([(s(x), s(y)) for x, y in hull], fill=(*IRON, 255))
    inner = [
        (32, 12 + body_dy),
        (45, 31 + body_dy),
        (42, 49 + body_dy),
        (22, 49 + body_dy),
        (19, 31 + body_dy),
    ]
    d.polygon([(s(x), s(y)) for x, y in inner], fill=(*pal["base"], 255))
    core = [
        (32, 22 + body_dy),
        (39, 33 + body_dy),
        (37, 44 + body_dy),
        (27, 44 + body_dy),
        (25, 33 + body_dy),
    ]
    d.polygon([(s(x), s(y)) for x, y in core], fill=(*pal["dark"], 255))
    # Weapon pods.
    d.ellipse([s(15), s(30 + body_dy), s(25), s(40 + body_dy)], fill=(*IRON_DARK, 255))
    d.ellipse([s(39), s(30 + body_dy), s(49), s(40 + body_dy)], fill=(*IRON_DARK, 255))
    # Barrel, forward.
    d.rectangle([s(29), s(2 + body_dy), s(35), s(20 + body_dy)], fill=(*IRON_DARK, 255))
    d.rectangle(
        [s(30.5), s(2 + body_dy), s(33.5), s(18 + body_dy)],
        fill=(*IRON_LIGHT, 255),
    )
    # Sight.
    d.ellipse(
        [s(29), s(24 + body_dy), s(35), s(30 + body_dy)], fill=(*pal["light"], 255)
    )
    suffix = "" if move == 0 else f"_move{move}"
    finish(img, px, f"sentinel_{faction}{suffix}")


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


def fabricator(faction: str, work: int = 0) -> None:
    """2x2 second factory: an industrial gantry hall — long assembly bays
    instead of the Foundry's melt pool."""
    px = 128
    pal = FACTIONS[faction]
    img, d = canvas(px)
    d.rounded_rectangle(
        [s(6), s(10), s(122), s(118)], radius=s(9), fill=(*IRON_DARK, 255)
    )
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
        d.rounded_rectangle(
            [s(bx), s(56), s(bx + 40), s(104)], radius=s(4), fill=(*pal["base"], 255)
        )
        d.rounded_rectangle(
            [s(bx + 5), s(61), s(bx + 35), s(99)], radius=s(3), fill=(*IRON_DARK, 255)
        )
        for stripe in range(3):
            d.rectangle(
                [s(bx + 7), s(92 - stripe * 9), s(bx + 33), s(95 - stripe * 9)],
                fill=(*IRON_LIGHT, 255),
            )
    # Gantry crane spanning the bays. Its carriage crosses the assembly
    # floor in authored work frames instead of a rectangle drawn by the
    # renderer over an otherwise static roof.
    d.rectangle([s(14), s(48), s(114), s(54)], fill=(*IRON_DARK, 255))
    carriage_x = (58, 34, 58, 82)[work]
    cable_end = (72, 72, 86, 72)[work]
    d.rectangle(
        [s(carriage_x), s(46), s(carriage_x + 12), s(56)],
        fill=(*pal["light"], 255),
    )
    d.rectangle(
        [s(carriage_x + 4), s(54), s(carriage_x + 8), s(cable_end)],
        fill=(*IRON_LIGHT, 255),
    )
    d.ellipse(
        [s(carriage_x + 1), s(cable_end - 3), s(carriage_x + 11), s(cable_end + 5)],
        fill=(*IRON_DARK, 255),
    )
    suffix = "" if work == 0 else f"_work{work}"
    finish(img, px, f"fabricator_{faction}{suffix}")


def scuttler(faction: str, move: int = 0) -> None:
    """Low, wide, and mean: a six-legged shredder that reads as vermin
    next to the Sentinel's arrowhead."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    phase = move % 3
    gait = ((0, 0, 0), (-4, 3, -3), (3, -4, 3))[phase]
    # Legs splay from under the carapace, three per side.
    for side in (-1, 1):
        for i, ly in enumerate((24, 34, 44)):
            x0 = 32 + side * 12
            x1 = 32 + side * (24 + 2 * i)
            step = gait[i] * side
            foot_y = ly + 6 + step
            d.line(
                [(s(x0), s(ly)), (s(x1), s(foot_y))], fill=(*IRON_DARK, 255), width=s(3)
            )
            d.ellipse(
                [s(x1 - 2), s(foot_y - 2), s(x1 + 2), s(foot_y + 2)],
                fill=(*IRON_LIGHT, 255),
            )
    # Carapace: a squat oval, wider than tall.
    d.ellipse([s(12), s(18), s(52), s(50)], fill=(*IRON, 255))
    d.ellipse([s(16), s(21), s(48), s(46)], fill=(*pal["base"], 255))
    d.ellipse([s(23), s(27), s(41), s(41)], fill=(*pal["dark"], 255))
    # Cutter jaws, forward.
    d.polygon([(s(24), s(20)), (s(30), s(8)), (s(32), s(18))], fill=(*IRON_LIGHT, 255))
    d.polygon([(s(40), s(20)), (s(34), s(8)), (s(32), s(18))], fill=(*IRON_LIGHT, 255))
    # A single hungry eye.
    d.ellipse([s(29), s(24), s(35), s(30)], fill=(*pal["light"], 255))
    suffix = "" if move == 0 else f"_move{move}"
    finish(img, px, f"scuttler_{faction}{suffix}")


def lancer(faction: str, move: int = 0) -> None:
    """Artillery on legs: a narrow chassis dwarfed by its rail — the
    barrel is the silhouette."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    phase = move % 3
    body_dx = (0, -1, 1)[phase]
    feet = (
        ((16, 34), (48, 34), (20, 52), (44, 52)),
        ((14, 30), (49, 37), (22, 55), (42, 49)),
        ((15, 37), (50, 30), (18, 49), (46, 55)),
    )[phase]
    anchors = ((23, 36), (41, 36), (24, 48), (40, 48))
    # Four stabilizer legs walk diagonally while the heavy rail shifts its
    # weight over the planted pair.
    for (fx, fy), (ax, ay) in zip(feet, anchors, strict=True):
        d.line(
            [(s(ax + body_dx), s(ay)), (s(fx), s(fy))],
            fill=(*IRON_DARK, 255),
            width=s(3),
        )
        d.ellipse([s(fx - 4), s(fy - 4), s(fx + 4), s(fy + 4)], fill=(*IRON_DARK, 255))
    # Compact hull sitting low and back.
    d.rounded_rectangle(
        [s(20 + body_dx), s(30), s(44 + body_dx), s(56)],
        radius=s(5),
        fill=(*IRON, 255),
    )
    d.rounded_rectangle(
        [s(23 + body_dx), s(33), s(41 + body_dx), s(53)],
        radius=s(4),
        fill=(*pal["base"], 255),
    )
    d.rounded_rectangle(
        [s(27 + body_dx), s(40), s(37 + body_dx), s(50)],
        radius=s(3),
        fill=(*pal["dark"], 255),
    )
    # The rail: long, thin, unmistakable, reaching well past the hull.
    d.rectangle([s(28 + body_dx), s(0), s(36 + body_dx), s(34)], fill=(*IRON_DARK, 255))
    d.rectangle(
        [s(30 + body_dx), s(0), s(34 + body_dx), s(32)], fill=(*IRON_LIGHT, 255)
    )
    d.rectangle(
        [s(31 + body_dx), s(0), s(33 + body_dx), s(30)], fill=(*pal["light"], 255)
    )
    # Recoil shrouds flanking the rail base.
    d.rectangle(
        [s(24 + body_dx), s(26), s(28 + body_dx), s(38)], fill=(*IRON_DARK, 255)
    )
    d.rectangle(
        [s(36 + body_dx), s(26), s(40 + body_dx), s(38)], fill=(*IRON_DARK, 255)
    )
    suffix = "" if move == 0 else f"_move{move}"
    finish(img, px, f"lancer_{faction}{suffix}")


def bombard(faction: str, move: int = 0) -> None:
    """Heavy siege mortar: a broad braced platform under one fat, short
    tube — the anti-silhouette of the Lancer's needle rail."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    phase = move % 3
    body_dx = (0, -1, 1)[phase]
    rear_steps = ((0, 0), (-4, 3), (3, -4))[phase]
    front_feet = (((14, 31), (50, 31)), ((12, 27), (51, 35)), ((13, 35), (52, 27)))[
        phase
    ]
    # Front walking shoes and rear recoil spades alternate as a slow,
    # four-point siege gait. The mortar body settles over the planted side.
    for (fx, fy), ax in zip(front_feet, (20, 44), strict=True):
        d.line(
            [(s(ax + body_dx), s(34)), (s(fx), s(fy))],
            fill=(*IRON_DARK, 255),
            width=s(3),
        )
        d.ellipse([s(fx - 4), s(fy - 3), s(fx + 4), s(fy + 3)], fill=(*IRON_DARK, 255))
    for side, sx in enumerate((14, 50)):
        step = rear_steps[side]
        d.polygon(
            [
                (s(sx), s(46)),
                (s(sx - 6 if sx < 32 else sx + 6), s(58 + step)),
                (s(sx + 4 if sx < 32 else sx - 4), s(56 + step)),
            ],
            fill=(*IRON_DARK, 255),
        )
    # Wide low hull.
    d.rounded_rectangle(
        [s(14 + body_dx), s(26), s(50 + body_dx), s(56)],
        radius=s(6),
        fill=(*IRON, 255),
    )
    d.rounded_rectangle(
        [s(18 + body_dx), s(30), s(46 + body_dx), s(52)],
        radius=s(5),
        fill=(*pal["base"], 255),
    )
    # Base ring for the tube.
    d.ellipse([s(20 + body_dx), s(18), s(44 + body_dx), s(42)], fill=(*IRON_DARK, 255))
    d.ellipse(
        [s(24 + body_dx), s(22), s(40 + body_dx), s(38)], fill=(*pal["dark"], 255)
    )
    # The mortar tube: short, fat, forward, with a gaping muzzle.
    d.rectangle([s(26 + body_dx), s(6), s(38 + body_dx), s(30)], fill=(*IRON_DARK, 255))
    d.rectangle(
        [s(28 + body_dx), s(6), s(36 + body_dx), s(28)], fill=(*IRON_LIGHT, 255)
    )
    d.ellipse([s(25 + body_dx), s(2), s(39 + body_dx), s(14)], fill=(*IRON_DARK, 255))
    d.ellipse(
        [s(28 + body_dx), s(5), s(36 + body_dx), s(11)], fill=(*pal["light"], 255)
    )
    suffix = "" if move == 0 else f"_move{move}"
    finish(img, px, f"bombard_{faction}{suffix}")


def flakhound(faction: str, tread: int = 0) -> None:
    """Ferrous-pattern anti-air crawler: a fat tracked slab carrying a
    quad flak battery — four skyward muzzles read as four rings."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    # Broad treads. Cleats advance inside the silhouette; the chassis stays
    # planted instead of wobbling and kicking up procedural dust.
    d.rounded_rectangle(
        [s(10), s(12), s(22), s(54)], radius=s(4), fill=(*IRON_DARK, 255)
    )
    d.rounded_rectangle(
        [s(42), s(12), s(54), s(54)], radius=s(4), fill=(*IRON_DARK, 255)
    )
    tread_phase = tread % 3
    for slot in range(5):
        y = 14 + (slot * 8 + tread_phase * 3) % 40
        cleat = IRON_LIGHT if (slot + tread_phase) % 2 == 0 else IRON
        d.rectangle([s(11), s(y), s(21), s(y + 3)], fill=(*cleat, 255))
        d.rectangle([s(43), s(y), s(53), s(y + 3)], fill=(*cleat, 255))
    drive_y = (15, 29, 43)[tread_phase]
    for x0, x1 in ((11, 21), (43, 53)):
        d.rounded_rectangle(
            [s(x0), s(drive_y), s(x1), s(drive_y + 7)],
            radius=s(2),
            fill=(*pal["light"], 255),
        )
    # Armored hull.
    d.rounded_rectangle([s(18), s(14), s(46), s(54)], radius=s(6), fill=(*IRON, 255))
    d.rounded_rectangle(
        [s(21), s(17), s(43), s(51)], radius=s(5), fill=(*pal["base"], 255)
    )
    # Quad flak battery: four upward muzzles.
    for cx, cy in ((26, 26), (38, 26), (26, 40), (38, 40)):
        d.ellipse([s(cx - 5), s(cy - 5), s(cx + 5), s(cy + 5)], fill=(*IRON_DARK, 255))
        d.ellipse([s(cx - 3), s(cy - 3), s(cx + 3), s(cy + 3)], fill=(*IRON_LIGHT, 255))
        d.ellipse(
            [s(cx - 1), s(cy - 1), s(cx + 1), s(cy + 1)], fill=(*pal["light"], 255)
        )
    suffix = "" if tread == 0 else f"_tread{tread}"
    finish(img, px, f"flakhound_{faction}{suffix}")


def stinger(faction: str, move: int = 0) -> None:
    """Cupric-pattern anti-air skiff: light chassis under a three-rocket
    rack — cheap, quick, and pointing at the sky."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    phase = move % 3
    # Three splayed wheel-legs. Each travel phase extends a different pair,
    # making the quick skiff's suspension readable after battlefield downscale.
    legs = (
        ((24, 40, 14, 52), (40, 40, 50, 52), (32, 44, 32, 58)),
        ((24, 40, 12, 49), (40, 40, 52, 54), (32, 44, 30, 59)),
        ((24, 40, 16, 54), (40, 40, 48, 49), (32, 44, 34, 59)),
    )[phase]
    for x0, y0, x1, y1 in legs:
        d.line([(s(x0), s(y0)), (s(x1), s(y1))], fill=(*IRON_DARK, 255), width=s(3))
        d.ellipse([s(x1 - 5), s(y1 - 5), s(x1 + 5), s(y1 + 5)], fill=(*IRON_DARK, 255))
        spoke = ((0, -4, 0, 4), (-3, -3, 3, 3), (-4, 0, 4, 0))[phase]
        d.line(
            [
                (s(x1 + spoke[0]), s(y1 + spoke[1])),
                (s(x1 + spoke[2]), s(y1 + spoke[3])),
            ],
            fill=(*pal["light"], 255),
            width=s(3),
        )
    # Slim triangular chassis.
    d.polygon([(s(32), s(14)), (s(46), s(46)), (s(18), s(46))], fill=(*IRON, 255))
    d.polygon(
        [(s(32), s(20)), (s(42), s(43)), (s(22), s(43))], fill=(*pal["base"], 255)
    )
    # Rocket rack: three tubes seen end-on, stacked forward.
    for i, cy in enumerate((22, 30, 38)):
        tip = pal["light"] if i == 0 else IRON_LIGHT
        d.ellipse([s(28), s(cy - 3), s(36), s(cy + 5)], fill=(*IRON_DARK, 255))
        d.ellipse([s(30), s(cy - 1), s(34), s(cy + 3)], fill=(*tip, 255))
    suffix = "" if move == 0 else f"_move{move}"
    finish(img, px, f"stinger_{faction}{suffix}")


def buzzard(faction: str) -> None:
    """Ferrous-pattern ground-attack flyer: a heavy delta wing with twin
    engine pods — slow, blunt, loaded."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    # Delta wing.
    d.polygon([(s(32), s(4)), (s(58), s(50)), (s(6), s(50))], fill=(*IRON, 255))
    d.polygon(
        [(s(32), s(12)), (s(52), s(46)), (s(12), s(46))], fill=(*pal["base"], 255)
    )
    d.polygon(
        [(s(32), s(24)), (s(44), s(43)), (s(20), s(43))], fill=(*pal["dark"], 255)
    )
    # Engine pods at the trailing corners.
    for cx in (16, 48):
        d.rounded_rectangle(
            [s(cx - 5), s(40), s(cx + 5), s(58)], radius=s(4), fill=(*IRON_DARK, 255)
        )
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
    d.polygon(
        [(s(32), s(2)), (s(38), s(34)), (s(32), s(56)), (s(26), s(34))],
        fill=(*IRON, 255),
    )
    d.polygon(
        [(s(32), s(8)), (s(36), s(33)), (s(32), s(50)), (s(28), s(33))],
        fill=(*pal["base"], 255),
    )
    # Swept blades.
    d.polygon([(s(30), s(26)), (s(8), s(44)), (s(28), s(38))], fill=(*pal["dark"], 255))
    d.polygon(
        [(s(34), s(26)), (s(56), s(44)), (s(36), s(38))], fill=(*pal["dark"], 255)
    )
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
    d.polygon(
        [(s(32), s(20)), (s(60), s(36)), (s(56), s(42)), (s(32), s(34))],
        fill=(*pal["dark"], 255),
    )
    d.polygon(
        [(s(32), s(20)), (s(4), s(36)), (s(8), s(42)), (s(32), s(34))],
        fill=(*pal["dark"], 255),
    )
    # Canards near the nose.
    d.polygon([(s(32), s(10)), (s(46), s(18)), (s(32), s(20))], fill=(*IRON, 255))
    d.polygon([(s(32), s(10)), (s(18), s(18)), (s(32), s(20))], fill=(*IRON, 255))
    # Fuselage.
    d.polygon(
        [(s(32), s(2)), (s(37), s(30)), (s(35), s(58)), (s(29), s(58)), (s(27), s(30))],
        fill=(*IRON, 255),
    )
    d.polygon(
        [(s(32), s(8)), (s(35), s(30)), (s(34), s(52)), (s(30), s(52)), (s(29), s(30))],
        fill=(*pal["base"], 255),
    )
    # Twin tail.
    d.polygon(
        [(s(29), s(50)), (s(20), s(62)), (s(30), s(56))], fill=(*pal["dark"], 255)
    )
    d.polygon(
        [(s(35), s(50)), (s(44), s(62)), (s(34), s(56))], fill=(*pal["dark"], 255)
    )
    d.ellipse([s(29), s(14), s(35), s(22)], fill=(*pal["light"], 255))
    finish(img, px, f"talon_{faction}")


def wisp(faction: str) -> None:
    """Cupric-pattern swarm wing: a tiny pod on stub wings — one is a
    joke, a dozen are a problem."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    # Stub wings.
    d.polygon(
        [(s(30), s(28)), (s(12), s(38)), (s(28), s(40))], fill=(*pal["dark"], 255)
    )
    d.polygon(
        [(s(34), s(28)), (s(52), s(38)), (s(36), s(40))], fill=(*pal["dark"], 255)
    )
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
    """1x1 anti-air foundation: a braced cruciform firing platform.

    The directional quad battery is a separate sprite so its silhouette
    can track aircraft and kick independently of the foundation.
    """
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    # Broad cardinal outriggers keep the platform distinct from the ordinary
    # Turret's square slab even when the rotating head is hidden by fog.
    outer = [
        (18, 3),
        (46, 3),
        (46, 8),
        (56, 8),
        (56, 18),
        (61, 18),
        (61, 46),
        (56, 46),
        (56, 56),
        (46, 56),
        (46, 61),
        (18, 61),
        (18, 56),
        (8, 56),
        (8, 46),
        (3, 46),
        (3, 18),
        (8, 18),
        (8, 8),
        (18, 8),
    ]
    inner = [
        (20, 8),
        (44, 8),
        (44, 13),
        (51, 13),
        (51, 20),
        (56, 20),
        (56, 44),
        (51, 44),
        (51, 51),
        (44, 51),
        (44, 56),
        (20, 56),
        (20, 51),
        (13, 51),
        (13, 44),
        (8, 44),
        (8, 20),
        (13, 20),
        (13, 13),
        (20, 13),
    ]
    d.polygon([(s(x), s(y)) for x, y in outer], fill=(*IRON_DARK, 255))
    d.polygon([(s(x), s(y)) for x, y in inner], fill=(*IRON, 255))
    # Four armored stabilizer pads and faction chevrons make the AA role read
    # as a deployed weapon rather than another circular machine.
    for x0, y0, x1, y1 in (
        (10, 10, 24, 21),
        (40, 10, 54, 21),
        (10, 43, 24, 54),
        (40, 43, 54, 54),
    ):
        d.rounded_rectangle(
            [s(x0), s(y0), s(x1), s(y1)],
            radius=s(2),
            fill=(*IRON_DARK, 255),
        )
    for points in (
        [(25, 9), (39, 9), (35, 15), (29, 15)],
        [(55, 25), (55, 39), (49, 35), (49, 29)],
        [(25, 55), (39, 55), (35, 49), (29, 49)],
        [(9, 25), (9, 39), (15, 35), (15, 29)],
    ):
        d.polygon([(s(x), s(y)) for x, y in points], fill=(*pal["dark"], 255))
    # A square traverse cradle leaves a strong H-shaped foundation beneath
    # the wider gun head.
    cradle = [
        (19, 24),
        (24, 19),
        (40, 19),
        (45, 24),
        (45, 40),
        (40, 45),
        (24, 45),
        (19, 40),
    ]
    d.polygon([(s(x), s(y)) for x, y in cradle], fill=(*pal["base"], 255))
    d.rounded_rectangle(
        [s(25), s(25), s(39), s(39)], radius=s(3), fill=(*IRON_DARK, 255)
    )
    finish(img, px, f"flak_turret_{faction}")


def flak_mount(faction: str) -> None:
    """Wide directional quad cannon with its pivot at canvas center."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    # Large feed pods and rear magazines keep the head legible when the game
    # scales the full tile down to its ordinary battlefield size.
    for x0, x1 in ((7, 28), (36, 57)):
        d.rounded_rectangle(
            [s(x0), s(20), s(x1), s(48)],
            radius=s(5),
            fill=(*IRON_DARK, 255),
        )
        d.rounded_rectangle(
            [s(x0 + 4), s(24), s(x1 - 4), s(42)],
            radius=s(3),
            fill=(*pal["base"], 255),
        )
        d.rectangle(
            [s(x0 + 5), s(30), s(x1 - 5), s(34)],
            fill=(*pal["light"], 255),
        )
        d.rounded_rectangle(
            [s(x0 + 3), s(44), s(x1 - 3), s(56)],
            radius=s(3),
            fill=(*pal["dark"], 255),
        )
    # Four parallel barrels retain the renderer's authored muzzle positions,
    # but each pair now shares an armored collar instead of reading as wires.
    for x0, x1 in ((15, 29), (35, 49)):
        d.rounded_rectangle(
            [s(x0), s(15), s(x1), s(31)],
            radius=s(3),
            fill=(*IRON_DARK, 255),
        )
    for x in (19, 25, 39, 45):
        d.rounded_rectangle(
            [s(x - 3), s(4), s(x + 3), s(28)],
            radius=s(2),
            fill=(*IRON_DARK, 255),
        )
        d.rectangle([s(x - 1), s(5), s(x + 1), s(26)], fill=(*IRON_LIGHT, 255))
        d.rounded_rectangle(
            [s(x - 4), s(2), s(x + 4), s(8)],
            radius=s(2),
            fill=(*pal["light"], 255),
        )
        d.rectangle([s(x - 2), s(2), s(x + 2), s(5)], fill=(*IRON_DARK, 255))
    # Common armored bridge and traverse hub bind the two batteries into one
    # unmistakably broad H silhouette.
    d.rounded_rectangle(
        [s(11), s(27), s(53), s(43)], radius=s(5), fill=(*IRON_DARK, 255)
    )
    d.rectangle([s(16), s(31), s(48), s(39)], fill=(*pal["dark"], 255))
    d.ellipse([s(23), s(23), s(41), s(43)], fill=(*pal["light"], 255))
    d.ellipse([s(28), s(28), s(36), s(36)], fill=(*IRON_DARK, 255))
    finish(img, px, f"flak_mount_{faction}")


def bastion(faction: str) -> None:
    """2x2 artillery foundation: a braced bunker and armored traverse pit.

    The cannon is authored separately so the live silhouette can aim and
    recoil instead of behaving like a painted circle.
    """
    px = 128
    pal = FACTIONS[faction]
    img, d = canvas(px)
    # A clipped square blast apron fills the 2x2 footprint. It is deliberately
    # architectural, not another enlarged circular turret base.
    outer = [
        (22, 5),
        (106, 5),
        (123, 22),
        (123, 106),
        (106, 123),
        (22, 123),
        (5, 106),
        (5, 22),
    ]
    inner = [
        (27, 12),
        (101, 12),
        (116, 27),
        (116, 101),
        (101, 116),
        (27, 116),
        (12, 101),
        (12, 27),
    ]
    d.polygon([(s(x), s(y)) for x, y in outer], fill=(*IRON_DARK, 255))
    d.polygon([(s(x), s(y)) for x, y in inner], fill=(*IRON, 255))

    # Four heavy buttresses point at the weapon's cardinal arcs. Broad color
    # plates survive at map scale and make ownership obvious through the gun.
    for points in (
        [(37, 13), (91, 13), (84, 35), (44, 35)],
        [(115, 37), (115, 91), (93, 84), (93, 44)],
        [(37, 115), (91, 115), (84, 93), (44, 93)],
        [(13, 37), (13, 91), (35, 84), (35, 44)],
    ):
        d.polygon([(s(x), s(y)) for x, y in points], fill=(*pal["dark"], 255))
    for x0, y0, x1, y1 in (
        (17, 17, 35, 35),
        (93, 17, 111, 35),
        (17, 93, 35, 111),
        (93, 93, 111, 111),
    ):
        d.rounded_rectangle(
            [s(x0), s(y0), s(x1), s(y1)],
            radius=s(4),
            fill=(*IRON_DARK, 255),
        )
        d.rectangle(
            [s(x0 + 4), s(y0 + 7), s(x1 - 4), s(y1 - 7)],
            fill=(*pal["base"], 255),
        )

    # An octagonal traverse well reads as a recessed mechanical socket. The
    # mount fills its center but leaves the thick segmented collar visible.
    pit_outer = [
        (42, 28),
        (86, 28),
        (100, 42),
        (100, 86),
        (86, 100),
        (42, 100),
        (28, 86),
        (28, 42),
    ]
    pit_mid = [
        (45, 35),
        (83, 35),
        (93, 45),
        (93, 83),
        (83, 93),
        (45, 93),
        (35, 83),
        (35, 45),
    ]
    pit_inner = [
        (49, 43),
        (79, 43),
        (85, 49),
        (85, 79),
        (79, 85),
        (49, 85),
        (43, 79),
        (43, 49),
    ]
    d.polygon([(s(x), s(y)) for x, y in pit_outer], fill=(*IRON_DARK, 255))
    d.polygon([(s(x), s(y)) for x, y in pit_mid], fill=(*pal["base"], 255))
    d.polygon([(s(x), s(y)) for x, y in pit_inner], fill=(*IRON_DARK, 255))

    # Two protected shell lockers sell the emplacement's artillery role even
    # before the barrel turns or fires.
    for x0, y0 in ((18, 57), (94, 57)):
        d.rounded_rectangle(
            [s(x0), s(y0), s(x0 + 16), s(y0 + 22)],
            radius=s(3),
            fill=(*IRON_DARK, 255),
        )
        for row in range(3):
            cy = y0 + 5 + row * 6
            d.rounded_rectangle(
                [s(x0 + 4), s(cy), s(x0 + 12), s(cy + 3)],
                radius=s(1),
                fill=(*SCRAP, 255),
            )
    finish(img, px, f"bastion_{faction}")


def bastion_mount(faction: str) -> None:
    """Massive single siege cannon, pivoted at the footprint center."""
    px = 128
    pal = FACTIONS[faction]
    img, d = canvas(px)
    # One broad, stepped barrel points up. The renderer's shell and muzzle
    # flash originate at the forward edge, and the tube reads as siege ordnance
    # instead of three narrow rails.
    d.rounded_rectangle(
        [s(49), s(14), s(79), s(70)], radius=s(8), fill=(*IRON_DARK, 255)
    )
    d.rounded_rectangle(
        [s(55), s(12), s(73), s(67)], radius=s(5), fill=(*IRON_LIGHT, 255)
    )
    d.rounded_rectangle(
        [s(45), s(5), s(83), s(22)], radius=s(5), fill=(*IRON_DARK, 255)
    )
    d.rectangle([s(51), s(6), s(77), s(17)], fill=(*pal["light"], 255))
    d.rounded_rectangle(
        [s(57), s(3), s(71), s(12)], radius=s(3), fill=(15, 13, 14, 255)
    )
    # Thick recoil cylinders flank the tube and terminate in oversized armor
    # collars instead of disappearing into the central housing.
    for x0, x1 in ((34, 51), (77, 94)):
        d.rounded_rectangle(
            [s(x0), s(30), s(x1), s(83)],
            radius=s(6),
            fill=(*IRON_DARK, 255),
        )
        d.rounded_rectangle(
            [s(x0 + 4), s(36), s(x1 - 4), s(76)],
            radius=s(3),
            fill=(*pal["dark"], 255),
        )

    # The enormous breech is wider than a unit and intentionally heavy at the
    # rear, so turning and recoil remain readable from the map-scale camera.
    housing = [
        (39, 45),
        (89, 45),
        (104, 61),
        (100, 87),
        (87, 99),
        (41, 99),
        (28, 87),
        (24, 61),
    ]
    d.polygon([(s(x), s(y)) for x, y in housing], fill=(*IRON_DARK, 255))
    inner = [
        (43, 52),
        (85, 52),
        (95, 63),
        (92, 82),
        (82, 91),
        (46, 91),
        (36, 82),
        (33, 63),
    ]
    d.polygon([(s(x), s(y)) for x, y in inner], fill=(*pal["base"], 255))
    d.rounded_rectangle(
        [s(45), s(58), s(83), s(86)], radius=s(8), fill=(*pal["dark"], 255)
    )
    d.rounded_rectangle(
        [s(53), s(61), s(75), s(83)], radius=s(6), fill=(*IRON_DARK, 255)
    )
    d.ellipse([s(58), s(66), s(70), s(78)], fill=(*IRON_LIGHT, 255))

    # Redraw the tube over the housing. Burying it underneath the bunker
    # left only a dark muzzle stub at game scale; this high-contrast spine
    # carries the artillery silhouette from the pivot to the footprint edge.
    d.rounded_rectangle(
        [s(53), s(1), s(75), s(69)], radius=s(6), fill=(*IRON_DARK, 255)
    )
    d.rounded_rectangle(
        [s(58), s(3), s(70), s(66)], radius=s(3), fill=(*IRON_LIGHT, 255)
    )
    d.rectangle([s(62), s(5), s(66), s(63)], fill=(*pal["light"], 255))
    d.rounded_rectangle(
        [s(48), s(0), s(80), s(13)], radius=s(4), fill=(*IRON_DARK, 255)
    )
    d.rounded_rectangle([s(55), s(2), s(73), s(9)], radius=s(3), fill=(12, 10, 11, 255))

    # A deep loader block and off-center shell rammer complete the fore/aft
    # silhouette. The asymmetric hatch makes aim direction obvious at rest.
    counterweight = [(40, 91), (88, 91), (98, 105), (88, 121), (40, 121), (30, 105)]
    d.polygon([(s(x), s(y)) for x, y in counterweight], fill=(*IRON_DARK, 255))
    d.rounded_rectangle(
        [s(42), s(98), s(86), s(115)], radius=s(5), fill=(*pal["dark"], 255)
    )
    d.rounded_rectangle(
        [s(69), s(99), s(84), s(113)], radius=s(4), fill=(*pal["light"], 255)
    )
    d.rounded_rectangle([s(44), s(102), s(63), s(111)], radius=s(3), fill=(*IRON, 255))
    finish(img, px, f"bastion_mount_{faction}")


def array(faction: str, work: int = 0) -> None:
    """1x1 radar mast: a lattice tower under a wide dish — the eyes that
    make long guns matter."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    d.rounded_rectangle(
        [s(10), s(10), s(54), s(54)], radius=s(7), fill=(*IRON_DARK, 255)
    )
    d.rounded_rectangle([s(14), s(14), s(50), s(50)], radius=s(5), fill=(*IRON, 255))
    # Lattice cross-braces.
    for x0, y0, x1, y1 in ((18, 18, 46, 46), (46, 18, 18, 46)):
        d.line([(s(x0), s(y0)), (s(x1), s(y1))], fill=(*IRON_DARK, 255), width=s(2))
    # The dish, with its feed direction carried by the sprite frame. This
    # replaces the world-space radar needle that used to float over it.
    d.ellipse([s(16), s(14), s(52), s(50)], fill=(*pal["dark"], 255))
    d.ellipse([s(20), s(18), s(48), s(46)], fill=(*pal["base"], 255))
    d.ellipse([s(24), s(22), s(44), s(42)], fill=(*pal["dark"], 255))
    heading = (-55, 35, 125, 215)[work]
    d.arc(
        [s(20), s(18), s(48), s(46)],
        heading - 60,
        heading + 60,
        fill=(*pal["light"], 255),
        width=s(2),
    )
    # Feed horn and its shadow.
    angle = math.radians(heading)
    horn = (34 + 14 * math.cos(angle), 32 + 14 * math.sin(angle))
    d.line(
        [(s(34), s(32)), (s(horn[0]), s(horn[1]))], fill=(*IRON_LIGHT, 255), width=s(2)
    )
    d.ellipse([s(31), s(29), s(37), s(35)], fill=(*BONE, 255))
    suffix = "" if work == 0 else f"_work{work}"
    finish(img, px, f"array_{faction}{suffix}")


def reclaimer(faction: str, work: int = 0) -> None:
    """1x1 debris grinder: hopper, drum, and a chute stained amber by
    everything it has ever eaten."""
    px = 64
    pal = FACTIONS[faction]
    img, d = canvas(px)
    d.rounded_rectangle([s(6), s(8), s(58), s(56)], radius=s(7), fill=(*IRON_DARK, 255))
    d.rounded_rectangle([s(10), s(12), s(54), s(52)], radius=s(5), fill=(*IRON, 255))
    # Intake hopper: a funnel mouth at the top.
    d.polygon(
        [(s(14), s(12)), (s(50), s(12)), (s(42), s(28)), (s(22), s(28))],
        fill=(*pal["dark"], 255),
    )
    d.polygon(
        [(s(18), s(14)), (s(46), s(14)), (s(40), s(24)), (s(24), s(24))],
        fill=(12, 10, 10, 255),
    )
    # Grinder drum with teeth. The whole drum advances through authored
    # frames, joined by a few chunks travelling from hopper to chute.
    d.ellipse([s(18), s(26), s(46), s(48)], fill=(*pal["base"], 255))
    for i in range(6):
        ang = i / 6 * 6.28318 + work * math.pi / 12
        cx, cy = 32 + 10 * math.cos(ang), 37 + 9 * math.sin(ang)
        d.ellipse([s(cx - 2), s(cy - 2), s(cx + 2), s(cy + 2)], fill=(*IRON_LIGHT, 255))
    # Three asymmetric cutter arms rotate a full 30 degrees per frame.
    # Their bright endpoints and thick spokes remain legible after the
    # 64px source is reduced to one battlefield tile.
    cutter_phase = work * math.pi / 6
    for arm in range(3):
        angle = cutter_phase + arm * math.tau / 3
        tip_x = 32 + 9 * math.cos(angle)
        tip_y = 37 + 8 * math.sin(angle)
        d.line(
            [(s(32), s(37)), (s(tip_x), s(tip_y))],
            fill=(*BONE, 255),
            width=s(4),
        )
        d.ellipse(
            [s(tip_x - 3), s(tip_y - 3), s(tip_x + 3), s(tip_y + 3)],
            fill=(*SCRAP, 255),
        )
    d.ellipse([s(27), s(32), s(37), s(42)], fill=(*pal["dark"], 255))
    chunk_y = (17, 27, 38, 49)[work]
    d.polygon(
        [
            (s(27), s(chunk_y)),
            (s(34), s(chunk_y - 3)),
            (s(39), s(chunk_y + 2)),
            (s(31), s(chunk_y + 7)),
        ],
        fill=(*SCRAP_LIGHT, 255),
    )
    # Output chute, amber-stained.
    d.rectangle([s(24), s(48), s(40), s(58)], fill=(*IRON_DARK, 255))
    d.rectangle([s(27), s(50), s(37), s(56)], fill=(*SCRAP_DARK, 255))
    d.rectangle([s(30), s(52), s(34), s(56)], fill=(*SCRAP, 255))
    suffix = "" if work == 0 else f"_work{work}"
    finish(img, px, f"reclaimer_{faction}{suffix}")


def _gear(
    d, cx: float, cy: float, radius: float, teeth: int, turn: float, color
) -> None:
    """Draws a compact top-down gear whose phase remains legible at 64px."""
    for tooth in range(teeth):
        angle = turn + tooth * math.tau / teeth
        tangent = (-math.sin(angle), math.cos(angle))
        radial = (math.cos(angle), math.sin(angle))
        inner = radius * 0.72
        outer = radius * 1.18
        half = radius * 0.16
        d.polygon(
            [
                (
                    s(cx + radial[0] * inner + tangent[0] * half),
                    s(cy + radial[1] * inner + tangent[1] * half),
                ),
                (
                    s(cx + radial[0] * outer + tangent[0] * half),
                    s(cy + radial[1] * outer + tangent[1] * half),
                ),
                (
                    s(cx + radial[0] * outer - tangent[0] * half),
                    s(cy + radial[1] * outer - tangent[1] * half),
                ),
                (
                    s(cx + radial[0] * inner - tangent[0] * half),
                    s(cy + radial[1] * inner - tangent[1] * half),
                ),
            ],
            fill=color,
        )
    d.ellipse(
        [s(cx - radius), s(cy - radius), s(cx + radius), s(cy + radius)],
        fill=color,
    )
    d.ellipse(
        [
            s(cx - radius * 0.35),
            s(cy - radius * 0.35),
            s(cx + radius * 0.35),
            s(cy + radius * 0.35),
        ],
        fill=(*IRON_DARK, 255),
    )


def repair_bay(faction: str, work: int = 0) -> None:
    """2x2 field workshop: an open service pad under a welding gantry —
    wounded machines roll in past the hazard chevrons and roll out whole."""
    px = 128
    pal = FACTIONS[faction]
    img, d = canvas(px)
    d.rounded_rectangle(
        [s(6), s(8), s(122), s(120)], radius=s(9), fill=(*IRON_DARK, 255)
    )
    d.rounded_rectangle([s(12), s(14), s(116), s(114)], radius=s(7), fill=(*IRON, 255))
    # Recessed service pad, open to the south.
    d.rounded_rectangle(
        [s(24), s(30), s(104), s(106)], radius=s(5), fill=(*IRON_DARK, 255)
    )
    d.rounded_rectangle(
        [s(28), s(34), s(100), s(102)], radius=s(4), fill=(12, 10, 10, 255)
    )
    # Hazard chevrons along the drive-in edge.
    for i in range(5):
        x0 = 30 + i * 14
        d.polygon(
            [
                (s(x0), s(102)),
                (s(x0 + 7), s(102)),
                (s(x0 + 11), s(110)),
                (s(x0 + 4), s(110)),
            ],
            fill=(*pal["light"], 255),
        )
    # Welding gantry spanning the pad.
    d.rectangle([s(18), s(52), s(110), s(60)], fill=(*IRON_DARK, 255))
    d.rectangle([s(18), s(52), s(110), s(54)], fill=(*IRON_LIGHT, 255))
    # Twin service arms reach into the bay while roof gears visibly turn.
    arm_shift = (0, -5, 3, 5)[work]
    for ax in (46 + arm_shift, 82 - arm_shift):
        d.rectangle([s(ax - 3), s(58), s(ax + 3), s(78)], fill=(*pal["dark"], 255))
        d.ellipse([s(ax - 6), s(74), s(ax + 6), s(86)], fill=(*pal["base"], 255))
        d.ellipse([s(ax - 2), s(78), s(ax + 2), s(82)], fill=(*BONE, 255))
    turn = work * math.pi / 8
    _gear(d, 35, 22, 7, 8, turn, (*pal["light"], 255))
    _gear(d, 93, 22, 7, 8, -turn, (*pal["base"], 255))
    # Corner service posts.
    for cx, cy in ((18, 20), (110, 20), (18, 108), (110, 108)):
        d.ellipse([s(cx - 6), s(cy - 6), s(cx + 6), s(cy + 6)], fill=(*IRON_DARK, 255))
        d.ellipse(
            [s(cx - 3), s(cy - 3), s(cx + 3), s(cy + 3)], fill=(*pal["base"], 255)
        )
    # Faction service band on the roof edge.
    d.rectangle([s(50), s(16), s(78), s(26)], fill=(*pal["dark"], 255))
    d.rectangle([s(54), s(18), s(74), s(24)], fill=(*pal["light"], 255))
    suffix = "" if work == 0 else f"_work{work}"
    finish(img, px, f"repair_bay_{faction}{suffix}")


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
        d.ellipse(
            [s(cx - 1.5), s(cy - 1.5), s(cx + 1.5), s(cy + 1.5)], fill=(*SCRAP, 255)
        )
    finish(img, px, "wreck_pile")


def air_shadow() -> None:
    """The soft blob a flyer casts on the ground — drawn separately and
    offset by the shell so altitude reads at a glance."""
    px = 64
    img, d = canvas(px)
    for r, alpha in [(22, 40), (16, 60), (10, 80)]:
        d.ellipse(
            [s(32 - r), s(36 - r * 0.6), s(32 + r), s(36 + r * 0.6)],
            fill=(8, 8, 12, alpha),
        )
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
                    [
                        (s(a), s(b)),
                        (s(a + rng.randrange(-6, 7)), s(b + rng.randrange(3, 9))),
                    ],
                    fill=(16, 16, 20, 120),
                    width=SS,
                )
    elif style == "plate":
        x, y = rng.randrange(10, 26), rng.randrange(10, 26)
        w, h = rng.randrange(18, 30), rng.randrange(14, 24)
        d.rectangle(
            [s(x), s(y), s(x + w), s(y + h)], outline=(70, 70, 82, 110), width=SS
        )
        for cx, cy in [
            (x + 3, y + 3),
            (x + w - 3, y + 3),
            (x + 3, y + h - 3),
            (x + w - 3, y + h - 3),
        ]:
            d.ellipse(
                [s(cx - 1), s(cy - 1), s(cx + 1), s(cy + 1)], fill=(70, 70, 82, 140)
            )
    elif style == "stain":
        for _ in range(rng.randrange(3, 5)):
            cx, cy = rng.randrange(16, 48), rng.randrange(16, 48)
            r = rng.randrange(6, 14)
            d.ellipse(
                [s(cx - r), s(cy - r), s(cx + r), s(cy + r)], fill=(24, 20, 16, 60)
            )
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


def _late_theme_prop(d, theme: str, variant: int, rng) -> None:
    """Low-profile physical dressing for the second theme-prop bank."""
    if theme == "rusted_yard":
        rust = (169, 78, 46, 180)
        edge = (211, 113, 67, 185)
        steel = (61, 48, 47, 205)
        dark = (26, 23, 26, 185)
        if variant == 3:
            # A low stack of oversized repair plates. Their broad faces and
            # fixed rivets keep them distinct from loose salvage pickups.
            plates = (
                [(12, 25), (43, 17), (54, 34), (22, 43)],
                [(9, 20), (39, 11), (51, 29), (19, 38)],
                [(16, 15), (47, 18), (45, 39), (13, 35)],
            )
            for layer, pts in enumerate(plates):
                shifted = [(x + 3, y + 4) for x, y in pts]
                d.polygon([(s(x), s(y)) for x, y in shifted], fill=dark)
                tone = (58 + layer * 7, 45 + layer * 3, 44 + layer * 2, 215)
                d.polygon([(s(x), s(y)) for x, y in pts], fill=tone)
                d.line(
                    [(s(x), s(y)) for x, y in [*pts, pts[0]]],
                    fill=rust,
                    width=s(2),
                )
                d.line(
                    [(s(pts[0][0]), s(pts[0][1])), (s(pts[1][0]), s(pts[1][1]))],
                    fill=edge,
                    width=SS,
                )
            for x, y in ((20, 21), (39, 22), (19, 32), (39, 34)):
                d.ellipse(
                    [s(x - 2), s(y - 2), s(x + 2), s(y + 2)],
                    fill=dark,
                    outline=edge,
                    width=SS,
                )
        elif variant == 4:
            # A recessed maintenance hatch with a raised, oxidized rim.
            d.ellipse([s(8), s(11), s(58), s(61)], fill=dark)
            d.ellipse([s(6), s(7), s(58), s(59)], fill=rust)
            d.ellipse([s(11), s(12), s(53), s(54)], fill=steel)
            d.arc([s(9), s(10), s(55), s(56)], 198, 342, fill=edge, width=s(3))
            d.ellipse([s(19), s(20), s(45), s(46)], fill=(31, 28, 31, 220))
            d.arc([s(19), s(20), s(45), s(46)], 190, 345, fill=rust, width=s(3))
            for angle in range(0, 360, 60):
                rad = math.radians(angle)
                x = 32 + math.cos(rad) * 20
                y = 33 + math.sin(rad) * 20
                d.ellipse(
                    [s(x - 2), s(y - 2), s(x + 2), s(y + 2)],
                    fill=dark,
                    outline=edge,
                    width=SS,
                )
        else:
            # Three collapsed conduit runs, held together by two steel bands.
            for y in (19, 31, 43):
                d.rounded_rectangle(
                    [s(7), s(y - 3), s(57), s(y + 5)], radius=s(3), fill=dark
                )
                d.rounded_rectangle(
                    [s(6), s(y - 5), s(56), s(y + 2)], radius=s(3), fill=steel
                )
                d.line([(s(10), s(y - 4)), (s(52), s(y - 4))], fill=edge, width=SS)
                for x in (7, 55):
                    d.rectangle([s(x - 2), s(y - 5), s(x + 2), s(y + 2)], fill=rust)
            for x in (22, 43):
                d.rectangle([s(x - 2), s(12), s(x + 3), s(49)], fill=dark)
                d.rectangle([s(x - 3), s(10), s(x + 2), s(47)], fill=(91, 63, 55, 220))
                d.line([(s(x - 2), s(11)), (s(x + 1), s(11))], fill=edge, width=SS)
    elif theme == "cold_circuitry":
        cyan = (110, 206, 218, 195)
        ice = (172, 231, 235, 205)
        blue = (39, 73, 88, 220)
        deep = (16, 32, 42, 210)
        if variant == 3:
            # A raised switching box bridging a buried conduit.
            pts = [(4, 34), (16, 34), (16, 14), (35, 14), (35, 47), (58, 47)]
            d.line([(s(x), s(y + 3)) for x, y in pts], fill=deep, width=s(8))
            d.line([(s(x), s(y)) for x, y in pts], fill=blue, width=s(7))
            d.line([(s(x), s(y - 2)) for x, y in pts], fill=cyan, width=s(2))
            for x, y in ((16, 34), (16, 14), (35, 14), (35, 47)):
                d.ellipse(
                    [s(x - 6), s(y - 4), s(x + 6), s(y + 8)],
                    fill=deep,
                )
                d.ellipse(
                    [s(x - 6), s(y - 6), s(x + 6), s(y + 6)],
                    fill=blue,
                    outline=cyan,
                    width=s(2),
                )
                d.rectangle([s(x - 2), s(y - 2), s(x + 2), s(y + 2)], fill=ice)
        elif variant == 4:
            # A low cooling fan in a beveled floor cassette.
            d.rounded_rectangle([s(7), s(10), s(59), s(62)], radius=s(7), fill=deep)
            d.rounded_rectangle([s(5), s(6), s(59), s(60)], radius=s(7), fill=blue)
            d.rounded_rectangle(
                [s(8), s(9), s(56), s(57)],
                radius=s(6),
                outline=cyan,
                width=s(3),
            )
            d.ellipse([s(15), s(16), s(49), s(50)], fill=deep, outline=cyan, width=s(2))
            for angle in (0, 90, 180, 270):
                rad = math.radians(angle)
                tangent = rad + math.pi / 2
                pts = [
                    (32 + math.cos(rad) * 4, 33 + math.sin(rad) * 4),
                    (
                        32 + math.cos(rad) * 14 + math.cos(tangent) * 5,
                        33 + math.sin(rad) * 14 + math.sin(tangent) * 5,
                    ),
                    (
                        32 + math.cos(rad) * 11 - math.cos(tangent) * 3,
                        33 + math.sin(rad) * 11 - math.sin(tangent) * 3,
                    ),
                ]
                d.polygon([(s(x), s(y)) for x, y in pts], fill=(68, 132, 147, 230))
            d.ellipse([s(27), s(28), s(37), s(38)], fill=ice, outline=deep, width=s(2))
            for x, y in ((12, 13), (52, 13), (52, 53), (12, 53)):
                d.rectangle([s(x - 2), s(y - 2), s(x + 2), s(y + 2)], fill=ice)
        else:
            # A sunken cable trench with heavy cross-braces.
            d.rounded_rectangle([s(7), s(10), s(57), s(56)], radius=s(5), fill=deep)
            d.line([(s(10), s(12)), (s(54), s(12))], fill=cyan, width=s(3))
            d.line([(s(9), s(54)), (s(55), s(54))], fill=(25, 48, 60, 235), width=s(3))
            for y in (18, 31, 44):
                d.rounded_rectangle(
                    [s(10), s(y - 3), s(54), s(y + 4)], radius=s(2), fill=blue
                )
                d.line([(s(13), s(y - 2)), (s(51), s(y - 2))], fill=cyan, width=SS)
            for x in (17, 32, 47):
                d.rectangle([s(x - 3), s(11), s(x + 3), s(55)], fill=(52, 96, 111, 235))
                d.line([(s(x - 2), s(12)), (s(x - 2), s(53))], fill=ice, width=SS)
    elif theme == "quarry_dust":
        dust = (180, 153, 108, 190)
        stone = (113, 96, 73, 220)
        cut = (57, 51, 45, 215)
        if variant == 3:
            # Nine drilled blast sockets, each with a chipped stone collar.
            for y in (15, 32, 49):
                for x in (14, 32, 50):
                    d.ellipse([s(x - 5), s(y - 2), s(x + 5), s(y + 6)], fill=cut)
                    d.ellipse([s(x - 5), s(y - 5), s(x + 5), s(y + 5)], fill=stone)
                    d.ellipse([s(x - 3), s(y - 3), s(x + 3), s(y + 3)], fill=cut)
                    d.arc(
                        [s(x - 5), s(y - 4), s(x + 5), s(y + 4)],
                        180,
                        350,
                        fill=dust,
                        width=s(2),
                    )
        elif variant == 4:
            # A shallow terraced extraction pit, broad enough to read at zoom.
            d.ellipse([s(6), s(12), s(60), s(57)], fill=cut)
            d.ellipse([s(7), s(8), s(59), s(53)], fill=stone)
            d.arc([s(7), s(8), s(59), s(53)], 190, 348, fill=dust, width=s(3))
            d.ellipse([s(14), s(16), s(52), s(48)], fill=(73, 63, 53, 235))
            d.arc([s(14), s(16), s(52), s(48)], 190, 348, fill=stone, width=s(3))
            d.ellipse([s(22), s(23), s(44), s(42)], fill=(39, 37, 35, 225))
            d.arc([s(22), s(23), s(44), s(42)], 190, 348, fill=dust, width=s(2))
        else:
            # A discarded drilled slab: one manufactured piece, not a scrap node.
            pts = [(10, 18), (43, 10), (56, 26), (47, 50), (16, 54), (7, 36)]
            d.polygon([(s(x + 3), s(y + 5)) for x, y in pts], fill=cut)
            d.polygon([(s(x), s(y)) for x, y in pts], fill=stone)
            d.line(
                [(s(x), s(y)) for x, y in [*pts, pts[0]]],
                fill=dust,
                width=s(2),
            )
            d.line(
                [(s(10), s(18)), (s(43), s(10))], fill=(216, 185, 132, 190), width=SS
            )
            for x, y in ((20, 25), (38, 21), (27, 40), (45, 36)):
                d.ellipse([s(x - 4), s(y - 3), s(x + 4), s(y + 5)], fill=cut)
                d.arc(
                    [s(x - 4), s(y - 4), s(x + 4), s(y + 4)],
                    180,
                    350,
                    fill=dust,
                    width=SS,
                )
    elif theme == "basalt":
        violet = (112, 97, 151, 195)
        glass = (50, 46, 70, 225)
        void = (13, 14, 22, 230)
        if variant == 3:
            # A collapsed lava blister with a glassy beveled lip.
            outer = [(8, 25), (18, 8), (40, 9), (57, 24), (49, 49), (26, 57), (8, 42)]
            inner = [
                (16, 27),
                (23, 16),
                (38, 16),
                (49, 27),
                (43, 42),
                (27, 48),
                (16, 39),
            ]
            d.polygon([(s(x + 2), s(y + 4)) for x, y in outer], fill=void)
            d.polygon([(s(x), s(y)) for x, y in outer], fill=glass)
            d.line(
                [(s(x), s(y)) for x, y in [*outer, outer[0]]],
                fill=violet,
                width=s(3),
            )
            d.polygon([(s(x), s(y)) for x, y in inner], fill=void)
            d.line(
                [(s(18), s(8)), (s(40), s(9)), (s(57), s(24))],
                fill=(146, 126, 184, 190),
                width=SS,
            )
            d.line(
                [(s(23), s(16)), (s(38), s(16)), (s(49), s(27))], fill=violet, width=SS
            )
        elif variant == 4:
            # Cooled pressure ridges, low but visibly raised off the surface.
            for off in (-8, 0, 8):
                pts = [
                    (3, 39 + off),
                    (18, 27 + off),
                    (32, 34 + off),
                    (47, 20 + off),
                    (62, 27 + off),
                ]
                d.line([(s(x), s(y + 4)) for x, y in pts], fill=void, width=s(8))
                d.line([(s(x), s(y)) for x, y in pts], fill=glass, width=s(7))
                d.line([(s(x), s(y - 2)) for x, y in pts], fill=violet, width=s(2))
        else:
            # A star-shaped impact well, with a faceted rim and deep center.
            center = (32, 31)
            rim = []
            for angle in range(0, 360, 45):
                rad = math.radians(angle + rng.randrange(-4, 5))
                radius = rng.randrange(23, 28)
                end = (
                    center[0] + math.cos(rad) * radius,
                    center[1] + math.sin(rad) * radius,
                )
                rim.append(end)
            d.polygon([(s(x + 2), s(y + 4)) for x, y in rim], fill=void)
            d.polygon([(s(x), s(y)) for x, y in rim], fill=glass)
            d.line([(s(x), s(y)) for x, y in [*rim, rim[0]]], fill=violet, width=s(2))
            d.ellipse([s(20), s(19), s(44), s(43)], fill=void)
            for end in rim:
                d.line(
                    [(s(center[0]), s(center[1])), (s(end[0]), s(end[1]))],
                    fill=void,
                    width=s(3),
                )
            d.arc([s(20), s(19), s(44), s(43)], 190, 342, fill=violet, width=s(3))
    elif theme == "slag":
        hot = (203, 77, 57, 195)
        ember = (236, 125, 73, 175)
        crust = (57, 38, 41, 225)
        char = (29, 23, 27, 230)
        if variant == 3:
            # Molten pockets trapped beneath a thick, cooled crust.
            pools = [(19, 20, 13, 7), (40, 31, 17, 9), (22, 47, 15, 6)]
            for cx, cy, rx, ry in pools:
                d.ellipse(
                    [s(cx - rx), s(cy - ry + 3), s(cx + rx), s(cy + ry + 6)], fill=char
                )
                d.ellipse([s(cx - rx), s(cy - ry), s(cx + rx), s(cy + ry)], fill=crust)
                d.ellipse(
                    [s(cx - rx + 4), s(cy - ry + 3), s(cx + rx - 4), s(cy + ry - 3)],
                    fill=(80, 38, 40, 225),
                )
                d.arc(
                    [s(cx - rx + 4), s(cy - ry + 3), s(cx + rx - 4), s(cy + ry - 3)],
                    190,
                    350,
                    fill=hot,
                    width=s(2),
                )
                d.arc(
                    [s(cx - rx), s(cy - ry), s(cx + rx), s(cy + ry)],
                    190,
                    350,
                    fill=ember,
                    width=SS,
                )
        elif variant == 4:
            # Raised slag blisters with hot seams at their upper edge.
            for x, y, r in ((13, 31, 7), (27, 25, 9), (43, 34, 11), (57, 28, 6)):
                d.ellipse(
                    [s(x - r), s(y - r * 0.45 + 3), s(x + r), s(y + r * 0.75 + 4)],
                    fill=char,
                )
                d.ellipse(
                    [s(x - r), s(y - r * 0.7), s(x + r), s(y + r * 0.7)], fill=crust
                )
                d.arc(
                    [s(x - r), s(y - r * 0.7), s(x + r), s(y + r * 0.7)],
                    200,
                    350,
                    fill=ember,
                    width=s(2),
                )
        else:
            # An open slag drain with a cast grate rather than painted chevrons.
            trough = [(5, 48), (45, 8), (58, 17), (17, 58)]
            d.polygon([(s(x + 2), s(y + 3)) for x, y in trough], fill=char)
            d.polygon([(s(x), s(y)) for x, y in trough], fill=crust)
            inner = [(12, 47), (45, 14), (52, 18), (17, 53)]
            d.polygon([(s(x), s(y)) for x, y in inner], fill=(34, 24, 29, 235))
            d.line([(s(8), s(47)), (s(45), s(10))], fill=ember, width=s(2))
            for x, y in ((16, 43), (24, 35), (32, 27), (40, 19)):
                d.line(
                    [(s(x - 8), s(y - 1)), (s(x + 7), s(y + 14))], fill=char, width=s(5)
                )
                d.line(
                    [(s(x - 7), s(y - 3)), (s(x + 8), s(y + 12))], fill=hot, width=s(2)
                )
    elif theme == "verdigris":
        patina = (67, 177, 149, 205)
        mint = (107, 209, 177, 190)
        copper = (163, 94, 53, 210)
        deep = (27, 54, 53, 225)
        if variant == 3:
            # An inset copper drain with a raised, oxidized frame.
            d.rounded_rectangle([s(10), s(17), s(57), s(55)], radius=s(6), fill=deep)
            d.rounded_rectangle(
                [s(7), s(11), s(56), s(52)],
                radius=s(5),
                fill=(50, 91, 81, 230),
                outline=copper,
                width=s(3),
            )
            d.line([(s(10), s(13)), (s(53), s(13))], fill=mint, width=s(2))
            for x in range(15, 52, 7):
                d.rounded_rectangle(
                    [s(x), s(18), s(x + 4), s(46)], radius=s(2), fill=deep
                )
                d.line([(s(x + 1), s(19)), (s(x + 1), s(44))], fill=patina, width=SS)
        elif variant == 4:
            # Two half-buried pipe flanges, with visible bores and bolt heads.
            for cx, cy, r in ((22, 25, 14), (42, 39, 16)):
                d.ellipse(
                    [s(cx - r), s(cy - r + 3), s(cx + r), s(cy + r + 6)], fill=deep
                )
                d.ellipse([s(cx - r), s(cy - r), s(cx + r), s(cy + r)], fill=copper)
                d.ellipse(
                    [s(cx - r + 4), s(cy - r + 4), s(cx + r - 4), s(cy + r - 4)],
                    fill=patina,
                )
                d.ellipse(
                    [s(cx - r + 8), s(cy - r + 8), s(cx + r - 8), s(cy + r - 8)],
                    fill=deep,
                )
                d.arc(
                    [s(cx - r), s(cy - r), s(cx + r), s(cy + r)],
                    190,
                    342,
                    fill=mint,
                    width=s(2),
                )
                for angle in range(0, 360, 90):
                    rad = math.radians(angle)
                    x = cx + math.cos(rad) * (r - 3)
                    y = cy + math.sin(rad) * (r - 3)
                    d.ellipse(
                        [s(x - 1.5), s(y - 1.5), s(x + 1.5), s(y + 1.5)], fill=mint
                    )
        else:
            # A raised conduit manifold with chunky junction collars.
            pts = [(4, 39), (18, 39), (24, 20), (40, 20), (46, 45), (60, 45)]
            d.line([(s(x), s(y + 4)) for x, y in pts], fill=deep, width=s(10))
            d.line([(s(x), s(y)) for x, y in pts], fill=copper, width=s(9))
            d.line([(s(x), s(y - 2)) for x, y in pts], fill=mint, width=s(2))
            for x, y in ((18, 39), (24, 20), (40, 20), (46, 45)):
                d.ellipse([s(x - 6), s(y - 3), s(x + 6), s(y + 9)], fill=deep)
                d.ellipse(
                    [s(x - 6), s(y - 6), s(x + 6), s(y + 6)],
                    fill=patina,
                    outline=copper,
                    width=s(2),
                )
                d.rectangle([s(x - 2), s(y - 2), s(x + 2), s(y + 2)], fill=mint)
    else:
        raise ValueError(f"unknown theme {theme!r}")


def theme_prop(theme: str, variant: int) -> None:
    """One-tile dressing for a shipped map theme.

    The first bank is surface history; the second is low-profile machinery,
    material, and recesses. None resembles a resource, structure, or cover
    marker. Surface marks in the first bank can rotate freely. The raised
    second bank keeps its authored world-space lighting upright in the shell.
    """
    px = 64
    img, d = canvas(px)
    rng = random.Random(2000 + sum(theme.encode()) * 17 + variant * 101)

    if variant >= 3:
        _late_theme_prop(d, theme, variant, rng)
        alpha = img.getchannel("A").point(
            lambda a: min(235, round(a * 1.35 + 10)) if a else 0
        )
        img.putalpha(alpha)
        finish(img, px, f"theme_{theme}_{variant}")
        return

    if theme == "rusted_yard":
        if variant == 0:
            # Rails buried flush in the yard, with faded cross-ties.
            for y in (23, 41):
                d.line(
                    [(s(5), s(y)), (s(59), s(y))], fill=(76, 55, 49, 120), width=s(3)
                )
                d.line(
                    [(s(5), s(y - 1)), (s(59), s(y - 1))],
                    fill=(133, 71, 49, 90),
                    width=SS,
                )
            for x in range(10, 60, 10):
                d.line(
                    [(s(x), s(18)), (s(x), s(46))], fill=(47, 43, 45, 100), width=s(2)
                )
        elif variant == 1:
            # A riveted repair plate, inset into rather than sitting on the floor.
            pts = [(12, 17), (48, 12), (55, 43), (18, 51)]
            d.polygon([(s(x), s(y)) for x, y in pts], fill=(66, 51, 49, 72))
            d.line(
                [(s(x), s(y)) for x, y in [*pts, pts[0]]],
                fill=(137, 70, 48, 105),
                width=s(2),
            )
            for x, y in pts:
                d.ellipse(
                    [s(x - 1), s(y - 1), s(x + 1), s(y + 1)], fill=(173, 102, 70, 130)
                )
        else:
            # Oil soaked into oxidized concrete.
            for _ in range(5):
                cx, cy = rng.randrange(18, 47), rng.randrange(18, 47)
                rx, ry = rng.randrange(6, 14), rng.randrange(3, 9)
                d.ellipse(
                    [s(cx - rx), s(cy - ry), s(cx + rx), s(cy + ry)],
                    fill=(24, 20, 22, 42),
                )
            d.arc(
                [s(15), s(19), s(50), s(48)],
                12,
                196,
                fill=(127, 65, 45, 85),
                width=s(2),
            )
    elif theme == "cold_circuitry":
        if variant == 0:
            # An exposed trace with low, flush contact pads.
            pts = [(7, 42), (22, 42), (22, 19), (43, 19), (43, 51), (58, 51)]
            d.line([(s(x), s(y)) for x, y in pts], fill=(92, 160, 174, 115), width=s(2))
            for x, y in (pts[0], pts[-1], (22, 19), (43, 51)):
                d.rectangle(
                    [s(x - 2), s(y - 2), s(x + 2), s(y + 2)], fill=(138, 193, 202, 125)
                )
        elif variant == 1:
            # A recessed cooling grille.
            d.rounded_rectangle(
                [s(12), s(18), s(52), s(46)],
                radius=s(4),
                fill=(30, 43, 50, 75),
                outline=(96, 133, 144, 105),
                width=s(2),
            )
            for x in range(18, 50, 7):
                d.line([(s(x), s(23)), (s(x), s(41))], fill=(15, 25, 31, 115), width=SS)
        else:
            # Hairline panel seams and cold status marks.
            pts = [(6, 27), (18, 15), (48, 15), (58, 25), (48, 49), (18, 49), (6, 37)]
            d.line([(s(x), s(y)) for x, y in pts], fill=(76, 111, 124, 85), width=SS)
            d.line(
                [(s(18), s(49)), (s(18), s(33)), (s(37), s(33))],
                fill=(114, 180, 190, 95),
                width=SS,
            )
    elif theme == "quarry_dust":
        if variant == 0:
            # Twin vehicle ruts, broken so they do not resemble walls.
            for y in (22, 42):
                for x in range(5, 58, 12):
                    d.line(
                        [(s(x), s(y)), (s(x + 7), s(y))],
                        fill=(104, 88, 67, 92),
                        width=s(3),
                    )
                    d.line(
                        [(s(x), s(y - 1)), (s(x + 7), s(y - 1))],
                        fill=(150, 130, 96, 55),
                        width=SS,
                    )
        elif variant == 1:
            # Saw scoring left in a worked stone floor.
            for offset in (-7, 0, 7):
                d.arc(
                    [s(9 + offset), s(10), s(55 + offset), s(55)],
                    205,
                    320,
                    fill=(126, 108, 82, 92),
                    width=s(2),
                )
        else:
            # Shallow drill-test dimples, not a loose pile.
            for angle in range(0, 360, 45):
                rad = math.radians(angle)
                x, y = 32 + math.cos(rad) * 17, 32 + math.sin(rad) * 12
                d.ellipse(
                    [s(x - 2), s(y - 2), s(x + 2), s(y + 2)], fill=(82, 72, 62, 105)
                )
                d.point((s(x - 0.5), s(y - 0.5)), fill=(173, 151, 112, 85))
    elif theme == "basalt":
        if variant == 0:
            # A branching cooled fracture.
            pts = [(5, 47), (19, 39), (27, 25), (39, 29), (57, 12)]
            d.line([(s(x), s(y)) for x, y in pts], fill=(20, 20, 29, 150), width=s(2))
            d.line([(s(27), s(25)), (s(23), s(11))], fill=(71, 67, 91, 80), width=SS)
            d.line([(s(39), s(29)), (s(50), s(43))], fill=(71, 67, 91, 70), width=SS)
        elif variant == 1:
            # A glassy seam with a faint mineral edge.
            pts = [(3, 38), (15, 31), (25, 34), (38, 24), (60, 29)]
            d.line([(s(x), s(y)) for x, y in pts], fill=(15, 16, 23, 120), width=s(5))
            d.line([(s(x), s(y - 2)) for x, y in pts], fill=(86, 76, 112, 65), width=SS)
        else:
            # Interlocking lava joints: broad geometry, no raised boulders.
            joints = [
                (8, 31),
                (18, 15),
                (38, 13),
                (54, 28),
                (45, 49),
                (23, 52),
                (8, 31),
            ]
            d.line([(s(x), s(y)) for x, y in joints], fill=(70, 65, 91, 82), width=s(2))
            d.line(
                [(s(18), s(15)), (s(27), s(32)), (s(23), s(52))],
                fill=(24, 23, 34, 115),
                width=SS,
            )
            d.line([(s(54), s(28)), (s(27), s(32))], fill=(24, 23, 34, 100), width=SS)
    elif theme == "slag":
        if variant == 0:
            # A thin plate of vitrified slag fused to the floor.
            pts = [(10, 19), (39, 11), (55, 26), (48, 48), (21, 53), (7, 36)]
            d.polygon([(s(x), s(y)) for x, y in pts], fill=(35, 28, 31, 62))
            d.line(
                [(s(x), s(y)) for x, y in [*pts, pts[0]]],
                fill=(120, 63, 62, 75),
                width=SS,
            )
            d.line(
                [(s(18), s(43)), (s(34), s(29)), (s(48), s(35))],
                fill=(157, 72, 61, 58),
                width=SS,
            )
        elif variant == 1:
            # Heat bloom baked into the surface.
            for rx, ry, alpha in ((23, 14, 30), (16, 9, 40), (9, 5, 48)):
                d.ellipse(
                    [s(32 - rx), s(32 - ry), s(32 + rx), s(32 + ry)],
                    outline=(137, 56, 52, alpha),
                    width=s(3),
                )
        else:
            # Parallel runoff channels, recessed and discontinuous.
            for offset in (-9, 0, 9):
                pts = [
                    (7, 18 + offset),
                    (23, 25 + offset),
                    (38, 22 + offset),
                    (57, 34 + offset),
                ]
                d.line(
                    [(s(x), s(y)) for x, y in pts], fill=(45, 31, 34, 105), width=s(2)
                )
    elif theme == "verdigris":
        if variant == 0:
            # Copper panel seams with oxidized edges.
            pts = [(9, 18), (31, 11), (55, 22), (49, 48), (22, 53), (9, 39), (9, 18)]
            d.line([(s(x), s(y)) for x, y in pts], fill=(74, 126, 111, 95), width=s(2))
            for x, y in pts[:-1:2]:
                d.ellipse(
                    [s(x - 1), s(y - 1), s(x + 1), s(y + 1)], fill=(142, 94, 60, 105)
                )
        elif variant == 1:
            # A buried conduit with flush junction caps.
            pts = [(4, 43), (18, 43), (27, 27), (46, 27), (58, 15)]
            d.line([(s(x), s(y)) for x, y in pts], fill=(119, 76, 52, 105), width=s(3))
            d.line(
                [(s(x), s(y - 1)) for x, y in pts], fill=(69, 153, 132, 105), width=SS
            )
            for x, y in ((18, 43), (46, 27)):
                d.ellipse(
                    [s(x - 3), s(y - 3), s(x + 3), s(y + 3)], fill=(48, 106, 94, 105)
                )
        else:
            # Patina leaching through the floor in an irregular bloom.
            for _ in range(7):
                cx, cy = rng.randrange(16, 49), rng.randrange(16, 49)
                rx, ry = rng.randrange(4, 11), rng.randrange(3, 8)
                d.ellipse(
                    [s(cx - rx), s(cy - ry), s(cx + rx), s(cy + ry)],
                    fill=(58, 151, 130, 24 + rng.randrange(0, 25)),
                )
    else:
        raise ValueError(f"unknown theme {theme!r}")

    alpha = img.getchannel("A").point(
        lambda a: min(235, round(a * 1.55 + 12)) if a else 0
    )
    img.putalpha(alpha)
    finish(img, px, f"theme_{theme}_{variant}")


def scrap_rich() -> None:
    """A dense, tall heap — the 'S' legend's double-value node."""
    scrap_pile("scrap_rich", seed=23, pieces=30, spread=19, lift=7)


def muzzle_flash() -> None:
    px = 32
    img, d = canvas(px)
    for r, alpha in [(11, 90), (7, 170), (4, 255)]:
        d.ellipse(
            [s(16 - r), s(16 - r), s(16 + r), s(16 + r)], fill=(255, 240, 200, alpha)
        )
    d.polygon(
        [(s(16), s(2)), (s(19), s(13)), (s(13), s(13))], fill=(255, 240, 200, 220)
    )
    d.polygon(
        [(s(16), s(30)), (s(19), s(19)), (s(13), s(19))], fill=(255, 240, 200, 220)
    )
    finish(img, px, "muzzle_flash")


def scorch() -> None:
    px = 128
    img, d = canvas(px)
    rng = random.Random(77)
    for r, alpha in [(56, 60), (44, 90), (30, 120)]:
        d.ellipse(
            [s(64 - r), s(64 - r), s(64 + r), s(64 + r)], fill=(12, 10, 10, alpha)
        )
    for _ in range(14):
        ang = rng.uniform(0, 6.28318)
        import math as _m

        fr = rng.uniform(30, 60)
        x, y = 64 + fr * _m.cos(ang), 64 + fr * _m.sin(ang)
        d.ellipse([s(x - 3), s(y - 3), s(x + 3), s(y + 3)], fill=(16, 14, 12, 90))
    finish(img, px, "scorch")


ICON = 48


def _icon_bar(d, a, b, w: float, color) -> None:
    """A thick line as a polygon — PIL's line joins are ragged at 4x."""
    import math as _m

    (x0, y0), (x1, y1) = a, b
    dx, dy = x1 - x0, y1 - y0
    ln = _m.hypot(dx, dy) or 1.0
    px, py = -dy / ln * w * 0.5, dx / ln * w * 0.5
    d.polygon(
        [
            (s(x0 + px), s(y0 + py)),
            (s(x1 + px), s(y1 + py)),
            (s(x1 - px), s(y1 - py)),
            (s(x0 - px), s(y0 - py)),
        ],
        fill=color,
    )


def icon_stop() -> None:
    """Everything halts: one solid block."""
    img, d = canvas(ICON)
    d.rectangle([s(13), s(13), s(35), s(35)], fill=(*BONE, 255))
    finish(img, ICON, "icon_stop")


def icon_move() -> None:
    """A plain march: arrow up, no opinions."""
    img, d = canvas(ICON)
    d.polygon([(s(24), s(6)), (s(40), s(24)), (s(8), s(24))], fill=(*BONE, 255))
    d.rectangle([s(19), s(24), s(29), s(42)], fill=(*BONE, 255))
    finish(img, ICON, "icon_move")


def icon_attack_move() -> None:
    """The fighting march: the move arrow wearing blades."""
    img, d = canvas(ICON)
    d.polygon([(s(24), s(4)), (s(40), s(22)), (s(8), s(22))], fill=(*BONE, 255))
    d.rectangle([s(19), s(22), s(29), s(40)], fill=(*BONE, 255))
    # Blades off the arrowhead.
    d.polygon([(s(4), s(28)), (s(14), s(24)), (s(12), s(33))], fill=(*SCRAP_LIGHT, 255))
    d.polygon(
        [(s(44), s(28)), (s(34), s(24)), (s(36), s(33))], fill=(*SCRAP_LIGHT, 255)
    )
    finish(img, ICON, "icon_attack_move")


def icon_attack() -> None:
    """A strike burst: eight points of contact."""
    img, d = canvas(ICON)
    cx, cy = 24, 24
    import math as _m

    pts = []
    for i in range(16):
        ang = _m.pi * 2 * i / 16 - _m.pi / 2
        r = 19 if i % 2 == 0 else 8
        pts.append((s(cx + _m.cos(ang) * r), s(cy + _m.sin(ang) * r)))
    d.polygon(pts, fill=(*BONE, 255))
    d.ellipse([s(19), s(19), s(29), s(29)], fill=(*SCRAP, 255))
    finish(img, ICON, "icon_attack")


def icon_patrol() -> None:
    """A closed round: the loop with its arrowhead."""
    img, d = canvas(ICON)
    for a, b in [
        ((12, 10), (36, 10)),
        ((38, 12), (38, 36)),
        ((36, 38), (18, 38)),
        ((10, 36), (10, 12)),
    ]:
        _icon_bar(d, a, b, 5.0, (*BONE, 255))
    # Arrowhead riding the bottom leg, pointing the way around.
    d.polygon([(s(12), s(38)), (s(22), s(30)), (s(22), s(46))], fill=(*BONE, 255))
    finish(img, ICON, "icon_patrol")


def icon_harvest() -> None:
    """The scrap pyramid: what the economy is made of."""
    img, d = canvas(ICON)
    for x, y in [(14, 14), (26, 14), (8, 26), (20, 26), (32, 26)]:
        d.rectangle([s(x), s(y), s(x + 9), s(y + 9)], fill=(*SCRAP, 255))
        d.rectangle([s(x), s(y), s(x + 9), s(y + 3)], fill=(*SCRAP_LIGHT, 255))
    finish(img, ICON, "icon_harvest")


def icon_build() -> None:
    """A wrench over the work."""
    img, d = canvas(ICON)
    d.ellipse([s(6), s(6), s(24), s(24)], fill=(*BONE, 255))
    d.ellipse([s(11), s(11), s(19), s(19)], fill=(0, 0, 0, 0))
    # The jaw notch.
    d.polygon([(s(20), s(4)), (s(30), s(14)), (s(20), s(20))], fill=(0, 0, 0, 0))
    _icon_bar(d, (17, 17), (38, 38), 7.5, (*BONE, 255))
    d.rectangle([s(33), s(33), s(43), s(43)], fill=(*BONE, 255))
    finish(img, ICON, "icon_build")


def icon_repair() -> None:
    """The weld: a plus, sparking."""
    img, d = canvas(ICON)
    d.rectangle([s(19), s(9), s(29), s(39)], fill=(*BONE, 255))
    d.rectangle([s(9), s(19), s(39), s(29)], fill=(*BONE, 255))
    d.ellipse([s(33), s(7), s(41), s(15)], fill=(*SCRAP_LIGHT, 255))
    finish(img, ICON, "icon_repair")


def icon_salvage() -> None:
    """Value coming back down: an arrow into the pile."""
    img, d = canvas(ICON)
    d.rectangle([s(19), s(4), s(29), s(20)], fill=(*BONE, 255))
    d.polygon([(s(24), s(32)), (s(38), s(18)), (s(10), s(18))], fill=(*BONE, 255))
    for x in (10, 20, 30):
        d.rectangle([s(x), s(36), s(x + 8), s(44)], fill=(*SCRAP, 255))
    finish(img, ICON, "icon_salvage")


def icon_cancel() -> None:
    """A refusal: the cross."""
    img, d = canvas(ICON)
    _icon_bar(d, (11, 11), (37, 37), 8.0, (*BONE, 255))
    _icon_bar(d, (37, 11), (11, 37), 8.0, (*BONE, 255))
    finish(img, ICON, "icon_cancel")


def icon_rally() -> None:
    """The rally pennant on its pole."""
    img, d = canvas(ICON)
    d.rectangle([s(12), s(6), s(17), s(42)], fill=(*BONE, 255))
    d.polygon([(s(17), s(8)), (s(42), s(15)), (s(17), s(24))], fill=(*SCRAP, 255))
    finish(img, ICON, "icon_rally")


def icon_idle() -> None:
    """Standing by: the three-beat wait."""
    img, d = canvas(ICON)
    for i, x in enumerate((8, 20, 32)):
        d.ellipse([s(x), s(21), s(x + 8), s(29)], fill=(*BONE, 200 - i * 30))
    finish(img, ICON, "icon_idle")


THEMES = (
    "rusted_yard",
    "cold_circuitry",
    "quarry_dust",
    "basalt",
    "slag",
    "verdigris",
)

BUILDING_STEMS = (
    "foundry",
    "turret",
    "fabricator",
    "flak_turret",
    "bastion",
    "array",
    "reclaimer",
    "repair_bay",
)


def construction_site_frame(
    stem: str,
    faction: str,
    stage: int,
    phase: int,
) -> None:
    """Builds one physical assembly frame from the final hull.

    The renderer selects the progress stage and trolley phase.  The actual
    structure rises bottom-up beneath a compact gantry; no translucent final
    silhouette or generic lattice is painted over the site.
    """
    source = REGISTRY[f"{stem}_{faction}"]
    width, height = source.size
    scale = width / 64
    out = Image.new("RGBA", source.size, (0, 0, 0, 0))

    def p(value: float) -> int:
        return round(value * scale)

    # Reveal the real authored hull from its foundation upward.  Each band
    # has a slightly different lift height so the leading edge reads as
    # individual panels being installed, not a rectangular crop wipe.
    reveal_y = (43, 27, 10)[stage]
    mask = Image.new("L", source.size, 0)
    md = ImageDraw.Draw(mask)
    band = max(1, width // 8)
    edge_steps = (2, 0, 3, 1, 1, 3, 0, 2)
    for index, lift in enumerate(edge_steps):
        left = index * band
        right = width if index == len(edge_steps) - 1 else (index + 1) * band
        top = min(height, p(reveal_y + lift))
        md.rectangle([left, top, right, height], fill=255)
    assembled = source.copy()
    assembled.putalpha(ImageChops.multiply(source.getchannel("A"), mask))
    out.alpha_composite(assembled)
    d = ImageDraw.Draw(out)

    dark = (*IRON_DARK, 245)
    beam = (*IRON_LIGHT, 245)
    pal = FACTIONS[faction]
    accent = (*pal["light"], 255)

    stroke = max(2, p(2))
    left_post = p(6)
    right_post = width - p(6)
    rail_y = p(8)
    floor_y = height - p(6)
    # A restrained permanent-looking build rig: two legs, one overhead rail,
    # and a foundation skid.  It frames the work without hiding the machine's
    # own silhouette.
    d.line([(left_post, rail_y), (left_post, floor_y)], fill=dark, width=stroke)
    d.line([(right_post, rail_y), (right_post, floor_y)], fill=dark, width=stroke)
    d.line([(left_post, rail_y), (right_post, rail_y)], fill=beam, width=stroke)
    d.line(
        [(p(8), floor_y), (width - p(8), floor_y)],
        fill=dark,
        width=max(3, p(3)),
    )
    # Short foot braces make the supports structural without recreating the
    # old full-face X lattice.
    d.line(
        [(left_post, floor_y), (p(13), floor_y - p(7))],
        fill=beam,
        width=max(1, p(1)),
    )
    d.line(
        [(right_post, floor_y), (width - p(13), floor_y - p(7))],
        fill=beam,
        width=max(1, p(1)),
    )

    trolley_x = p(20 if phase == 0 else 44)
    d.rounded_rectangle(
        [trolley_x - p(4), rail_y - p(3), trolley_x + p(4), rail_y + p(4)],
        radius=max(1, p(1)),
        fill=accent,
    )
    cable_end = p((36, 29, 20)[stage] + phase * 2)
    d.line(
        [(trolley_x, rail_y + p(3)), (trolley_x, cable_end)],
        fill=beam,
        width=max(1, p(1)),
    )
    # The carried hull plate is the moving part.  It shortens as the build
    # nears completion, reading as a cap rather than an abstract hook.
    plate_half_w = p((7, 6, 4)[stage])
    plate_h = p((5, 4, 3)[stage])
    d.rounded_rectangle(
        [
            trolley_x - plate_half_w,
            cable_end,
            trolley_x + plate_half_w,
            cable_end + plate_h,
        ],
        radius=max(1, p(1)),
        fill=dark,
        outline=accent,
        width=max(1, p(1)),
    )
    d.line(
        [
            (trolley_x - plate_half_w + p(2), cable_end + plate_h // 2),
            (trolley_x + plate_half_w - p(2), cable_end + plate_h // 2),
        ],
        fill=beam,
        width=max(1, p(1)),
    )

    name = f"{stem}_{faction}_site{stage}_{phase}"
    out.save(OUT / f"{name}.png")
    REGISTRY[name] = out
    print(f"  {name}.png")


def generate(output: Path) -> None:
    """Generates the complete sprite directory at `output`."""
    global OUT
    OUT = output
    REGISTRY.clear()
    OUT.mkdir(parents=True, exist_ok=True)
    print(f"writing {OUT}")
    for i in range(6):
        ground(i)
    for i in range(4):
        rock(i)
    rock_skirt()
    for mask in range(16):
        for variant in range(2):
            peak_barrier(mask, variant)
    decal("decal_crack", 41, "crack")
    decal("decal_plate", 42, "plate")
    decal("decal_stain", 43, "stain")
    decal("decal_wreck", 44, "wreck")
    for theme in THEMES:
        for variant in range(6):
            theme_prop(theme, variant)
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
        for work in range(1, 4):
            foundry(faction, work)
        harvester(faction)
        harvester(faction, dig=1)
        harvester(faction, dig=2)
        harvester(faction, tread=1)
        harvester(faction, tread=2)
        sentinel(faction)
        sentinel(faction, move=1)
        sentinel(faction, move=2)
        scuttler(faction)
        scuttler(faction, move=1)
        scuttler(faction, move=2)
        lancer(faction)
        lancer(faction, move=1)
        lancer(faction, move=2)
        bombard(faction)
        bombard(faction, move=1)
        bombard(faction, move=2)
        flakhound(faction)
        flakhound(faction, tread=1)
        flakhound(faction, tread=2)
        stinger(faction)
        stinger(faction, move=1)
        stinger(faction, move=2)
        buzzard(faction)
        darter(faction)
        talon(faction)
        wisp(faction)
        turret(faction)
        turret_barrel(faction)
        fabricator(faction)
        for work in range(1, 4):
            fabricator(faction, work)
        flak_turret(faction)
        flak_mount(faction)
        bastion(faction)
        bastion_mount(faction)
        array(faction)
        for work in range(1, 4):
            array(faction, work)
        reclaimer(faction)
        for work in range(1, 4):
            reclaimer(faction, work)
        repair_bay(faction)
        for work in range(1, 4):
            repair_bay(faction, work)
    _install_finalized_sprite_bank()
    for faction in FACTIONS:
        for stem in BUILDING_STEMS:
            for stage in range(3):
                for phase in range(2):
                    construction_site_frame(stem, faction, stage, phase)
    _install_finalized_construction_bank()
    accent_masks()
    icon_stop()
    icon_move()
    icon_attack_move()
    icon_attack()
    icon_patrol()
    icon_harvest()
    icon_build()
    icon_repair()
    icon_salvage()
    icon_cancel()
    icon_rally()
    icon_idle()
    pack_atlas()
    print("done")


def check_reproducible() -> None:
    """Regenerates out of tree and compares every committed asset byte."""
    committed = Path(__file__).resolve().parent.parent / "assets" / "sprites"
    with tempfile.TemporaryDirectory(prefix="oxide-sprite-check-") as temp:
        generated = Path(temp) / "sprites"
        generate(generated)
        expected_files = {
            p.name: p.read_bytes() for p in committed.iterdir() if p.is_file()
        }
        actual_files = {
            p.name: p.read_bytes() for p in generated.iterdir() if p.is_file()
        }
    missing = sorted(expected_files.keys() - actual_files.keys())
    extra = sorted(actual_files.keys() - expected_files.keys())
    changed = sorted(
        name
        for name in expected_files.keys() & actual_files.keys()
        if expected_files[name] != actual_files[name]
    )
    if missing or extra or changed:
        raise SystemExit(
            "generated sprites differ from the committed source of truth: "
            f"missing={missing}, extra={extra}, changed={changed}"
        )
    print(f"reproducible: {len(actual_files)} files match byte-for-byte")


def main() -> None:
    import argparse

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="regenerate in a temporary directory and compare committed bytes",
    )
    args = parser.parse_args()
    if args.check:
        check_reproducible()
    else:
        generate(Path(__file__).resolve().parent.parent / "assets" / "sprites")


if __name__ == "__main__":
    main()
