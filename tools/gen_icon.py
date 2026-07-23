# /// script
# requires-python = ">=3.14"
# dependencies = ["pillow==12.3.0"]  # pinned: asset bytes must reproduce
# ///
"""The Oxide application icon: the Ferrous Foundry, exactly as it looks
in the game.

Reproduces `foundry()` from tools/gen_sprites.py at icon resolution —
same 128-space coordinates, drawn 8x larger so nothing is an upscale.
Outputs the 1024px master, a 256px copy, and raw RGBA dumps at 16/32/64
for the window icon (miniquad hands the 64 to the macOS dock).
`tools/package_macos.sh` builds the .icns from the master.

Run with `uv run tools/gen_icon.py`; commit script and output together.
"""

from pathlib import Path

from PIL import Image, ImageDraw

OUT = Path(__file__).resolve().parent.parent / "assets" / "icon"
SS = 2  # supersample factor
PX = 1024
SCALE = 8  # sprite space (128) to icon space (1024)

# The Oxide palette, same constants as tools/gen_sprites.py.
IRON = (52, 52, 62)
IRON_DARK = (38, 38, 46)
IRON_LIGHT = (72, 72, 84)
BONE = (232, 228, 216)
FERROUS = {
    "base": (196, 87, 59),
    "dark": (126, 56, 38),
    "light": (232, 137, 107),
}


def s(v: float) -> int:
    """Scale a sprite-space coordinate into the supersampled canvas."""
    return round(v * SCALE * SS)


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    pal = FERROUS
    img = Image.new("RGBA", (PX * SS, PX * SS), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    # The shapes below mirror gen_sprites.py foundry() line for line.
    # Baseplate with a bevel.
    d.rounded_rectangle([s(6), s(6), s(122), s(122)], radius=s(10), fill=(*IRON_DARK, 255))
    d.rounded_rectangle([s(12), s(12), s(116), s(116)], radius=s(8), fill=(*IRON, 255))
    # Faction roof panels, chevroned toward the center.
    d.polygon([(s(12), s(12)), (s(64), s(12)), (s(12), s(64))], fill=(*pal["dark"], 255))
    d.polygon([(s(116), s(116)), (s(64), s(116)), (s(116), s(64))], fill=(*pal["dark"], 255))
    d.rectangle([s(20), s(20), s(108), s(108)], fill=(*IRON, 255))
    d.rectangle([s(26), s(26), s(102), s(102)], fill=(*pal["base"], 255))
    d.rectangle([s(34), s(34), s(94), s(94)], fill=(*IRON_DARK, 255))
    # The melt pool — the glowing heart of the works.
    d.ellipse([s(44), s(44), s(84), s(84)], fill=(*pal["base"], 255))
    d.ellipse([s(50), s(50), s(78), s(78)], fill=(*pal["light"], 255))
    d.ellipse([s(57), s(57), s(71), s(71)], fill=(*BONE, 255))
    # Chimney stack, top-right, with a dark throat.
    d.ellipse([s(88), s(16), s(112), s(40)], fill=(*IRON_LIGHT, 255))
    d.ellipse([s(93), s(21), s(107), s(35)], fill=(*IRON_DARK, 255))
    # Rivets on the corners that have no chimney.
    for cx, cy in ((22, 22), (22, 106), (106, 106)):
        d.ellipse([s(cx - 3), s(cy - 3), s(cx + 3), s(cy + 3)], fill=(*IRON_LIGHT, 255))

    master = img.resize((PX, PX), Image.LANCZOS)
    master.save(OUT / "oxide_1024.png")
    print("  oxide_1024.png")
    master.resize((256, 256), Image.LANCZOS).save(OUT / "oxide_256.png")
    print("  oxide_256.png")
    for size in (16, 32, 64):
        (OUT / f"oxide_{size}.rgba").write_bytes(
            master.resize((size, size), Image.LANCZOS).tobytes()
        )
        print(f"  oxide_{size}.rgba")


if __name__ == "__main__":
    main()
