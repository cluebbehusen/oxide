# oxide-kit

`oxide-kit` holds Oxide-specific engine services shared by the shell and the
driver. Keeping them here lets the graphical game and the headless harness use
the same replay, statistics, rendering, and scenario-running code without either
depending on the other.

This is not the home for game rules or UI state. Rules stay in `oxide-sim`,
while reusable game-independent primitives stay in `chassis`.

## Main pieces

- `bot_execution` collects commands in input seat order, using a shared pool of
  up to four workers when multiple bots are due. A busy or unavailable pool uses
  serial execution, so independent headless matches do not queue behind it.
  Batch workers that already run matches concurrently use `serially` to avoid
  adding bot threads to a saturated workload.

- `load_replay` owns bounded Oxide replay loading and version-scoped setup
  compatibility.
- `runner` executes scenarios and replays headlessly through the same
  record-then-tick composition. Its opt-in traced step returns player-facing bot
  diagnostics without changing replay input or the ordinary step path.
- `playback` provides bounded seeking and replay-viewer state.
- `stats` derives match summaries from simulation truth.
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
