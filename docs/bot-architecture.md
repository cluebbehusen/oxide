# Bot architecture

This document describes the maintained fog-honest controller and its ownership
boundaries. [Bot strategy](bot-strategy.md) defines desired behavior;
[simulation architecture](simulation-architecture.md) owns authoritative rules.
The [scripted-bot skill](../.agents/skills/scripted-bot/SKILL.md) owns
procedures and targeted regression guidance. Schema details live with their Rust
types.

## Crate boundary

`oxide-bot` depends on `oxide-sim`, never the reverse. Simulation owns the
observation schema and authoritative fog filtering; bot owns derived caches,
profile resolution, memory and decisions. Shared command geometry stays with the
rules so prediction and execution use the same tie-breaks.

`Brain` accepts observations. `SeatBot` is the state-aware host adapter, gating
finished matches, cadence and resignation before invoking the decision core. The
existing host worker pool captures each due seat's observation on its worker and
collects commands in canonical seat order. No observation prepass serializes
that work, and policy does not create nested workers. A bot-only source edit
does not invalidate the simulation crate's compiled code.

## Controller and profiles

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

## Knowledge and allocation

The player-facing controller distinguishes current sight from remembered
evidence in `StrategicIntelligence`. Persistent planners retain phased air,
lift, raid, and allied-relief operations across decisions. Every maintained
controller owns all four planners; idle operations are optional, planners are
not. `allocation::admit_decision` owns their advancement, shared portfolio
settlement, lower-priority operation admission, and final utility grant. `Brain`
supplies oriented evidence and lowers the admitted intents without
reconstructing funding policy.

Planner decisions keep held scrap separate from immediate producer requests;
their current commitment is the reserve plus the ordinary purchase costs.
Allocation projects those exact requests after previously accepted producer
commands. Admission freezes the operation's charged capital before combining its
commands with purchases funded by other portfolio owners, so command insertion
or replacement cannot charge the operation twice. Queue timing stays with the
shared resource projection rather than a second planner-side schedule.

The portfolio remains one `AllocationSession` transaction. Subsequent lift and
raid admission preserves the existing priority tiers and uses explicit remaining
actor, capital, and producer access. Fresh lift work is prepared on a candidate,
validated as an exact `ClaimBundle`, and committed only after its ownership and
current-bank bounds hold. No live lift is mutated and then rolled back for a
fresh-admission failure. Retained paid occurrences and future schedules keep
their existing owners and deadlines. A `UtilityGrant` carries the final
protected units, core exclusions, current commitment, future producer lanes, and
retained construction handoff into `UtilityPolicy`.

Admission remains ordered within a seat: later grants depend on earlier exact
claims. Independent seats use the existing bounded executor. Shared immutable
query inputs support parallel evaluation, but same-seat planning allowances and
ownership cannot depend on worker completion order.

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
Connected operations retain their objective, admission priority, admissible
minimum force and preparation deadline. Their unpaid production is reconstructed
from current route-capable live providers, paid queues and eligible factories.
Shared allocation schedules that demand flexibly on each decision; an earlier
quote's factory or enqueue time is not a persistent obligation. Marginal growth
above the admissible minimum stays optional and is adjudicated afresh on each
decision rather than promoted to mandatory debt, so a revision may field a
smaller opportunity-scaled force than an earlier decision funded. Only purchases
emitted now enter the operation's paid ledger. Completed entries leave that
ledger, so they cannot claim later ordinary work of the same kind. Blocked
production retains ownership beyond its predicted completion until the queue
advances. Emergency economy recovery asks only whether remaining demand still
needs scrap, so any accessible paid queue satisfies it regardless of which
program bought that work. `RetainedWork` owns that reconciliation and returns
the claim snapshot, imported capacity, saved expansion, active revision, and
admission guards used by fresh preparation. It selects the retained
Foundry/island/lift order once and refreshes planner claims at the existing
advancement boundaries. Standing-army ownership is imported after operation
advancement and before a trailing Foundry. Equal admission ticks preserve
island-before-lift and operation-before-Foundry ties. Repair renewal finishes
before fresh support quotations.

