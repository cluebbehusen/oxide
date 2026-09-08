"""Abandoned industrial obstacles in the approved quarry material family."""

import math

from tools.production_sprite_sources.quarry_props import Surface

SS = 4
VOID = (17, 17, 21)
DEEP = (28, 27, 32)
IRON = (52, 52, 55)
EDGE = (88, 84, 78)
SAGE = (67, 75, 66)
RUST = (95, 67, 46)


def gear(s, cx, cy, radius, teeth, phase=0):
    points = []
    for i in range(teeth * 4):
        angle = phase + i * math.tau / (teeth * 4)
        r = radius * (1 if i % 4 in (1, 2) else 0.85)
        points.append((cx + r * math.cos(angle), cy + r * math.sin(angle)))
    s.poly([(x, y + 3) for x, y in points], VOID)
    s.poly(points, (82, 80, 68))
    r = radius * 0.60
    s.oval((cx - r, cy - r, cx + r, cy + r), DEEP)
    for angle in (phase, phase + math.tau / 3, phase + math.tau * 2 / 3):
        s.line(
            [(cx, cy), (cx + r * math.cos(angle), cy + r * math.sin(angle))], IRON, 4
        )
    s.oval((cx - 6, cy - 6, cx + 6, cy + 6), EDGE)
    s.oval((cx - 3, cy - 3, cx + 3, cy + 3), VOID)


def skid(s, width, height):
    for y in (height - 19, 11):
        s.plate((10, y + 5, width - 9, y + 12), VOID, 3)
        s.plate((8, y, width - 11, y + 7), IRON, 3)
        s.line([(13, y + 1), (width - 18, y + 1)], EDGE)
    for x in (22, width - 26):
        s.plate((x - 5, 10, x + 5, height - 10), DEEP, 2)
        s.bolt(x, 14)
        s.bolt(x, height - 15)


def bushing(s, x, y):
    s.box((x - 5, y - 5, x + 5, y + 7), VOID, 2)
    for dy, r in [(3, 5), (0, 5), (-3, 4)]:
        s.oval((x - r, y + dy - 2, x + r, y + dy + 3), (73, 56, 47))
        s.line([(x - r + 1, y + dy - 1), (x + r - 1, y + dy - 1)], (108, 85, 64))
    s.bolt(x, y - 3)


def transformer_bank():
    s = Surface(192, 128)
    s.plate((8, 17, 183, 117), DEEP, 8)
    s.plate((8, 13, 183, 109), (49, 48, 48), 8)
    s.line([(16, 14), (174, 14), (181, 21)], (78, 73, 66), 2)
    s.line([(59, 99), (57, 112)], VOID, 2)
    s.line([(129, 95), (135, 102), (133, 113)], VOID, 2)
    for x, top, w in [(18, 25, 49), (74, 34, 42), (126, 28, 44)]:
        s.plate((x, top + 6, x + w, 104), VOID, 5)
        s.plate((x, top, x + w, 97), SAGE, 5)
        s.box((x + 5, top + 25, x + w - 5, 91), DEEP, 2)
        for xx in range(x + 8, x + w - 6, 6):
            s.box((xx, top + 28, xx + 3, 91), (81, 86, 73), 1)
            s.line([(xx, top + 29), (xx, 88)], (100, 102, 85))
        s.box((x + 7, top + 13, x + 22, top + 18), (111, 90, 53), 1)
        for xx in (x + 8, x + w - 8):
            s.plate((xx - 3, top - 2, xx + 3, 102), IRON, 1)
            s.bolt(xx, 95)
        bushing(s, x + 13, top + 5)
        bushing(s, x + w - 13, top + 5)
    s.line([(31, 27), (54, 27), (67, 22), (88, 22), (88, 36)], VOID, 5)
    s.line([(31, 25), (54, 25), (67, 20), (88, 20), (88, 34)], RUST, 3)
    s.line([(102, 36), (119, 25), (138, 25), (138, 30)], RUST, 3)
    s.line([(157, 30), (174, 30), (177, 40)], RUST, 3)
    s.box((84, 55, 109, 74), VOID, 2)
    s.line([(89, 58), (89, 69), (97, 71)], RUST, 2)
    s.line([(103, 59), (100, 65), (103, 72)], (103, 95, 69), 2)
    s.poly([(99, 55), (112, 49), (121, 64), (108, 72)], (76, 82, 70))
    s.bolt(109, 59)
    s.poly(
        [
            (142, 62),
            (151, 56),
            (151, 70),
            (160, 75),
            (155, 84),
            (161, 95),
            (148, 91),
            (144, 75),
        ],
        VOID,
    )
    s.line([(151, 59), (148, 69), (155, 77)], (112, 107, 87))
    s.line([(160, 86), (174, 89), (173, 101), (181, 108)], VOID, 5)
    s.line([(160, 85), (174, 88), (173, 100), (181, 107)], IRON, 2)
    s.line([(180, 107), (186, 108), (185, 112)], RUST)
    return s.finish()


