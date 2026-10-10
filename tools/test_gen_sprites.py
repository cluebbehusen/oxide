import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from PIL import Image

from tools import gen_sprites


class SpriteReproducibilityTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="oxide-sprite-test-")
        root = Path(self.temp.name)
        self.expected = root / "expected"
        self.actual = root / "actual"
        self.expected.mkdir()
        self.actual.mkdir()

    def tearDown(self) -> None:
        self.temp.cleanup()

    def test_paged_atlas_preserves_pixels_extrusion_and_duplicate_aliases(self) -> None:
        frames = {
            f"sprite_{i}": Image.new("RGBA", (1024, 1024), (i * 17, 80, 90, 255))
            for i in range(10)
        }
        frames["duplicate"] = frames["sprite_0"].copy()
        with (
            patch.object(gen_sprites, "OUT", self.actual),
            patch.object(gen_sprites, "REGISTRY", frames),
        ):
            gen_sprites.pack_atlas()
        manifest = json.loads((self.actual / "atlas.json").read_text())
        self.assertEqual(set(manifest), set(frames))
        self.assertEqual(manifest["duplicate"], manifest["sprite_0"])
        pages = [
            Image.open(self.actual / name).convert("RGBA")
            for name in ("atlas.png", "atlas_1.png")
        ]
        self.assertTrue(all(max(page.size) <= 4096 for page in pages))
        for key, (x, y, w, h) in manifest.items():
            page = pages[y // 4096]
            y %= 4096
            self.assertEqual(
                page.crop((x, y, x + w, y + h)).tobytes(), frames[key].tobytes(), key
            )
            self.assertEqual(
                page.crop((x, y - 1, x + w, y)).tobytes(),
                frames[key].crop((0, 0, w, 1)).tobytes(),
            )
            self.assertEqual(
                page.crop((x - 1, y, x, y + h)).tobytes(),
                frames[key].crop((0, 0, 1, h)).tobytes(),
            )
        expected = {p.name: p.read_bytes() for p in self.actual.iterdir()}
        with (
            patch.object(gen_sprites, "OUT", self.actual),
            patch.object(gen_sprites, "REGISTRY", dict(reversed(list(frames.items())))),
        ):
            gen_sprites.pack_atlas()
        self.assertEqual(
            expected, {p.name: p.read_bytes() for p in self.actual.iterdir()}
        )
        with (
            patch.object(gen_sprites, "OUT", self.actual),
            patch.object(gen_sprites, "REGISTRY", {"small": Image.new("RGBA", (4, 4))}),
        ):
            gen_sprites.pack_atlas()
        self.assertFalse((self.actual / "atlas_1.png").exists())

    def test_probe_renders_become_masks_and_never_ship(self) -> None:
        self.assertEqual(gen_sprites.variant_tag("base"), "")
        self.assertEqual(gen_sprites.variant_tag("probe"), gen_sprites.PROBE_TAG)
        with self.assertRaises(ValueError):
            gen_sprites.variant_tag("other")
        base = Image.new("RGBA", (2, 1), (52, 52, 62, 255))
        base.putpixel((0, 0), (*gen_sprites.PALETTES["base"]["base"], 255))
        probe = base.copy()
        probe.putpixel((0, 0), (*gen_sprites.PALETTES["probe"]["base"], 255))
        registry = {}
        for variant, image in (("base", base), ("probe", probe)):
            name = f"unit{gen_sprites.variant_tag(variant)}"
            gen_sprites.save_sprite(image, self.actual, name)
            registry[name] = image
        with (
            patch.object(gen_sprites, "OUT", self.actual),
            patch.object(gen_sprites, "REGISTRY", registry),
        ):
            gen_sprites.accent_masks()
        self.assertEqual(set(registry), {"unit", "unit_accent"})
        self.assertEqual(
            {path.name for path in self.actual.iterdir()},
            {"unit.png", "unit_accent.png"},
        )
        self.assertGreater(registry["unit_accent"].getpixel((0, 0))[3], 0)
        self.assertEqual(registry["unit_accent"].getpixel((1, 0))[3], 0)

    def test_png_compression_differences_preserve_reproducibility(self) -> None:
        pixels = Image.new("RGBA", (3, 2))
        pixels.putdata(
            [
                (0, 0, 0, 0),
                (196, 87, 59, 255),
                (63, 148, 130, 255),
                (232, 228, 216, 255),
                (35, 35, 41, 255),
                (217, 164, 65, 128),
            ]
        )
        expected_png = self.expected / "sprite.png"
        actual_png = self.actual / "sprite.png"
        pixels.save(expected_png, compress_level=0)
        pixels.save(actual_png, compress_level=9)
        (self.expected / "atlas.json").write_bytes(b'{"sprite":[0,0,3,2]}\n')
        (self.actual / "atlas.json").write_bytes(b'{"sprite":[0,0,3,2]}\n')

        self.assertNotEqual(expected_png.read_bytes(), actual_png.read_bytes())
        self.assertEqual(
            gen_sprites._sprite_asset_differences(self.expected, self.actual),
            ([], [], []),
        )

    def test_one_pixel_difference_fails_reproducibility(self) -> None:
        expected = Image.new("RGBA", (2, 2), (35, 35, 41, 255))
        actual = expected.copy()
        actual.putpixel((1, 0), (36, 35, 41, 255))
        expected.save(self.expected / "sprite.png")
        actual.save(self.actual / "sprite.png")

        self.assertEqual(
            gen_sprites._sprite_asset_differences(self.expected, self.actual),
            ([], [], ["sprite.png"]),
        )

    def test_non_png_metadata_remains_byte_exact(self) -> None:
        (self.expected / "atlas.json").write_bytes(b'{"sprite":[0,0,2,2]}\n')
        (self.actual / "atlas.json").write_bytes(b'{ "sprite": [0, 0, 2, 2] }\n')

        self.assertEqual(
            gen_sprites._sprite_asset_differences(self.expected, self.actual),
            ([], [], ["atlas.json"]),
        )


if __name__ == "__main__":
    unittest.main()
