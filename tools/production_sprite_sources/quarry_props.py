"""Layered quarry stone and visibly abandoned industrial hardware."""

import math

from PIL import Image, ImageDraw

SS = 4
VOID = (17, 17, 21)
DEEP = (28, 27, 32)
IRON = (52, 52, 55)
EDGE = (88, 84, 78)
SAGE = (67, 75, 66)
RUST = (95, 67, 46)


class Surface:
    def __init__(self, width, height):
        self.size = (width, height)
        self.image = Image.new("RGBA", (width * SS, height * SS))
        self.draw = ImageDraw.Draw(self.image)

    def points(self, points):
        return [(round(x * SS), round(y * SS)) for x, y in points]

    def poly(self, points, color):
        self.draw.polygon(self.points(points), fill=color)

    def line(self, points, color, width=1):
        self.draw.line(self.points(points), fill=color, width=round(width * SS))

    def box(self, bounds, color, radius=0):
        self.draw.rounded_rectangle(
            tuple(round(v * SS) for v in bounds), radius=round(radius * SS), fill=color
        )

    def oval(self, bounds, color):
        self.draw.ellipse(tuple(round(v * SS) for v in bounds), fill=color)

    def bolt(self, x, y):
        self.box((x - 2, y - 2, x + 2, y + 2), VOID, 1)
        self.line([(x - 1, y - 1), (x + 1, y - 1)], EDGE)
        self.box((x - 0.6, y - 0.5, x + 0.7, y + 0.7), (62, 60, 55))

    def plate(self, bounds, color, cut=4):
        x, y, r, b = bounds
        self.poly(
            [
                (x + cut, y),
                (r - cut, y),
                (r, y + cut),
                (r, b - cut),
                (r - cut, b),
                (x + cut, b),
                (x, b - cut),
                (x, y + cut),
            ],
            color,
        )

    def finish(self):
        return self.image.resize(self.size, Image.Resampling.LANCZOS)


def stone(s, top, face, ridge, height=8):
    s.poly([(x, y + height) for x, y in top], DEEP)
    s.poly(face, (44, 42, 48))
    s.poly(top, (79, 76, 79))
    s.poly(ridge, (94, 89, 87))


def rock(index):
    s = Surface(64, 64)
    if index == 1:
        stone(
            s,
            [(8, 30), (15, 16), (27, 12), (30, 26), (26, 42), (12, 40)],
            [(12, 40), (26, 42), (30, 26), (30, 37), (26, 52), (12, 48)],
            [(14, 27), (18, 19), (25, 17), (27, 26), (22, 33)],
            9,
        )
        stone(
            s,
            [(34, 17), (46, 21), (54, 36), (46, 44), (33, 42), (31, 30)],
            [(33, 42), (46, 44), (54, 36), (53, 47), (44, 54), (33, 51)],
            [(35, 21), (44, 25), (47, 33), (35, 31)],
            8,
        )
        s.line([(13, 43), (24, 46), (27, 40)], DEEP)
        s.line([(35, 45), (44, 48), (50, 44)], DEEP)
        s.poly([(24, 53), (28, 47), (34, 51), (33, 56)], (60, 56, 59))
    elif index == 2:
        stone(
            s,
            [(15, 25), (26, 9), (43, 15), (51, 32), (42, 44), (20, 44)],
            [(20, 44), (42, 44), (51, 32), (51, 45), (42, 56), (21, 54)],
            [(20, 25), (28, 14), (39, 19), (40, 30), (26, 34)],
            11,
        )
        s.poly(
            [(40, 18), (46, 26), (49, 33), (41, 41), (36, 34), (40, 30)], (62, 60, 66)
        )
        s.line([(24, 40), (31, 38), (34, 34), (42, 34)], DEEP, 1.4)
        s.line([(22, 47), (34, 49), (44, 44)], DEEP)
        s.line([(24, 51), (35, 53), (44, 49)], DEEP)
        s.poly([(6, 46), (11, 40), (17, 44), (16, 50), (9, 53)], (60, 56, 59))
        s.poly([(48, 53), (54, 47), (59, 50), (58, 56)], (60, 56, 59))
    else:
        stone(
            s,
            [(9, 25), (15, 17), (27, 21), (27, 31), (15, 35), (8, 30)],
            [(8, 30), (15, 35), (27, 31), (25, 40), (15, 43), (8, 37)],
            [(13, 25), (17, 20), (24, 24), (20, 29)],
            7,
        )
        stone(
            s,
            [(29, 15), (44, 14), (55, 27), (52, 40), (37, 45), (27, 36)],
            [(27, 36), (37, 45), (52, 40), (53, 49), (39, 54), (29, 46)],
            [(30, 26), (33, 19), (43, 19), (49, 28), (40, 32)],
            8,
        )
        s.poly(
            [(43, 20), (50, 25), (53, 29), (49, 38), (41, 40), (40, 32)], (62, 60, 66)
        )
        s.line([(32, 40), (39, 45), (50, 42)], DEEP)
        stone(
            s,
            [(13, 47), (22, 42), (30, 46), (28, 53), (19, 54)],
            [(19, 54), (28, 53), (28, 58), (19, 59), (13, 54)],
            [(17, 47), (23, 45), (27, 47), (22, 50)],
            4,
        )
    return s.finish()


