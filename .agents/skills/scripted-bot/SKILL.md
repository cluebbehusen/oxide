---
name: scripted-bot
description:
  Design, change, debug, and evaluate Oxide's fair rules-based opponent. Use for
  Brain, UtilityPolicy, Dials, Observation, Intent, Executive, SeatBot,
  BotConfig, scripted openings, economy, scouting, tech, combat, expansion, team
  conduct, liveness, bot replays, bot difficulty proposals, or whether a match
  looks credible and fun.
---

# Oxide scripted bot

Build one opponent that plays a recognizable, complete game under the same
constraints as a person. Do not turn aggregate success into a claim that its
matches are sensible or fun.

## Preserve the level playing field

The bot is an ordinary command source. It receives a fog-honest observation and
may issue only shared `PlayerCommand` values. It gets no hidden income, vision,
stats, prerequisites, queue space, build privileges, movement, or combat rules.

`PublicMapBriefing` may explicitly provide the authored facts disclosed before
play: static terrain, Extractor frames, initial scrap, teams, and starting
Foundry anchors. Treat starts and resources as priors, never as current enemy
contacts or live amounts. Other hidden state and omniscient driver views may not
influence a decision. When behavior needs dynamic information the observation
does not contain, either add a fog-honest observation with tests or design
behavior that does not require it.

## Know the controller stack

The maintained path is:

```text
immutable PublicMapBriefing + fog-honest Observation
  -> oriented public priors + StrategicIntelligence
  -> persistent playbooks + UtilityPolicy Intent
  -> Executive
  -> PlayerCommand[]
```

- `Brain::scripted` is the configurable player-facing controller;
  `Brain::balanced` is its default-profile convenience constructor.
- `BotConfig::scripted` records difficulty, stance, and personality seed.
- `ResolvedProfile` and `DifficultyTuning` derive stable strategic and cognitive
  dials before play begins.
- `StrategicIntelligence` separates current evidence from timestamped memory.
- `StrategicPlanner`, `LiftPlanner`, `RaidPlanner`, and `TeamReliefPlanner`
  retain phased operations across decisions, reserve exact units, and budget
  committed scrap.
- `sim/src/bot/strategy.rs` owns air operations, `sim/src/bot/lift.rs` owns
  severed-ground transport operations, and `sim/src/bot/routing.rs` owns their
  fog-honest route projection and exact command-subset checks.
- `UtilityPolicy` fills work not claimed by those operations, while `Executive`
  owns exact-unit bookkeeping and lowers every intent to commands.
- `Brain::overseer` is a separate stable QA anchor. Do not silently change it
  while tuning the playable opponent.
- `seat_bots` constructs controllers requested by scenario `BotConfig`.
- Replays preserve the exact configuration and emitted commands, so playback
  does not rerun the controller.

Keep policy memory controller-local and deterministic. A resumed replay rebuilds
controller memory by observing the authoritative recorded prefix; never add an
unrecorded state mutation to make resume convenient.

Admit new persistent strategic work on the shared 24-tick boundaries, not on a
difficulty's private think cadence. Air, lift, and raid admission,
remembered-to-current air promotion, and the initial team-relief pressure watch
must sample a world tick available to every rung; apply rung-specific reaction
and commitment latency afterward. This keeps a faster controller from freezing a
different roster or contact merely because it sampled between shared boundaries.

Treat renewable economy as a strategic demand problem, not a bot-only entity
cap. The player-facing opening preserves the exact costs of its fourth Harvester
and a visible home Extractor restoration while establishing its difficulty core.
The frame must be supported by the exact living authored starting Foundry and
have an exact safe builder. Restore only after the whole frame is explored, and
pause while current sight or recent local salvage evidence makes its footprint
unsafe. Keep persistent combat rally points off known frames so restoring one
cannot invalidate an existing order.

Once a built Fabricator unlocks expansion Foundries, an owned completed
Extractor without completed or projected support is a priority objective, but
only across fog-honestly known reachable ground. Reserve capital only when a
legal support footprint has a route-capable builder, and try other known
Extractors when the nearest one cannot be supported. Count paid and deferred
Foundry claims before promising another.

Do not impose a player-facing Foundry count ceiling. Rank every exact legal site
by the economic work it would add: the supported-minus-remote gain for owned
completed Extractors, Foundry drip only when the site serves an external
objective, and reduced hauling distance for currently visible positive scrap.
Calculate hauling savings from public-ground routes that avoid observed dynamic
danger, not geometric distance. Public unbuilt Extractor frames are scouting
priors, not live income. Start recurring payback only after construction
completes, and let greed plus genuinely uncommitted scrap extend a bounded
forecast rather than unlock the capability.