Lift, connected, and connected-revision funding share one conflict/retry path.
Only a production conflict attributed to that exact retained owner can trigger
its recovery. The path first defers an eligible younger or equal-tick Foundry
and retries with a recomputed horizon; deferred planning and another owner's
failure do not prove this owner unfundable. Recovery remains domain-specific and
preserves its existing live-member, paid-work, and deadline rules. If a quoted
connected revision recovers, fresh preparation explicitly discards that quote,
rederives standing work from current ownership, and clears the dependent
economic alternatives. The two original rollback checkpoint boundaries remain
unchanged. The historical Airworks-capacity trace field remains zero; retained
preparation no longer carries an inert reserve for it.

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
search runs through retained deterministic work slices. Connected package
feasibility uses that same funded FIFO service, without a separate unbounded
structural assignment search. Cheap necessary capacity bounds screen speculative
rosters; passing them is not a schedule witness. A depleted allowance defers
work rather than proving infeasibility or cancelling an existing package.
Producer eligibility is prepared once per distinct unit kind in each preflight.
Input projection, claim construction and window-bound preparation are
synchronous, so the search allowance alone is not a wall-clock bound on a
controller tick.

Mandatory claims are staged together before validating their complete producer
schedule. A fixed job may depend on an earlier job owned by another obligation;
an isolated prefix is not sufficient evidence that its retained timing fails.
Future transport demand and active revision additions use bounded production
preflight before joining allocation. Their unpaid assignments remain flexible
until the complete portfolio is chosen, so a compatible capital investment may
still move their payment time. Pending new work does not revoke prior orders.

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

Standing-force preparation receives explicit observed, ownership, and demand
inputs. Repair, protection, and raid work belongs to the preparation phase,
rather than mutable session scratch. It enumerates absent, minimum, and marginal
connected-operation ownership in order, deduplicates equal live-unit and paid
production inputs, then evaluates each distinct input serially with local
scratch. Paid production preserves queue multiplicity. Results return in the
original context order; economic capability demand comes from the first context.
Recovery rebuilds current ownership inputs before deriving replacement work.

Accepted payloads retain the exact site, builder, objective, force membership,
unit kind, and current purchases selected for their domain. A defensive payload
includes the scorer-selected role and footprint, its route-proven builder, and
its quoted opportunity evidence. Commitment does not rerun domain ranking or
placement. A connected package may add the largest feasible marginal extension
only from the capacity left after its minimum and any compatible expansion,
defense, or standing-force purchase. Any malformed input or failed exact commit
freezes residual spending for that decision and restores speculative planner
state; the decision trace records the allocator result or coordinator failure.
Lower-priority lift and raid work then passes through the same admission
pipeline with explicit remaining grants. Future producer reservations prevent it
or `UtilityPolicy` from occupying an accepted lane during that decision.
Connected future rows are scheduling evidence; retained Lift, reconnaissance and
standing-force bookings still have fixed timings. Advancing a Lift projects its
fixed work jointly with flexible connected demand rather than validating a lane
in isolation from the work that precedes it.

Resolution returns either an exact settlement or a failure; deferred refinement
may first try a portfolio containing only retained commitments. If retained
connected demand still needs an unpriced flexible schedule, the transaction
freezes instead of discarding that demand and exposing its budget to fresh
spending. Planning progress survives this rollback. Commit adapters return
errors directly and stop at the first rejection. Partially produced commands and
budget effects stay private to commitment and are returned only on success. One
outer boundary freezes spending and restores ownership on failure. Planner
checkpoints precede active-work advancement; the policy checkpoint follows
reconnaissance and support observation. Restoration retains observed outcome
journals, unfinished planning, and maintenance commands from that observation
phase. It does not provide rollback after a panic.

`UtilityPolicy` separates three controller-owned lifetimes. `PolicyState` holds
decision-relevant memory and commitments, including work history observed before
allocation and dispatch records written during commitment. A `PolicyCheckpoint`
captures only that state and restores it at the existing rejection boundary.
`PlanningWork` owns deterministic allowances, unfinished jobs, and refinement
cursors; rejection neither refunds work nor discards progress. `PolicyQueries`
owns recomputable navigation, resource-access, egress, danger, harvest-service,
and expansion answers. These caches survive rejection; producer-egress answers
retain at most 256 planned layouts per base geometry, and the other services
keep their existing bounds. Full-controller cloning still copies all three
owners.

Retained query answers are keyed by their effective inputs, independently of
policy checkpoint identity. Resource access includes worker targets and eligible
resource tiles, with current amounts repriced on retrieval. Path queries include
the movement surface and hypothetical footprint. Egress includes retained
foundations after same-decision cancellations. Danger, harvest service, and
expansion fields distinguish their threat, terrain, drop-off, and source inputs.
Warm answers after rejected speculation must equal cold answers for both
restored and newly observed inputs; cache warmth cannot grant additional
planning work.

