"""Housed Extractor cutter and its connected ore discharge."""

import math

from PIL import Image

from tools import gen_sprites as gen
from tools.production_sprite_sources import extractor_reclaimer_final as original
from tools.production_sprite_sources.installed_defenses_final import bolt, plate
from tools.production_sprite_sources.specialists_final import (
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


def render_extractor(faction: str, phase: int) -> Image.Image:
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    plate(d, (7, 9, 121, 121), DEEP, 8)
    for x, y in ((16, 18), (112, 18), (16, 111), (112, 111)):
        plate(d, (x - 7, y - 6, x + 7, y + 7), DARK, 3)
        bolt(d, x, y)
    box(d, (29, 27, 101, 100), VOID, 5)
    original._belt(d, (53, 72, 76, 106), phase, faction=faction)
    for j in range(3):
        y = 77 + (j * 9 + phase * 3) % 25
        poly(d, [(58, y), (63, y - 2), (68, y + 2), (63, y + 5)], gen.SCRAP_DARK)
        line(d, [(59, y), (63, y - 1)], gen.SCRAP)
    for x in (50, 79):
        line(d, [(x, 77), (x, 108)], IRON, 4)
        line(d, [(x - 1, 78), (x - 1, 106)], EDGE)
    circle(d, (32, 27, 101, 95), VOID)
    circle(d, (33, 22, 100, 87), IRON)
    circle(d, (38, 27, 95, 82), DARK)
    circle(d, (44, 33, 89, 76), VOID)
    for n in range(6):
        a = n * math.tau / 6 + phase * math.pi / 12
        ux, uy = (math.cos(a), math.sin(a))
        vx, vy = (-uy, ux)
        pts = [
            (66 + ux * r + vx * w, 54 + uy * r + vy * w)
            for r, w in ((8, -3), (22, -4), (24, 2), (8, 4))
        ]
        poly(d, pts, IRON)
        line(d, [pts[0], pts[1]], EDGE)
        poly(
            d,
            [
                (66 + ux * r + vx * w, 54 + uy * r + vy * w)
                for r, w in ((18, -4), (24, -3), (24, 2), (18, 3))
            ],
            STEEL,
        )
    for x in (19, 101):
        plate(d, (x - 7, 31, x + 9, 91), DARK, 3)
        box(d, (x - 3, 38, x + 5, 55), paint, 1)
        line(d, [(x + 1, 65), (x + 1, 82)], STEEL, 2)
    plate(d, (38, 11, 91, 32), DARK, 4)
    box(d, (46, 16, 67, 27), paint, 2)
    vent(d, 71, 15, 13)
    plate(d, (43, 27, 90, 56), DARK, 5)
    plate(d, (52, 31, 78, 62), IRON, 4)
    box(d, (56, 35, 74, 46), DEEP, 2)
    line(d, [(59, 37), (71, 37)], EDGE)
    for x in (49, 84):
        bolt(d, x, 44)
    circle(d, (58, 49, 75, 66), VOID)
    circle(d, (61, 51, 72, 62), IRON)
    bolt(d, 66, 56)
    for x in (36, 87):
        plate(d, (x, 56, x + 9, 76), DARK, 3)
        line(d, [(x + 2, 60), (x + 2, 71)], EDGE)
    plate(d, (43, 78, 86, 90), DARK, 3)
    box(d, (53, 82, 77, 92), VOID, 1)
    for x in (47, 82):
        bolt(d, x, 84)
    original._hopper(d, (42, 103, 87, 119), faction)
    poly(d, [(51, 113), (59, 109), (64, 112), (73, 110), (79, 115)], gen.SCRAP_DARK)
    line(d, [(54, 112), (59, 111), (63, 113)], gen.SCRAP)
    for x in (24, 94):
        box(d, (x, 111, x + 11, 114), paint, 1)
    return finish(im)
