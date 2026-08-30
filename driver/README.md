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
- `bot_eval` runs the player-facing controller to a decision, tick ceiling, or
  stall-loop anomaly, and emits compact JSONL with candidate, scenario,
  tick-ceiling, exact-profile, and anomaly provenance. It can exchange complete
  controller configurations between seats for paired personality or difficulty
  comparisons, or compare a player-facing profile with the frozen Overseer
  through an evaluation-only command source. Persisted batches are staged and
  never replace earlier evidence. A returned publication error rolls back files
  created by that invocation. Abrupt process termination can leave hidden
  staging files or a partial replay set because arbitrary final paths cannot be
  published atomically; inspect and remove that incomplete batch, then rerun it
  under a fresh candidate.
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
cargo run -p oxide-driver -- bot-eval skirmish \
  --difficulty prime --stance balanced --against-overseer --paired \
  --ticks 60000 --scenario-seeds 7000,7001 \
  --personality-seeds 9000,9001 --faction-cells fc,cf \
  --geometries authored,rot180 --overseer-policy-seed 0 \
  --candidate prime-overseer-a \
  --out replays/prime-overseer-a.jsonl \
  --replay-dir replays/prime-overseer-a
cargo test -p oxide-driver --locked
```

`--against-overseer` seats Overseer only inside the evaluation plan; it does not
make the frozen QA controller available to ordinary scenarios or match setup.
`--overseer-policy-seed` fixes Overseer's small legacy army-size jitter to one
identity that moves unchanged between seats; it defaults to zero and is recorded
in every row and replay description. Replay evidence requires `--out`, keeping
the exact structured controller provenance beside every saved replay. For a
controlled Prime-versus-Overseer block, cross `--faction-cells fc,cf` with
`--geometries authored,rot180` and use `--paired` so controller, physical-seat,
faction, and map-end effects can be separated. Supply independent
`--scenario-seeds` and `--personality-seeds`: the former controls simulation
randomness, the latter Prime's deterministic profile, and `bot-eval` evaluates
their cross-product rather than confounding the two sources. The runner refuses
nominal axis cells that resolve to the same executable matchup.

`--against-overseer` refuses a map whose seats share no ground route: the frozen
Overseer has no severed-ground play, so such a cell would measure a missing
capability rather than Prime. Compare player-facing profiles there instead.

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
