# oxide-kit

`oxide-kit` holds Oxide-specific engine services shared by the shell and the
driver. Keeping them here lets the graphical game and the headless harness use
the same replay, statistics, rendering, and scenario-running code without either
depending on the other.

This is not the home for game rules or UI state. Rules stay in `oxide-sim`,
while reusable game-independent primitives stay in `chassis`.

## Main pieces

- `checkpoint` captures a completed tick boundary: scenario, validated world,
  canonical controller roster, pending inputs, and optional incremental
  statistics. Restoration installs those parts without executing historical
  ticks. The session, simulation, and controller revisions are checked
  independently. A fingerprint binds the captured scenario and world, rejecting
  later mismatches even if both scenario copies change. Capture trusts the
  host's pairing; the fingerprint neither authenticates data nor proves
  historical origin. `RecordedCheckpoint` carries the existing recorder
  alongside this core so current hosts can continue exporting complete legacy
  replays. Recorder setup and duration are checked; restoration does not
  re-execute the log to prove its correspondence to the world. This internal
  contract does not change ordinary saves, Continue, or recovery.

- `bot_execution` collects commands in input seat order, using a shared pool of
  up to four workers when multiple bots are due. A busy or unavailable pool uses
  serial execution, so independent headless matches do not queue behind it.
  Batch workers that already run matches concurrently use `serially` to avoid
  adding bot threads to a saturated workload.

- `recovery` keeps a bounded incremental command journal, distinguishes prepared
  commands from completed ticks, and exports verified replay prefixes with build
  provenance. Its worker handles disk durability without blocking the caller.
- `load_replay` owns bounded Oxide replay loading and version-scoped setup
  compatibility.
- `runner` executes scenarios and replays headlessly through the same
  record-then-tick composition. Its opt-in traced step returns player-facing bot
  diagnostics without changing replay input or the ordinary step path.
- `recording` supplies a validated world-only origin for replay segments.
  Scenario-start records retain their original JSON shape. Checkpoint-origin
  records retain absolute ticks; their first available tick can be nonzero.
  Playback and replay statistics never execute controllers. A world-only segment
  cannot resume live play without its separate session checkpoint.
- `playback` provides bounded seeking between the recording origin and its end.
- `stats` derives match summaries from simulation truth. Replay statistics cover
  the available segment; live checkpoint statistics retain earlier session
  totals.
- `render` is the deterministic CPU renderer used for previews and goldens.
- `matchup` and `bench` build controlled combat and scale fixtures.
- `perceptual` compares rendered images without entering gameplay logic.

## Development

Run commands from the workspace root:

```sh
cargo test -p oxide-kit --locked
cargo test -p oxide-driver --test golden --locked
cargo clippy -p oxide-kit --all-targets --locked -- -D warnings
```

`diagnostics` optionally observes coarse shell operations and bot phases through
the existing parallel executor. It retains bounded timing history and runs an
independent atomic-progress watchdog. Diagnostic output is observational;
recovery's prepared/completed journal alone determines the playable prefix.
Bot-total timings include incremental planning work counters when a decision
runs. The counters cover shared field and site refinement, not every synchronous
planner operation; phase durations remain necessary for finding other stalls.
