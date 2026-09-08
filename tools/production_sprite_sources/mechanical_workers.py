"""Five quarry machines using the accepted layered metal vocabulary."""

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
    track,
    vent,
)

SCRAP = (151, 100, 47)
SCRAP_EDGE = (184, 133, 66)


def pin(d, x, y, radius=3):
    circle(d, (x - radius, y - radius, x + radius, y + radius), VOID)
    circle(d, (x - radius + 1, y - radius + 1, x + radius - 1, y + radius - 1), STEEL)


def hopper(d, bounds, paint):
    x0, y0, x1, y1 = bounds
    box(d, bounds, VOID, 3)
    poly(
        d,
        [(x0 + 2, y0 + 2), (x1 - 2, y0 + 2), (x1 - 5, y1 - 3), (x0 + 5, y1 - 3)],
        DEEP,
    )
    line(d, [(x0 + 2, y0 + 2), (x0 + 4, y1 - 3), (x1 - 4, y1 - 3)], IRON, 2)
    box(d, (x0 + 5, y1 - 2, x1 - 5, y1 + 3), paint, 1)


def load(d, level, x0=45, y0=72, width=38, height=28):
    for i in range(level):
        y = y0 + height - 7 - i * 6
        poly(
            d, [(x0 + 3, y + 4), (x0 + 6, y - 2), (x0 + 15, y), (x0 + 14, y + 5)], SCRAP
        )
        poly(
            d,
            [
                (x0 + 17, y + 4),
                (x0 + 20, y - 3),
                (x0 + width - 4, y),
                (x0 + width - 5, y + 5),
            ],
            IRON,
        )
        line(d, [(x0 + 20, y - 2), (x0 + width - 6, y)], SCRAP_EDGE)


