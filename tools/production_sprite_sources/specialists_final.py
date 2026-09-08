"""Canonical artillery, anti-air, and continuously scanning scout frames."""

from PIL import Image, ImageDraw

from tools import gen_sprites as gen

SS = 4


SIZE = 128


VOID = (11, 11, 15)


DEEP = (24, 25, 31)


DARK = gen.IRON_DARK


IRON = gen.IRON


EDGE = (112, 111, 119)


STEEL = (143, 140, 135)


def canvas():
    image = Image.new("RGBA", (SIZE * SS, SIZE * SS))
    return image, ImageDraw.Draw(image)


def box(d, bounds, color, radius=0):
    d.rounded_rectangle(
        tuple(round(v * SS) for v in bounds),
        radius=round(radius * SS),
        fill=(*color, 255),
    )


def line(d, points, color, width=1):
    d.line(
        [(round(x * SS), round(y * SS)) for x, y in points],
        fill=(*color, 255),
        width=round(width * SS),
    )


def poly(d, points, color):
    d.polygon([(round(x * SS), round(y * SS)) for x, y in points], fill=(*color, 255))


def circle(d, bounds, color):
    d.ellipse(tuple(round(v * SS) for v in bounds), fill=(*color, 255))


def finish(image, rim=True):
    image = image.resize((SIZE, SIZE), Image.Resampling.LANCZOS)
    return gen.rim_light(image) if rim else image


def track(d, bounds, phase, paint):
    x0, y0, x1, y1 = bounds
    box(d, bounds, VOID, 5)
    box(d, (x0 + 2, y0 + 2, x1 - 2, y1 - 2), DARK, 3)
    box(d, (x0 + 5, y0 + 4, x1 - 5, y1 - 4), DEEP, 2)
    for y in range(round(y0 + 5), round(y1 - 5), 9):
        offset = (phase * 3) % 9
        if y + offset + 3 < y1 - 3:
            box(d, (x0 + 3, y + offset, x1 - 3, y + offset + 3), IRON, 1)
    line(d, [(x0 + 2, y0 + 6), (x0 + 2, y1 - 7)], EDGE)
    box(d, (x0 + 5, y0 + 6, x1 - 5, y0 + 11), paint, 1)


def vent(d, x, y, width=16):
    box(d, (x, y, x + width, y + 14), VOID, 2)
    for yy in (y + 3, y + 7, y + 11):
        line(d, [(x + 3, yy), (x + width - 3, yy)], IRON)


FLASH = (232, 180, 97)


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


BRASS = (130, 105, 66)


GLASS = (94, 133, 134)


def piston(d, x, top, bottom, recoil=0):
    box(d, (x - 4, top, x + 4, bottom), VOID, 2)
    box(d, (x - 2, top + 2, x + 2, bottom - 2), IRON, 1)
    box(d, (x - 1, top + 2, x + 1, top + 13 + recoil), STEEL)
    box(d, (x - 3, top + 13 + recoil, x + 3, bottom - 2), DARK, 1)


def round_shell(d, x, y):
    poly(
        d,
        [(x - 3, y + 6), (x - 3, y - 4), (x, y - 8), (x + 3, y - 4), (x + 3, y + 6)],
        VOID,
    )
    box(d, (x - 2, y - 3, x + 2, y + 5), BRASS, 1)
    poly(d, [(x - 2, y - 3), (x, y - 7), (x + 2, y - 3)], IRON)
    line(d, [(x - 1, y - 2), (x - 1, y + 3)], EDGE)


def muzzle(d, x, y, width=5):
    poly(
        d,
        [
            (x - width, y),
            (x - width / 2, y - 4),
            (x, y - 11),
            (x + width / 2, y - 4),
            (x + width, y),
        ],
        FLASH,
    )
    poly(d, [(x - 2, y), (x, y - 7), (x + 2, y)], (238, 215, 160))


