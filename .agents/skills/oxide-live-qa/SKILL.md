---
name: oxide-live-qa
description: Drive, inspect, reproduce, profile, and verify Oxide through its live debug server, windowless session, replay tools, native GPU shell, screenshots, and smoke harness. Use for gameplay QA, UI or input checks, replay forensics, performance profiling, live-shell automation, visual verification, or changes to driver/debug protocol behavior.
---

# Oxide live QA

Choose the narrowest harness that can answer the question, then preserve the
boundary between simulation evidence and presentation evidence.

## Choose the evidence source

- Use `driver run`, `replay`, `replay-inspect`, or `replay-stats` for exact,
  deterministic simulation and replay questions.
- Use `driver session` for a persistent windowless match driven through the
  same shared protocol as the shell. Treat its screenshots as CPU schematics,
  never as visual-polish evidence.
- Use `oxide-shell --debug-server --paused` for real input, menus, camera, HUD,
  fog, audio, and GPU rendering under a driven clock.
- Use `driver profile-shell` for native frame timings. Do not substitute
  headless tick throughput or session screenshots for GPU-shell profiling.
- Use `driver smoke --spawn` for an isolated end-to-end shell check.
- Use `driver shots` only as a local per-machine visual regression gate. Its
  gitignored references are not portable CI goldens.

Ask each command for `--help` when the surface may have changed. The driver CLI
is canonical; examples below are an operating pattern, not a replacement for
its parser.

## Drive a deterministic session

Start either a real paused shell or a windowless server:

```sh
cargo run -p oxide-shell -- --debug-server --paused
cargo run -p oxide-driver -- session --scenario skirmish
```

Use the same client against either:

```sh
driver() { cargo run -q -p oxide-driver -- "$@"; }
driver live status
driver live state --map
driver live fog 0
driver live step 1
driver live advance 300
driver live attack-move 0 --units 3 --to 34,18
driver live screenshot -o screenshots/check.png
driver live save-replay replays/session.json
```

Keep the server in driven mode while reproducing tick-sensitive behavior.
Stage all game mutations through tick-stamped commands; never add a QA path
that edits simulation state directly.

Treat `live state` as omniscient QA data and `live fog <seat>` as the player's
honest knowledge. Use the latter for bot, visibility, remembered salvage, and
information-leak claims. A windowless server must refuse window-only requests
rather than emulate them.

## Inspect and reproduce replays

Use the record as the source of truth:

```sh
driver replay replays/session.json
driver replay-inspect replays/session.json --tick 3000,6000 --fog-seat 1 --map
driver replay-stats replays/session.json
driver replay-summary replays/session.json --minimaps sparse
```

Start bot-conduct review with `replay-summary`: it narrates the whole match
as text — first contact, battles, expansions, eliminations, lulls, per-seat
digests (including command-rejection and stall counts), and coarse ASCII
minimaps — for a fraction of a screenshot's cost. `--until T` summarizes a
prefix, `--every N` sets digest cadence, `--json` is the stable contract.
Reserve schematic screenshots for confirming what the summary surfaces.

Interpret snapshot tick `N` as state before commands stamped `N` execute.
Compare the reproduced final hash with the live hash. Respect replay version
checks; use version-mismatch overrides only for explicit archaeology and label
the result non-authoritative.

For a visual replay, launch `oxide-shell --watch <replay>`. For a saved match,
load it as a live continuation rather than exposing a fog-free mid-match
viewer.

## Inspect native presentation

Capture the real shell whenever judging appearance, animation, interaction, or
audio. Exercise the relevant camera zoom, fog state, faction, selection state,
and actual action trigger. Read every captured PNG or sequence at normal play
scale; do not judge the game from generated review cards or the CPU renderer.

Use injected events only through the shell input funnel:

```sh
driver live inject-wheel 2.0
driver live inject-key escape
driver live inject-text "my save"
driver live capture-sequence --present --out screenshots/motion
driver live performance
```

For timing a replay-derived gameplay interval:

```sh
driver profile-shell replays/session.json --from 4500 --to 5750 --speed 8
```

Use release mode unless debug-build behavior is the question. Select a window
that remains on the Playing screen through `--to`; the harness correctly
refuses an interval that has already reached a result. `profile-shell`
reconstructs only through `--from`; commands after that tick come from the live
continuation, not the recorded replay suffix.

### Profile screen transitions

Launch with `--profile-frames` and bracket one transition at a time with
`driver live performance --reset`. Debug requests such as `load-replay` run
outside frame timing, so measure their command latency separately from the
first rendered frame. Capture screenshots only after the timing window because
GPU readback and PNG encoding contaminate the sampled frame.

Interpret `work` as CPU time from frame entry through presentation handoff.
Use frame `interval` to detect a blocked presentation, driver, vsync, or OS
gap; it is not a direct GPU timestamp. Since `interval.max` has no attached
screen label, isolate Results, Final Map, and steady-state windows with resets.
Repeat cold-process transitions and warm toggles separately, keeping replay,
window size, DPI, camera, machine, and build profile fixed. Choose acceptance
thresholds before comparing the candidate.

For a natural Playing-to-Results stall, preserve a near-end save. Reset frame
samples, externally time the decisive `driver live step 1`, then inspect the
first Results frame separately. If that still combines too much work, add
bounded per-phase instrumentation instead of guessing from one long interval.

## Validate changes

Run focused protocol, replay, session-parity, shell, and UI tests first. Run
ignored native batteries when their GPU or window behavior is in scope. Then
run the repository-wide tests, Clippy, and format checks required by the root
instructions.

Keep screenshots and replays in their gitignored scratch directories. Put
stable test fixtures under crate test directories, and inspect the index so no
local capture or replay enters a production commit.
