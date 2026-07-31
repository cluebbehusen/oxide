# /// script
# requires-python = ">=3.14"
# ///
"""Generates every sound in assets/sounds/.

Same contract as gen_sprites.py: this script is the source of truth — edit,
run `uv run tools/gen_sounds.py`, commit script and WAVs together. Output is
deterministic (seeded noise, pure synthesis, no timestamps).

The palette is chunky 8-bit-adjacent synthesis: square-wave zaps, noise
bursts, little sine chimes, and restrained industrial score beds. Mono,
22050 Hz, 16-bit; the score stays lightweight enough to ship uncompressed.
"""

import argparse
import io
import math
import random
import wave
from pathlib import Path

RATE = 22050
OUT = Path(__file__).resolve().parent.parent / "assets" / "sounds"
GENERATED: dict[str, bytes] = {}


def write(name: str, samples: list[float]) -> None:
    data = io.BytesIO()
    with wave.open(data, "wb") as f:
        f.setnchannels(1)
        f.setsampwidth(2)
        f.setframerate(RATE)
        frames = bytearray()
        for s in samples:
            v = max(-1.0, min(1.0, s))
            frames += int(v * 32767).to_bytes(2, "little", signed=True)
        f.writeframes(bytes(frames))
    GENERATED[f"{name}.wav"] = data.getvalue()
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


def rail_fire() -> None:
    # The Lancer's shot: a deep falling square with a bass thud under it —
    # unmistakably heavier than the standard zap.
    n = int(0.16 * RATE)
    phase = 0.0
    out = []
    for i in range(n):
        freq = 620.0 - (420.0 * i / n)
        phase += freq / RATE
        square = 1.0 if (phase % 1.0) < 0.5 else -1.0
        thud = math.sin(2 * math.pi * 55.0 * i / RATE)
        out.append((0.42 * square + 0.4 * thud) * decay(i, n, 3.5))
    write("rail_fire", out)


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
        rumble = 0.6 * math.sin(2 * math.pi * 62.0 * t) + 0.4 * math.sin(
            2 * math.pi * 47.0 * t
        )
        out.append((0.5 * noise + 0.6 * rumble) * decay(i, n, 4.0))
    write("building_boom", out)


def flak() -> None:
    # A tight triple pop of bandpassed noise — anti-air fire bursting
    # against the sky, distinct from any ground zap.
    rng = random.Random(17)
    n = int(0.20 * RATE)
    out = [0.0] * n
    for burst_at in (0.0, 0.06, 0.12):
        start = int(burst_at * RATE)
        length = int(0.06 * RATE)
        band = 0.0
        for i in range(length):
            if start + i >= n:
                break
            # Cheap one-pole toward fresh noise keeps it hissy but round.
            band += 0.35 * (rng.uniform(-1.0, 1.0) - band)
            ring = math.sin(2 * math.pi * 900.0 * i / RATE)
            out[start + i] += (0.5 * band + 0.25 * ring) * decay(i, length, 6.0)
    write("flak", out)


def artillery_boom() -> None:
    # A shell landing: sharp crack into a rolling low boom — bigger than
    # the rail, smaller than a building coming down.
    rng = random.Random(29)
    n = int(0.4 * RATE)
    out = []
    for i in range(n):
        t = i / RATE
        crack = rng.uniform(-1.0, 1.0) * decay(i, n, 22.0)
        boom = (
            0.7 * math.sin(2 * math.pi * 70.0 * t)
            + 0.3 * math.sin(2 * math.pi * 52.0 * t)
        ) * decay(i, n, 5.0)
        out.append(0.5 * crack + 0.6 * boom)
    write("artillery_boom", out)


def chime(
    name: str, freqs: list[float], each: float, volume: float, dark: bool = False
) -> None:
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


def laser_alt() -> None:
    # The zap's sibling, a step lower — alternated per shot so volleys
    # read as many guns, not one clip on repeat.
    n = int(0.09 * RATE)
    phase = 0.0
    out = []
    for i in range(n):
        freq = 1280.0 - (700.0 * i / n)
        phase += freq / RATE
        square = 1.0 if (phase % 1.0) < 0.5 else -1.0
        out.append(0.5 * square * decay(i, n, 4.0))
    write("laser2", out)


def artillery_launch() -> None:
    # The gun speaking, not the shell arriving: a short muffled thump
    # with a breath of low noise — the boom belongs to the impact clip.
    n = int(0.14 * RATE)
    out = []
    seed = 1234567
    for i in range(n):
        seed = (seed * 1103515245 + 12345) % (2**31)
        noise = (seed / 2**31) * 2.0 - 1.0
        thump = math.sin(2 * math.pi * 62.0 * i / RATE)
        out.append((0.55 * thump + 0.18 * noise) * decay(i, n, 6.0))
    write("artillery_launch", out)


def loop_frequency(target: float, seconds: float) -> float:
    """Quantizes a tone to a whole number of cycles per loop."""
    return round(target * seconds) / seconds


