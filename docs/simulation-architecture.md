# Simulation architecture

This document describes the current architectural contracts of `oxide-sim`.
It maps responsibilities and invariants, not tuning or history. Training
procedures belong in the bot-training skill; balance lives in `sim/src/stats.rs`.

## Authority and reproducibility

`State` is the complete authoritative world. The public mutation boundary is
`State::tick(&[PlayerCommand])`, which advances exactly one fixed simulation
step. Camera state, input state, interpolation, effects, audio, and UI never
enter `State` and cannot affect an outcome.

A `Scenario` contains the map, seed, players, starting entities, bot setup,
and presentation metadata needed to begin a match. It is embedded in a replay,
so replay reconstruction does not depend on the original scenario file. The
scenario seed initializes the simulation RNG and deterministic bot streams.

All outcome-relevant arithmetic uses fixed point or integers. Entity tables
are kept in stable id order, random choices use `chassis::rng::Pcg32`, and
selection rules end in explicit deterministic tie-breakers. `State::hash`
serializes the authoritative state canonically; readable protocol views are
not substitutes for that fingerprint.

`TickReport` and its `Event` values are output only. Consumers may use them for
statistics, effects, animation, sound, and assertions, or drop them entirely.
The simulation never reads an event back.

## State construction and trust boundary

`Scenario::build` is the normal constructor. It parses and validates authored
data, normalizes team ids, places Foundries and starting entities, and builds
tick-zero vision.

All `State` fields are crate-private. External crates receive narrow immutable
accessors and can change the world only by supplying commands to `tick`.
`inspect_command_phase` is a deliberate exception for prediction: it clones
the state, applies only command validation and command-phase effects, exposes
a restricted read-only view to a callback, and discards the clone. It never
advances or installs authoritative state.

Deserialization uses a private mirror type and then calls
`State::validate_invariants`; there is no public unchecked deserialization
path. Validation covers, among other things:

- player, team, result, map, and vision-table consistency;
- sorted entity ids and monotonic next-id counters;
- hp, cooldown, progress, queue, coordinate, and tick envelopes;
- valid owners, faction production, entity references, and shell fields;
- coherent construction, salvage, recovery, ghost, radar, and memory state;
- canonical ordering for every collection whose order is observable.

Each new serialized field needs an invariant decision and an adversarial case
in `sim/tests/state_integrity.rs`. The same test suite also round-trips states
the real simulation produces, preventing the validator from becoming stricter
than reachable reality.

## Tick pipeline

Phase order is game behavior. `State::tick` currently performs:

1. Capture any newly stranded economy's finite recovery entitlement.
2. Validate and apply this tick's commands in their recorded order.
3. Advance production queues and spawn completed units.
4. Run unit and building behavior in deterministic order.
5. Buffer and resolve combat, construction, salvage, and repair work.
6. Evict pathless ground bodies from newly claimed building footprints.
7. Follow paths, then resolve same-domain unit collisions.
8. Remove dead entities and deposit eligible wreck salvage.
9. Apply wreck decay on its global cadence.
10. Rebuild team-shared visibility and reconcile fog memory.
11. Determine victory or draw from surviving, non-resigned teams.

Damage and work are buffered while actors decide against the same phase
snapshot. Damage resolves before construction, salvage, or repair gains, so
fire wins a same-tick tie and destroyed targets cannot be repaired back into
existence. Retaliation is derived after damage from surviving victims.

Once a result exists, later calls ignore commands and skip world phases, but
the tick counter still advances so external timelines remain aligned.
Per-tick acceleration structures, including the unit spatial index, are local
scratch. They are rebuilt at their use points and never serialized or hashed.

## Commands and unit programs

A `PlayerCommand` pairs an issuer with a `Command`; the command layer proves
ownership, command eligibility, fog legality, costs, queue capacity, target
validity, and placement. Rejection produces an event and must leave the state
unchanged. Lists of unit or building ids have set semantics: dispatch sorts
and deduplicates them even though replay bytes preserve the original payload.

Commands generally establish intent rather than moving or damaging anything
immediately. Each unit has one active `Order`, a bounded FIFO queue, and a
`looping` flag:

- an ordinary non-queued order replaces the current program;
- a queued order appends behind it;
- completing a plain program pops the next order or becomes idle;
- a patrol rotates completed legs to the back until interrupted;
- a stall or overriding command clears the abandoned program as one unit.

Movement stances are distinct contracts. `Move` walks without engaging,
`Advance` keeps moving but may take already-visible in-range primary shots,
and `AttackMove` acquires and pursues enemies along the route. Explicit
`Attack` commits to its target; idle self-acquisition and retaliation may carry
a leash back to the unit's station. `Harvest`, `Build`, `Found`, `Repair`,
`RepairUnit`, and `Salvage` are persistent work programs lowered by unit
behavior over later ticks.

## Movement and collision

Ground routes use deterministic eight-direction A* with no diagonal corner
cutting. Ground passability is open terrain with no live building footprint
or undepleted scrap node. Air movement ignores rocks, scrap, and buildings,
but Peaks own their air column and remain impassable.

A path is advisory rather than a reservation. Every ground step rechecks its
next waypoint because construction can claim ground after the path was made;
an invalid path is dropped and behavior may route again on the next tick.
When a site appears under a pathless ground body, the eviction pre-pass gives
it a real escape path while preserving its order and work progress.

Units never make tiles impassable to pathfinding. They are physical bodies,
however, and deterministic relaxation passes separate overlapping units after
path movement. Ground collides only with ground and air only with air. Moving
bodies slide around contacts, while anchored harvesting, firing, and
building-repair stances resist displacement. Terrain wins over a proposed push,
and a per-tick budget prevents dense groups from exploding outward. Iteration
direction alternates with tick parity to avoid a permanent id-order advantage.

