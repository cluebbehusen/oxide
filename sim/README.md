# oxide-sim

`oxide-sim` contains all Oxide game rules. It is a pure, headless simulation:
given the same scenario and command log, it must produce the same state on every
platform. Rendering, hardware input, wall-clock time, and presentation state
belong elsewhere.

`State::tick(&[PlayerCommand])` is the only game-state transition. Humans, bots,
replays, and debug clients all enter through the same command types, so a bot is
an ordinary command source rather than a separate ruleset.

## Main pieces

- `State` owns the complete serializable world and validates its invariants.
- `Scenario` builds the initial state from authored map and player data.
- `command` and `event` define the simulation's input and output vocabulary.
- `tick` implements the fixed phase order for commands, production, movement,
  combat, cleanup, and victory.
- `stats` is the single home for units, buildings, and balance constants.
- `bot` contains observation, policy, and command-lowering code for automated
  players.
- `vision` provides visibility and explored-world state.

## Development

Run commands from the workspace root:

```sh
cargo test -p oxide-sim --locked
cargo test -p oxide-sim --test state_integrity --locked
cargo clippy -p oxide-sim --all-targets --locked -- -D warnings
```
