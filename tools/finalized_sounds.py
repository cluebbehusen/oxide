"""Deterministic synthesis for the combat sounds approved for Oxide 0.14.

This module is deliberately independent of the temporary review generators.
The hashes below are the approval boundary: changing a synthesis primitive or
mix cannot silently replace a clip that was already auditioned and accepted.
"""

from __future__ import annotations

import hashlib
import io
import math
import random
import struct
import wave
from collections.abc import Callable, Sequence

RATE = 22_050
MAX_PEAK = 0.82

EXPECTED_SHA256 = {
    "attack_sentinel.wav": "91ed3cc9c56f61f2f21637ca4a2102f94b57f20ce69145e0020a17ae860a4e84",
    "attack_lancer.wav": "08f42926c8cdabf1db7f8fa6c51608530ccc650628603e720ef3f28e0df67d12",
    "attack_bombard.wav": "70be0fb1f6058dafb6b73a5e5a8e5d6bf349169da5c343bc3013581de021515c",
    "attack_flakhound.wav": "e37962a9f293f8c8635dd669ca64b3c085c437f17fd9efe63e50f2e68cad6614",
    "attack_stinger.wav": "658c52bcd4ab395bf51d2035db001c98aeb366c09e6d4eeb29d25e9d7319f429",
    "attack_buzzard.wav": "20b84177a33267d6761ac30616329437a4f4a7c86ef35e54c0fd14e18eca9c9e",
    "attack_darter.wav": "b859658367d0823af6ab09cc0997c7de729d5b8b28f4a74394bee38771560d2f",
    "attack_talon.wav": "2cfedec3cc81ea394b088ff7eed24891af6e2fef830f551adae7aa3712b830a0",
    "attack_wisp.wav": "cd20c9635e8691f7dbe4dc69c5676642d5e24ce99013a16da9f0191ff0580874",
    "attack_bastion.wav": "c1b65700bc07c46791065c012194bbf30abeeab0c926b697c6da2bb46b721c38",
    "attack_flak_turret.wav": "96646db4542ed4dd07eb8b477839ef0a47fbada0bb1c44f3ffa9b4827702f33c",
}


def _blank(seconds: float) -> list[float]:
    return [0.0] * round(seconds * RATE)


def _window(index: int, frames: int, attack: int, release: int) -> float:
    if index < attack:
        return 0.5 - 0.5 * math.cos(math.pi * index / max(1, attack - 1))
    if index >= frames - release:
        remaining = frames - 1 - index
        return 0.5 - 0.5 * math.cos(math.pi * remaining / max(1, release - 1))
    return 1.0


def _add_pressure_transient(
    out: list[float],
    at: float,
    volume: float,
    seed: int,
    *,
    duration: float = 0.030,
    brightness: float = 0.38,
) -> None:
    start = round(at * RATE)
    frames = round(duration * RATE)
    if start < 0 or start + frames > len(out):
        raise ValueError("pressure transient crosses the clip boundary")
    rng = random.Random(seed)
    attack = max(2, round(0.0012 * RATE))
    release = max(2, round(min(0.009, duration * 0.35) * RATE))
    fast = slow = previous = 0.0
    for index in range(frames):
        raw = rng.uniform(-1.0, 1.0)
        fast += min(brightness, 0.50) * (raw - fast)
        slow += 0.045 * (raw - slow)
        band = fast - slow
        edge = raw - previous
        previous = raw
        progress = index / max(1, frames - 1)
        envelope = _window(index, frames, attack, release) * math.exp(-5.8 * progress)
        out[start + index] += volume * envelope * (0.98 * band + 0.02 * edge)


def _add_body_modes(
    out: list[float],
    at: float,
    duration: float,
    volume: float,
    modes: Sequence[tuple[float, float]],
    *,
    decay: float = 5.0,
) -> None:
    start = round(at * RATE)
    frames = round(duration * RATE)
    if start < 0 or start + frames > len(out):
        raise ValueError("body resonance crosses the clip boundary")
    attack = max(2, round(0.0018 * RATE))
    release = max(2, round(min(0.035, duration * 0.25) * RATE))
    for index in range(frames):
        age = index / RATE
        progress = index / max(1, frames - 1)
        envelope = _window(index, frames, attack, release)
        color = 0.0
        for mode_index, (frequency, weight) in enumerate(modes):
            mode_decay = math.exp(-(decay + mode_index * 1.15) * progress)
            color += weight * mode_decay * math.sin(math.tau * frequency * age)
        out[start + index] += volume * envelope * color


