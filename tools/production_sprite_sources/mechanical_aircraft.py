"""Combat and transport aircraft in the accepted quarry-machine materials."""

import math

from tools import gen_sprites as gen
from tools.production_sprite_sources.mechanical_ground import (
    DARK,
    DEEP,
    EDGE,
    IRON,
    STEEL,
    VOID,
    box,
    canvas,
    circle,
    finish,
    line,
    poly,
    vent,
)

HEAT = (169, 104, 52)
FLASH = (236, 190, 112)


def panel(d, points, paint):
    poly(d, points, VOID)
    cx = sum(p[0] for p in points) / len(points)
    cy = sum(p[1] for p in points) / len(points)
    inner = [(cx + (x - cx) * 0.88, cy + (y - cy) * 0.88) for x, y in points]
    poly(d, inner, paint)
    line(d, inner[:2], EDGE)


def strut(d, a, b, width=6):
    line(d, [a, b], VOID, width + 3)
    line(d, [a, b], IRON, width)
    for x, y in (a, b):
        circle(d, (x - 2, y - 2, x + 2, y + 2), STEEL)


def fan(d, x, y, r, phase, paint, heavy=True):
    circle(d, (x - r, y - r, x + r, y + r), VOID)
    circle(d, (x - r + 2, y - r + 2, x + r - 2, y + r - 2), IRON if heavy else DARK)
    circle(d, (x - r + 5, y - r + 5, x + r - 5, y + r - 5), DEEP)
    for i in range(4):
        a = (i * math.pi / 2) + (phase * math.pi / 6)

        def pt(rad, angle):
            return (x + rad * math.cos(angle), y + rad * math.sin(angle))

        poly(
            d,
            [pt(3, a), pt(r - 6, a + 0.2), pt(r - 6, a + 0.48), pt(4, a + 0.65)],
            IRON,
        )
    for side in (-1, 1):
        box(d, (x + side * (r - 2) - 1, y - 3, x + side * (r - 2) + 1, y + 3), paint)
    circle(d, (x - 4, y - 4, x + 4, y + 4), VOID)
    circle(d, (x - 2, y - 2, x + 2, y + 2), STEEL)


def engine(d, x, y, width, height, paint, phase):
    box(d, (x, y, x + width, y + height), VOID, 4)
    box(d, (x + 2, y + 2, x + width - 2, y + height - 2), DARK, 3)
    box(d, (x + 5, y + 3, x + width - 5, y + 8), DEEP, 1)
    line(d, [(x + 4, y + 2), (x + width - 4, y + 2)], EDGE)
    box(d, (x + 5, y + 12, x + width - 5, y + height - 13), paint, 2)
    box(d, (x + 4, y + height - 10, x + width - 4, y + height - 3), DEEP, 1)
    for i in range(3):
        xx = x + 5 + i * (width - 10) / 2
        line(
            d,
            [(xx, y + height - 8), (xx, y + height - 5)],
            EDGE if (i + phase) % 2 else IRON,
        )


def gun(d, x, y, length, width, action):
    recoil = (0, 0, 4, 2, 0)[action]
    box(d, (x - width, y + length - 12, x + width, y + length + 6), VOID, 2)
    box(d, (x - width + 2, y + length - 10, x + width - 2, y + length + 3), IRON, 1)
    box(d, (x - width / 2 - 1, y + recoil, x + width / 2 + 1, y + length), VOID, 1)
    box(d, (x - width / 2 + 1, y + recoil + 2, x + width / 2 - 1, y + length - 2), DARK)
    line(
        d,
        [(x - width / 2 + 1, y + recoil + 3), (x - width / 2 + 1, y + length - 4)],
        EDGE,
    )
    box(d, (x - width / 2 - 1, y + recoil, x + width / 2 + 1, y + recoil + 3), IRON)
    if action == 2:
        poly(
            d,
            [
                (x - 3, y + recoil - 1),
                (x - 2, y + recoil - 5),
                (x, y + recoil - 9),
                (x + 2, y + recoil - 5),
                (x + 3, y + recoil - 1),
            ],
            FLASH,
        )
    if action in (1, 3):
        box(d, (x - 2, y + length - 8, x + 2, y + length - 5), HEAT)