def music_pad(
    name: str,
    tones: list[tuple[float, float]],
    *,
    seed: int,
    motion_cycles: int,
) -> None:
    """A slow industrial harmonic field, deliberately without a melody."""
    seconds = 12.0
    n = int(seconds * RATE)
    rng = random.Random(seed)
    phases = [rng.random() for _ in tones]
    freqs = [loop_frequency(freq, seconds) for freq, _ in tones]
    texture_phases = [rng.random(), rng.random()]
    texture_freqs = [
        loop_frequency(713.0, seconds),
        loop_frequency(997.0, seconds),
    ]
    weight = sum(level for _, level in tones)
    out: list[float] = []
    for i in range(n):
        t = i / RATE
        phase = i / n
        bed = 0.0
        for (_, level), freq, offset in zip(tones, freqs, phases, strict=True):
            fundamental = math.sin(2 * math.pi * (freq * t + offset))
            overtone = math.sin(2 * math.pi * (freq * 2.0 * t + offset * 0.5))
            bed += level * (0.82 * fundamental + 0.18 * overtone)
        bed /= weight
        motion = 0.78 + 0.22 * math.sin(
            2 * math.pi * (motion_cycles * phase + phases[0])
        )
        texture = sum(
            math.sin(2 * math.pi * (freq * t + offset))
            for freq, offset in zip(texture_freqs, texture_phases, strict=True)
        )
        out.append(0.58 * bed * motion + 0.006 * texture)
    write(name, out)


def music_combat() -> None:
    """A pressure stem that can sit over the calm match bed."""
    seconds = 12.0
    n = int(seconds * RATE)
    out = [0.0] * n
    rng = random.Random(140)
    beat = 0.75
    for step in range(int(seconds / beat)):
        start = int((0.3 + step * beat) * RATE)
        length = int(0.24 * RATE)
        for i in range(length):
            at = start + i
            if at >= n:
                break
            t = i / RATE
            sweep = 72.0 - 25.0 * i / length
            kick = math.sin(2 * math.pi * sweep * t) * decay(i, length, 7.0)
            out[at] += 0.68 * kick
        if step % 2 == 1:
            start += int(0.36 * RATE)
            length = int(0.11 * RATE)
            if start + length >= n:
                continue
            filtered = 0.0
            for i in range(length):
                at = start + i
                if at >= n:
                    break
                filtered += 0.5 * (rng.uniform(-1.0, 1.0) - filtered)
                out[at] += 0.24 * filtered * decay(i, length, 10.0)
    drone = loop_frequency(46.25, seconds)
    for i in range(n):
        t = i / RATE
        out[i] += 0.13 * math.sin(2 * math.pi * drone * t)
    write("music_combat", out)


def emit(check: bool) -> None:
    expected = set(GENERATED)
    actual = {path.name for path in OUT.glob("*.wav")}
    if check:
        if actual != expected:
            missing = sorted(expected - actual)
            extra = sorted(actual - expected)
            raise SystemExit(f"sound bank differs: missing={missing}, extra={extra}")
        changed = [
            name
            for name, data in GENERATED.items()
            if (OUT / name).read_bytes() != data
        ]
        if changed:
            raise SystemExit(f"sound bank is stale: {', '.join(sorted(changed))}")
        print(f"checked {len(GENERATED)} deterministic WAVs")
        return
    OUT.mkdir(parents=True, exist_ok=True)
    for name, data in GENERATED.items():
        (OUT / name).write_bytes(data)
    print(f"wrote {len(GENERATED)} deterministic WAVs")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify the checked-in WAV bank without rewriting it",
    )
    args = parser.parse_args()
    print(f"{'checking' if args.check else 'writing'} {OUT}")
    laser()
    laser_alt()
    rail_fire()
    artillery_launch()
    unit_death()
    building_boom()
    flak()
    artillery_boom()
    chime("deposit", [780.0, 1170.0], 0.06, 0.4)
    chime("train_done", [520.0, 660.0, 880.0], 0.05, 0.4)
    chime("click", [1100.0], 0.03, 0.35)
    chime("ack", [880.0, 990.0], 0.03, 0.3)
    chime("denied", [233.08, 174.61], 0.09, 0.4, dark=True)
    chime("victory", [523.25, 659.25, 783.99, 1046.5], 0.16, 0.45)
    chime("defeat", [392.0, 329.63, 261.63, 196.0], 0.16, 0.45, dark=True)
    music_pad(
        "music_menu",
        [(55.0, 1.0), (82.5, 0.7), (110.0, 0.45), (165.0, 0.18)],
        seed=114,
        motion_cycles=2,
    )
    music_pad(
        "music_calm",
        [(46.25, 1.0), (69.38, 0.7), (92.5, 0.42), (138.75, 0.16)],
        seed=214,
        motion_cycles=1,
    )
    music_combat()
    music_pad(
        "music_result",
        [(61.74, 1.0), (92.5, 0.65), (123.47, 0.4), (185.0, 0.14)],
        seed=314,
        motion_cycles=1,
    )
    music_pad(
        "music_victory",
        [(55.0, 1.0), (82.5, 0.62), (110.0, 0.4), (138.6, 0.3), (165.0, 0.16)],
        seed=414,
        motion_cycles=2,
    )
    music_pad(
        "music_defeat",
        [(49.0, 1.0), (73.5, 0.68), (98.0, 0.4), (116.5, 0.24), (147.0, 0.12)],
        seed=514,
        motion_cycles=1,
    )
    emit(args.check)
    print("done")


if __name__ == "__main__":
    main()
