"""Production and support machinery in the accepted Oxide material family."""

import math

from PIL import Image

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

BRASS = (130, 105, 66)
HOT = (187, 109, 53)
TEAL = (85, 125, 123)


def plate(d, bounds, fill=DARK, cut=5):
    x, y, r, b = bounds
    pts = [
        (x + cut, y),
        (r - cut, y),
        (r, y + cut),
        (r, b - cut),
        (r - cut, b),
        (x + cut, b),
        (x, b - cut),
        (x, y + cut),
    ]
    poly(d, pts, VOID)
    poly(
        d,
        [
            (x + cut, y + 2),
            (r - cut, y + 2),
            (r - 2, y + cut),
            (r - 2, b - cut),
            (r - cut, b - 2),
            (x + cut, b - 2),
            (x + 2, b - cut),
            (x + 2, y + cut),
        ],
        fill,
    )
    line(
        d,
        [(x + 2, b - cut), (x + 2, y + cut), (x + cut, y + 2), (r - cut, y + 2)],
        EDGE,
    )


def bolt(d, x, y):
    box(d, (x - 2, y - 2, x + 2, y + 2), VOID, 1)
    box(d, (x - 1, y - 1, x + 1, y), STEEL)


def ram(d, a, b, size=5):
    line(d, [a, b], VOID, size + 4)
    line(d, [a, b], IRON, size)
    c = (a[0] + (b[0] - a[0]) * 0.55, a[1] + (b[1] - a[1]) * 0.55)
    line(d, [c, b], STEEL, max(2, size - 2))
    for x, y in (a, b):
        circle(d, (x - 3, y - 3, x + 3, y + 3), VOID)
        circle(d, (x - 2, y - 2, x + 2, y + 2), BRASS)


def foundation(d, bounds=(8, 8, 120, 120)):
    plate(d, bounds, DEEP, 9)
    x, y, r, b = bounds
    for xx, yy in [(x + 8, y + 8), (r - 8, y + 8), (x + 8, b - 8), (r - 8, b - 8)]:
        bolt(d, xx, yy)


