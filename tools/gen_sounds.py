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
import struct
import wave
from itertools import pairwise
from pathlib import Path

RATE = 22050
MUSIC_SECONDS = 12
MUSIC_FRAMES = MUSIC_SECONDS * RATE
MUSIC_MAX_PEAK = 0.82
MUSIC_MAX_DC = 0.0005
MUSIC_MAX_SEAM = 0.001
# Five percent of full scale admits the authored upper partials but rejects
# the one-sample edge that produced the combat-layer speaker pop.
MUSIC_MAX_DELTA = 0.05
OUT = Path(__file__).resolve().parent.parent / "assets" / "sounds"
GENERATED: dict[str, bytes] = {}


def validate_music_wav(name: str, data: bytes) -> dict[str, float]:
    """Checks the score's loop, PCM format, and click-prevention budget."""
    with wave.open(io.BytesIO(data), "rb") as f:
        if (f.getnchannels(), f.getsampwidth(), f.getframerate()) != (1, 2, RATE):
            raise ValueError(f"{name}: music must be mono 16-bit PCM at {RATE} Hz")
        if f.getnframes() != MUSIC_FRAMES:
            raise ValueError(
                f"{name}: expected {MUSIC_FRAMES} frames, got {f.getnframes()}"
            )
        if f.getcomptype() != "NONE":
            raise ValueError(f"{name}: music must be uncompressed PCM")
        frames = f.readframes(MUSIC_FRAMES)

    pcm = [sample[0] for sample in struct.iter_unpack("<h", frames)]
    normalized = [sample / 32768.0 for sample in pcm]
    peak = max(abs(sample) for sample in normalized)
    dc = abs(sum(normalized) / len(normalized))
    seam = abs(normalized[0] - normalized[-1])
    delta = max(abs(current - previous) for previous, current in pairwise(normalized))
    if max(abs(sample) for sample in pcm) >= 32767 or peak > MUSIC_MAX_PEAK:
        raise ValueError(f"{name}: peak {peak:.6f} leaves inadequate headroom")
    if dc > MUSIC_MAX_DC:
        raise ValueError(f"{name}: DC offset {dc:.6f} exceeds {MUSIC_MAX_DC}")
    if seam > MUSIC_MAX_SEAM:
        raise ValueError(f"{name}: loop seam {seam:.6f} exceeds {MUSIC_MAX_SEAM}")
    if delta > MUSIC_MAX_DELTA:
        raise ValueError(
            f"{name}: adjacent delta {delta:.6f} exceeds {MUSIC_MAX_DELTA}"
        )
    return {"peak": peak, "dc": dc, "seam": seam, "delta": delta}


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
    wav = data.getvalue()
    GENERATED[f"{name}.wav"] = wav
    detail = ""
    if name.startswith("music_"):
        metrics = validate_music_wav(name, wav)
        detail = (
            f", peak {metrics['peak']:.3f}, seam {metrics['seam']:.6f}, "
            f"dc {metrics['dc']:.6f}, delta {metrics['delta']:.3f}"
        )
    print(f"  {name}.wav ({len(samples) / RATE:.2f}s{detail})")


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


MUSIC_BEAT = 2.0 / 3.0
MUSIC_STEP = MUSIC_BEAT / 2.0
MUSIC_PHRASE = 6.0 * MUSIC_BEAT


def raised_cosine_hit(i: int, n: int, attack: int, release: int) -> float:
    """A click-free attack/release envelope that reaches zero at both ends."""
    if not (1 < attack < n and 1 < release < n):
        raise ValueError("a music hit needs non-trivial attack and release")
    if i < attack:
        return 0.5 - 0.5 * math.cos(math.pi * i / (attack - 1))
    release_at = n - release
    if i >= release_at:
        remaining = n - 1 - i
        return 0.5 - 0.5 * math.cos(math.pi * remaining / (release - 1))
    return 1.0


def note_frequency(root: float, semitones: int) -> float:
    return root * 2.0 ** (semitones / 12.0)


def add_tone_hit(
    out: list[float],
    start_seconds: float,
    duration: float,
    frequency: float,
    volume: float,
    *,
    metallic: bool,
) -> None:
    """Adds one gated tone; even its faintest partial shares the envelope."""
    start = round(start_seconds * RATE)
    n = round(duration * RATE)
    if start < 0 or start + n > len(out):
        raise ValueError("music hit crosses the loop boundary")
    attack = max(2, round((0.014 if metallic else 0.022) * RATE))
    release = max(2, round((0.07 if metallic else 0.11) * RATE))
    partials = (
        ((1.0, 0.74), (2.01, 0.17), (3.97, 0.07), (6.03, 0.02))
        if metallic
        else ((1.0, 0.82), (2.0, 0.18))
    )
    for i in range(n):
        envelope = raised_cosine_hit(i, n, attack, release)
        age = i / RATE
        color = sum(
            weight * math.sin(2.0 * math.pi * frequency * ratio * age)
            for ratio, weight in partials
        )
        out[start + i] += volume * envelope * color


def add_noise_hit(
    out: list[float],
    start_seconds: float,
    duration: float,
    volume: float,
    rng: random.Random,
) -> None:
    """Adds a softened metal scrape with no discontinuous noise edge."""
    start = round(start_seconds * RATE)
    n = round(duration * RATE)
    if start < 0 or start + n > len(out):
        raise ValueError("music noise crosses the loop boundary")
    attack = max(2, round(0.012 * RATE))
    release = max(2, round(0.055 * RATE))
    fast = 0.0
    slow = 0.0
    for i in range(n):
        raw = rng.uniform(-1.0, 1.0)
        fast += 0.16 * (raw - fast)
        slow += 0.035 * (raw - slow)
        band = fast - slow
        ring = math.sin(2.0 * math.pi * 1080.0 * i / RATE)
        envelope = raised_cosine_hit(i, n, attack, release)
        out[start + i] += volume * envelope * (0.78 * band + 0.22 * ring)


