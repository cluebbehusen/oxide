# Persistent operations and defense

[Architecture and ownership map](../bot-architecture.md).

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
use necessary funding-time and shared-lane throughput bounds. Complete minima
receive funded FIFO refinement; explored growth candidates are then refined in
score order until one is feasible. An infeasible favorite does not discard the
other explored compositions, while deferred work keeps a verified minimum
available. The offered growth ladder is rebuilt in that final order, so
enlarging or revising a package keeps earlier job identities and payment times
intact. Fully refined comparisons preserve the monotonic opportunity checks for
more scrap, time, or completed production capability. Fresh admission may offer
a verified prefix while larger variants remain pending. Pending refinement
cannot prune an active roster: an unfinished revision preserves the accepted
operation and its exact orders.

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
