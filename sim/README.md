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
- `bot` turns fog-honest observations and an immutable public briefing into
  ordinary commands. Domains own evidence, proposals and tactics; shared
  allocation owns exact resources; the Executive owns command lowering. Retained
  operations and fresh work pass through one admission pipeline. See
  [Bot architecture](../docs/bot-architecture.md) for current responsibilities
  and [Bot strategy](../docs/bot-strategy.md) for the intended strategic model.
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

`bot::navigation` owns bot route queries, search storage, and cache lifetimes.
Its `commands` module projects ordinary movement and Build routes from player
knowledge; `paths` provides canonical endpoint routes and bounds;
`public_fields` provides terrain and danger-aware travel distances; `service`
retains producer and target connectivity; `egress` certifies producer exits; and
`flood` handles connectivity and placement witnesses. Barricade foothold
valuation asks for costs instead of full paths. Planners supply knowledge,
safety rules, and target preferences; navigation preserves command orientation,
path ties, and search limits. Ground, air, and hypothetical layouts retain
separate bounded caches.

Voluntary defense rejects construction kinds that cannot meet the current
allocation reserve before searching for sites. Final allocation still owns the
exact funding and compatibility decision.

## Development

Run commands from the workspace root:

```sh
cargo test -p oxide-sim --locked
cargo test -p oxide-sim --test state_integrity --locked
cargo clippy -p oxide-sim --all-targets --locked -- -D warnings
```

Bot observation is an optional callback surface in `bot::observer`. Callbacks
report paired phase boundaries, deterministic planning-work counts, and opt-in
caller-attributed query work around the ordinary controller. They do not
introduce clocks, serialized fields, or alternative planning paths; callers own
timing and persistence.
