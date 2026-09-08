"""Bounded reduction of the original open-frame economy machines."""

import math

from tools import gen_sprites as gen
from tools.production_sprite_sources import extractor_reclaimer_final as original


def render_extractor(faction, phase):
    image, draw = original._new_sprite(128)
    box, points, scale = original._box, original._points, original._s
    palette = gen.FACTIONS[faction]
    original._foundation(draw, 128, faction)
    # A driven cutter in a well, carried by two supports, feeds one discharge.
    for bounds in ((16, 22, 34, 87), (92, 22, 110, 87)):
        original._plate(draw, bounds, fill=gen.IRON)
        x0, y0, x1, _ = bounds
        draw.rectangle(
            box((x0 + 4, y0 + 9, x1 - 4, y0 + 19)), fill=(*palette["dark"], 255)
        )
    # The casing sits above the discharge belt, with a deep lower skirt.
    draw.ellipse(box((29, 25, 101, 96)), fill=(*original.VOID, 255))
    draw.ellipse(box((30, 23, 98, 92)), fill=(*gen.IRON_DARK, 255))
    draw.arc(
        box((30, 23, 98, 92)), 5, 175, fill=(*original.IRON_DEEP, 255), width=scale(5)
    )
    draw.ellipse(box((30, 17, 98, 85)), fill=(*gen.IRON, 255))
    draw.arc(
        box((31, 18, 97, 84)), 185, 300, fill=(*gen.IRON_LIGHT, 255), width=scale(2)
    )
    draw.ellipse(box((35, 22, 93, 80)), fill=(*palette["dark"], 255))
    draw.arc(
        box((36, 23, 92, 79)), 8, 172, fill=(*original.IRON_DEEP, 255), width=scale(3)
    )
    draw.ellipse(box((42, 30, 86, 74)), fill=(*original.VOID, 255))
    for arm in range(4):
        angle = arm * math.tau / 4 + phase * math.pi / 8
        inner = (64 + 7 * math.cos(angle), 52 + 7 * math.sin(angle))
        outer = (64 + 22 * math.cos(angle), 52 + 22 * math.sin(angle))
        original._strut(draw, inner, outer, color=gen.IRON_LIGHT, width=4)
    draw.ellipse(box((56, 44, 72, 60)), fill=(*gen.IRON_DARK, 255))
    draw.ellipse(box((60, 48, 68, 56)), fill=(*gen.IRON, 255))
    original._belt(draw, (53, 85, 75, 99), phase, faction=faction)
    original._hopper(draw, (42, 94, 86, 116), faction)
    draw.polygon(
        points(((52, 103), (62, 100), (72, 104), (76, 109), (51, 109))),
        fill=(*gen.SCRAP_DARK, 255),
    )
    draw.line(points(((55, 103), (62, 102))), fill=(*gen.SCRAP, 255), width=scale(2))
    original._plate(draw, (14, 37, 30, 64), fill=original.IRON_DEEP, radius=2)
    draw.rectangle(box((19, 43, 25, 55)), fill=(*palette["dark"], 255))
    return original._finish(image, 128)
