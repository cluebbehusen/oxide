---
created: 2026-09-12T09:35:38
updated: 2026-09-12T14:02:52
---

# Movement research closure

## Goal

Preserve the complete movement investigation and the decision to stop it, so
another agent can retain the useful presentation work without repeating the
rejected controller research.

## Decisions

- On September 12, 2026, Connor chose to stop broad movement-controller
  research. Retain the approved turning, braking and
  differential-tread/animation improvements. Accept main-style lateral collision
  correction, including its unrealistic slide, to preserve useful traffic flow.
  Strictly non-sliding motion is no longer the product requirement for this
  work.
- Preserve the experiments and their evidence, but do not promote the
  full-journey rewrite, contact-steering candidates, or later ordering
  coordinator on the strength of their best isolated case. Do not resume their
  unfinished research checklists without a new, bounded request.
- This is a product and cost decision. We did not prove that convincing
  non-sliding RTS movement is impossible, or that all of main’s throughput
  advantage is caused by sliding. We failed to demonstrate a replacement whose
  responsiveness, generality, CPU cost and native appearance justified more
  work.
- If movement work is revisited, start from one concrete bad replay on the
  retained controller, set a finite budget and stopping condition, and judge
  per-unit responsiveness and native play. Another open-ended architectural
  sweep is not the next action.
- On September 12, Connor authorized assembling the retained improvements on
  main with sliding preserved. This supersedes the documentation-only boundary
  for subsequent assembly work; the rejected research remains archived.
- At research closure, the retained combination was not yet assembled: disabling
  the ordering spike selected the full-journey engine, not main. The subsequent
  assembly below starts from main and selectively ports the presentation and
  motor behavior. It does not switch off a feature in the rejected engine.
- The initial closure changed documentation only. The subsequent authorized
  assembly uses a new main-based worktree. Original research trees and the
  primary checkout remain preserved; commits, pushes, compatibility changes and
  fixture blessings still require their separate authorizations.
- The assembly branch now carries the continued canonical copy of this note. The
  earlier research notebook remains a historical snapshot with a pointer here.
- Connor accepted the assembled movement after playing the 40-unit sandbox: the
  improved turning animations help substantially and the result is fine for now.
  This is acceptance of the retained sliding controller, not a reopening of
  strict non-sliding coordination research.
- Connor authorized refreshing the affected fixtures while keeping main’s
  version. Freshly fetched main remains at 73a58fe with version 0.16.0, so the
  assembly uses an explicitly approved same-version bless at 0.16.0.
- After reviewing the paired match results, Connor approved retaining the three
  review fixes and increasing the balanced-mirror completion ceiling from 30,000
  to 50,000 ticks. The local stationarity contracts are correct; the longer
  match showed continued combat and economic activity rather than a permanent
  stall. Broader pacing and bot forecasting remain separate work. The existing
  fixture approval remains at version 0.16.0.

## Findings

### Read this before using the old results

This note reconstructs the original implementing session, the independent Python
research, the Rust integration, and the final service-allocation experiments.
Historical outcomes below come from the archived reports, raw result ledgers and
source history. They were not all rerun for this documentation task. The first
session continued after its initial handoff; its later contact-steering work is
included here.

There were three materially different references:

| Reference                  | Identity                                                                   | What it actually runs                                                                              |
| -------------------------- | -------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| Sliding main               | Frozen `c8ef5f7`, with comparison harness at `fcdbe44`                     | Ground path following plus bounded lateral/radial collision displacement outside normal propulsion |
| Original movement research | `cjl/spike/collision-presentation`, `105db6c`, plus captured dirty changes | Physical turning/braking, retained complete journeys, service reservations and repair              |
| Later integration          | `cjl/spike/movement-order`, signed main merge `11616c0`                    | The preceding research engine when the feature is off; the new ordering spike when it is on        |

The first session’s ordinary comparison was **800 scrap on sliding main versus
680 on its full-journey candidate**. The later integration’s ordinary comparison
was **680 on that full-journey reference versus 140 on the new coordinator**.
The 680 result must never be relabeled “main with sliding.” Other fixture
families also differ: a scenario called “parked” in one archive can contain
later move commands in another. Match fixture hashes and command logs before
combining numbers.

The earlier Python synthesis, `CURRENT-REPORT.md`, described the architectural
research question as resolved. That was written before the unsuccessful Rust
integration. Its measured sandbox results remain evidence; its recommendation is
superseded by the stopping decision above.

### 1. Original session: presentation and successive Oxide controllers

#### Presentation and permissive movement

The visual problem became obvious after the art improvements: hulls slid
sideways through collision correction, avoidance repeatedly invoked braking and
pivoting, and tread animation made motion mismatches visible. The desired scenes
included recurring twelve-worker traffic, idle units between heap and Foundry,
compact mixed-size arrivals, narrow doors, and useful firing positions. Modest
slowdown was acceptable; long stalls and stranded units were not.

The first session made tread travel depend on actual signed hull displacement
and differential turning, including opposite belt motion during a pivot. A
discrete three-frame tread attempt still aliased and read poorly; continuous
tread shoes replaced it. Heavy tracked units were included. These presentation
changes are distinct from accepting the navigation algorithm that happened to
drive them. `31c9cd1` introduces continuous tracks together with steering, so it
is not a presentation-only cherry-pick. `7be1203` extends the actual-motion belt
rendering to heavy vehicles. Relevant source is `shell/src/track_motion.rs` and
the track/motion rendering modules; the contact-steering archive also carries
presentation separately from its simulator changes.

Main’s collision solver first permits ordinary propulsion, then applies bounded
lateral/radial corrections to overlapping bodies. Correction has its own
per-unit budget across relaxation passes and does not require steering through
the added displacement. That gives traffic a cheap way to untangle, at the
visible cost of sliding. No clean ablation isolated its share of the total
throughput advantage.

#### Local steering and increasingly specific coordination

The early controller combined heading-constrained predictive steering,
stationary-body route repair and waypoint lookahead. It then accumulated
retained group destinations, work positions, leases, passage
direction/admission, off-axis waiting, shared rally reservations and courtesy
relocation of idle friends. These repaired particular failures but produced new
interactions:

- Waiting positions near the shortest route could form a wall across the worker
  corridor.
- Treating a neighbor’s advisory route as guaranteed future occupancy caused
  reciprocal waiting after those routes changed.
- Repaired routes were ineffective if lookahead cut through the same bodies
  again.
- New groups competed with earlier parked groups; accepting the outside of a
  goal region could build an impenetrable perimeter.
- Passage waiters, temporary evacuation and actual idle units needed different
  treatment. Relocation could overwrite suspended work or foundation-exit
  intent.
- Worker improvements did not preserve combat participation: an early comparison
  had 5 of 20 close attackers contributing versus 20 on main.

The architecture was becoming the collection of special cases Connor wanted to
avoid. Some bookkeeping and geometry fixes were real, but the whole controller
was not validated by those fixes.

#### Shared physical integrator and short-horizon reservations

The redesign used continuous destination regions for arrivals, work and fire;
shared static guidance; and one fixed-point physical step for prediction and
execution. The initial planner looked 40 ticks ahead and committed four ticks,
only two seconds of lookahead at 20 Hz. It supported real pivoting,
acceleration, braking and reverse.

Several defects were isolated here: full-speed/brake-only choices caused
stop-start following; admitting lower speeds improved it. A unit needed
permission to turn during a wait rather than paying turning time again at
departure. Early interruption safety required reserving braking from each
proposed step, not only from the horizon endpoint. Yielding needed a reachable
vacant endpoint, retained intent, and route-based progress. Firing units could
make room only while preserving range and line of fire.

Numerical geometry also mattered. An off-center Avalanche could stall at a
one-tile opening without any other traffic. Squared distance could round a
nonzero final offset to zero; route simplification could discard a required
entrance-alignment waypoint. Those are navigation/control defects, not evidence
for another traffic priority rule. Breaker terrain clearance remained capped at
0.5 tiles while its full 0.55 body radius was used against units; enlarging
terrain footprints to match artwork would break required corridor access.