def harvester(faction, move=0, work=0, cargo=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    for bounds in ((17, 36, 36, 112), (92, 36, 111, 112)):
        track(d, bounds, move, paint)
    poly(d, [(34, 48), (44, 38), (84, 38), (94, 48), (90, 111), (38, 111)], VOID)
    poly(d, [(38, 50), (46, 43), (82, 43), (90, 50), (85, 106), (43, 106)], DARK)
    for x in (38, 82):
        box(d, (x, 51, x + 8, 89), paint, 2)
        line(d, [(x + 2, 54), (x + 2, 72)], EDGE)
    hopper(d, (44, 67, 84, 101), paint)
    load(d, cargo, 46, 72, 36, 24)
    box(d, (49, 44, 79, 64), DEEP, 3)
    box(d, (53, 47, 75, 61), paint, 2)
    line(d, [(55, 49), (72, 49)], EDGE)
    vent(d, 51, 104, 26)
    base = finish(im)
    im, d = canvas()
    reach = (0, 9, 4)[work]
    gap = (14, 20, 7)[work]
    for side in (-1, 1):
        root = (64 + side * 12, 52)
        elbow = (64 + side * 19, 36 - reach / 2)
        tip = (64 + side * gap, 23 - reach)
        line(d, [root, elbow, tip], VOID, 8)
        line(d, [root, elbow], IRON, 5)
        line(d, [elbow, tip], STEEL, 3)
        pin(d, *root)
        pin(d, *elbow, 2)
        x, y = tip
        poly(
            d,
            [
                (x - side * 2, y - 6),
                (x + side * 10, y - 3),
                (x + side * 10, y + 8),
                (x - side * 5, y + 11),
                (x - side * 5, y + 6),
                (x + side * 3, y + 4),
            ],
            VOID,
        )
        line(
            d,
            [(x + side * 8, y - 2), (x + side * 8, y + 6), (x - side * 3, y + 8)],
            IRON,
            3,
        )
        line(d, [(x - side * 2, y - 5), (x + side * 7, y - 2)], EDGE)
    base.alpha_composite(finish(im, rim=False))
    return base


def excavator(faction, move=0, work=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    for bounds in ((14, 38, 35, 114), (93, 38, 114, 114)):
        track(d, bounds, move, paint)
    poly(d, [(33, 45), (44, 36), (84, 36), (95, 45), (90, 112), (38, 112)], VOID)
    poly(d, [(38, 47), (47, 40), (81, 40), (90, 47), (86, 108), (42, 108)], DARK)
    for x in (36, 84):
        box(d, (x, 52, x + 8, 91), paint, 1)
        line(d, [(x + 2, 54), (x + 2, 78)], EDGE)
    box(d, (51, 39, 77, 65), VOID, 2)
    for y in range(43, 64, 6):
        line(d, [(55, y), (73, y)], IRON, 3)
    hopper(d, (44, 68, 84, 104), paint)
    box(d, (46, 106, 82, 113), paint, 1)
    vent(d, 51, 108, 26)
    base = finish(im)
    im, d = canvas()
    shift = (0, -5, -8, -4, 0)[work]
    for x in (43, 85):
        line(d, [(x, 58), (x, 28 + shift)], VOID, 7)
        line(d, [(x, 55), (x, 38)], IRON, 4)
        line(d, [(x, 38), (x, 25 + shift)], STEEL, 2)
        pin(d, x, 53)
    box(d, (23, 19 + shift, 105, 38 + shift), VOID, 4)
    box(d, (29, 22 + shift, 99, 34 + shift), DARK, 3)
    for i in range(6):
        x = 33 + i * 11
        y = 23 + shift + ((i + work) % 2) * 4
        poly(d, [(x, y), (x + 6, y), (x + 4, y + 6), (x - 2, y + 6)], IRON)
        line(d, [(x, y), (x + 5, y)], EDGE)
    for x in (23, 99):
        box(d, (x, 21 + shift, x + 6, 36 + shift), paint, 2)
    line(d, [(29, 20 + shift), (98, 20 + shift)], EDGE)
    base.alpha_composite(finish(im, rim=False))
    return base


def excavator_cargo(level):
    im, d = canvas()
    load(d, level, 47, 73, 34, 26)
    return finish(im, rim=False)


def tender(faction, move=0, work=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    for bounds in ((17, 40, 36, 113), (92, 40, 111, 113)):
        track(d, bounds, move, paint)
    poly(d, [(33, 47), (44, 37), (82, 37), (94, 50), (87, 112), (41, 112)], VOID)
    poly(d, [(39, 50), (47, 43), (79, 43), (88, 52), (83, 107), (46, 107)], DARK)
    box(d, (44, 47, 69, 70), paint, 3)
    line(d, [(47, 49), (66, 49)], EDGE)
    for x in (48, 60):
        box(d, (x, 52, x + 6, 64), DEEP, 2)
        line(d, [(x + 1, 54), (x + 1, 60)], IRON)
    circle(d, (44, 77, 74, 103), VOID)
    circle(d, (48, 81, 70, 99), IRON)
    circle(d, (52, 84, 66, 97), DEEP)
    pin(d, 59, 90)
    line(d, [(49, 89), (68, 91)], EDGE, 2)
    box(d, (77, 76, 86, 98), paint, 1)
    for y in (81, 88, 95):
        line(d, [(78, y), (84, y)], DARK, 2)
    base = finish(im)
    im, d = canvas()
    paths = [
        [(81, 58), (89, 42), (76, 33)],
        [(81, 58), (83, 32), (69, 22)],
        [(81, 58), (77, 30), (64, 13)],
        [(81, 58), (77, 30), (64, 13)],
        [(81, 58), (83, 32), (69, 22)],
    ]
    arm = paths[work]
    line(d, [(68, 90), (85, 85), (92, 66), arm[1], arm[2]], VOID, 4)
    line(d, [(68, 90), (85, 85), (92, 66), arm[1], arm[2]], IRON, 1)
    line(d, arm, VOID, 9)
    line(d, arm[:2], paint, 5)
    line(d, arm[1:], STEEL, 3)
    for x, y in arm[:2]:
        pin(d, x, y, 3)
    x, y = arm[-1]
    box(d, (x - 4, y - 4, x + 4, y + 6), DEEP, 1)
    line(d, [(x, y), (x, y - 7)], STEEL, 2)
    if work == 3:
        circle(d, (x - 2, y - 10, x + 2, y - 6), SCRAP_EDGE)
        for dx, dy in ((-7, -10), (6, -12), (5, -4)):
            line(d, [(x + dx, y + dy), (x + dx * 1.3, y + dy - 2)], SCRAP_EDGE)
    base.alpha_composite(finish(im, rim=False))
    return base


def leg(d, root, knee, foot):
    line(d, [root, knee, foot], VOID, 7)
    line(d, [root, knee], IRON, 4)
    line(d, [knee, foot], DARK, 5)
    line(d, [(foot[0] - 3, foot[1]), (foot[0] + 3, foot[1])], STEEL, 2)
    pin(d, *knee, 2)


def scuttler(faction, move=0, action=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    for side in (-1, 1):
        for i, y in enumerate((48, 69, 90)):
            shift = (0, 5, -5)[move] * (1 if (i + (side == 1)) % 2 else -1)
            leg(
                d,
                (64 + side * 14, y),
                (64 + side * 30, y - 5 + shift),
                (64 + side * 41, y + 1 + shift),
            )
    poly(
        d,
        [
            (47, 38),
            (55, 33),
            (73, 33),
            (81, 38),
            (79, 96),
            (72, 109),
            (56, 109),
            (49, 96),
        ],
        VOID,
    )
    poly(
        d,
        [
            (51, 43),
            (58, 38),
            (70, 38),
            (77, 43),
            (74, 96),
            (69, 103),
            (59, 103),
            (54, 96),
        ],
        DARK,
    )
    for y, w in ((50, 23), (68, 21), (86, 17)):
        box(d, (64 - w / 2, y, 64 + w / 2, y + 12), paint, 2)
        line(d, [(65 - w / 2, y + 1), (62 + w / 2, y + 1)], EDGE)
    box(d, (59, 38, 69, 43), DEEP, 1)
    base = finish(im)
    im, d = canvas()
    gap = (14, 20, 2, 9, 14)[action]
    for side in (-1, 1):
        root = (64 + side * 13, 47)
        elbow = (64 + side * 25, 31)
        tip = (64 + side * gap, 18)
        line(d, [root, elbow], VOID, 9)
        line(d, [root, elbow], IRON, 5)
        pin(d, *root)
        x, y = tip
        poly(
            d,
            [
                (64 + side * 27, 33),
                (x + side * 9, y - 2),
                (x - side * 2, y - 3),
                (x - side * 5, y + 3),
                (64 + side * 17, 36),
            ],
            VOID,
        )
        line(
            d,
            [(x + side * 7, y), (x - side * 1, y + 1), (64 + side * 17, 34)],
            STEEL,
            2,
        )
    base.alpha_composite(finish(im, rim=False))
    return base


def sapper(faction, move=0, action=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    for side in (-1, 1):
        for i, y in enumerate((45, 91)):
            shift = (0, 5, -5)[move] * (1 if (i + (side == 1)) % 2 else -1)
            spread = 5 if action else 0
            leg(
                d,
                (64 + side * 21, y),
                (64 + side * (31 + spread), y + shift),
                (64 + side * (39 + spread), y + 7 + shift),
            )
            box(
                d,
                (
                    64 + side * (39 + spread) - 5,
                    y + 3 + shift,
                    64 + side * (39 + spread) + 5,
                    y + 12 + shift,
                ),
                IRON,
                2,
            )
    poly(
        d,
        [
            (43, 31),
            (85, 31),
            (91, 45),
            (87, 103),
            (77, 114),
            (51, 114),
            (41, 103),
            (37, 45),
        ],
        VOID,
    )
    poly(
        d,
        [
            (47, 36),
            (81, 36),
            (86, 47),
            (82, 100),
            (74, 108),
            (54, 108),
            (46, 100),
            (42, 47),
        ],
        DARK,
    )
    box(d, (48, 45, 80, 97), DEEP, 4)
    for x in (50, 66):
        box(d, (x, 48, x + 12, 92), paint, 3)
        line(d, [(x + 2, 51), (x + 9, 51)], EDGE)
    for y in (57, 82):
        box(d, (45, y, 83, y + 7), VOID, 1)
        line(d, [(47, y + 2), (80, y + 2)], IRON, 3)
    box(d, (57, 98, 71, 104), VOID, 1)
    box(d, (60, 100, 68, 102), SCRAP_EDGE if action else DARK)
    base = finish(im)
    im, d = canvas()
    reach = 4 if action else 0
    for x in (51, 77):
        line(d, [(x, 44), (x, 24 - reach)], VOID, 6)
        line(d, [(x, 41), (x, 25 - reach)], IRON, 3)
        box(d, (x - 6, 18 - reach, x + 6, 27 - reach), DEEP, 2)
        line(d, [(x - 4, 20 - reach), (x + 4, 20 - reach)], STEEL, 2)
    box(d, (57, 30, 71, 43), IRON, 2)
    line(d, [(61, 33), (67, 33)], SCRAP_EDGE if action else EDGE)
    base.alpha_composite(finish(im, rim=False))
    return base
