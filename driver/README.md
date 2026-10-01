# oxide-driver

`oxide-driver` is Oxide's headless harness and remote control. Its library
combines reusable inspection and automation tools; its CLI runs scenarios and
replays, renders maps, audits maps and match behavior, drives a live shell, or
serves the same debug protocol without a window.

The driver observes and orchestrates the game through public simulation and
protocol boundaries. It does not contain alternate gameplay rules, and its
automated players use the same command path as every other player.

Build provenance belongs to this executable. Its build script watches the driver
and shared dependency package trees plus shared build inputs, assets, and
scenarios; shell-only edits and private workspace notes do not contribute to its
dirty status. Reports retain both the original recording identity and this
exporter's identity. Source archives report unknown provenance. The build also
embeds a reference digest of the `oxide-bot`, simulation and `chassis` sources,
the `kit` code that hosts `oxide-bot`, and `Cargo.lock`, which evaluation
records as the frozen reference it compares against.

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
- `bot_eval` runs the player-facing controllers to a decision, tick ceiling, or
  stall-loop anomaly, and emits compact JSONL with candidate, scenario,
  tick-ceiling, exact-profile, and anomaly provenance. `--controller` selects
  `scripted` (`oxide-bot`, the default) or `opponent` (`oxide-opponent`), and
  `--opponent-controller` overrides seat one of a two-seat scenario. It can
  exchange complete controller configurations between seats for paired
  controller, personality or difficulty comparisons, including crossed exact
  simulation seeds, personality seeds, faction assignments, and geometry cells.
  Persisted batches are staged and never replace earlier evidence. Optional
  decision traces stream fog-honest controller diagnostics to a separate JSONL
  sidecar without entering compact rows or replays. A returned publication error
  rolls back files created by that invocation. Abrupt process termination can
  leave hidden staging files or a partial replay set because arbitrary final
  paths cannot be published atomically; inspect and remove that incomplete
  batch, then rerun it under a fresh candidate.
- Evaluation rows also record each seat's team and elimination tick, the
  producing build and reference digest, omniscient failure detectors (repeated
  impossible orders, abandoned paid construction, starved production,
  `oxide-opponent` missions stuck in one phase, and an army idle at home) with
  per-producer idle diagnostics, the fate of armed ground units trained on
  severed ground, and income against a saturated-economy estimate while the seat
  is still in the match. These are QA evidence computed from authoritative state
  and the missions a controller reports; they never reach a controller.
- `bot_matrix` expands a manifest from `evaluation/` into cells of
  `oxide-opponent` against `oxide-bot`, every seat sharing one personality seed,
  plus one `oxide-bot` mirror leg per cell. A duel is a head-to-head pair with
  the seats exchanged. A team map has a head-to-head pair, one team of each bot
  and then the sides swapped, and a mixed pair: both bots on each team,
  alternating along its front so that facing enemies run different bots, then
  every seat flipped. A free-for-all is a mixed pair on alternating seats. Every
  seat is controlled, including a map's authored human chair. A mixed leg goes
  to the bot whose seats outlast the other's more often. Mirror rows are cached
  under the reference digest and reused while it is unchanged. `bot-matrix`
  publishes labelled rows and prints pair outcomes, the new bot's share of won
  legs with Wilson intervals, decided rates, placement, failure incidents and
  income by match mode, overall and by difficulty, stance and map family;
  `bot-matrix-report` re-reads published rows. `--replay-dir` saves a replay of
  every evaluated leg. Evaluation inputs live in `evaluation/`, not
  `scenarios/`, whose every file the shell menu, map gates and golden sweeps
  read.
- `bot_pressure` runs the staged scenarios in `evaluation/pressure/`: a scripted
  attacker seat presses one situation (an early rush, air harassment, an
  artillery siege, a transport drop) on a bot seat, and a check over
  authoritative state reports whether the bot answered by the deadline. The
  checks are QA evidence and never reach a controller.
- `bot_cost` times controller decisions on named workloads: a Skirmish duel, the
  seven-bot Skyhook game, and a staged mature-army match kept under
  `tests/fixtures/performance/` so it stays out of the shipped pool. It reports
  average and p99 wall time per decision, total CPU, and the fog-honest
  observation and `oxide-bot` orientation costs, per seat and per controller.
  Seats decide serially with tracing off; its command and final hashes equal an
  untimed run.
- `audit`, `sweep`, `pace`, and `factorial`, plus the `matchup` CLI backed by
  `oxide-kit`, measure map geometry, configured-bot pacing, seat effects, and
  combat behavior.
- `auto`, `smoke`, `shots`, and `profile` exercise the real shell where a
  headless run is not enough.
- `ios` (aliased as `cargo ios`) builds, installs, and launches the iPad app on
  a paired iPad or an iPad simulator.

Run `oxide-driver --help` for the current command tree. Procedures for live
shell QA belong in the repository's `oxide-live-qa` skill rather than here.

## Development

Run commands from the workspace root:

```sh
cargo run -p oxide-driver -- --help
cargo run -p oxide-driver -- run skirmish --ticks 2000 --all-bots
cargo run -p oxide-driver -- bot-eval skirmish --difficulty prime --paired
cargo test -p oxide-driver --locked
```

The
[evaluation procedure](../.agents/skills/scripted-bot/references/evaluation.md)
owns comparison matrices, trace capture, seed/profile provenance and anomaly
interpretation. The live-QA skill owns native inspection. Aggregate results do
not establish opponent quality or isolate simulation fairness.

`bot-eval --jobs N` bounds concurrent matches (default four, capped by available
CPUs and leg count). Multiple match workers disable nested bot-seat parallelism;
`--jobs 1` uses ordinary seat scheduling. Each worker stages replay and trace
output privately; results merge in input-plan order before publication. This
improves batch throughput, not individual tick latency.

## Performance regression checks

The
[performance and persistence procedure](../.agents/skills/oxide-live-qa/references/performance.md)
provides fresh-checkout workload commands, portable save-size and planning-work
guards, and scoped native timing requirements. Use its fixed inputs for
candidate/control comparisons; wall-clock thresholds are machine-qualified.

## Coverage boundaries

`tests/hashes.rs` runs six small controller-free scenarios for economy,
construction, ground combat, air transport, fog and targeting, and team victory.
Each scenario asserts accepted commands and the expected events before comparing
named milestone hashes. Independent execution, intermediate state round trips,
and reconstruction of the recorded command stream are checked throughout.
`tests/goldens/state-hashes.json` is shared by the existing cross-platform
matrix.

`tests/lockstep.rs` runs an `oxide-net` host and two clients in one process over
delayed links on a virtual clock. Each human seat's orders come from a scripted
controller on that seat's own machine and cross the wire. It checks that every
machine executes identical batches under latency and jitter, and covers a
stalled client, a stuck client, a closed connection, a silent host, and a
diverged client. One test instead joins, starts, and plays a short match over
real localhost TCP with a real clock.

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
quotas, or a historical command stream. With `OXIDE_SOAK_TRACE_DIR` set, each
run also writes a state hash every 20 ticks and its replay into that directory;
CI compares those hash files across operating systems without a checked-in
golden. The exhaustive all-map sweep is ignored by default and uses the same
checks, extending large, vast, and grand maps to 24,000 ticks. Cheap map gates
and small rule-hash fixtures retain their scope. The opening image and renderer
showcase cover drawing without depending on an autonomous midgame.

Run the exhaustive sweep explicitly:

```sh
cargo test -p oxide-driver --test headless --locked every_shipped_scenario_preserves_state_integrity -- --ignored --exact --nocapture
```