Trials of an eight-tick committed prefix, larger mandatory heavy-body gaps,
relaxed cosmetic parking tolerance, and staggered per-unit renewal did not
generalize. They reduced passage reliability, parking separation or attack
participation and were rejected. Exact compass, braking and clearance shortcuts
retained outcomes; their bounded CPU gains did not solve crowd behavior.
`830b792` and `826ffd0` are useful alignment/braking checkpoints, not a ready
main-based package. A shipped-map sweep eventually passed, but crowded-flow cost
and native behavior remained separate concerns.

#### Complete-journey scheduling

Longer task intent replaced the short horizon: retain collection, dwell,
delivery and a clear departure, with timed swept trajectories, terminal
occupancy and continuation planning. The planner became resumable; cached
collision profiles avoided rebuilding unchanged geometry. Later revisions
compared spatial detours with long temporal waits, let blocked units yield while
retaining their orders, and replaced affected future commitments together while
preserving active service and executable braking prefixes.

History anchors are `266cd43` for integration, `0f26e33` for on-demand
expansion/departures, `8d16a37` for delayed-route comparison and retained-order
yielding, `33bce04` for future revision, `2ab173c` for travel stops and
compatible journey reuse, and `105db6c` for combat stops and safe detours.

Rejected shortcuts are worth remembering:

| Attempt                                                  | Why it was rejected                                                                                                      |
| -------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| Broader early departure and intermediate stops           | Some initial responses improved, but worker throughput and later parked traffic worsened; some moves remained unfinished |
| Accept the first feasible preferred route                | Reduced recurring service and attack fan-out; feasibility alone chose poor destinations/timing                           |
| Approach a conflict and wait nearby                      | Improved several starts but retained the long army delay and reduced interrupted-yard output                             |
| Move waits onto earliest service arrivals                | Occupied shared work space without materially fixing response; some waits exceeded 1,000 ticks                           |
| Larger planning allowance or a future-release hook alone | Much waiting was already scheduled; repair could also spend its allowance checking superseded trajectories               |
| Recovery trajectory cache                                | Preserved recorded outcomes but demonstrated no reliable speedup; one Gantry pair was slower with it                     |

The final four-case comparison used three alternating timing pairs over 3,000
ticks. Sliding main versus `105db6c` plus its dirty recovery cache delivered
800/680 ordinary, 980/460 interrupted yard, 800/590 initially parked, and
800/460 passage scrap. Mean tick times were approximately 0.0093/1.599,
0.0307/2.572, 0.0152/2.218 and 0.0153/2.275 ms. These are simulation timings
from that historical host/fixture set, not FPS or cross-machine benchmarks.

The candidate ended those cases with no pending moves and no correction ticks,
but the last ordinary worker first responded after 17.9 seconds. Complete intent
had produced valid yet costly, conservative commitments. It did not make their
priority order globally efficient.

#### Bounded local-reservation replacement

The next trial kept stable spatial routes and destination claims, with 24-tick
local physical reservations. It chose full/half pace, braking and side
maneuvers; then added route repair around newly stopped bodies. It had no
complete service timetable or general joint router.

| Trial                                                                 | Ordinary / interrupted-yard scrap | Outcome                                                            |
| --------------------------------------------------------------------- | --------------------------------: | ------------------------------------------------------------------ |
| Stable routes plus local reservations                                 |                         400 / 330 | Immediate response, several stuck workers                          |
| Add stopped-body route repair                                         |                         750 / 650 | All workers repeatedly delivered in these two cases                |
| Restore intended group destinations/spacing and integration contracts |                         750 / 340 | Two yard moves unfinished; mixed arrival/crossing/door left 1/2/19 |

The favorable second trial was later tested more broadly: initially
parked/passage cases left 2/1 moves unfinished; mixed arrival/crossing/door left
6/0/19. It was not a hidden general solution ruined only by trial three. No
native acceptance was obtained for these trials. They were archived and the
original full-journey tree was restored.

#### Main-based contact steering and the final local revision

After the initial handoff, the original session also tried a cheaper design in
`cjl/spike/contact-steering` at `fcdbe44`. It carried differential treads
separately and returned to main-based path following. Contact geometry biased
steering and clipped motor travel, permitted 0.03 tiles of compression, and
retained small residual separation. A second arm added bounded repair around
stationary friends. There was no future timetable or new serialized planner
state, and no retained acceleration/braking integrator from the full-journey
rewrite.

Steering alone delivered 720 ordinary scrap; adding stationary routing delivered
710 ordinary, 670 yard, 730 parked and 660 passage. Ordinary
correction/propulsion fell from main’s 23.8% to 3.4%. However, stationary
routing regressed mixed arrival to two unfinished units, and both arms stranded
nineteen at the narrow door.

Human review rejected the second arm for jitter, ramming and full turns. The
exact tick-862 recording showed twelve workers choosing only three southern
service tiles, six sharing one point. A worker turned a net 688 degrees and
traveled six tiles inside roughly a 1.1-tile square without collecting. The
“waiting” metric excluded turning and translation, so low waiting totals
concealed the failure.

The final bounded revision spread workers over all eight perimeter tiles, tested
six forward contact bearings, retained passing side through chassis orientation,
and stopped rotating when no useful forward direction existed. It removed the
recorded confined full-turn loops. On the exact rejected recording it delivered
230 scrap versus rejected steering’s 180 and main’s 240. On longer saved
workloads it delivered 793 ordinary, 790 yard, 800 parked and 790 passage scrap.

It still failed the replacement contract: two opposing Breakers in open terrain
stopped for over 1,000 ticks; mixed arrival left one move, the doorway eleven,
and Gantry two. A body-size minimum on lookahead did not repair the pair and was
reverted. The bot completion test also exceeded its unchanged 25-minute limit
while replay/serialization tests passed. Work stopped instead of adding more
recovery layers. This uncommitted candidate remains in the contact-steering
worktree; the earlier review’s statement that the active tree is the second arm
is superseded by `last-local/review.md`.

The result of this entire first phase was useful presentation work and several
narrow fixes, but no general replacement controller. Neither a good worker
income number nor a green narrow suite outweighed actual stranded units or
rejected native appearance.

### 2. Independent Python research

The independent study deliberately left Oxide behind. Its initial physical
requirement was solid bodies, finite acceleration/braking and turning, pivots
and reverse, with no sideways displacement. It used floating-point Python, not
Oxide’s fixed-point state, so same-environment reproduction did not establish
cross-platform bit identity. Bodies were discs of different sizes rather than
oriented rectangular hulls. The models below were successive experiments, not
one finished controller that accumulated every demonstrated capability.

#### Fixed routes, small graphs and early service models

The first physical comparisons used predetermined paths. Rigid whole-itinerary
reservations delayed the last initial translation to a median 40.65 seconds.
Allowing waits between legs while retaining whole-itinerary priority reduced
that to 9.55 seconds, matching rolling one-leg commitments. Completion was
98.65/80.25/80.60 seconds for rigid whole/flexible whole/rolling. Rolling also
used fewer conflict checks. This corrected the initial interpretation: rigid
timing caused much of the delay; long horizons alone were not the culprit.

In the 18-body, 12-leg stress case, rigid itineraries scheduled 204 of 216 legs
and left one body unstarted at a 600-second search bound. Flexible and rolling
variants completed all legs. That was a bounded-search failure, not a proof of
physical impossibility. A heading-only reverse policy also failed by backing
along long routes at half speed; choosing reverse by total pivot-plus-travel
time corrected it. Forward and reverse-enabled crossing traces then duplicated
each other and were not independent samples.

A tiny joint graph oracle established that a two-body exchange required a
passing pocket and a temporary move away from the goal. Larger tiny-graph tests
showed why repeatedly abandoning that retreat oscillated: across 489 feasible
cases, naive replanning solved 407, remembering the original progress threshold
solved 461, and unrestricted search solved all. Freezing arrived bodies made
matters worse. These were exact results on small graph families, not a scalable
continuous planner.

Graph solutions were converted to finite-pivot, rest-to-rest physical motion. A
fully occupied square could rotate jointly even though no single move into an
empty destination existed. A known launch delay broke one rotation schedule;
retiming restored it. That did not test a surprise stop during execution. The
cost of adding stops was measured separately: an unobstructed trip took 11.7
seconds as one leg and 26.0 seconds as ten short legs.

A discrete service network tested admission, downstream capacity and exit
reservation. Greedy entry could deadlock buffered cycles; reserving exit space
prevented the tested buffered failures. Adding workers beyond bottleneck
saturation produced no benefit. An initial false deadlock came from a
reservation-release bug after completion and was explicitly discarded.

