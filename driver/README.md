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
- `client` speaks the debug protocol; `session` serves it windowlessly.
- `replay_inspect` and `replay_summary` provide exact snapshots and compact
  match narratives.
- `bot_eval` runs the player-facing controller to a decision or tick ceiling,
  emits compact JSONL with candidate, scenario, tick-ceiling, exact-profile, and
  anomaly provenance, and can exchange complete controller configurations
  between seats for paired personality or difficulty comparisons. Persisted
  batches are staged and never replace earlier evidence. A returned publication
  error rolls back files created by that invocation. Abrupt process termination
  can leave hidden staging files or a partial replay set because arbitrary final
  paths cannot be published atomically; inspect and remove that incomplete
  batch, then rerun it under a fresh candidate.
- `audit`, `sweep`, `pace`, and `factorial`, plus the `matchup` CLI backed by
  `oxide-kit`, measure maps, pacing, fairness, and combat behavior.
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
cargo run -p oxide-driver -- bot-eval skirmish --candidate candidate-a \
  --replay-dir replays/bot-eval --out replays/bot-eval.jsonl
cargo test -p oxide-driver --locked
```

Each `bot-eval` row reports rejected commands and stalled orders by reason. Its
per-unit stall breakdown distinguishes one persistently blocked order from a
controller-wide failure and points replay inspection at the exact unit.
