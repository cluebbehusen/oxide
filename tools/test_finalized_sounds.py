import hashlib
import io
import unittest
import wave

from tools.finalized_sounds import EXPECTED_SHA256, RATE, finalized_wavs


class FinalizedSoundsTests(unittest.TestCase):
    def test_approved_bank_is_byte_exact(self) -> None:
        generated = finalized_wavs()
        self.assertEqual(set(generated), set(EXPECTED_SHA256))
        self.assertEqual(
            {
                name: hashlib.sha256(data).hexdigest()
                for name, data in generated.items()
            },
            EXPECTED_SHA256,
        )

    def test_clips_use_the_shipping_pcm_contract(self) -> None:
        for name, data in finalized_wavs().items():
            with self.subTest(name=name), wave.open(io.BytesIO(data), "rb") as audio:
                self.assertEqual(audio.getnchannels(), 1)
                self.assertEqual(audio.getsampwidth(), 2)
                self.assertEqual(audio.getframerate(), RATE)
                self.assertEqual(audio.getcomptype(), "NONE")
                self.assertGreater(audio.getnframes(), 0)


if __name__ == "__main__":
    unittest.main()
