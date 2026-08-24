# oxide-kit

`oxide-kit` holds Oxide-specific engine services shared by the shell and the
driver. Keeping them here lets the graphical game and the headless harness use
the same replay, statistics, rendering, and scenario-running code without either
depending on the other.

This is not the home for game rules or UI state. Rules stay in `oxide-sim`,
while reusable game-independent primitives stay in `chassis`.

## Main pieces

- `runner` executes scenarios and replays headlessly.
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
