# Neural artifact lineage

Read this file only when migrating, continuing, comparing, or replacing the
shipped policy. Treat repository code and artifact digests as canonical if this
snapshot ever disagrees with the live tree.

## Current state: the promoted 0.15 actor

The shipped artifact (`sim/src/bot/ladder_weights.json`) is the r17
candidate of the from-scratch gym-v9 campaign, promoted 2026-08-10 on the
complete native-Q12 battery.

- Contract: gym v9 — 107 named features, 12 named conditions, 43 actions
  across four policy heads. Parity-clean by construction: the mask encodes
  shared legality only.
- Gameplay digest: `320706eb6eb5882e`.
- Content-addressed training lineage:
  `sha256:12f69dd13ac584f563c0be16059e9bdf518bd92c12edf3c9613103a21acd05cd`
  (phase `style-distillation`).
- Provenance chain: BC prior on the four v9-surface teachers (77,404
  samples) -> PPO league phases r1-r10 (economy/tree consolidation, Array
  rebalance, faction-deal rush hardening; peak checkpoint
  r10-consolidation ckpt-01975) -> r11 production-entropy diversity polish
  (endpoint ckpt-02035) -> r13 lock-in (ckpt-02075, the trunk) -> r14
  profile-columns-only style-bonus phase (ckpt-02155) -> r17
  named-condition teacher distillation into the five profile columns
  (construction head cloned from the fortify teacher only). The trunk and
  raw-aggression path are byte-identical to r13-02075, proven by the
  battery's parent-match gate.
- Promotion battery (all raw reports under `tools/train/runs/`,
  experiments under `experiments/2026-08-10-*.md`): neural-cup 90% vs the
  Overseer over 120 games (54F/54C), faction pairs 90/80/90/80 for
  ff/cc/fc/cf; rush canary 51% (known residual: the trunk carries
  profile-specific rush softness — fortress-family personalities lose to
  the expert all-in while 7/9 personalities hold; shared by every
  candidate in the family); complete fun gate (rhythm, growth, reach,
  spam floors, all under expert-execution probes); profile behavior gates
  (diversity, team-role liveness, style semantics 7/7 on all four
  signatures); deterministic full-match replay; repair probe 8/8; Level
  ladder ordered 15/28/36/40 wins on the freshly recalibrated rungs
  (Easy 900‰/34t, Medium 800‰/48t, Hard 650‰/34t, Expert 0‰/34t).

The campaign trained from scratch on the parity-clean v9 surface,
bootstrapped by Overseer demonstrations — not by any checkpoint below.

## Historical lineage (deleted actors, for provenance only)

The last shipped artifact (0.14):

- Contract: gym v8 — 81 named features, 12 named conditions, 26 actions
  across three policy heads.
- Promoted source: full-policy continuation R2 update 140.
- Gameplay digest: `c36fce50824b9fb5`.
- Content-addressed training lineage:
  `sha256:3e1df7598e5b1dd1438bc96bad326a4f732b88f416a87176bec4ece15af6090c`.
- A v8-to-v9 bridge of these weights (digest `15eed88f6f9e3388`, zero
  feature columns + floored head rows, byte-identical behavior behind the
  legacy surface) existed briefly on the 0.15 branch and was deleted with
  the actor.

Its direct v7 ancestor was update 105 followed by selective production-row
revival:

- Gameplay digest: `4473f3e795891915`.
- Content-addressed lineage:
  `sha256:41de14c644a34fa26597a717cc6f01883529b24325436651120682d55799fe70`.

The v8 bridge appended five profile-condition columns with exact zero
first-layer weights; removing those columns reconstructed the promoted v7
artifact exactly. `tools/train/widen.py` retains the v7-to-v8 arm as the
recorded form of that migration.

## Why the masks existed (and must never return)

The 0.14 policy behaved only inside the masked surface it trained on.
Removing its caps without retraining produced zero decided matches across
the 25-map probe, 10.21 Fabricators per seat on average, 0/160 wins for
every ladder rung at 40,000 ticks, failed liveness, and movement in all 25
hash fixtures. The lesson going forward: train on exactly the surface that
ships, gate promotion on the full battery, and never reintroduce bot-only
rules to paper over a policy collapse.
