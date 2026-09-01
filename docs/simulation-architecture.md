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
from that seed, not the scenario seed. The profile-free Overseer QA constructor
retains its documented scenario-seeded army-size jitter.

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
- valid owners, faction production, entity references, and shell fields;
- coherent construction, salvage, recovery, ghost, radar, and memory state;
- canonical ordering for every collection whose order is observable.

Each new serialized field needs an invariant decision and an adversarial case in
`sim/tests/state_integrity.rs`. The same test suite also round-trips states the
real simulation produces, preventing the validator from becoming stricter than
reachable reality.

## Tick pipeline

Phase order is game behavior. `State::tick` currently performs:

1. Capture any newly stranded economy's finite recovery entitlement.
2. Validate and apply this tick's commands in their recorded order.
3. Apply recurring income, advance production queues, and spawn completed units.
4. Decay unclaimed tier-zero construction sites on their global cadence.
5. Run unit brains and building behavior, land arriving shells, and resolve
   buffered damage, construction, salvage, repair, and deferred founding.
6. Resolve boarding and unloading after every unit has decided.
7. Evict pathless ground bodies from newly claimed blocking footprints.
8. Follow paths, then resolve same-domain unit collisions.
9. Detonate armed Scuttle Charges under hostile post-movement bodies.
10. Remove dead entities and deposit eligible wreck salvage.
11. Apply wreck decay on its global cadence.
12. Rebuild team-shared visibility and reconcile fog memory.
13. Determine victory or draw from surviving, non-resigned teams.

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

Turn-limited aircraft fly heading-first: only the heading steers, at most
`turn_rate` compass steps per tick, so every waypoint is accepted inside the
kind's turn-acceptance ring rather than at an exact center. Every turn is a
committed arc of one fixed radius, and the simulation reasons about that arc
against the map's flight envelope in fixed point. Steering takes the shorter
rotation only when the arc it sweeps stays inside the world and ends in a state
the airframe can still be flown out of, otherwise the longer one. A wall reflex
banks the aircraft away whenever one more straight tick would leave no such arc.
A committed airframe never stops: without a path it orbits on the bank whose
fitting arc is longest, tangent to the point where the route ran out, and if it
is ever pressed into the envelope it slides along the boundary while turning
back in. A step into a Peak drops the route so the brain replans from the actual
position, while the airframe slides along the face.

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
escape path while preserving its order and work progress.

Approaching a footprint orders passable doorsteps in the body's local approach
frame, then uses an owner-local unit rank to spread equivalent workers.
Production orders spawn doorsteps in the producer's radial frame around the map.
In both cases, dot and cross products replace an absolute scan direction, so
half-turned producers and workers receive corresponding geometric orderings.

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
a finite load, and deposit at a Foundry. Destroyed eligible entities leave
decaying wreck salvage; wrecks do not block movement. Recurring economy runs in
the production phase: Reclaimers and Refineries pay on their cadences, restored
Extractors provide fixed remote income, and a completed same-owner Foundry
within the support radius raises an Extractor's fixed yield without stacking.
Completed Foundries also provide the baseline drip and a finite recovery
entitlement for a stranded seat. Crucibles consume nearby wreck salvage for
income. These are ordinary authoritative rules, not shell conveniences.

Extractor frames are immutable authored map features. Only an Extractor may
claim one, other foundations cannot cover one, and destroying an Extractor
reveals the same frame for another claim. Support is computed directly from the
two completed building footprints, so it adds no serialized connection or hidden
ownership state.

An accepted immediate build pays for and places an unfinished site at partial
hp. A non-stealthy footprint blocks ground from that command onward; the buried
Scuttle Charge is the deliberate exception. Harvesters raise the site over time.
A deferred build, used to claim remembered ground, instead installs a `Found`
program. The worker walks there, then proves the strict placement predicate with
current sight before payment and placement. A matching paid site can be joined
without charging twice. Hidden state cannot alter the earlier intent verdict or
preview; the final authoritative claim may still stall if the ground is taken
when the worker arrives.

An unfinished tier-zero site decays on a fixed cadence only when no living own
construction-capable worker has an active or queued commitment to build it. An
upgrade pays up front and takes a completed building offline as a committed site
on its new tier. The building refits itself at one progress tick per simulation
tick: it cannot be accelerated by workers, paused, cancelled, or abandoned to
decay. Its hp gain and completion use the shared damage-first work resolver, so
lethal fire wins a completion-tick tie and nonlethal damage remains when it
returns to service.

