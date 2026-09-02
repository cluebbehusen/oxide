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
  fog-honest intelligence alongside an immutable briefing of public authored map
  facts; maintains persistent air, lift, raid, and team playbooks; admits new
  strategic work on shared 24-tick boundaries; and lowers exact reservations
  plus utility intents into ordinary commands. The briefing's starts and
  resource amounts are pre-match priors, never current contacts or live economy
  state. Air and lift operations remain useful alone but can coordinate when
  they share an objective, stop replacement loops after losing a dispatched
  scout, and preserve first-carrier capital only when fog-honest evidence proves
  it useful. Adaptive production fills an unreserved ordinary fighting line
  before specialties. Player-facing difficulties protect opening floors of four,
  five, six, and eight Sentinel-equivalents before voluntary capital, while
  preserving the fourth Harvester and exact safe home Extractor opening. Only a
  current visible ground threat can justify an emergency Turret, and emergency
  Flak requires a current visible ground-attack aircraft. A think that starts
  short stays recovery-gated until the next observation even though same-think
  line orders count toward its projection. Once ready, capital leaves a Sentinel
  queued beyond the upcoming production phase or keeps its exact cost available
  while a ground objective remains. Unpaid remote foundations keep that cost in
  the bank until they land. An admitted team-relief credibility watch may
  persist while the gate is closed without becoming a new operation; its
  designated home screen still counts toward the core. The bot keeps one
  baseline Tender and adds more only for distinct reachable wounded ground
  combatants, while persistent operations exclusively own bomber and
  ground-attack-air cohorts. Scrapheap uses a reduced decision cadence;
  Standard, Veteran, and Prime share the competent cadence and separate through
  the remaining fair cognitive limits, including a fixed rung-specific strength
  uncertainty that personality cannot change. Prime additionally uses the
  ordinary focus-fire command to coordinate overlapping static defenses on one
  currently visible threat. Player-facing construction places every defensive
  building against credible approaches with role-specific weapon, spotting,
  trigger, or path-disruption geometry and predicts builders against public
  terrain plus observed dynamic blockers. Its Harvest recovery promotes only
  matching worker damage to durable quarantine, preserves the union of
  overlapping incidents, then clears each exact region through a bounded
  current-sight sweep. A recalled recovery scout stays reserved until it is
  observed safely home, where its retry cooldown begins. Arrays use a separate
  sensor-site scorer that preserves useful in-map and nonredundant radar
  coverage, faces equally useful sites toward fog-honest hostile evidence,
  preserves active resource access, and binds a builder whose route is proven
  against public terrain. Player-facing Foundry expansion has no count ceiling:
  it ranks every exact legal site by bounded post-build Extractor, drip, and
  route-safe visible hauling value, then prices candidate-specific security
  outside uncommitted wealth before committing one unpaid claim. An accepted
  unpaid plan persists its exact site, builder, and fund across decisions. Its
  builder lease prevents unrelated work from consuming that worker until the
  matching build command is emitted, the plan becomes invalid, survival takes
  priority, or bounded recovery expires. Greed changes the forecast and
  justified preparation, not which actions the bot may take. Each player-facing
  utility pass builds one fog-honest resource snapshot that keeps current scrap
  separate from conservative completed-income forecasts and records exact active
  or queued builder obligations and completed producer queues. Queued order
  contents remain private; the policy sees only which own units already have a
  continuing program. A deterministic commitment ledger imports existing
  strategic and deferred work, then owns current-bank, unit, builder, site, and
  producer claims; unmigrated channels retain their existing order through
  explicit legacy claims. An operation accepted before a saved Foundry plan
  retains its earlier bank priority, while later operations may spend only the
  excess. The profile-free Overseer retains its frozen legacy expansion policy.
  The player-facing controller can optionally emit one compact, fog-honest
  decision trace on each think tick; its resource section reports current scrap
  separately from bounded forecast income plus current builder and producer
  capacity. Traces are output-only diagnostics and never enter controller
  memory, state, commands, or replays. The frozen Overseer emits no decision
  trace.
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
