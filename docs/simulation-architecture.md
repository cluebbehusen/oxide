# Simulation architecture

This document describes the current architectural contracts of `oxide-sim`. It
maps responsibilities and invariants, not tuning or history. Scripted-bot
procedures belong in the scripted-bot skill; balance lives in
`sim/src/stats.rs`.

## Authority and reproducibility

`State` is the complete authoritative world. The public mutation boundary is
`State::tick(&[PlayerCommand])`, which advances exactly one fixed simulation
step. Camera state, input state, interpolation, effects, audio, and UI never
enter `State` and cannot affect an outcome.

A `Scenario` contains the map, seed, players, starting entities, and bot setup
needed to begin a match. It may also carry browser metadata that the simulation
deliberately ignores. The complete scenario is embedded in a replay, so
reconstruction does not depend on the original scenario file. Its seed
initializes the simulation RNG. Each player-facing bot seat carries a separate
personality seed in its configuration; `Brain::scripted` derives its profile
from that seed, not the scenario seed.

All outcome-relevant arithmetic uses fixed point or integers. Fixed-point vector
scaling computes unsigned magnitudes before restoring the sign, preserving exact
negation for representable results so opposite movement rays cannot drift by one
raw unit. Entity tables are kept in stable id order, random choices use
`chassis::rng::Pcg32`, and selection rules end in explicit deterministic
tie-breakers. `State::hash` serializes the authoritative state canonically;
readable protocol views are not substitutes for that fingerprint.

`TickReport` and its `Event` values are output only. Consumers may use them for
statistics, effects, animation, sound, and assertions, or drop them entirely.
The simulation never reads an event back.

Player-facing bot decision traces have the same one-way boundary. An opt-in
`Brain::act_traced` call reports only facts already owned by the fog-honest
coordinator while returning the same ordinary commands as `Brain::act`. The
trace recorder is local to that call; traces are not controller memory,
authoritative state, replay input, or replay metadata. Ticks on which no
player-facing decision occurs produce no trace. Trace schema version 12 reports
current scrap separately from a bounded forecast based only on completed income
sources, together with current builder and producer capacity. Proposal and
allocation evidence records the coordinator's actual inputs and verdicts,
including economic action and defensive proposal identities, exact building
claims, exact repair ownership, refit income losses, and arbitrary-size layout
conflicts, rather than reconstructing decisions after the fact. Battlefield
evidence, Executive mission ownership, bounded episode reports, decayed
preferences, and effective allocation return are separate trace fields. Raw
proposal consequence, urgency, confidence, and safety are not rewritten to
express historical preferences.

### Controller-local battlefield loop

Playable army contact includes observed completed defenses with compatible
weapon range and known fire geometry. Local strength counts nearby participants
and the defenses actually covering them. An army returns after a newly observed
casualty if neither the previous nor current observation showed contact. It
records an inconclusive episode without inventing a hidden attacker. Visible
objective completion takes precedence; earlier combat casualties do not trigger
a later return. Withdrawal from static fire also uses ordinary movement to avoid
reacquiring the position. Pressure admission counts known gun coverage along a
projected approach as well as defenders near the objective. Against a building,
an escorted siege body seeks a reachable position inside its guns' range and its
screen's sight. It prefers positions outside known defensive fire; if the
defender matches its range, an already-admitted assault accepts exposure instead
of waiting forever for range superiority. Its faster screen advances at most
three route steps ahead of the rearmost gun, then waits for it before
approaching the shared firing position. Artillery with a visible target in range
remains engaged rather than being discarded as a stalled march.

Player-facing maintenance advances tactical armies first. The decision then
observes battlefield evidence and work outcomes once, including on the early
economy-recovery path, before preparing new investment alternatives. Spatial
contact bins distinguish physical and target domains. Movement history stores
two actual observations and their ticks, never an extrapolated position. Radar
contributes unresolved regions rather than identified units. Reachable local
service is credited once when describing uncovered asset pressure.

The immutable assessment feeds reconnaissance, protective support, Array
coverage, and general ground-mission selection. Fresh deployments are finalized
after allocation, against its exact reservations. Army responsibilities belong
to the Executive's existing `ArmyId`; they do not create another unit lease in
the allocator. Exact formation and reorganization validate the complete change
before modifying either body. Staging splits retain coherent groups and home
strength. Engaged or withdrawing bodies are not split. Ordinary movement handles
recovery so tactical reacquisition cannot restart an abandoned chase; observed
return explicitly reopens reserve reinforcement. Tactical emergency withdrawal
has precedence over a mission directive and replaces the abandoned mission with
recovery after recording its outcome. An accepted defense assignment dispatches
its exact body even when proximity has already marked it engaged. Lowering
receipts expose actual acceptance and refusal boundaries independently of
command counts.

Pressure objectives bind the observed owner, kind, and complete footprint, plus
a live id only when current sight supplied one. Remembered placeholders are not
entity identities: reacquiring the same site preserves its mission and fixed
deadline, while another remembered site cannot keep it alive. Objective anchors
use footprint orientation independently of movement goals. Outcome watches
resolve current identity at that exact site before measuring observed damage.

Operation and work owners submit bounded episode reports to `experience`.
Ground, air, transport, raid, relief, reconnaissance, repair, harvest, and
construction reporting use observed progress and owner-only presence. Carried
units remain present but unavailable. Foundation observation continues after the
builder leaves; a delivery watch continues without ownership of its landed
troops. Shared coordination credits prevent these components from teaching the
same outcome repeatedly. Stronger assault evidence may replace preliminary
delivery credit. Ambiguous attribution cannot earn broad doctrine credit.
Inconclusive lost-contact or deadline reports with known own casualties retain
half-strength contextual evidence and can request approach reconnaissance. They
remain inconclusive about the objective and never contribute to doctrine. Frozen
objective owner, kind, and footprint anchor link failed approaches to remembered
buildings without relying on their placeholder ids. Ground reports entering
policy memory orient both their context point and frozen objective footprint, so
later observation matching uses one coordinate frame. Defensive service uses a
provider's movement domain for routing, including parked aircraft; target
exposure still uses its current body domain.

Contextual return and corroborated doctrine preferences decay toward neutral;
each contextual contribution keeps its original completion time, including when
new evidence arrives for the same context. Replacing shared credit removes that
credit alone. Storage retains at most 64 contributions per context, 128
contexts, and 64 recent episode reports, with canonical oldest-first eviction.
These preferences alter candidate ranking and effective allocation return
without rewriting raw consequence, urgency, confidence, or safety. Retry records
for dispatched harvest and construction attempts expire and require fresh legal
preparation. Current footprint occupation invalidates a construction attempt
without a route penalty; remembered buildings alone cannot establish that
occupation. Work observation also indexes active builders' occupied tiles once
per decision. Fresh blocking foundations cannot displace those workers from
their current work tiles; movement and completion release this protection
without changing ordinary terrain routing. Contested-harvest quarantine retains
its separate complete-sweep and safe-return requirements. These components are
reconstructed by replaying the observed command prefix, not serialized into
authoritative `State`.

An unpaid Foundry's recovery interval spans both funding and execution blockage.
Restored funding permits another readiness check; only a ready builder and site
clear the interval. Continuous execution blockage therefore releases the unpaid
claim after the same bounded recovery period as continuous funding failure.

## State construction and trust boundary

`Scenario::build` is the normal constructor. It parses and validates authored
data, normalizes team ids, places Foundries and starting entities, and builds
tick-zero vision.

All `State` fields are crate-private. External crates receive narrow immutable
accessors and can change the world only by supplying commands to `tick`.
`inspect_command_phase` is a deliberate exception for prediction: it clones the
state, applies only command validation and command-phase effects, exposes a
restricted read-only view to a callback, and discards the clone. It never
advances or installs authoritative state.

Deserialization uses a private mirror type and then calls
`State::validate_invariants`; there is no public unchecked deserialization path.
Validation covers, among other things:

- player, team, result, map, and vision-table consistency;
- sorted entity ids and monotonic next-id counters;
- hp, cooldown, progress, queue, coordinate, and tick envelopes;
- valid owners, faction production, entity references, and shell fields;
- coherent construction, salvage, recovery, ghost, radar, and memory state;
- canonical ordering for every collection whose order is observable.

Each new serialized field needs an invariant decision and an adversarial case in
`sim/tests/state_integrity.rs`. The same test suite also round-trips states the
real simulation produces, preventing the validator from becoming stricter than
reachable reality.

## Tick pipeline

Phase order is game behavior. `State::tick` currently performs:

1. Capture any newly stranded economy's finite recovery entitlement and resolve
   provisional sites whose full footprints are visible.
2. Validate and apply this tick's commands in their recorded order, refunding
   unstarted sites after their last worker commitment is replaced.
3. Apply recurring income, advance production queues, and spawn completed units.
4. Cancel unstarted construction over known mines, then decay unclaimed
   tier-zero construction sites on their global cadence.
5. Run unit brains and building behavior, land arriving shells, and resolve
   buffered damage, construction, salvage, and repair.
6. Resolve boarding and unloading after every unit has decided.
7. Evict pathless ground bodies from newly claimed blocking footprints.
8. Follow paths, then resolve same-domain unit collisions.
9. Retain large aircraft motion and resolve due crash impacts against current
   positions.
10. Detonate armed Scuttle Charges under hostile post-movement bodies.
11. Schedule airborne crashes, remove dead entities, deposit eligible wreck
    salvage, and refund unstarted sites with no surviving worker commitments.
12. Apply wreck decay on its global cadence.
13. Rebuild team-shared visibility and reconcile fog memory. Activate or refund
    newly visible provisional sites. Newly discovered mines cancel unstarted
    sites before the next command.