Brain lends immutable `DecisionEvidence` to allocation and residual utility from
its current battlefield assessment and experience. Oriented Executive missions
are scoped to residual execution; mission, unavailable-unit, enlisted-unit, and
relief slices are borrowed through `GroundMissionInputs`. Independent utility
calls explicitly omit mission ownership. No previous decision's stored input
selects the next call's execution path.

This ownership boundary leaves independent computations able to borrow shared
evidence and use private scratch. Strategic mutation and planning admission
still run in deterministic order. Existing parallel seat execution remains
unchanged; sharing a planning allowance behind a lock would not make same-seat
scheduling deterministic.

The Foundry commitment component owns accepted identity, fixed funding premises,
recovery, and dispatch acknowledgement. Allocation validates and funds its exact
claims and emits construction only from current capital. Its residual handoff
protects the saved capital and construction channel without acquiring the
builder or site again. Late safety evidence may cancel an unpaid plan or retain
or recover it within bounded recovery, but cannot select another plan, fund a
purchase, or emit construction. An exact command surviving Executive lowering
releases the lease; refused or displaced intents keep their identity for the
next decision. An observed transition into paid construction or founding also
retires the unpaid saving without canceling that work.

Residual utility consumes explicit upstream funding and the Foundry handoff.
Starvation and salvage use the fixed admission bank; distinct retained deferred
foundations and protected Foundry capital reduce the later spending allowance.
Shared allocation owns exact unit, builder, site, and producer claims. Residual
channels respect reservations, same-think intents, and accepted producer lanes.
Opening worker and combat-core purchases, residual utility, and strategic air
scheduling share immediate queue accounting. It combines paid queue depth with
staged appends and checks accepted future reservations without mutating the
observation. Domains retain purchase priorities, spending limits, queue-depth
limits, and producer ordering; immediate accounting does not rerun horizon
planning or spend forecast income. The observation remains immutable: explicit
foundation cancellations determine available workers, projected sites,
placement, and producer exits. Cancellations do not release paid sites or queued
programs. Same-decision stop commands still reserve their workers against
construction. The shared `Executive` owns command-lowering bookkeeping and
converts the combined intents into ordinary candidate commands.

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

## Economic investment

Economy competes through exact worker, foundation, or self-refit alternatives.
Worker value is finite harvest output or recovery of orphaned paid construction,
net of reachable existing and queued workers. Technology and factories serve
capability demand derived before prerequisite eligibility, with construction and
production delay, missing-chain costs, and eventual capacity accounted for. Live
harvest workers pay initial travel to visible work before contributing output. A
harvest cycle is gathering, the haul out and back at full speed, and the
reversal a ground chassis makes from rest at each end: its half-turn pivot plus
the motor's ramps, from `UnitKind::ground_reversal_ticks`. When work and
drop-off share a tile, the cycle is gathering plus one deposit tick, with no
travel or reversal charge. Harvest valuation separates drop-off service geometry
from finite resource work and live or queued worker returns. One fog-honest
navigation projection supplies connectivity, danger-avoiding distance fields,
and bidirectional canonical command-route safety. Each source contributes once
to its selected service region; overlapping work tiles count once. Live workers
pay initial travel, and paid worker occurrences retain their producer readiness.
One controller-local cache retains the latest service geometry and at most 4,096
corridor answers per service region. Its exact equality key includes ordinary
passability, allowed passability, the danger mask, orientation, public map
knowledge, and completed drop-offs. Changed blockers or danger discard the old
service evidence. Resource amounts, visibility, work eligibility, live workers,
and producer schedules are evaluated afresh; cached geometry grants no resource
or worker credit. Canonical command paths beneath those corridor verdicts have a
separate navigation-owned lifetime. Their cache keys contain ordinary
passability in the command coordinate frame and directed endpoints, so changed
danger can reuse a path while checking every step against the current safety
projection. Service connectivity and danger-avoiding distances still invalidate
with danger. Concurrent air and lift demand share each Airworks lane's time
once, bounded by readiness, customer deadlines, and route reachability. Local
Foundry throughput opportunities reuse the expansion admission and security
path. Additional throughput is capped by current unprotected capital and
completed income after the candidate and its missing prerequisites are paid.
Capacity confidence and urgency come from the demand contributing its marginal
return; an already-covered current need cannot strengthen a speculative capacity
case. A proposed first Airworks tries current targets by value and regional
distance until it finds a complete connected scout, suppression, and strike
minimum. This witness excludes optional force growth and target-cluster
expansion. The hypothetical factory exists only inside this pure sizing
calculation: its construction capital and delay are removed before the ordinary
package and route checks run, and existing live units are excluded from
speculative ownership. Retained obligations must fit the post-construction
capacity before campaign and route derivation begins. The complete minimum must
fit alongside retained capital promises and producer jobs in shared allocation.
This supported investment value cannot justify duplicate Airworks, issue a
production command, or admit an operation before its real prerequisites exist.
Recurring-income investments are capped by unfunded useful work; completed
income alone supplies spendable forecasts. Self-refits own exact building ids
and withhold their offline source income separately from purchase capital.
Defensive refit valuation estimates protection at nearby asset approaches, using
actual weapon coverage, redundancy, health, and offline time. Public terrain
connectivity filters ground-threat priors; nearby current attackers prevent
refitting. This estimate does not reconstruct remote army routes or credit
distant choke-point protection. The residual technology scalar and the
operational Airworks capital tax are absent. Economic purchases keep a fixed
funding deadline separate from their return horizon. Shared allocation
rebalances their current and forecast capital alongside fixed producer payments;
a missed funding deadline releases the unpaid plan for reconsideration. Issuing
a build pays for its site immediately, including travel through fog; paid
foundations and refits follow ordinary simulation rules. Extractor development
compares explored, safe frame groups around a common Foundry site by their total
return after restoration, support, travel, and build costs. The existing
expansion security check must admit the shared support site. Only the next
restoration owns capital and a builder; later steps are re-evaluated as
construction completes, and their projected income never becomes spendable
forecast credit.