## Economy, construction, salvage, and repair

Scrap nodes block ground until exhausted. Harvesters work a bounded zone,
carry a finite load, and deposit at a Foundry. Destroyed eligible entities
leave decaying wreck salvage; wrecks do not block movement. Reclaimer and
Foundry recovery income remain ordinary authoritative state.

An accepted immediate build pays for and places an unfinished site. Its
footprint blocks ground from that command onward and starts at partial hp;
Harvesters raise it over time. A deferred build, used to claim remembered fog,
instead installs a `Found` program. The worker walks there, then proves the
strict placement predicate with current sight before payment and placement. A
matching paid site can be joined without charging twice. Taken ground stalls
without revealing anything still hidden.

Cancelling an unfinished paid site returns value proportional to its remaining
hp. Salvaging is active dismantling of a built own structure other than a
Foundry; its cumulative refund ledger prevents rounding drift, and a salvaged
building does not count as a combat loss or create a wreck. Prepaid production
on a successfully salvaged producer is refunded.

Building repair and Harvester field welding are billed per accepted hp. Repair
Bays feed the same unit-heal resolver as field welders. Concurrent workers are
resolved against shared room, excess fully unusable paid work is refunded, and
no repair source can resurrect a unit or building destroyed by that tick's
volley.

## Combat, weapons, and terrain

Every unit or armed building reads immutable stats describing range, minimum
range, cooldown, damage, target domains, splash, indirect fire, and whether the
shot is hitscan or a real projectile. Buildings count as ground targets.
Weapons may cover ground, air, or both; sidearms are separate weapon slots and
cooldowns are stored per slot.

Targeted attacks require current team sight. Shared allied sight can spot for
a long-range weapon, but a remembered building or unidentified radar contact
cannot authorize a shot. Direct ground-to-ground fire traces terrain: rocks
provide cover, while buildings and scrap do not. Fire involving aircraft and
indirect weapons ignores ordinary rock cover. Peaks block every relevant line
and artillery arc.

Hitscan attacks buffer damage for same-tick resolution. Projectile weapons
launch a serialized `Shell` toward a fixed fire-time aim point. Predictive aim
may lead a unit's current path before launch, but a shell is unguided after it
leaves the weapon. On arrival, buildings take only a direct hit; eligible enemy
units may take splash according to the weapon's domain mask.

## Fog, memory, radar, and teams

Each seat has `visible` and `explored` grids. Visibility is rebuilt every tick
from completed allied buildings and allied units; explored ground only grows.
Rocks do not occlude vision. Teammates receive byte-identical shared sight and
memory, computed once per team and cloned to later seats.

Enemy buildings remain as last-seen ghosts until their footprint is observed
again. Scrap and wreck amounts likewise freeze at the last visible value.
Arrays add sorted, deduplicated radar contact tiles outside true sight; a
contact carries no owner, type, program, or persistent memory. Recent hostile
damage remembers only the allied impact tile for a bounded caution period, not
the attacker's identity or location.

All allegiance checks route through normalized team ids. Teammates share
vision, cannot target one another, and win or lose as a team. Resignation makes
a seat command-ineligible and removes its Foundries from victory accounting;
its remaining machines continue as autonomous remnants.

Bots live outside `State::tick`. A bot reads a state-derived observation and
emits ordinary `PlayerCommand` values, which the shell or runner records before
the simulation sees them. Neural seats use the fog-honest observation path and
deterministic quantized inference: `seat_bots` seats the embedded promoted
artifact (`sim/src/bot/ladder_weights.json`) for every configured bot seat,
resolved through the seat's named profile and Level handicaps. The Overseer
(`Brain::overseer`) is the only scripted commander — training and QA
infrastructure, never player-facing. Read-only playback runs no bot because
the recorded command stream is already the match.

## Source and test map

| Contract | Primary source | Behavioral evidence |
|---|---|---|
| State, hashing, validation, teams | `sim/src/state.rs` | `sim/tests/state_integrity.rs`, `sim/tests/determinism.rs`, `sim/tests/teams.rs` |
| Tick phase order and cleanup | `sim/src/tick/mod.rs` | `sim/tests/behavior_rules.rs`, `sim/tests/determinism.rs` |
| Command vocabulary and canonicalization | `sim/src/command.rs`, `sim/src/tick/commands.rs` | `sim/tests/command_canonicalization.rs`, `sim/tests/fuzz.rs` |
| Programs, movement, and collision | `sim/src/tick/brain.rs`, `sim/src/tick/movement.rs` | `sim/tests/behavior_movement.rs`, `sim/tests/movement_lab.rs` |
| Economy and building work | `sim/src/tick/brain/economy.rs`, `sim/src/tick/production.rs` | `sim/tests/behavior_economy.rs`, `sim/tests/behavior_construction.rs`, `sim/tests/salvage.rs`, `sim/tests/repair_unit.rs` |
| Weapons and simultaneous resolution | `sim/src/stats.rs`, `sim/src/tick/brain/combat.rs` | `sim/tests/behavior_combat.rs`, `sim/tests/combat_edges.rs`, `sim/tests/shells.rs`, `sim/tests/peaks.rs` |
| Fog, memory, and radar | `sim/src/vision.rs` | `sim/tests/behavior_rules.rs`, `sim/tests/combat_edges.rs`, `sim/tests/bastion_acquisition.rs` |
| Bot command-source boundary | `sim/src/bot/mod.rs`, `sim/src/bot/observation.rs` | `sim/tests/bot_brain.rs`, `sim/tests/neural_bot.rs` |