14. Determine victory or draw from surviving, non-resigned teams and discard any
    remaining pending crashes when the match ends.

Shots and hp work are buffered while actors decide against stable positions, hp,
and live entity tables. Orders, paths, harvesting, and billing may still change
in their declared deterministic order; the whole brain phase is not an immutable
snapshot. Damage resolves before construction, salvage, or repair work, so fire
wins a same-tick tie and nothing can repair a destroyed target back into
existence. Retaliation is derived afterward from surviving victims.

Once a result exists, later calls ignore commands and skip world phases, but the
tick counter still advances so external timelines remain aligned. Per-tick
acceleration structures, including the unit spatial index, are local scratch.
They are rebuilt at their use points and never serialized or hashed.

Destroyed airborne Condors, Moths, and Skyhooks leave a pending crash. The
record retains owner, kind, heading, launch and contact positions, and start and
arrival ticks. Contact is fixed at death using the last actual airborne
displacement, capped at flight speed, with 20% average deceleration over 13
ticks. A hovering transport therefore falls in place. The removed aircraft and
its cargo cease acting immediately, and ordinary death salvage is unchanged.

At contact, a two-tile blast damages every hostile ground unit and nearby
building footprint: 50 damage for Condor, 40 for Moth and Skyhook. Allies and
airborne units are immune; impacts over pits do no damage. Targets can move into
or out of the blast before arrival. Pending crashes resolve in death-tick and
unit-id order and survive state serialization. Victory is immediate once the
Foundry condition is met; any crashes still pending are discarded without
damage.

## Commands and unit programs

A `PlayerCommand` pairs an issuer with a `Command`; the command layer proves
ownership, command eligibility, fog legality, costs, queue capacity, target
validity, and placement. Rejection produces an event and must leave the state
unchanged. Lists of unit or building ids have set semantics: dispatch sorts and
deduplicates them even though replay bytes preserve the original payload.

Commands generally establish intent rather than moving or damaging anything
immediately. Each unit has one active `Order`, a bounded FIFO queue, and a
`looping` flag:

- an ordinary non-queued order replaces the current program;
- a queued order appends behind it;
- completing a plain program pops the next order or becomes idle;
- a patrol rotates completed legs to the back until interrupted;
- a stall or overriding command clears the abandoned program as one unit.

Movement stances are distinct contracts. `Move` walks without engaging,
`Advance` keeps moving but may take already-visible in-range primary shots, and
`AttackMove` acquires and pursues enemies along the route. Explicit `Attack`
commits to its target; idle self-acquisition and retaliation may carry a leash
back to the unit's station. `Harvest`, `Build`, `Found`, `Repair`, `RepairUnit`,
and `Salvage` are persistent work programs lowered by unit behavior over later
ticks.

## Movement and collision

Ground routes use deterministic eight-direction A* with no diagonal corner
cutting. Equal-cost open-set ties use a query-oriented tile rank: the rank
reverses with the start-to-goal query under a map half-turn, so a rotated query
returns the rotated canonical path instead of inheriting an absolute row-major
preference. Ground passability is open terrain with no undepleted scrap node or
non-stealthy building footprint. A buried Scuttle Charge deliberately blocks
nothing. Air movement ignores rocks, scrap, and buildings, but Peaks own their
air column and remain impassable.

Talon, Darter, Shrike, Sylph, Kestrel, and Gnat cruise heading-first but can
hover at rest. Their travel and fixed-gun traverse rates are independent of
bomber flight: eight compass steps per tick for Talon, six for Shrike, ten for
Darter, Sylph, and Kestrel, and twelve for Gnat. At full speed these give turn
radii of roughly 0.7 to 1.1 tiles. Near waypoints they slow to tighten the arc;
intermediate waypoints can be rounded only when the onward segment is clear. An
obstructed step holds position while the nose turns and replans from the actual
position. Arrivals, Stop, and in-range attacks hover rather than orbit or land.
Fixed guns traverse with the body before firing ordinary hitscan shots; Advance
only fires when already aligned and does not turn away from its route to aim.
These aircraft spawn facing the map center, matching mirrored initial turn
costs. Buzzard, Wisp, and Skyhook retain independent travel without a cruise
turn radius.

Condor and Moth use committed heading-first flight: only the heading steers, at
most `turn_rate` compass steps per tick, so every waypoint is accepted inside
the kind's turn-acceptance ring rather than at an exact center. Every turn is a
committed arc of one fixed radius, and the simulation reasons about that arc
against the map's flight envelope in fixed point. Steering takes the shorter
rotation only when the arc it sweeps stays inside the world and ends in a state
the airframe can still be flown out of, otherwise the longer one. A wall reflex
banks the aircraft away whenever one more straight tick would leave no such arc.
A committed airframe never stops: without a path it orbits on the bank whose
fitting arc is longest, tangent to the point where the route ran out, and if it
is ever pressed into the envelope it slides along the boundary while turning
back in. A step into a Peak drops the route so the brain replans from the actual
position, while the airframe slides along the face. Near the target bearing, a
three-quarter-step angular deadband prevents alternating corrections across the
256-step compass boundary. Turns settle on the nearer of the two bearings
bracketing the goal ray.

A bomber's roll-out after a release, and its departure leg when it is inside
release range or inside its own acceptance ring of the attack tile, go only to
goals it can still be flown out of on arrival, bending progressively further
when a wall or corner closes the line ahead. When the straight approach to an
attack tile would reach the acceptance ring in an unrecoverable state, the run
is planned through an initial point so the final leg runs parallel to a wall: a
corner target is attacked along one of its walls rather than by a dive the turn
radius cannot recover from. Bombs fall on the targeted building's center rather
than on the footprint edge point that range is measured to, so a corner shared
with a neighbouring footprint cannot hand the hit to the neighbour.

Turn-limited aircraft land on any ordinary ground tile, and there is no landing
command: a flier's ground destination is a landing. A move, attack-move, or
advance with nothing queued behind it and no patrol loop hands over to an
internal `Land` order once the airframe is within `LANDING_HANDOFF_REACH` of its
goal and nothing is in acquisition range, snapping to the nearest clear landable
tile within `GOAL_SNAP_RADIUS`; with an enemy in reach it keeps the ordinary
arrival contract and fights as an idle unit would. A tile is landable only when
some run-in bearing exists whose parked heading the airframe could fly out of
again, either by a half turn or by straight flight into open ground; corner
tiles therefore land only with the nose toward the field. The `Land` order flies
straight in on whatever bearing the tile lies whenever the nose can settle onto
that line before reaching it; otherwise it flies a run-in entered from a fix
twice as far out as the initial point on the same bearing, so the leg is joined
lined up rather than from whatever heading reached it, and then chases a carrot
on the run-in centerline two turn radii ahead of its own projection. Both fixes
must sit strictly inside the flight envelope on a heading the airframe can fly
out of, and the whole line must be open sky; a tile with no such line is not
landable. Acquisition is judged from the landing tile, not from the airframe on
its way there, so a retreat past a gun still completes; judged from anywhere but
the unit's own position it is gated on what the player currently sees, since the
tile can lie beyond the airframe's own eyes. A touchdown also needs clearance:
no other ground body on the tile or within the two bodies' combined radius of
the resting point, because parked bodies are immovable and such an overlap would
never resolve. It touches down when it passes within `LANDING_TOUCHDOWN` of the
tile center and rests where it met the tile, keeping its heading; the `landed`
flag makes it a ground body for targeting, collision, buried charges, and
footprints while it never moves. A tile that fills during the approach sends the
landing around to the nearest clear tile; an overflown tile costs a fresh
run-in. An idle aircraft lands itself after `AUTO_LAND_IDLE_TICKS` of orbit, and
both it and a go-around prefer a tile they can fly straight in to over the
nearest one, so a self-chosen pad rarely costs a procedure turn; run-in
distances are tried nearest first. Any program other than idling or landing
where it rests lifts a landed airframe off at its next brain tick, and takeoff
is ordinary heading-first flight from the parked heading; a landed body evicted
from a claimed footprint lifts off along its escape route. The validator refuses
a landed non-aircraft, a landed body beyond touchdown reach of its tile center
or holding a path, a parked heading it could not fly out of, and a rest on
terrain no ground body can stand on (terrain only: a friendly site may claim the
tile between ticks and eviction resolves it on the next), and two parked bodies
inside their combined radius. It deliberately allows a parked airframe over live
scrap: a flyer downed over the tile deposits wreck salvage there, so that state
is reachable in play even though a landing never starts on scrap. A parked
airframe is a legal weld patient; the torch ends when it lifts off.

A path is advisory rather than a reservation. Every ground step rechecks its
next waypoint because construction can claim ground after the path was made; an
invalid path is dropped and behavior may route again on the next tick. When a
site appears under a pathless ground body, the eviction pre-pass gives it a real
escape path while preserving its order and work progress. Its scan frame
reverses under a map half-turn, with body heading breaking an exact map-center
tie.

If a newly accepted foundation leaves a body without an escape route, make-way
relocates it to a passable perimeter tile. That ring is ordered in the founder's
approach frame, so rotating the map rotates the fallback destinations too.
Relocation happens only after acceptance and payment.

Approaching a footprint orders passable doorsteps in the body's local approach
frame, then uses an owner-local unit rank to spread equivalent workers.
Ground-production orders spawn doorsteps in the producer's radial frame around
the map. In both cases, dot and cross products replace an absolute scan
direction, so half-turned producers and workers receive corresponding geometric
orderings. Airworks aircraft instead spawn at the authoritative center of the
open roof bay and follow ordinary idle, rally, or player-issued orders from
there.