`EconomicQuotes` owns one quotation pass over an immutable observation, resource
snapshot, retained obligations, and capability demand. Capacity-Foundry fallback
and ordinary economic alternatives share its service-route cache and funding
calendar. Worker quotations, capital preparation, bounded construction-site
selection, construction valuation, upgrade valuation, and final ranking retain
their original order. Construction quotes and Extractor development share a lazy
`ConstructionChecks` context. It owns exact builder safety and travel,
resource-access checks, and future-producer exit certificates over one fixed
observation, public map, orientation, danger projection, and contested-work
memory. Eligible builders and projected worker positions remain query inputs.
Support uses the same feasibility implementation; defensive valuation adds asset
values, hostile approaches, and weapon coverage separately. Upgrade quotes
prepare defensive valuation only when needed. Policy planning allowances, site
rotation, and final combined-layout validation remain serial.

The funding calendar collects retained payments once, then prices each purchase
against its exact deadline using that immutable schedule. A new observation or
obligation set requires a new pass. Shared preparation changes derived work, not
quotation identity, funding semantics, or command dispatch.

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

## Support ownership

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
the simulation's aura when valuing new and overlapping service.

## Reconnaissance

Reconnaissance is a separate allocation domain: the controller reconciles
retained questions before proposing new work, then compares exact live
observers, paid queue occurrences, and dedicated purchases. Each accepted
question retains its consumer, evidence, goal, and useful deadline
independently. An unpaid purchase retains its exact accepted producer schedule
and current/forecast funding; only a current-funded append becomes a command.
Question-local loss cooldowns and quiet intervals permit valuable safe
reconsideration without requiring new enemy sight. Reconnaissance no longer uses
one global scout slot. A completed paid occurrence binds only a newly observed
eligible scout at its exact producer exit; a lost occurrence cannot adopt
another assignment's later newborn. A surviving observer that can no longer
arrive before its fixed deadline remains owned for recall, not recorded as lost.
Operational and question-driven scouts share exact
`(producer, kind, occurrence)` exclusions: excluded items still occupy the FIFO
lane but cannot supply another assignment. Recent objective sightings suppress
immediate repeat purchases; remembered positive buildings are not uncleared
authored starts. Existing paid observers cover overlapping point questions only
when their route and deadline serve the entire footprint. This overlap credit
never replaces contested sweeps. Full-footprint negative evidence and
contested-region sweep completion remain separate from safe return. It no longer
originates residual scout purchases. Scuttlers enter Standing Force allocation
only through a viable current raid objective's exact missing tactical pair. Paid
and serviceable live supply reduce that request; its preparation deadline is
fixed, and target loss does not revoke paid queues. Raid preparation retains
exact paid queue occurrences as allocation obligations until they bind newly
observed members at their producer exits. Residual advancement excludes other
owners while retaining the raid's own muster, and launches only against the
procurement objective. A missing paid member releases preparation into bounded
recovery without adopting a later birth. Bomber, ground-attack-air, and
transport cohorts remain owned by persistent operations. Their outstanding work
and fixed deadlines contribute economic capacity demand; they do not impose an
unowned factory reserve.

