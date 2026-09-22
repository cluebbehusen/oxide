# Allocation ownership

[Architecture and ownership map](../bot-architecture.md).

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
