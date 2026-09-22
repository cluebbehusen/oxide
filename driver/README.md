# oxide-driver

`oxide-driver` is Oxide's headless harness and remote control. Its library
combines reusable inspection and automation tools; its CLI runs scenarios and
replays, renders maps, audits maps and match behavior, drives a live shell, or
serves the same debug protocol without a window.

The driver observes and orchestrates the game through public simulation and
protocol boundaries. It does not contain alternate gameplay rules, and its
automated players use the same command path as every other player.

## Main pieces

- Re-exported `runner`, `render`, `playback`, and `stats` come from `oxide-kit`
  and keep headless execution shared with the shell.
- `client` speaks the debug protocol; `session` serves it windowlessly. Serde on
  `Session` uses the shared session checkpoint and retained recorder, restoring
  controllers and queued input without replaying earlier ticks. The existing CLI
  and debug save/load commands continue to use replay files.
- `recovery-inspect <session-directory> [--export <new-report-directory>]`
  verifies an interrupted journal and exports its completed replay prefix plus
  available diagnostic sidecars without needing a responsive shell.
- `replay_inspect` and `replay_summary` provide exact snapshots and compact
  match narratives. Checkpoint-origin recordings report their first available
  absolute tick, and summaries and inactivity windows cover only that segment.
  Inspection schema 2 and summary schema 4 expose this boundary; inspection
  rejects requests for unavailable earlier ticks.
- `bot_eval` runs the player-facing controller to a decision, tick ceiling, or
  stall-loop anomaly, and emits compact JSONL with candidate, scenario,
  tick-ceiling, exact-profile, and anomaly provenance. It can exchange complete
  controller configurations between seats for paired personality or difficulty
  comparisons, including crossed exact simulation seeds, personality seeds,
  faction assignments, and geometry cells. Persisted batches are staged and
  never replace earlier evidence. Optional decision traces stream fog-honest
  controller diagnostics to a separate JSONL sidecar without entering compact
  rows or replays. A returned publication error rolls back files created by that
  invocation. Abrupt process termination can leave hidden staging files or a
  partial replay set because arbitrary final paths cannot be published
  atomically; inspect and remove that incomplete batch, then rerun it under a
  fresh candidate.
- `audit`, `sweep`, `pace`, and `factorial`, plus the `matchup` CLI backed by
  `oxide-kit`, measure map geometry, configured-bot pacing, seat effects, and
  combat behavior.
- `auto`, `smoke`, `shots`, and `profile` exercise the real shell where a
  headless run is not enough.

Run `oxide-driver --help` for the current command tree. Procedures for live
shell QA belong in the repository's `oxide-live-qa` skill rather than here.

## Development

Run commands from the workspace root:

```sh
cargo run -p oxide-driver -- --help
cargo run -p oxide-driver -- run skirmish --ticks 2000 --all-bots
cargo run -p oxide-driver -- bot-eval skirmish --difficulty prime --paired
cargo run -p oxide-driver -- bot-eval skirmish --difficulty prime \
  --opponent-difficulty standard --same-personality-seed --paired
cargo run -p oxide-driver -- bot-eval skirmish \
  --difficulty prime --stance balanced --opponent-difficulty standard --paired \
  --ticks 60000 --scenario-seeds 7000,7001 \
  --personality-seeds 9000,9001 --faction-cells fc,cf \
  --geometries authored,rot180 \
  --candidate prime-standard-a \
  --out replays/prime-standard-a.jsonl \
  --replay-dir replays/prime-standard-a
cargo test -p oxide-driver --locked
```

Controlled comparisons cross `--faction-cells fc,cf` with
`--geometries authored,rot180`. `--paired` exchanges complete controller
profiles while holding the physical map and faction rosters fixed. Independent
`--scenario-seeds` and `--personality-seeds` form a Cartesian product: the
latter sets each cell's primary personality, with ordinary seat-seed assignment
and `--same-personality-seed` semantics. `--runs` selects consecutive seed cells
and cannot be combined with explicit axes. The runner refuses nominal cells that
resolve to the same executable matchup. Replay evidence requires `--out`,
keeping exact structured controller provenance beside every saved replay.

`sweep`, `pace-sweep`, `sweep-factorial`, and `bench --scenario` accept
`--difficulty`, `--stance`, and `--personality-seed`, defaulting to
Standard/Balanced/zero. Every seat receives that same full profile, fixed across
simulation seeds. Structured sweep reports and textual output identify the exact
profile and simulation version. These results measure the configured bot
interacting with the simulation; symmetric profiles do not isolate engine or map
fairness. The synthetic mass-battle benchmark remains simulation-only.

`--decision-trace-out` requires `--out` and an explicit candidate. It records
only diagnostics produced by the player-facing controller at actual decision
ticks for either seat. The sidecar is captured during the authoritative
evaluation run because reconstructing policy reasoning later from a replay may
use different controller code. It is not replay input, and enabling it does not
change the compact row, command stream, final hash, or replay payload.

`bot-eval --jobs N` bounds concurrent matches (default four, capped by available
CPUs and leg count). Multiple match workers disable nested bot-seat parallelism;
`--jobs 1` runs legs serially with ordinary seat scheduling. Workers stage each
completed replay and stream trace records to private files. Completed payloads
are not accumulated in memory. Rows and traces merge in input-plan order before
the whole invocation publishes. This improves evaluation throughput, not the
latency of an individual simulation tick.

Each `bot-eval` row reports rejected commands and stalled orders by reason. Its
per-unit stall breakdown distinguishes one persistently blocked order from a
controller-wide failure and points replay inspection at the exact unit. When one
unit stalls the same way `--stall-loop-limit` times (200 by default, 0
disables), the leg stops with `termination: stall_loop` and a `stall_loop`
record naming the seat, unit, reason, count, and tick, instead of burning the
ceiling on an order a controller re-issues every think. The command-stream hash
exposes different seed cells that nevertheless generated identical play. Treat
those metrics as diagnostics; inspect the preserved replays and use human play
and replay judgment to decide whether behavior is credible or fun.

## Coverage boundaries

`tests/hashes.rs` runs six small controller-free scenarios for economy,
construction, ground combat, air transport, fog and targeting, and team victory.
Each scenario asserts accepted commands and the expected events before comparing
named milestone hashes. Independent execution, intermediate state round trips,
and reconstruction of the recorded command stream are checked throughout.
`tests/goldens/state-hashes.json` is shared by the existing cross-platform
matrix.

`tests/player_facing_hashes.rs` checks exact recovery funding and repeated
nearby harvest deliveries through the maintained controller. Behavioral
assertions precede short state and tick-stamped command hashes. Independent
controllers, state round trips, and replay reconstruction must agree at every
step.

The regular integrity harness runs Skirmish, Twin Forges, and Basalt Spine for
up to 12,000 ticks each, covering an open duel, team play, and constrained
terrain. It seats Standard/Balanced bots with personality seed zero and checks
state validity and serialization, including the initial and final states. It
stops after validating a terminal result without requiring a winner, activity
quotas, or a historical command stream. The exhaustive all-map sweep is ignored
by default and uses the same checks, extending large, vast, and grand maps to
24,000 ticks. Cheap map gates and small rule-hash fixtures retain their scope.
The opening image and renderer showcase cover drawing without depending on an
autonomous midgame.

Run the exhaustive sweep explicitly:

```sh
cargo test -p oxide-driver --test headless --locked every_shipped_scenario_preserves_state_integrity -- --ignored --exact --nocapture
```
