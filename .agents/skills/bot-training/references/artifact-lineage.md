# Neural artifact lineage

Read this file only when migrating, continuing, comparing, or replacing the
shipped policy. Treat repository code and artifact digests as canonical if this
snapshot ever disagrees with the live tree.

## Current state: the auto-2 champion (0.15.2)

The shipped artifact (`sim/src/bot/ladder_weights.json`) is the auto-2
autopilot campaign's champion, promoted 2026-08-12 on the complete
native-Q12 battery under the 0.15.2 rules (harvester replan stagger +
base-Array charge detection).

- Contract: gym v9 — 107 named features, 12 named conditions, 43
  actions across four heads. Parity-clean by construction.
- Gameplay digest: `21d1018489498a8e`.
- Content-addressed training lineage:
  `sha256:7c453c727ed8c2b13beee3a2d0a5c00a5137ea92575f4e9dde5849098b1bdc35`.
- Float parent committed at
  `tools/train/lineage-checkpoints/auto2-g1m3.pt`.
- Provenance chain: the r17 actor (see historical section) -> auto-1
  population fine-tune under the 0.15.1 harvester rule (champion
  g1m1, ckpt-02275: the measured 120-update sweet spot) -> auto-2
  population fine-tune under 0.15.2 with base-Array charge detection
  (champion g1m3, ckpt-02395; config: rusher 0.196, team 0.197,
  fixed-map-leaning). Both campaigns ran the training autopilot
  (tools/train/autopilot.py) with the fun gate as a hard constraint;
  experiments/2026-08-11-autopilot-auto-1.md and
  2026-08-12-autopilot-auto-2.md carry the full curves.
- Promotion battery: COMPLETE fun gate pass (the fine-tune era's
  first — arrays re-adopted once base-tier detection made radar pay);
  neural-cup vs the Overseer 83% mixed (24F/26C), faction pairs
  80/95/83/100 for ff/cc/fc/cf; rush canary 57% mixed with per-profile
  structure — fortress holds half (up from zero on every prior
  artifact), six personalities perfect, and the air-opening family
  (air-combined, swarm, air-raider) loses to the expert ground all-in:
  committed air openings are gambles by design, the documented
  residual. Profile battery 7/7/7/7 style signatures WITHOUT
  re-distillation (the personality columns survived both fine-tune
  campaigns); determinism exact; repair probe 8/8; ladder re-pinned
  from the champion's own sweep to Easy 900‰/34t, Medium 800‰/48t,
  Hard 800‰/34t, Expert 0‰/34t = 16/29/37/40 wins, strictly falling
  ticks. The parent-match gate is inapplicable to a full-policy
  fine-tune and was replaced by this lineage record.

## Historical lineage (superseded actors, for provenance only)

The 0.15.0 actor (r17, superseded by the auto-2 champion):

- Contract: gym v9, same as current.
- Gameplay digest: `320706eb6eb5882e`.
- Content-addressed training lineage:
  `sha256:12f69dd13ac584f563c0be16059e9bdf518bd92c12edf3c9613103a21acd05cd`
  (phase `style-distillation`).
- Chain: BC prior on the four v9 teachers -> PPO league r1-r10
  (peak ckpt r10-01975) -> r11 diversity polish -> r13 lock-in
  (trunk, ckpt-02075) -> r14 style-bonus columns -> r17
  named-condition distillation. Float parent committed at
  `tools/train/lineage-checkpoints/r17-distilled.pt`.
- Battery at promotion: Overseer cup 90% over 120 games, faction
  pairs 80-90%, complete fun gate under 0.15.0 rules, profile battery
  with parent-match to the r13 trunk, ladder 15/28/36/40 on its own
  sweep. Known residual then: fortress-family rush softness.

The last 0.14 artifact (deleted):

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