Autonomous harvest replacement preserves worker distance, safe route length,
source amount, anchor distance and source kind as its economic priorities. Exact
ties use coordinates oriented by the worker's approach to the work-zone anchor.
A worker standing on that anchor uses its hull bearing instead, so mirrored
workers choose mirrored sources without depending on seat or unit ids.

Group `Move`, `Advance`, and `AttackMove` commands likewise resolve a blocked
center and spread per-unit destinations in the approaching body's half-turn
frame. The same orientation governs both decisions: mirroring a group, its
requested center, and the map therefore mirrors every lowered unit goal even
when the requested tile is occupied.

Units never make tiles impassable to pathfinding. They are physical bodies,
however, and deterministic relaxation passes separate overlapping units after
path movement. Ground collides only with ground and air only with air, and
turn-limited aircraft take part in no collision at all: a committed arc that
steering has already checked against the world cannot be shoved off it. Moving
bodies slide around contacts, while anchored harvesting, firing, and
building-repair stances resist displacement. Terrain wins over a proposed push,
and a per-tick budget prevents dense groups from exploding outward. Iteration
direction alternates with tick parity to avoid a permanent id-order advantage.
When bodies are perfectly stacked and geometry provides no separating vector,
the deterministic owner-local-rank direction is rotated into the stack's
map-relative half-turn frame.

## Economy, construction, salvage, and repair

Scrap nodes block ground until exhausted. Harvesters work a bounded zone, carry
a finite load, and deposit at a Foundry. Gathering and unloading require the
worker's center to be within 0.75 tiles of the footprint edge, including
diagonal doorsteps; merely entering a neighboring tile does not start work.

`ReturnCargo` replaces loaded workers' active and queued work with a delivery.
The command selects an owned, living, completed Foundry before replacing each
worker's program. Automatic selection tries safe, team-known routes in squared
center-distance order, with building-id ties. Each worker reuses fully exhausted
reachability scans across destinations within the same safety pass. If only
dangerous routes exist, it selects the nearest reachable destination and waits
for safe passage. Delivery uses the ordinary danger-aware routes, physical
unloading reach, bank credit, and recovery accounting. An explicit Foundry
remains the destination; its destruction or loss of access stalls the order
without losing the load. A failed request preserves the prior program. Delivery
ends idle unless the player has since queued new work. A Foundry-click delivery
may continue into ordinary paid repair; a patient healed in transit still
receives its cargo.

Destroyed eligible entities leave decaying wreck salvage; wrecks do not block
movement. Recurring economy runs in the production phase: Reclaimers and
Refineries pay on their cadences, restored Extractors provide fixed remote
income, and a completed same-owner Foundry within the support radius raises an
Extractor's fixed yield without stacking. Completed Foundries also provide the
baseline drip and a finite recovery entitlement for a stranded seat. Crucibles
consume nearby wreck salvage for income. These are ordinary authoritative rules,
not shell conveniences.

Extractor frames are immutable authored map features. Only an Extractor may
claim one, other foundations cannot cover one, and destroying an Extractor
reveals the same frame for another claim. Support is computed directly from the
two completed building footprints, so it adds no serialized connection or hidden
ownership state.

An accepted immediate build pays for and places an unfinished site at partial
hp. A non-stealthy footprint blocks ground from that command onward; the buried
Scuttle Charge is the deliberate exception. Harvesters raise the site over time.
Wreck salvage beneath a site remains, with ordinary decay, until the first crew
work clears its footprint. Cancelling before that work preserves the salvage. A
deferred build pays for one provisional scaffold and installs a `Found` program
for its workers. Provisional scaffolds provide no vision, physical occupancy,
damage target, or construction progress. Once the owner's team sees the entire
footprint, the simulation checks placement against that knowledge: a blocker
cancels the scaffold with a full refund; clear ground activates its occupancy
and converts every matching worker commitment to `Build` using the same building
id, without another charge. Shared crews pay once per site. Hidden occupancy
cannot change command acceptance, payment, or the preview. Provisional Foundries
do not count toward survival or the bot's home selection.

Completed enemy Scuttle Charges remain concealed without detector coverage;
unfinished charges are visible under ordinary sight. A witnessed charge retains
its last-seen marker after concealment, even on visible ground, without
revealing its current hp or completion. Detection, observed removal, or a
visible replacement covering the mine's tile from its own team clears that
memory. Known live or remembered charges block placement. An undiscovered charge
allows the same immediate or deferred order as empty ground; an unstarted paid
site can temporarily overlap it. Discovery cancels that site's active and queued
construction commitments and refunds its full price. Provisional scaffolds
receive the same full refund. Unrelated queued orders survive cancellation.

The first actual crew work over an undiscovered armed charge triggers its blast
before construction hp or completion resolves. The new site is destroyed; nearby
hostile ground units and charges take the ordinary mine damage. Ground movement
retains its existing post-movement trigger. Construction triggers resolve in
mine-id order after the volley; a charge destroyed by the volley or an earlier
blast does not fire. Multiple workers cannot multiply one detonation. An
artillery impact on an overlapping scaffold hits the scaffold directly; the
buried charge remains vulnerable to the shell's ordinary splash damage.

An unstarted tier-zero site is cancelled with a full refund when its final
living worker loses its active or queued commitment, including replacement,
Stop, failed travel, boarding, or death. Replacement construction can use these
refunds atomically: rejection preserves the old sites and programs. Once work
has started, an abandoned site retains the existing decay and health-based
refund rules. Provisional scaffolds never decay. An upgrade pays up front and
takes a completed building offline as a committed site on its new tier. The
building refits itself at one progress tick per simulation tick: it cannot be
accelerated by workers, paused, cancelled, or abandoned to decay. Its hp gain
and completion use the shared damage-first work resolver, so lethal fire wins a
completion-tick tie and nonlethal damage remains when it returns to service.

Cancelling before the first build tick returns the full price. Cancelling an
unfinished site after work starts returns value proportional to its remaining
hp. Salvaging is active dismantling of a built own structure other than a
Foundry; its cumulative refund ledger prevents rounding drift, and a salvaged
building does not count as a combat loss or create a wreck. Prepaid production
on a successfully salvaged producer is refunded.

Building repair and Harvester field welding are billed per accepted hp. Repair
Bays feed the same unit-heal resolver as field welders and the same
building-work resolver as repair crews. Their aura heals own wounded units and
completed structures in range, but a Bay does not heal itself; overlapping Bays
may repair one another. Units retain first claim on limited aura scrap. A
structure with an active or queued salvage commitment receives no automatic
repair, preserving the command layer's mutual exclusion between repair and
teardown. Concurrent sources are resolved against shared room, excess fully
unusable paid work is refunded, and no repair source can resurrect a unit or
building destroyed by that tick's volley.

## Combat, weapons, and terrain

Every unit or armed building reads immutable stats describing range, minimum
range, cooldown, damage, target domains, splash, indirect fire, and whether the
shot is hitscan or a real projectile. Buildings count as ground targets. Weapons
may cover ground, air, or both; sidearms are separate weapon slots and cooldowns
are stored per slot.

Ground chassis retain a motor speed independently of collision displacement.
They accelerate from rest over six ticks and brake from full speed over three. A
sharp route change first brakes along the existing heading, then pivots; final
approaches reduce speed to stop at the goal. Stop and lost paths brake without
retaining the old order. Newly blocked terrain can arrest that coast. Turn rate
remains the ceiling of movement speed times 64, bounded to four through ten
compass steps per tick; Breaker retains four, and Avalanche and Bombard retain
three. Translation resumes within eight of 256 compass steps. Ground units spawn
facing the map center so mirrored placements have mirrored initial turn costs.
Independent weapon mounts can aim during travel; fixed weapons wait for the
motor to stop before turning to aim.

A pathless ground unit can still be braking. Group arrival propagation and
anchored collision priority require its motor speed to be zero.

Paths and destination allocation remain advisory. The ordinary collision
relaxation still separates bodies laterally after propulsion, with its original
per-tick budgets and alternating order. No future journey, service timetable or
contact-steering coordinator controls ground travel. Motor speed is serialized
and validated; observational motion reports split propulsion from collision
correction without affecting state or hashes.

Collision corrections require every tile touching the proposed position to be
passable in the body's domain. An exact grid edge touches two tiles and a corner
touches four; neither face of a blocking tile admits an exact-edge correction.
This contact rule is separate from ordinary `TilePos::containing` lookup and
retains the same displacement budget and deterministic pair order.

Ground weapons require alignment within two compass steps before firing.
Sentinel, Warden, and Lancer have independent serialized `turret_heading`
bearings, traversing eight, five, and six steps per tick respectively while the
hull follows its route. An absent bearing initially follows the hull; other unit
kinds cannot deserialize this field. Fixed ground weapons aim with the chassis.
During Advance they only take already-aligned opportunistic shots; independent
mounts can traverse while advancing. Ground sidearms share the current weapon
bearing and cannot fire off-axis. Workers turn toward their stationary work
target without delaying work progress.

Buzzard uses its compass heading for an independent turret, while its air
movement remains unrestricted by heading. The turret traverses six compass steps
per tick and shares the two-step firing tolerance. It tracks visible, shootable
targets during reload. An advance keeps its route while traversing toward its
ordinary opportunistic target; cooldown starts only once the turret aligns.
Hidden structures cannot attract an advancing weapon or reveal themselves
through turret tracking.

Bombard keeps serialized `brace_ticks` from zero to twelve. It turns with the
spades stowed, then spends twelve aligned ticks deploying; its heading stays
fixed inside the firing tolerance while planted. After a shot, eight ticks of
recoil protection precede retraction at three deployment ticks per tick. A new
aim, lost firing solution, or movement order retracts the spades before further
turning or translation. Reloading at an unchanged firing stance keeps them
planted. Advance does not fire Bombard potshots. Deserialization bounds the
deployment counter, rejects it on other kinds and requires transported riders to
have stowed spades.