def foundry(faction, work=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    # Short foundation feet and a broad rear beam anchor the assembly bed.
    plate(d, (8, 24, 120, 114), DEEP, 9)
    plate(d, (37, 7, 89, 37), DEEP, 5)
    plate(d, (12, 24, 116, 42), DARK, 4)
    for x in (8, 98):
        plate(d, (x, 99, x + 22, 120), IRON, 4)
        for xx in (x + 6, x + 16):
            bolt(d, xx, 113)
    # Recessed assembly bed, side rails, feed magazine and travelling gantry.
    box(d, (34, 27, 96, 113), VOID, 4)
    box(d, (42, 34, 88, 106), DEEP, 3)
    for x in (37, 92):
        line(d, [(x, 30), (x, 110)], IRON, 3)
        line(d, [(x - 1, 31), (x - 1, 108)], EDGE)
    for bounds in [(12, 44, 31, 93), (100, 44, 116, 93)]:
        plate(d, bounds)
    box(d, (15, 49, 28, 68), paint, 2)
    vent(d, 15, 80, 12)
    for y in (50, 71):
        box(d, (103, y, 113, y + 11), VOID, 1)
        line(d, [(105, y + 2), (111, y + 2)], IRON, 2)
    plate(d, (42, 11, 83, 35), paint)
    box(d, (49, 15, 75, 27), VOID, 2)
    for x in (51, 59, 67):
        box(d, (x, 18, x + 5, 26), IRON, 1)
    box(d, (57, 28, 70, 42), DARK, 1)
    line(d, [(59, 30), (59, 40)], BRASS)
    # The workpiece is clamped into a bed, never a pulsing central eye.
    plate(d, (51, 68, 80, 95), IRON, 4)
    box(d, (57, 74, 73, 90), DARK, 2)
    for x in (47, 81):
        box(d, (x, 73, x + 3, 90), BRASS, 1)
    y = {0: 42, 1: 51, 2: 62, 3: 76, 4: 54}[work]
    box(d, (31, y - 5, 99, y + 7), VOID, 2)
    box(d, (34, y - 3, 96, y + 4), paint, 1)
    line(d, [(35, y - 3), (95, y - 3)], EDGE)
    for x in (34, 91):
        box(d, (x, y - 7, x + 5, y + 10), IRON, 1)
    plate(d, (57, y - 7, 74, y + 14), IRON, 3)
    box(d, (61, y + 8, 70, y + 19), BRASS, 1)
    if work in (2, 3):
        line(d, [(65, y + 19), (65, y + 23)], HOT, 2)
    box(d, (47, 108, 83, 119), VOID, 1)
    for x in (48, 80):
        line(d, [(x, 109), (x, 117)], BRASS, 2)
    line(d, [(53, 114), (77, 114)], IRON)
    return finish(im)


def fabricator(faction, work=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    # Offset tool tower and horizontal feed explain the asymmetric footprint.
    poly(
        d,
        [
            (11, 10),
            (45, 10),
            (52, 17),
            (52, 33),
            (111, 33),
            (121, 44),
            (121, 92),
            (110, 103),
            (103, 115),
            (16, 115),
            (9, 108),
            (9, 17),
        ],
        VOID,
    )
    poly(
        d,
        [
            (14, 13),
            (43, 13),
            (49, 19),
            (49, 36),
            (109, 36),
            (118, 46),
            (118, 90),
            (107, 101),
            (100, 112),
            (18, 112),
            (12, 106),
            (12, 19),
        ],
        DEEP,
    )
    line(d, [(12, 106), (12, 19), (17, 13), (43, 13)], EDGE)
    plate(d, (14, 12, 46, 109), paint, 6)
    plate(d, (19, 28, 40, 56), DARK, 3)
    vent(d, 22, 33, 14)
    box(d, (23, 73, 38, 99), VOID, 2)
    for y in (77, 84, 91):
        line(d, [(25, y), (36, y)], BRASS, 3)
    box(d, (47, 37, 111, 100), VOID, 3)
    for x in (52, 106):
        line(d, [(x, 44), (x, 102)], IRON, 3)
    plate(d, (61, 62, 97, 86), DEEP, 4)
    plate(d, (70, 66, 90, 83), IRON, 3)
    box(d, (70, 73, 90, 77), paint)
    # Two opposing jaws press a billet; a side arm transfers it to the exit.
    shift = {0: 0, 1: 3, 2: 8, 3: 4, 4: 0}[work]
    for y, tip in ((39 + shift, 57 + shift), (103 - shift, 85 - shift)):
        box(d, (61, min(y, tip), 99, max(y, tip)), VOID, 2)
        box(d, (65, min(y, tip) + 2, 95, max(y, tip) - 2), IRON, 1)
        line(d, [(67, tip), (93, tip)], STEEL, 2)
    joint = (53, 57 if work in (0, 4) else 63)
    ram(d, (37, 61), joint, 5)
    ram(d, joint, (67, 69 if work != 3 else 88), 4)
    plate(d, (99, 46, 125, 90), DARK, 4)
    for y in (52, 65, 78):
        box(d, (113, y, 122, y + 7), paint, 1)
    for x in (68, 79, 90):
        line(d, [(x, 108), (x + 4, 112)], BRASS, 2)
    return finish(im)


def airworks(faction, work=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    # Split hangar roof retracts sideways above a recessed launch channel.
    foundation(d, (9, 12, 119, 119))
    box(d, (34, 33, 94, 117), VOID, 3)
    box(d, (43, 43, 85, 113), DEEP, 2)
    for y in (79, 92, 105):
        line(d, [(61, y), (67, y)], IRON, 2)
    plate(d, (18, 17, 110, 34), DARK, 4)
    for x in (29, 91):
        box(d, (x - 6, 21, x + 6, 29), VOID, 1)
        line(d, [(x - 4, 24), (x + 4, 24)], BRASS if work in (2, 3, 4) else IRON, 2)
    # Long roof ribs retain broad quiet surfaces; hinges give the doors weight.
    gap = {0: 0, 1: 0, 2: 0, 3: 10, 4: 24}[work]
    for side in (-1, 1):
        inner = 64 + side * (2 + gap)
        outer = 64 + side * (40 + gap * 0.2)
        lo, hi = sorted((inner, outer))
        poly(
            d,
            [
                (lo, 45),
                (lo + 5, 35),
                (hi - 4, 35),
                (hi, 46),
                (hi, 99),
                (hi - 9, 109),
                (lo, 99),
            ],
            VOID,
        )
        poly(
            d,
            [
                (lo + 2, 46),
                (lo + 7, 38),
                (hi - 5, 38),
                (hi - 2, 47),
                (hi - 2, 97),
                (hi - 10, 105),
                (lo + 2, 97),
            ],
            DARK,
        )
        line(d, [(lo + 5, 48), (lo + 5, 96)], EDGE)
        line(d, [(lo + 8, 41), (hi - 7, 41)], paint, 4)
        x = outer - side * 3
        for y in (57, 86):
            box(d, (x - 3, y - 4, x + 3, y + 4), IRON, 1)
    for x in (28, 100):
        box(d, (x - 4, 108, x + 4, 117), VOID, 1)
        line(d, [(x - 2, 110), (x + 2, 110)], BRASS if work >= 3 else IRON, 2)
    return finish(im)


def crucible(faction, work=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    # A pressure vessel in a square buttress, with a rear heat exchanger.
    foundation(d)
    plate(d, (18, 13, 110, 38), DARK, 5)
    for x in range(26, 105, 10):
        line(d, [(x, 19), (x, 31)], IRON, 3)
    for x in (15, 94):
        plate(d, (x, 47, x + 19, 108), paint, 4)
        for y in (59, 84):
            bolt(d, x + 9, y)
    circle(d, (29, 33, 100, 105), VOID)
    circle(d, (33, 37, 96, 101), IRON)
    circle(d, (40, 44, 89, 94), DEEP)
    circle(d, (47, 51, 82, 87), VOID)
    # Heavy segmented lid retracts to expose a thin, contained hot seam.
    opening = {0: 0, 1: 2, 2: 5, 3: 2}[work]
    for side in (-1, 1):
        x = 64 + side * (3 + opening)
        pts = [
            (x, 52),
            (x + side * 11, 56),
            (x + side * 15, 69),
            (x + side * 11, 82),
            (x, 86),
        ]
        poly(d, pts, DARK)
        line(d, [pts[0], pts[1], pts[2]], EDGE)
    if work:
        line(d, [(64, 59), (64, 79)], HOT, 2)
    for x, y in [(42, 42), (87, 42), (42, 94), (87, 94)]:
        ram(d, (x, y - 4), (x, y + 5), 4)
    plate(d, (45, 104, 85, 117), DARK, 3)
    box(d, (53, 108, 77, 113), VOID, 1)
    for x in (55, 64, 73):
        box(d, (x, 109, x + 3, 111), BRASS)
    return finish(im)


def repair_bay(faction, work=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    # An open service apron between unequal tool cabinets, not another factory box.
    poly(
        d,
        [
            (13, 23),
            (30, 13),
            (99, 13),
            (115, 24),
            (115, 105),
            (97, 114),
            (31, 114),
            (13, 105),
        ],
        VOID,
    )
    poly(
        d,
        [
            (16, 25),
            (31, 17),
            (97, 17),
            (111, 25),
            (111, 103),
            (96, 110),
            (32, 110),
            (16, 103),
        ],
        DEEP,
    )
    plate(d, (19, 26, 41, 103), DARK, 4)
    plate(d, (87, 35, 109, 103), DARK, 4)
    plate(d, (32, 17, 96, 32), paint, 4)
    for bounds in [(22, 33, 37, 52), (91, 43, 105, 64)]:
        box(d, bounds, paint, 2)
    vent(d, 23, 79, 13)
    vent(d, 92, 79, 13)
    # Fine inset guides frame the empty patient space without painting a huge icon.
    for x in (47, 81):
        line(d, [(x, 57), (x, 103)], IRON, 2)
    for x in (49, 77):
        line(d, [(x, 104), (x + 3, 108)], BRASS, 2)
    reach = {0: 0.0, 1: 1.0, 2: 1.0, 3: 1.0, 4: 0.35}[work]
    for x, sign, y in [(31, 1, 43), (98, -1, 54)]:
        # Equal-length links keep the elbow articulated instead of stretching.
        tip = (x + sign * (12 + 15 * reach), y + 28)
        dx, dy = tip[0] - x, tip[1] - y
        distance = math.hypot(dx, dy)
        rise = math.sqrt(23**2 - (distance / 2) ** 2)
        elbow = (
            (x + tip[0]) / 2 + sign * dy / distance * rise,
            (y + tip[1]) / 2 - sign * dx / distance * rise,
        )
        ram(d, (x, y), elbow, 6)
        ram(d, elbow, tip, 5)
        box(d, (tip[0] - 4, tip[1] - 3, tip[0] + 4, tip[1] + 4), DARK, 1)
        line(d, [(tip[0], tip[1] + 2), (tip[0], tip[1] + 7)], BRASS, 3)
        if work in (1, 2, 3):
            contact = (tip[0], tip[1] + 7)
            circle(
                d,
                (contact[0] - 2, contact[1] - 2, contact[0] + 2, contact[1] + 2),
                (228, 186, 110),
            )
            for j in range(3):
                angle = (j * 2.1 + work * 1.3) * sign
                length = (5, 8, 4)[(j + work) % 3]
                a = (contact[0] + math.cos(angle) * 4, contact[1] + math.sin(angle) * 4)
                b = (
                    contact[0] + math.cos(angle) * length,
                    contact[1] + math.sin(angle) * length,
                )
                line(d, [a, b], (196, 137, 64), 1)
    return finish(im)


def array(faction, work=0, tier=0, part=None):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    foundation(d, (17, 19, 111, 110))
    plate(d, (23, 76, 105, 104), DARK, 4)
    box(d, (30, 83, 57, 96), paint, 2)
    vent(d, 73, 81, 23)
    for x, y in ((29, 31), (98, 31), (29, 66), (98, 66)):
        plate(d, (x - 8, y - 7, x + 8, y + 7), IRON, 3)
        bolt(d, x, y)
    circle(d, (41, 26, 88, 73), VOID)
    circle(d, (46, 31, 83, 68), IRON)
    circle(d, (51, 36, 78, 63), DARK)
    box(d, (59, 66, 69, 78), VOID, 2)
    line(d, [(62, 69), (62, 76)], BRASS, 2)
    if tier:
        for x in (18, 100):
            box(d, (x, 38, x + 10, 68), VOID, 2)
            for y in (42, 50, 58):
                line(d, [(x + 2, y), (x + 8, y)], BRASS, 2)
    base = finish(im)
    rotor, d = canvas()
    # A counterweighted slotted aerial rotates mechanically over its bearing.
    box(d, (60, 28, 68, 87), VOID, 2)
    box(d, (62, 32, 66, 81), IRON, 1)
    plate(d, (28, 39, 100, 58), DARK, 4)
    line(d, [(34, 43), (94, 43)], EDGE, 2)
    for x in (35, 47, 59, 71, 83):
        box(d, (x, 47, x + 8, 53), VOID, 1)
        line(d, [(x + 1, 48), (x + 6, 48)], TEAL)
    if tier:
        plate(d, (30, 59, 98, 69), DARK, 3)
        line(d, [(35, 62), (93, 62)], TEAL)
    box(d, (55, 72, 73, 83), paint, 2)
    circle(d, (59, 44, 69, 54), IRON)
    angle = -20 if work == 0 else (work - 1) * 60 - 20
    rotor = finish(rotor, False)
    if part == "base":
        return base
    if part == "rotor":
        return rotor
    rotor = rotor.rotate(angle, Image.Resampling.BICUBIC, center=(64, 49))
    base.alpha_composite(rotor)
    return base
