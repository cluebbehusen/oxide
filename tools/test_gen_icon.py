"""Reproducibility and format contracts for the application icon."""

import hashlib
import json
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
            "oxide_desktop_1024.png",
            "oxide_256.png",
            "oxide_16.rgba",
            "oxide_32.rgba",
            "oxide_64.rgba",
        }

        with tempfile.TemporaryDirectory(prefix="oxide-icon-test-") as temp:
            output = Path(temp)
            with (
                patch.object(gen_icon, "OUT", output / "desktop"),
                patch.object(gen_icon, "IOS_OUT", output / "ios"),
                redirect_stdout(StringIO()),
            ):
                gen_icon.main()

            self.assertEqual(
                {path.name for path in (output / "desktop").iterdir()},
                expected_names,
                "the generator must neither omit nor invent packaged icon files",
            )
            for name in expected_names:
                with self.subTest(name=name):
                    actual = output / "desktop" / name
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

            self.assertEqual(
                {path.name for path in (output / "ios").iterdir()},
                {"oxide_1024.png"},
            )
            self.assertEqual(
                (output / "ios" / "oxide_1024.png").read_bytes(),
                (gen_icon.IOS_OUT / "oxide_1024.png").read_bytes(),
            )

    def test_ios_catalog_uses_the_approved_opaque_master(self) -> None:
        catalog = json.loads((gen_icon.IOS_OUT / "Contents.json").read_text())
        self.assertEqual(len(catalog["images"]), 1)
        entry = catalog["images"][0]
        self.assertEqual(entry["idiom"], "universal")
        self.assertEqual(entry["platform"], "ios")
        self.assertEqual(entry["size"], "1024x1024")
        ios_icon = gen_icon.IOS_OUT / entry["filename"]
        self.assertEqual(
            ios_icon.read_bytes(), (gen_icon.OUT / "oxide_1024.png").read_bytes()
        )
        with Image.open(ios_icon) as image:
            self.assertEqual(image.mode, "RGB")
            self.assertEqual(image.size, (1024, 1024))
            self.assertEqual(
                hashlib.sha256(image.tobytes()).hexdigest(),
                "a28bb9efbbd7d63bd633d9ac6d59d8c6124e42c71265a1d22813b1c4d987ad1f",
                "the approved icon artwork must remain unchanged across exports",
            )

    def test_desktop_mask_only_removes_background(self) -> None:
        with (
            Image.open(gen_icon.OUT / "oxide_1024.png") as master,
            Image.open(gen_icon.OUT / "oxide_desktop_1024.png") as desktop,
        ):
            self.assertEqual(desktop.mode, "RGBA")
            self.assertEqual(desktop.size, master.size)
            self.assertEqual(desktop.convert("RGB").tobytes(), master.tobytes())
            removed_colors = {
                rgb
                for rgb, rgba in zip(
                    master.get_flattened_data(), desktop.get_flattened_data()
                )
                if rgba[3] != 255
            }
            self.assertEqual(removed_colors, {(28, 28, 34)})

    def test_raw_window_icons_have_exact_rgba_dimensions(self) -> None:
        with tempfile.TemporaryDirectory(prefix="oxide-icon-format-") as temp:
            output = Path(temp)
            with (
                patch.object(gen_icon, "OUT", output / "desktop"),
                patch.object(gen_icon, "IOS_OUT", output / "ios"),
                redirect_stdout(StringIO()),
            ):
                gen_icon.main()

            for size in (16, 32, 64):
                with self.subTest(size=size):
                    raw = (output / "desktop" / f"oxide_{size}.rgba").read_bytes()
                    self.assertEqual(len(raw), size * size * 4)
                    image = Image.frombytes("RGBA", (size, size), raw)
                    alpha = image.getchannel("A")
                    self.assertEqual(alpha.getpixel((0, 0)), 0)
                    self.assertEqual(alpha.getpixel((size // 2, size // 2)), 255)


if __name__ == "__main__":
    unittest.main()