A physical facility then added interruption and template reuse, but still on
constrained paths with prescribed encounter groups. Unrepaired time-based
release collided after holds; actual-clearance release did not. Crucially, a
stronger timed controller that updated the deadline using the known delay
exactly matched event release in all 108 pairs. The lesson was to keep
reservations consistent with execution, not to reject clocks categorically.
Braking only one participant could collide; braking the interacting group
preserved unrelated progress. Cached maneuvers were rejected when a parked body
or larger radius invalidated their geometry. A separate following test showed
that heterogeneous braking needs an explicit stopping-distance allowance.

The user correctly challenged the fixed-path assumptions. These batches were
retained as scheduling and feasibility evidence, and the study moved to
genuinely free 2D control.

#### Free 2D local motion and cooperation

The new model generated curved trajectories from planar position, heading and
signed velocity. It used rectangle obstacles and size-aware static fields, with
no supplied lanes, common progress parameter or authored passing sequence. A
single-body arc/obstacle test established that it could turn while moving. A
small vehicle used a narrow gap while a larger one routed around the wall.

Two prototype defects mattered enough to invalidate earlier conclusions:

- Bilinear field interpolation mixed blocked-cell sentinel values into
  free-space costs, inventing a barrier near a traversable gap. Masking those
  samples fixed the route-size probe.
- A shared analytic motion formula lost numerical precision near zero angular
  velocity. Planner and executor agreed with each other yet produced sideways
  jumps. An independent quadrature audit found the defect; stable sinc/Taylor
  evaluation replaced it and the whole affected batch was rerun. The provisional
  loop result of 111/120 must not replace the corrected 110/120.

Corrected independent local control completed open crossings but only 2 of 24
narrow-opening arrivals. Retaining already-certified continuations avoided the
unsafe “no candidate, just brake” fallback. Online blocker-group discovery and
bounded planar maneuver search improved occupied service from 1/2 to 2/2 and
recurring obstacle service from 110/120 to 120/120. Choke completion barely
moved, 5/40 to 6/40; no heading seed cleared the fleet. Search could take about
a second in a tick. Idle relocation helped, but local cooperative search was not
a general bottleneck solution.

#### Geometry-derived admission

The next controller derived rooms and narrow interfaces from clearance geometry.
Throat-only FIFO admission completed 0/40 arrivals. Protecting waiting and exit
space improved that to 24/40; selecting by readiness/proximity with accumulating
waiting credit reached 40/40. These gains were real on the single choke.

They did not generalize directly. FIFO could nominate a unit trapped behind the
waiting queue. Exits could land inside walls or obstruct the next passage. A
combination of entry waypoints, ownership and off-axis exit bays cleared
parallel openings, but connected chokes still managed only 1/8. Two early
parallel runs crossed without ownership and were excluded. Relocating a
completed exit blocker restored a 9/9 service case, while a longer loop still
stopped at 22/24 after leases had been released. Passage admission did not solve
destination access.

#### Complete transfers, then concurrency

The study consolidated around one physical transfer: exact start state,
executable controls, service/dwell if required, departure and a stopped terminal
position. A serial executor provided a feasibility reference before concurrency.
Grid fallback recovered a route missed by bounded pose search; a discrete
distance-matched speed profile fixed a connector that overshot by 0.169 units.
These were route/control errors rather than reasons to add traffic exceptions.

All 36 consolidated cases completed, including connected chokes and loops, but
serial motion was slow: the gate loop took roughly 526–536 seconds. A bounded
best-first reference improved only four tiny cases and otherwise retained its
greedy incumbent. Correct composition had not supplied useful fleet throughput.

Offline schedule compaction showed available concurrency without changing
trajectories. Online compatible batches initially did little because route
generation froze neighbors at current positions. A stronger batch control
included relocation transfers; it improved matters but still wasted clearing
moves. Rolling admission compiled against predicted terminal positions and
checked actual timed continuations. The 120-run comparison completed all
declared tasks, with mean loop completion around 129.7 seconds versus serial’s
519.6. This was a substantial sandbox gain, but it bundled route/relocation and
scheduling choices; it was not a universal fourfold algorithm improvement.

#### Changed orders, dependencies and earlier release

Changed-order tests replaced future suffixes at physical stops while retaining
unaffected controls and task identity. Local repair beat finishing the old trip
in 17 of 24 matched cases, tied six and lost one. Mean new-service delay fell
from 24.44 to 17.42 seconds. Pair search tried 956 combinations, accepted none
and reproduced singleton repair. Simple stop-and-hold counterfactuals sometimes
collided; an interrupted unit could not discard its reservation arbitrarily.

The next comparison froze the same 2D controls and changed execution
coordination. Rigid schedules overlapped in 27/36 delayed runs. Dependency
ordering completed all 48 safely. Opportunistic occupancy gates completed only
the independent cases and stalled in all 40 congested ones. An exploratory delay
matrix was discarded because it imposed an extra delay inside an existing wait
instead of testing the declared interruption.

Cached dependency graphs predicted how a hold propagated: 520 candidate stops
included holds with zero, partial and full makespan penalties. Geometry
construction was excluded from the cheap prediction timing. This supported
caching immutable conflicts, not storing a permanent timetable for every unit
and map.

Releasing after actual rear clearance, while retaining the remaining path to a
safe stop, reduced congestion times by roughly 7–19%. Searching alternative
orders for the same routes found only one useful congested alternative at the
largest beam width. Most orders were constrained by the already-chosen routes
and stops. Time-scaled actuators also ran safely when release tracked physical
progress instead of elapsed ticks; this did not imply arbitrary mid-motion-stop
tolerance.

#### Actual braking, recovery and permanent obstacles

Single-body stop/hold/reconnect tests completed 156 audited runs, including a
later curved-motion extension. The first 96 repairs were all direct shots, so a
second matrix deliberately sought obstructed reconnections. Only three distinct
stops in that extension required indirect routing. An artifact naming bug had
also collapsed decimal labels and overwritten outputs; the affected batch was
archived and rerun rather than counted as evidence.

Advance braking envelopes reserved the nominal route plus straight-braking
branches from allowed command-time states. A constructed two-vehicle case was
nominally clear but collided under simultaneous braking; the enlarged envelope
rejected the joint launch. A false terrain rejection came from square-inflated
corners and was corrected to the disc geometry. Simultaneous pair/all-body stops
then completed 192 labeled tests safely, with duplicate injections identified.
All 424 repairs were direct first attempts, so this still did not cover lasting
obstruction or live compute limits.

Permanent disabled bodies exposed the gap. Retaining old routes completed 73/208
survivor visits; replanning reached 97/208. Some tasks were physically
disconnected; others were search/parking failures. Cheap separating-cut
certificates prevented futile relocation but did not add visits. An
exact-clearance grid fallback raised completion to 104/208, and prioritizing
bodies that obstructed unfinished destinations raised it to 110/208.

A warm geometry cache made repeated route queries much cheaper without changing
their outputs. It did not by itself resolve mutually obstructing parked units. A
selected manual clearance witness helped diagnose that distinction, then a
fairer two-parking-alternative priority ablation tested it without the
hand-picked pair.

General configuration-space connectivity found that 63 of 65 remaining next
visits were disconnected even under optimistic geometry. Two small vehicles had
routes that both fine grids missed. A conservative polygon-boundary route
fallback executed those routes and brought the full batch to 116/208, matching
the independently calculated reachable ordered-goal ceiling in all 24 cases. No
relocation budget was exhausted. That resolved the declared disabled-body
matrix; it did not mean all 208 visits were feasible or that arbitrary maps were
solved. Some equal-completion cases became slower.

#### Larger fleets, smaller commitments and real planning supply

Twelve- and twenty-four-body shared-service tests exposed an inherited parking
assumption: old spawn locations were protected forever because earlier fixtures
returned there. The new tasks never did. Removing only that unjustified
exclusion changed twenty-four-body completion from 264/432 to 432/432 visits.
Actual bodies, tasks, clearance and braking protection remained. Faster search
through the old constraints would not have fixed this.