def bombard(faction, move=0, action=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    recoil = {4: 9, 5: 4, 6: 1}.get(action, 0)
    for bounds in ((13, 46, 34, 112), (94, 46, 115, 112)):
        track(d, bounds, move, paint)
    panel(
        d,
        [
            (33, 47),
            (43, 32),
            (85, 32),
            (95, 47),
            (91, 108),
            (81, 115),
            (47, 115),
            (37, 108),
        ],
        DARK,
    )
    for x in (35, 82):
        box(d, (x, 50, x + 11, 77), paint, 2)
    # Rear ammunition rack, central rammer, and separate service compartment.
    box(d, (38, 82, 53, 106), VOID, 2)
    for y in (89, 101):
        round_shell(d, 46, y)
    box(d, (75, 85, 90, 107), IRON, 2)
    vent(d, 77, 89, 11)
    box(d, (55, 69, 73, 109), VOID, 2)
    for x in (57, 70):
        line(d, [(x, 73), (x, 105)], EDGE)
    if action in (0, 1, 2):
        round_shell(d, 64, 78 if action == 2 else 94)
    rammer = {1: 104, 2: 89, 3: 73}.get(action, 105)
    box(d, (60, rammer, 68, 110), DARK, 1)
    line(d, [(61, rammer + 2), (61, 108)], IRON, 2)
    base = finish(im)
    im, d = canvas()
    # The cradle stays fixed while the short tube slides between its trunnions.
    poly(
        d,
        [
            (43, 43),
            (49, 42),
            (49, 68),
            (79, 68),
            (79, 42),
            (85, 43),
            (85, 72),
            (78, 78),
            (50, 78),
            (43, 72),
        ],
        VOID,
    )
    line(d, [(46, 46), (46, 70), (51, 74), (77, 74), (82, 70), (82, 46)], IRON, 3)
    for x in (42, 86):
        piston(d, x, 57, 92, recoil)
    box(d, (49, 32 + recoil, 79, 63 + recoil), VOID, 5)
    box(d, (52, 34 + recoil, 76, 62 + recoil), DARK, 4)
    line(d, [(53, 37 + recoil), (53, 57 + recoil)], IRON, 2)
    circle(d, (47, 18 + recoil, 81, 46 + recoil), VOID)
    circle(d, (49, 20 + recoil, 79, 44 + recoil), IRON)
    circle(d, (53, 23 + recoil, 75, 40 + recoil), VOID)
    line(
        d,
        [(53, 25 + recoil), (57, 22 + recoil), (67, 21 + recoil), (73, 23 + recoil)],
        EDGE,
        2,
    )
    box(d, (53, 58 + recoil, 75, 66 + recoil), DEEP, 2)
    for x in (46, 82):
        circle(d, (x - 4, 49, x + 4, 59), VOID)
        circle(d, (x - 2, 51, x + 2, 56), STEEL)
    if action == 4:
        muzzle(d, 64, 24 + recoil, 7)
    base.alpha_composite(finish(im, False))
    return base


def wheel(d, x, y, radius, phase, paint):
    circle(d, (x - radius, y - radius, x + radius, y + radius), VOID)
    circle(d, (x - radius + 2, y - radius + 2, x + radius - 2, y + radius - 2), DARK)
    line(d, [(x - radius + 3, y - 2), (x - radius + 3, y + 3)], EDGE)
    offset = (-3, 0, 3)[phase % 3]
    box(d, (x - 3, y + offset - 1, x + 3, y + offset + 1), IRON)
    circle(d, (x - 2, y - 2, x + 2, y + 2), paint)


def aa_tube(d, x, y, length, recoil, width=9, report=False):
    box(
        d,
        (x - width / 2 - 2, y + recoil, x + width / 2 + 2, y + length + recoil),
        VOID,
        2,
    )
    box(
        d,
        (x - width / 2, y + 2 + recoil, x + width / 2, y + length - 2 + recoil),
        DARK,
        1,
    )
    line(
        d,
        [
            (x - width / 2 + 1, y + 4 + recoil),
            (x - width / 2 + 1, y + length - 5 + recoil),
        ],
        EDGE,
    )
    box(d, (x - width / 2 - 1, y + recoil, x + width / 2 + 1, y + 5 + recoil), IRON, 1)
    box(d, (x - width / 2 + 1, y + 1 + recoil, x + width / 2 - 1, y + 3 + recoil), VOID)
    if report:
        muzzle(d, x, y + recoil, 4)


def flakhound(faction, move=0, action=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    for bounds in ((11, 61, 34, 116), (94, 61, 117, 116)):
        track(d, bounds, move, paint)
    for x in (23, 105):
        wheel(d, x, 45, 11, move, paint)
        strut(d, (x, 48), (64, 69), 5)
    panel(
        d,
        [
            (32, 46),
            (44, 32),
            (84, 32),
            (96, 46),
            (90, 107),
            (80, 116),
            (48, 116),
            (38, 107),
        ],
        DARK,
    )
    for x in (35, 79):
        box(d, (x, 76, x + 14, 104), paint, 2)
    box(d, (48, 94, 80, 110), IRON, 2)
    vent(d, 52, 95, 24)
    base = finish(im)
    im, d = canvas()
    circle(d, (35, 39, 93, 88), VOID)
    circle(d, (40, 43, 88, 83), IRON)
    box(d, (41, 54, 87, 83), DARK, 3)
    for side in (-1, 1):
        firing = action == (6 if side < 0 else 7)
        recoil = 6 if firing else (3 if action == 8 else 0)
        for x in (64 + side * 9, 64 + side * 23):
            aa_tube(d, x, 16, 48, recoil, 7, firing)
        x = 64 + side * 30
        box(d, (x - 7, 68, x + 7, 89), VOID, 2)
        box(d, (x - 5, 71, x + 5, 86), IRON, 1)
        strut(d, (x, 78), (64 + side * 17, 69), 4)
        count = (
            4 if action in (0, 5) else min(4, max(0, action - 1)) if action <= 5 else 1
        )
        for index in range(4):
            yy = 73 + index * 3
            line(d, [(x - 3, yy), (x + 3, yy)], BRASS if index < count else DEEP, 2)
    box(d, (58, 72, 70, 85), paint, 2)
    base.alpha_composite(finish(im, False))
    return base


def stinger(faction, move=0, action=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    for x, y, radius in ((29, 85, 10), (99, 85, 10), (64, 110, 8)):
        strut(d, (64, 76 if y == 85 else 93), (x, y), 4)
        wheel(d, x, y, radius, move, paint)
    panel(d, [(64, 47), (87, 73), (79, 98), (49, 98), (41, 73)], DARK)
    panel(d, [(64, 59), (78, 75), (73, 88), (55, 88), (50, 75)], paint)
    box(d, (57, 86, 71, 97), DEEP, 2)
    line(d, [(59, 89), (69, 89)], IRON, 2)
    base = finish(im)
    im, d = canvas()
    recoil = (0, 0, 6, 3, 0)[action]
    box(d, (46, 58, 82, 72), VOID, 3)
    box(d, (49, 60, 79, 69), IRON, 2)
    for x in (53, 75):
        aa_tube(d, x, 23, 37, recoil, 6, action == 2)
        circle(d, (x - 6, 56, x + 6, 69), VOID)
        circle(d, (x - 3, 59, x + 3, 65), IRON)
    line(d, [(53, 68), (60, 77), (68, 77), (75, 68)], EDGE, 2)
    box(d, (60, 66, 68, 74), paint, 1)
    base.alpha_composite(finish(im, False))
    return base


def radar_mount(d, x, y):
    circle(d, (x - 11, y - 11, x + 11, y + 11), VOID)
    circle(d, (x - 9, y - 9, x + 9, y + 9), IRON)
    circle(d, (x - 7, y - 7, x + 7, y + 7), DEEP)
    for dx, dy in ((0, -8), (8, 0), (0, 8), (-8, 0)):
        box(d, (x + dx - 1, y + dy - 1, x + dx + 1, y + dy + 1), DARK)


def scout_radar():
    im, d = canvas()
    box(d, (54, 60, 74, 67), VOID, 2)
    box(d, (55, 61, 73, 65), IRON, 1)
    line(d, [(56, 61), (72, 61)], GLASS)
    line(d, [(56, 64), (72, 64)], DARK)
    box(d, (56, 62, 58, 64), BRASS)
    circle(d, (62, 63, 66, 67), VOID)
    circle(d, (63, 64, 65, 66), STEEL)
    return finish(im, rim=False)


def kestrel(faction, move=0, action=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    panel(
        d,
        [
            (64, 15),
            (109, 49),
            (102, 100),
            (80, 93),
            (64, 103),
            (48, 93),
            (26, 100),
            (19, 49),
        ],
        DARK,
    )
    for side in (-1, 1):
        panel(
            d,
            [
                (64 + side * 11, 32),
                (64 + side * 34, 51),
                (64 + side * 28, 74),
                (64 + side * 15, 68),
            ],
            paint,
        )
        engine(d, 64 + side * 28 - 9, 68, 18, 30, paint, move)
    panel(d, [(54, 32), (64, 23), (74, 32), (76, 101), (64, 110), (52, 101)], IRON)
    box(d, (58, 75, 70, 96), paint, 2)
    radar_mount(d, 64, 47)
    box(d, (60, 65, 68, 69), DEEP, 1)
    return finish(im)


def gnat(faction, move=0, action=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    for side in (-1, 1):
        strut(d, (64 + side * 6, 67), (64 + side * 25, 96), 4)
        engine(d, 64 + side * 28 - 8, 84, 16, 28, paint, move)
    panel(d, [(57, 28), (71, 28), (75, 45), (72, 80), (56, 80), (53, 45)], DARK)
    box(d, (59, 62, 69, 76), paint, 2)
    line(d, [(64, 31), (64, 17)], IRON, 3)
    line(d, [(61, 19), (67, 19)], EDGE)
    radar_mount(d, 64, 44)
    return finish(im)


def bombard_spades(phase):
    im, d = canvas()
    fraction = phase / 4
    for side in (-1, 1):
        x = 64 + side * 22
        foot = (x + side * (3 + 8 * fraction), 106 + 10 * fraction)
        strut(d, (x, 96), foot, 4)
        box(d, (foot[0] - 7, foot[1] - 3, foot[0] + 7, foot[1] + 2), DARK, 1)
        line(d, [(foot[0] - 5, foot[1] - 2), (foot[0] + 5, foot[1] - 2)], EDGE)
    return finish(im)


def source_frames():
    for faction in gen.FACTIONS:
        for kind, count in [
            ("bombard", 6),
            ("flakhound", 9),
            ("stinger", 4),
            ("kestrel", 0),
            ("gnat", 0),
        ]:
            builder = {
                "bombard": bombard,
                "flakhound": flakhound,
                "stinger": stinger,
                "kestrel": kestrel,
                "gnat": gnat,
            }[kind]
            yield f"{kind}_{faction}", builder(faction)
            for phase in (1, 2):
                suffix = "tread" if kind == "flakhound" else "move"
                yield f"{kind}_{faction}_{suffix}{phase}", builder(faction, move=phase)
            for phase in range(1, count + 1):
                yield f"{kind}_{faction}_action{phase}", builder(faction, action=phase)
    for phase in range(5):
        yield f"bombard_spades_{phase}", bombard_spades(phase)
    yield "scout_radar", scout_radar()


def install_specialists(registry, out):
    for key, image in source_frames():
        registry[key] = image
        image.save(out / f"{key}.png")
