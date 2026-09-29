# Persistent operations and defense

[Architecture and ownership map](../bot-architecture.md). Applies only to
`oxide-bot`; [`oxide-opponent`](../oxide-opponent.md) has its own specification.

## Support ownership

The residual Foundry pass does not originate player-facing ordinary combat,
siege, anti-air, or Tender orders. Residual construction does not originate a
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
out-of-position protector returns with an ordinary Run order, even while busy,
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
reconsideration without requiring new enemy sight. Reconnaissance has no global
scout slot. A completed paid occurrence binds only a newly observed eligible
scout at its exact producer exit; a lost occurrence cannot adopt another
assignment's later newborn. A surviving observer that can no longer arrive
before its fixed deadline remains owned for recall, not recorded as lost.
Operational and question-driven scouts share exact
`(producer, kind, occurrence)` exclusions: excluded items still occupy the FIFO
lane but cannot supply another assignment. Recent objective sightings suppress
immediate repeat purchases; remembered positive buildings are not uncleared
authored starts. Existing paid observers cover overlapping point questions only
when their route and deadline serve the entire footprint. This overlap credit
never replaces contested sweeps. Full-footprint negative evidence and
contested-region sweep completion remain separate from safe return. Residual
production does not purchase scouts. Scuttlers enter Standing Force allocation
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
counted again as optional bombing collateral. Bounded package search ranks
capped useful capability first. Personality weights how otherwise competitive
marginal capability is divided between air, siege, direct strike, and attack-run
bombing, but never gates a provider or family. For identical evidence, more
scrap, time, or completed production cannot revoke a fully refined admission or
reduce its capped useful capability. Enlarging or revising a package keeps
earlier job identities and payment times. Fresh admission may offer a verified
minimum or prefix while larger variants remain pending. Pending refinement
cannot prune an active roster: an unfinished revision preserves the accepted
operation and its exact orders.

Current connected targets are ranked canonically and admitted only with a
complete package. A route-feasible optional member of its bounded current
cluster is retained only when the complete package still fits the same producer
access, queues, funds, and fixed preparation deadline. Admission commits the
operation to that cluster. Its owner, primary building, primary anchor,
canonical member anchors, minimum capability, admission tick, and preparation
deadline are frozen. The primary and its anchor identify the operation to
allocation, paid purchases, production search, and experience; its owner and
anchor identify it to a coordinated lift. The member set never grows. A member
leaves the operation only when current sight finds it gone; out of sight, it
stays live on its remembered contact. Traces report the admitted anchors, the
live anchors, the focus, and the anchors that sized the current package. Staging
and the strike prefer a sticky focus member, which moves during preparation only
after it leaves current sight; the scout covers every live member's unseen
footprint. During Recon and Verify, negative anti-air evidence requires current
visibility over every footprint tile of every live member; the scout focuses the
first unknown tile before the operation may commit.

Forecast income is proposal evidence, not command credit. Providers, producer
exits, artillery staging, reconnaissance, and strike routes must remain viable
through public terrain and observed dynamic blockers. Route preflight reproduces
the authoritative command geometry: seat orientation, center snapping, group
spreading, producer doorsteps, and a reachable legal firing stand for every
exact suppression `Attack` member. Snapping and spreading match exactly only for
explored goals: the simulation spreads a group over an unexplored goal once the
seat's team explores it, on ground preflight could only assume, so a projected
spread there stays approximate. Each production command still spends only the
spendable current bank, uses an exact completed producer, and must fit that
producer's conservative queue and egress bound.

During Recon and Assemble, a live member in current sight lets available
resources and uncommitted providers revise the package against the committed
members only. Members in current sight and remembered members of positive
confidence count toward target durability and value; anti-air evidence remains
current-only. A revision that cannot size every such member leaves the current
package in place. Revision never changes the commitment or extends its
preparation deadline. The operation freezes its exact assigned ids on entering
SuppressAa. Later suppression- and strike-cohort losses are measured against the
committed capability minimum; the required scout remains an exact-identity
requirement. Missing the preparation deadline enters bounded recovery instead of
extending or replacing the cohort indefinitely.