def compressor():
    s = Surface(128, 64)
    for y in (7, 49):
        s.plate((7, y + 3, 121, y + 9), VOID, 2)
        s.plate((7, y, 120, y + 6), IRON, 2)
        s.line([(11, y + 1), (117, y + 1)], EDGE)
        s.line([(14, y + 5), (32, y + 5)], RUST, 1.5)
    for x in (17, 49, 84, 108):
        s.plate((x - 5, 8, x + 5, 58), DEEP, 2)
        s.bolt(x, 10)
        s.bolt(x, 52)
    s.box((9, 18, 33, 47), VOID, 8)
    s.box((10, 13, 31, 42), SAGE, 8)
    s.line([(15, 16), (24, 16), (28, 20)], (105, 103, 85), 1.5)
    s.box((12, 26, 30, 30), (50, 58, 52), 1)
    s.bolt(20, 19)
    s.box((30, 26, 43, 34), VOID, 1)
    s.box((31, 27, 43, 31), EDGE, 1)
    s.box((40, 15, 87, 49), VOID, 7)
    s.box((39, 11, 86, 44), (72, 72, 62), 6)
    s.box((45, 14, 81, 40), (38, 40, 36), 3)
    for x in range(49, 80, 5):
        s.box((x, 13, x + 2, 40), (91, 90, 74), 1)
        s.line([(x + 3, 15), (x + 3, 40)], DEEP)
    s.poly(
        [
            (44, 14),
            (40, 23),
            (44, 28),
            (40, 37),
            (46, 42),
            (46, 29),
            (43, 24),
            (48, 18),
        ],
        VOID,
    )
    for x in (48, 79):
        s.plate((x - 3, 10, x + 3, 47), (63, 65, 55), 2)
        s.bolt(x, 14)
        s.bolt(x, 40)
    s.plate((56, 17, 73, 25), (81, 82, 68), 2)
    s.bolt(60, 21)
    s.bolt(69, 21)
    s.box((86, 25, 92, 32), EDGE, 1)
    s.box((91, 20, 118, 45), VOID, 4)
    s.box((90, 17, 116, 40), SAGE, 4)
    for y in (22, 27, 32):
        s.line([(94, y), (112, y)], (100, 103, 87))
        s.line([(94, y + 2), (112, y + 2)], (40, 45, 41))
    s.plate((99, 32, 112, 44), (55, 63, 57), 2)
    s.bolt(103, 36)
    s.line([(30, 43), (36, 49), (35, 56), (42, 59), (47, 57)], VOID, 4)
    s.line([(30, 42), (36, 48), (35, 55), (42, 58), (47, 56)], (62, 60, 49), 2)
    s.line([(46, 56), (49, 55), (50, 57)], (118, 86, 51))
    s.poly([(105, 8), (109, 3), (118, 6), (117, 11)], IRON)
    s.bolt(112, 7)
    for x, y in [(14, 34), (24, 37), (54, 45), (85, 14), (110, 19)]:
        s.line([(x, y), (x + 4, y + 1)], RUST, 1.2)
    return s.finish()