`AttackTarget` resolves through team knowledge to a visible entity, remembered
building footprint, or mobile contact. Explicit attacks approach weapon range
without requiring sight. Building attacks end when observation clears the
memory; contact attacks end when sight and radar are both lost, or
identification reveals an incompatible domain. Contact loss also clears defense
focus and advances queued orders.

Automatic acquisition prefers visible eligible enemies over radar contacts and
does not acquire building ghosts. Automatic radar fire may stop Idle,
AttackMove, or Patrol to aim, but cannot pursue, retreat, or start a bomber run.
Move and Advance retain their existing movement and firing rules. A fireable
explicit target takes priority. Defenses retain blocked or out-of-range focus
while firing at fallback targets; Stop clears focus.

Ground-capable defenses automatically acquire visible hostile buildings when no
eligible visible unit is in firing range with a clear shot. Building acquisition
uses the shared apparency gate, so concealed Scuttle Charges require detection.
Candidates rank by distance to the closest footprint point, then building id;
weapon range, minimum range, and terrain cover still apply. Air-only defenses
never acquire buildings.

Direct ground-to-ground fire traces terrain: rocks provide cover, while
buildings and scrap do not. Fire involving aircraft and indirect weapons ignores
ordinary rock cover. For anonymous contacts, ground-capable direct fire uses
ground cover and air-only fire uses air rules. Peaks block both. Blind hitscan
spends a shot and selects one eligible hostile in the reported tile by distance
to its center, then id. Remembered-building hitscan resolves against a footprint
at the aim point. Both retain normal splash rules and report firing coordinates
without a victim id, including on misses.

Hitscan attacks buffer damage for same-tick resolution. Projectile weapons
launch a serialized `Shell` toward a fixed fire-time aim point. Predictive aim
samples ground motor speed and heading, including pathless coasting; air units
retain the current steering-line estimate. The snapshot precedes unit brains and
does not consult later route turns. A shell is unguided after it leaves the
weapon. Radar aim uses a fixed-point velocity estimated from the rolling
one-second tile-center history, clamped to a global movement bound; a fresh
contact has zero estimated velocity. A serialized projectile kind preserves
missile, bomb, or shell identity independently of shooter survival.
Deserialization rejects a kind inconsistent with a shooter that still exists. On
arrival, buildings take only a direct hit; eligible enemy units may take splash
according to the weapon's domain mask.

## Fog, memory, radar, and teams

Each seat has `visible` and `explored` grids. Visibility is rebuilt every tick
from completed allied buildings and allied units; explored ground only grows.
Rocks do not occlude vision. Teammates receive byte-identical shared sight and
memory, computed once per team and cloned to later seats.

The bot `Observation` copies both masks in canonical row-major order. Policies
therefore distinguish current sight from remembered terrain without consulting
authoritative state; seat orientation transforms both masks with the rest of the
observed world. Observation schema 19 distinguishes explored pits from
fire-blocking rock and peaks, marks provisional footprints and paid deferred
sites, and exposes continuous contact tracks and each own carried unit's
identity, kind, health, and carrier separately from available units. This is
presence evidence, not permission to assign or command a passenger. Allied and
enemy manifests remain opaque.

The maintained player-facing controller also receives a `PublicMapBriefing`
derived from the final authored `Scenario`. It contains static terrain,
Extractor frames, initial scrap locations and amounts, teams, and each seat's
starting Foundry anchor. These are the same facts available through the
pre-match map and roster: a starting anchor is a reconnaissance prior rather
than a current enemy contact, and an initial resource amount says nothing about
later depletion. The briefing is immutable, stays separate from
`StrategicIntelligence`, and is transformed once into the same latched seat
orientation as the dynamic observation.

Enemy buildings remain as last-seen ghosts until their footprint is observed
again. Scrap and wreck amounts likewise freeze at the last visible value. Arrays
add sorted, deduplicated radar contact tiles outside true sight; a contact
carries no owner, type, domain, or concealed entity id. Team-shared tracks
retain a contact id and one second of reported positions. Spatial buckets bound
one-to-one matching, ranked by predicted distance, previous distance, contact
id, and coordinates. Visible identity can link visible observations; radar
matching uses only reported movement. Unmatched tracks end immediately, and ids
are never reused.

Salvage-relevant hostile incidents, such as a Harvester hit or an allied loss,
remember only the victim's tile for a bounded caution period, never the
attacker's identity or location. A worker already inside a remembered static
firing envelope may retreat laterally or outward, without approaching any
overlapping gun. This escape rule never makes the source eligible for work and
does not permit crossing mobile or radar pressure.

All allegiance checks route through normalized team ids. Teammates share vision,
cannot target one another, and win or lose as a team. Resignation makes a seat
command-ineligible and removes its Foundries from victory accounting; its
remaining machines continue as autonomous remnants.

Bots live outside `State::tick`. A bot reads a state-derived observation and
emits ordinary `PlayerCommand` values, which the shell or runner records before
the simulation sees them. A configured seat carries one strict `BotConfig` with
a difficulty, stance, and personality seed. `seat_bots` passes that exact setup
and one shared immutable scenario briefing to the fog-honest `Brain::scripted`
controller. Player-facing decisions stop when the own seat resigns or has no
remaining Foundry, even while teammates keep the match alive; remnant units
continue their ordinary simulation programs without new bot commands. Resume
rebuilds that briefing from the scenario embedded in the replay before
fast-forwarding controller memory, so it adds no hidden save state or ambient
input.

Profile resolution turns the seed into six bounded preferences: air, siege,
support, fortification, greed, and guile. Stance bounds their strategic posture;
difficulty changes fair cognitive, execution, and macro-competence limits such
as reaction, attention, memory, estimate accuracy, commitment timing, and the
minimum ordinary opening force protected from voluntary spending. Scrapheap also
uses a reduced decision cadence; Standard, Veteran, and Prime share one
competent cadence. Neither mechanism grants information, resources,
capabilities, or stronger units. Personality also does not vary private
competence: each difficulty uses one fixed conservative strength-estimation
error. The traits leave visible signatures rather than unlocking private
strategies: air changes ordinary and island strike composition and timing; siege
changes artillery volume and preference; support changes support-unit, flak, and
allied relief investment; fortification changes cross-domain defense priority
and otherwise competitive defensive roles; greed changes worker targets and
renewable-expansion payback appetite; and guile changes raid size, timing,
withdrawal, and some mine or airborne-screen emphasis. Every adaptive identity
may propose every defensive role its ordinary prerequisites permit. Exposed
value, credible approach evidence, existing coverage, and diminishing return
bound investment instead of personality-derived count ceilings. Before the
protected core exists, a current visible threat may justify one matching
emergency Turret, while emergency Flak requires a current visible aircraft
capable of attacking ground. A pure air-to-air flyer cannot unlock that
exception. A player-facing controller requires this actionable air evidence
before investing in emergency flak, so an anonymous radar blip cannot turn a
small seeded preference into an opening economy cliff.

Difficulty schedules are structurally monotone. Scrapheap thinks every 24 ticks;
Standard, Veteran, and Prime share a 12-tick cadence so controller APM does not
invert the difficulty ladder. Every decision tick available to a lower rung
remains available to the next higher rung, while reaction and commitment delays
shrink and attention and memory never shrink. Private uncertainty is fixed per
rung and conservative: a lower rung never estimates its own force as stronger,
or a hostile force as weaker, than a higher rung using the same evidence. Their
stance- and personality-independent opening floors are respectively four, five,
six, and eight Sentinel-equivalents, so the protected macro commitment is
monotone as well. Veteran and Prime coordinate engaged army fire. Prime also
uses the ordinary focus-fire command to direct an overlapping static-defense
line at one current visible ground threat; the simulation retains ordinary
acquisition whenever that preference is blocked or out of range. Veteran and
Prime share the same optional-operation attention ceiling, so Prime does not
split off a raid while air and lift work already run together.

Every ordinary difficulty cadence divides a shared 24-tick strategic admission
interval. New air, lift, and raid operations, a remembered air objective's
promotion to a current assault, and the start of a team-relief pressure watch
use those common boundaries. This lets every rung freeze the same world snapshot
before its own reaction and commitment delays take effect; private controller
cadence never grants an earlier strategic observation boundary. A team-relief
credibility watch samples current pressure at such a boundary. It prepares an
exact route-capable group without starting or owning a deployment. Only a
successful Support allocation launches that frozen group; an active relief
advances once per decision even when fresh admission is closed. Useful current
pressure and protected home strength determine membership, with a two-member
tactical minimum and no personality eligibility or group-size cap.

The player-facing controller distinguishes current sight from remembered
evidence in `StrategicIntelligence`. Persistent planners retain phased air,
lift, raid, and allied-relief operations across decisions. One
`AllocationSession` coordinates their retained work with current investment
opportunities before `UtilityPolicy` fills the remaining economy, production,
defense, support, and combat work.

The session constructs one immutable `ResourceSnapshot` from the current
`Observation`. It keeps current scrap distinct from a conservative forecast
derived only from completed recurring-income buildings. It also records exact
own units, construction-capable workers and their active or queued obligations,
and completed producer lanes with current queues, exact owner-visible progress
for each front training item, legal output, conditional timing, and current
egress evidence. The progress rows align with own buildings and queues;
malformed alignment, progress beyond the exact front item's training time, or
nonzero progress on an empty queue yields only conservative unknown-progress
bounds. Own queued programs are exposed only as a sorted set of occupied unit
ids, not as order contents; allied and hostile production progress and programs
remain opaque. Forecast income may establish feasibility at a later command
boundary, but it never becomes current credit.