The strike attacks the best live member in current sight, preferring the focus.
When every live member has left sight, it hunt orders toward the best remembered
member if no known anti-air covers the route or approach; known anti-air there
recovers the operation as new air defense. Once current sight has found every
admitted member gone, the operation ends even if those anchors have since left
sight: before Strike, as a lost objective; after a settled strike, as complete,
with the whole cluster recorded as observed gone. Before Strike, live members
that have all been out of sight beyond the active-operation memory end the
operation as stale intelligence.

## Air and lift operations

Each air operation runs one of three plans: Reacquire watches a remembered
objective with a scout until current sight admits an assault, Island masses
aircraft against a ground-severed objective, and Connected commits combined arms
to a ground-connected target cluster.

Air operations own one lifecycle state: watching a remembered objective,
admitted reconnaissance, assembly, suppression, verification, strike, or
recovery. Recovery carries its reason and prior assault admission; other states
derive admission directly. Diagnostic phases remain stable, with watching
reported as Recon. Phase, admission, and recovery reason cannot be mutated
independently.

On severed ground, wealthy bots may run two independent operations. The air
planner builds a screen and bomber wing, scouts the route, attacks currently
visible flak along it, and then commits against a current objective. Its screen
and bomber targets are sized once at admission from renewable income, roster
strength, stance, and personality; exact members freeze when suppression begins.
The lift planner grows its payload and matching Skyhook target during Provision,
then freezes exact manifests when Boarding begins. Carrier demand follows
payload and usable landing capacity rather than an arbitrary controller cap,
while a ground-capable reserve remains at home instead of being stripped into a
bulk lift. Drop planning treats unexplored peaks as open sky. A carrier whose
drop proves sealed in flight sets its riders down as close to it as the sky
allows and returns empty; only riders that can still reach the drop join the
assault.

The wealthy island air operation also has a separate admission gate of 12
currently armed units. That standing-roster check establishes readiness to open
the operation; it neither sets nor caps the screen or bomber demand.

Before a remembered objective is reacquired, an unadmitted Recon operation may
hold exactly one Skyhook's cost out of otherwise uncommitted scrap. It does so
only for a built, non-expired contact when the bot has a completed Airworks, a
transportable payload, no live or queued usable carrier, and optimistic routing
still proves the pickup and objective ground-disconnected. The hold also
requires a successful allocation and open voluntary operations: the opening core
is ready and no economic saving is held. This is a prospective capital floor
only: it neither creates a lift nor claims a payload or queues a carrier before
current sight.

Fresh typed investments resolve before that residual Recon lifecycle. The
allocator therefore previews the exact retained or newly admissible Recon target
and raises every fresh proposal's shared minimum-residual floor by the
prospective carrier cost. The floor does not claim the bank:
`allocation::operations` confirms or releases it from the resulting planner
state and records the actual hold exactly once.

The operations choose and execute objectives independently. When both select the
same target, they exchange an explicit target-specific hold, release, or abort
signal so a lift can follow air-defense suppression without depending on it.
Signals and shared experience credit match the target by owner and anchor. A
connected operation keeps signaling its admitted primary anchor while its
tactics focus another member, and an operation whose survivors move to standby
still ends with a release or abort signal. An economy-emergency recall returns
before Lift runs, so it keeps the operation in recovery, even with no survivors,
until the next full decision settles it and hands Lift the abort. Only
targetless or safely staged Executive armies can transfer into a lift; units
holding a currently contested objective remain enlisted. Carrier production and
extra Airworks use the same ordinary queues, costs, prerequisites, and
deterministic capital ledger as all other bot production.

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

Defensive geometry shares a bot-owned route cache with economic valuation.
Retained routes are keyed by map dimensions, the complete fog-honest passability
surface, and the candidate footprint. Normal ground, hypothetical combined-build
layouts, and air have independent bounded retention; eviction or an oversized
entry falls back to search, and failed searches are not cached as unreachable.
Retained distance fields may prune searches but preserve canonical route choices
and mobile firing positions. Investment scores, threat evidence, asset values,
and budgets are recomputed from the current observation rather than retained
with passability.