A fair first-feasible owner policy reduced twelve-body candidate queries from
1,798 to 216 and planning wall time substantially, at the cost of 3.7–13.6%
longer serial completion. The expensive full twenty-four-body comparison was
narrowed rather than implying unmeasured policy equivalence.

Concurrent whole-transfer release still used only 3–4 moving bodies at peak.
Splitting the exact same controls at genuine zero-speed and service boundaries
improved completion another 14.6–39.0%, with 5–10 peak movers. This did not
insert new stops: it exposed stops already in the physical trajectories. A
coarse clearance bound was inconclusive on a few pairs; finer independent
integration certified them without lowering the required margin. Calibrated
delay tests also distinguished extra launch delay from holds absorbed by
existing waits.

A virtual plan-availability model replayed measured planning costs while
simulation advanced. It was explicitly not a real asynchronous planner and
excluded graph/install costs. The final experiment replaced it with a spawned
process generating actual transfers, a bounded queue, validated publication and
incremental dependency installation.

The open twelve-body case completed 36 visits in 270 simulated seconds versus
267 with every plan available initially. The twenty-four-body gate completed 72
visits in 1,632.7 seconds at a target 20× simulation cadence and 3,710.1 at 50×,
versus an offline 798.1-second reference. Main-thread maxima were about 95–99 ms
in the gate runs; queue blocking was negligible. Background work had not removed
synchronous installation spikes or plan starvation. These accelerated
shared-host Python measurements were not a production RTS benchmark.

All 180 live-run visits passed the scoped audits. Replaying recorded publication
ticks reproduced all 56,128 physical ticks and the independently generated final
graph. Unrecorded OS scheduling would not be deterministic. Immutable tasks and
geometry were assumed; this did not combine changing orders, permanent obstacles
and large asynchronous fleets into one tested controller.

#### What the Python work established, and what it did not

It demonstrated useful mechanisms: shared static guidance; executable controls
and braking envelopes; compatible conflict order; release from actual progress;
useful terminal-space allocation; and bounded replacement of affected motion.
Some declared failure matrices were fully resolved. It would be inaccurate to
say every Python experiment failed.

It did not establish acceptable Oxide movement. Many successes inherited one
another’s fixtures or route sets, had only a few seeds, used finite task lists
and circular bodies, or paused simulated demand while planning. Some “wins”
disappeared when the control comparison was strengthened. No sandbox result
certified attack fan-out, native animation quality, the full game lifecycle or
production CPU cost. The later Rust probe was necessary precisely because those
gaps remained.

Historical research also covered cooperative pathfinding, safe intervals,
dynamic-window control, hybrid route planning, reciprocal avoidance, flow
fields, endpoint conditions and action dependencies. The inspected Supreme
Commander 2 account included shared flow caches but also pushing/sliding; OpenRA
exposed waiting, repathing and blocker cooperation. No internal algorithm was
verified for other top-down RTS games. Those games remained visual references.
The bibliography and its scope limits are preserved in the original reports; an
algorithm name was never evidence that our implementation had its guarantees.

The simple cache experiment is still a useful boundary: for repeated goals,
three shared fields used about 9,313 expansions versus 100,753 for per-query A*.
Mostly unique goals required about 237 fields and 735,727 expansions versus
77,845 for A*. Those were static distance queries, excluding geometry
construction, reconstruction, control and changing occupancy. Cache by reusable
geometry/movement class when reuse exists; do not assume a complete per-unit
route cache is automatically efficient.

### 3. Bringing the Python contracts into Oxide

The integration used a new worktree, `cjl/spike/movement-order`, from `105db6c`
plus an exact copy of the original dirty changes. It preserved the first
session’s controller, rendering, treads and physical parameters as its
reference. The original movement checkout was not edited.

The opt-in `movement-order-spike` coordinator constructed complete physical
trips against a serial forecast of terminal body positions. It split them at
genuine stopped/service boundaries, reserved nominal travel plus
straight-braking branches, and released conflicts from actual progress.
Compilation used existing Oxide guidance and physical controls, with one pending
compiler and a fixed logical-work allowance. Initial activation required
repeating work and stopped visible bodies.

This was a partial transfer of the Python design, not a complete port. It reused
Oxide’s eager compiler and departure allocator, treated idle helpers as
stationary obstacles, and used coarse drain-and-reset behavior on changed
objectives or world state. It did not bring over bounded local repair, boundary
routing, a background producer, general body removal or
changing-combat/visibility handling. Those omissions limit what the failure says
about the broader research design.

| Integration arm                 | Change                                               | Ordinary scrap in 3,000 ticks | Finding                                                       |
| ------------------------------- | ---------------------------------------------------- | ----------------------------: | ------------------------------------------------------------- |
| Original research reference     | Full-journey controller                              |                           680 | The reference is not sliding main                             |
| Initial ordering                | Serial forecast plus progress-gated stopped segments |                           150 | Workers 6–11 never launched                                   |
| More braking margin in guidance | Extra route padding                                  |                           160 | Same participation failure                                    |
| Strict grid-edge clearance      | Check connections, not just clear vertices           |                           140 | Every worker eventually delivered; waiting remained excessive |
| Four-trip admission cap         | Limit outstanding complete trips                     |                           140 | No throughput recovery                                        |

The strict-edge correction fixed a real contract mismatch. Two individually
clear grid vertices at (22.5,23.0) and (22.5,23.5) had an edge passing only
0.5997681 tiles from a neighbor’s center. Combined physical radius was 0.6 and
required reservation clearance 0.605. Full physical admission correctly rejected
the route, but guidance kept returning the unchecked edge. More padding did not
repair the missing edge predicate. The regression proves a separately reviewable
routing issue; it does not prove the old controller executed a collision.

After that correction, ordinary workers accumulated 29,416 scheduled-wait ticks
and only 79 planning-wait ticks, out of 36,000 worker-ticks. Average moving
workers fell from the reference’s 5.3447 to 1.2003; the last first response grew
from 17.9 to 99.35 seconds. The cap mainly relabeled waiting as
planning/admission delay. It did not establish CPU starvation or justify another
cache.

Final four-workload results were reference/spike scrap of 680/140 ordinary,
590/40 with Sentinels entering/leaving traffic, 460/60 through a constrained
passage, and 590/140 with newly staged stationary blockers. The spike left six
and three non-harvest moves unfinished in the two interruption cases. The old
archive’s “parked” replay duplicated an entering/leaving fixture, so a genuinely
static drill was created instead of counting duplicate input as another case.

All four targeted audits had zero overlaps and zero position corrections, valid
state every tick, and exact serialized continuation/event parity. Their
2,682,000 checked pair intervals are repeated observations within four
scenarios, not a universal safety certificate. Native comparisons with identical
cameras showed the long queue. This is strong evidence against promotion even
though the selected physical audits passed.

Broader checks found new failures in recurring service, mixed-army crossing and
full-command state integrity at tick 270. An internal accepted-program lookup
also assumed the old representation; that assertion is distinct from a gameplay
failure. The reference passed the cooperative and integrity controls. Several
unit failures were inherited. Full workspace checks and coverage were not green,
and no version or fixture bless was used to conceal drift.

Connor requested an update to remote main during the spike. Signed merge
`11616c0` includes fetched `73a58fe`. Conflicts preserved ground propulsion and
tread inputs while incorporating aircraft cruise steering and sprite LOD. A
missed aircraft-alignment condition was caught and fixed. All four rebuilt
comparisons reproduced their original per-unit outcomes and hashes. The main
merge was committed; experimental changes were restored uncommitted, and nothing
was pushed. The merge does not turn the branch into main’s movement
implementation.

### 4. Final isolation of service, parking and precedence

A test-only Rust harness used the same Oxide physical compiler and propulsion,
homogeneous radius-0.3 Harvesters, fixed pickup/delivery footprints, 100/1-tick
dwell and a constant planning allowance. Routes were generated in free 2D.
Service completion was a harness event, not full-economy scrap, so its counts
must not be mixed with the integration table.

Assigned terminal bays outside the work area did not automatically improve
circulation. At twelve workers over 150 seconds, automatic departure completed
twelve deliveries; assigned bays and bays plus real entry stops completed nine.
Some workers never moved. Outside the service footprint was not the same as
outside other units’ routes.

The next comparison froze identical complete trips before execution and varied
admission only:

- Full precedence protected every earlier foreign segment’s remaining envelope.
- Actual-only admission protected current bodies and active motion but ignored
  unstarted future segments.