def cooling_fan():
    s = Surface(128, 128)
    s.plate((11, 19, 117, 117), VOID, 12)
    s.plate((9, 10, 115, 109), IRON, 10)
    s.plate((16, 17, 108, 103), (39, 40, 43), 8)
    for x in (16, 108):
        s.box((x - 3, 24, x + 3, 97), (62, 66, 60), 1)
    s.line([(21, 11), (103, 11), (113, 21)], EDGE, 2)
    s.line([(22, 107), (102, 107)], DEEP, 3)
    s.oval((21, 22, 104, 107), VOID)
    s.oval((23, 18, 101, 99), (78, 78, 70))
    s.oval((27, 22, 97, 94), (44, 47, 44))
    s.oval((30, 25, 94, 90), VOID)
    for i in range(5):
        a = i * math.tau / 5 - 0.25

        def rotate(p, a=a):
            x, y = p
            return (
                62 + x * math.cos(a) - y * math.sin(a),
                58 + x * math.sin(a) + y * math.cos(a),
            )

        pts = [(5, -5), (12, -10), (28, -6), (29, 7), (17, 5), (7, 3)]
        if i == 3:
            pts = [(5, -5), (12, -10), (19, -7), (15, 0), (18, 5), (7, 3)]
        s.poly([rotate(p) for p in pts], (77, 82, 70))
        s.line([rotate(p) for p in pts[:3]], (111, 108, 86), 1.5)
        s.line([rotate((13, -6)), rotate((19, -4))], RUST)
    s.oval((51, 47, 73, 70), DEEP)
    s.oval((53, 46, 70, 63), IRON)
    s.bolt(61, 54)
    # Bent cage remnants leave the stopped impeller clearly visible.
    s.line([(30, 35), (38, 37), (45, 32)], IRON, 3)
    s.line([(79, 87), (84, 78), (95, 74)], IRON, 3)
    for x, y in [(19, 20), (105, 20), (19, 99), (105, 99)]:
        s.bolt(x, y)
    s.box((41, 100, 78, 104), DEEP, 1)
    for x in (46, 55, 64, 73):
        s.line([(x, 101), (x + 2, 103)], (107, 84, 48), 1.5)
    s.poly([(100, 51), (114, 49), (114, 56), (108, 62), (109, 75), (101, 76)], DEEP)
    s.line([(104, 53), (112, 52), (110, 57)], EDGE)
    return s.finish()


def wreck():
    s = Surface(64, 64)
    s.poly(
        [(9, 25), (19, 18), (37, 24), (52, 33), (49, 42), (31, 46), (12, 38)],
        (20, 20, 24, 95),
    )
    s.poly([(12, 29), (19, 22), (35, 28), (30, 38), (16, 36)], (48, 47, 46))
    s.poly([(19, 22), (35, 28), (31, 31), (17, 26)], (73, 70, 60))
    s.line([(17, 30), (29, 33)], (91, 77, 51))
    s.poly([(36, 32), (45, 28), (51, 36), (42, 42), (34, 38)], (39, 43, 41))
    s.line([(38, 33), (45, 30), (49, 36)], (71, 79, 68))
    s.line([(8, 39), (22, 43), (28, 41)], DEEP, 3)
    s.line([(8, 38), (22, 42), (28, 40)], (77, 66, 50), 1.5)
    s.box((27, 16, 32, 19), (66, 63, 55), 1)
    s.line([(44, 45), (49, 47), (54, 45)], (64, 56, 46))
    return s.finish()


def frames():
    for i in range(1, 4):
        yield f"rock_{i}", rock(i)
    yield "ground_blocker_cooling_fan", cooling_fan()
    yield "ground_blocker_compressor_skid", compressor()
    yield "decal_wreck", wreck()