def sensor(d, x, y, paint):
    box(d, (x - 5, y - 5, x + 5, y + 4), VOID, 2)
    box(d, (x - 3, y - 3, x + 3, y + 1), paint, 1)
    line(d, [(x - 3, y - 3), (x + 1, y - 3)], EDGE)


def buzzard(faction, move=0, action=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    for x, y in ((25, 40), (103, 40), (25, 94), (103, 94)):
        strut(d, (64, 64 if y < 64 else 84), (x, y), 7)
        fan(d, x, y, 20, move or action, paint)
    panel(
        d,
        [
            (45, 26),
            (54, 20),
            (74, 20),
            (83, 26),
            (84, 104),
            (73, 113),
            (55, 113),
            (44, 104),
        ],
        DARK,
    )
    for x in (46, 76):
        box(d, (x, 36, x + 7, 89), paint, 2)
        line(d, [(x + 1, 40), (x + 1, 59)], EDGE)
    box(d, (54, 59, 74, 88), VOID, 2)
    for y in (64, 70, 76):
        box(d, (57, y, 71, y + 3), IRON, 1)
    vent(d, 54, 92, 20)
    base = finish(im)
    im, d = canvas()
    gun(d, 64, 12, 48, 10, action)
    base.alpha_composite(finish(im, False))
    return base


def wisp(faction, move=0, action=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    for i, (x, y) in enumerate(((34, 38), (94, 38), (37, 89), (91, 89))):
        strut(d, (64, 61 if y < 64 else 75), (x, y), 3)
        fan(d, x, y, 12, (move or action) + i % 2, paint, False)
    panel(
        d, [(54, 46), (64, 38), (74, 46), (75, 85), (68, 92), (60, 92), (53, 85)], DARK
    )
    box(d, (58, 68, 70, 85), paint, 2)
    box(d, (60, 72, 68, 81), DEEP, 1)
    sensor(d, 64, 59, paint)
    base = finish(im)
    im, d = canvas()
    gun(d, 64, 34, 20, 5, action)
    base.alpha_composite(finish(im, False))
    return base


def darter(faction, move=0, action=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    for side in (-1, 1):
        panel(
            d,
            [
                (64 + side * 7, 39),
                (64 + side * 48, 86),
                (64 + side * 42, 93),
                (64 + side * 12, 72),
            ],
            paint,
        )
        line(d, [(64 + side * 16, 57), (64 + side * 34, 80)], IRON, 2)
        engine(d, 64 + side * 23 - 9, 74, 18, 35, paint, move)
    panel(d, [(64, 12), (73, 37), (75, 92), (64, 112), (53, 92), (55, 37)], DARK)
    panel(d, [(64, 26), (68, 45), (68, 88), (64, 98), (60, 88), (60, 45)], paint)
    sensor(d, 64, 47, IRON)
    base = finish(im)
    im, d = canvas()
    gun(d, 64, 14, 23, 5, action)
    base.alpha_composite(finish(im, False))
    return base


def talon(faction, move=0, action=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    for side in (-1, 1):
        panel(
            d,
            [
                (64 + side * 13, 39),
                (64 + side * 47, 58),
                (64 + side * 51, 84),
                (64 + side * 24, 91),
                (64 + side * 12, 73),
            ],
            DARK,
        )
        panel(
            d,
            [
                (64 + side * 21, 49),
                (64 + side * 39, 60),
                (64 + side * 35, 73),
                (64 + side * 18, 65),
            ],
            paint,
        )
        engine(d, 64 + side * 34 - 11, 62, 22, 41, paint, move)
        panel(
            d, [(64 + side * 9, 96), (64 + side * 24, 111), (64 + side * 6, 107)], IRON
        )
    panel(d, [(54, 39), (74, 39), (79, 81), (71, 111), (57, 111), (49, 81)], DARK)
    box(d, (57, 71, 71, 94), paint, 2)
    sensor(d, 64, 63, paint)
    vent(d, 56, 96, 16)
    base = finish(im)
    im, d = canvas()
    for side in (-1, 1):
        x = 64 + side * 12
        box(d, (x - 5, 22, x + 5, 57), VOID, 2)
        box(d, (x - 3, 24, x + 3, 52), IRON, 1)
        box(d, (x - 2, 25, x + 2, 41), paint)
        line(d, [(x - 3, 26), (x - 3, 48)], EDGE)
    gun(d, 64, 19, 34, 5, action)
    base.alpha_composite(finish(im, False))
    return base


def shrike(faction, move=0, action=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    for side in (-1, 1):
        panel(
            d,
            [
                (64 + side * 8, 28),
                (64 + side * 53, 51),
                (64 + side * 55, 70),
                (64 + side * 24, 73),
                (64 + side * 11, 85),
            ],
            DARK,
        )
        panel(
            d,
            [
                (64 + side * 16, 42),
                (64 + side * 46, 56),
                (64 + side * 46, 64),
                (64 + side * 21, 62),
            ],
            paint,
        )
        engine(d, 64 + side * 25 - 12, 68, 24, 43, paint, move)
        panel(
            d, [(64 + side * 13, 96), (64 + side * 26, 119), (64 + side * 5, 109)], IRON
        )
    panel(d, [(64, 9), (76, 28), (76, 94), (64, 112), (52, 94), (52, 28)], DARK)
    panel(d, [(64, 23), (69, 35), (69, 78), (59, 78), (59, 35)], paint)
    sensor(d, 64, 47, IRON)
    box(d, (59, 83, 69, 97), VOID, 1)
    base = finish(im)
    im, d = canvas()
    gun(d, 64, 12, 25, 7, action)
    for side in (-1, 1):
        x = 64 + side * 26
        box(d, (x - 8, 55, x + 8, 63), VOID, 2)
        box(d, (x - 5, 57, x + 5, 60), IRON, 1)
    base.alpha_composite(finish(im, False))
    return base


def sylph(faction, move=0, action=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    for side in (-1, 1):
        panel(
            d,
            [
                (64 + side * 9, 48),
                (64 + side * 41, 27),
                (64 + side * 37, 52),
                (64 + side * 20, 81),
                (64 + side * 8, 82),
            ],
            paint,
        )
        line(d, [(64 + side * 15, 57), (64 + side * 32, 42)], IRON, 2)
        engine(d, 64 + side * 15 - 8, 74, 16, 38, paint, move)
        panel(
            d,
            [
                (64 + side * 5, 95),
                (64 + side * 31, 108),
                (64 + side * 34, 116),
                (64 + side * 8, 108),
            ],
            DARK,
        )
    panel(d, [(64, 7), (71, 35), (72, 92), (64, 108), (56, 92), (57, 35)], DARK)
    box(d, (61, 46, 67, 85), paint, 1)
    sensor(d, 64, 38, IRON)
    base = finish(im)
    im, d = canvas()
    gun(d, 64, 10, 19, 5, action)
    base.alpha_composite(finish(im, False))
    return base


def skyhook(faction, move=0, action=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    for x in (25, 103):
        strut(d, (64, 43), (x, 34), 9)
        fan(d, x, 34, 22, move or action, paint)
    panel(
        d,
        [
            (45, 24),
            (55, 17),
            (73, 17),
            (83, 24),
            (85, 96),
            (78, 112),
            (50, 112),
            (43, 96),
        ],
        DARK,
    )
    for x in (46, 75):
        box(d, (x, 45, x + 7, 101), paint, 2)
        line(d, [(x + 1, 49), (x + 1, 92)], EDGE)
    vent(d, 55, 27, 18)
    box(d, (55, 48, 73, 104), VOID, 2)
    gap = (0, 4, 7, 3, 0)[action]
    box(d, (56 - gap, 49, 63 - gap, 102), IRON, 1)
    box(d, (65 + gap, 49, 72 + gap, 102), DARK, 1)
    line(d, [(58 - gap, 52), (58 - gap, 97)], EDGE)
    for y in (67, 93):
        for side in (-1, 1):
            reach = (0, 6, 10, 4, 0)[action]
            a = (64 + side * 19, y)
            b = (64 + side * (32 + reach), y + 8)
            strut(d, a, b, 5)
            x, yy = b
            line(d, [(x, yy - 2), (x, yy + 5), (x - side * 5, yy + 5)], HEAT, 3)
            circle(d, (a[0] - 3, a[1] - 3, a[0] + 3, a[1] + 3), STEEL)
    return finish(im)
