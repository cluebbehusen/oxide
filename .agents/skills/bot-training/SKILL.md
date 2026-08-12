---
name: bot-training
description: Change, train, export, evaluate, and promote Oxide neural bot policies and their deterministic gym contract. Use for bot masks or actions, policy features and conditions, PPO or imitation campaigns, Q12 artifacts, ladder difficulty, personality profiles, bot parity, balance probes, neural cups, repair probes, the Overseer, or promotion gates.
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

The current v9 contract has 107 named integer features, 12 conditions, and 43
masked actions split into independent production, construction/maintenance,
upgrade, and military-operation heads (a step submits one action per head).
Confirm those numbers in live code before any edit; a contract change must
update every native and Python consumer together.

## The 0.15 starting point: no shipped actor

Every legacy bot is deleted: the frozen 0.14 neural ladder and its bot-only
masks, the scripted difficulty tiers, and the classic rule cascade. The v9
mask encodes shared legality only — parity-clean by construction. Bot seats
are inert until this campaign promotes an actor; there is no incumbent to
compare against, only gates to pass.

The Overseer (`Brain::overseer`) is the campaign's scripted foundation:
demonstration source for the BC prior, league opponent anchor, evaluation
yardstick, and the anchor for the repo's liveness/determinism/hash gates. It
plays the whole 0.15 tree fog-honestly on the shared intent surface. It is
training and QA infrastructure only — never wire it to any player-facing
surface.

Historical warning (why the masks existed): naively unmasking the frozen 0.14
actor collapsed it into Fabricator spam — 0/25 decided matches, no units on
some maps. A policy only behaves inside the surface it trained on; evaluate
candidates on exactly the surface they will ship with, and never weaken a
gate to admit a regression.

## Change a policy contract

1. State the shared gameplay change and the bot-observable contract separately.
2. Update the Rust-authored feature, condition, action, mask, and lowering
   catalogs. Keep action lowering responsible for lifecycle races that occur
   after a sampled decision.
3. Run native/Python handshake tests before collecting data.
4. When widening a float checkpoint across a contract change, use
   `tools/train/widen.py`; do not hand-edit tensor shapes. Preserve old
   behavior with exact zero feature columns and floored new action rows.
5. Bump the gym contract deliberately. Reject incompatible external
   artifacts at the loader rather than guessing.

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
  --mix "self=0.35,past=0.15,overseer=0.15,rusher=0.10,ffa=0.25"
uv run tournament.py --ckpt runs/run/pool/ckpt-XXXXX.pt
uv run export.py --ckpt <winner> --out runs/candidate.json
```

Use each league phase's fresh, write-once run name. Continue into a new phase;
never append changed settings or pool members to an existing run. Preserve the
content-addressed lineage manifest and exact driver, trainer, scenario, map,
initializer, and anchor identities it records.

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
cargo test -p oxide-sim --test neural_bot --locked -- --ignored --nocapture
cargo test -p oxide-sim --test bot_profiles --locked -- --ignored --nocapture
cargo test -p oxide-driver --test headless --locked
cargo test -p oxide-driver --test hashes --locked
```

Also run the candidate under the dealt profile, Turtle variant 1, and Balanced
variant 1; cover every faction and physical seat; inspect competitive-lifetime
value and body-time composition, structure reach, unhealthy caps, decisiveness,
repair behavior, profile identity, FFA/team behavior, and seat/geometry effects.
The probe also reports per-kind reach over competitive lifetimes and each
kind's share of the scrap bill. Both are diagnostic — no floor gates them —
and they are the readings that catch a kind nothing ever builds and money
sunk where no body-time share can see it.
Use `sweep`, `sweep-factorial`, `pace-sweep`, `matchup`, and
`tools/train/fun_gate.py` when their axis is affected. Compare candidates on
the same maps, seeds, ticks, factions, profiles, and schema; the Overseer is
the fixed scripted yardstick.

Never infer health from match duration alone. Inspect recent combat, economy,
production, roster movement, live or queued Harvesters, Reclaimers, remaining
scrap, and recovery-income routes before calling a cap deadlocked.

Finish with the complete workspace tests, strict Clippy, formatting, Python
training tests, deterministic hashes, and any versioned re-bless required by
the root instructions. Do not change thresholds, bless fixtures, or promote
weights merely to hide a candidate failure.

## Promote deliberately

Record the exported artifact's gameplay digest, lineage id, source checkpoint,
full command matrix, and raw reports. Promotion re-establishes the shipped
actor: embed the artifact, restore `seat_bots` construction for configured
seats, and recalibrate the Level execution handicaps — only after every
affected gate passes. Commit the artifact with the contract, tests,
version/hash movement, and concise current lineage update. Keep rejected runs
and local checkpoints out of production commits.
