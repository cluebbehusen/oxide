"""Coordinated machine study using the Extractor's open industrial construction."""

from tools import gen_sprites as gen
from tools.production_sprite_sources import extractor_reclaimer_final as metal

S = 4
VOID = metal.VOID
DEEP = metal.IRON_DEEP
IRON = gen.IRON
DARK = gen.IRON_DARK
LIGHT = gen.IRON_LIGHT
EDGE = (112, 111, 119)


def box(draw, bounds, color, radius=0):
    draw.rounded_rectangle(
        tuple(round(v * S) for v in bounds),
        radius=round(radius * S),
        fill=(*color, 255),
    )


def line(draw, points, color, width=1):
    draw.line(
        [(round(x * S), round(y * S)) for x, y in points],
        fill=(*color, 255),
        width=round(width * S),
    )


def polygon(draw, points, color):
    draw.polygon([(round(x * S), round(y * S)) for x, y in points], fill=(*color, 255))


def plate(draw, bounds, color=IRON, radius=3):
    x0, y0, x1, y1 = bounds
    box(draw, bounds, DARK, radius)
    box(draw, (x0 + 2, y0 + 2, x1 - 2, y1 - 2), color, max(0, radius - 1))
    line(draw, ((x0 + 3, y0 + 2), (x1 - 3, y0 + 2)), LIGHT)


def finish(image):
    return metal._finish(image, 128)


def foundation(draw, faction):
    metal._foundation(draw, 128, faction)
    for x in (16, 96):
        plate(draw, (x, 22, x + 16, 91))
        box(draw, (x + 4, 34, x + 12, 51), gen.FACTIONS[faction]["dark"])


def crusher(draw, faction, phase):
    accent = gen.FACTIONS[faction]["dark"]
    # Hopper -> opposed corrugated jaws -> open outlet.
    polygon(draw, ((33, 20), (95, 20), (86, 49), (42, 49)), DARK)
    polygon(draw, ((38, 24), (90, 24), (82, 43), (46, 43)), VOID)
    line(draw, ((37, 22), (91, 22)), accent, 3)
    line(draw, ((38, 25), (45, 43)), LIGHT, 2)
    line(draw, ((89, 25), (83, 43)), IRON, 2)
    chunk_y = (30, 34, 39, 43)[phase]
    polygon(
        draw,
        ((57, chunk_y), (65, chunk_y - 2), (72, chunk_y + 2), (64, chunk_y + 6)),
        gen.SCRAP_DARK,
    )
    line(draw, ((58, chunk_y), (65, chunk_y - 1)), gen.SCRAP, 1)
    box(draw, (35, 47, 93, 83), VOID, 3)
    for y, direction in ((51, 1), (69, -1)):
        box(draw, (41, y, 87, y + 11), DARK, 4)
        box(draw, (44, y + 2, 84, y + 8), IRON, 2)
        line(draw, ((46, y + 2), (82, y + 2)), LIGHT, 2)
        offset = direction * (0, 2, 4, 6)[phase]
        for x in (49, 61, 73):
            ridge = 46 + (x - 46 + offset) % 36
            line(draw, ((ridge, y + 3), (ridge + 2, y + 7)), EDGE, 2)
        for x in (36, 87):
            plate(draw, (x, y - 1, x + 5, y + 12), accent, 1)
    # The working gap stays dark; only the material advances through it.
    if phase in (1, 2):
        box(draw, (59, 63, 69, 67), gen.SCRAP_DARK, 1)


def render_reclaimer(faction, phase):
    image, draw = metal._new_sprite(128)
    foundation(draw, faction)
    crusher(draw, faction, phase)
    metal._belt(draw, (50, 82, 78, 103), phase, faction=faction)
    metal._hopper(draw, (42, 100, 86, 117), faction)
    polygon(
        draw, ((53, 109), (63, 106), (75, 110), (73, 113), (52, 113)), gen.SCRAP_DARK
    )
    line(draw, ((56, 108), (63, 107)), gen.SCRAP, 2)
    return finish(image)