Expansion saving and construction must consume the same exact safe
worker-and-site assessment. Preserve the difficulty's unreserved ordinary core,
assign current and confidence-weighted remembered ground threats to the nearest
reachable Foundry, and charge a forward site one additional Sentinel-equivalent
when it advances toward an uncleared reachable public hostile start. Completed
ground defenses count only where their real weapon and sight geometry covers the
asset. A valuable unsafe site may prepare the exact missing ordinary core only
when its projected surplus and greed justify that security; wealth never waives
the requirement. Price each candidate after removing its own missing-security
cost from genuinely uncommitted wealth; buying that protection must reduce the
remaining cost and cash together so the opportunity does not invalidate itself.
Reserve Foundry capital only after projected security is ready, and keep at most
one unpaid Foundry claim. Once a safe opportunity owns a partial or complete
Foundry fund, close later voluntary production, construction, and paid-repair
spending until its exact `BuildWith` command is emitted. Automatic Repair Bay
pulses remain ordinary simulation spending.

Treat a generic frontier nearer to a known enemy Foundry than to any projected
own Foundry as enemy-controlled, not as a reason to hoard. Count completed,
upgrading, pending, and uniquely deferred Reclaimer income once when projecting
supply. Reclaimer construction should answer completed production demand and
known resource exhaustion; do not impose a fixed count ceiling that a human
player does not share. Preserve Overseer's documented legacy Foundry cap and
policy when evolving these rules.

For adaptive profiles, fill an ordinary unreserved ground core before optional
specialties. HP-weight live Sentinel, Warden, and Breaker hulls; count queued
and same-think orders exactly once; exclude exact persistent-operation
reservations but not ordinary Executive armies. Protect difficulty floors of
four, five, six, and eight Sentinel-equivalents for Scrapheap, Standard,
Veteran, and Prime. Stance and personality must not alter those floors.

While below the floor, pause voluntary capital, upgrades, discretionary
production, mobile support, paid repairs, and new strategic operations. Existing
operations may advance or release units but must not purchase, and paid sites or
queues remain intact merely because the core fell; independent current-danger
safety rules may still cancel an unsafe unattended defense site. Permit only the
fourth Harvester, the exact safe authored home Extractor, one Turret for a
current visible ground threat, and one Flak Turret for a current visible
ground-attack aircraft. Pure air-to-air aircraft do not threaten the defended
ground assets and cannot unlock emergency Flak. Memory, public starts, radar
blips, and raid history are not emergency evidence. A think that begins below
the floor remains recovery-gated until the next observation: same-think core
orders count toward the projected floor so the bot does not over-order, but they
do not reopen voluntary spending or strategic admission during that think. Once
the floor is observed, voluntary capital must leave a Sentinel that remains
shallow after the upcoming production phase or preserve its exact cost, unless
honest known routing proves no ground objective exists. A lone existing
front-slot Sentinel may complete before a deferred founder pays and is not
enough. Keep the exact reserve in the bank while an unpaid founder travels;
after payment, return it to shallow production before another voluntary project.
Reapply the gate whenever projected core strength later falls.

Keep one baseline Tender; each additional Tender up to the seeded support
ceiling needs a distinct currently wounded ground combatant reachable over known
terrain. Count live, queued, and same-think Tenders once, and release the
specialist fund when that demand disappears. Fill shallow Foundry queues
breadth-first and reserve a remaining shortfall without double-counting the
ordinary fighting reserve. Generic production must not create partial bomber or
ground-attack-air cohorts; a persistent air or lift operation owns those cohorts
and its outstanding Airworks capacity. A shallow independently useful
air-defense purchase is allowed only when no operation owns that capacity.

Defend every completed owned Foundry, not only the starting base. Choose sites
for Turrets, Bastions, Flak Turrets, Scuttle Charges, and Barricades from
exposed strategic value and credible hostile approaches. Score each kind's
actual firing, spotting, trigger, or path-disruption geometry; preserve builder
egress and resource access; and treat unfinished defenses as reservations rather
than live fire. Predict the exact ordinary builder route with the public static
terrain the bot was briefed on, while taking dynamic blockers only from current
observation. Current contacts and remembered sites remain stronger evidence than
an uncleared public starting prior. Keep the frozen Overseer's legacy placements
separate from this player-facing policy.