Cancelling an unfinished paid site returns value proportional to its remaining
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

Targeted attacks require current team sight. Shared allied sight can spot for a
long-range weapon, but a remembered building or unidentified radar contact
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

The bot `Observation` copies both masks in canonical row-major order. Policies
therefore distinguish current sight from remembered terrain without consulting
authoritative state; seat orientation transforms both masks with the rest of the
observed world.

The maintained player-facing controller also receives a `PublicMapBriefing`
derived from the final authored `Scenario`. It contains static terrain,
Extractor frames, initial scrap locations and amounts, teams, and each seat's
starting Foundry anchor. These are the same facts available through the
pre-match map and roster: a starting anchor is a reconnaissance prior rather
than a current enemy contact, and an initial resource amount says nothing about
later depletion. The briefing is immutable, stays separate from
`StrategicIntelligence`, and is transformed once into the same latched seat
orientation as the dynamic observation. The profile-free Overseer receives no
briefing.

Enemy buildings remain as last-seen ghosts until their footprint is observed
again. Scrap and wreck amounts likewise freeze at the last visible value. Arrays
add sorted, deduplicated radar contact tiles outside true sight; a contact
carries no owner, type, program, or persistent memory. Salvage-relevant hostile
incidents, such as a Harvester hit or an allied loss, remember only the victim's
tile for a bounded caution period, never the attacker's identity or location.

All allegiance checks route through normalized team ids. Teammates share vision,
cannot target one another, and win or lose as a team. Resignation makes a seat
command-ineligible and removes its Foundries from victory accounting; its
remaining machines continue as autonomous remnants.

Bots live outside `State::tick`. A bot reads a state-derived observation and
emits ordinary `PlayerCommand` values, which the shell or runner records before
the simulation sees them. A configured seat carries one strict `BotConfig` with
a difficulty, stance, and personality seed. `seat_bots` passes that exact setup
and one shared immutable scenario briefing to the fog-honest `Brain::scripted`
controller. Resume rebuilds that briefing from the scenario embedded in the
replay before fast-forwarding controller memory, so it adds no hidden save state
or ambient input.

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
allied relief investment; fortification changes turrets, mines, and defensive
reserve; greed changes worker targets and renewable-expansion payback appetite;
and guile changes raid size, timing, withdrawal, and some mine or
airborne-screen emphasis. Every adaptive identity receives one perimeter turret
after locating the enemy and completing its protected opening core; only an
observed raid unlocks the remainder of its fortification target. Before that
core exists, a current visible threat may justify one matching emergency Turret,
while emergency Flak requires a current visible aircraft capable of attacking
ground. A pure air-to-air flyer cannot unlock that exception. A player-facing
controller requires this actionable air evidence before investing in flak, so an
anonymous radar blip cannot turn a small seeded preference into an opening
economy cliff.

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
credibility watch admitted at such a boundary may persist while its exact
assignment remains available even if the opening-core gate closes later. It does
not become a new relief operation until admission reopens, and the fighters
explicitly held home by that watch continue to count toward the protected core.

The player-facing controller distinguishes current sight from remembered
evidence in `StrategicIntelligence`. Persistent planners retain phased air,
lift, raid, and allied-relief operations across decisions. They name exact units
and budget committed scrap before `UtilityPolicy` fills the remaining economy,
production, defense, support, and combat work. The shared `Executive` owns the
exact-unit bookkeeping and lowers every intent into ordinary candidate commands.
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

Adaptive production first projects an ordinary core in Sentinel-equivalent
ground strength. It HP-weights live Sentinel, Warden, and Breaker hulls, counts
queued units and orders already planned during the same decision exactly once,
and excludes exact persistent-operation reservations without excluding ordinary
Executive armies or units merely held as a Team operation's home watch. It then
fills shallow Foundry queues breadth-first before discretionary production can
spend the remaining bank. Raiders, artillery, anti-air, support, and actual
outbound persistent-operation reservations do not stand in for that line.

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
existing front-slot Sentinel can complete before a deferred founder pays and
therefore does not satisfy that condition. An unpaid deferred project keeps the
exact reserve as bank escrow through its walk; once it becomes a paid site, the
reserve returns to shallow production before another voluntary project. Losing
enough core strength reapplies the same gate.