## Connected operations

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
counted again as optional bombing collateral. Minimum composition and marginal
growth each retain a width-eight beam. It ranks capped useful capability first
and uses personality to weight how otherwise competitive marginal capability is
divided between air, siege, direct strike, and attack-run bombing. Personality
never gates a provider or family. One beam slot preserves the cheapest
alternative; existing useful providers remain eligible at minimum strength, and
every extension recomputes its canonical funding order. Speculative compositions
use necessary throughput bounds; complete minima and the selected growth ladder
receive funded FIFO refinement before they are offered. The offered growth
ladder is rebuilt in that final order, so enlarging or revising a package keeps
earlier job identities and payment times intact. Fully refined comparisons
preserve the monotonic opportunity checks for more scrap, time, or completed
production capability. Fresh admission may offer a verified prefix while larger
variants remain pending. Pending refinement cannot prune an active roster: an
unfinished revision preserves the accepted operation and its exact orders.

Current connected targets are ranked canonically and admitted only with a
complete package. A route-feasible optional member of its bounded current
cluster is retained only when the complete revised package still fits the same
producer access, queues, funds, and fixed preparation deadline. The package
stores and traces the canonical anchors it actually admitted. During Recon and
Verify, negative anti-air evidence requires current visibility over every
footprint tile of every surviving admitted target; the scout focuses the first
unknown tile before the operation may commit.

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

## Air and lift operations

Air operations own one lifecycle state: watching a remembered objective,
admitted reconnaissance, assembly, suppression, verification, strike, or
recovery. Recovery carries its reason and prior assault admission; other states
derive admission directly. Diagnostic phases remain stable, with watching
reported as Recon. Phase, admission, and recovery reason cannot be mutated
independently.

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
capital floor only: it neither creates a lift nor claims a payload or queues a
carrier before current sight.

Fresh typed investments resolve before that residual Recon lifecycle. The
allocator therefore previews the exact retained or newly admissible Recon target
and raises every fresh proposal's shared minimum-residual floor by the
prospective carrier cost. The floor does not claim the bank:
`allocation::operations` confirms or releases it from the resulting planner
state and records the actual hold exactly once.

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

## Contested harvest

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

## Defensive investment

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
newly prepared bounds also reorder the remaining endpoint pairs before path
construction. Adding a blocking footprint cannot improve that bound. For long
paths, exact fields for the current footprint also prune A* branches that cannot
belong to a shortest route. Queue ordering remains unchanged to preserve the
complete route-choice key and mobile firing positions. This pruning is disabled
when the map exceeds the expansion cap or the start is blocked, preserving
capped searches and escape from blocked origins. A* still constructs every
uncached selected path. Investment scores, threat evidence, asset values, and
budgets are recomputed from the current observation rather than retained with
passability.

## Navigation ownership

`bot::navigation` owns all bot path, cost, connectivity, and distance-field
searches, including their scratch storage, cache invalidation, and retention.
Command projection preserves orientation, goal spreading, and Build doorstep
selection. Service connectivity, safe travel costs, work-distance queries, and
producer-exit certificates retain answers within their navigation contexts.

Each `Observation` owns lazily prepared navigation inputs. Its serialized
`ObservationData` remains player knowledge only; derived inputs do not affect
serialization or equality. Unmodified clones share preparation. Mutable access
invalidates it before exposing the data, including same-tick hypothetical edits
and orientation transforms. Ground and air surfaces, with and without public
terrain, share their component labels and command-frame masks across consumers.
Exploration restrictions have separate derived masks; danger and hypothetical
footprints remain query overlays. Search scratch remains query- or worker-owned.
Alternate public briefings and command orientations are checked against exact
inputs and cannot reuse an incompatible surface.

Exact component labels can also be shared across observations with identical
dimensions and complete passability masks. Each worker retains at most sixteen
such masks and 4 MiB of mask/label payloads. Cache hits and eviction affect CPU
cost only; they never change work allowances or yield points.

