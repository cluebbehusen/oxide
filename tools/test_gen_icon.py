"""Reproducibility and format contracts for the application icon."""

import tempfile
import unittest
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path
from unittest.mock import patch

from PIL import Image

from tools import gen_icon


class IconGenerationTests(unittest.TestCase):
    def test_generation_reproduces_every_checked_in_icon_artifact(self) -> None:
        committed = Path(__file__).resolve().parent.parent / "assets" / "icon"
        expected_names = {
            "oxide_1024.png",
            "oxide_256.png",
            "oxide_16.rgba",
            "oxide_32.rgba",
            "oxide_64.rgba",
        }

        with tempfile.TemporaryDirectory(prefix="oxide-icon-test-") as temp:
            output = Path(temp)
            with patch.object(gen_icon, "OUT", output), redirect_stdout(StringIO()):
                gen_icon.main()

            self.assertEqual(
                {path.name for path in output.iterdir()},
                expected_names,
                "the generator must neither omit nor invent packaged icon files",
            )
            for name in expected_names:
                with self.subTest(name=name):
                    actual = output / name
                    expected = committed / name
                    if name.endswith(".png"):
                        with (
                            Image.open(actual) as actual_image,
                            Image.open(expected) as expected_image,
                        ):
                            self.assertEqual(actual_image.mode, expected_image.mode)
                            self.assertEqual(actual_image.size, expected_image.size)
                            self.assertEqual(
                                actual_image.tobytes(),
                                expected_image.tobytes(),
                                f"{name} pixels no longer reproduce from tools/gen_icon.py",
                            )
                    else:
                        self.assertEqual(
                            actual.read_bytes(),
                            expected.read_bytes(),
                            f"{name} no longer reproduces from tools/gen_icon.py",
                        )

    def test_raw_window_icons_have_exact_rgba_dimensions(self) -> None:
        with tempfile.TemporaryDirectory(prefix="oxide-icon-format-") as temp:
            output = Path(temp)
            with patch.object(gen_icon, "OUT", output), redirect_stdout(StringIO()):
                gen_icon.main()

            for size in (16, 32, 64):
                with self.subTest(size=size):
                    raw = (output / f"oxide_{size}.rgba").read_bytes()
                    self.assertEqual(len(raw), size * size * 4)
                    image = Image.frombytes("RGBA", (size, size), raw)
                    alpha = image.getchannel("A")
                    self.assertEqual(alpha.getpixel((0, 0)), 0)
                    self.assertEqual(alpha.getpixel((size // 2, size // 2)), 255)


if __name__ == "__main__":
    unittest.main()
