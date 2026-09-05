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
- `bot` implements the normal configurable opponent and the frozen Overseer QA
  yardstick. It turns a fog-honest `Observation` plus immutable public-map
  briefing into persistent strategic plans, exact investment claims, utility
  intents, and finally ordinary player commands. One allocation session imports
  retained work and compares Foundry expansion, connected offense, defensive
  investment, economic investment, and ranked route-local standing-force
  alternatives across shared scrap, builders, sites, units, and producer timing.
  Voluntary defense values exposed assets, credible approaches, marginal
  coverage, reinforcement timing, and construction risk without controller-only
  role caps. Its accepted quote preserves the exact kind, footprint, and
  route-proven builder; Arrays use the same portfolio as sensor proposals whose
  value comes from novel usable radar coverage. Connected minimum and marginal
  contexts account for their exact live and paid queue ownership before ordinary
  demand is derived. Non-urgent standing demand may offer a capital-only wait
  for a strictly better unlocked provider beside its affordable fallback.
  Economic alternatives value finite safe harvest work, orphaned construction,
  recurring-income payback, capability prerequisites, producer throughput, and
  self-refits. Live worker output includes initial travel, and concurrent air
  operations share deadline-bound factory time. They retain exact worker lanes,
  building identities, or foundation sites and builders. Unpaid saving and
  deferred travel share one fixed deadline; paid work is not cancelled on loss
  of the opening core. All five domains compete in shared allocation, and
  forecast income never funds a command. Current-threat emergency defense
  remains exact survival work with precedence over voluntary proposals, and
  admitted island-air work advances through the same transaction. Accepted
  domain payloads keep their exact choices; compatible work may proceed
  together, while unmigrated planners and utility use only the residual
  capacity. Connected air-and-siege operations derive opportunity-scaled
  reconnaissance, suppression, direct strike, and current-visible
  non-suppression bombing value, then freeze exact members at commitment. Their
  route and queue preflight covers the complete admitted target cluster, whose
  canonical anchors are exposed in optional decision traces without entering
  controller state. See [Bot Strategy](../docs/bot-strategy.md) for the policy
  direction and [Simulation Architecture](../docs/simulation-architecture.md)
  for the current implementation contracts.
- `vision` provides visibility and explored-world state.

Repair and salvage share one damage-first building-work resolver and remain
mutually exclusive. Completed Repair Bays automatically heal nearby owned units
before completed buildings, use the ordinary player bank, and skip structures
with active or queued salvage commitments.

Outcome-relevant geometry is also fair under a map half-turn. Fixed-point vector
scaling, equal-cost paths, group-goal snapping and spreading, footprint
doorsteps, ground-production spawns, and perfectly stacked collision separation
use owner-local ranks and query-, footprint-, or map-relative frames instead of
global entity ids or an absolute screen corner. Airworks aircraft spawn at the
authoritative center of the open roof bay, then obey their ordinary orders from
there.

## Development

Run commands from the workspace root:

```sh
cargo test -p oxide-sim --locked
cargo test -p oxide-sim --test state_integrity --locked
cargo clippy -p oxide-sim --all-targets --locked -- -D warnings
```