Before considering fresh work, the session imports exact obligations for
already-paid or retained construction, protected opening work, standing and
planner-owned units, saved Foundry expansion, and active connected operations.
When the opening core is deficient, one current-threat emergency defense may
also enter as a survival obligation with its scorer-selected site and builder.
The remaining opening reserve receives only the bank left after that defense.
The session also adapts same-think decisions from active team-relief, lift,
raid, and admitted island-air planners into explicit legacy claims. The current
proposal set contains at most one safe, command-legal Foundry expansion, one
connected offense package, a best-first group of mutually exclusive exact
defensive alternatives, mutually exclusive economic actions, and a best-first
group of mutually exclusive standing-force alternatives. The allocator seeds a
feasible portfolio in global proposal-rank order, then examines up to 64
best-first zero-or-one domain combinations. Every accepted combination must fit
current and deadline-scoped forecast scrap, builders, sites, units, producer
FIFO timing, and incompatible construction footprints. Exhausted refinement
retains the feasible seed; an unexamined better alternative is traced as
`NotRefined`, not as an infeasibility proof. Named urgency, confidence, value,
time-to-impact, and safety bands decide first; personality resolves only a
genuine semantic tie and never removes a domain or defensive role from
consideration.

Portfolio evaluation stages all ownership and capital claims before solving the
combined producer schedule once. Acceptance reuses that validated schedule. Only
financially feasible portfolios receive exact combined-layout checks. Those
checks retain full multi-foundation egress and builder safety, remember rejected
sets, and share results across connected-force contexts in the same decision;
unused construction combinations are not enumerated in advance. Production
search and candidate enumeration can yield deterministic work slices; the
current synchronous scheduling adapter still drains them to a decision, so this
alone does not bound a controller tick.

Fixed jobs on one factory follow their retained enqueue and execution times;
funding priority does not reorder that lane. Production preflight rejects
overlapping fixed execution intervals on the same factory before enumerating
flexible schedules. A job occupies its completion tick, so the next fixed job
may start on the following tick. Extra forecast income and alternative
assignments for other jobs cannot resolve that overlap. Each tentative schedule
also preserves the payment deadlines and earliest possible execution of every
remaining fixed job. A flexible append that already makes a fixed job impossible
is rejected before searching its successors. Flexible jobs also carry optimistic
payment deadlines tightened by mandatory same-owner FIFO work. Before exploring
a partial schedule, the allocator checks that its remaining jobs can still start
and receive funding. This preserves canonical schedule order while pruning
impossible timing combinations. Capacity bounds count whole jobs in the free
windows before and after fixed reservations; a job cannot borrow time across a
reserved production interval. These deadlines and window constraints are
prepared once per scheduling attempt and reused during search.

Connected campaign assessment shares artillery firing geometry and exact command
reachability across candidate rosters and target groups within one immutable
observation. Excluded live providers do not repeat route checks. The batch
retains positive and negative answers within a bounded cache; reaching the cache
limit only disables further retention, not evaluation.

Defense derivation skips expensive placement for roles whose real current cost
cannot fit after imported fixed capital. This prefilter leaves viable quotes
unchanged; the allocator still owns producer funding and portfolio
compatibility.

Accepted payloads retain the exact site, builder, objective, force membership,
unit kind, and producer assignments selected by their domain. A defensive
payload includes the scorer-selected role and footprint, its route-proven
builder, and its quoted opportunity evidence. Commitment does not rerun domain
ranking or placement. A connected package may add the largest feasible marginal
extension only from the capacity left after its minimum and any compatible
expansion, defense, or standing-force purchase. Any malformed input or failed
exact commit freezes residual spending for that decision and restores
speculative planner state; the decision trace records the allocator result or
coordinator failure. Otherwise, still-unmigrated fresh lift and raid work runs
against the true residual bank, and future producer reservations prevent it or
`UtilityPolicy` from occupying an accepted lane.

Within the residual utility pass, a fresh `CommitmentLedger` imports upstream
committed scrap, reserved units, strategic queue appends, persistent saving, and
retained deferred foundations. It attributes current-bank spending and holds
plus exact units, builders, footprints, and contiguous producer appends to
deterministic owners. Failed utility proposals roll back atomically; releasing
one owner returns only revisable claims and reindexes surviving producer
appends. The shared `Executive` owns command-lowering bookkeeping and converts
the combined intents into ordinary candidate commands.

Reusable air-operation survivors keep their roles only through the operation
cooldown; aborts caused by unreachable routes or newly observed defenses release
them immediately. Offensive ground policy compares the force that the Executive
will actually march with defenses near the chosen objective, so staged artillery
cannot justify a push while its escort quorum would leave it behind. Visible
defenses count at full strength; remembered defenses contribute according to the
existing intelligence-confidence decay and remain usable as probe targets after
their strength estimate expires. An army holding a live objective remains
enlisted there but does not absorb the next generation of fighters, which forms
a separate muster closer to home.

Adaptive opening production first projects an ordinary core in
Sentinel-equivalent ground strength. It HP-weights live Sentinel, Warden, and
Breaker hulls, counts queued units and orders already planned during the same
decision exactly once, and excludes exact persistent-operation reservations
without excluding ordinary Executive armies or units merely held as a Team
operation's home watch. It fills shallow Foundry queues breadth-first only until
that survival prerequisite is projected. Raiders, artillery, anti-air, support,
and actual outbound persistent-operation reservations do not stand in for the
line.

After the opening core is ready, the standing-force domain derives ranked,
mutually exclusive, one-unit alternatives for independently useful ordinary
production. Its demand accounts for current and remembered hostile capability,
paid construction and expansion security, reachable wounded combatants, useful
ground objectives, completed technology, public-terrain routes, exact live and
queued inventory, same-think orders, and exact units or paid queue work already
owned by persistent operations. Each alternative has a stable identity of unit
kind plus a canonical service point or footprint. Inventory and producers count
only when public terrain and observed dynamic blockers let them serve that
target, preserving independent same-kind alternatives on disconnected fronts.
The domain retains useful tier-one providers while allowing higher-tier line,
siege, anti-air, and support units to substitute when their role, route, cost,
and readiness fit better. Immediate alternatives are current-funded, enqueue-now
work through one completed producer; forecast income cannot make an unaffordable
command legal. For a non-urgent need, completed recurring income may add a
bounded future purchase of a strictly better unlocked provider beside the
immediate fallback. The search can start below the fallback's price and uses the
bounded strategic preparation window instead of requiring repayment within one
cheap production cycle. Acceptance retains the exact producer, purchase tick,
completion, deadline, and originating need. Subsequent allocation imports that
schedule before fresh spending. Fresh future purchases are scheduled against all
retained claims, including compatible work on other factories. Nearby motion of
the same ground threat preserves the purchase, provided the producer can still
serve it; it cannot transfer the claim to a different kind of need or a distant
front. Loss of the producer, income, useful need, or opening core releases
unpaid work. Immediate threats can preempt it. Enemy fortifications motivate
deliberate siege preparation without making every siege purchase an emergency.
Economic saving preserves its capital claims while allowing compatible military
alternatives to compete.

Portfolio ranking compares sorted complete urgency, confidence, consequence,
impact-time, and safety cases before rewarding additional compatible work.
Experience, personality, domain preference, and capital resolve subsequent ties.
Several incremental purchases cannot win merely by outnumbering a material
investment in the same urgency and confidence bands. Optional connected scale
retains the production slots already assigned to the stronger portfolio.

When a fresh Connected proposal exists, the session derives separate Standing
proposal sets for Connected absence, its minimum, and every cumulative marginal.
The selected context excludes its exact live units and canonical paid
`(producer, kind)` occurrences before deriving ordinary demand. Retained and
same-think paid ownership combine by maximum multiset multiplicity rather than
addition, so the same queue occurrence is neither double-owned nor exposed as
free. Context selection and its exact marginal depth are diagnostic trace state,
not authoritative simulation state.

If a retained Connected revision can no longer preserve its exact producer
schedule, the session removes its typed obligation and selected-only Standing
contexts together, enters bounded recovery while retaining surviving operation
units, and rederives unconditional Standing proposals against the remaining paid
ownership. This downgrade is one allocation preparation transition; no context
derived from the failed revision reaches portfolio selection.

Retained funding first preserves the preferred allocation split, then tries a
deadline-compatible split when forecast income matures into current scrap.
Maturation alone must not invalidate an executable retained schedule. Protected
current purchases remain mandatory; if they make an immutable Lift schedule
unfundable, only its unpaid production obligation is released into bounded
return-home recovery, with surviving members still owned.

