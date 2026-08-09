import hashlib
import io
import json
import math
import struct
import tempfile
import unittest
import wave
from contextlib import redirect_stdout
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
SFX_NAMES = tuple(row[0] for row in gen_sounds.SFX_METADATA)


def pcm_wav(
    *,
    frames: int = gen_sounds.MUSIC_FRAMES,
    channels: int = 1,
    rate: int = gen_sounds.RATE,
    samples: bytes | bytearray | None = None,
) -> bytes:
    data = io.BytesIO()
    with wave.open(data, "wb") as f:
        f.setnchannels(channels)
        f.setsampwidth(2)
        f.setframerate(rate)
        f.writeframes(samples or b"\0\0" * frames * channels)
    return data.getvalue()


class SfxPcmTests(unittest.TestCase):
    def test_checked_in_effects_match_approval_hashes_and_audio_budgets(self) -> None:
        sounds = Path(__file__).resolve().parent.parent / "assets" / "sounds"
        self.assertEqual(set(SFX_NAMES), set(gen_sounds.SFX_BUILDERS))
        self.assertEqual(
            {f"{name}.wav" for name in SFX_NAMES},
            set(gen_sounds.EXPECTED_SFX_SHA256),
        )
        for name in SFX_NAMES:
            with self.subTest(name=name):
                data = (sounds / f"{name}.wav").read_bytes()
                metrics = gen_sounds.validate_sfx_wav(name, data)
                self.assertEqual(
                    hashlib.sha256(data).hexdigest(),
                    gen_sounds.EXPECTED_SFX_SHA256[f"{name}.wav"],
                )
                self.assertLessEqual(metrics["peak"], gen_sounds.SFX_MAX_PEAK)
                self.assertLessEqual(metrics["dc"], gen_sounds.SFX_MAX_DC)

    def test_manifest_covers_each_approved_effect_once(self) -> None:
        sounds = Path(__file__).resolve().parent.parent / "assets" / "sounds"
        manifest = (sounds / "manifest.json").read_bytes()
        self.assertEqual(manifest, gen_sounds.manifest_bytes())
        parsed = json.loads(manifest)
        entries = parsed["sounds"]
        self.assertEqual([entry["name"] for entry in entries], list(SFX_NAMES))
        self.assertEqual(
            {entry["filename"] for entry in entries},
            {f"{name}.wav" for name in SFX_NAMES},
        )
        for entry in entries:
            self.assertGreater(entry["mixer_volume"], 0.0)
            self.assertGreater(entry["min_gap"], 0.0)

    def test_sfx_validator_rejects_wrong_rate(self) -> None:
        with self.assertRaisesRegex(ValueError, "44100 Hz"):
            gen_sounds.validate_sfx_wav(
                "laser", pcm_wav(frames=100, rate=gen_sounds.RATE)
            )

    def test_sfx_validator_rejects_clipping_and_dc(self) -> None:
        frames = round(0.2 * gen_sounds.SFX_RATE)
        clipped = bytearray(b"\0\0" * frames)
        struct.pack_into("<h", clipped, len(clipped) // 2, 32767)
        with self.assertRaisesRegex(ValueError, "headroom"):
            gen_sounds.validate_sfx_wav(
                "laser",
                pcm_wav(
                    frames=frames,
                    rate=gen_sounds.SFX_RATE,
                    samples=clipped,
                ),
            )
        with self.assertRaisesRegex(ValueError, "DC offset"):
            gen_sounds.validate_sfx_wav(
                "laser",
                pcm_wav(
                    frames=frames,
                    rate=gen_sounds.SFX_RATE,
                    samples=struct.pack("<h", 256) * frames,
                ),
            )

    def test_sfx_validator_rejects_low_speaker_energy_and_long_clips(self) -> None:
        frames = round(0.2 * gen_sounds.SFX_RATE)
        low = b"".join(
            struct.pack(
                "<h",
                round(12000 * math.sin(math.tau * 90.0 * index / gen_sounds.SFX_RATE)),
            )
            for index in range(frames)
        )
        with self.assertRaisesRegex(ValueError, "spectral energy"):
            gen_sounds.validate_sfx_wav(
                "laser",
                pcm_wav(
                    frames=frames,
                    rate=gen_sounds.SFX_RATE,
                    samples=low,
                ),
            )

        long_frames = round(0.7 * gen_sounds.SFX_RATE)
        with self.assertRaisesRegex(ValueError, "duration"):
            gen_sounds.validate_sfx_wav(
                "laser",
                pcm_wav(
                    frames=long_frames,
                    rate=gen_sounds.SFX_RATE,
                    samples=b"\0\0" * long_frames,
                ),
            )


class MusicPcmTests(unittest.TestCase):
    def test_generate_can_write_two_isolated_banks_in_one_process(self) -> None:
        committed = Path(__file__).resolve().parent.parent / "assets" / "sounds"
        with tempfile.TemporaryDirectory(prefix="oxide sound review ") as temp:
            first = Path(temp) / "first bank"
            second = Path(temp) / "second bank"
            with redirect_stdout(io.StringIO()):
                gen_sounds.generate(first)
                gen_sounds.generate(second)

            expected = {
                path.name: path.read_bytes() for path in committed.glob("*.wav")
            }
            for output in (first, second):
                actual = {path.name: path.read_bytes() for path in output.glob("*.wav")}
                self.assertEqual(actual, expected)
                self.assertEqual(
                    (output / "manifest.json").read_bytes(),
                    gen_sounds.manifest_bytes(),
                )

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