Emergency detection first excludes threat/asset pairs beyond its local radius,
including weapon standoff range, and stops after its first qualifying approach.
Distant threats do not require full routes to prove they are not local
emergencies. Planners supply player knowledge, safety predicates, and candidate
preferences; investment scores and service eligibility remain planner-owned.
Barricade footholds consume scalar costs, while coverage, mobile standoff, and
retreat retain canonical paths. Costs distinguish exact and bounded success from
disconnection and search limits. Detour limits still precede local-support
filtering; construction and layout checks retain their existing route contracts.
Local defensive support uses one bounded eight-step flood for every asset around
a candidate, rather than a shortest-route query per asset. Future-producer exit
checks request connectivity only; maps above the canonical expansion cap retain
the capped search contract. Repeated command-safety queries accumulate work by
endpoint. Once that work exceeds the map area, they share a field proving
whether all shortest routes avoid danger. The shared distance traversal
propagates safety along improving and tied predecessor edges, so classification
uses the same movement rules without a second sorted traversal. Ambiguous routes
still use canonical A*, and maps above its expansion cap bypass the proof. Each
immutable route projection retains at most sixteen safety fields, independently
of the directed route-answer cache. Canonical command routes and ambiguous
safety fallbacks share a worker-local path cache retaining up to eight ordinary
surfaces with 1 MiB per surface. Multiple seats can reuse their distinct
fog-honest surfaces within an 8 MiB total accounted allowance. Ground and air
share that allowance, keyed by their exact passability; danger verdicts remain
projection-local. This command cache does not enter policy rollback snapshots or
change planning allowances. Failed searches still run with their own
capped-search evidence. Producer-exit certificates first repair an intersected
route inside a small rectangle around the candidate footprint, including nearby
replacement doors or destinations when an endpoint is blocked. Producers share
one row/column index per connected component; its four live coordinate extrema
select the exact farthest destination without per-producer rankings. Failed
local repairs use the full connectivity check; a failed local detour never
proves that a producer is trapped. Long cardinal proofs first try a bounded
monotone search. A successful probe retains the canonical shortest path; detours
fall back to the complete search.

Reinforcement valuation requests scalar travel costs and visits units in
optimistic arrival order, stopping when no remaining unit can arrive sooner.
Candidate placement scans share indexed obstacle, frame, foundation, and enemy
occupancy masks for their immutable observation. Current policy reservations,
exploration, worker access, and producer exits remain separate checks. Defense
and support share resource-access evidence while blocking geometry, eligible
resource tiles, active harvest targets, and supporting Foundries are unchanged.
Scrap amounts reprice those assets without rebuilding their routes.

Each immutable campaign comparison shares ground and air connectivity, staging
choices, and artillery firing stands across target selection, producer access,
and exact group admission. Comparing another target or production mix does not
rebuild its movement projections. These caches retain the ordinary command
reachability and firing-position contracts. Firing geometry is shared by weapon
type and target; provider origins only filter reachability and order the legal
stands. Reachability stops at one witness without materializing the complete
assignment list.

Match setup prepares and shares a public terrain index for each seat
orientation, with connected components inside 16-by-16 regions and deterministic
boundary links. Region components also answer exact public-terrain connectivity
queries without a fresh map traversal. Regional distances rank strategic
targets; they do not certify live reachability, route safety, command timing, or
placement legality. Orientation and terrain changes invalidate the index
independently of dynamic navigation caches. Public distance fields reuse the
prepared terrain index to materialize passability once and use an owned
traversal that can yield after a deterministic number of queue entries. Fresh
Foundry logistics, voluntary coverage, and production refinement share a
controller-owned work allowance. Passability preparation and traversal consume
work; unfinished fields resume on later decisions and never mean unreachable.
Foundry retains four jobs. Ground coverage, air coverage, and candidate routing
each retain at most sixteen pending jobs and 32 MiB of completed field payloads.
Candidate fields include the hypothetical footprint in their key without
replacing other candidates' progress. Exact terrain and blocking changes
invalidate affected work; unfinished field jobs expire after 120 ticks without a
request. Active requests retain their progress even when small work slices need
longer to finish. Pending Foundry, ground, air, candidate-route, and production
work divide half of each decision's allowance, leaving half for current
requests. Navigation and speculative production forecasts leave one quarter of
the shared allowance for final production admission. Field requests can register
without work and receive their share on the next decision; production admission
cannot be starved by repeated navigation calls. Allocation rollback preserves
this work and the Foundry and defensive-site cursors. Weapon and Array
refinement share the allowance and retain their four-new-site limit; incumbent
validation is separate. Diagnostic counters cover these services, not
synchronous planner preparation or mandatory validation. Saved Foundry
validation remains immediate.

