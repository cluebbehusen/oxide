import io
import struct
import unittest
import wave
from pathlib import Path

from tools import gen_sounds

MUSIC_NAMES = (
    "music_menu",
    "music_calm",
    "music_combat",
    "music_result",
    "music_victory",
    "music_defeat",
)


def pcm_wav(
    *,
    frames: int = gen_sounds.MUSIC_FRAMES,
    channels: int = 1,
    rate: int = gen_sounds.RATE,
    samples: bytes | None = None,
) -> bytes:
    data = io.BytesIO()
    with wave.open(data, "wb") as f:
        f.setnchannels(channels)
        f.setsampwidth(2)
        f.setframerate(rate)
        f.writeframes(samples or b"\0\0" * frames * channels)
    return data.getvalue()


class MusicPcmTests(unittest.TestCase):
    def test_checked_in_music_meets_the_loop_and_pcm_contract(self) -> None:
        sounds = Path(__file__).resolve().parent.parent / "assets" / "sounds"
        for name in MUSIC_NAMES:
            with self.subTest(name=name):
                metrics = gen_sounds.validate_music_wav(
                    name, (sounds / f"{name}.wav").read_bytes()
                )
                self.assertLess(metrics["peak"], gen_sounds.MUSIC_MAX_PEAK)
                self.assertLessEqual(metrics["dc"], gen_sounds.MUSIC_MAX_DC)
                self.assertLessEqual(metrics["seam"], gen_sounds.MUSIC_MAX_SEAM)
                self.assertLessEqual(metrics["delta"], gen_sounds.MUSIC_MAX_DELTA)

    def test_validator_rejects_wrong_format_and_duration(self) -> None:
        with self.assertRaisesRegex(ValueError, "mono 16-bit PCM"):
            gen_sounds.validate_music_wav("stereo", pcm_wav(channels=2))
        with self.assertRaisesRegex(ValueError, "expected .* frames"):
            gen_sounds.validate_music_wav(
                "short", pcm_wav(frames=gen_sounds.MUSIC_FRAMES - 1)
            )

    def test_validator_rejects_clipping(self) -> None:
        samples = bytearray(b"\0\0" * gen_sounds.MUSIC_FRAMES)
        struct.pack_into("<h", samples, len(samples) // 2, 32767)
        with self.assertRaisesRegex(ValueError, "headroom"):
            gen_sounds.validate_music_wav("clipped", pcm_wav(samples=samples))

    def test_validator_rejects_dc_offset(self) -> None:
        samples = struct.pack("<h", 32) * gen_sounds.MUSIC_FRAMES
        with self.assertRaisesRegex(ValueError, "DC offset"):
            gen_sounds.validate_music_wav("dc", pcm_wav(samples=samples))

    def test_validator_rejects_a_bad_loop_seam(self) -> None:
        samples = bytearray(b"\0\0" * gen_sounds.MUSIC_FRAMES)
        struct.pack_into("<h", samples, 0, 128)
        struct.pack_into("<h", samples, len(samples) - 2, -128)
        with self.assertRaisesRegex(ValueError, "loop seam"):
            gen_sounds.validate_music_wav("seam", pcm_wav(samples=samples))

    def test_validator_rejects_an_abrupt_sample_edge(self) -> None:
        samples = bytearray(b"\0\0" * gen_sounds.MUSIC_FRAMES)
        struct.pack_into("<h", samples, len(samples) // 2, 4096)
        with self.assertRaisesRegex(ValueError, "adjacent delta"):
            gen_sounds.validate_music_wav("edge", pcm_wav(samples=samples))


if __name__ == "__main__":
    unittest.main()
