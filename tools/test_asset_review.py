import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VIEWER = ROOT / "tools" / "asset_review.html"


class AssetReviewTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.source = VIEWER.read_text(encoding="utf-8")

    def test_directory_selection_supports_native_and_compatibility_pickers(
        self,
    ) -> None:
        directory_input = re.search(r'<input id="directory"[^>]+>', self.source)
        self.assertIsNotNone(directory_input)
        markup = directory_input.group(0)
        self.assertIn('type="file"', markup)
        self.assertIn("webkitdirectory", markup)
        self.assertIn("directory", markup)
        self.assertIn("multiple", markup)
        self.assertIn("window.showDirectoryPicker", self.source)
        self.assertIn("await collectFiles(handle, handle.name, files)", self.source)
        self.assertIn("file.webkitRelativePath", self.source)

    def test_images_and_gifs_open_in_a_dismissible_lightbox(self) -> None:
        self.assertIn('id="lightbox"', self.source)
        self.assertIn("function enableZoom(element, file)", self.source)
        self.assertIn("openLightbox(file, element)", self.source)
        self.assertIn("enableZoom(image, file)", self.source)
        self.assertIn("enableZoom(canvas, file)", self.source)
        self.assertIn('event.key === "Escape"', self.source)
        self.assertIn("closeLightbox()", self.source)

    def test_leading_numeric_filename_is_a_stable_review_id(self) -> None:
        self.assertIn("function reviewNumber(file, index)", self.source)
        self.assertIn(r"file.name.match(/^(\d{2,4})(?=[._ -])/)", self.source)
        self.assertIn('String(index + 1).padStart(2, "0")', self.source)
        self.assertIn("number.textContent = reviewNumber(file, index)", self.source)

    def test_gif_controls_start_stop_and_freeze_the_preview(self) -> None:
        self.assertIn('button.textContent = "Start GIF"', self.source)
        self.assertIn('button.textContent = "Stop GIF"', self.source)
        self.assertIn("if (playing)", self.source)
        self.assertIn("freeze();", self.source)
        self.assertIn("load(true);", self.source)
        self.assertIn('image.removeAttribute("src")', self.source)

    def test_audio_controls_start_stop_and_honor_loop_files(self) -> None:
        self.assertIn('file.name.toLowerCase().includes("-loop")', self.source)
        self.assertIn("audio.loop = isLoop", self.source)
        self.assertIn('isLoop ? "Start loop" : "Start audio"', self.source)
        self.assertIn('isLoop ? "Stop loop" : "Stop audio"', self.source)
        self.assertIn("await audio.play()", self.source)
        self.assertIn("audio.pause()", self.source)
        self.assertIn("audio.currentTime = 0", self.source)
        self.assertIn('audio.addEventListener("ended"', self.source)


if __name__ == "__main__":
    unittest.main()