def finish_music(out: list[float]) -> list[float]:
    """Centers a stem while retaining the identical silent loop endpoints."""
    dc = sum(out) / len(out)
    return [sample - dc for sample in out]


def music_bed(
    name: str,
    *,
    root: float,
    ostinato: tuple[int | None, ...],
    motif: tuple[int, ...],
    bass: tuple[int, ...],
    seed: int,
) -> None:
    """A gated industrial pulse with a sparse four-note melodic phrase."""
    if len(ostinato) != 12 or len(motif) != 12 or len(bass) != 9:
        raise ValueError("music beds require three complete four-second phrases")
    out = [0.0] * MUSIC_FRAMES
    rng = random.Random(seed)
    motif_steps = (2, 5, 8, 10)
    for phrase in range(3):
        phrase_at = phrase * MUSIC_PHRASE
        for step, degree in enumerate(ostinato):
            if degree is None or step in motif_steps:
                continue
            add_tone_hit(
                out,
                phrase_at + step * MUSIC_STEP,
                0.19,
                note_frequency(root * 2.0, degree),
                0.09,
                metallic=True,
            )
        for step, degree in zip(
            motif_steps, motif[phrase * 4 : phrase * 4 + 4], strict=True
        ):
            add_tone_hit(
                out,
                phrase_at + step * MUSIC_STEP,
                0.3,
                note_frequency(root * 4.0, degree),
                0.15,
                metallic=True,
            )
        for bass_slot, degree in enumerate(bass[phrase * 3 : phrase * 3 + 3]):
            add_tone_hit(
                out,
                phrase_at + bass_slot * 2.0 * MUSIC_BEAT,
                0.44,
                note_frequency(root, degree),
                0.18,
                metallic=False,
            )
        for step in (3, 9):
            add_noise_hit(
                out,
                phrase_at + step * MUSIC_STEP,
                0.15,
                0.055,
                rng,
            )
    write(name, finish_music(out))


def music_combat() -> None:
    """A syncopated D-minor pressure layer for the calm match stem."""
    out = [0.0] * MUSIC_FRAMES
    rng = random.Random(140)
    motif = (0, 7, 10, 3, 0, 10, 7, 12, 10, 7, 3, 0)
    motif_steps = (1, 4, 7, 10)
    for phrase in range(3):
        phrase_at = phrase * MUSIC_PHRASE
        for beat in range(6):
            degree = 0 if beat % 3 != 2 else -5
            add_tone_hit(
                out,
                phrase_at + beat * MUSIC_BEAT,
                0.26,
                note_frequency(73.42, degree),
                0.2,
                metallic=False,
            )
        for step, degree in zip(
            motif_steps, motif[phrase * 4 : phrase * 4 + 4], strict=True
        ):
            add_tone_hit(
                out,
                phrase_at + step * MUSIC_STEP,
                0.22,
                note_frequency(73.42 * 4.0, degree),
                0.18,
                metallic=True,
            )
        for step in (1, 3, 5, 7, 9, 11):
            add_noise_hit(
                out,
                phrase_at + step * MUSIC_STEP,
                0.12,
                0.065,
                rng,
            )
    write("music_combat", finish_music(out))


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
    music_bed(
        "music_menu",
        root=82.41,
        ostinato=(0, None, 7, 0, 3, None, 7, 10, 0, None, 3, 7),
        motif=(0, 3, 7, 10, 0, 7, 12, 10, 7, 3, 0, 7),
        bass=(0, 0, 3, 0, 10, 7, 0, 3, 0),
        seed=114,
    )
    music_bed(
        "music_calm",
        root=73.42,
        ostinato=(0, None, 7, 0, 3, None, 0, 10, 0, None, 7, 3),
        motif=(0, 3, 7, 10, 0, 7, 3, 12, 10, 7, 3, 0),
        bass=(0, 0, 3, 0, 10, 7, 0, 3, 0),
        seed=214,
    )
    music_combat()
    music_bed(
        "music_result",
        root=65.41,
        ostinato=(0, None, 7, 0, 5, None, 10, 7, 0, None, 5, 10),
        motif=(0, 5, 7, 10, 0, 7, 5, 12, 10, 7, 5, 0),
        bass=(0, 0, 5, 0, 10, 7, 0, 5, 0),
        seed=314,
    )
    music_bed(
        "music_victory",
        root=73.42,
        ostinato=(0, None, 7, 0, 4, None, 7, 12, 0, None, 4, 7),
        motif=(0, 4, 7, 12, 4, 7, 11, 12, 7, 4, 9, 12),
        bass=(0, 0, 5, 0, 7, 5, 0, 5, 0),
        seed=414,
    )
    music_bed(
        "music_defeat",
        root=73.42,
        ostinato=(0, None, 3, 0, -2, None, 3, 7, 0, None, -2, 3),
        motif=(10, 7, 3, 0, 7, 3, 0, -2, 3, 0, -2, -5),
        bass=(0, -2, -5, 0, -2, -5, 0, -5, -12),
        seed=514,
    )
    emit(args.check)
    print("done")


if __name__ == "__main__":
    main()