Generic production may buy a shallow defensive interceptor, but bomber and
ground-attack-air cohorts belong to persistent operations; while an air or lift
plan has outstanding factory work, that plan owns Airworks capacity. Support
production keeps one baseline Tender and adds another, up to the seeded ceiling,
for each distinct currently wounded ground combatant reachable from a Tender or
Fabricator. Live, queued, and same-think Tenders count once. The profile-free
Overseer retains its legacy order.

On severed ground, wealthy bots may run two independent operations. The air
planner builds a screen and bomber wing, scouts the route, attacks currently
visible flak along it, and then commits against a current objective. Its force
targets can grow with newly observed wealth and roster strength during Recon and
Assemble, then freeze when suppression begins. The lift planner likewise grows
its payload and matching Skyhook target during Provision, then freezes exact
manifests when Boarding begins. Carrier demand follows payload and usable
landing capacity rather than an arbitrary controller cap, while a ground-capable
reserve remains at home instead of being stripped into a bulk lift.

Before a remembered objective is reacquired, an unadmitted Recon operation may
hold exactly one Skyhook's cost out of otherwise uncommitted scrap. It does so
only for a built, non-expired contact when the bot has a completed Airworks, a
transportable payload, no live or queued usable carrier, and optimistic routing
still proves the pickup and objective ground-disconnected. This is a prospective
capital reservation only: it neither creates a lift nor claims a payload or
queues a carrier before current sight.

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

The separate utility scouting channel may fund its first dedicated flyer when a
public-start route is currently disconnected, when no current unit can perform a
contested-region sweep, or after a ground probe proves that reconnaissance must
cross severed terrain. The first two demands are recomputed from current
knowledge and eligibility; only an actual failed or unsafe ground probe is
persistent. If a dispatched flyer dies, the channel releases its Airworks claim
and capital and stays suspended until actionable current enemy sight first goes
dark after the loss and later returns. Persistent sight, remembered ghosts, and
cross-sight between opposing dedicated scouts cannot restart the replacement
cycle.

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

Player-facing static-defense construction uses one fog-honest strategic site
scorer for Turrets, Bastions, Flak Turrets, Scuttle Charges, and Barricades. It
values owned production, technology, support, renewable economy, and active
resource work; then intersects credible hostile approaches with each kind's
actual weapon, spotting, trigger, or path-disruption geometry. Current contacts,
remembered enemy sites, and uncleared public starts form descending evidence
tiers. Existing defenses reduce marginal value, while unfinished sites reserve
coverage without pretending to fire. Candidate footprints must preserve builder
access, egress, and resource routes. Exact builder-route prediction combines the
public static terrain briefing with fog-honest observed dynamic blockers; public
resource priors are not treated as live obstacles. The frozen Overseer retains
its legacy placement rules.

The pre-core emergency path is deliberately narrower than that full scorer. It
uses only a current visible armed ground threat for a Turret or a current
visible ground-attack aircraft for Flak, places only the matching defense, and
cannot inherit threat authority from pure air-to-air aircraft, memory, public
starts, radar blips, or raid history.

Arrays use a separate player-facing sensor-site scorer because information
coverage is not weapon coverage. Candidate sites extend up to the Array's radar
radius from the starting Foundry, preserve ordinary placement, producer-egress,
and active resource-access rules, and bind the exact route-capable builder
proven through public static terrain plus observed dynamic danger. The scorer
first extends radar area not already supplied by an allied Array, then retains
usable in-map coverage; off-map tiles and Peaks contribute nothing because no
unit can occupy them. Current contacts, remembered contacts, and uncleared
public starting priors break otherwise equivalent sites toward credible hostile
approaches. Compact maps may use a partial radar disc. The profile-free Overseer
retains its legacy first-valid placement scan.

The player-facing budget counts each unique deferred construction claim until
its site is paid and stops voluntary repair programs that could drain that
commitment. Player-facing Foundry expansion has no count ceiling. It ranks every
exact legal site by bounded post-construction payback from newly supported owned
Extractors, Foundry drip attached to an external objective, and shorter hauling
for currently visible scrap. Hauling value uses public-ground route distance
while avoiding observed dynamic danger rather than geometric distance. Public
unbuilt Extractor frames remain scouting priors rather than live capital value.
Greed and genuinely uncommitted scrap extend the forecast without changing
capability.

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
A generic frontier nearer to a known enemy Foundry than to any projected own
Foundry is not eligible, and only one unpaid Foundry claim may exist. This is
controller discipline, not simulation escrow: automatic Repair Bay pulses
continue to follow the ordinary bank rules. The profile-free Overseer retains
its frozen legacy cap, policy, and lowering order. The simulation's command
layer remains the final legality authority. `Brain::overseer` is a separate
profile-free QA anchor.

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