def render_refinery(faction, phase):
    image, draw = metal._new_sprite(128)
    foundation(draw, faction)
    accent = gen.FACTIONS[faction]["dark"]
    plate(draw, (14, 22, 35, 103), accent)
    plate(draw, (93, 67, 115, 103), accent)
    crusher(draw, faction, phase)
    # The same crusher feeds a heated finishing press and an ingot tray.
    plate(draw, (32, 80, 96, 109), accent)
    box(draw, (39, 85, 89, 105), DEEP, 2)
    box(draw, (44, 87, 84, 95), VOID)
    box(draw, (49, 89, 79, 93), metal.AMBER)
    press = (0, 2, 4, 1)[phase]
    plate(draw, (44, 96 - press, 84, 104 - press), IRON, 1)
    metal._hopper(draw, (43, 105, 85, 119), faction)
    for x in (52, 65):
        box(draw, (x, 111, x + 10, 115), gen.SCRAP_DARK, 1)
        line(draw, ((x + 1, 111), (x + 8, 111)), gen.SCRAP, 1)
    # One side-mounted exhaust identifies the added thermal stage.
    plate(draw, (94, 5, 115, 82), accent)
    box(draw, (96, 9, 113, 34), gen.FACTIONS[faction]["dark"], 2)
    box(draw, (99, 12, 110, 29), VOID, 1)
    line(draw, ((99, 10), (110, 10)), LIGHT, 1)
    for y in (51, 58, 65):
        line(draw, ((99, y), (110, y)), DEEP, 2)
    return finish(image)


def track(draw, x, phase):
    box(draw, (x, 32, x + 16, 107), VOID, 5)
    box(draw, (x + 2, 35, x + 14, 104), DARK, 3)
    for y in range(37, 100, 9):
        at = 37 + (y - 37 + phase * 3) % 63
        box(draw, (x + 3, at, x + 13, at + 4), IRON, 1)
    line(draw, ((x + 3, 38), (x + 3, 100)), LIGHT)


def rocket(draw, nose, tail):
    # 24 source pixels = 0.375 world tiles at the Avalanche's 2-tile draw size.
    polygon(
        draw, ((64, nose), (67, nose + 6), (67, tail), (61, tail), (61, nose + 6)), VOID
    )
    box(draw, (62, nose + 6, 66, tail - 1), (151, 146, 134))
    polygon(draw, ((64, nose + 1), (66, nose + 6), (62, nose + 6)), (210, 199, 171))
    line(draw, ((62, nose + 7), (62, tail - 4)), (189, 183, 167))
    polygon(draw, ((61, tail - 6), (59, tail), (63, tail - 1)), LIGHT)
    polygon(draw, ((67, tail - 6), (69, tail), (65, tail - 1)), DARK)


def render_avalanche(faction, phase=0, action=0):
    image, draw = metal._new_sprite(128)
    accent = gen.FACTIONS[faction]["dark"]
    track(draw, 23, phase)
    track(draw, 89, phase)
    polygon(draw, ((40, 39), (88, 39), (96, 56), (91, 102), (37, 102), (32, 56)), VOID)
    polygon(draw, ((42, 42), (86, 42), (92, 56), (87, 99), (41, 99), (36, 56)), IRON)
    line(draw, ((42, 43), (85, 43)), LIGHT, 2)
    for x in (39, 79):
        plate(draw, (x, 48, x + 10, 79), accent, 2)
    plate(draw, (40, 85, 57, 97), DEEP, 2)
    for y in (89, 93):
        line(draw, ((44, y), (53, y)), LIGHT)
    plate(draw, (73, 85, 88, 97), accent, 2)
    # The circular pivot, two lifting struts, and recessed rail are separate parts.
    draw.ellipse(metal._box((43, 51, 85, 91)), fill=(*DEEP, 255))
    draw.ellipse(metal._box((49, 57, 79, 85)), fill=(*DARK, 255))
    for a, b in (((49, 69), (44, 90)), ((79, 69), (84, 90))):
        line(draw, (a, b), VOID, 6)
        line(draw, (a, b), LIGHT, 3)
    chassis = finish(image)
    image, draw = metal._new_sprite(128)
    extension = 3 if action in (1, 2) else 0
    plate(draw, (53, 18 - extension, 75, 94), DEEP, 3)
    box(draw, (59, 21 - extension, 69, 88), VOID, 1)
    line(draw, ((56, 22 - extension), (56, 84)), LIGHT, 2)
    line(draw, ((72, 22 - extension), (72, 84)), IRON, 2)
    for y in (47, 72, 86):
        line(draw, ((60, y), (68, y)), DARK, 2)
    plate(draw, (51, 77, 77, 85), accent, 1)
    if action in (0, 1):
        rocket(draw, 18, 42)
    elif action == 4:
        rocket(draw, 48, 72)
    chassis.alpha_composite(image.resize((128, 128), metal.Image.Resampling.LANCZOS))
    return chassis