Fresh economic infrastructure refines two sites per building type: the nearest
builder estimate and a rotating alternative. Saved investments bypass that
shortlist. Extractor frames remain available together for cluster valuation;
infrastructure refinement does not reduce a resource group to one frame.

Voluntary Barricade routes request incremental fields before constructing their
canonical paths. A pending query preserves its site for later refinement and
never enters the route cache as an unreachable result. Endpoint evaluation order
does not depend on observational cache warmth. Completed fields prune ordinary
A* while retaining its route choices and mobile firing positions; path
reconstruction and immediate placement validation remain synchronous.

Production refinement first constructs an earliest-funded schedule, then resumes
repair if that attempt fails. Income probes seek the funding boundary inside
each job's legal enqueue window. Work charges scale with the problem's job and
producer counts. Sixteen retained tasks share background progress, expire after
120 ticks, and stop after a bounded total repair allowance. Exhaustion means
unrefined, not infeasible. Claim identities include capital reservations and
funding priorities; a retained schedule must pass current queue, deadline, and
funding checks before acceptance. Portfolio selection, standing-force wait
binding, connected growth, and prospective Airworks allocation use this service.
Complete campaign candidates and active provider forecasts use this same
production service, including queue slots, timing, current funding, and shared
work limits. Composition exploration uses shared necessary capacity bounds
instead of scheduling every intermediate roster. The separate recursive
funded-lane scheduler is a test-only oracle. A pending minimum remains a
deferred target; a pending active revision preserves its operation, paid work
and deadline. Forecasts never reserve resources themselves. Fixed obligations
reuse the same witness validator directly; they do not search for an alternative
schedule. It checks current queue state, request identity, owner order, exact
timing, and rebased funding. Unassigned mandatory purchases use the same bounded
service. If they remain pending, the session can settle only independently
committed claims, restoring the prior operation instead of accepting its
replacement and leaving new transport requests unbound. Retained connected
demand prevents this fallback from releasing unpriced commitments; that decision
freezes spending until refinement can settle. Read-only funding checks preserve
existing plans on deferred results. The exhaustive production solver is compiled
only as a test oracle.

Site refinement and prospective Airworks targets use the same bounded candidate
cursor. Airworks valuation admits two candidate factory sites and examines two
new targets per site each decision, retaining a pending or successful target for
current revalidation. Site admission is frozen for the decision and rotates
through the alternatives; a retained target keeps its site in consideration.
Pending targets expire after 120 ticks; a rotating tail continues while one
target awaits production refinement. Each controller retains at most sixteen
Airworks site comparisons. Unexamined targets provide no feasibility verdict.
Diagnostics count target evaluations separately from field and production work.

Voluntary coverage uses one reverse field per asset's destination set to serve
all threat origins. Its representative routes have exact shortest costs and
legal edges, but need not share the command router's tied-path shape. Weapon
sites score these prepared corridors, filtered to locally supported assets,
without predicting a new enemy route around each hypothetical weapon. One
coverage batch shares existing-defense and spotter geometry across candidate
sites. New coverage ranks by asset-weighted corridor span, capped at eight
unique tiles per asset, before whole-asset coverage; a newly covered edge tile
cannot earn the same preference as an extended unprotected approach. Barricade
valuation retains candidate detour costs because obstruction is its benefit.
Candidate neighborhoods use a grid union and geometry checks; existing exit
certificates provide a cheap positive ranking preference. Exact producer exits,
resource access, and builder safety are checked during site refinement. Array
candidates use the same split: indexed geometry and coverage rank the scan, with
exact exit checks confined to the four-site refinement slice and retained-site
validation. A still-valid retained Array avoids scanning alternatives.

A deferred field stops the evidence ladder; it cannot turn a current threat into
a weaker public-prior case. Direct attacks, minimum-range retreat, and firing
standoff retain their weapon rules. Emergency response, Build commands, and
candidate construction safety retain their exact route checks.

Exact Build-route checks share observed and public ground passability across
builders and reuse A* storage across candidate sites. Component labels include
the candidate footprint and additional blockers, rejecting disconnected
preferred doorsteps before A*. The most recent candidate layout is shared across
builders. Safety queries reject dangerous origins before searching. On maps
below the A* expansion cap, connectivity also identifies the selected doorstep,
allowing a dangerous endpoint to reject a route before constructing it. Larger
maps retain the capped search verdict. These checks preserve exact paths and
doorstep preference. Build and ordinary movement routes use the shared
canonical-path service, including adaptive distance fields and bounded
per-thread retention keyed by complete candidate passability. Danger is checked
again on every use; cached paths cannot certify safety. Authoritative doorstep
ranking remains query-local.