| Contract                                           | Primary source                                                                                                                                                                 | Focused evidence                                                                                                                    |
| -------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------- |
| Scenario build and authored map                    | `sim/src/scenario.rs`, `sim/src/map.rs`                                                                                                                                        | inline module tests, `sim/tests/pits.rs`, `sim/tests/extractors.rs`                                                                 |
| State, hashing, validation, and teams              | `sim/src/state.rs`, `chassis/src/hash.rs`                                                                                                                                      | `sim/tests/state_integrity.rs`, `sim/tests/determinism.rs`, `sim/tests/teams.rs`                                                    |
| Placement, deferred founding, and upgrades         | `sim/src/state/placement.rs`, `sim/src/tick/commands.rs`, `sim/src/tick/brain.rs`, `sim/src/tick/brain/economy.rs`                                                             | `sim/tests/behavior_construction.rs`, `sim/tests/extractors.rs`, `sim/tests/upgrades.rs`, `sim/tests/foundries.rs`                  |
| Tick scheduling, production, cleanup, and charges  | `sim/src/tick/mod.rs`, `sim/src/tick/production.rs`                                                                                                                            | `sim/tests/behavior_rules.rs`, `sim/tests/behavior_economy.rs`, `sim/tests/mines_015.rs`                                            |
| Command vocabulary and set semantics               | `sim/src/command.rs`, `sim/src/tick/commands.rs`                                                                                                                               | `sim/tests/command_canonicalization.rs`, `sim/tests/fuzz.rs`                                                                        |
| Unit programs, routing, movement, and collision    | `sim/src/tick/brain.rs`, `sim/src/tick/brain/locomotion.rs`, `sim/src/tick/movement.rs`, `chassis/src/path.rs`                                                                 | `sim/tests/behavior_movement.rs`, `sim/tests/movement_lab.rs`, `sim/tests/peaks.rs`, `sim/tests/pits.rs`                            |
| Boarding and unloading                             | `sim/src/tick/brain/logistics.rs`                                                                                                                                              | `sim/tests/transports_015.rs`                                                                                                       |
| Harvesting, income, salvage, and repair            | `sim/src/tick/brain/economy.rs`, `sim/src/tick/production.rs`                                                                                                                  | `sim/tests/harvest_zones.rs`, `sim/tests/salvage.rs`, `sim/tests/repair_unit.rs`, `sim/tests/repair_bay.rs`, `sim/tests/smelter.rs` |
| Weapons and simultaneous resolution                | `sim/src/stats.rs`, `sim/src/tick/brain/combat.rs`                                                                                                                             | `sim/tests/behavior_combat.rs`, `sim/tests/combat_edges.rs`, `sim/tests/shells.rs`, `sim/tests/peaks.rs`                            |
| Fog, memory, radar, and stealth                    | `sim/src/vision.rs`, `sim/src/state.rs`                                                                                                                                        | `sim/tests/bot_brain.rs`, `sim/tests/bastion_acquisition.rs`, `sim/tests/mines_015.rs`                                              |
| Bot knowledge, profiles, and fair difficulty       | `sim/src/bot/briefing.rs`, `sim/src/bot/observation.rs`, `sim/src/bot/intelligence.rs`, `sim/src/bot/orient.rs`, `sim/src/bot/profile.rs`, `sim/src/bot/difficulty.rs`         | inline module tests, `sim/tests/bot_brain.rs`                                                                                       |
| Bot playbooks, routing, reservations, and lowering | `sim/src/bot/strategy.rs`, `sim/src/bot/lift.rs`, `sim/src/bot/raid.rs`, `sim/src/bot/team.rs`, `sim/src/bot/routing.rs`, `sim/src/bot/utility.rs`, `sim/src/bot/executive.rs` | inline module tests, `sim/tests/bot_policy.rs`, `sim/tests/scripted_bot.rs`                                                         |