def exposed_gearbox():
    s = Surface(128, 128)
    s.plate((15, 25, 108, 117), VOID, 14)
    s.poly(
        [
            (13, 39),
            (23, 20),
            (75, 17),
            (98, 37),
            (113, 80),
            (101, 105),
            (72, 112),
            (24, 103),
        ],
        SAGE,
    )
    s.poly(
        [
            (22, 41),
            (29, 29),
            (72, 27),
            (88, 40),
            (100, 80),
            (93, 96),
            (70, 103),
            (29, 94),
        ],
        VOID,
    )
    s.line([(22, 37), (29, 25), (72, 23), (91, 39)], EDGE, 2)
    s.box((7, 54, 28, 66), IRON, 2)
    s.box((87, 74, 119, 84), IRON, 2)
    s.line([(9, 55), (24, 55)], EDGE, 2)
    gear(s, 49, 57, 27, 12, 0.15)
    gear(s, 82, 83, 19, 10, 0.28)
    s.oval((48, 54, 57, 65), IRON)
    s.oval((50, 55, 54, 60), VOID)
    for x, y in [(24, 33), (70, 22), (101, 85), (74, 106), (25, 95)]:
        s.bolt(x, y)
    s.poly([(81, 9), (103, 13), (119, 38), (105, 57), (94, 49), (99, 35)], DEEP)
    s.poly([(82, 7), (104, 11), (120, 35), (107, 53), (97, 47), (102, 32)], SAGE)
    s.line([(88, 12), (104, 16), (115, 34)], EDGE)
    s.line([(100, 38), (108, 41), (113, 36)], RUST, 2)
    s.bolt(107, 43)
    s.line([(17, 79), (13, 89), (21, 100)], RUST, 2)
    return s.finish()


def conveyor_drive():
    s = Surface(192, 128)
    skid(s, 192, 128)
    for y in (27, 87):
        s.plate((14, y, 143, y + 11), SAGE, 3)
        s.line([(20, y + 1), (137, y + 1)], EDGE, 2)
        for x in (23, 62, 118, 137):
            s.bolt(x, y + 5)
    s.box((22, 38, 138, 91), VOID, 6)
    for x in (30, 121):
        s.box((x - 8, 37, x + 8, 88), (70, 73, 65), 5)
        s.line([(x - 3, 40), (x - 3, 84)], (99, 100, 83), 3)
        s.oval((x - 7, 32, x + 7, 44), IRON)
        s.oval((x - 7, 81, x + 7, 94), IRON)
        s.bolt(x, 38)
        s.bolt(x, 87)
    s.poly(
        [
            (38, 40),
            (108, 40),
            (102, 52),
            (109, 56),
            (97, 60),
            (104, 66),
            (97, 77),
            (43, 83),
        ],
        (39, 40, 37),
    )
    s.line([(43, 44), (94, 44)], (60, 60, 52), 2)
    s.line([(45, 75), (83, 69), (98, 69)], (61, 62, 52), 2)
    s.poly(
        [
            (103, 60),
            (113, 61),
            (124, 82),
            (116, 101),
            (96, 106),
            (79, 97),
            (84, 90),
            (102, 95),
            (109, 84),
        ],
        (51, 52, 43),
    )
    s.line([(103, 63), (120, 83), (113, 97), (99, 101), (84, 94)], (82, 81, 64), 2)
    s.line([(107, 52), (111, 49), (115, 55), (118, 50)], (111, 103, 78))
    s.plate((144, 36, 179, 90), DEEP, 7)
    s.plate((141, 29, 175, 81), SAGE, 7)
    for y in range(38, 70, 6):
        s.line([(146, y), (170, y)], EDGE, 2)
        s.line([(146, y + 3), (170, y + 3)], DEEP, 2)
    s.box((132, 53, 144, 64), IRON, 2)
    s.plate((152, 73, 175, 95), IRON, 3)
    s.bolt(160, 82)
    s.line([(175, 93), (181, 95), (176, 107)], VOID, 4)
    s.line([(175, 92), (181, 94), (176, 106)], RUST, 1.5)
    return s.finish()