- An endpoint guard retained the current-body/active-motion checks but protected
  only the candidate’s final stopped position against earlier future envelopes.

For twelve workers with assigned bays, endpoint guarding completed those exact
trips at tick 2924 versus 4542 with full precedence, 35.6% sooner. Bays plus
entry stops improved 26.7%. Automatic departure did not improve. Crossing future
space and parking there imposed different constraints.

Removing future protection entirely caused permanent obstruction. In the
automatic twelve-body case, unit 8 finished at tick 1006 and parked across unit
1’s remaining movement envelope. All motion stopped after tick 1299. At tick
12000, there was no active segment or pending plan, every unfinished unit
already had a queue, and no queued head could start. Under those frozen
objectives only the clock could change. That is a permanent block under the
tested rule, not merely a timeout.

Repeated service was harder than one frozen wave. Over 600 simulated seconds:

| Twelve-worker layout  | Full precedence: deliveries / worst wait | Endpoint guard: deliveries / worst wait |
| --------------------- | ---------------------------------------- | --------------------------------------- |
| Automatic departure   | 58 / 128.5 s                             | 61 / 128.5 s                            |
| Assigned bays         | 47 / 199.0 s                             | 59 / 119.8 s                            |
| Bays plus entry stops | 44 / 189.3 s                             | 54 / 115.95 s                           |

Actual-only admission could raise total output while starving units. A
six-worker bay case completed 78 deliveries versus full precedence’s 48, but one
worker delivered zero. The policy that cleared the frozen twelve-worker
entry-stop wave completed only two deliveries when repeated planning was
enabled. Neither throughput totals nor first-wave completion was an adequate
acceptance test.

Finally, destinations were distributed among eight pickup and twelve delivery
points already inside the unchanged legal service regions. This addressed
snapshots in which eight workers chose one delivery point and pickup choices
clustered on one side. With six workers and full precedence, deliveries rose
from 55 to 74 and longest wait fell from 55.5 to 27.9 seconds. With twelve,
deliveries barely changed, 58 to 59, and the longest wait remained 104.65
seconds. Combining distribution with endpoint guards did not produce a scaling
solution.

The batches passed their physical checks, sampled invariants and final 100-tick
serialized continuations. Nine twelve-worker recurring outputs reproduced
exactly, and the final-source automatic control matched the earlier run.
Alternative admission policies remain test-only. Full workspace tests retain the
prior PNG failures and coverage timed out. This was the final experiment batch
before Connor chose to stop.

### Why we are stopping

The investigation made diagnostic progress and solved several declared sandbox
problems. It did not demonstrate a game-ready replacement for the permissive
controller. The failures were recurring rather than one missing constant: idle
and terminal occupancy, poor destination allocation, overly broad commitments,
starvation behind valid plans, costly geometry/repair, and integration with real
task state. Physical accuracy made those obligations explicit; it did not solve
them.

The strongest caution is that gains did not add together automatically. Better
parking, earlier release, less conservative ordering and more distributed
service each helped selected cases. Combinations still failed at higher density
or during continued work. The Python library of successful mechanisms was not a
validated single Oxide controller, and the Rust probe omitted meaningful parts
of it. Calling the remaining work “bounded” understated its engineering and
product cost.

This does not justify replacing the old system with the least bad experiment, or
explaining main’s entire advantage as cheating. It justifies keeping the
approved presentation work, accepting sliding as a product approximation,
preserving the evidence, and ending this broad line of work.

### Evidence and source map

The closure note is the current decision record. Older notes contain superseded
“next experiment” recommendations and unchecked tasks; they are historical
context, not instructions to resume work. Preserve the raw artifacts if cleaning
targets or deleting worktrees. Binary names alone do not identify their source;
use the saved fingerprints and patches.

| Material                                    | Durable location and use                                                                                                                                                                                                                                                                                       |
| ------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Original handoff                            | [Desktop handoff](/Users/cluebbehusen/Desktop/Oxide-Movement-Fable-Handoff.md); ends before the later contact-steering trials                                                                                                                                                                                  |
| Original session’s full ledger              | [collision-research.md](/Users/cluebbehusen/oxide-ui-polish/review-afk/worktrees/collision-research/agent-notes/collision-research.md); includes later contact work and superseded intermediate findings                                                                                                       |
| First-session evidence root                 | [movement evidence](/Users/cluebbehusen/oxide-ui-polish/review-afk/evidence/movement); native captures, patches, replays and benchmark generations                                                                                                                                                             |
| Final full-journey comparison               | [summary.json](/Users/cluebbehusen/oxide-ui-polish/review-afk/evidence/movement/journey-recovery-cache/main-comparison/summary.json)                                                                                                                                                                           |
| Local-reservation trials                    | [middle-ground summary](/Users/cluebbehusen/oxide-ui-polish/review-afk/evidence/movement/middle-ground/summary.json); inspect each trial’s separately saved untracked module                                                                                                                                   |
| Main-based contact comparison and rejection | [contact review](/Users/cluebbehusen/oxide-ui-polish/review-afk/evidence/movement/contact-steering/review.md); user-review recording and trace are beside it                                                                                                                                                   |
| Latest first-session local revision         | [last-local review](/Users/cluebbehusen/oxide-ui-polish/review-afk/evidence/movement/contact-steering/last-local/review.md); supersedes the preceding review’s active-source description                                                                                                                       |
| Python archive                              | [Oxide-Movement-Research-2026-09-11](/Users/cluebbehusen/Desktop/Oxide-Movement-Research-2026-09-11); runnable source, manifests, reports, rejected pilots and animations                                                                                                                                      |
| Earlier Python synthesis                    | [CURRENT-REPORT.md](/Users/cluebbehusen/Desktop/Oxide-Movement-Research-2026-09-11/CURRENT-REPORT.md); retain its findings, supersede its integration recommendation                                                                                                                                           |
| Integration report                          | [RESULTS.md](/Users/cluebbehusen/oxide/review-movement-order/RESULTS.md); four workload comparisons, source and compatibility boundaries                                                                                                                                                                       |
| Native integration comparison               | [worker-loop video](/Users/cluebbehusen/oxide/review-movement-order/post-main-update/native-comparison-steady.mp4); both binaries include the main merge                                                                                                                                                       |
| Service-allocation follow-up                | [report](/Users/cluebbehusen/oxide/review-movement-order/service-allocation/RESULTS.md), [summary](/Users/cluebbehusen/oxide/review-movement-order/service-allocation/summary.json), and [schematic video](/Users/cluebbehusen/oxide/review-movement-order/service-allocation/service-position-comparison.mp4) |
| Complete second-session notebook            | [research-log.md](/Users/cluebbehusen/oxide/review-movement-order/service-allocation/research-log.md); includes Python and integration history, with historical recommendations superseded here                                                                                                                |

The Python archive is organized by experiment, so a future reader does not need
to reconstruct it from temporary directories:

| Directory          | Question and evidence                                                                                      |
| ------------------ | ---------------------------------------------------------------------------------------------------------- |
| Archive root       | Initial fixed-route scheduling, tiny pocket oracle, static cache comparison                                |
| `continuation`     | Retreat memory, finite-capacity service network, occupied-square rotation and graph-to-physical conversion |
| `hypothesis`       | Physical facility release, known-delay timing repair, local versus global braking, cache-context checks    |
| `free-2d`          | Free planar controller, field interpolation and mixed-size routes                                          |
| `cooperation`      | Retained continuations, generic joint maneuvers and discarded unstable-integrator batch                    |
| `admission`        | Geometry-derived portals, holding/exit space, idle blockers and connected-choke failures                   |
| `transfers`        | Complete service/departure contract, serial reference and route/control corrections                        |
| `concurrency`      | Offline compaction, stronger batching and rolling admission                                                |
| `interruptions`    | Task identity, retained prefixes, singleton versus pair repair                                             |
| `dependencies`     | Rigid timing, opportunistic gates, compatible order and cached delay propagation                           |
| `release`          | Actual rear clearance, bounded reordering and variable-duration actuators                                  |
| `recovery`         | Single physical stops and indirect reconnections; overwritten-filename history                             |
| `brake-envelope`   | Nominal-versus-braking counterexample and simultaneous stops                                               |
| `disabled`         | Permanent stationary bodies, survivor task failures and disconnection certificates                         |
| `exact-fallback`   | Fine exact-clearance routes, geometry reuse and mutually obstructing endpoints                             |
| `goal-priority`    | Controlled destination-blocker relocation ranking                                                          |
| `boundary-routing` | General connectivity, narrow boundary routes and the 116-visit reachable ceiling                           |
| `fleet-scale`      | 12/24-body fairness, erroneous spawn protection and corrected parking                                      |
| `granularity`      | Natural stopped segments, refined clearance audit and virtual plan availability                            |
| `streaming`        | Real producer, recorded publication ticks, synchronous installation costs and completion audit             |