Economy competes through exact worker, foundation, or self-refit alternatives.
Worker value is finite harvest output or recovery of orphaned paid construction,
net of reachable existing and queued workers. Technology and factories serve
capability demand derived before prerequisite eligibility, with construction and
production delay, missing-chain costs, and eventual capacity accounted for. Live
harvest workers pay initial travel to visible safe work before contributing
output. Concurrent air and lift demand share each Airworks lane's time once,
bounded by readiness, customer deadlines, and route reachability. Local Foundry
throughput opportunities reuse the expansion admission and security path.
Additional throughput is capped by current unprotected capital and completed
income after the candidate and its missing prerequisites are paid. Capacity
confidence and urgency come from the demand contributing its marginal return; an
already-covered current need cannot strengthen a speculative capacity case. A
proposed first Airworks tries current targets by value and regional distance
until it finds a complete connected scout, suppression, and strike minimum. This
witness excludes optional force growth and target-cluster expansion. The
hypothetical factory exists only inside this pure sizing calculation: its
construction capital and delay are removed before the ordinary package and route
checks run, and existing live units are excluded from speculative ownership.
Retained obligations must fit the post-construction capacity before campaign and
route derivation begins. The complete minimum must fit alongside retained
capital promises and producer jobs in shared allocation. This supported
investment value cannot justify duplicate Airworks, issue a production command,
or admit an operation before its real prerequisites exist. Recurring-income
investments are capped by unfunded useful work; completed income alone supplies
spendable forecasts. Self-refits own exact building ids and withhold their
offline source income separately from purchase capital. Defensive refit
valuation estimates protection at nearby asset approaches, using actual weapon
coverage, redundancy, health, and offline time. Public terrain connectivity
filters ground-threat priors; nearby current attackers prevent refitting. This
estimate does not reconstruct remote army routes or credit distant choke-point
protection. The residual technology scalar and the operational Airworks capital
tax are absent. Economic purchases keep a fixed funding deadline separate from
their return horizon. Shared allocation rebalances their current and forecast
capital alongside fixed producer payments; a missed funding deadline releases
the unpaid plan for reconsideration. Issuing a build pays for its site
immediately, including travel through fog; paid foundations and refits follow
ordinary simulation rules. Extractor development compares explored, safe frame
groups around a common Foundry site by their total return after restoration,
support, travel, and build costs. The existing expansion security check must
admit the shared support site. Only the next restoration owns capital and a
builder; later steps are re-evaluated as construction completes, and their
projected income never becomes spendable forecast credit.

Before the difficulty floor is projected, the player-facing policy pauses new
voluntary construction and upgrades, discretionary production, mobile support,
and paid repairs. New air, lift, raid, and team-relief operations are not
admitted. Existing operations may advance, withdraw, or release their units but
receive no purchase budget, while already paid sites and queues are never
canceled merely because the core fell. A separately unsafe, unattended defense
site may still be canceled by its ordinary current-danger rule. Automatic Repair
Bay pulses remain ordinary simulation behavior. The opening preserves a fourth
Harvester and one safe Extractor frame supported by the exact living authored
starting Foundry. At most one Turret for a current visible ground threat and one
Flak Turret for a current visible ground-attack aircraft may bypass the floor;
pure air-to-air aircraft, memory, public starts, radar blips, and raid history
cannot. A think that starts below the floor stays recovery-gated through that
decision. Core orders issued during the think count toward projected strength
and prevent duplicate purchases, but voluntary spending and strategic admission
do not reopen until the next observation confirms the floor. After the floor,
voluntary capital must leave a Sentinel that will remain shallow after the
upcoming production phase or keep its exact cost unspent, unless fog-honest
knowledge plus public terrain proves there is no ground objective. A lone
existing front-slot Sentinel that completes during the upcoming production phase
does not satisfy that condition. Construction pays at command acceptance, so its
travel does not reserve additional purchase capital. Losing enough core strength
reapplies the same gate.

The residual Foundry pass no longer originates player-facing ordinary combat,
siege, anti-air, or Tender orders. Residual construction no longer originates a
player-facing Turret, Bastion, Flak Turret, Scuttle Charge, Barricade, Array, or
Repair Bay; it retains recovery. Support allocation compares finite own repair
work, exact worker assignments, Tender purchases, and marginal Repair Bay sites.
Repair programs reserve current scrap through their next decision boundary,
renew without reissuing unchanged orders, and stop before purchases when
unfunded. All selected foundations share one complete layout certificate.
Existing workers, paid Tenders, and local built or pending Bays consume the same
finite patient workload before procurement is valued. Repair cost and patient
replacement value are distinct. Current threats to specific own assets produce
protective deployment requests; Support retains exact deployment actors, while
ordinary Standing Force owns any missing screen or anti-air procurement. An
out-of-position protector returns with an ordinary Move order, even while busy,
and resumes protection inside the service radius without renewing its deadline.
Building-patient Bay coverage uses both complete footprint rectangles, matching
the simulation's aura when valuing new and overlapping service. Reconnaissance
is a separate allocation domain: the controller reconciles retained questions
before proposing new work, then compares exact live observers, paid queue
occurrences, and dedicated purchases. Each accepted question retains its
consumer, evidence, goal, and useful deadline independently. An unpaid purchase
retains its exact accepted producer schedule and current/forecast funding; only
a current-funded append becomes a command. Question-local loss cooldowns and
quiet intervals permit valuable safe reconsideration without requiring new enemy
sight. Reconnaissance no longer uses one global scout slot. A completed paid
occurrence binds only a newly observed eligible scout at its exact producer
exit; a lost occurrence cannot adopt another assignment's later newborn. A
surviving observer that can no longer arrive before its fixed deadline remains
owned for recall, not recorded as lost. Operational and question-driven scouts
share exact `(producer, kind, occurrence)` exclusions: excluded items still
occupy the FIFO lane but cannot supply another assignment. Recent objective
sightings suppress immediate repeat purchases; remembered positive buildings are
not uncleared authored starts. Existing paid observers cover overlapping point
questions only when their route and deadline serve the entire footprint. This
overlap credit never replaces contested sweeps. Full-footprint negative evidence
and contested-region sweep completion remain separate from safe return. It no
longer originates residual scout purchases. Scuttlers enter Standing Force
allocation only through a viable current raid objective's exact missing tactical
pair. Paid and serviceable live supply reduce that request; its preparation
deadline is fixed, and target loss does not revoke paid queues. Raid preparation
retains exact paid queue occurrences as allocation obligations until they bind
newly observed members at their producer exits. Residual advancement excludes
other owners while retaining the raid's own muster, and launches only against
the procurement objective. A missing paid member releases preparation into
bounded recovery without adopting a later birth. Bomber, ground-attack-air, and
transport cohorts remain owned by persistent operations. Their outstanding work
and fixed deadlines contribute economic capacity demand; they do not impose an
unowned factory reserve.

On connected ground, the air planner admits a force package only when current
sight, the spendable current bank after prior reserves, completed recurring
income net of earlier forecast promises, and completed producer lanes can field
viable reconnaissance, suppression of currently observed operational
ground-targetable static and mobile anti-air covering the bounded target
cluster, and ground-strike capability by one fixed decision deadline. A
currently observed air-domain anti-air source rejects the package because its
ground suppression cannot remove that threat. Existing and already-paid
lower-tier providers retain their value; completed advanced production may add
higher-tier providers. Every package has a shared capability minimum and an
opportunity-specific useful capability target rather than a fixed roster cap.
The target's currently visible, actually splash-vulnerable ground units and
buried charges create a separate optional bombing opportunity; ordinary
buildings remain direct-strike value rather than fictional splash victims, and
operational mobile anti-air already priced as mandatory suppression is not
counted again as optional bombing collateral. After every family reaches the
minimum, a width-eight beam ranks capped total useful capability first and uses
personality to weight how otherwise competitive marginal capability is divided
between air, siege, direct strike, and attack-run bombing. Personality never
gates a provider or family. One beam slot preserves the cheapest alternative;
existing useful providers remain eligible at minimum strength, and every
extension revalidates its canonical funding order. For otherwise identical
evidence, more current scrap, more available preparation time at derivation, or
additional completed usable production capability cannot revoke admission or
reduce capped total useful capability.

Current connected targets are ranked canonically and tried in order until one
admits a complete package. A route-feasible optional member of its bounded
current cluster is retained only when the complete revised package still fits
the same producer access, queues, funds, and fixed preparation deadline. The
package stores and traces the canonical anchors it actually admitted. During
Recon and Verify, negative anti-air evidence requires current visibility over
every footprint tile of every surviving admitted target; the scout focuses the
first unknown tile before the operation may commit.

Forecast income is proposal evidence, not command credit. Providers, producer
exits, artillery staging, reconnaissance, and strike routes must remain viable
through public terrain and observed dynamic blockers. Route preflight carries
the active seat's original `Orientation` into the oriented observation. It
reproduces authoritative center snapping and group spreading for `Move` and
`AttackMove`, uses the authoritative world-frame producer doorstep, and
validates one reachable legal firing stand for every exact suppression `Attack`
member without applying group spread. When every demanded suppression member is
already live, staging preflight reproduces that roster's one authoritative
spread. If scheduled members do not yet have positions, it conservatively proves
both possible deterministic spread scans from the admitted source component.
Each production command still spends only the spendable current bank, uses an
exact completed producer, and must fit that producer's conservative queue and
egress bound.

Current target value, current operational anti-air, available resources, and
uncommitted providers may revise a connected package during Recon and Assemble,
but revision never extends its preparation deadline. The operation freezes its
exact assigned ids on entering SuppressAa. Later suppression- and strike-cohort
losses are measured against the package's shared capability minimum; the
required scout remains an exact-identity requirement. Missing the preparation
deadline enters bounded recovery instead of extending or replacing the cohort
indefinitely.

On severed ground, wealthy bots may run two independent operations. The air
planner builds a screen and bomber wing, scouts the route, attacks currently
visible flak along it, and then commits against a current objective. Its force
targets can grow with newly observed wealth and roster strength during Recon and
Assemble, then freeze when suppression begins. The lift planner likewise grows
its payload and matching Skyhook target during Provision, then freezes exact
manifests when Boarding begins. Carrier demand follows payload and usable
landing capacity rather than an arbitrary controller cap, while a ground-capable
reserve remains at home instead of being stripped into a bulk lift.

The wealthy island air operation also has a separate admission gate of 12
currently armed units. That standing-roster check establishes readiness to open
the operation; it neither sets the screen or bomber demand nor caps later force
scaling.

