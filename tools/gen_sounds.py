# /// script
# requires-python = ">=3.11"
# dependencies = [
#   "numpy==2.5.1",
#   "scipy==1.18.0",
# ]
# ///
"""Generates every sound in assets/sounds/.

Same contract as gen_sprites.py: this script is the source of truth — edit,
run `uv run tools/gen_sounds.py`, commit script and WAVs together. Output is
deterministic (seeded noise, pure synthesis, no timestamps).

The effects use dry, synth-first 8-bit-adjacent gestures at 44100 Hz. The
temporary generated score remains 22050 Hz until the licensed soundtrack
replaces it. Every file is mono 16-bit PCM.

Pass --out DIRECTORY to write a complete alternate bank for review without
touching the checked-in assets.
"""

import argparse
import hashlib
import io
import json
import math
import random
import struct
import wave
from itertools import pairwise
from pathlib import Path

import numpy as np
from scipy import signal as sig

RATE = 22050
SFX_RATE = 44100
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

SFX_MAX_PEAK = 0.91
SFX_MAX_DC = 0.001
SFX_CATEGORY_BUDGETS = {
    "interface": (0.8, 0.50),
    "economy": (1.2, 0.50),
    "generic-weapon": (0.6, 0.50),
    "signature-weapon": (1.5, 0.50),
    "destruction": (1.5, 0.50),
    "result": (3.0, 0.50),
}
SFX_ATTACK_AUDIBILITY_NAMES = {
    "attack_breaker",
    "demolition_boom",
    "attack_flakhound",
    "attack_flak_turret",
    "attack_bombard",
    "attack_bastion",
    "artillery_boom",
}
SFX_ATTACK_AUDIBLE_FLOOR = 0.45

# This table is shared by generation, validation, the review manifest, and the
# shell's mixer contract. The paired animation is optional: the generic viewer
# matches it when the selected directory tree happens to contain that file.
SFX_METADATA = (
    ("click", "interface", 0.25, 0.05, None),
    ("ack", "interface", 0.18, 0.15, None),
    ("denied", "interface", 0.30, 0.05, None),
    ("alert", "interface", 0.40, 1.50, None),
    ("deposit", "economy", 0.25, 0.15, "02-harvester-bite-cargo.gif"),
    ("train_done", "economy", 0.30, 0.05, "26-foundry-dark-gantry-animated.gif"),
    ("laser", "generic-weapon", 0.18, 0.09, "67-projectile-forge-spot-animated.gif"),
    ("laser2", "generic-weapon", 0.18, 0.09, "28-turret-open-ring-animated.gif"),
    (
        "attack_sentinel",
        "generic-weapon",
        0.26,
        0.08,
        "18-sentinel-low-casemate-animated.gif",
    ),
    (
        "attack_stinger",
        "generic-weapon",
        0.25,
        0.10,
        "24-stinger-inspection-trike-animated.gif",
    ),
    (
        "attack_talon",
        "generic-weapon",
        0.28,
        0.10,
        "42-talon-compact-interceptor-animated.gif",
    ),
    ("attack_wisp", "generic-weapon", 0.23, 0.10, "44-wisp-quadcopter-animated.gif"),
    (
        "attack_darter",
        "generic-weapon",
        0.25,
        0.10,
        "40-darter-shear-wing-animated.gif",
    ),
    (
        "attack_buzzard",
        "generic-weapon",
        0.35,
        0.10,
        "38-buzzard-quad-fan-carriage-animated.gif",
    ),
    (
        "attack_lancer",
        "signature-weapon",
        0.32,
        0.15,
        "36-lancer-dark-channel-animated.gif",
    ),
    (
        "attack_scuttler",
        "signature-weapon",
        0.20,
        0.09,
        "20-scuttler-centipede-shear-animated.gif",
    ),
    (
        "attack_flakhound",
        "signature-weapon",
        0.30,
        0.12,
        "04-flakhound-paired-yoke-carrier.gif",
    ),
    (
        "attack_flak_turret",
        "signature-weapon",
        0.34,
        0.12,
        "01-flak-turret-paired-yokes.gif",
    ),
    ("attack_warden", "generic-weapon", 0.30, 0.10, None),
    ("attack_breaker", "signature-weapon", 0.55, 0.15, None),
    ("avalanche_launch", "signature-weapon", 0.50, 0.15, None),
    ("bomb_release", "signature-weapon", 0.45, 0.15, None),
    ("demolition_boom", "destruction", 0.80, 0.20, None),
    ("upgrade_done", "economy", 0.55, 0.10, None),
    (
        "attack_bombard",
        "signature-weapon",
        0.50,
        0.20,
        "22-bombard-recoil-spade-animated.gif",
    ),
    (
        "attack_bastion",
        "signature-weapon",
        0.55,
        0.20,
        "32-bastion-buttressed-turntable-animated.gif",
    ),
    (
        "artillery_launch",
        "signature-weapon",
        0.40,
        0.20,
        "69-projectile-heated-nose-shell-animated.gif",
    ),
    (
        "artillery_boom",
        "signature-weapon",
        0.50,
        0.20,
        "69-projectile-heated-nose-shell-animated.gif",
    ),
    ("unit_death", "destruction", 0.35, 0.12, None),
    ("building_boom", "destruction", 0.60, 0.05, None),
    ("victory", "result", 0.60, 0.05, None),
    ("defeat", "result", 0.60, 0.05, None),
)

