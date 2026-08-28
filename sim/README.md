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
- `bot` resolves difficulty, stance, and seeded personality; maintains
  fog-honest intelligence and persistent air, lift, raid, and team playbooks;
  admits new strategic work on shared 24-tick boundaries; and lowers exact
  reservations plus utility intents into ordinary commands. Air and lift
  operations remain useful alone but can coordinate when they share an
  objective, stop replacement loops after losing a dispatched scout, and
  preserve first-carrier capital only when fog-honest evidence proves it useful.
  Adaptive production fills an unreserved ordinary fighting line before
  specialties. It keeps one baseline Tender and adds more only for distinct
  reachable wounded ground combatants, while persistent operations exclusively
  own bomber and ground-attack-air cohorts. Scrapheap uses a reduced decision
  cadence; Standard, Veteran, and Prime share the competent cadence and separate
  through the remaining fair cognitive limits, including a fixed rung-specific
  strength uncertainty that personality cannot change. Prime additionally uses
  the ordinary focus-fire command to coordinate overlapping static defenses on
  one currently visible threat.
- `vision` provides visibility and explored-world state.

Outcome-relevant geometry is also fair under a map half-turn. Fixed-point vector
scaling, equal-cost paths, group-goal snapping and spreading, footprint
doorsteps, production spawns, and perfectly stacked collision separation use
owner-local ranks and query-, footprint-, or map-relative frames instead of
global entity ids or an absolute screen corner.

## Development

Run commands from the workspace root:

```sh
cargo test -p oxide-sim --locked
cargo test -p oxide-sim --test state_integrity --locked
cargo clippy -p oxide-sim --all-targets --locked -- -D warnings
```