Current worktrees are also not interchangeable. `collision-research` is at
`105db6c` with its original dirty patch. `contact-steering` is at `fcdbe44` with
the final local candidate uncommitted. `review-movement-order/worktree` is at
`11616c0` with the later spike and harness uncommitted. The primary
`/Users/cluebbehusen/oxide` checkout is a separate active workstream; its
unrelated dirty work must not be used as a rollback target. This documentation
task did not change any of their simulation or presentation code.

### 5. Retained assembly on main, September 12

Connor then authorized assembling the retained behavior. The new branch is
`cjl/feat/sliding-movement-polish` in
`/Users/cluebbehusen/oxide/review-movement-assembly/worktree`, based on freshly
fetched `origin/main` at `73a58fe`. This is a fourth reference, later than the
frozen main used in the historical research. Its measurements must not be mixed
with the earlier 800/680 comparison as though the source and fixtures were
identical.

The implementation keeps main’s path generation, group destination assignment,
service choices, air steering, independent turrets and collision resolver. The
resolver still applies lateral and radial displacement. No complete-journey
planner, service reservation system, admission manager, ordering coordinator or
contact-steering candidate is included.

A small ground motor follows main’s advisory waypoints: six ticks to reach full
speed, three ticks to brake, and braking along the old heading before a sharp
turn. Stopped units pivot at their existing chassis turn rates. The motor slows
for the retained waypoint when early advancement would cut a blocked corner. It
may translate along the target direction within the existing heading tolerance;
it is not a strict nonholonomic controller. Reverse-capable tread odometry is
retained, but this motor does not introduce autonomous reverse maneuvers.

Motor speed is serialized and validated separately from collision displacement.
Carried units have zero speed. Fixed weapons wait for braking before aiming or
bracing; independent turrets retain their separate behavior. Field welding
requires both bodies to have stopped. The shell draws continuous differential
tread belts for Sentinel, Warden, Lancer, Harvester, Avalanche and Breaker,
driven by propulsion and hull rotation. Collision corrections do not spin the
belts as if they were powered travel. Replay seeks reset the transient odometry.

Three actual integration defects were fixed during validation. The first terrain
guard prevented an embedded unit from leaving a newly claimed foundation;
within-tile travel now permits the existing eviction route to work while
crossing into a new blocked tile still stops the motor. Testing heading
alignment before a stopped unit’s permitted turn introduced an unwanted
departure delay; throttle now uses the post-turn alignment. Slowing only for
final destinations caused overshoot and repeated repathing at required
intermediate corners; arrival-speed limiting now also applies to those retained
intermediate targets. Existing construction, corner-repath, group-arrival and
departing-repair-patient regressions exercise these boundaries.

The comparison uses separately frozen main and assembled binaries with identical
fixture hashes in each pair. All 23 cases have zero pending finite orders at the
horizon and zero recorded long-stall classifications. Worker loops remain
ongoing work, so this is not a claim that every looping task has ended. Saved
3,000-tick workloads produced main/assembly scrap totals of ordinary 800/800,
interrupted yard 980/850, parked 800/800 and passage 800/800. Generated worker
loops produced 370/340 with four workers, 730/620 with eight, 990/880 with
twelve and 1600/1500 with twenty-four. Thus acceleration and braking cost about
6–15% in the affected loops, including 13.3% in the interrupted yard. There is
no coordinator-style collapse in this batch, but no evidence that the new motor
is free or universally better. Collision corrections persist and often increase.
CPU timings were collected alongside builds and are not a clean performance
comparison.

A build-provenance error was caught before the final comparison: using one Cargo
target directory across two source checkouts left a supposedly fresh assembly
executable byte-identical to the main reference. The final assembly was rebuilt
in its own exclusive release target and frozen separately; hashes distinguish it
from main. Only `final-bin`, `final` results and `native/final-*` recordings
support the final assembly claims. Earlier `candidate` and unprefixed motor
recordings are intermediate artifacts.

Native captures show the six chassis starting, braking after retasking, pivoting
and moving again, plus ordinary worker traffic beside main. The final frames and
the changed CPU golden images were inspected. The CPU images show changed unit
positions and combat timing; they do not validate tread animation. The native
clips support presentation inspection, not Connor’s final judgment of feel.

Evidence:
[paired measurements](/Users/cluebbehusen/oxide/review-movement-assembly/comparison.json),
[main left and assembly right](/Users/cluebbehusen/oxide/review-movement-assembly/native/final-main-left-assembly-right.mp4),
[start, retask and stop](/Users/cluebbehusen/oxide/review-movement-assembly/native/final-start-retask-stop.mp4),
and
[binary hashes](/Users/cluebbehusen/oxide/review-movement-assembly/binary-sha256.json).
The artifact root also holds the original main source snapshot, fixture sources,
raw result files and native replay inputs.

Validation also exposed fixture assumptions affected by the timing change.
Repair fixtures used melee raiders that now died before inflicting the wound the
test needed; ranged raiders restore that premise while retaining repair
assertions. A supposed Sapper bystander instead chased the attacker out of the
blast; the revised scene uses a stationary Harvester and asserts its distance to
the actual blast point before checking splash damage. The artillery peak
fixture’s range clamp stopped its projection just short of the peak; moving the
shooter and target one tile east restores the blocked prediction and explicitly
verifies that weapon reach extends past the peak. The CPU showcase runs at tick
540 instead of 480 so its partially depleted node remains represented.

The island-scout test had treated any NoRoute stall as failed cross-island
scouting. Its recorded failure was a home Sentinel rally near (3.34, 3.75)
toward (4, 2) at tick 482, without a new command. The replacement assertion
directly rejects ground Move, AttackMove or Advance commands across the dividing
wall, while still requiring a scout aircraft and enemy discovery. This does not
establish that home rallies never stall; no general rally fix was made.

Before fixture authorization, validation passed the entire workspace test suite
with only the four already-failing compatibility fixture comparisons excluded:
shipped state hashes, scripted-controller hashes, showcase PNG and midgame
skirmish PNG. These exclusions describe a partial validation run, not a changed
repository gate. The earlier unfiltered workspace run demonstrated those
differences. Focused Sapper and artillery suites passed after the premise
corrections. Final Clippy, rustdoc and Rust formatting passed. Final unit
coverage was 90.32% of lines against the existing 83.5% gate. Combined coverage
was attempted but stopped at the unapproved PNG differences, so no final
combined-coverage pass is claimed. At that checkpoint, neither SIM_VERSION nor
any checked-in golden/hash fixture had changed. Compatibility validation and
native acceptance were still open; the later acceptance and same-version fixture
decision are recorded in Decisions.

### 6. Fixture refresh and PR validation

- With Connor’s explicit same-version approval, the driver fixture refresh
  passed at 0.16.0. All 66 shipped-map hash rows and all 14 scripted-controller
  rows changed; their key sets and version metadata stayed the same. The two
  changed PNGs were inspected and reflect the expected altered unit positions
  and combat timing. The opening PNG remained unchanged.
- The final PR check runs without BLESS or BLESS_SAME_VERSION. The standard
  combined-coverage alias keeps only its repository-defined exclusions for
  expensive long-horizon oracles; the ordinary workspace run includes those
  oracles. No gate configuration, threshold or test exclusion was added for this
  branch.
- Renamed the version-suffixed integration suites to field_kit.rs, bombers.rs,
  overseer.rs, roster.rs and transports.rs, and updated documentation
  references. Agent-note filenames retain their historical versions at Connor’s
  request.
