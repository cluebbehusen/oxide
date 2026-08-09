import tempfile
import unittest
from pathlib import Path

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
