# Neural artifact lineage

Read this file only when migrating, continuing, comparing, or replacing the
shipped policy. Treat repository code and artifact digests as canonical if this
snapshot ever disagrees with the live tree.

## Current state: no shipped artifact

The frozen 0.14 actor was deleted with the rest of the legacy bots in the
0.15 migration, along with its embed (`sim/src/bot/ladder_weights.json`),
its bot-only masks (per-kind building caps, private two-item queue
threshold, Fabricator screen gate), the `legacy_surface` bridge flag, and
`widen.py`'s v8-to-v9 arm. Bot seats are inert until the from-scratch gym-v9
campaign promotes an actor through the complete native-Q12 battery.

The 0.15 campaign trains from scratch on the parity-clean v9 surface (107
features, 12 conditions, 43 actions, four heads), bootstrapped by Overseer
demonstrations — not by any checkpoint below.

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
