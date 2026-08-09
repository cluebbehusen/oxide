# Shipped neural artifact lineage

Read this file only when migrating, continuing, comparing, or replacing the
shipped policy. Treat repository code and artifact digests as canonical if this
snapshot ever disagrees with the live tree.

## Current artifact

- Runtime: `sim/src/bot/ladder_weights.json`, quantized Q12 tensors evaluated in
  deterministic `i64` inference.
- Contract: gym v8, with 81 named features, 12 named conditions, and 26 actions
  across three policy heads.
- Promoted source: full-policy continuation R2 update 140.
- Gameplay digest: `c36fce50824b9fb5`.
- Content-addressed training lineage:
  `sha256:3e1df7598e5b1dd1438bc96bad326a4f732b88f416a87176bec4ece15af6090c`.
- The checked-in weights are the byte-exact exporter output of that promoted
  source. Later profile-only continuations did not pass the promotion battery.

## Direct ancestor and bridge

The decisive v7 source was update 105 followed by selective production-row
revival. Its promoted artifact had:

- Gameplay digest: `4473f3e795891915`.
- Content-addressed lineage:
  `sha256:41de14c644a34fa26597a717cc6f01883529b24325436651120682d55799fe70`.

The v8 bridge appended five profile-condition columns with exact zero
first-layer weights. Removing those columns reconstructs the promoted v7
artifact exactly. `tools/train/widen.py` is the supported v7-to-v8 artifact and
checkpoint migration; the v8 loader must refuse unmigrated external artifacts
instead of inferring a shape conversion.

## Current migration boundary

The 0.14 policy still carries temporary bot-only building caps, a private
two-item production-queue threshold, and a Fabricator screen gate. These are
known parity violations kept only because the actor was trained with those
masks.

Removing them without retraining produced zero decided matches across the
25-map probe, 10.21 Fabricators per seat on average, 0/160 wins for every ladder
rung at 40,000 ticks, failed liveness, and movement in all 25 hash fixtures.
Remove those restrictions only as part of the coordinated 0.15 economy and
full-action-surface retraining campaign, and require the complete native-Q12
promotion battery before shipping.