- Final unfiltered workspace validation passed all 2,955 tests. Rust formatting,
  workspace Clippy with warnings denied, rustdoc with warnings denied, and
  repository Markdown formatting passed. Unit line coverage passed at 90.32% and
  combined line coverage passed at 91.79%, above the unchanged 83.5% and 86.0%
  gates. Both coverage commands exited successfully. The full run verified the
  refreshed fixtures with blessing disabled.

### 7. Movement integration review follow-up

- Independent review probes found two consumers that still treated a missing
  path as proof of rest. An arriving leader could finish a neighbor’s order
  while still coasting, and a braking worker could receive anchored collision
  priority. Both predicates now require zero motor speed. Regressions include
  the transition back to ordinary settled/anchored behavior after stopping.
- The canonical scripted-bot skill retained a command for the renamed Overseer
  suite. It now uses the current test target; all canonical skill directories
  were validated.
- The proposed global travel-forecast surcharge was not included. Full-tick
  straight Harvester moves of one, three and ten tiles completed their orders at
  exactly the forecast 8, 24 and 80 ticks; stopping took two more ticks.
  Motor-only exact-center timings do not establish a general allocation
  regression. A concrete builder or worker deadline failure is needed before
  changing those heuristics.
- The three new regressions failed against the submitted code and passed after
  the fixes. The existing same-version fixture approval remains in force at
  0.16.0; the follow-up is a new signed commit and ordinary push, not an
  amendment or force push.
- The artillery integration fixtures now distinguish issuing a move from
  reaching cruise speed. The Advance case accelerates through real ticks before
  firing. The neighbor-visibility cases seed an already cruising, west-facing
  group so the shot remains on the authored fog boundary; they assert motor
  speed and heading as well as the original visibility and splash geometry.
- The old projectile snapshot assigned maximum speed along the current waypoint
  and omitted pathless units. A retargeted Scuttler was predicted moving left at
  about 0.16 tiles/tick while actually braking right at about 0.107. Ground
  snapshots now use retained motor speed and hull heading without consulting the
  route. This represents propulsion during acceleration, retargeting and
  pathless coasting. It remains constant-velocity extrapolation; it does not
  predict future braking, steering or lateral collision correction. Air
  estimation is unchanged.
- The follow-up changed all 66 shipped-map hash rows and all 14
  scripted-controller rows. Both fixture stamps remain 0.16.0. The two changed
  PNGs were inspected side by side: the showcase preserves its feature coverage,
  and the skirmish frame reflects changed unit positions and economic timing.
  The opening frame is unchanged. The driver fixture refresh passed, including
  the long-horizon scripted-controller probe.
- The archived submitted binary finishes the same default balanced mirror at
  tick 13,196 (loser elimination at 13,195), versus 42,108 with the corrections.
  The fixed run therefore changes this matchup substantially, not merely a small
  boundary overshoot. That comparison was disclosed before accepting any longer
  completion horizon.
- The unfiltered suite exposed a completion-horizon failure that combined
  coverage deliberately skips: the balanced mirror had no result at 30,000
  ticks. The extended diagnostic run reached victory at tick 42,108, with
  continued production and combat through 30,000 and 40,000, no rejected
  commands, and final elimination at 42,107. After the controlled investigation
  in section 8, Connor approved a 50,000-tick completion ceiling with the
  victory and command-validity assertions retained. The resulting closure is
  recorded in section 9.

### 8. Isolating the longer match and evaluating travel forecasts

The research below changes only scratch copies and observational harnesses. The
review fixes remain uncommitted in the assembly worktree. The completion test
still requires victory within 30,000 ticks; neither its ceiling nor bot travel
forecasting was changed.

#### Controlled comparison

The scratch workspace copies the current sim and chassis, then places each of
the three behavior predicates behind a compile-time feature. With features
disabled it reproduces the submitted match at tick 13,196. With all enabled it
reproduces the local fixed match at tick 42,108; its state hash at that tick,
`0x5d45571cdb6e8ce3`, matches playback through the production driver. There are
no runtime environment reads inside the simulation. Each variant ran the same
five personality seeds, 0 through 4, on the same built-in Skirmish scenario,
Standard difficulty and Balanced stance. The ceiling was 60,000 ticks. These are
a small paired sample, not five independent maps or a broad balance evaluation.

| Enabled fixes              | Seed 0 |  Seed 1 | Seed 2 | Seed 3 |  Seed 4 |
| -------------------------- | -----: | ------: | -----: | -----: | ------: |
| None, submitted behavior   | 13,196 |  14,100 | 13,196 | 13,808 |  15,193 |
| Group arrival only         | 18,744 | >60,000 | 14,168 | 53,075 | >60,000 |
| Anchoring only             | 46,383 |  10,202 | 21,229 | 10,505 |  10,202 |
| Projectile prediction only | 13,196 |  14,100 | 13,196 | 13,808 |  15,193 |
| All fixes                  | 42,108 |  22,004 | 17,293 | 23,574 |  16,692 |

The greater-than entries mean no victory by the ceiling, not proof of permanent
deadlock. Their snapshots still show economic deliveries, changing rosters and
combat. All 25 matches issued zero rejected commands. Aiming-only is
bit-identical to the baseline across all five final states. Those matches fired
no projectiles, so aiming cannot explain the default mirror change. The combined
fixes finish all five matches in 13.9–35.1 simulated minutes, with an
18.3-minute median versus 11.5 minutes before the fixes. Anchoring alone makes
three of the five matches shorter. The default mirror is therefore a sensitive
trajectory, not evidence that every unit or task became three times slower. The
combined sample nevertheless has longer matches and does not establish
acceptable game pacing or human fun.

#### First divergences and their consequences

- Anchoring first changes state at tick 451, on both mirrored worker groups. A
  worker has just deposited, cleared its path and retained a motor speed of
  about 0.0417 tiles/tick. Its adjacent partner is stationary and extracting.
  The correction removes the moving worker's anchored priority; the two bodies
  separate with different shares. The first position differences are about
  0.02–0.04 tiles. Commands, deposits and banks are still equal on that tick.
- That displacement changes subsequent haul timing. At tick 1,000 the anchoring
  variant has delivered 210 scrap per seat against 190 in the baseline. The
  first different command is at tick 912: both bots can train a Sentinel in the
  variant while the baseline cannot yet afford it. This is an earlier purchase,
  not an economy freeze.
- Between ticks 2,000 and 3,000, eight already-existing Sentinels disappear in
  the anchoring variant and none in the baseline. The different purchases and
  positioning have already changed the combat sequence. The anchoring-only
  default ultimately flips the winning team. Subsequent match duration includes
  production, combat, losses, strategic reassessment and finishing the opponent;
  it is not a travel-time measurement.
- Group arrival first diverges at tick 1,814. Sentinels 12–15 continue their
  allocated rally moves instead of becoming idle against a neighbor whose path
  has cleared but whose motor is still running. Its standalone default match
  finishes at 18,744. This change interacts with anchoring rather than adding a
  fixed number of ticks to the final match.

The original suggestion to simply extend one timeout was premature without this
isolation. The local predicates satisfy the intended stationary-body contracts,
but their match-level effect needs a calibration decision. Retain the focused
behavioral regressions and judge completion over a small fixed batch rather than
treating one exact match length as proof of motor correctness. No timeout change
or publication is implied by this recommendation.

#### What the forecasts actually control

The two shared distance-to-time conversions are in
`bot/utility/economic_value.rs` and `bot/utility/defense.rs`; raid procurement
and standing-force repair also duplicate the conversion. About a dozen consuming
modules use these values for worker and infrastructure investment, orphaned
construction, defensive construction versus reinforcements, Array readiness,
reconnaissance arrival, support deployment and repairs, and raid preparation
deadlines. Reconnaissance already adds its own 24-tick allowance. Route costs,
readiness margins and endpoint meanings therefore need reconciliation before
adding another universal margin.

Worker `HarvestWork` uses a resource-weighted mean haul cost and a count of
available work positions. `WorkerService` carries only unit kind and readiness
delay. Its output is the number of full loads fitting after readiness, using
gathering time plus two distance/speed legs, capped by finite resource amount
and available positions. It affects expected marginal return and investment
selection. It is not the allocator's guaranteed income: `ResourceSnapshot`
forecasts completed Foundries, Extractors, Reclaimers and Refineries. Worker
income does not enter those guaranteed streams, and commands still spend current
banked scrap. The review comment correctly identifies approximate travel times,
but its suggestion of workers funding impossible purchases needs this
qualification.

