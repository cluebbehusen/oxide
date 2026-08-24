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
cargo test -p oxide-driver --locked
```