def _add_mechanical_clack(
    out: list[float], at: float, volume: float, seed: int, *, dark: bool = False
) -> None:
    _add_pressure_transient(
        out,
        at,
        volume * 0.52,
        seed,
        duration=0.018,
        brightness=0.28 if dark else 0.48,
    )
    modes = (
        ((370.0, 0.56), (610.0, 0.29), (1030.0, 0.15))
        if dark
        else ((620.0, 0.50), (1040.0, 0.31), (1730.0, 0.19))
    )
    _add_body_modes(out, at, 0.075, volume * 0.48, modes, decay=8.5)


def _add_gun_report(
    out: list[float],
    at: float,
    volume: float,
    seed: int,
    modes: Sequence[tuple[float, float]],
    *,
    body_duration: float,
    crack_duration: float = 0.030,
    brightness: float = 0.40,
) -> None:
    _add_pressure_transient(
        out,
        at,
        volume,
        seed,
        duration=crack_duration,
        brightness=brightness,
    )
    _add_body_modes(out, at, body_duration, volume * 0.88, modes, decay=5.2)


def _finish(samples: list[float], target_peak: float) -> list[float]:
    if not samples:
        raise ValueError("cannot finish an empty sound")
    dc = sum(samples) / len(samples)
    centered = [sample - dc for sample in samples]
    fade = min(len(centered) // 3, max(2, round(0.002 * RATE)))
    for index in range(fade):
        gain = 0.5 - 0.5 * math.cos(math.pi * index / max(1, fade - 1))
        centered[index] *= gain
        centered[-1 - index] *= gain
    peak = max(abs(sample) for sample in centered)
    if peak == 0.0:
        raise ValueError("candidate is silent")
    if not 0.0 < target_peak <= MAX_PEAK:
        raise ValueError("target peak exceeds the sound-bank headroom budget")
    return [sample * target_peak / peak for sample in centered]


def _sentinel() -> list[float]:
    out = _blank(0.25)
    _add_gun_report(
        out,
        0.006,
        0.55,
        411,
        ((102.0, 0.58), (151.0, 0.28), (228.0, 0.14)),
        body_duration=0.16,
        crack_duration=0.027,
        brightness=0.46,
    )
    _add_mechanical_clack(out, 0.084, 0.23, 412)
    return _finish(out, 0.66)


def _bombard() -> list[float]:
    out = _blank(0.50)
    _add_gun_report(
        out,
        0.007,
        0.68,
        431,
        ((55.0, 0.57), (79.0, 0.29), (108.0, 0.14)),
        body_duration=0.30,
        crack_duration=0.034,
        brightness=0.32,
    )
    _add_mechanical_clack(out, 0.092, 0.20, 432, dark=True)
    _add_body_modes(out, 0.150, 0.14, 0.05, ((88.0, 0.7), (136.0, 0.3)), decay=8.0)
    return _finish(out, 0.70)


def _flakhound() -> list[float]:
    out = _blank(0.36)
    for offset, gain, seed in ((0.008, 0.53, 441), (0.098, 0.48, 442)):
        _add_gun_report(
            out,
            offset,
            gain,
            seed,
            ((83.0, 0.60), (124.0, 0.27), (182.0, 0.13)),
            body_duration=0.18,
            crack_duration=0.028,
            brightness=0.40,
        )
    _add_mechanical_clack(out, 0.178, 0.18, 443, dark=True)
    return _finish(out, 0.70)


def _stinger() -> list[float]:
    out = _blank(0.25)
    for offset, gain, seed in ((0.006, 0.42, 451), (0.052, 0.38, 452)):
        _add_gun_report(
            out,
            offset,
            gain,
            seed,
            ((145.0, 0.54), (224.0, 0.29), (340.0, 0.17)),
            body_duration=0.095,
            crack_duration=0.020,
            brightness=0.62,
        )
    _add_mechanical_clack(out, 0.108, 0.12, 453)
    return _finish(out, 0.60)


def _buzzard() -> list[float]:
    out = _blank(0.34)
    _add_gun_report(
        out,
        0.007,
        0.58,
        461,
        ((70.0, 0.58), (101.0, 0.27), (149.0, 0.15)),
        body_duration=0.25,
        crack_duration=0.029,
        brightness=0.35,
    )
    _add_mechanical_clack(out, 0.072, 0.15, 462, dark=True)
    return _finish(out, 0.70)


def _darter() -> list[float]:
    out = _blank(0.16)
    _add_gun_report(
        out,
        0.005,
        0.39,
        471,
        ((158.0, 0.51), (247.0, 0.31), (382.0, 0.18)),
        body_duration=0.085,
        crack_duration=0.018,
        brightness=0.72,
    )
    _add_mechanical_clack(out, 0.050, 0.09, 472)
    return _finish(out, 0.55)


def _talon() -> list[float]:
    out = _blank(0.29)
    _add_gun_report(
        out,
        0.006,
        0.51,
        481,
        ((88.0, 0.56), (132.0, 0.28), (197.0, 0.16)),
        body_duration=0.20,
        crack_duration=0.025,
        brightness=0.45,
    )
    _add_mechanical_clack(out, 0.066, 0.14, 482, dark=True)
    return _finish(out, 0.66)


def _wisp() -> list[float]:
    out = _blank(0.14)
    _add_pressure_transient(out, 0.004, 0.31, 491, duration=0.017, brightness=0.26)
    _add_body_modes(
        out,
        0.004,
        0.070,
        0.18,
        ((240.0, 0.52), (390.0, 0.30), (670.0, 0.18)),
        decay=9.5,
    )
    _add_mechanical_clack(out, 0.034, 0.10, 492, dark=True)
    return _finish(out, 0.50)


def _bastion() -> list[float]:
    out = _blank(0.62)
    _add_gun_report(
        out,
        0.008,
        0.76,
        501,
        ((43.0, 0.52), (61.0, 0.30), (83.0, 0.18)),
        body_duration=0.48,
        crack_duration=0.038,
        brightness=0.30,
    )
    _add_body_modes(
        out,
        0.008,
        0.52,
        0.32,
        ((36.0, 0.46), (52.0, 0.34), (74.0, 0.20)),
        decay=1.6,
    )
    _add_mechanical_clack(out, 0.095, 0.16, 502, dark=True)
    _add_body_modes(out, 0.170, 0.27, 0.11, ((68.0, 0.68), (104.0, 0.32)), decay=7.5)
    return _finish(out, 0.82)


def _flak_turret() -> list[float]:
    out = _blank(0.34)
    for offset, gain, seed in ((0.008, 0.55, 511), (0.075, 0.50, 512)):
        _add_gun_report(
            out,
            offset,
            gain,
            seed,
            ((91.0, 0.57), (137.0, 0.28), (205.0, 0.15)),
            body_duration=0.15,
            crack_duration=0.024,
            brightness=0.50,
        )
    _add_mechanical_clack(out, 0.148, 0.16, 513, dark=True)
    return _finish(out, 0.70)


def _raised_cosine_hit(index: int, frames: int, attack: int, release: int) -> float:
    if not (1 < attack < frames and 1 < release < frames):
        raise ValueError("a sound needs non-trivial attack and release")
    if index < attack:
        return 0.5 - 0.5 * math.cos(math.pi * index / (attack - 1))
    if index >= frames - release:
        remaining = frames - 1 - index
        return 0.5 - 0.5 * math.cos(math.pi * remaining / (release - 1))
    return 1.0


def _add_review_thump(
    out: list[float],
    at: float,
    volume: float,
    *,
    frequency: float,
    duration: float,
) -> None:
    start = round(at * RATE)
    frames = round(duration * RATE)
    attack = max(2, round(0.006 * RATE))
    release = max(2, round(0.18 * RATE))
    for index in range(frames):
        age = index / RATE
        envelope = _raised_cosine_hit(index, frames, attack, release)
        envelope *= math.exp(-4.8 * index / frames)
        body = 0.75 * math.sin(math.tau * frequency * age)
        body += 0.25 * math.sin(math.tau * frequency * 1.51 * age)
        out[start + index] += volume * envelope * body


def _add_review_plate(
    out: list[float],
    at: float,
    frequency: float,
    volume: float,
    *,
    duration: float,
    dark: bool,
) -> None:
    start = round(at * RATE)
    frames = round(duration * RATE)
    attack = max(2, round(0.008 * RATE))
    release = max(2, round(0.28 * RATE))
    partials = (
        ((1.0, 0.7), (1.43, 0.22), (2.71, 0.08))
        if dark
        else ((1.0, 0.58), (1.51, 0.23), (2.73, 0.13), (4.11, 0.06))
    )
    for index in range(frames):
        age = index / RATE
        envelope = _raised_cosine_hit(index, frames, attack, release)
        envelope *= math.exp(-4.0 * index / frames)
        color = sum(
            weight * math.sin(math.tau * frequency * ratio * age)
            for ratio, weight in partials
        )
        out[start + index] += volume * envelope * color


def _add_lancer_chirp(
    out: list[float],
    at: float,
    duration: float,
    start_hz: float,
    end_hz: float,
    volume: float,
) -> None:
    start = round(at * RATE)
    frames = round(duration * RATE)
    attack = max(2, round(min(0.008, duration * 0.16) * RATE))
    release = max(2, round(min(0.075, duration * 0.42) * RATE))
    phase = 0.0
    partials = ((1.0, 0.82), (1.49, 0.13), (2.13, 0.05))
    for index in range(frames):
        progress = index / max(1, frames - 1)
        frequency = start_hz * (end_hz / start_hz) ** progress
        phase += math.tau * frequency / RATE
        envelope = _raised_cosine_hit(index, frames, attack, release)
        envelope *= math.exp(-2.7 * progress)
        color = sum(weight * math.sin(phase * ratio) for ratio, weight in partials)
        out[start + index] += volume * envelope * color


def _finish_lancer(samples: list[float], target_peak: float) -> list[float]:
    dc = sum(samples) / len(samples)
    centered = [sample - dc for sample in samples]
    fade = min(len(centered) // 3, max(2, round(0.004 * RATE)))
    for index in range(fade):
        gain = 0.5 - 0.5 * math.cos(math.pi * index / max(1, fade - 1))
        centered[index] *= gain
        centered[-1 - index] *= gain
    peak = max(abs(sample) for sample in centered)
    return [sample * target_peak / peak for sample in centered]


def _lancer() -> list[float]:
    out = _blank(0.43)
    _add_review_thump(out, 0.010, 0.40, frequency=46.0, duration=0.38)
    _add_lancer_chirp(out, 0.012, 0.27, 1180.0, 145.0, 0.48)
    _add_review_plate(out, 0.018, 126.0, 0.15, duration=0.36, dark=True)
    return _finish_lancer(out, 0.76)


def _encode_wav(samples: Sequence[float]) -> bytes:
    peak = max(abs(sample) for sample in samples)
    if peak > MAX_PEAK + 1e-9:
        raise ValueError(f"sound peak {peak:.6f} exceeds {MAX_PEAK}")
    data = io.BytesIO()
    with wave.open(data, "wb") as audio:
        audio.setnchannels(1)
        audio.setsampwidth(2)
        audio.setframerate(RATE)
        frames = bytearray()
        for sample in samples:
            frames += struct.pack("<h", round(sample * 32767.0))
        audio.writeframes(frames)
    return data.getvalue()


BUILDERS: dict[str, Callable[[], list[float]]] = {
    "attack_sentinel.wav": _sentinel,
    "attack_lancer.wav": _lancer,
    "attack_bombard.wav": _bombard,
    "attack_flakhound.wav": _flakhound,
    "attack_stinger.wav": _stinger,
    "attack_buzzard.wav": _buzzard,
    "attack_darter.wav": _darter,
    "attack_talon.wav": _talon,
    "attack_wisp.wav": _wisp,
    "attack_bastion.wav": _bastion,
    "attack_flak_turret.wav": _flak_turret,
}


def finalized_wavs() -> dict[str, bytes]:
    """Returns the exact approved clips under their production filenames."""
    encoded = {name: _encode_wav(builder()) for name, builder in BUILDERS.items()}
    actual = {name: hashlib.sha256(data).hexdigest() for name, data in encoded.items()}
    if actual != EXPECTED_SHA256:
        changed = sorted(
            name
            for name in EXPECTED_SHA256
            if actual.get(name) != EXPECTED_SHA256[name]
        )
        raise ValueError(f"approved combat audio changed: {', '.join(changed)}")
    return encoded
