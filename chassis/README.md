# chassis

`chassis` is the reusable foundation for deterministic simulations. It owns
fixed-point math, stable randomness and hashing, grids and pathfinding, replay
records, and durable file writes. It contains no Oxide game rules, rendering,
input, or engine dependencies.

Simulation-facing code in this crate must produce the same result on every
platform. That means integer or fixed-point math, explicit ordering, and
algorithms whose behavior cannot change with a dependency's implementation.

## Main pieces

- `fx`, `grid`, and `compass` provide fixed-point world geometry.
- `rng` and `hash` provide frozen random streams and canonical fingerprints.
- `path` implements deterministic tile routing and line-of-travel checks.
- `replay` records a generic setup plus tick-stamped commands and bounds
  untrusted files before they enter a game-specific loader.
- `fsx` is the shared boundary for atomic, durable writes.

## Development

Run commands from the workspace root:

```sh
cargo test -p chassis --locked
cargo clippy -p chassis --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc -p chassis --no-deps --locked
```
