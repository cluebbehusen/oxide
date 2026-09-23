# Navigation ownership

[Architecture and ownership map](../bot-architecture.md).

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
120 ticks, and stop after a bounded total repair allowance. Exhausted tasks
release search storage and report terminal unknown work separately from
resumable work; neither result proves infeasibility. Changed funding, queue
availability, producer eligibility or horizon permits a fresh attempt at
exhausted work; shifting otherwise identical observation-relative inputs does
not. Claim identities include capital reservations and funding priorities; a
retained schedule must pass current queue, deadline, and funding checks before
acceptance. Portfolio selection, standing-force wait binding, connected growth,
and prospective Airworks allocation use this service. Complete campaign
candidates and active provider forecasts use this same production service,
including queue slots, timing, current funding, and shared work limits.
Composition exploration uses shared necessary capacity bounds instead of
scheduling every intermediate roster. The separate recursive funded-lane
scheduler is a test-only oracle. A pending minimum remains a deferred target; a
pending active revision preserves its operation, paid work and deadline.
Forecasts never reserve resources themselves. Fixed obligations reuse the same
witness validator directly; they do not search for an alternative schedule. It
checks current queue state, request identity, owner order, exact timing, and
rebased funding. Unassigned mandatory purchases use the same bounded service. If
they remain pending, the session can settle only independently committed claims,
restoring the prior operation instead of accepting its replacement and leaving
new transport requests unbound. Retained connected demand prevents this fallback
from releasing unpriced commitments; that decision freezes spending until
refinement can settle. Read-only funding checks preserve existing plans on
deferred results. The exhaustive production solver is compiled only as a test
oracle.

Connected package selection retains a per-objective candidate cursor separately
from production tasks, so task retirement and expiry cannot repeatedly restart
ranking at the first candidate. It examines two new ranked alternatives per
decision while rechecking a retained candidate. Indices are traversal hints:
current observations rebuild every composition, and only a currently validated
schedule can select it. Up to sixteen objective comparisons survive checkpoint
restoration; inactive comparisons and incumbents expire after 120 ticks. A
proved minimum remains available while marginal alternatives are unresolved.

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