def crusher_motor():
    s = Surface(192, 128)
    skid(s, 192, 128)
    for x in (51, 123):
        s.plate((x - 8, 23, x + 8, 109), DEEP, 3)
        s.plate((x - 6, 27, x + 6, 103), IRON, 2)
        s.bolt(x, 99)
    s.box((30, 40, 142, 101), VOID, 20)
    s.box((27, 28, 138, 91), SAGE, 22)
    s.box((41, 33, 121, 85), (43, 48, 42), 9)
    for x in range(49, 119, 7):
        s.box((x, 33, x + 3, 85), (87, 91, 75), 2)
        s.line([(x + 4, 40), (x + 4, 80)], DEEP)
    s.box((30, 42, 44, 79), IRON, 5)
    s.line([(33, 44), (33, 73)], EDGE, 2)
    s.box((16, 53, 33, 68), IRON, 2)
    s.line([(17, 55), (31, 55)], EDGE, 2)
    s.plate((66, 22, 103, 43), (72, 76, 64), 4)
    s.bolt(73, 29)
    s.bolt(96, 29)
    s.box((76, 31, 92, 36), (106, 91, 57), 1)
    s.oval((128, 31, 178, 96), VOID)
    s.oval((125, 25, 175, 88), (80, 78, 65))
    s.oval((131, 31, 168, 80), DEEP)
    s.oval((139, 41, 161, 69), IRON)
    for y in (35, 76):
        s.bolt(151, y)
    s.oval((145, 47, 155, 61), VOID)
    s.poly([(131, 32), (141, 28), (145, 39), (136, 47)], VOID)
    s.line([(104, 29), (117, 31), (121, 40)], RUST, 2)
    s.line([(99, 93), (106, 101), (121, 100)], VOID, 4)
    s.line([(99, 92), (106, 100), (121, 99)], IRON, 2)
    return s.finish()


def track_assembly():
    s = Surface(192, 64)
    s.plate((9, 13, 181, 58), VOID, 16)
    s.plate((13, 11, 177, 52), DEEP, 15)
    s.box((31, 19, 157, 46), (40, 42, 42), 8)
    for x in (31, 159):
        gear(s, x, 31, 17, 10, 0.1)
    for x in (58, 82, 106, 130):
        s.oval((x - 10, 23, x + 10, 46), VOID)
        s.oval((x - 9, 21, x + 9, 40), (61, 65, 57))
        s.oval((x - 4, 25, x + 4, 33), IRON)
        s.bolt(x, 29)
    for x in range(25, 169, 11):
        for y in (9, 46):
            if y == 9 and 79 < x < 113:
                continue
            s.plate((x - 5, y, x + 5, y + 8), IRON, 1)
            s.line([(x - 3, y + 1), (x + 3, y + 1)], EDGE, 1.5)
            s.line([(x - 3, y + 6), (x + 3, y + 6)], DEEP)
    for x in (13, 171):
        for y in (22, 34):
            s.box((x - 3, y - 4, x + 4, y + 5), IRON, 1)
            s.line([(x - 2, y - 3), (x + 3, y - 3)], EDGE)
    s.line([(76, 12), (89, 18), (103, 17)], RUST, 3)
    s.plate((94, 8, 103, 16), IRON, 1)
    s.plate((109, 4, 119, 12), IRON, 1)
    s.line([(47, 51), (62, 51)], RUST, 2)
    return s.finish()


