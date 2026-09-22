# Simulation architecture

This document describes the current architectural contracts of `oxide-sim`. It
maps responsibilities and invariants, not tuning or history. Scripted-bot
procedures belong in the scripted-bot skill; balance lives in
`sim/src/stats.rs`.

`oxide-sim` does not depend on `oxide-bot`. It owns serializable player
knowledge and its fog filtering in `observation`; bot policy and derived
navigation caches live in `oxide-bot`. Pure command and production geometry
shared with predictors lives in `geometry`, outside the private tick
implementation.

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
- harvest-capacity bounds on scrap held by walking and transported units;
- valid owners, faction production, entity references, and shell fields;
- coherent construction, salvage, recovery, ghost, radar, and memory state;
- canonical ordering for every collection whose order is observable.

`Pcg32` validates its odd stream increment at its own deserialization boundary.

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

Weapon hits, mine blasts, and aircraft impacts share HP reduction and
`DamageTaken` notification for the victim's owner. They retain their individual
tick phases and targeting rules. Zero damage, already-dead entities, and
provisional scaffolds produce no damage notification. Salvage-risk memory and
retaliation remain weapon-resolution responsibilities.

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

Routes stay body-blind, but a ground follower never admits a lookahead leg
through a friendly ground body standing still, and a body whose contact cancels
most of its intended progress toward its waypoint for twelve running ticks drops
its route so its brain plans again from where it actually is; the counter is
serialized and validated. Only friendly bodies count for the lookahead: steering
around an unseen enemy before contact would leak its position, so hostile bodies
are still met by the collision resolver alone.

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
workers choose mirrored sources without depending on seat or unit ids. Work
tiles that other friendly workers hold or are heading for are last resorts,
taken only when every tile around a source is spoken for; a parked worker also
claims every tile whose center lies within 0.9 tiles of its hull.

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
consume nearby wreck salvage for income, nearest tile first and then the richer
one, with exact ties ordered in the crucible's half-turn frame so mirrored
crucibles burn mirrored tiles. These are ordinary authoritative rules, not shell
conveniences.

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
They accelerate from rest over six ticks and brake from full speed over three.
The hull turns toward its target every tick and keeps rolling through a bend,
easing off as the heading error grows; off the exact bearing the body travels
along its heading, so a bend is driven as an arc. Only an error past 96 of 256
compass steps (135 degrees) brakes along the existing heading and pivots in
place. Final approaches reduce speed to stop at the goal, and inside the last
braking step the body lands on the point whatever its bearing. Stop and lost
paths brake without retaining the old order. Newly blocked terrain can arrest
that coast. Turn rate remains the ceiling of movement speed times 64, bounded to
four through ten compass steps per tick; Breaker retains four, and Avalanche and
Bombard retain three. Within eight compass steps of the bearing the body tracks
the target point directly. Ground units spawn facing the map center so mirrored
placements have mirrored initial turn costs. Independent weapon mounts can aim
during travel; fixed weapons wait for the motor to stop before turning to aim.

A ground follower does not drive a grid route corner by corner. Each tick it
looks ahead a bounded number of waypoints and steers for the furthest one its
hull can reach on a straight leg, tested as a swept line of its body radius
against terrain, building occupancy and friendly bodies at rest, so a staircase
is driven as one line and a corner is rounded only once the far side is actually
clear. A waypoint reached that way is revalidated each tick with the same swept
terrain test, so ground claimed beside the leg drops it for a fresh route; an
adjacent waypoint keeps the tile rules the route was planned under, so a wide
hull beside a wall never loses a leg it could always walk.

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
observed world. Observation schema 20 distinguishes explored pits from
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

Bot knowledge, admission, planner ownership and command lowering are described
in [Bot architecture](bot-architecture.md). The simulation enforces the same
command, cost and visibility rules for human and bot command sources.

## Maintained entry points

This table names the first source and focused suites to inspect. It is a routing
map rather than an exhaustive test inventory.