`UnitObs` intentionally exposes tile-level location, without precise position,
heading, speed or route. The current travel helper takes only kind and scalar
route cost. Neither input can distinguish a stopped unit facing the destination
from one braking in the opposite direction, or a straight route from one with
repeated turns. Own-unit kinematics can be exposed honestly; enemy route intent
must remain private, with hostile arrival estimates retaining uncertainty.

#### Measurements of actual jobs

The harness uses real commands and `State::tick`, retaining the command tick.
Move completion is the order becoming Idle, construction completion is a built
structure, and haul cycles are consecutive credited deposits. The distance-only
comparison uses the same conversion as the bot, applied to the actual selected
route. It does not claim to reproduce every regional weighted quote or the
planner's chosen builder. Initial motor states are explicit fixtures. A passive
friendly aircraft supplies sight for distant scrap; it does not collide with
workers. Early exploratory distant-harvest rows rejected for lack of sight were
excluded and corrected in the final probe.

- A stopped Harvester moving one tile takes 8 ticks when facing the goal, 14
  when facing 90 degrees away, and 22 when facing away. All three scalar
  forecasts are 8 ticks. A one-tile Breaker move forecasts 19 ticks but takes 13
  facing the goal and 42 facing away. A uniform positive surcharge can therefore
  worsen some already-conservative estimates.
- Across 72 isolated movement cases, including four chassis, three distances,
  three headings and stopped/cruising starts, scalar error ranges from 28 ticks
  early to 8 ticks late.
- Nine actual Fabricator construction jobs vary distance and initial heading.
  Forecast completion ranges from 15 ticks early to 1 tick late even when
  supplied the actual selected route cost. For example, a route costing 104
  tenths plus construction forecasts 364 ticks; completion is 371 or 379
  depending on heading. These are measured service jobs, unlike the earlier
  straight Move-only probes.
- Lone Harvester haul routes of 3, 6 and 11 tiles forecast cycles of 148, 196
  and 276 ticks, but repeat at 180, 228 and 308 ticks. Each adds 32 ticks over
  the distance-only cycle. Ignoring that overhead overstates steady, unsaturated
  output by approximately 22%, 16% and 12%, respectively. Finite deposits and
  saturation still cap the bot's actual quote. The discrepancy already exists in
  the submitted motor, independently of these review fixes.
- A route around a wall forecasts 144 ticks from its 180-tenths path cost but
  takes 174. A straight 12-tile control takes its predicted 96 ticks. Turn shape
  matters beyond total route distance.

#### Shared-motor prototype and implementation scope

A scratch helper clones one unit on an already assigned route and runs the
existing ground motor until the relevant arrival predicate is met. It advances
no other actor, bot or combat. After the initial command has selected a route,
the helper predicts the remaining travel using the current position, heading and
motor speed; construction adds the actual work duration and phase offset. This
is a feasibility demonstration for sharing the movement model, not a
production-ready pre-command bot quote. Its `State`-based interface must be
replaced by fog-honest geometry and own-unit observations before use by the bot.

The helper matches all 72 isolated moves, all nine construction completions, and
both route-shape controls exactly. Four-body arrival also matches in this
fixture; eight- and sixteen-body crowds introduce additional delays up to 27
ticks, and one body arrives a tick sooner through collision
correction/settlement. Correct free-flow physics does not predict future
traffic.

Five batches of 1,000 forecasts for a 12-tile path have a median cost of about
37.9 microseconds per query on this machine, using the optimized-development sim
with no concurrent build or match batch. This includes cloning the unit/path and
stepping the current geometry checks. It is a limited cost probe, not a
production frame-time benchmark. Hundreds of candidates would add milliseconds;
reusing route geometry, filtering candidates cheaply, quoting only decisions
near a deadline, and profiling the actual caller load matter.

Recommended bounded implementation:

1. Define what each consumer needs: entering a goal tile, reaching a work
   surface, delivering a load, or stopping. Separate nominal travel from
   uncertainty and authoritative completion.
2. Extract or expose a pure motor projection shared with movement. Supply
   precise own-unit kinematics and a projected route; handle newly trained units
   and unknown initial heading explicitly. Avoid duplicating tuned acceleration
   and turning rules in bot code.
3. Route the shared converters and duplicate raid/repair calculations through
   that model. Keep air estimation and hidden enemy intent separate. Preserve
   lower-cost geometric estimates for broad candidate ranking.
4. Model recurring work as gather, outbound travel, deposit and return with the
   correct terminal conditions and turnaround state. Refresh from actual
   completed service, and use conservative handling for deadline-sensitive
   commitments. Congestion cannot receive a guaranteed finite ETA merely from a
   static route.
5. Validate short trips, bends, opposite-heading retasks, newly spawned units,
   complete harvest loops and builder deadlines, then paired bot outcomes and
   per-think cost. Shared deterministic code and current-knowledge boundaries
   must hold throughout.

This is a medium refactor, plausibly several hundred lines across roughly a
dozen consumers plus tests, rather than a new movement scheduler. A rough
engineering budget is a few focused development days for reliable free-flow
quotes and integration, followed by separate crowding and gameplay calibration.
Exact future congestion prediction would be a much larger coordination problem
and is not recommended as the next step.

The scratch examples compile, format and pass Clippy with warnings denied. The
measurements and comparison assertions are retained under
`review-movement-assembly/review-comments/investigation/`, including per-variant
binaries, traces, results, source and the motor prototype. Production simulation
source was unchanged during this investigation. The previously reported
full-workspace timeout remains unresolved, so the follow-up commit and push
remain pending.

### 9. Approved closure after the investigation

Connor subsequently approved the completion-ceiling change, final validation and
ordinary follow-up publication. This closes the pending decision and publication
status recorded at the end of the section 8 investigation. The balanced-mirror
test now permits 50,000 ticks and still requires a decisive victory and no
rejected commands. The movement and animation improvements, collision sliding
and version 0.16.0 are retained.

Forecasting implementation is deferred to
[Account for turning and motor state in bot travel forecasts](https://linear.app/cluebbehusen/issue/CL-18/account-for-turning-and-motor-state-in-bot-travel-forecasts),
created with the Oxide Bug template at Low priority in Backlog. It includes the
actual move, detour, construction and haul measurements, the limits of scalar
route costs, and the bounded shared-motor approach. No production bot forecast
changed in this follow-up.

- Final validation passed with fixture blessing disabled: the complete workspace
  suite, including the formerly failing balanced mirror, formatting, Clippy with
  warnings denied, rustdoc and Markdown checks. Unit and combined coverage
  passed at 90.41% and 91.78%; their measured production code was unchanged by
  the completion-ceiling adjustment. All canonical skill directories passed
  validation. The signed follow-up contains the three reviewed fixes, focused
  regressions, artillery fixture setup corrections, approved 0.16.0 fixture
  refresh and durable findings; scratch research remains outside the commit.

## Actions

- [x] Reconstruct the original Oxide work, independent Python experiments, and
      Rust integration from their evidence.
- [x] Verify baseline identities, retained-work boundaries, and evidence links.
- [x] Assemble and measure the retained main-based motor and tread presentation
      with collision sliding preserved.
- [x] Obtain the compatibility decision, regenerate approved fixtures, and
      complete the unfiltered gates.
  - Human acceptance and all required gates completed before publication. Connor
    then authorized committing, pushing and opening the PR. The research
    sources, metrics harness, native captures and sandbox remain outside the
    production change; the comprehensive closure note is included.
- [x] Review the final native clips with Connor before treating appearance and
      feel as accepted.
  - Native acceptance came from the live sandbox with 12 Harvesters, 12
    Sentinels, 8 Breakers, 8 Wardens and six rich-scrap clusters. The screenshot
    and scenario are retained in the assembly artifact directory.
- [x] Prepare validated fixes for coasting stationarity, projectile motion
      snapshots and the renamed Overseer workflow for the authorized signed
      follow-up commit and ordinary push.
- [x] Isolate the match-duration change by review fix, inspect the first
      divergence, and measure travel forecasts against complete worker and
      builder jobs before recommending a forecasting change.

## Open Questions

- General non-sliding RTS coordination remains an unsolved product question for
  this project. It is deliberately shelved, not an active task for the next
  agent.
