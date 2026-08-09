---
name: bot-training
description: Change, train, widen, export, evaluate, and promote Oxide neural bot policies and their deterministic gym contract. Use for bot masks or actions, policy features and conditions, PPO or imitation campaigns, Q12 artifacts, ladder difficulty, personality profiles, bot parity, balance probes, neural cups, repair probes, promotion gates, or ladder_weights.json.
---

# Oxide bot training

Treat the bot as a fog-honest command source operating under the same legal
rules as a player. Treat the exported Q12 artifact, not a PyTorch checkpoint,
as the candidate that can ship.

Read [references/artifact-lineage.md](references/artifact-lineage.md) before
widening, continuing, comparing, or replacing the shipped policy.

## Protect the contracts

- Preserve deterministic inference: Q12 integers evaluated in `i64`, stable
  entity ordering, explicit tie-breaks, and no wall-clock or nondeterministic
  randomness in simulation behavior.
- Preserve fog honesty. Build observations and rewards only from information
  available to that player. Never shape on hidden enemy state.
- Keep counts, queues, costs, prerequisites, command validation, build times,
  movement, combat, and economy identical for humans and bots.
- Let difficulty change execution cadence and hesitation only. Let personality
  change preferences, not legal strategy.
- Distinguish finite authored opening milestones and recovery controllers from
  hidden rules. They may choose among shared-legal actions and must be gated for
  liveness and effectiveness.
- Keep `FEATURE_NAMES`, `CONDITION_NAMES`, action catalogs, canonical profile
  vectors, native inference, and the Python handshake exact. Make skew fail at
  startup.

The current v8 contract has 81 named integer features, 12 conditions, and 26
masked actions split into independent production, construction/maintenance,
and military-operation heads. Confirm those numbers in live code before any
edit; a contract change must update every native and Python consumer together.

## Respect the temporary 0.14 restriction

Do not remove or tune the current bot-only building caps, private two-item queue
threshold, or hard Fabricator screen gate in 0.14. They are acknowledged legacy
violations, not approved design. Remove them only together with the 0.15
economy/action-surface retrain and full promotion battery.

Unmasking them under the unchanged actor was catastrophic: the 25-map probe
fell from 6/25 decided matches to 0/25, average Fabricators rose from 0.99 to
10.21 per seat, combat entropy fell from 2.40 to 1.42 bits, every ladder rung
scored 0/160 wins at 40,000 ticks, liveness failed, and all 25 hash fixtures
moved. Some maps trained no units while building 11 to 14 Fabricators. Never
bless this outcome or weaken a gate to admit it.

The repository does not yet document a supported same-shape v8-to-v9 bridge or
a recoverable path to the R2 update-140 float checkpoint. Establish and test
those inputs before the 0.15 campaign; do not invent a migration command or
assume a local checkpoint is canonical.

## Change a policy contract

1. State the shared gameplay change and the bot-observable contract separately.
2. Update the Rust-authored feature, condition, action, mask, and lowering
   catalogs. Keep action lowering responsible for lifecycle races that occur
   after a sampled decision.
3. Run native/Python handshake tests before collecting data.
4. When widening, use `tools/train/widen.py`; do not hand-edit tensor shapes.
   Preserve old behavior with exact zero feature columns and unreachable new
   action floors in the shipped bridge. Give a trainable float resume reachable
   logits only when the campaign intentionally learns the new action.
5. Evaluate the embedded weights unchanged on the widened contract first.
   Treat behavior outside their trained masks as out-of-distribution evidence,
   not a justification for a permanent restriction.
6. Bump the gym contract deliberately. Reject incompatible external artifacts
   with an actionable migration command rather than guessing.

## Train reproducibly

Work from `tools/train/` using its locked `uv` project. Build the release driver
first; the Python gym launches that Rust binary and validates the handshake.

Typical campaign stages are:

```sh
cargo build -p oxide-driver --release --locked
cd tools/train
uv run bc.py --arch deep --episodes 48 --out runs/prior.pt
uv run league.py --name run --initialize-from runs/prior.pt \
  --anchor runs/prior.pt --collection episodes --maps grand \
  --mix "self=0.35,past=0.15,tier=0.15,rusher=0.10,ffa=0.25"
uv run tournament.py --ckpt runs/run/pool/ckpt-XXXXX.pt
uv run export.py --ckpt <winner> --out runs/candidate.json
```

Use each league phase's fresh, write-once run name. Continue into a new phase;
never append changed settings or pool members to an existing run. Preserve the
content-addressed lineage manifest and exact driver, trainer, scenario, map,
initializer, anchor, and incumbent identities it records.

Choose checkpoints by tournament and in-loop canaries, not recency. Prefer a
short fixed-anchor continuation from a strong, decisive parent over a long
fresh league unless evidence proves otherwise. Keep episode-aligned collection
for variable-length games, propagate team payoff through valid prefixes, and
distinguish truncation from a true terminal.

Use seeded shaping only for successful, fog-safe effects, never sampled intent
or action quotas. Use narrow row revival only as a downstream-gated surgical
tool:

```sh
uv run revive.py --initialize-from runs/parent.json --actions 3,8 \
  --promote-actions 3 --out runs/revived.pt
```

## Evaluate the shipping artifact

Export first, then measure native Q12 inference. A torch-side tournament is a
checkpoint filter, not a promotion verdict. Run the following commands from the
repository root.

At minimum, run and retain raw results for:

```sh
cargo run -p oxide-driver --release -- neural-cup --weights tools/train/runs/candidate.json
cargo run -p oxide-driver --release -- neural-cup --weights tools/train/runs/candidate.json --factions ff
cargo run -p oxide-driver --release -- neural-cup --weights tools/train/runs/candidate.json --factions cc
cargo run -p oxide-driver --release -- neural-cup --weights tools/train/runs/candidate.json --factions fc
cargo run -p oxide-driver --release -- neural-cup --weights tools/train/runs/candidate.json --factions cf
cargo run -p oxide-driver --release -- balance-probe --weights tools/train/runs/candidate.json --seeds 3 --ticks 40000
cargo run -p oxide-driver --release -- repair-probe --weights tools/train/runs/candidate.json
cargo test -p oxide-sim --test neural_ladder --locked -- --nocapture
cargo test -p oxide-driver --test headless --locked
cargo test -p oxide-driver --test hashes --locked
```

Also run the candidate under the dealt profile, Turtle variant 1, and Balanced
variant 1; cover every faction and physical seat; inspect competitive-lifetime
value and body-time composition, structure reach, unhealthy caps, decisiveness,
repair behavior, profile identity, FFA/team behavior, and seat/geometry effects.
Use `sweep`, `sweep-factorial`, `yardstick`, `pace-sweep`, `matchup`, and
`tools/train/fun_gate.py` when their axis is affected. Compare candidate and incumbent on
the same maps, seeds, ticks, factions, profiles, and schema.

Never infer health from match duration alone. Inspect recent combat, economy,
production, roster movement, live or queued Harvesters, Reclaimers, remaining
scrap, and recovery-income routes before calling a cap deadlocked.

Finish with the complete workspace tests, strict Clippy, formatting, Python
training tests, deterministic hashes, and any versioned re-bless required by
the root instructions. Do not change thresholds, bless fixtures, or promote
weights merely to hide a candidate failure.

## Promote deliberately

Record the exported artifact's gameplay digest, lineage id, source checkpoint,
full command matrix, and raw reports. Replace `sim/src/bot/ladder_weights.json`
only after every affected gate passes. Commit the artifact with the contract,
tests, version/hash movement, and concise current lineage update. Keep rejected
runs and local checkpoints out of production commits.