Treat an Array as a persistent sensor, not as an unarmed defense. Search within
its radar radius of home for the most usable map coverage, preferring coverage
not already supplied by an allied Array and using current contacts, remembered
contacts, then uncleared public starts to break equally useful ties toward a
credible approach. Off-map area and Peaks provide no detection value because no
unit can occupy them. Preserve active resource access and bind the exact
ordinary route-capable builder proven through public terrain and current dynamic
danger. Allow partial coverage on maps smaller than the radar diameter. Keep the
frozen Overseer's first-valid Array placement unchanged.

Harvest work must also respect anonymous regional loss evidence, but a wreck
near a dead combat unit is not automatically a dangerous replacement source. Use
an authoritative incident as immediate short-lived caution, and promote it to a
durable quarantine only when a matching own or visible allied Harvester lost HP
or disappeared near its position or active source. Retain no attacker identity.
Keep distinct incident centers when their danger regions overlap; coalescing
them to one center can reopen part of a kill zone before it is swept. Renew only
an exactly repeated center. After current warnings and projected danger clear,
scout every still unseen safe tile in the exact quarantined region through a
bounded, deterministically ordered sweep. Clear the region on complete safe
coverage; danger, an unreachable target, or no progress must recall the scout,
reserve it until it is observed back in the safe home area, and only then
schedule a bounded retry. An idle body or elapsed timer in the field must not
release it into another role or a replacement loop.

Treat severed-ground attacks as coordinated operations, not a singleton ferry.
Let a lift's payload and matching carrier target grow while it remains in
Provision, then freeze exact, disjoint manifests when Boarding begins. Derive
carrier demand from that payload and usable landing space instead of imposing an
arbitrary controller cap, and retain a ground-capable home-defense floor. Launch
only after a shared boarding quorum. Bound every provision, boarding, landing,
and recovery phase; an incomplete wave must recover or shrink deterministically
rather than leak one carrier at a time.

While a remembered, built objective is being reacquired, reserve at most the
first Skyhook's exact cost only when optimistic fog-honest routing still proves
the target ground-disconnected, a completed Airworks and transportable payload
exist, and no usable carrier is live or queued. Treat this as capital only. Do
not start the lift, claim riders, or queue the carrier until current evidence
admits the ordinary operation, and release the reservation when any premise no
longer holds.

A wealthy island bot should consider a screen and bomber wing even when air is
not its seeded specialty, because personality may change emphasis but cannot
remove the only credible attack domain. Let airborne screen and bomber targets
grow during Recon and Assemble, then freeze the requested force when the
operation enters SuppressAa. Use current sight for uncoordinated commitments,
remembered objectives only for honest reconnaissance, and currently visible flak
along the complete known corridor for suppression. Air and lift plans must
remain independently viable. When both choose the same objective, coordinate
only through an explicit target-specific hold, release, or abort signal; neither
may infer the other's success from missing omniscient state.

Aircraft that can land are ground bodies while parked. `UnitObs::grounded`
reports that physical fact for own, allied, and visible enemy units, and
`UnitObs::body_domain` resolves the domain a body occupies right now; hit
legality, focus fire, matchup strength, and pressure censuses read it, while
routing, air-defense exposure, and procurement keep the kind's flight domain
because the next flight is planned in the air. There is no landing command: a
flier's ground destination is a landing, so the player-facing planner holds its
bomber wing by moving it to a deterministic pad near the home anchor through
`Intent::MoveUnits`, while a fixed-wing screen keeps an airborne hold over home.
The later strike order lifts the parked wing off exactly as a person's would.

Distinguish an empty scout slot from a lost dispatched scout. The planner may
fill or train the former before commitment. The latter must abort into bounded
recovery, release its factory bank, return surviving claimed units once, and
respect a cooldown rather than drafting a replacement into an endless probe
loop. Cover Recon and Assemble separately because both phases can otherwise
replace a missing unit before loss handling observes it.

Audit the utility scout separately from the persistent air planner. After the
first dedicated flyer is dispatched and lost, suspend that production channel
and release its Airworks capital. Rearm only after actionable current enemy
sight has gone dark after the loss and later returns; persistent sight,
remembered ghosts, and cross-sight between opposing dedicated scouts do not
count. Keep recomputable demand from public-start connectivity and current
contested-recon eligibility separate from the persistent latch set by a proven
ground-probe failure. Temporary map priors must clear when current sight changes
their route, while a lost or unsafe probe must remain learned. Exercise this
through the whole `Brain` or utility economy path because an isolated
strategic-planner test cannot see the solo scout conveyor.

## Keep identity and difficulty honest