Before a remembered objective is reacquired, an unadmitted Recon operation may
hold exactly one Skyhook's cost out of otherwise uncommitted scrap. It does so
only for a built, non-expired contact when the bot has a completed Airworks, a
transportable payload, no live or queued usable carrier, and optimistic routing
still proves the pickup and objective ground-disconnected. This is a prospective
capital reservation only: it neither creates a lift nor claims a payload or
queues a carrier before current sight.

Fresh typed investments resolve before that residual Recon lifecycle. The
allocator therefore previews the exact retained or newly admissible Recon target
and raises every fresh proposal's shared minimum-residual floor by the
prospective carrier cost. The floor does not claim the bank: residual
coordination confirms or releases it from the resulting planner state and
records the actual hold exactly once.

The operations choose and execute objectives independently. When both select the
same target, they exchange an explicit target-specific hold, release, or abort
signal so a lift can follow air-defense suppression without depending on it.
Only targetless or safely staged Executive armies can transfer into a lift;
units holding a currently contested objective remain enlisted. Carrier
production and extra Airworks use the same ordinary queues, costs,
prerequisites, and deterministic capital ledger as all other bot production.

An undispatched scout slot may still be filled or trained while an air operation
prepares. Once the operation has sent its exact scout, losing that unit during
Recon or Assemble aborts into Recover instead of silently drafting or training a
replacement; losing required reconnaissance or strike forces in later phases
does the same. Recovery releases factory capital, sends routable survivors home
once, has a finite completion bound, and then observes the normal operation
cooldown.

Question-driven reconnaissance may propose a dedicated flyer when the exact
useful question lacks a safe, timely live or paid observer. Losing a dispatched
observer releases only that question's unpaid capital and enters a 3,600-tick
loss cooldown. After a 300-tick quiet interval, an unanswered and still-useful
question may compete again through a newly validated safe approach without fresh
enemy sight. Neither timer expiry nor unchanged loss evidence automatically buys
a replacement; independent questions and operational scouts retain their own
assignments.

The fog-honest observation carries the same bounded, anonymous salvage-danger
incidents that authoritative vision records for autonomous Harvest. The
player-facing economy rejects sources inside a current warning as well as
current radar and mobile pressure and remembered static weapon envelopes. An
incident contains only an allied impact tile, never the unseen attacker's
identity. The controller promotes that warning to persistent contested-region
memory only when a matching own or visible allied Harvester lost HP or
disappeared near its previous position, current position, or active source.
Ordinary combat losses therefore expire instead of quarantining nearby salvage
forever. Distinct incident centers remain distinct even when their danger
regions overlap, so merging evidence cannot shrink the union that recovery must
prove safe.

Once the warning and projected danger clear, recovery reconnaissance visits
deterministically ordered unseen cells across the exact danger region and
accumulates only current safe sight. Complete coverage clears the quarantine.
Fresh danger, an unreachable next cell, or a bounded no-progress timeout recalls
the scout. The controller reserves that exact unit until current observation
places it back in the safe home area; neither an idle body in the field nor an
elapsed timer releases it. The bounded retry delay starts after safe return.
Worker evacuation and later Harvest routes avoid both projected danger and
quarantined cells, so recovery cannot cross the same kill zone it is trying to
prove safe. Lowering remembers a dispatched Harvest only long enough to audit an
immediate no-route bounce.

Player-facing static-defense investment is a typed allocation domain. At most
one exact candidate for each of Turret, Bastion, Flak Turret, Scuttle Charge,
Barricade, and Array is derived from a shared grounding, and the portfolio may
select at most one of those mutually exclusive alternatives per decision. The
weapon-bearing roles share one fog-honest strategic site scorer. It values owned
production, technology, support, renewable economy, and active resource work;
then intersects credible hostile approaches with each kind's actual weapon,
spotting, trigger, or path-disruption geometry. Current contacts, remembered
enemy sites, and uncleared public starts form descending evidence tiers.

Before site search, fixed producer payments retain the current capital they
still need after completed-source income at each payment deadline. Defense roles
whose construction cost exceeds the remaining bank are omitted; joint allocation
still checks all future claims and exact producer scheduling. Initial approaches
and candidate evaluations reuse successful canonical endpoint paths within the
same immutable grounding, separately for ground and air. Failed searches retain
their exhaustion or expansion-limit evidence.

The investment case prices only marginal protection not already owned by a live
defense or reserved by a paid unfinished footprint. New protected value counts
fully and reinforced value has diminishing return. Current or remembered
threats, construction and builder travel time, the readiness of mobile
reinforcement, and risk before completion determine the named urgency, value,
confidence, time-to-impact, and safety bands compared by allocation. Ordinary
technology prerequisites are the only role gates; personality ranks otherwise
legal cases across and within the domain but never removes a role. Candidate
footprints must preserve builder access, producer egress, and active resource
routes. Exact builder-route prediction combines the public static terrain
briefing with fog-honest observed dynamic blockers; public resource priors are
not treated as live obstacles. The accepted proposal retains the scorer's exact
site and builder through `BuildWith`.

Voluntary defense checks a necessary current-capital bound before quoting each
kind. It includes imported fixed claims and the maximum of the carrier floor and
the smaller of the shallow guard or a Sentinel purchase. A current shallow
Sentinel can discharge the voluntary guard; the admission bound therefore never
becomes an additional proposal debit. Active Connected revisions retain full
quotation because subsequent recovery can release imported claims. Defense
completion times rank utility but do not extend the allocator's funding horizon:
these proposals pay construction capital entirely from current scrap. When a
saved Foundry's footprint covers its assigned worker, every combined defense
layout would fail the existing builder-start check. The session detects that
overlap before quoting defense. Full layout validation uses the same cheap check
for selected workers covered by any blocking footprint; nonblocking mines do not
cover starts, and unselected workers do not trigger this rejection.

Voluntary weapon-bearing site search ranks inexpensive approach-frontage
estimates. Nearby mobile threats with the same weapon capability, local terrain
region, and direction toward an asset share one reachable representative route;
static threats and distinct fronts remain separate. Weapon and Array placement
share a small per-role refinement slice and continue beyond rejected sites on
later decisions. A retained candidate avoids rediscovery but must pass current
builder, egress, resource-route, support, and coverage checks before reuse;
uncommitted candidates expire. Emergency defense retains exhaustive best-site
selection with conservative coverage bounds. A candidate rejected by investment
valuation does not remain the role's incumbent.

Defensive geometry shares a bot-owned route cache across economic and defensive
valuation. Each retained generation compares map dimensions and the complete
fog-honest passability surface. Directed endpoint paths include the candidate
footprint in their key; air routes ignore ground-only footprints. Normal ground,
hypothetical combined-build layouts, and air surfaces have independent bounded
retention: one normal generation has an 8 MiB budget, two hypothetical
generations have 1 MiB each, and air has 8 MiB. After the blocking grid,
endpoint paths own half the payload budget; normal and candidate reverse
distance fields each own a quarter. Eviction in one class cannot discard
another. These bound accounted retained payload and entry allowances, not
process RSS. Cache eviction or an oversized entry falls back to search.
Successful hits clear old exhausted-component evidence, while failures are not
stored as bare unreachable results.

Lazy reverse distance fields use the same open-tile graph, 10/14 movement costs,
and diagonal corner rules. Endpoint ranking uses an obstacle-free bound unless
an exact field is already retained. Repeated uncached routes sharing either
endpoint earn a normal field after their accumulated expansions reach the map's
cell count; tracking retains at most 256 endpoints. Large endpoint sets can
prepare one origin field sooner when the measured first search projects more
work than the field across the batch. Cache hits contribute no search work.
Existing fields reject endpoint pairs whose cost exceeds a route already found;
adding a blocking footprint cannot improve that bound. For long paths, exact
fields for the current footprint also prune A* branches that cannot belong to a
shortest route. Queue ordering remains unchanged to preserve the complete
route-choice key and mobile firing positions. This pruning is disabled when the
map exceeds the expansion cap or the start is blocked, preserving capped
searches and escape from blocked origins. A* still constructs every uncached
selected path. Investment scores, threat evidence, asset values, and budgets are
recomputed from the current observation rather than retained with passability.

`bot::navigation` owns all bot path, cost, connectivity, and distance-field
searches, including their scratch storage, cache invalidation, and retention.
Command projection preserves orientation, goal spreading, and Build doorstep
selection. Service connectivity, safe travel costs, work-distance queries, and
producer-exit certificates retain answers within their navigation contexts.
Planners supply player knowledge, safety predicates, and candidate preferences;
investment scores and service eligibility remain planner-owned. Barricade
footholds consume scalar costs, while coverage, mobile standoff, and retreat
retain canonical paths. Costs distinguish exact and bounded success from
disconnection and search limits. Detour limits still precede local-support
filtering; construction and layout checks retain their existing route contracts.

Match setup prepares and shares a public terrain index for each seat
orientation, with connected components inside 16-by-16 regions and deterministic
boundary links. Regional distances rank strategic targets; they do not certify
live reachability, route safety, command timing, or placement legality.
Orientation and terrain changes invalidate the index independently of dynamic
navigation caches. Public distance fields materialize passability once and use
an owned traversal that can yield after a deterministic number of queue entries.
Fresh Foundry logistics uses a controller-owned allowance shared across its
field requests. Passability preparation and traversal both consume work; an
unfinished field resumes on later decisions and never means unreachable. At most
four jobs survive, with exact terrain and danger invalidation and a 120-tick
unfinished lifetime. Allocation rollback preserves work already done and spent
allowance. Saved Foundry validation remains immediate; other synchronous field
consumers still request completion.

