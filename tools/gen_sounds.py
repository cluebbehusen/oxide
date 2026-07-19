# /// script
# requires-python = ">=3.10"
# ///
"""Generates every sound in assets/sounds/.

Same contract as gen_sprites.py: this script is the source of truth — edit,
run `uv run tools/gen_sounds.py`, commit script and WAVs together. Output is
deterministic (seeded noise, pure synthesis, no timestamps).

The palette is chunky 8-bit-adjacent synthesis: square-wave zaps, noise
bursts, and little sine chimes. Mono, 22050 Hz, 16-bit — a few dozen KB for
the whole set.
"""

from __future__ import annotations

import math
import random
import wave
from pathlib import Path

RATE = 22050
OUT = Path(__file__).resolve().parent.parent / "assets" / "sounds"


def write(name: str, samples: list[float]) -> None:
    with wave.open(str(OUT / f"{name}.wav"), "wb") as f:
        f.setnchannels(1)
        f.setsampwidth(2)
        f.setframerate(RATE)
        frames = bytearray()
        for s in samples:
            v = max(-1.0, min(1.0, s))
            frames += int(v * 32767).to_bytes(2, "little", signed=True)
        f.writeframes(bytes(frames))
    print(f"  {name}.wav ({len(samples) / RATE:.2f}s)")


def decay(i: int, n: int, sharpness: float = 5.0) -> float:
    """Exponential fade-out over the clip."""
    return math.exp(-sharpness * i / n)


def laser() -> None:
    # A falling square-wave zap.
    n = int(0.09 * RATE)
    phase = 0.0
    out = []
    for i in range(n):
        freq = 1500.0 - (850.0 * i / n)
        phase += freq / RATE
        square = 1.0 if (phase % 1.0) < 0.5 else -1.0
        out.append(0.5 * square * decay(i, n, 4.0))
    write("laser", out)


def unit_death() -> None:
    # A crunchy noise pop with a thud underneath.
    rng = random.Random(3)
    n = int(0.22 * RATE)
    out = []
    for i in range(n):
        noise = rng.uniform(-1.0, 1.0)
        thud = math.sin(2 * math.pi * 150.0 * i / RATE)
        out.append((0.55 * noise + 0.45 * thud) * decay(i, n, 6.0))
    write("unit_death", out)


def building_boom() -> None:
    # Long rumble: heavy noise over sub-bass sines.
    rng = random.Random(9)
    n = int(0.55 * RATE)
    out = []
    for i in range(n):
        t = i / RATE
        noise = rng.uniform(-1.0, 1.0)
        rumble = 0.6 * math.sin(2 * math.pi * 62.0 * t) + 0.4 * math.sin(2 * math.pi * 47.0 * t)
        out.append((0.5 * noise + 0.6 * rumble) * decay(i, n, 4.0))
    write("building_boom", out)


def chime(name: str, freqs: list[float], each: float, volume: float, dark: bool = False) -> None:
    """A little melody of overlapping sine notes."""
    step = int(each * RATE)
    total = step * len(freqs) + int(0.15 * RATE)
    out = [0.0] * total
    for note, freq in enumerate(freqs):
        start = note * step
        length = int(each * 1.6 * RATE)
        for i in range(length):
            if start + i >= total:
                break
            t = i / RATE
            s = math.sin(2 * math.pi * freq * t)
            if dark:
                s = 0.7 * s + 0.3 * math.sin(2 * math.pi * freq * 0.5 * t)
            out[start + i] += volume * s * decay(i, length, 4.0)
    write(name, out)


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    print(f"writing {OUT}")
    laser()
    unit_death()
    building_boom()
    chime("deposit", [780.0, 1170.0], 0.06, 0.4)
    chime("train_done", [520.0, 660.0, 880.0], 0.05, 0.4)
    chime("click", [1100.0], 0.03, 0.35)
    chime("victory", [523.25, 659.25, 783.99, 1046.5], 0.16, 0.45)
    chime("defeat", [392.0, 329.63, 261.63, 196.0], 0.16, 0.45, dark=True)
    print("done")


if __name__ == "__main__":
    main()
