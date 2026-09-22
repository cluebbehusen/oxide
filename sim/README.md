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
- `observation` projects serializable player knowledge through fog and vision
  memory. It owns information access, not controller policy or navigation
  caches.
- `geometry` shares canonical command and production tie rules with predictors.
- `vision` provides visibility and explored-world state.

Queued construction pays for one site immediately. Fogged footprints remain
provisional, nonblocking scaffolds until full visibility verifies the ground.
Invalid sites and sites abandoned by their last worker before work starts refund
in full; activation keeps the same site identity and never charges again.

Repair and salvage share one damage-first building-work resolver and remain
mutually exclusive. Completed Repair Bays automatically heal nearby owned units
before completed buildings, use the ordinary player bank, and skip structures
with active or queued salvage commitments.

Outcome-relevant geometry is also fair under a map half-turn. Fixed-point vector
scaling, equal-cost paths, group-goal snapping and spreading, footprint
doorsteps, ground-production spawns, autonomous harvest replacement, and
perfectly stacked collision separation use owner-local ranks and query-,
footprint-, or map-relative frames instead of global entity ids or an absolute
screen corner. Airworks aircraft spawn at the authoritative center of the open
roof bay, then obey their ordinary orders from there.

## Development

Scenarios default to `mode: "match"`, which requires a Foundry anchor per seat
and uses Foundry-based elimination and victory. `mode: "sandbox"` permits
optional anchors, allied-only scenes and disconnected arenas, and runs without
automatic elimination or victory. It still requires at least one valid seat and
validates every placed entity. Commands keep their ordinary costs and ownership
rules; a sandbox seat can command units without a Foundry. Explicit surrender
still relinquishes that seat's command authority. Mode is part of serialized
world state and replay setup.

Run commands from the workspace root:

```sh
cargo test -p oxide-sim --locked
cargo test -p oxide-sim --test state_integrity --locked
cargo clippy -p oxide-sim --all-targets --locked -- -D warnings
```
