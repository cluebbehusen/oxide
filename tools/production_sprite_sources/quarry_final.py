"""Approved quarry floor, salvage sites, surface dressing, and layered stone."""

import math
import random
from pathlib import Path

from PIL import Image

from tools import gen_sprites as gen
from tools.production_sprite_sources.installed_defenses_final import bolt, plate
from tools.production_sprite_sources.specialists_final import (
    DARK,
    DEEP,
    IRON,
    VOID,
    box,
    canvas,
    circle,
    finish,
    line,
    poly,
)


def ground(control, phase):
    image = control.copy()
    lifts = [-6, -3, 0, 2, 4, 7]

    def weights(coordinate):
        distance = min(coordinate + 0.5, 63.5 - coordinate)
        t = min(distance / 6, 1.0)
        neighbor = 0.5 * (1 - t * t * (3 - 2 * t))
        return [(0, 1 - neighbor), (-1 if coordinate < 32 else 1, neighbor)]

    for y in range(64):
        for x in range(64):
            blended = sum(
                lifts[(phase + dx - dy) % 6] * wx * wy
                for dx, wx in weights(x)
                for dy, wy in weights(y)
            )
            shift = round(blended - lifts[phase])
            r, g, b, a = control.getpixel((x, y))
            image.putpixel((x, y), (r + shift, g + shift, b + shift, a))
    return image


def extractor_site():
    im, d = canvas()
    for x, y in ((18, 23), (67, 23), (18, 68), (67, 68)):
        plate(d, (x, y, x + 44, y + 42), (48, 46, 48), 4)
        line(d, [(x + 8, y + 4), (x + 13, y + 11), (x + 10, y + 16)], DEEP)
    plate(d, (38, 37, 92, 94), VOID, 12)
    circle(d, (40, 37, 90, 87), IRON)
    circle(d, (45, 42, 85, 82), DEEP)
    circle(d, (50, 47, 80, 77), VOID)
    circle(d, (52, 54, 78, 77), DARK)
    box(d, (59, 80, 72, 108), VOID, 1)
    line(d, [(62, 87), (64, 96), (68, 102)], (130, 105, 66), 2)
    line(d, [(75, 27), (72, 33)], VOID, 2)
    for x, y in ((23, 27), (103, 27), (23, 104), (103, 104)):
        plate(d, (x - 8, y - 6, x + 8, y + 7), DARK, 3)
        bolt(d, x, y)
    return finish(im, False)