def vent_blower():
    s = Surface(128, 128)
    s.plate((22, 31, 115, 118), VOID, 14)
    s.plate((23, 24, 113, 109), SAGE, 18)
    s.plate((17, 13, 70, 53), IRON, 5)
    s.plate((23, 18, 64, 46), VOID, 2)
    for x in (29, 39, 49, 59):
        s.line([(x, 21), (x, 42)], (64, 68, 60), 3)
    s.oval((34, 34, 108, 106), (84, 87, 72))
    s.oval((39, 39, 105, 103), (50, 57, 49))
    s.oval((45, 45, 98, 96), VOID)
    for i in range(9):
        a = i * math.tau / 9
        pts = []
        for r, delta in [(8, 0), (22, 0.12), (22, 0.38), (11, 0.22)]:
            pts.append((72 + r * math.cos(a + delta), 71 + r * math.sin(a + delta)))
        s.poly(pts, (86, 88, 70))
    s.oval((62, 62, 82, 81), IRON)
    s.bolt(72, 70)
    s.poly(
        [(81, 43), (94, 48), (103, 60), (102, 81), (95, 89), (87, 83), (92, 60)], SAGE
    )
    s.line([(88, 49), (95, 56), (98, 68)], EDGE, 2)
    s.box((9, 60, 33, 100), VOID, 5)
    s.box((7, 55, 30, 93), IRON, 5)
    for y in (62, 69, 76, 83):
        s.line([(11, y), (26, y)], EDGE)
    s.box((29, 69, 43, 76), IRON, 1)
    for x, y in [(28, 29), (100, 32), (104, 99), (35, 101)]:
        s.bolt(x, y)
    s.line([(20, 98), (21, 108), (30, 112)], VOID, 4)
    s.line([(20, 97), (21, 107), (30, 111)], RUST, 1.5)
    s.poly([(46, 15), (57, 16), (55, 23), (49, 26)], DEEP)
    return s.finish()


def generator_pallet():
    s = Surface(128, 128)
    skid(s, 128, 128)
    s.plate((20, 24, 92, 102), VOID, 6)
    s.box((25, 19, 93, 40), (80, 75, 57), 6)
    s.line([(31, 21), (85, 21)], (111, 101, 75), 2)
    s.oval((72, 24, 81, 32), DEEP)
    s.bolt(76, 27)
    s.plate((30, 42, 80, 74), SAGE, 5)
    for x in (38, 47, 56, 65, 74):
        s.box((x, 46, x + 3, 68), EDGE, 1)
        s.line([(x + 4, 49), (x + 4, 67)], DEEP)
    s.plate((35, 73, 80, 101), IRON, 7)
    s.oval((43, 77, 72, 97), DEEP)
    for x in (49, 56, 63, 70):
        s.line([(x, 80), (x, 93)], (75, 79, 65), 2)
    s.box((80, 52, 94, 67), DEEP, 2)
    s.box((90, 32, 101, 92), IRON, 5)
    s.line([(93, 35), (93, 84)], EDGE, 2)
    s.line([(96, 89), (103, 98), (111, 98)], VOID, 6)
    s.line([(96, 88), (103, 97), (111, 97)], IRON, 3)
    s.box((15, 44, 31, 70), DEEP, 2)
    s.poly([(16, 43), (27, 40), (32, 55), (19, 60)], SAGE)
    s.line([(19, 53), (23, 60), (22, 73), (32, 78)], RUST, 2)
    for x, y in [(35, 40), (76, 40), (38, 99), (80, 99)]:
        s.bolt(x, y)
    s.line([(49, 26), (60, 26)], RUST, 2)
    return s.finish()


def frames():
    for key, make in [
        ("transformer_bank", transformer_bank),
        ("exposed_gearbox", exposed_gearbox),
        ("conveyor_drive", conveyor_drive),
        ("crusher_motor", crusher_motor),
        ("track_assembly", track_assembly),
        ("vent_blower", vent_blower),
        ("generator_pallet", generator_pallet),
    ]:
        yield f"ground_blocker_{key}", make()