Exact Build-route checks index observed and public ground passability once per
defensive grounding and reuse A* storage across builders and candidate sites.
Lazy component labels reject builder/site pairs with no connected base doorstep
before searching candidate routes. Candidate footprints and additional blockers
can only remove connections, so this rejection preserves exact paths, doorstep
preference, and bounded-search behavior for eligible pairs. Proposed footprints,
extra blockers, danger checks, and authoritative doorstep ranking remain
query-local, so cached terrain cannot change the selected route. Travel-cost and
safety checks share only their most recent exact route; danger is checked again
on every use, and a changed builder, target, or orientation requires a new
route.

Test-only navigation work counters enforce structural regression bounds across
consumers without using wall-clock timing or affecting controller state.

The pre-core emergency path is deliberately narrower than voluntary allocation.
It uses only a current visible armed ground threat for a Turret or a current
visible ground-attack aircraft for Flak, places only the matching defense, and
cannot inherit threat authority from pure air-to-air aircraft, memory, public
starts, radar blips, or raid history. Shared allocation freezes its exact site,
builder, footprint, and current construction cost before ordinary opening-core
recovery. This survival obligation has precedence over voluntary defense and no
second utility ranking pass runs.

Arrays enter the Defense proposal group through a separate player-facing
sensor-site scorer because information coverage is not weapon coverage.
Candidate sites extend up to the Array's radar radius from relevant owned
assets, preserve ordinary placement, producer-egress, and active resource-access
rules, and bind the exact route-capable builder proven through public static
terrain plus observed dynamic danger. An Array requires positive usable coverage
not already reserved by another proposal. The scorer first ranks strategic
demand remaining useful at the selected builder's arrival and construction
completion, then extends radar area not already supplied by own, allied, or
pending Arrays, then retains usable in-map coverage; off-map tiles and Peaks
contribute nothing because no unit can occupy them. Current contacts, remembered
contacts, and uncleared public starting priors break otherwise equivalent sites
toward credible hostile approaches. Sensor cases remain bounded below an
immediate survival defense regardless of coverage, and compact maps may use a
partial radar disc. Array refinement mixes coverage-ranked sites with nearby
sites, rejects disconnected builder doorsteps, and checks exact readiness and
safety within the shared site allowance.

The player-facing budget observes provisional scaffolds as paid construction
with an exact site id. Their prices have already left the bank, so retained
worker obligations do not reserve that money again. Player-facing Foundry
expansion has no count ceiling. It groups known resources by public terrain
region, ranks potential sites using approximate travel, and prices a bounded
pair exactly. The strongest regional candidate remains in consideration while
remaining regions rotate on actual planning requests; skipped admissions and
repeated same-tick queries cannot skip or refill that rotation. Payback includes
supported owned Extractors, Foundry drip attached to an external objective, and
shorter visible-scrap hauls. Reverse fields from shortlisted Foundry footprints
preserve exact haul costs while avoiding observed danger. Public unbuilt frames
remain scouting priors. Greed and uncommitted scrap extend the forecast without
changing capability.

Expansion saving and construction share one exact claim: a legal footprint and a
specific worker with a known safe route and work area. Admission preserves the
difficulty's ordinary core, assigns fog-honest ground threats across the current
Foundry network, credits only completed defenses whose real coverage protects an
asset, and adds a one-Sentinel forward reserve toward uncleared reachable public
starts. A worthwhile underprotected site prepares only its exact missing core;
its candidate-specific security cost is removed from genuinely uncommitted
wealth before payback is admitted. Buying that core reduces the outstanding cost
and cash together. Foundry capital is reserved after projected protection is
ready, and a partial or complete fund closes later voluntary production,
construction, and paid-repair spending until the exact build command is emitted.
Once saving begins, `UtilityPolicy` preserves the exact site, worker, and
required total across decisions and imports them into the per-think ledger. The
worker lease prevents scouting, repair, salvage, queued work, unrelated
construction, and implicit drafting from stealing it during intent lowering;
only the matching Foundry build may consume it. Transition, invalidation, or an
urgent opening-core shortfall releases the claim immediately. A temporarily
blocked plan gets a bounded recovery window, while a plan that needs new
protection releases its fund for that preparation before it can be reconsidered.
A generic frontier nearer to a known enemy Foundry than to any projected own
Foundry is not eligible, and only one unpaid Foundry claim may exist. This is
controller discipline, not simulation escrow: automatic Repair Bay pulses
continue to follow the ordinary bank rules. The simulation's command layer
remains the final legality authority.

The current wire format deliberately has one maintained controller, `scripted`;
only difficulty, stance, and personality seed are stored, not the resolved
traits or planner memory. Replays record the commands a bot emitted, so
read-only playback does not rerun that controller. Replay compatibility remains
governed by the simulation version rather than by retaining obsolete bot
implementations. Authored scenarios and current-version replays accept only the
current shape. The Oxide replay loader recognizes known retired
bot-configuration shapes only inside a replay stamped with another simulation
version, normalizing that setup metadata so deliberate archaeology can reach the
version check. Serialization emits only the current shape.

## Maintained entry points

This table names the first source and focused suites to inspect. It is a routing
map rather than an exhaustive test inventory.

| Contract                                           | Primary source                                                                                                                                                                                            | Focused evidence                                                                                                                    |
| -------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| Scenario build and authored map                    | `sim/src/scenario.rs`, `sim/src/map.rs`                                                                                                                                                                   | inline module tests, `sim/tests/pits.rs`, `sim/tests/extractors.rs`                                                                 |
| State, hashing, validation, and teams              | `sim/src/state.rs`, `chassis/src/hash.rs`                                                                                                                                                                 | `sim/tests/state_integrity.rs`, `sim/tests/determinism.rs`, `sim/tests/teams.rs`                                                    |
| Placement, deferred founding, and upgrades         | `sim/src/state/placement.rs`, `sim/src/tick/commands.rs`, `sim/src/tick/brain.rs`, `sim/src/tick/brain/economy.rs`                                                                                        | `sim/tests/behavior_construction.rs`, `sim/tests/extractors.rs`, `sim/tests/upgrades.rs`, `sim/tests/foundries.rs`                  |
| Tick scheduling, production, cleanup, and charges  | `sim/src/tick/mod.rs`, `sim/src/tick/production.rs`                                                                                                                                                       | `sim/tests/behavior_rules.rs`, `sim/tests/behavior_economy.rs`, `sim/tests/field_kit.rs`                                            |
| Command vocabulary and set semantics               | `sim/src/command.rs`, `sim/src/tick/commands.rs`                                                                                                                                                          | `sim/tests/command_canonicalization.rs`, `sim/tests/fuzz.rs`                                                                        |
| Unit programs, routing, movement, and collision    | `sim/src/tick/brain.rs`, `sim/src/tick/brain/locomotion.rs`, `sim/src/tick/movement.rs`, `chassis/src/path.rs`                                                                                            | `sim/tests/behavior_movement.rs`, `sim/tests/movement_lab.rs`, `sim/tests/peaks.rs`, `sim/tests/pits.rs`                            |
| Boarding and unloading                             | `sim/src/tick/brain/logistics.rs`                                                                                                                                                                         | `sim/tests/transports.rs`                                                                                                           |
| Harvesting, income, salvage, and repair            | `sim/src/tick/brain/economy.rs`, `sim/src/tick/production.rs`                                                                                                                                             | `sim/tests/harvest_zones.rs`, `sim/tests/salvage.rs`, `sim/tests/repair_unit.rs`, `sim/tests/repair_bay.rs`, `sim/tests/smelter.rs` |
| Weapons and simultaneous resolution                | `sim/src/stats.rs`, `sim/src/tick/brain/combat.rs`                                                                                                                                                        | `sim/tests/behavior_combat.rs`, `sim/tests/combat_edges.rs`, `sim/tests/shells.rs`, `sim/tests/peaks.rs`                            |
| Fog, memory, radar, and stealth                    | `sim/src/vision.rs`, `sim/src/state.rs`                                                                                                                                                                   | `sim/tests/bot_brain.rs`, `sim/tests/bastion_acquisition.rs`, `sim/tests/field_kit.rs`                                              |
| Bot knowledge, profiles, and fair difficulty       | `sim/src/bot/briefing.rs`, `sim/src/bot/observation.rs`, `sim/src/bot/intelligence.rs`, `sim/src/bot/orient.rs`, `sim/src/bot/profile.rs`, `sim/src/bot/difficulty.rs`                                    | inline module tests, `sim/tests/bot_brain.rs`                                                                                       |
| Bot resource evidence and planning commitments     | `sim/src/bot/resources.rs`, `sim/src/bot/resources/ledger.rs`, `sim/src/bot/resources/production.rs`, `sim/src/bot/utility.rs`, `sim/src/bot/executive/lowering.rs`                                       | inline module tests, `sim/tests/bot_policy.rs`, `sim/tests/scripted_bot.rs`                                                         |
| Bot cross-domain investment allocation             | `sim/src/bot/resources/planning.rs`, `sim/src/bot/allocation.rs`, `sim/src/bot/allocation/`                                                                                                               | inline allocation, adapter, coordinator, session, and Brain tests                                                                   |
| Bot playbooks, routing, reservations, and lowering | `sim/src/bot/strategy.rs`, `sim/src/bot/strategy/force_package.rs`, `sim/src/bot/lift.rs`, `sim/src/bot/raid.rs`, `sim/src/bot/team.rs`, `sim/src/bot/navigation/commands.rs`, `sim/src/bot/executive.rs` | inline module tests, `sim/tests/bot_policy.rs`, `sim/tests/scripted_bot.rs`                                                         |