def scrap(stage):
    fullness = {"full": 1.0, "mid": 0.55, "low": 0.25}.get(stage, 1)
    rich = stage == "rich"
    pieces = 30 if rich else int(14 * fullness) + 4
    spread = 19 if rich else 10 * fullness + 6
    lift = 7 if rich else 3 * fullness
    im, d = gen.canvas(64)
    rng = random.Random(23 if rich else 11)
    radius = spread + 5
    d.ellipse(
        [
            gen.s(32 - radius),
            gen.s(36 - radius * 0.6),
            gen.s(32 + radius),
            gen.s(36 + radius * 0.6),
        ],
        fill=(24, 20, 16, 120),
    )
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
    for i, (dist, cx, cy, w, h) in enumerate(placed):
        tone = (
            rng.choice((gen.SCRAP, gen.SCRAP_LIGHT))
            if dist < spread * 0.45
            else rng.choice((gen.SCRAP, gen.SCRAP_DARK, gen.SCRAP_DARK))
        )
        dx = rng.uniform(-2.5, 2.5)
        pts = [
            (cx - w / 2 - dx, cy - h / 2),
            (cx + w / 2, cy - h / 2 + dx / 2),
            (cx + w / 2 + dx, cy + h / 2),
            (cx - w / 2, cy + h / 2 - dx / 2),
        ]
        d.polygon([(gen.s(x), gen.s(y)) for x, y in pts], fill=(*tone, 255))
        d.line(
            [(gen.s(x), gen.s(y)) for x, y in pts[1:]],
            fill=(100, 73, 33, 255),
            width=gen.SS,
        )
        if i % 3 == 0:
            d.line(
                [(gen.s(x), gen.s(y)) for x, y in pts[:2]],
                fill=(227, 189, 113, 255),
                width=gen.SS,
            )
        if i % 5 == 1:
            d.line(
                [
                    (gen.s(cx - w * 0.2), gen.s(cy - h * 0.15)),
                    (gen.s(cx + w * 0.2), gen.s(cy + h * 0.1)),
                ],
                fill=(106, 84, 43, 255),
                width=gen.SS,
            )
    for _ in range(max(1, pieces // 6)):
        cx, cy = 32 + rng.uniform(-6, 6), 32 - lift * 0.7 + rng.uniform(-4, 4)
        d.rectangle(
            [gen.s(cx - 1.2), gen.s(cy - 0.8), gen.s(cx + 1.2), gen.s(cy + 0.8)],
            fill=(242, 215, 157, 255),
        )
    return im.resize((64, 64), Image.Resampling.LANCZOS)


def dressing(index):
    im, d = gen.canvas(64)
    rng = random.Random(3809 + index * 177)

    def ln(points, color, width=1):
        d.line(
            [(gen.s(x), gen.s(y)) for x, y in points], fill=color, width=gen.s(width)
        )

    if index < 4:
        for layer in range(3):
            points = []
            for j in range(12):
                a = j * math.tau / 12
                r = rng.uniform(9, 19) * (1 - layer * 0.13)
                points.append(
                    (gen.s(32 + math.cos(a) * r), gen.s(32 + math.sin(a) * r * 0.6))
                )
            d.polygon(points, fill=(31 + layer * 2, 25 + layer, 25, 55 + layer * 12))
        ln([(24, 36), (31, 37), (39, 33)], (65, 46, 31, 55))
    elif index < 8:
        x, y = rng.randrange(14, 22), rng.randrange(16, 23)
        w = rng.randrange(23, 32)
        h = rng.randrange(15, 24)
        d.rectangle(
            [gen.s(x), gen.s(y), gen.s(x + w), gen.s(y + h)], fill=(15, 16, 20, 160)
        )
        ln([(x, y + h), (x, y), (x + w, y)], (77, 73, 69, 160))
        for xx in range(x + 4, x + w - 2, 5):
            ln([(xx, y + 3), (xx, y + h - 3)], (57, 57, 61, 185), 2)
        ln([(x + w - 3, y + h), (x + w + 2, y + h - 3)], (104, 68, 43, 130), 2)
    else:
        pts = [(9, 42), (18, 38), (22, 25), (37, 23), (46, 31), (55, 28)]
        pts = [(x, y + rng.randrange(-4, 5)) for x, y in pts]
        ln(pts, (12, 13, 17, 200), 3)
        ln([(x, y - 1) for x, y in pts], (78, 65, 56, 180))
        for x, y in [pts[1], pts[-2]]:
            ln([(x - 2, y - 3), (x + 2, y + 2)], (89, 78, 61, 190), 2)
    return im.resize((64, 64), Image.Resampling.LANCZOS)


def rock():
    im, d = gen.canvas(64)

    def polygon(points, color):
        d.polygon([(gen.s(x), gen.s(y)) for x, y in points], fill=(*color, 255))

    polygon(
        [(7, 36), (15, 20), (28, 10), (43, 16), (53, 33), (51, 48), (36, 55), (16, 51)],
        (28, 27, 32),
    )
    polygon(
        [(10, 35), (18, 19), (29, 12), (43, 18), (49, 34), (35, 42), (20, 39)],
        (79, 76, 79),
    )
    polygon([(20, 39), (35, 42), (49, 34), (49, 46), (35, 53), (19, 48)], (44, 42, 48))
    polygon([(13, 32), (21, 22), (31, 20), (40, 26), (35, 32), (22, 31)], (94, 89, 87))
    polygon([(31, 20), (36, 14), (44, 20), (49, 33), (39, 36), (40, 26)], (62, 60, 66))
    for points in [
        [(18, 39), (34, 43), (46, 37)],
        [(20, 44), (34, 48), (45, 42)],
        [(23, 28), (31, 27), (38, 31)],
    ]:
        d.line(
            [(gen.s(x), gen.s(y)) for x, y in points],
            fill=(29, 29, 34, 255),
            width=gen.SS,
        )
    for pts in [
        [(8, 47), (11, 39), (17, 41), (20, 50), (13, 54)],
        [(46, 52), (48, 46), (57, 46), (59, 51), (54, 57)],
    ]:
        polygon(pts, (60, 56, 59))
    return im.resize((64, 64), Image.Resampling.LANCZOS)


def source_frames(controls: dict[str, Image.Image]):
    for phase in range(6):
        key = f"ground_{phase}"
        yield key, ground(controls[key], phase)
    yield "extractor_frame", extractor_site()
    for stage in ("full", "mid", "low", "rich"):
        yield f"scrap_{stage}", scrap(stage)
    yield "rock_0", rock()
    for index in range(12):
        yield f"quarry_dressing_{index}", dressing(index)


def install_quarry(registry: dict[str, Image.Image], out: Path) -> None:
    for key, image in source_frames(registry):
        registry[key] = image
        image.save(out / f"{key}.png")