Test-only navigation work counters enforce structural regression bounds across
consumers without using wall-clock timing or affecting controller state. Query
entry points require an explicit caller purpose, which follows nested services
and retained jobs across decisions. Optional observer reports attribute cache
hits to the requesting caller and report operation-specific work units; these
counters never influence planning allowances or decisions.

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

## Expansion ownership

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
required total across decisions; allocation imports its exact claims into the
shared portfolio. The worker lease prevents scouting, repair, salvage, queued
work, unrelated construction, and implicit drafting from stealing it during
intent lowering; only the matching Foundry build may consume it. Transition,
invalidation, or an urgent opening-core shortfall releases the claim
immediately. A temporarily blocked plan gets a bounded recovery window, while a
plan that needs new protection releases its fund for that preparation before it
can be reconsidered. A generic frontier nearer to a known enemy Foundry than to
any projected own Foundry is not eligible, and only one unpaid Foundry claim may
exist. This is controller discipline, not simulation escrow: automatic Repair
Bay pulses continue to follow the ordinary bank rules. The simulation's command
layer remains the final legality authority.

## Configuration and replay

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

## Diagnostic traces

Player-facing bot decision traces are output only. An opt-in `Brain::act_traced`
call reports only facts already owned by the fog-honest coordinator while
returning the same ordinary commands as `Brain::act`. The trace recorder is
local to that call; traces are not controller memory, authoritative state,
replay input, or replay metadata. Ticks on which no player-facing decision
occurs produce no trace. Trace schema version 13 reports current scrap
separately from a bounded forecast based only on completed income sources,
together with current builder and producer capacity. Proposal and allocation
evidence records the coordinator's actual inputs and verdicts, including
economic action and defensive proposal identities, exact building claims, exact
repair ownership, refit income losses, and arbitrary-size layout conflicts,
rather than reconstructing decisions after the fact. Battlefield evidence,
Executive mission ownership, bounded episode reports, decayed preferences, and
effective allocation return are separate trace fields. Raw proposal consequence,
urgency, confidence, and safety are not rewritten to express historical
preferences.

## Controller-local battlefield loop

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

Utility work journals own dispatched harvest probes, construction attempts,
foundation watches and bounded failure exclusions. After lowering, the
controller records emitted commands in output order, converting their positions
back into the policy's orientation. An immediate retask clears the superseded
harvest probe and closes affected attempts; queued movement preserves the
current assignment. Observation advances this work once per tick before
allocation checkpoints. Residual utility planning does not repeat the audit or
maintain a separate pending-site lifecycle. Focused policy tests use this
maintained preparation and utility context inside the bot crate; controller
integration tests use `Brain` or `SeatBot`.

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

Experience subjects distinguish building and unit identities, construction and
production kinds, upgrades, harvest, and reconnaissance. Construction reports
and allocation queries share the same context constructor. Related work shares
broad doctrine preferences deliberately; matching numeric IDs or enum ordinals
do not transfer contextual credit. Trace subjects serialize their variant and
value instead of an untyped integer.

Contextual return and corroborated doctrine preferences decay toward neutral.
Each retained credit owns its strongest contextual evidence and, independently,
its strongest qualified doctrine evidence, including report identity,
confidence, completion time and score. Weaker follow-ups cannot replace either
winner after the recent report history evicts it. Evidence expires at its own
completion time; new evidence for another credit cannot renew it. Replacing
shared credit moves only that credit. Storage retains at most 64 credits per
context, 128 contexts, and 64 recent episode reports, with canonical
oldest-first eviction. Recent reports support diagnostics and approach
reconnaissance; both preference scores come from retained credit evidence.
Context scores fold in canonical completion order with saturation so
counterevidence can break an entrenched preference. Experience refreshes its six
doctrine scores after observation or a report changes retained evidence.
Proposal queries read those prepared scores rather than rescanning every credit;
contextual lookup inspects at most one bounded context's contributions. These
preferences alter candidate ranking and effective allocation return without
rewriting raw consequence, urgency, confidence, or safety. Retry records for
dispatched harvest and construction attempts expire and require fresh legal
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