The personality seed resolves independent, stance-bounded preferences for air,
siege, support, fortification, greed, and guile. These rank otherwise legal
choices; they never alter vision, costs, prerequisites, capabilities, or unit
strength, and they never roll private competence such as strength-estimation
accuracy. Expect each axis to leave an observable signature: air in wing size
and timing; siege in artillery volume and preference; support in support units,
flak, and allied relief; fortification in turrets, mines, and defensive reserve;
greed in worker targets and renewable-expansion payback appetite; and guile in
raid size, timing, withdrawal, and some mine or airborne-screen emphasis. Store
the seed in the scenario and replay rather than serializing resolved traits or
planner state.

Keep early defensive choices bounded across identities. After the protected core
is projected, one perimeter turret may precede contact once the enemy has been
located; unlock the remainder of a fortification target only after a real raid.
Below the core, only current visible armed evidence may justify the one matching
emergency Turret or Flak Turret. Emergency Flak specifically requires an
aircraft capable of attacking ground; a pure air-superiority flyer is not a
threat to the defended ground assets. Anonymous radar blips are not confirmed
air and must not independently trigger flak construction.

Scrapheap, Standard, Veteran, and Prime use the same strategic repertoire.
Scrapheap alone thinks less often; Standard, Veteran, and Prime intentionally
share one competent decision cadence because additional controller APM must not
become a disadvantage. Higher rungs still react sooner, remember more, service
no fewer simultaneous concerns, use a smaller fixed conservative error in
private estimates, coordinate focus fire, and hesitate less before commitment.
Prime also directs an overlapping static-defense line through the same explicit
focus-fire command available to a person. It locks one currently visible ground
threat until the target or firing overlap disappears; blocked or out-of-range
defenses retain the simulation's ordinary target fallback. No rung may receive a
rules advantage or lose an entire strategy merely to become easier.

Keep those limits structurally monotone. Lower-rung decision ticks must nest
inside higher-rung schedules; reaction and commitment windows must become no
slower as difficulty rises; and attention and memory must become no smaller. The
four, five, six, and eight opening-core floors must remain monotone and
independent of stance and personality. Lower rungs use a fixed deterministic
underestimate of their own force, so they may miss a marginal opening; Veteran
and Prime coordinate whole-army focus, while Scrapheap and Standard rely on
ordinary unit acquisition. Veteran and Prime share the same optional-operation
attention ceiling: neither peels a raid off while air and lift work already run
together. Personality must never change these competence limits.

A successful New Match chooses new personality seeds. Restart, Rematch, save
loading, and replay reconstruction must preserve the recorded difficulty,
stance, and seed.

## Change one behavior at a time

For opening-economy work, cover exact floor boundaries, HP weighting,
live/queued/same-think accounting, strategic-reservation exclusions, and every
blocked spend channel. Exercise both current-threat emergency domains and
noncurrent negatives; exact home-frame identity and builder safety; shallow
Sentinel, exact-remainder, and no-ground-objective boundaries; a later core
loss; and active planners advancing without purchases while new admissions stay
closed.

State the player-visible problem before tuning. Good targets are concrete:

- leaves harvesters idle while known safe scrap exists;
- repeats an impossible build forever;
- never reaches a named tech rung on a map where it can;
- sends an army through a visible losing fight;
- hoards through a winning window;
- stops issuing meaningful commands for a long interval.

Capture the smallest deterministic scenario and seed that exhibits the problem.
Test the observation, intent, or lowering layer that owns it. Avoid adding a
special case in a later layer to hide an earlier bad decision.

Use explicit stable tie-breakers and dedicated RNG streams for genuine seeded
variation. Do not use randomness to make a broken policy harder to diagnose.

## Evaluate from cheap to expensive

Run focused bot tests first:

```sh
cargo test -p oxide-sim --test bot_brain --locked
cargo test -p oxide-sim --test bot_policy --locked
cargo test -p oxide-sim --test scripted_bot --locked
cargo test -p oxide-sim --test overseer_015 --locked
```

Then run complete seeded matches on representative shapes: a normal duel, an
island or severed-ground map, a team map, and a long or grand map. Ask the
driver for its current syntax rather than copying stale flags:

```sh
cargo run -p oxide-driver -- run --help
cargo run -p oxide-driver -- replay-summary --help
```

The complete-match path is `run <scenario> --all-bots`; ordinary `--bots` honors
the scenario's configured chairs and therefore leaves its human chair under
human control. Add `--save-replay <path>` for review evidence.

Sample every difficulty and stance across the review set, plus multiple
personality seeds. Lower difficulty is not required to lose every paired match,
but its cognitive limits should remain visible and internally consistent.

Use `bot-eval` for reproducible player-facing profile cells. It stops when the
match decides, emits one compact JSONL row per leg, and can preserve the replay:

```sh
cargo run -p oxide-driver -- bot-eval skirmish \
  --difficulty prime --stance balanced \
  --scenario-seed-base 7000 --personality-seed-base 9000 \
  --paired --candidate candidate-a --out replays/bot-eval.jsonl \
  --replay-dir replays/bot-eval
```

For the maintained Prime-versus-Overseer yardstick, keep Overseer confined to
the evaluation-only `--against-overseer` path. Do not encode it in `BotConfig`,
a scenario, or player-facing match setup. Run a controlled paired block across
both faction assignments and both map-end geometries:

```sh
cargo run -p oxide-driver -- bot-eval skirmish \
  --difficulty prime --stance balanced --against-overseer --paired \
  --ticks 60000 --scenario-seeds 7000,7001 \
  --personality-seeds 9000,9001 --faction-cells fc,cf \
  --geometries authored,rot180 --overseer-policy-seed 0 \
  --candidate prime-overseer-a \
  --out replays/prime-overseer-a.jsonl \
  --replay-dir replays/prime-overseer-a
```

`--paired` exchanges the two complete command sources while holding each
transformed scenario fixed. `--overseer-policy-seed` fixes Overseer's legacy
army-size jitter to one identity that moves with the controller; it defaults to
zero and must not vary with the simulation seed. Crossing
`--faction-cells fc,cf` with `--geometries authored,rot180` separates controller
performance from physical seat, faction roster, and authored map end. Supply
independent `--scenario-seeds` and `--personality-seeds`: simulation randomness
and Prime's deterministic profile are separate factors, and the evaluator
crosses the two lists instead of confounding them. Use `--runs N` for simpler
consecutive seed cells outside this controlled workflow. The evaluator must
refuse nominal axis cells that resolve to the same executable matchup.

Use the per-unit stall breakdown to distinguish one blocked order from a broad
command failure. A leg ends as `termination: stall_loop` once one unit stalls
the same way `--stall-loop-limit` times (200 by default, 0 disables); that row
names the seat, unit, reason, count, and tick, and is an anomaly to inspect, not
a result. `--against-overseer` refuses maps whose seats share no ground route,
because the frozen Overseer has no severed-ground play and is not a valid
yardstick there. Treat rejections, stalls, and outcomes as diagnostic evidence,
not a quality score. Persisted evidence requires an explicit stable
`--candidate`; replay evidence also requires its JSONL `--out` sidecar. Rows
record the complete scenario and execution fingerprints, the exact Overseer
policy identity, a seed-independent command-stream hash, and the requested tick
limit. Use repeated command hashes to identify seed cells that generated the
same play rather than counting them as independent samples. The driver stages
the whole invocation, rolls back normal publication errors, and refuses to
replace an existing JSONL or replay. This is not a cross-path crash transaction:
abrupt process termination can leave hidden staging files or a partial replay
set. Inspect and remove the incomplete batch, then rerun it under a fresh
candidate rather than treating those files as complete evidence.

For each candidate, preserve the scenario, seed, replay, final hash, result,
duration, and a short behavioral verdict. Compare repeated identical runs for
exact hashes. Check that the controller:

- keeps an economy alive and replaces losses;
- builds and uses the reachable tech tree rather than merely owning it;
- scouts, reacts to discovered threats, and attacks through legal knowledge;
- escapes or changes plans after a failed route or site;
- behaves coherently after the opening and through the match's end;
- remains active on every seat, faction, and team shape in scope.

When paired results follow the physical seat, reduce the divergence to a
half-turned scenario before tuning policy. Compare authoritative state after
each relevant tick phase and audit equal-cost A* ties, footprint doorsteps,
production spawns, blocked group-goal snapping and spreading, signed fixed-point
vector scaling, and perfectly stacked collision separation. Outcome-relevant
tie-breaks belong in a query-, local-, or map-relative frame; an absolute
row-major or compass preference can turn a mechanical asymmetry into a false
personality or difficulty signal.

Use `replay-summary` to find long silences, nonsense loops, missed tech,
one-sided non-participation, and suspicious endings. Then watch the suspicious
and representative replays. Metrics are a triage tool; they do not certify
credible play.

## Promote by human judgment

A scripted change is not done because it wins, decides more games, or improves
an average. Play against it and watch full matches beyond the opening. Record
what the bot appeared to be trying to do, where that intention became legible,
and where it behaved nonsensically.

Review every difficulty and stance as the same opponent under explainable
cognitive limits. Do not promote a rung because its win rate alone looks
plausible, and do not hide a broken strategy behind personality variation.

Finish with the full Rust gates from `AGENTS.md` and the native QA path from the
`oxide-live-qa` skill whenever setup UI or player-facing behavior changed.