# Approval boundary for the 0.14 bank. These hashes are intentionally checked
# after synthesis so a library or recipe change cannot silently replace clips
# already accepted by ear.
EXPECTED_SFX_SHA256 = {
    "ack.wav": "0450d0bb65444c92d60082978685f135c910979446425aa71c75d97842a1081f",
    "alert.wav": "7acef654cf90a5c7df6bc4effd15f533c077c2d7bb5d9067bb863bec32898398",
    "artillery_boom.wav": "6ead280916761589497cd1d75bdb34a21404c14980194de2af331c28466a7de2",
    "artillery_launch.wav": "f593a4702d758c908aa973c868a8e61adb46a2b9585be3093f5fd8451f773950",
    "attack_bastion.wav": "a5366dbb160e864d72ee72e8459af0d17848457f3423c3784c574fb4800507fd",
    "attack_bombard.wav": "1848571286a2aca7eabf4c484a4a9bf138517b837098c678cffd6453675e8ea1",
    "attack_buzzard.wav": "1ca99804b3a0db9dd921f6e7eb3211368622d8d1efcae17bca1e7670ef5cfc73",
    "attack_darter.wav": "094a0e5217308c3478aa6a14dafb538e51b2bf682199a0610b38ad94d4b196a8",
    "attack_flak_turret.wav": "c714bb0819222b727ff6d635971d8df5bf0445d819537fbf740c8246b98cb3fc",
    "attack_flakhound.wav": "c7a38e3db03e0272b601273fee9e923c7d041ae34c2c9a444a3e67e7d9465fbc",
    "attack_lancer.wav": "6669626a430c184db5e4349b3aee202577e3bb5daece6f751af311330bbb2099",
    "attack_scuttler.wav": "8ec9947be252a33d3b07955d3d0b76e840f7837cc74204882b0c167c7667f686",
    "attack_sentinel.wav": "2c72d99ac0ee6cca8c9e9a54e34ad0262fc69731e3d00e378f5b0ea93e352df1",
    "attack_stinger.wav": "8d1f5e28776e234f6b4d36b705d0a9515e4d2e16049445d54aab16ad074f0a0b",
    "attack_talon.wav": "b0ef6f653c04484795e87cd880448d0035e78adc1ef19b38473fcae92580fbde",
    "attack_wisp.wav": "1eddeb755bd30db15c4c6bf9d2756a2933aee6fbeac091d0cab857a878218450",
    "building_boom.wav": "47445ba8778ef6badf80ae326f55ccf5a3e65dd4931c320978ef620ceaeeef12",
    "click.wav": "8d148979dc943755b924611a63317e8e48830d0f6e0fe2a5277b97dc05301da4",
    "defeat.wav": "ae70d17788e2a91f63ebac69d0c302ba30d101ec157bd2a30caddff1342a387b",
    "denied.wav": "e77550193b4ce7615f6aae8afe9e817f55c8df84ff58361b5703d6cd0de2714c",
    "deposit.wav": "d92bbc7bfd413943db5fe5bd52ef03fcb318ae550a7eb8291fca80ddf6e0a036",
    "attack_breaker.wav": "33f327ca63858234a34ce4ddcba6d5cf72f77c1ecddc497a77219a30771fc3e2",
    "attack_warden.wav": "9c094a4623eeca1d205db2eba26efbb54c9d559bdb56871eae79e7428739697c",
    "avalanche_launch.wav": "e56a8da38dfab471ccd248716f41498cc6954c1dbea9a1a3f0b7804df97d2afd",
    "bomb_release.wav": "6b18f6e7db76d0a51a46930f3e17c017055575e3daec2dae01372a130d489413",
    "demolition_boom.wav": "8a7d117cc678fb98bba9b2d4ba72cb26bdb47c7ff77fa100256d767e14be69cd",
    "upgrade_done.wav": "f60e6045845b53946224d33ce1cc8dbf96671903893a36fe3a0738c73ea646f0",
    "laser.wav": "252519a8fb00d4587deefb182d1025c2d6c309d63f51d176119e39f24bdf63cd",
    "laser2.wav": "b44d1b4311b750fc6005b22d1a6021db39d1dbdbddabce7dd240d19a0b5ac1b3",
    "train_done.wav": "6a5c193c7c749915cb7356aa3b2b426db50ffd3a017af2466a93dd17c1937827",
    "unit_death.wav": "b875ac45ad46385a8c5acc3f29f150496fb39071dc2c2bcb036b5d28b0538fec",
    "victory.wav": "ce13a0edb4afb7ad38fcbdf2d33d6afdf4f52ae6f9f3108fe37c8e4291e22ff2",
}
SFX_META_BY_NAME = {row[0]: row for row in SFX_METADATA}


def validate_sfx_wav(name: str, data: bytes) -> dict[str, float]:
    """Checks PCM shape, headroom, duration, DC, and speaker audibility."""
    try:
        _, category, _, _, _ = SFX_META_BY_NAME[name]
    except KeyError as error:
        raise ValueError(f"{name}: no SFX metadata") from error

    with wave.open(io.BytesIO(data), "rb") as source:
        if (
            source.getnchannels(),
            source.getsampwidth(),
            source.getframerate(),
            source.getcomptype(),
        ) != (1, 2, SFX_RATE, "NONE"):
            raise ValueError(
                f"{name}: effects must be mono 16-bit uncompressed PCM at {SFX_RATE} Hz"
            )
        frame_count = source.getnframes()
        frames = source.readframes(frame_count)

    if frame_count == 0:
        raise ValueError(f"{name}: effect is empty")
    pcm = np.frombuffer(frames, dtype="<i2")
    normalized = pcm.astype(np.float64) / 32768.0
    duration = frame_count / SFX_RATE
    peak = float(np.max(np.abs(normalized)))
    dc = abs(float(np.mean(normalized)))
    max_duration, category_floor = SFX_CATEGORY_BUDGETS[category]
    audible_floor = 0.30 if name == "artillery_launch" else category_floor

    spectrum = np.abs(np.fft.rfft(normalized)) ** 2
    frequencies = np.fft.rfftfreq(frame_count, 1.0 / SFX_RATE)
    total_energy = float(np.sum(spectrum))
    audible = (
        float(np.sum(spectrum[frequencies >= 180.0])) / total_energy
        if total_energy > 0.0
        else 0.0
    )
    attack_frames = min(frame_count, round(0.3 * SFX_RATE))
    attack = normalized[:attack_frames]
    attack_spectrum = np.abs(np.fft.rfft(attack)) ** 2
    attack_frequencies = np.fft.rfftfreq(attack_frames, 1.0 / SFX_RATE)
    attack_energy = float(np.sum(attack_spectrum))
    attack_audible = (
        float(np.sum(attack_spectrum[attack_frequencies >= 180.0])) / attack_energy
        if attack_energy > 0.0
        else 0.0
    )
    if int(np.max(np.abs(pcm.astype(np.int32)))) >= 32767 or peak > SFX_MAX_PEAK:
        raise ValueError(f"{name}: peak {peak:.6f} leaves inadequate headroom")
    if dc > SFX_MAX_DC:
        raise ValueError(f"{name}: DC offset {dc:.6f} exceeds {SFX_MAX_DC}")
    if duration > max_duration:
        raise ValueError(
            f"{name}: duration {duration:.3f}s exceeds {max_duration:.3f}s "
            f"for {category}"
        )
    if audible < audible_floor:
        raise ValueError(
            f"{name}: only {audible:.1%} of spectral energy is at or above "
            f"180 Hz; {audible_floor:.0%} required"
        )
    if (
        name in SFX_ATTACK_AUDIBILITY_NAMES
        and attack_audible < SFX_ATTACK_AUDIBLE_FLOOR
    ):
        raise ValueError(
            f"{name}: only {attack_audible:.1%} of its first 300 ms is at or "
            f"above 180 Hz; {SFX_ATTACK_AUDIBLE_FLOOR:.0%} required"
        )
    return {
        "duration": duration,
        "peak": peak,
        "dc": dc,
        "audible": audible,
        "attack_audible": attack_audible,
    }


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


# ----------------------------------------------------------------- SFX kit