| Contract                                           | Primary source                                                                                                                                                                | Focused evidence                                                                                                                    |
| -------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| Scenario build and authored map                    | `sim/src/scenario.rs`, `sim/src/map.rs`                                                                                                                                       | inline module tests, `sim/tests/pits.rs`, `sim/tests/extractors.rs`                                                                 |
| State, hashing, validation, and teams              | `sim/src/state.rs`, `chassis/src/hash.rs`                                                                                                                                     | `sim/tests/state_integrity.rs`, `sim/tests/determinism.rs`, `sim/tests/teams.rs`                                                    |
| Placement, deferred founding, and upgrades         | `sim/src/state/placement.rs`, `sim/src/tick/commands.rs`, `sim/src/tick/brain.rs`, `sim/src/tick/brain/economy.rs`                                                            | `sim/tests/behavior_construction.rs`, `sim/tests/extractors.rs`, `sim/tests/upgrades.rs`, `sim/tests/foundries.rs`                  |
| Tick scheduling, production, cleanup, and charges  | `sim/src/tick/mod.rs`, `sim/src/tick/production.rs`                                                                                                                           | `sim/tests/behavior_rules.rs`, `sim/tests/behavior_economy.rs`, `sim/tests/field_kit.rs`                                            |
| Command vocabulary and set semantics               | `sim/src/command.rs`, `sim/src/tick/commands.rs`                                                                                                                              | `sim/tests/command_canonicalization.rs`, `sim/tests/fuzz.rs`                                                                        |
| Unit programs, routing, movement, and collision    | `sim/src/tick/brain.rs`, `sim/src/tick/brain/locomotion.rs`, `sim/src/tick/movement.rs`, `chassis/src/path.rs`                                                                | `sim/tests/behavior_movement.rs`, `sim/tests/movement_lab.rs`, `sim/tests/peaks.rs`, `sim/tests/pits.rs`                            |
| Boarding and unloading                             | `sim/src/tick/brain/logistics.rs`                                                                                                                                             | `sim/tests/transports.rs`                                                                                                           |
| Harvesting, income, salvage, and repair            | `sim/src/tick/brain/economy.rs`, `sim/src/tick/production.rs`                                                                                                                 | `sim/tests/harvest_zones.rs`, `sim/tests/salvage.rs`, `sim/tests/repair_unit.rs`, `sim/tests/repair_bay.rs`, `sim/tests/smelter.rs` |
| Weapons and simultaneous resolution                | `sim/src/stats.rs`, `sim/src/tick/brain/combat.rs`                                                                                                                            | `sim/tests/behavior_combat.rs`, `sim/tests/combat_edges.rs`, `sim/tests/shells.rs`, `sim/tests/peaks.rs`                            |
| Fog, memory, radar, and stealth                    | `sim/src/vision.rs`, `sim/src/state.rs`                                                                                                                                       | `bot/tests/bot_brain.rs`, `sim/tests/bastion_acquisition.rs`, `sim/tests/field_kit.rs`                                              |
| Bot knowledge, profiles, and fair difficulty       | `bot/src/briefing.rs`, `sim/src/observation.rs`, `bot/src/intelligence.rs`, `bot/src/orient.rs`, `bot/src/profile.rs`, `bot/src/difficulty.rs`                                | inline module tests, `bot/tests/bot_brain.rs`                                                                                       |
| Bot resource evidence and planning commitments     | `bot/src/resources.rs`, `bot/src/resources/site.rs`, `bot/src/allocation.rs`, `bot/src/resources/production.rs`, `bot/src/utility.rs`, `bot/src/executive/lowering.rs`        | inline module tests, `bot/src/utility/policy_tests.rs`, `bot/tests/scripted_bot.rs`                                                 |
| Bot cross-domain investment allocation             | `bot/src/resources/planning.rs`, `bot/src/allocation.rs`, `bot/src/allocation/`                                                                                               | inline allocation, adapter, coordinator, session, and Brain tests                                                                   |
| Bot playbooks, routing, reservations, and lowering | `bot/src/strategy.rs`, `bot/src/strategy/force_package.rs`, `bot/src/lift.rs`, `bot/src/raid.rs`, `bot/src/team.rs`, `bot/src/navigation/commands.rs`, `bot/src/executive.rs` | inline module tests, `bot/src/utility/policy_tests.rs`, `bot/tests/scripted_bot.rs`                                                 |
