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
- `stats` holds the unit and building tables and most balance constants.
- `observation` projects serializable player knowledge through fog and vision
  memory. It owns information access, not controller policy or navigation
  caches.
- `geometry` shares canonical command and production tie rules with predictors.
- `building_contact` owns fixed approximate building surfaces for local
  interaction.
- `vision` provides visibility and explored-world state.

Queued construction pays for one site immediately. Fogged footprints remain
provisional, nonblocking scaffolds until full visibility verifies the ground.
Invalid sites and sites abandoned by their last worker before work starts refund
in full; activation keeps the same site identity and never charges again.
Discovering any tile of an Extractor frame permits a deferred build on the whole
frame, including after sight is lost. Known claims still block the order; unseen
occupants are checked when the full footprint comes into view.

Tile goals keep the tile the player clicked, which must lie on the map. When the
issuer's team has explored it, a group spreads over the open ground around it at
once; otherwise every member heads for the tile itself and takes its spread slot
at the end of the tick its team explores the tile.

A walk whose goal cannot be reached stops on the reachable tile nearest it,
completes, and lets its program continue, reporting `OrderStalled { NoRoute }`
once. Other orders that cannot be routed drop only themselves; a refused chase,
an empty bank, or a full sling still ends the whole program.

Repair and salvage share one damage-first building-work resolver and remain
mutually exclusive. Completed Repair Bays automatically heal nearby owned units
before completed buildings, use the ordinary player bank, and skip structures
with active or queued salvage commitments.

Tile routing and most geometric tie rules are also fair under a map half-turn.
Building contact follows the fixed artwork, so asymmetric outlines can have
different frontage after a map half-turn. Fixed-point vector scaling, equal-cost
paths, group-goal snapping and spreading, footprint doorsteps, ground-production
spawns, autonomous harvest replacement, and perfectly stacked collision
separation use owner-local ranks and query-, footprint-, or map-relative frames
instead of global entity ids or an absolute screen corner. Airworks aircraft
spawn at the authoritative center of the open roof bay, then obey their ordinary
orders from there.

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
cargo test -p oxide-sim --locked --test integration state_integrity::
cargo clippy -p oxide-sim --all-targets --locked -- -D warnings
```
