# /// script
# requires-python = ">=3.14"
# dependencies = ["pillow==12.3.0"]
# ///
"""Generate the shared cast-metal O icon for desktop and iOS.

The artwork resolves to 128 native pixels before nearest-neighbor enlargement.
iOS uses the opaque square master; desktop exports add transparent outer corners.
Run with `uv run tools/gen_icon.py`; commit the generator and outputs together.
"""

from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "assets" / "icon"
IOS_OUT = ROOT / "ios" / "Oxide" / "Assets.xcassets" / "AppIcon.appiconset"
SS = 4
NATIVE_SIZE = 128
MASTER_SIZE = 1024

VOID = (9, 9, 12)
IRON_DEEP = (17, 18, 23)
IRON_DARK = (38, 38, 46)
IRON = (52, 52, 62)
IRON_LIGHT = (72, 72, 84)
BONE = (232, 228, 216)
RUST = (196, 87, 59)
RUST_DARK = (126, 56, 38)
RUST_LIGHT = (232, 137, 107)
CUPRIC_DARK = (39, 96, 79)
CUPRIC = (63, 148, 130)
CUPRIC_LIGHT = (119, 196, 176)


def render_master() -> Image.Image:
    image = Image.new("RGBA", (NATIVE_SIZE * SS, NATIVE_SIZE * SS))
    draw = ImageDraw.Draw(image)

    def polygon(points, color):
        draw.polygon([(x * SS, y * SS) for x, y in points], fill=(*color, 255))

    def octagon(bounds, cut, color):
        x, y, right, bottom = bounds
        polygon(
            (
                (x + cut, y),
                (right - cut, y),
                (right, y + cut),
                (right, bottom - cut),
                (right - cut, bottom),
                (x + cut, bottom),
                (x, bottom - cut),
                (x, y + cut),
            ),
            color,
        )

    def line(points, color, width=1):
        draw.line(
            [(x * SS, y * SS) for x, y in points],
            fill=(*color, 255),
            width=width * SS,
            joint="curve",
        )

    def rectangle(bounds, color):
        draw.rectangle(tuple(v * SS for v in bounds), fill=(*color, 255))

    octagon((10, 11, 118, 119), 29, VOID)
    octagon((12, 12, 116, 117), 28, IRON_DARK)
    line(((12, 87), (12, 40), (40, 12), (87, 12)), BONE)
    line(((14, 88), (14, 41), (41, 14), (87, 14)), IRON_LIGHT)
    line(((117, 42), (117, 88), (89, 117), (40, 117)), IRON_DEEP, 2)
    octagon((17, 17, 111, 112), 25, IRON)
    octagon((19, 19, 109, 110), 24, RUST_DARK)
    octagon((21, 20, 107, 107), 23, RUST)
    polygon(
        (
            (107, 44),
            (107, 84),
            (84, 107),
            (44, 107),
            (23, 86),
            (27, 86),
            (46, 103),
            (83, 103),
            (103, 83),
            (103, 45),
        ),
        RUST_DARK,
    )
    line(((21, 61), (21, 44), (44, 21), (60, 21)), RUST_LIGHT)
    line(((68, 21), (83, 21), (105, 43)), RUST_LIGHT)
    octagon((34, 35, 94, 96), 15, RUST_DARK)
    octagon((35, 36, 93, 95), 14, VOID)
    octagon((37, 38, 91, 93), 13, IRON_DARK)
    line(((38, 77), (38, 51), (51, 38), (77, 38)), IRON_LIGHT)
    line(((51, 92), (78, 92), (90, 80), (90, 52)), IRON_DEEP, 2)
    octagon((42, 43, 86, 88), 11, VOID)
    line(((43, 75), (43, 55), (55, 43), (75, 43)), IRON_DEEP)
    line(((55, 87), (75, 87), (85, 77)), IRON_LIGHT)
    rectangle((62, 13, 65, 36), VOID)
    rectangle((63, 16, 64, 32), IRON_LIGHT)
    rectangle((62, 94, 65, 115), VOID)
    rectangle((63, 98, 64, 113), IRON_LIGHT)
    polygon(
        (
            (29, 37),
            (35, 31),
            (38, 31),
            (38, 34),
            (35, 34),
            (35, 37),
            (32, 37),
            (32, 40),
            (29, 40),
        ),
        IRON,
    )
    polygon(((47, 21), (54, 21), (54, 23), (51, 23), (51, 25), (47, 25)), IRON)
    polygon(((26, 79), (29, 79), (29, 83), (32, 83), (32, 86), (28, 85)), IRON_DARK)
    polygon(
        (
            (96, 76),
            (100, 76),
            (100, 70),
            (105, 70),
            (105, 85),
            (84, 106),
            (74, 106),
            (74, 102),
            (81, 102),
            (81, 98),
            (86, 98),
            (86, 92),
            (91, 92),
            (91, 84),
            (96, 84),
        ),
        CUPRIC_DARK,
    )
    polygon(
        (
            (98, 78),
            (102, 78),
            (102, 74),
            (104, 74),
            (104, 84),
            (83, 103),
            (78, 103),
            (78, 102),
            (84, 99),
            (88, 95),
            (88, 93),
            (94, 88),
            (94, 84),
            (98, 84),
        ),
        CUPRIC,
    )
    line(((98, 78), (101, 78), (103, 76)), CUPRIC_LIGHT)
    line(((82, 102), (87, 97), (91, 95)), CUPRIC_LIGHT)
    rectangle((13, 77, 34, 78), VOID)
    line(((15, 79), (33, 79)), IRON_LIGHT)
    rectangle((94, 51, 114, 52), VOID)
    line(((96, 53), (112, 53)), IRON_LIGHT)
    rectangle((59, 23, 68, 28), VOID)
    rectangle((60, 24, 67, 27), IRON_DARK)
    line(((60, 24), (67, 24)), IRON_LIGHT)
    line(((61, 26), (65, 26)), IRON)
    rectangle((60, 101, 67, 107), VOID)
    rectangle((61, 102, 66, 106), IRON_DARK)
    line(((61, 102), (65, 102)), IRON_LIGHT)
    line(((63, 104), (65, 104)), IRON)

    native = image.resize((NATIVE_SIZE, NATIVE_SIZE), Image.Resampling.BOX)
    # Match the sprites' one-pixel top-left rim at native resolution.
    alpha = native.getchannel("A")
    edge = ImageChops.subtract(alpha.filter(ImageFilter.MaxFilter(3)), alpha)
    shifted = ImageChops.subtract(
        edge, edge.transform(edge.size, Image.Transform.AFFINE, (1, 0, -1, 0, 1, -1))
    )
    rim = Image.new("RGBA", native.size, (255, 244, 224, 0))
    rim.putalpha(shifted.point(lambda value: min(value, 110)))
    native.alpha_composite(rim)
    background = Image.new("RGBA", native.size, (28, 28, 34, 255))
    background.alpha_composite(native)
    return background.resize(
        (MASTER_SIZE, MASTER_SIZE), Image.Resampling.NEAREST
    ).convert("RGB")


def desktop_icon(master: Image.Image) -> Image.Image:
    # Legacy .icns and window icons do not receive iOS's automatic corner mask.
    mask = Image.new("L", (MASTER_SIZE * SS, MASTER_SIZE * SS))
    ImageDraw.Draw(mask).rounded_rectangle(
        tuple(v * SS for v in (48, 48, 976, 976)), radius=196 * SS, fill=255
    )
    desktop = master.convert("RGBA")
    desktop.putalpha(mask.resize(master.size, Image.Resampling.LANCZOS))
    return desktop


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    IOS_OUT.mkdir(parents=True, exist_ok=True)
    master = render_master()
    master.save(OUT / "oxide_1024.png")
    master.save(IOS_OUT / "oxide_1024.png")
    desktop = desktop_icon(master)
    desktop.save(OUT / "oxide_desktop_1024.png")
    desktop.resize((256, 256), Image.Resampling.BOX).save(OUT / "oxide_256.png")
    for size in (16, 32, 64):
        (OUT / f"oxide_{size}.rgba").write_bytes(
            desktop.resize((size, size), Image.Resampling.BOX).tobytes()
        )
    print("Generated desktop and iOS icons")


if __name__ == "__main__":
    main()
