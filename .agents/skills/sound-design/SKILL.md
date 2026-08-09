---
name: sound-design
description: Generate, modify, audition, and productionize explicitly approved Oxide sound effects. Use for weapon reports, alerts, UI cues, destruction, mixer tuning, camera-aware sound, or changes to tools/gen_sounds.py and assets/sounds. This skill preserves approved audio byte-for-byte and keeps experiments out of production commits.
---

# Oxide sound design

Use this workflow for any Oxide audio work. Sound direction is a product
decision made by ear; synthesis metrics and visual waveforms are gates, not
substitutes for listening.

## Start from the production contract

1. Read `docs/audio-approvals.md`, `tools/gen_sounds.py`, and the relevant
   mixer/event mappings before generating anything.
2. Treat every approved clip as frozen. Carry it into comparison banks as a
   control and do not change its recipe or output bytes unless Connor explicitly
   reopens it.
3. Keep audition banks, rejected attempts, batch scripts, and review pages
   untracked. Never delete, ignore, move, or commit them as part of promotion.
4. Generate only the requested event family. Do not revive the old goal of one
   continuous loop or unique weapon voice per sprite.

## Sound language

- Synth first, never realistic foley. Use pitch-swept basic waves, moving PWM,
  hard sync, ring modulation, and fused noise.
- Keep weapon reports dry. A short dark slapback is the most space a combat
  sound should carry.
- Use light 10- or 11-bit crush and sample-hold as glue, never harsh static on
  the transient.
- Center the musical language around D. Reserve a falling tritone for denied,
  defeat, and alert; victory rises D-A-D.
- Make depth audible on laptop speakers with missing-fundamental voicing. Deep
  events still need meaningful energy above 180 Hz, especially in the first
  300 ms.
- Prefer one decisive gesture. Multi-report audio is valid only when the
  finalized animation visibly fires more than once.

The combat roster is deliberately small:

- Most weapons use one generic zap family with slight pitch, length, and weight
  variation.
- Lancer gets one large, obvious laser discharge with no charge-up sound.
- Scuttler gets a mechanical two-stroke shear and snip.
- Flakhound and Flak Turret get the paired crack-and-thump report matching their
  offset barrels.
- Bombard and Bastion get one crack followed by a heavy synthetic thud.
- Destruction is a synthetic power-down, not a realistic building collapse.

Avoid realistic layered booms, clean arcade chirps, woodblock-like noise hits,
static, whistles, debris garnish, structural groans, and extra shots that the
animation does not show. When a design feels busy, simplify it before adding
another layer.

## Audition and approval

1. Record the control WAV's SHA-256 and include that exact byte sequence in the
   comparison. If a direction is close, make bounded variations beside it.
2. Generate open candidates with an untracked, narrowly named batch generator.
   It must emit numbered WAVs and a viewer-compatible `manifest.json` into an
   untracked audition directory. Do not put unapproved recipes or hashes into
   `tools/gen_sounds.py`.
3. Copy the paired finalized GIF into the selected audition tree when animation
   timing matters; production generation intentionally does not depend on the
   untracked visual-review bank.
4. Open `tools/audio_review.html` and select the audition directory. Exercise
   normal mixer level, rapid retrigger, and the paired animation. Present the
   numbered options with plain descriptions, then stop for Connor's by-ear
   approval. Do not infer a winner from spectra or diagnostics.
5. Record only an explicit approval in `docs/audio-approvals.md`. Then port that
   exact recipe into the production generator and assert that its generated WAV
   is byte-identical to the approved audition file.

## Productionize an approved clip

- Fold the approved recipe into the single source of truth,
  `tools/gen_sounds.py`. Do not leave a second production generator.
- Use explicit fixed seeds. Pin synthesis dependencies exactly in the PEP 723
  header.
- Keep the generator, tests, approval ledger, and generated production WAVs in
  one reviewable change. Commit them together only when the user has explicitly
  authorized a commit. Do not stage or commit the source audition bank.
- SFX are mono 16-bit PCM at 44,100 Hz. The temporary generated music beds stay
  at 22,050 Hz until the licensed soundtrack replaces them.
- Keep the sound-name bijection exact across generator output, loaded assets,
  event mapping, and mixer configuration.
- Preserve the alert as a protected UI-level signal: camera distance, zoom
  detail suppression, and mass-event coalescing must not hide an attack alert.
- Camera mixing should emphasize nearby detail when zoomed in and coalesce or
  reduce minor reports when zoomed out. Heavy threats remain legible.
- Inspect the dirty tree before staging, stage exact production paths rather
  than `git add -A`, and verify the staged generator contains no review routes.

## Validate before handoff

Run the generator's focused tests and reproducibility check, then Rust tests for
asset loading, event mapping, and mixer behavior. The mechanical gates must
cover format, duration, peak, DC offset, spectral audibility, deterministic
bytes, complete asset mapping, retrigger gaps, zoom weighting, coalescing, and
the protected alert. Finish with the repository-wide tests, Clippy, and format
check required by `AGENTS.md`.
