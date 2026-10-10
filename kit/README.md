# oxide-kit

`oxide-kit` holds Oxide-specific engine services shared by the shell and the
driver, so the graphical game and the headless harness use the same controller
hosting, checkpoint, replay, recovery, statistics, and rendering code without
either depending on the other. A few services, such as the headless `runner` and
`bench`, serve only the driver.

This is not the home for game rules or UI state. Rules stay in `oxide-sim`,
while reusable game-independent primitives stay in `chassis`.

## Main pieces

- `controller` hosts `oxide-opponent` as one `SeatController` per configured
  seat. `seat_controllers` builds them in seat order, sharing one map model that
  `OpponentMap` builds only for a roster with a seat. Each seat also holds a
  bounded buffer of its own `OrderStalled` and `CommandRejected` events. Hosts
  call `record_events` after every tick that runs with controllers,
  fast-forwards included; the seat's next decision consumes the buffer and its
  checkpoint saves it.
- `checkpoint` captures a completed tick boundary: scenario, validated world,
  canonical controller roster, pending inputs, and optional incremental
  statistics. Each seat's controller must match the scenario's configuration.
  Restoration installs those parts without executing historical ticks. The
  session, simulation, and controller revisions are checked independently. A
  fingerprint binds the captured scenario and world, rejecting later mismatches
  even if both scenario copies change. Capture trusts the host's pairing; the
  fingerprint neither authenticates data nor proves historical origin. Player
  saves use the core checkpoint without historical commands; recovery pairs it
  with a world origin and a completed command suffix.

- `bot_execution` collects commands in input seat order, using a small shared
  worker pool when several bots are due. A busy or unavailable pool falls back
  to serial execution, so independent headless matches do not queue behind it;
  batch workers that already run matches concurrently use `serially`. Live
  sessions can speculate one decision on controller clones against an immutable
  shared world. Collection validates the world, tick, and roster and installs
  the complete clones before returning commands, while the session's own
  controllers stay available for saves. Speculation drains own-event buffers
  only in its clones, so events are consumed when its result is installed.

- `recovery` keeps a bounded incremental command journal, distinguishes prepared
  commands from completed ticks, and exports verified replay prefixes with build
  provenance. Its worker handles disk durability without blocking the caller.
  The host supplies build identity; the kit never probes Git or embeds a sibling
  executable's revision. Diagnostics share the recording identity, while export
  also records the identity of the executable preparing the report.
- `load_replay` owns bounded, strict Oxide replay loading.
- `runner` executes scenarios and replays headlessly, recording each tick's
  commands before the tick runs. Its opt-in traced step returns player-facing
  bot diagnostics without changing replay input or the ordinary step path.
- `recording` supplies a validated world-only origin for replay segments.
  Scenario-start records carry no origin. Checkpoint-origin records retain
  absolute ticks; their first available tick can be nonzero. Playback and replay
  statistics never execute controllers. A world-only segment cannot resume live
  play without its separate session checkpoint.
- `playback` provides bounded seeking between the recording origin and its end.
- `stats` derives match summaries from simulation truth. Replay statistics cover
  the available segment; live checkpoint statistics retain earlier session
  totals.
- `render` is the deterministic CPU renderer used for previews and goldens.
- `bench` builds the mass-battle scale fixture.
- `perceptual` compares rendered images without entering gameplay logic.

## Development

Run commands from the workspace root:

```sh
cargo test -p oxide-kit --locked
cargo test -p oxide-driver --test golden --locked
cargo clippy -p oxide-kit --all-targets --locked -- -D warnings
```

`diagnostics` is the process's crash and freeze monitor: main-thread stages and
bot seat heartbeats published through atomics, a stall watchdog, and a panic
hook that records a backtrace. Recent frame timing stays in memory and is
written only with an incident, to the open recording or the recovery root.
Diagnostic output is observational; recovery's prepared/completed journal alone
determines the playable prefix.