def sfx_axis(n: int) -> np.ndarray:
    return np.arange(n) / SFX_RATE


def sfx_decay(n: int, sharp: float = 5.0) -> np.ndarray:
    return np.exp(-sharp * np.arange(n) / n)


def sfx_attack(samples: np.ndarray, milliseconds: float) -> np.ndarray:
    n = min(len(samples), max(1, int(SFX_RATE * milliseconds / 1000)))
    ramp = 0.5 - 0.5 * np.cos(np.linspace(0.0, np.pi, n))
    result = samples.copy()
    result[:n] *= ramp
    return result


def sfx_fade_edges(samples: np.ndarray, milliseconds: float = 4.0) -> np.ndarray:
    n = min(len(samples) // 2, max(1, int(SFX_RATE * milliseconds / 1000)))
    result = samples.copy()
    result[:n] *= np.linspace(0.0, 1.0, n)
    result[-n:] *= np.linspace(1.0, 0.0, n)
    return result


def sfx_glide(n: int, start: float, end: float) -> np.ndarray:
    return start * (end / start) ** np.linspace(0.0, 1.0, n)


def sfx_tone(frequencies: np.ndarray, phase: float = 0.0) -> np.ndarray:
    return np.sin(2.0 * np.pi * np.cumsum(frequencies) / SFX_RATE + phase)


def sfx_softsquare(frequencies: np.ndarray, hard: float = 2.5) -> np.ndarray:
    return np.tanh(hard * sfx_tone(frequencies)) / np.tanh(hard)


def sfx_saw(frequencies: np.ndarray) -> np.ndarray:
    phase = np.cumsum(frequencies) / SFX_RATE
    return sig.sawtooth(2.0 * np.pi * phase)


def sfx_bandpass(
    samples: np.ndarray, low: float, high: float, order: int = 4
) -> np.ndarray:
    high = min(high, SFX_RATE / 2 - 100)
    sos = sig.butter(order, [low, high], btype="bandpass", fs=SFX_RATE, output="sos")
    return sig.sosfilt(sos, samples)


def sfx_lowpass(samples: np.ndarray, cutoff: float, order: int = 2) -> np.ndarray:
    sos = sig.butter(
        order,
        min(cutoff, SFX_RATE / 2 - 100),
        btype="lowpass",
        fs=SFX_RATE,
        output="sos",
    )
    return sig.sosfilt(sos, samples)


def sfx_highpass(samples: np.ndarray, cutoff: float, order: int = 2) -> np.ndarray:
    sos = sig.butter(order, cutoff, btype="highpass", fs=SFX_RATE, output="sos")
    return sig.sosfilt(sos, samples)


def sfx_comb(
    samples: np.ndarray, delay: int, gain: float, damping: float = 0.25
) -> np.ndarray:
    delay = max(2, delay)
    denominator = np.zeros(delay + 2)
    denominator[0] = 1.0
    denominator[delay] = -gain * (1.0 - damping)
    denominator[delay + 1] = -gain * damping
    return sig.lfilter([1.0], denominator, samples)


def sfx_allpass(samples: np.ndarray, delay: int, gain: float = 0.5) -> np.ndarray:
    numerator = np.zeros(delay + 1)
    denominator = np.zeros(delay + 1)
    numerator[0], numerator[delay] = -gain, 1.0
    denominator[0], denominator[delay] = 1.0, -gain
    return sig.lfilter(numerator, denominator, samples)


def sfx_space(
    samples: np.ndarray,
    mix: float = 0.16,
    decay_amount: float = 0.74,
    tail: float = 0.5,
) -> np.ndarray:
    """Adds the short dark tail used only by approved interface sounds."""
    dry = np.concatenate([samples, np.zeros(int(tail * SFX_RATE))])
    wet = np.zeros_like(dry)
    for delay in (1687, 1931, 2237, 2617):
        wet += sfx_comb(dry, delay, decay_amount, damping=0.5)
    wet /= 4.0
    wet = sfx_allpass(wet, 347, 0.5)
    wet = sfx_allpass(wet, 113, 0.5)
    wet = sfx_lowpass(wet, 3800)
    predelay = int(0.011 * SFX_RATE)
    wet = np.concatenate([np.zeros(predelay), wet[:-predelay]])
    return dry + mix * wet


def sfx_drive(samples: np.ndarray, amount: float = 1.8) -> np.ndarray:
    return np.tanh(amount * samples) / np.tanh(amount)


def sfx_crush(samples: np.ndarray, bits: int = 10, hold: int = 3) -> np.ndarray:
    levels = float(2 ** (bits - 1))
    result = np.round(samples * levels) / levels
    if hold > 1:
        result = np.repeat(result[::hold], hold)[: len(result)]
    return result


def sfx_ringmod(
    samples: np.ndarray, frequency: float, depth: float = 0.5
) -> np.ndarray:
    return samples * (
        1.0 - depth + depth * np.sin(2.0 * np.pi * frequency * sfx_axis(len(samples)))
    )


def sfx_noise(n: int, seed: int) -> np.ndarray:
    return np.random.default_rng(seed).uniform(-1.0, 1.0, n)


def sfx_ping(
    frequency: float,
    duration: float,
    seed: int,
    damping: float = 0.35,
    sharp: float = 6.0,
) -> np.ndarray:
    n = int(duration * SFX_RATE)
    burst = np.zeros(n)
    burst[:48] = sfx_noise(48, seed)
    result = sfx_comb(burst, int(SFX_RATE / frequency), 0.994, damping=damping)
    result = sfx_bandpass(result, frequency * 0.5, frequency * 4.5)
    return result * sfx_decay(n, sharp)


def sfx_bell(
    n: int,
    frequency: float,
    ratio: float = 3.01,
    index: float = 1.8,
    sharp: float = 5.0,
    detune: float = 0.004,
) -> np.ndarray:
    axis = sfx_axis(n)
    result = np.zeros(n)
    for offset in (1.0 - detune, 1.0 + detune):
        modulation = (
            np.sin(2.0 * np.pi * frequency * ratio * offset * axis)
            * index
            * sfx_decay(n, sharp * 1.4)
        )
        result += np.sin(2.0 * np.pi * frequency * offset * axis + modulation)
    return result / 2.0 * sfx_decay(n, sharp)


def sfx_body(
    frequencies: np.ndarray,
    amplitudes: tuple[float, ...] = (1.0, 0.6, 0.35, 0.18),
) -> np.ndarray:
    result = np.zeros(len(frequencies))
    for harmonic, amplitude in enumerate(amplitudes, start=1):
        result += amplitude * sfx_tone(frequencies * harmonic)
    return result / sum(amplitudes)


class SfxCanvas:
    def __init__(self, duration: float):
        self.buffer = np.zeros(int(duration * SFX_RATE))

    def add(
        self, samples: np.ndarray, at: float = 0.0, gain: float = 1.0
    ) -> "SfxCanvas":
        start = int(at * SFX_RATE)
        required = start + len(samples)
        if required > len(self.buffer):
            self.buffer = np.pad(self.buffer, (0, required - len(self.buffer)))
        self.buffer[start:required] += samples * gain
        return self

    def output(self) -> np.ndarray:
        return self.buffer


def sfx_finish(
    samples: np.ndarray, highpass: float = 70.0, peak: float = 0.88
) -> np.ndarray:
    result = sfx_highpass(samples, highpass)
    result -= np.mean(result)
    result = sfx_fade_edges(result, 4.0)
    maximum = np.max(np.abs(result))
    return result * (peak / maximum) if maximum > 0 else result


def sfx_pwm_sweep(
    start: float,
    end: float,
    duration: float,
    duty_start: float = 0.5,
    duty_end: float = 0.22,
) -> np.ndarray:
    n = int(duration * SFX_RATE)
    phase = np.cumsum(sfx_glide(n, start, end)) / SFX_RATE % 1.0
    duty = np.linspace(duty_start, duty_end, n)
    return np.where(phase < duty, 1.0, -1.0)


def sfx_sync_zap(
    start: float,
    end: float,
    duration: float,
    ratio_start: float = 3.2,
    ratio_end: float = 1.3,
) -> np.ndarray:
    n = int(duration * SFX_RATE)
    fraction = np.cumsum(sfx_glide(n, start, end)) / SFX_RATE % 1.0
    ratio = np.linspace(ratio_start, ratio_end, n)
    return np.sin(2.0 * np.pi * ratio * fraction)


def sfx_zap(
    seed: int,
    start: float,
    end: float,
    duration: float,
    thump: float = 0.45,
    snap: float = 0.3,
    sync_gain: float = 0.35,
    sharp: float = 6.0,
) -> np.ndarray:
    canvas = SfxCanvas(duration + 0.06)
    n = int(duration * SFX_RATE)
    canvas.add(sfx_pwm_sweep(start, end, duration) * sfx_decay(n, sharp), 0.0, 0.75)
    canvas.add(
        sfx_sync_zap(start, end, duration) * sfx_decay(n, sharp * 1.1),
        0.0,
        sync_gain,
    )
    if snap:
        canvas.add(
            sfx_bandpass(sfx_noise(int(0.003 * SFX_RATE), seed), 2000, 7000),
            0.0,
            snap,
        )
    if thump:
        thump_frames = int(min(duration, 0.09) * SFX_RATE)
        canvas.add(
            sfx_drive(
                sfx_tone(sfx_glide(thump_frames, end * 1.6, end * 0.9))
                * sfx_decay(thump_frames, 7.0),
                2.5,
            ),
            0.002,
            thump,
        )
    return canvas.output()


def sfx_swept_lowres(
    samples: np.ndarray,
    start: float,
    end: float,
    resonance: float = 6.0,
    block: int = 64,
) -> np.ndarray:
    cutoffs = sfx_glide(len(samples), start, end)
    result = np.zeros_like(samples)
    state = np.zeros((1, 2))
    for begin in range(0, len(samples), block):
        finish = min(begin + block, len(samples))
        angle = 2.0 * np.pi * min(cutoffs[begin], SFX_RATE / 2 - 200) / SFX_RATE
        alpha = np.sin(angle) / (2.0 * resonance)
        cosine = np.cos(angle)
        denominator = 1.0 + alpha
        sos = np.array(
            [
                [
                    (1 - cosine) / 2 / denominator,
                    (1 - cosine) / denominator,
                    (1 - cosine) / 2 / denominator,
                    1.0,
                    -2 * cosine / denominator,
                    (1 - alpha) / denominator,
                ]
            ]
        )
        result[begin:finish], state = sig.sosfilt(sos, samples[begin:finish], zi=state)
    return result / (np.max(np.abs(result)) + 1e-9)


def sfx_pound(
    seed: int,
    start: float,
    end: float,
    duration: float,
    bloom: float = 0.0,
    knock: float = 0.55,
    drive_amount: float = 3.0,
    sharp: float = 5.0,
) -> np.ndarray:
    n = int(duration * SFX_RATE)
    canvas = SfxCanvas(duration + 0.05)
    core = (
        0.55 * sfx_tone(sfx_glide(n, start, end))
        + sfx_tone(sfx_glide(n, 2 * start, 2 * end))
        + 0.5 * sfx_tone(sfx_glide(n, 3 * start, 3 * end))
    ) / 2.05
    canvas.add(sfx_drive(core * sfx_decay(n, sharp), drive_amount))
    if knock:
        knock_frames = int(min(duration, 0.1) * SFX_RATE)
        canvas.add(
            sfx_drive(
                sfx_tone(sfx_glide(knock_frames, 340.0, 210.0))
                * sfx_decay(knock_frames, 7.5),
                2.5,
            ),
            0.001,
            knock,
        )
    if bloom:
        canvas.add(
            sfx_swept_lowres(sfx_noise(n, seed + 1), 1600.0, 200.0, resonance=8.0)
            * sfx_decay(n, sharp),
            0.004,
            bloom,
        )
    return canvas.output()


def sfx_crack(seed: int, noise_gain: float = 0.8) -> np.ndarray:
    zap_frames = int(0.016 * SFX_RATE)
    zap = sfx_tone(sfx_glide(zap_frames, 3200.0, 380.0)) * sfx_decay(zap_frames, 9.0)
    noise_frames = int(0.01 * SFX_RATE)
    noise_layer = sfx_bandpass(sfx_noise(noise_frames, seed), 900, 8000) * sfx_decay(
        noise_frames, 6.0
    )
    canvas = SfxCanvas(0.03).add(sfx_drive(zap, 2.5)).add(noise_layer, 0.0, noise_gain)
    return sfx_drive(canvas.output(), 2.0)


def sfx_slapback(
    samples: np.ndarray, gap: float = 0.1, times: int = 2, gain: float = 0.3
) -> np.ndarray:
    result = np.zeros(len(samples) + int(gap * times * SFX_RATE) + int(0.05 * SFX_RATE))
    result[: len(samples)] += samples
    echo = samples
    for repeat in range(1, times + 1):
        echo = sfx_lowpass(echo, 2400.0) * gain
        start = int(repeat * gap * SFX_RATE)
        result[start : start + len(echo)] += echo
    return result


# -------------------------------------------------------- approved SFX bank


def sfx_click() -> np.ndarray:
    n = int(0.035 * SFX_RATE)
    tick = sfx_bandpass(sfx_noise(n, 101), 2800, 7000) * sfx_decay(n, 22.0)
    tap = sfx_tone(np.full(n, 1150.0)) * sfx_decay(n, 18.0)
    return sfx_finish(0.4 * tick + 0.8 * tap, highpass=200, peak=0.7)


def sfx_ack() -> np.ndarray:
    canvas = SfxCanvas(0.16)
    first = int(0.05 * SFX_RATE)
    second = int(0.06 * SFX_RATE)
    canvas.add(
        sfx_softsquare(np.full(first, 622.0), 2.2) * sfx_decay(first, 8.0),
        0.0,
        0.55,
    )
    canvas.add(
        sfx_softsquare(np.full(second, 932.0), 2.2) * sfx_decay(second, 7.0),
        0.055,
        0.5,
    )
    canvas.add(
        sfx_tone(np.full(second, 2489.0)) * sfx_decay(second, 12.0),
        0.055,
        0.08,
    )
    return sfx_finish(
        sfx_space(sfx_crush(canvas.output(), 11, 2), 0.08, tail=0.12),
        highpass=180,
        peak=0.72,
    )


def sfx_denied() -> np.ndarray:
    n = int(0.2 * SFX_RATE)
    frequencies = sfx_glide(n, 233.0, 218.0)
    buzz = (sfx_saw(frequencies * 0.997) + sfx_saw(frequencies * 1.003)) / 2.0
    buzz = sfx_lowpass(buzz, 1300) * sfx_decay(n, 4.5)
    knock_frames = int(0.1 * SFX_RATE)
    knock = sfx_body(sfx_glide(knock_frames, 150.0, 132.0), (1.0, 0.4)) * sfx_decay(
        knock_frames, 8.0
    )
    canvas = SfxCanvas(0.24).add(buzz, 0.0, 0.8).add(knock, 0.1, 0.55)
    return sfx_finish(
        sfx_space(sfx_crush(canvas.output(), 10, 3), 0.1, tail=0.15),
        highpass=90,
        peak=0.78,
    )


def sfx_alert() -> np.ndarray:
    canvas = SfxCanvas(0.34)
    first = int(0.07 * SFX_RATE)
    second = int(0.09 * SFX_RATE)
    canvas.add(
        sfx_softsquare(np.full(first, 587.33), 2.2) * sfx_decay(first, 7.0),
        0.0,
        0.6,
    )
    canvas.add(
        sfx_softsquare(np.full(second, 415.3), 2.2) * sfx_decay(second, 6.0),
        0.085,
        0.62,
    )
    knock_frames = int(0.09 * SFX_RATE)
    canvas.add(
        sfx_body(sfx_glide(knock_frames, 160.0, 138.0), (1.0, 0.4))
        * sfx_decay(knock_frames, 7.0),
        0.085,
        0.5,
    )
    return sfx_finish(
        sfx_space(sfx_crush(canvas.output(), 11, 2), 0.1, tail=0.15),
        highpass=90,
        peak=0.8,
    )


def sfx_deposit() -> np.ndarray:
    canvas = SfxCanvas(0.42)
    thump_frames = int(0.09 * SFX_RATE)
    canvas.add(
        sfx_body(sfx_glide(thump_frames, 150.0, 126.0), (1.0, 0.5, 0.25))
        * sfx_decay(thump_frames, 7.0),
        0.0,
        0.6,
    )
    chunk_frames = int(0.05 * SFX_RATE)
    chunk = sfx_comb(
        sfx_bandpass(sfx_noise(chunk_frames, 202), 400, 2600),
        int(SFX_RATE / 560),
        0.98,
        damping=0.4,
    )
    canvas.add(chunk * sfx_decay(chunk_frames, 9.0), 0.004, 0.5)
    rng = np.random.default_rng(203)
    for index, frequency in enumerate((1420.0, 1140.0, 930.0, 745.0)):
        frequency *= 1.0 + rng.uniform(-0.02, 0.02)
        canvas.add(
            sfx_ping(frequency, 0.09, 204 + index, sharp=8.0),
            0.085 + 0.068 * index,
            0.42 - 0.07 * index,
        )
    return sfx_finish(
        sfx_space(sfx_crush(canvas.output(), 11, 2), 0.12, tail=0.2),
        highpass=90,
        peak=0.8,
    )


def sfx_train_done() -> np.ndarray:
    canvas = SfxCanvas(0.7)
    vent_frames = int(0.13 * SFX_RATE)
    vent = sfx_bandpass(sfx_noise(vent_frames, 301), 900, 3200) * sfx_decay(
        vent_frames, 6.0
    )
    canvas.add(sfx_attack(vent, 6.0), 0.0, 0.28)
    bell_frames = int(0.6 * SFX_RATE)
    canvas.add(
        sfx_bell(bell_frames, 523.25, ratio=3.01, index=1.6, sharp=5.0),
        0.05,
        0.7,
    )
    canvas.add(
        sfx_bell(bell_frames, 349.23, ratio=2.0, index=0.9, sharp=4.5),
        0.05,
        0.4,
    )
    return sfx_finish(
        sfx_space(sfx_crush(canvas.output(), 11, 2), 0.18, tail=0.35),
        highpass=110,
        peak=0.8,
    )


def sfx_victory() -> np.ndarray:
    canvas = SfxCanvas(2.0)
    drone_frames = int(1.9 * SFX_RATE)
    drone = sfx_body(sfx_glide(drone_frames, 73.42, 73.42), (0.4, 0.5, 0.3, 0.15))
    drone *= np.minimum(np.linspace(0.0, 2.2, drone_frames), 1.0) * sfx_decay(
        drone_frames, 2.4
    )
    canvas.add(drone, 0.0, 0.5)
    for index, frequency in enumerate((293.66, 440.0, 587.33)):
        bell_frames = int((1.9 - 0.28 * index) * SFX_RATE)
        canvas.add(
            sfx_bell(
                bell_frames,
                frequency,
                ratio=2.76,
                index=1.5,
                sharp=3.6,
                detune=0.003,
            ),
            0.28 * index,
            0.55,
        )
    shimmer_frames = int(1.1 * SFX_RATE)
    shimmer = (
        sfx_tone(np.full(shimmer_frames, 1174.66 * 0.996))
        + sfx_tone(np.full(shimmer_frames, 1174.66 * 1.004))
    ) / 2.0
    canvas.add(shimmer * sfx_decay(shimmer_frames, 3.0), 0.84, 0.1)
    return sfx_finish(
        sfx_space(canvas.output(), 0.3, decay_amount=0.8, tail=0.7),
        highpass=60,
        peak=0.85,
    )


def sfx_defeat() -> np.ndarray:
    canvas = SfxCanvas(2.1)
    drone_frames = int(2.0 * SFX_RATE)
    drone = sfx_body(sfx_glide(drone_frames, 98.0, 58.0), (0.6, 0.5, 0.3, 0.15))
    tremolo = 1.0 - np.linspace(0.0, 0.45, drone_frames) * (
        0.5 + 0.5 * np.sin(2 * np.pi * 6.5 * sfx_axis(drone_frames))
    )
    drone *= tremolo * sfx_decay(drone_frames, 2.2)
    canvas.add(sfx_attack(drone, 30.0), 0.0, 0.55)
    for index, frequency in enumerate((587.33, 415.3, 293.66)):
        bell_frames = int((1.8 - 0.3 * index) * SFX_RATE)
        bell = sfx_bell(
            bell_frames,
            frequency,
            ratio=3.53,
            index=1.9,
            sharp=3.4,
            detune=0.008,
        )
        if index == 2:
            bell = sfx_ringmod(bell, 4.0, 0.3)
        canvas.add(bell, 0.35 * index, 0.5)
    whir_frames = int(0.9 * SFX_RATE)
    whir = sfx_lowpass(sfx_saw(sfx_glide(whir_frames, 700.0, 140.0)), 1600) * sfx_decay(
        whir_frames, 4.0
    )
    canvas.add(sfx_crush(whir, 9, 3), 1.1, 0.16)
    return sfx_finish(
        sfx_space(canvas.output(), 0.3, decay_amount=0.82, tail=0.7),
        highpass=55,
        peak=0.85,
    )


def sfx_laser() -> np.ndarray:
    return sfx_finish(
        sfx_crush(sfx_zap(401, 1450.0, 320.0, 0.09), 10, 2),
        highpass=130,
        peak=0.82,
    )


def sfx_laser2() -> np.ndarray:
    return sfx_finish(
        sfx_crush(sfx_zap(403, 1150.0, 260.0, 0.1), 10, 2),
        highpass=120,
        peak=0.82,
    )


def sfx_sentinel() -> np.ndarray:
    return sfx_finish(
        sfx_crush(
            sfx_zap(
                501,
                900.0,
                210.0,
                0.13,
                thump=0.6,
                sync_gain=0.4,
                sharp=5.5,
            ),
            10,
            2,
        ),
        highpass=100,
        peak=0.85,
    )


def sfx_stinger() -> np.ndarray:
    return sfx_finish(
        sfx_crush(sfx_zap(621, 1600.0, 520.0, 0.07, thump=0.3, snap=0.25), 10, 2),
        highpass=180,
        peak=0.82,
    )


def sfx_talon() -> np.ndarray:
    return sfx_finish(
        sfx_crush(sfx_zap(631, 1250.0, 420.0, 0.08, thump=0.4), 10, 2),
        highpass=150,
        peak=0.84,
    )


def sfx_wisp() -> np.ndarray:
    return sfx_finish(
        sfx_crush(sfx_zap(641, 1900.0, 700.0, 0.055, thump=0.2, snap=0.2), 10, 2),
        highpass=250,
        peak=0.8,
    )


def sfx_darter() -> np.ndarray:
    return sfx_finish(
        sfx_crush(sfx_zap(711, 1800.0, 430.0, 0.065, thump=0.4), 10, 2),
        highpass=140,
        peak=0.83,
    )


def sfx_buzzard() -> np.ndarray:
    result = sfx_crush(
        sfx_zap(
            701,
            750.0,
            140.0,
            0.22,
            thump=0.8,
            snap=0.4,
            sync_gain=0.45,
            sharp=4.8,
        ),
        10,
        2,
    )
    return sfx_finish(sfx_slapback(result, 0.09, 1, 0.25), highpass=70, peak=0.88)


def sfx_lancer() -> np.ndarray:
    result = sfx_crush(
        sfx_zap(
            504,
            1650.0,
            230.0,
            0.26,
            thump=0.75,
            snap=0.5,
            sync_gain=0.45,
            sharp=5.0,
        ),
        10,
        2,
    )
    return sfx_finish(sfx_slapback(result, 0.08, 2, 0.28), highpass=85, peak=0.87)


def sfx_scuttler() -> np.ndarray:
    canvas = SfxCanvas(0.32)
    for index, at in enumerate((0.0, 0.115)):
        stroke_frames = int(0.075 * SFX_RATE)
        stroke = sfx_noise(stroke_frames, 951 + index)
        stroke *= sfx_tone(sfx_glide(stroke_frames, 2400.0 - 300.0 * index, 950.0))
        stroke = sfx_bandpass(
            sfx_comb(stroke, int(SFX_RATE / 1650), 0.955, damping=0.3),
            700,
            6500,
        )
        canvas.add(
            sfx_attack(stroke * sfx_decay(stroke_frames, 6.5), 3.0),
            at,
            0.85 - 0.1 * index,
        )
        thump_frames = int(0.05 * SFX_RATE)
        canvas.add(
            sfx_drive(
                sfx_tone(sfx_glide(thump_frames, 320.0, 210.0))
                * sfx_decay(thump_frames, 8.0),
                2.2,
            ),
            at + 0.004,
            0.3,
        )
    canvas.add(sfx_ping(2650.0, 0.05, 955, sharp=10.0), 0.23, 0.28)
    return sfx_finish(sfx_crush(canvas.output(), 9, 2), highpass=200, peak=0.83)


def sfx_flakhound() -> np.ndarray:
    canvas = SfxCanvas(0.42)
    for index, at in enumerate((0.0, 0.115)):
        canvas.add(sfx_crack(601 + 3 * index), at, 0.85 - 0.1 * index)
        canvas.add(
            sfx_pound(
                602 + 3 * index,
                165.0 * (1.0 + 0.06 * index),
                62.0,
                0.16,
                bloom=0.5,
            ),
            at + 0.003,
            1.0 - 0.15 * index,
        )
    return sfx_finish(sfx_crush(canvas.output(), 10, 2), highpass=70, peak=0.87)


def sfx_flak_turret() -> np.ndarray:
    canvas = SfxCanvas(0.5)
    for index, at in enumerate((0.0, 0.135)):
        canvas.add(sfx_crack(611 + 3 * index), at, 0.9 - 0.1 * index)
        canvas.add(
            sfx_pound(
                612 + 3 * index,
                150.0 * (1.0 + 0.06 * index),
                56.0,
                0.18,
                bloom=0.55,
            ),
            at + 0.003,
            1.0 - 0.12 * index,
        )
    result = sfx_slapback(sfx_crush(canvas.output(), 10, 2), 0.11, 1, 0.22)
    return sfx_finish(result, highpass=65, peak=0.88)


def sfx_bombard() -> np.ndarray:
    canvas = SfxCanvas(0.7)
    canvas.add(sfx_crack(810), 0.0, 1.0)
    canvas.add(
        sfx_pound(811, 120.0, 44.0, 0.42, bloom=0.75, sharp=4.6),
        0.004,
        1.0,
    )
    result = sfx_crush(sfx_drive(canvas.output(), 2.0), 10, 2)
    result = sfx_slapback(result, 0.11, 2, 0.24)
    return sfx_finish(result, highpass=55, peak=0.9)


def sfx_bastion() -> np.ndarray:
    canvas = SfxCanvas(0.95)
    canvas.add(sfx_crack(820), 0.0, 1.0)
    canvas.add(
        sfx_pound(821, 105.0, 40.0, 0.55, bloom=0.85, sharp=4.2),
        0.005,
        1.0,
    )
    result = sfx_crush(sfx_drive(canvas.output(), 2.1), 10, 2)
    result = sfx_slapback(result, 0.13, 2, 0.26)
    return sfx_finish(result, highpass=50, peak=0.9)


def sfx_artillery_launch() -> np.ndarray:
    canvas = SfxCanvas(0.55)
    report = sfx_pound(801, 95.0, 48.0, 0.4, bloom=0.4, sharp=4.5)
    canvas.add(sfx_attack(report, 20.0), 0.0, 0.95)
    return sfx_finish(sfx_crush(canvas.output(), 10, 2), highpass=60, peak=0.86)


def sfx_artillery_boom() -> np.ndarray:
    canvas = SfxCanvas(1.0)
    canvas.add(sfx_crack(830), 0.0, 1.1)
    canvas.add(
        sfx_pound(
            831,
            150.0,
            46.0,
            0.5,
            bloom=0.9,
            drive_amount=3.0,
            sharp=4.4,
        ),
        0.004,
        1.0,
    )
    result = sfx_crush(sfx_drive(canvas.output(), 2.2), 10, 2)
    result = sfx_slapback(result, 0.14, 2, 0.26)
    return sfx_finish(result, highpass=50, peak=0.9)


def sfx_unit_death() -> np.ndarray:
    n = int(0.4 * SFX_RATE)
    frequencies = sfx_glide(n, 520.0, 138.0)
    vibrato = 1.0 + np.linspace(0.0, 0.055, n) * np.sin(2.0 * np.pi * 9.0 * sfx_axis(n))
    down = (
        sfx_softsquare(frequencies * vibrato * 0.9965, 2.2)
        + sfx_softsquare(frequencies * vibrato * 1.0035, 2.2)
    ) / 2.0
    down = sfx_crush(down * sfx_decay(n, 3.6), 9, 4)
    canvas = SfxCanvas(0.5).add(sfx_attack(down, 8.0), 0.0, 0.85)
    canvas.add(sfx_ping(620.0, 0.07, 901, sharp=9.0), 0.33, 0.22)
    canvas.add(sfx_ping(415.0, 0.08, 902, sharp=9.0), 0.4, 0.18)
    return sfx_finish(
        sfx_space(canvas.output(), 0.12, tail=0.18), highpass=95, peak=0.82
    )


def sfx_building_boom() -> np.ndarray:
    n = int(0.85 * SFX_RATE)
    frequencies = sfx_glide(n, 440.0, 72.0)
    vibrato = 1.0 + np.linspace(0.0, 0.07, n) * np.sin(2.0 * np.pi * 7.0 * sfx_axis(n))
    down = (
        sfx_softsquare(frequencies * vibrato * 0.996, 2.2)
        + sfx_softsquare(frequencies * vibrato * 1.004, 2.2)
    ) / 2.0
    down = sfx_crush(down * sfx_decay(n, 3.2), 9, 4)
    canvas = SfxCanvas(1.0).add(sfx_attack(down, 10.0), 0.0, 0.9)
    canvas.add(sfx_ping(300.0, 0.1, 921, sharp=8.0), 0.68, 0.24)
    canvas.add(sfx_ping(205.0, 0.12, 922, sharp=8.0), 0.8, 0.2)
    return sfx_finish(
        sfx_space(canvas.output(), 0.12, tail=0.25), highpass=60, peak=0.88
    )


def sfx_warden() -> np.ndarray:
    # The Warden's fork cannon: the sentinel zap grown up — lower,
    # longer, with more chest and the same dry report.
    return sfx_finish(
        sfx_crush(
            sfx_zap(731, 640.0, 150.0, 0.17, thump=0.8, sync_gain=0.45, sharp=4.8),
            10,
            2,
        ),
        highpass=85,
        peak=0.87,
    )


def sfx_breaker() -> np.ndarray:
    # The siege mortar: one crack into the deepest pound in the roster,
    # still leading with energy above 180 Hz for laptop speakers.
    canvas = SfxCanvas(0.85)
    canvas.add(sfx_crack(741, noise_gain=1.1), 0.0, 1.0)
    canvas.add(sfx_pound(742, 118.0, 42.0, 0.5, bloom=0.85, sharp=4.2), 0.006, 0.9)
    result = sfx_crush(sfx_drive(canvas.output(), 2.1), 10, 2)
    result = sfx_slapback(result, 0.14, 2, 0.26)
    return sfx_finish(result, highpass=52, peak=0.9)


def sfx_avalanche_launch() -> np.ndarray:
    # The rocket bank leaving its tubes: a dark fused-noise sweep rising
    # out of a soft ignition, no impact — the shells land elsewhere.
    n = int(0.55 * SFX_RATE)
    rush = sfx_bandpass(sfx_noise(n, 751), 380, 3200)
    ramp = np.linspace(0.3, 1.0, n) * sfx_decay(n, 1.6)
    body = sfx_softsquare(sfx_glide(n, 180.0, 460.0), 2.0) * sfx_decay(n, 2.4)
    canvas = SfxCanvas(0.7)
    canvas.add(sfx_attack(rush * ramp, 8.0), 0.0, 0.9)
    canvas.add(body, 0.02, 0.3)
    return sfx_finish(
        sfx_space(sfx_crush(canvas.output(), 10, 3), 0.10, tail=0.2),
        highpass=90,
        peak=0.85,
    )


def sfx_bomb_release() -> np.ndarray:
    # The bay opens and the load drops away: one falling glide with a
    # mechanical unlatch, no boom — the landing shell owns the boom.
    canvas = SfxCanvas(0.5)
    latch_frames = int(0.06 * SFX_RATE)
    latch = sfx_bandpass(sfx_noise(latch_frames, 761), 1200, 4200) * sfx_decay(
        latch_frames, 7.0
    )
    canvas.add(sfx_attack(latch, 3.0), 0.0, 0.5)
    n = int(0.4 * SFX_RATE)
    fall = sfx_softsquare(sfx_glide(n, 520.0, 190.0), 2.4) * sfx_decay(n, 2.0)
    canvas.add(fall, 0.04, 0.6)
    return sfx_finish(
        sfx_crush(canvas.output(), 10, 3),
        highpass=120,
        peak=0.8,
    )


def sfx_demolition_boom() -> np.ndarray:
    # One buried charge, one gesture: a hard crack fused straight into a
    # floor-shaking pound with missing-fundamental voicing. Serves the
    # Scuttle Charge and the Sapper alike.
    canvas = SfxCanvas(0.9)
    canvas.add(sfx_crack(771, noise_gain=1.2), 0.0, 1.0)
    canvas.add(sfx_pound(772, 112.0, 38.0, 0.55, bloom=0.95, sharp=3.8), 0.004, 0.9)
    result = sfx_crush(sfx_drive(canvas.output(), 2.2), 9, 3)
    result = sfx_slapback(result, 0.15, 2, 0.28)
    return sfx_finish(result, highpass=52, peak=0.9)


def sfx_upgrade_done() -> np.ndarray:
    # The works comes back online a rung higher: the train_done vent
    # into a two-step RISING figure (D up to A — reserved rising motion,
    # kin to victory's, never the falling tritone).
    canvas = SfxCanvas(0.8)
    vent_frames = int(0.11 * SFX_RATE)
    vent = sfx_bandpass(sfx_noise(vent_frames, 781), 700, 2800) * sfx_decay(
        vent_frames, 6.0
    )
    canvas.add(sfx_attack(vent, 6.0), 0.0, 0.3)
    bell_frames = int(0.5 * SFX_RATE)
    canvas.add(sfx_bell(bell_frames, 293.66, ratio=2.0, index=1.1, sharp=4.5), 0.04, 0.55)
    canvas.add(sfx_bell(bell_frames, 440.0, ratio=3.01, index=1.5, sharp=5.0), 0.2, 0.7)
    return sfx_finish(
        sfx_space(sfx_crush(canvas.output(), 11, 2), 0.16, tail=0.3),
        highpass=105,
        peak=0.8,
    )


SFX_BUILDERS = {
    "click": sfx_click,
    "ack": sfx_ack,
    "denied": sfx_denied,
    "alert": sfx_alert,
    "deposit": sfx_deposit,
    "train_done": sfx_train_done,
    "laser": sfx_laser,
    "laser2": sfx_laser2,
    "attack_sentinel": sfx_sentinel,
    "attack_stinger": sfx_stinger,
    "attack_talon": sfx_talon,
    "attack_wisp": sfx_wisp,
    "attack_darter": sfx_darter,
    "attack_buzzard": sfx_buzzard,
    "attack_lancer": sfx_lancer,
    "attack_scuttler": sfx_scuttler,
    "attack_flakhound": sfx_flakhound,
    "attack_flak_turret": sfx_flak_turret,
    "attack_warden": sfx_warden,
    "attack_breaker": sfx_breaker,
    "avalanche_launch": sfx_avalanche_launch,
    "bomb_release": sfx_bomb_release,
    "demolition_boom": sfx_demolition_boom,
    "upgrade_done": sfx_upgrade_done,
    "attack_bombard": sfx_bombard,
    "attack_bastion": sfx_bastion,
    "artillery_launch": sfx_artillery_launch,
    "artillery_boom": sfx_artillery_boom,
    "unit_death": sfx_unit_death,
    "building_boom": sfx_building_boom,
    "victory": sfx_victory,
    "defeat": sfx_defeat,
}


def sfx_wav(samples: np.ndarray) -> bytes:
    data = io.BytesIO()
    with wave.open(data, "wb") as output:
        output.setnchannels(1)
        output.setsampwidth(2)
        output.setframerate(SFX_RATE)
        pcm = np.clip(samples, -1.0, 1.0)
        output.writeframes((pcm * 32767.0).astype("<i2").tobytes())
    return data.getvalue()


def generate_sfx_bank() -> None:
    if set(SFX_BUILDERS) != set(SFX_META_BY_NAME):
        raise ValueError("SFX builders and metadata differ")
    for name, _, _, _, _ in SFX_METADATA:
        data = sfx_wav(SFX_BUILDERS[name]())
        metrics = validate_sfx_wav(name, data)
        digest = hashlib.sha256(data).hexdigest()
        expected = EXPECTED_SFX_SHA256[f"{name}.wav"]
        if digest != expected:
            raise ValueError(
                f"{name}: approved bytes changed ({digest}, expected {expected})"
            )
        GENERATED[f"{name}.wav"] = data
        print(
            f"  {name}.wav ({metrics['duration']:.2f}s, "
            f"peak {metrics['peak']:.2f}, >=180Hz {metrics['audible']:.0%})"
        )


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


def manifest_bytes() -> bytes:
    sounds = [
        {
            "name": name,
            "filename": f"{name}.wav",
            "category": category,
            "mixer_volume": volume,
            "min_gap": min_gap,
            "paired_animation": animation,
        }
        for name, category, volume, min_gap, animation in SFX_METADATA
    ]
    document = {
        "format": 1,
        "sample_rate": SFX_RATE,
        "sounds": sounds,
    }
    return (json.dumps(document, indent=2, sort_keys=True) + "\n").encode()


def emit(output: Path, check: bool) -> None:
    expected = set(GENERATED)
    actual = {path.name for path in output.glob("*.wav")}
    manifest = manifest_bytes()
    if check:
        if actual != expected:
            missing = sorted(expected - actual)
            extra = sorted(actual - expected)
            raise SystemExit(f"sound bank differs: missing={missing}, extra={extra}")
        changed = [
            name
            for name, data in GENERATED.items()
            if (output / name).read_bytes() != data
        ]
        if changed:
            raise SystemExit(f"sound bank is stale: {', '.join(sorted(changed))}")
        manifest_path = output / "manifest.json"
        if not manifest_path.exists() or manifest_path.read_bytes() != manifest:
            raise SystemExit("sound manifest is stale")
        print(f"checked {len(GENERATED)} deterministic WAVs and manifest")
        return
    output.mkdir(parents=True, exist_ok=True)
    for name, data in GENERATED.items():
        (output / name).write_bytes(data)
    (output / "manifest.json").write_bytes(manifest)
    print(f"wrote {len(GENERATED)} deterministic WAVs and manifest")


def generate(output: Path, check: bool = False) -> None:
    """Generates or checks the complete sound bank at `output`."""
    GENERATED.clear()
    print(f"{'checking' if check else 'writing'} {output}")
    generate_sfx_bank()
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
    emit(output, check)
    print("done")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    destination = parser.add_mutually_exclusive_group()
    destination.add_argument(
        "--check",
        action="store_true",
        help="verify the checked-in WAV bank without rewriting it",
    )
    destination.add_argument(
        "--out",
        type=Path,
        metavar="DIRECTORY",
        help="write the complete sound bank to DIRECTORY instead of assets/sounds",
    )
    args = parser.parse_args()
    generate(args.out if args.out is not None else OUT, check=args.check)


if __name__ == "__main__":
    main()
