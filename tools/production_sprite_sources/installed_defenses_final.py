"""Compact installed defenses in Oxide's accepted machine materials."""

from tools import gen_sprites as gen
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

BRASS = (130, 105, 66)


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


def flash(d, x, y, width=5):
    poly(
        d,
        [
            (x, y - 9),
            (x + width, y - 3),
            (x + 2, y + 1),
            (x - 2, y + 1),
            (x - width, y - 3),
        ],
        (210, 166, 91),
    )
    line(d, [(x, y - 5), (x, y)], (234, 205, 144), 2)


def barrel(d, x, y, bottom, width):
    box(d, (x - width / 2 - 2, y, x + width / 2 + 2, bottom), VOID, 2)
    box(d, (x - width / 2, y + 2, x + width / 2, bottom - 2), IRON, 1)
    line(d, [(x - width / 2 + 1, y + 4), (x - width / 2 + 1, bottom - 4)], EDGE)
    box(d, (x - width / 2 - 2, y, x + width / 2 + 2, y + 5), DARK, 1)
    line(d, [(x - width / 2 + 1, y + 2), (x + width / 2 - 1, y + 2)], VOID, 2)


def bastion_base(faction, phase=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    plate(d, (22, 28, 109, 111), DEEP, 13)
    plate(d, (28, 34, 103, 105), DARK, 10)
    plate(d, (35, 41, 96, 98), VOID, 9)
    for x, y in [(28, 33), (102, 33), (28, 104), (102, 104)]:
        plate(d, (x - 7, y - 5, x + 7, y + 6), IRON, 3)
        bolt(d, x, y)
    for bounds in [(34, 31, 56, 37), (78, 102, 97, 108)]:
        box(d, bounds, paint, 1)
    circle(d, (42, 42, 86, 86), DEEP)
    circle(d, (47, 47, 81, 81), IRON)
    circle(d, (53, 53, 75, 75), DARK)
    # Floor-mounted ammunition stays below the traversing carriage.
    plate(d, (17, 45, 33, 98), DARK, 3)
    charge = (5, 1, 2, 3, 4, 5, 5, 0, 0, 0)[phase]
    for i in range(5):
        y = 50 + i * 8
        box(d, (21, y, 29, y + 5), BRASS if i < charge else DEEP, 1)
        if i < charge:
            line(d, [(22, y + 1), (27, y + 1)], (162, 137, 91))
    return finish(im)


def bastion_mount(faction, phase=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    recoil = {6: 2, 7: 7, 8: 3}.get(phase, 0)
    # Traverse fork -> recoil slides -> breech -> barrel, with a rear rammer.
    plate(d, (43, 51, 85, 85), DARK, 5)
    for x in (45, 79):
        box(d, (x - 3, 41, x + 3, 91), VOID, 2)
        box(d, (x - 1, 44, x + 1, 73 + recoil), EDGE, 1)
        box(d, (x - 4, 72 + recoil, x + 4, 90), IRON, 2)
    barrel(d, 64, 3 + recoil, 70 + recoil, 10)
    box(d, (56, 40 + recoil, 72, 58 + recoil), DARK, 2)
    line(d, [(58, 42 + recoil), (58, 55 + recoil)], EDGE, 2)
    plate(d, (50, 60 + recoil, 78, 83 + recoil), paint, 4)
    box(d, (56, 65 + recoil, 72, 78 + recoil), DARK, 2)
    plate(d, (55, 85, 73, 103), DEEP, 3)
    if phase in (1, 2, 3):
        y = 96 - (phase - 1) * 4
        box(d, (61, y - 4, 67, y + 4), BRASS, 1)
        line(d, [(64, y + 4), (64, 101)], IRON, 3)
    else:
        box(d, (61, 90, 67, 100), IRON, 1)
    if phase == 6:
        flash(d, 64, 3 + recoil, 8)
    return finish(im, False)


def turret_base(faction, tier=0):
    if tier == 2:
        return bulwark_base(faction)
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    plate(d, (14 - tier * 2, 25 - tier * 3, 114 + tier * 2, 110 + tier * 2), DEEP, 18)
    for x, y in [(24, 36), (104, 36), (24, 99), (104, 99)]:
        plate(d, (x - 8, y - 6, x + 8, y + 7), IRON, 3)
        bolt(d, x, y)
    circle(d, (32 - tier * 3, 34 - tier * 3, 96 + tier * 3, 98 + tier * 3), VOID)
    circle(d, (37 - tier * 2, 39 - tier * 2, 91 + tier * 2, 93 + tier * 2), IRON)
    circle(d, (44, 46, 84, 86), DARK)
    for x in (33, 83):
        box(d, (x, 97, x + 12, 104), paint, 1)
    if tier:
        for x in (18, 99):
            plate(d, (x, 48, x + 11, 87), paint, 3)
    return finish(im)


def turret_mount(faction, tier=0, phase=0):
    if tier == 2:
        return bulwark_mount(faction, phase)
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    recoil = {1: 2, 2: 7, 3: 1}.get(phase, 0)
    plate(d, (43 - tier * 2, 51, 85 + tier * 2, 88), DARK, 7)
    barrel(d, 64, 12 - tier * 2 + recoil, 67 + recoil, 7 + tier * 3)
    plate(d, (49 - tier, 53 + recoil, 79 + tier, 82 + recoil), paint, 4)
    box(d, (56, 59 + recoil, 72, 77 + recoil), IRON, 2)
    box(d, (59, 63 + recoil, 69, 74 + recoil), DARK, 1)
    plate(d, (29 - tier * 2, 57, 45, 88), DARK, 3)
    box(d, (33 - tier * 2, 61, 41, 78), paint, 1)
    line(d, [(42, 69), (49, 69)], BRASS, 4)
    for i in range(3):
        box(d, (42 + i * 3, 67, 44 + i * 3, 71), EDGE, 1)
    if tier:
        plate(d, (84, 58, 100 + tier * 2, 88), DARK, 3)
        vent(d, 87, 63, 11)
        line(d, [(80, 73), (86, 73)], BRASS, 3)
    if phase == 1:
        flash(d, 64, 12 - tier * 2 + recoil, 5 + tier)
    return finish(im, False)


def flak_base(faction, tier=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    for a, b in [((25, 29), (104, 105)), ((103, 29), (24, 105))]:
        line(d, [a, b], VOID, 18)
        line(d, [a, b], IRON, 11)
    for x, y in [(24, 28), (104, 28), (24, 106), (104, 106)]:
        plate(d, (x - 10, y - 7, x + 10, y + 8), DARK, 4)
        bolt(d, x, y)
    plate(d, (37, 38, 91, 104), DEEP, 12)
    circle(d, (44, 43, 84, 83), VOID)
    circle(d, (50, 49, 78, 77), IRON)
    box(d, (44, 89, 84, 97), paint, 2)
    if tier:
        for x in (24, 91):
            plate(d, (x, 48, x + 13, 85), paint, 4)
    return finish(im)


def flak_mount(faction, tier=0, phase=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    plate(d, (33 - tier * 4, 58, 95 + tier * 4, 87), DARK, 6)
    plate(d, (44, 67, 84, 85), paint, 3)
    for side, cx in enumerate((44 - tier * 3, 84 + tier * 3)):
        recoil = 6 if phase == 5 + side else 2 if phase == 7 else 0
        plate(
            d,
            (cx - 13 - tier * 3, 48 + recoil, cx + 13 + tier * 3, 70 + recoil),
            IRON,
            4,
        )
        n = 2 + tier
        for j in range(n):
            x = cx + (j - (n - 1) / 2) * 8
            barrel(d, x, 16 + recoil, 56 + recoil, 3.5)
            if phase == 5 + side:
                flash(d, x, 16 + recoil, 4)
        box(d, (cx - 8, 54 + recoil, cx + 8, 61 + recoil), DARK, 1)
    # The small feed moves through the existing four preparation stages.
    charge = (
        phase if 1 <= phase <= 4 else 2 if phase == 5 else 0 if phase in (6, 7) else 4
    )
    plate(d, (44, 88, 84, 99), DARK, 2)
    for i in range(4):
        box(d, (48 + i * 8, 91, 52 + i * 8, 96), BRASS if i < charge else DEEP, 1)
    if tier:
        box(d, (60, 37, 68, 60), DARK, 2)
        line(d, [(62, 40), (62, 55)], EDGE)
    return finish(im, False)


def barricade(faction):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    for x in (9, 104):
        plate(d, (x, 70, x + 16, 111), DEEP, 3)
        poly(d, [(x + 2, 102), (x + 6, 68), (x + 13, 68), (x + 14, 102)], IRON)
        bolt(d, x + 8, 103)
    box(d, (0, 27, 128, 91), VOID, 3)
    poly(d, [(0, 37), (128, 37), (124, 82), (4, 82)], DARK)
    box(d, (0, 26, 128, 39), IRON, 2)
    line(d, [(2, 28), (126, 28)], EDGE, 2)
    for x in (3, 47, 91):
        plate(d, (x, 43, x + 34, 75), DEEP, 3)
        box(d, (x + 5, 51, x + 29, 63), paint, 1)
    for x in (40, 84):
        box(d, (x - 3, 38, x + 3, 87), IRON, 1)
        bolt(d, x, 47)
        bolt(d, x, 73)
    return finish(im)


def scuttle_charge(faction):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["base"]
    plate(d, (23, 36, 105, 94), DEEP, 14)
    plate(d, (30, 42, 98, 86), IRON, 10)
    plate(d, (37, 47, 91, 79), paint, 7)
    box(d, (49, 54, 79, 71), DEEP, 3)
    line(d, [(53, 56), (75, 56)], IRON)
    for x in (28, 92):
        plate(d, (x, 57, x + 8, 76), DARK, 2)
        bolt(d, x + 4, 66)
    box(d, (45, 84, 83, 90), paint, 1)
    box(d, (59, 84, 69, 90), VOID, 1)
    box(d, (61, 86, 67, 88), BRASS)
    return finish(im)


def bulwark_base(faction):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    plate(d, (13, 29, 115, 116), DEEP, 15)
    plate(d, (20, 35, 108, 110), IRON, 12)
    circle(d, (29, 29, 99, 99), VOID)
    circle(d, (36, 36, 92, 92), IRON)
    for x in (17, 99):
        plate(d, (x, 56, x + 12, 99), DARK, 3)
        box(d, (x + 3, 61, x + 9, 87), paint, 1)
        bolt(d, x + 6, 95)
    for x in (30, 88):
        bolt(d, x, 105)
    box(d, (46, 102, 82, 111), DARK, 2)
    vent(d, 50, 104, 27)
    return finish(im)


def bulwark_mount(faction, phase=0):
    im, d = canvas()
    paint = gen.FACTIONS[faction]["dark"]
    recoil = {1: 2, 2: 7, 3: 2}.get(phase, 0)
    # Armored cheeks shelter twin recoil cylinders and the belt-fed breech.
    plate(d, (29, 42, 99, 101), DARK, 13)
    for x in (32, 81):
        plate(d, (x, 45, x + 15, 93), paint, 5)
        line(d, [(x + 3, 49), (x + 3, 80)], IRON, 2)
        bolt(d, x + 8, 87)
    for x in (50, 78):
        box(d, (x - 3, 32, x + 3, 74), VOID, 2)
        box(d, (x - 1, 35, x + 1, 61 + recoil), EDGE, 1)
        box(d, (x - 4, 62 + recoil, x + 4, 82), IRON, 2)
    barrel(d, 64, 8 + recoil, 73 + recoil, 14)
    plate(d, (53, 28 + recoil, 75, 48 + recoil), IRON, 3)
    box(d, (59, 31 + recoil, 69, 44 + recoil), DARK, 1)
    plate(d, (51, 60 + recoil, 77, 83 + recoil), IRON, 4)
    box(d, (57, 65 + recoil, 71, 78 + recoil), DARK, 2)
    plate(d, (45, 89, 83, 105), DEEP, 4)
    for x in range(50, 79, 6):
        box(d, (x, 93, x + 3, 100), BRASS, 1)
    if phase == 1:
        flash(d, 64, 8 + recoil, 7)
    return finish(im, False)


def source_frames():
    """Yield the complete installed-defense rows before masks and construction."""
    for faction in gen.FACTIONS:
        for phase in range(10):
            suffix = f"_action{phase}" if phase else ""
            yield f"bastion_{faction}{suffix}", bastion_base(faction, phase)
            yield f"bastion_mount_{faction}{suffix}", bastion_mount(faction, phase)
        for tier in range(3):
            tier_suffix = f"_t{tier}" if tier else ""
            yield f"turret{tier_suffix}_{faction}", turret_base(faction, tier)
            for phase in range(5):
                suffix = f"_action{phase}" if phase else ""
                yield (
                    f"turret_barrel{tier_suffix}_{faction}{suffix}",
                    turret_mount(faction, tier, phase),
                )
        for tier in range(2):
            tier_suffix = f"_t{tier}" if tier else ""
            yield f"flak_turret{tier_suffix}_{faction}", flak_base(faction, tier)
            for phase in range(9):
                suffix = f"_action{phase}" if phase else ""
                yield (
                    f"flak_mount{tier_suffix}_{faction}{suffix}",
                    flak_mount(faction, tier, phase),
                )
        yield f"scuttle_charge_{faction}", scuttle_charge(faction)
        yield f"barricade_{faction}", barricade(faction)


def install_defenses(registry, out):
    """Install canonical frames without reading any review workspace."""
    for key, image in source_frames():
        registry[key] = image
        image.save(out / f"{key}.png")
