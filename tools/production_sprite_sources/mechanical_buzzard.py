"""Independent gun carriage on the accepted Buzzard quadcopter chassis."""

from tools import gen_sprites as gen
from tools.production_sprite_sources.mechanical_aircraft import FLASH, fan, panel, strut
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


def hull(faction, move=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    for x, y in ((25, 40), (103, 40), (25, 94), (103, 94)):
        strut(d, (64, 64 if y < 64 else 84), (x, y), 7)
        fan(d, x, y, 20, move, paint)
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
    circle(d, (41, 41, 87, 87), VOID)
    circle(d, (44, 44, 84, 84), IRON)
    circle(d, (47, 47, 81, 81), DEEP)
    for x, y in ((64, 44), (44, 64), (84, 64), (64, 84)):
        box(d, (x - 2, y - 2, x + 2, y + 2), EDGE, 1)
    return finish(im)


def mount(faction, action=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    poly(
        d,
        [
            (51, 49),
            (75, 49),
            (82, 58),
            (82, 77),
            (74, 84),
            (53, 84),
            (46, 76),
            (46, 59),
        ],
        VOID,
    )
    poly(
        d,
        [
            (53, 52),
            (73, 52),
            (78, 60),
            (78, 74),
            (72, 80),
            (55, 80),
            (50, 74),
            (50, 61),
        ],
        DARK,
    )
    line(d, [(51, 60), (55, 53), (72, 53)], EDGE, 2)
    box(d, (54, 65, 74, 77), paint, 2)
    box(d, (78, 57, 90, 76), VOID, 2)
    box(d, (81, 59, 88, 73), paint, 1)
    for y in (61, 65, 69):
        line(d, [(79, y), (84, y)], IRON, 2)
    barrel(d, action)
    line(d, [(54, 60), (54, 67)], STEEL, 2)
    line(d, [(74, 60), (74, 67)], STEEL, 2)
    return finish(im, False)


def buzzard(faction, move=0, action=0):
    image = hull(faction, move)
    image.alpha_composite(mount(faction, action))
    return image


def barrel(d, action):
    x, y, length, width = 64, 20, 43, 10
    recoil = (0, 0, 6, 3, 0)[action]
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
