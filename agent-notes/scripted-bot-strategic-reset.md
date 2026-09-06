---
created: 2026-09-02T05:47:32
updated: 2026-09-06T19:47:17
---

# Scripted Bot Strategic Reset

## Goal

Evolve the player-facing bot into a fog-honest strategic allocator that scales
coherent plans from observed opportunity, available resources, and personality
without arbitrary controller caps.

## Decisions

- Treat `docs/bot-strategy.md` as the normative strategic model for this
  workstream.
- Use a strangler migration. Preserve unmigrated behavior until its replacement
  is exercised, then remove the legacy path rather than maintaining two
  permanent controllers.
- Preserve the valuable controller boundaries: fog-honest observation,
  timestamped intelligence, persistent domain planners, exact reservations,
  Intent lowering, ordinary commands, deterministic replay, and frozen Overseer.
- Generalize only proven coordination seams such as investment cases, resource
  claims, production demand, capacity forecasts, and exact ownership. Keep
  targeting, placement, tactical phases, retreat, and micro domain-specific.
- Prefer exhaustive proposal and claim types over a universal optimizer, generic
  operation framework, or opaque aggregate score.
- Apply personality at both layers: it biases allocation among domains and
  choices within a funded domain, but never removes access to a strategy, unit,
  command, fact, or rule.
- Keep difficulty orthogonal to personality. Every rung retains the complete
  repertoire; difficulty changes only fair cognitive and execution competence.
- Treat forecasts as evidence, never credit. Commands spend only current scrap,
  and projections include only completed recurring income and completed
  production capacity with explicit uncertainty.
- Let proposals revise before commitment. At the domain-specific commitment
  boundary, freeze exact sites, builders, factories, units, routes, and
  current-scrap claims until completion, cancellation, or bounded recovery.
- Do not change unit or building balance as part of this workstream without
  separate human approval.
- Keep Overseer as a frozen QA anchor. Metrics and match outcomes expose
  failures; representative replays and human play remain the promotion gate for
  credible and fun behavior.
- Treat the merged opportunity-scaled Foundry work as the first domain
  precedent. `scripted-bot-opportunity-scaling.md` owns its history; this note
  owns the remaining strategic migration.
- The migration is complete only when every voluntary spend traces to an
  accepted proposal, every rejected opportunity has an intelligible reason, and
  no arbitrary player-facing unit, army, or expansion cap remains.
- Deliver the reset as ten stacked pull requests in dependency order. Each slice
  introduces at most one coordination seam and exercises it through real
  behavior.
- The user explicitly approved refreshing behavior fixtures whenever needed
  across this stack. Intermediate PRs remain on simulation version 0.16.0
  because their final combined hashes will ship under 0.17.0.
- The tenth and final PR owns the workspace and `SIM_VERSION` bump to 0.17.0
  plus the final fixture refresh. No earlier PR changes either version.
- Approved a six-minute pre-contact defense bound while retaining exact
  allocation, placement, completion, and command-legality assertions. Economic
  investment may precede voluntary static defense; the prior diagnostic placed
  it at tick 5808.
- Prioritized cheap, behavior-preserving performance improvements before
  publication. The user accepts remaining overhead for this slice if removing it
  would require major architectural changes; preserve the measured cost and
  profiling evidence instead of expanding the migration into a redesign.
- Approved including the narrow pre-existing allied-frame home-Extractor
  recovery fix in the economic migration, with a regression and renewed
  validation before publication.
- Hash and version approval is an implementing-agent responsibility. Reviewers
  should assess technical compatibility, not flag missing approval that may have
  been granted outside their context.
- Approved concurrent independent reconnaissance questions, bounded value- and
  safety-based retries without mandatory fresh enemy sight, and demand-driven
  raid procurement while retaining raid execution and its tactical pair minimum.
  The implementation remains unpublished until separately authorized.
- Authorized committing, pushing, and opening the reconnaissance/support PR
  after implementation, complete validation, replay/native review, and recorded
  limitations. Keep the PR description empty under the repository workflow.
- Approved removing only the defense integration test requirement that
  construction precede first enemy contact, because question-driven
  reconnaissance legitimately discovers the enemy earlier. Preserve the
  six-minute bound, exact allocation and command ownership, legal placement, and
  forward-approach checks.
- Authorized fixing bugs discovered during implementation and final review
  without an additional permission checkpoint. Include the narrow player-facing
  eliminated-seat guard now; retain the frozen Overseer and ordinary simulation
  command rules.
- Expanded battlefield adaptation to purposeful ground missions, strategically
  motivated scouts and Arrays, and bounded contextual/doctrine experience.
  Observed movement remains fog-honest; predictive opponent modeling and final
  calibration are deferred. Implementation approved without publication.

## Findings

- The existing domain execution boundaries are strong enough to preserve. The
  coordination layer, where sequential planners rewrite available budgets and
  implicitly arbitrate spending, is the reset target.
- Opportunity-scaled Foundry expansion is already merged and provides a concrete
  precedent for marginal-value investment without a player-facing count ceiling.
- The current planners often represent a rejected or absent candidate as
  silence. PR 1 cannot truthfully reconstruct proposal forecasts or rejection
  reasons without changing policy, so its trace must not infer them by rerunning
  predicates.
- At the start of the reset, connected operations assembled a fixed two-Bombard
  and one-Moth cohort, while ordinary production fell back to repeated Sentinel
  and Lancer demand. PR 3 replaces the fixed operation cohort; PR 5 owns the
  ordinary-production fallback.
- A repeated Paired Claims team probe exposed a pre-existing restoration retry
  storm: bots repeatedly issue rejected Extractor builds against contested
  frames hidden by fog. The selection and reissue path is unchanged from the
  Action 5 base and belongs in a separate bug fix.
- Terminal Basin seed 20260814 with personality seeds 9008-9015 exposed a
  pre-existing home-restoration omission: seat 1 retried an allied Extractor
  frame (13,23) 143 times from ticks 8568 through 11976. At tick 11976, allied
  seat 0 Extractor 109 occupies the frame in both authoritative state and seat 1
  knowledge. The selector checked own/enemy buildings but omitted allied
  buildings; the approved narrow fix adds that same occupancy check for allies
  without changing frozen Overseer or simulation rules.

## Actions

- [x] 1. Make strategic decisions explainable through a deterministic decision
      trace and representative baselines.
  - Boundary: observe and report the current controller decisions without
    changing policy, command order, authoritative state, or replay semantics.
  - Record only facts the current coordinator actually owns: control flow,
    explicit gates, scrap holds, planner lifecycle and effects, exact claims,
    commitments, utility output, and lowering. Extend the schema with real
    proposal evidence and rejection reasons when those concepts exist in later
    slices.
  - Close only after identical seed and command inputs retain their existing
    command and state hashes, trace output is deterministic and bounded,
    representative baselines are preserved, and the driver exposes every
    coordinator gate or idle outcome it can state truthfully without omniscient
    data.
  - Added an opt-in, schema-versioned decision trace at player-facing decision
    ticks, limited to fog-honest facts already owned by the coordinator and
    excluded from authoritative state and replays.
  - Streamed trace sidecars transactionally with compact evaluation evidence,
    linked by evaluation fingerprint, leg, seat, and tick; unfinished streams
    cannot be published.
  - Verified that traced and ordinary runs produce identical commands, replays,
    and state hashes; trace rows are deterministic and bounded to actual
    decision ticks, while Overseer and cadence skips emit none.
- [x] 2. Establish one authoritative typed resource and commitment model.
  - Represent current scrap, conservative forecasts, builders, producer lanes
    and time, exact units, and existing obligations once; migrate a real
    consumer and remove its doctored observations and ad hoc budget plumbing.
  - Boundary: replace one consumer at a time, beginning with merged Foundry
    expansion, while unconverted planners contribute explicit legacy claims and
    retain their current priority.
  - Cover current versus forecast funds, live and queued work, same-think
    reservations, release and rollback, exact builder and unit ownership,
    producer-time conflicts, and prevention of double counting; do not change
    cross-domain arbitration yet.
  - Built one fog-honest resource snapshot for each player-facing utility pass,
    separating current scrap from conservative completed-income forecasts and
    recording exact builders, obligations, units, producer queues, timing, and
    egress evidence
  - Added a deterministic commitment ledger for current-bank spending and holds
    plus exact unit, builder, site, and contiguous producer claims, with atomic
    rollback and owner-scoped release
  - Adapted upstream strategic commitments, reserved units, queue appends,
    persistent Foundry saving, and deferred foundations into explicit owners
    while leaving unconverted channel priority intact
  - Migrated Foundry saving across decisions: it freezes one exact site,
    builder, and admission fund; yields to survival or required preparation;
    survives unrelated lowering; and releases on exact dispatch, invalidation,
    or bounded recovery
  - Covered current versus forecast funds, canonical ownership, producer timing
    and conflicts, gross-versus-net legacy decisions, strategic competition,
    safety-guard changes, rollback, refusal, timeout, mirrored lowering, and
    successful release with focused regressions
  - Extended deterministic decision traces with bounded resource, builder,
    producer, and saved-expansion evidence; refreshed only the approved
    player-facing behavior rows while frozen Overseer hashes remained unchanged
  - Verified the resource boundary against authoritative income cadence and
    support-radius edges, live producer blockage and recovery, current tech
    prerequisites with prepaid queues, both operation-versus-expansion admission
    orderings, mirrored exact lowering, and recovery-clock renewal
- [x] 3. Replace fixed connected-operation rosters with opportunity-scaled force
      packages.
  - Derive capability demand and deterministic providers from target value,
    observed defenses, technology, existing forces, protected capital,
    throughput, personality, and a fixed horizon; freeze exact membership at
    commitment while preserving tactical execution.
  - Boundary: migrate only the connected air and siege operation plus the
    production demand it owns; leave raids, relief, lifts, defensive investment,
    and unit statistics unchanged.
  - Cover minimum viable packages, monotone useful scaling with wealth and
    throughput, tier-two and tier-three providers, observed AA and target value,
    personality emphasis without capability gates, non-extending deadlines,
    exact-ID freeze, abort, and recovery; evaluate across ordinary and rich
    maps.
  - Isolated prerequisite: classified connected versus island opportunities from
    the immutable public terrain briefing, so unexplored authored terrain cannot
    send a wealthy bot into the wrong doctrine.
  - Scope clarification: preserved the Recon -> Assemble -> Suppress AA ->
    Verify -> Strike -> Recover lifecycle and its existing move, hold, attack,
    and recovery mechanics. Changed admission, package composition, route
    preflight, live strike-target selection, and suppression-target selection.
  - Replaced the fixed connected cohort with a capability package derived from
    current target value, operational AA, completed production, available
    funding, technology, existing forces, and personality; revisions stop at
    exact-ID commitment and cannot extend the deadline.
  - Aligned reconnaissance, target-cluster liveness, artillery firing stands,
    group spreading, producer egress, and attack routes with public terrain and
    the authoritative command geometry; an infeasible preferred target now
    yields to the next viable current objective.
  - Evaluated 14 persisted ordinary- and rich-map matches with 34,338 trace
    rows. Connected packages ranged from the shared three-unit minimum to 15
    units, scaled suppression through Bombards and Avalanche against observed
    AA, substituted Buzzards and Condors at higher opportunity, and reproduced
    the same command and terminal hashes under an identical seed.
  - Closed adversarial review gaps: remembered frozen targets remain inside AA
    clearance until their full footprints are re-scouted, and admission now
    proves a distinct reachable firing stand for every suppression provider
    rather than extrapolating from one artillery piece.
  - Revalidated active hidden-target preparation against the latest spendable
    bank and surviving completed-income forecast, so a destroyed income source
    releases an infeasible package instead of holding resources until timeout.
  - Added an exact deterministic producer scheduler with no policy count cutoff;
    independent brute-force oracles matched 43,702 count-portfolio and
    lane-allocation cases.
- [x] 4. Admit compatible investments through a deterministic cross-domain
      allocator.
  - Boundary: compare structured proposals and claims while leaving each domain
    responsible for its own target, placement, composition, phases, and micro;
    adapt unmigrated work through explicit legacy obligations.
  - Cover hard survival constraints, already-paid work, compatible concurrency,
    mutually exclusive scrap, builders, factories and units, stable tie-breaks,
    personality weighting without zeroing a domain, and traceable approval or
    rejection.
  - First slice: compare exactly two fresh proposals, one currently safe and
    command-legal Foundry expansion and one connected-operation minimum. Treat
    saved Foundry plans, active operations, paid work, opening recovery,
    bootstrap work, emergency survival, and the shallow Sentinel as obligations;
    leave other fresh unmigrated channels on residual resources.
  - Select among the four possible proposal subsets with no search cutoff. Apply
    current and forecast scrap, exact builders, sites, units, and shared
    producer timing as one atomic claim bundle so higher-order conflicts cannot
    double-spend a forecast or factory lane.
  - Store exact proposal payloads and commit only the accepted site, builder,
    target, minimum package, and production evidence without rerunning domain
    ranking. Scale connected marginal capability only from resources left after
    the accepted minimum and expansion.
  - Verify an independent four-mask oracle, atomic rollback, current and
    forecast conflicts, compatible concurrency, producer hyperedges, persistent
    obligations, deterministic lowering and tie-breaks, a real personality
    near-tie, and a composed state-accepted expansion-plus-offense case.
  - Implemented the pure two-domain allocator with exhaustive four-mask
    selection, named semantic bands, exact current and deadline-scoped forecast
    capital, actor and site ownership, producer FIFO scheduling, atomic
    rollback, deterministic ties, and additions-only connected scaling.
  - Rejected a producer abstraction that conflated command enqueue and payment
    with production start; allocation must retain decision-tick admission, FIFO
    start, slot reopening, strict readiness, and post-income spendability.
  - Completed cross-decision producer commitments for connected offense and
    lifts: retained jobs preserve exact identity, funding, lane, timing, and
    deadline; jointly validate shared capacity; emit due commands once; and
    enter bounded recovery when the accepted promise becomes impossible.
  - Brought admitted island-air work and current-threat emergency defense into
    the same transaction, then extracted post-allocation residual coordination
    so rollback has one explicit owner.
  - Fixed adversarial seam failures found by composed tests: chronological
    replay of retained lane work, same-tick offense versus saved-Foundry
    priority, stale tactical latches after roster growth, rollback after lost
    forecast backing, and absent planner-owned units reaching the live Utility
    ledger.
  - Removed the test-only legacy connected controller and unreachable
    player-facing Foundry and emergency-defense rungs. Shipped Brain-to-State,
    route-restricted production, exact rollback, and every allocation outranking
    basis now have direct coverage.
  - Deferred the reduced-observation and raw-budget adapter for unmigrated
    Utility channels, scheduler consolidation, trace and band-type
    consolidation, and physical test-module splits to the later domain
    migrations or final cleanup; do not extend those seams in the meantime.
  - Evaluated paired Prime personalities, controlled Prime-versus-Overseer
    matches, The Scattering to 60,000 ticks, and Skyhook Anchorage to 60,000
    ticks. No allocator stall loop or dead economy surfaced; duel outcomes
    remained map-end-confounded, while rich and island matches sustained
    expansion, high-tier production, and concurrent operations.
  - Verified frozen Overseer state hashes unchanged; refreshed only the approved
    player-facing behavior rows; full workspace tests, Clippy, rustdoc,
    Markdown, skill validation, and unit and combined coverage gates passed.
- [x] 5. Replace default unit sinks with tech-aware standing-army and
      ordinary-production demand.
  - Derive standing force from known counters, strategic plans, technology,
    capacity, and personality; remove Sentinel and Lancer as automatic fallbacks
    while retaining tier-one units as useful screens and counters.
  - Cover live, queued, reserved, and same-think capability accounting; current
    and remembered enemy counters; production bottlenecks; useful higher-tier
    substitution; home-defense floors; reinforcement; and avoidance of idle
    factories, hoarding, or partial strategic cohorts.
  - Chose StandingForce as the third fresh allocator domain so ordinary
    production competes with economic and connected-offense investment instead
    of receiving call-order leftovers.
  - Bound this slice to player-facing standing combat production. Preserve
    frozen Overseer, persistent operation execution, opening survival recovery,
    worker and technology policy, unit statistics, and strategic aircraft
    ownership.
  - Represent each cadence as ranked, mutually exclusive, current-funded
    one-unit alternatives derived from current and remembered threats, completed
    technology, exact live and paid queued inventory, planner ownership,
    expansion and paid-site security, reachable wounded support demand, useful
    ground objectives, public-terrain routing, and personality.
  - Retire the player-facing adaptive ordinary-combat scheduler after
    StandingForce ships. Preserve only post-bootstrap renewable Harvesters,
    Excavators, and the existing bounded Scuttler roster in a narrow residual
    Foundry pass until their owning domains migrate in Actions 7 and 8.
  - Verify tier-one fallback, useful higher-tier substitution, counter memory,
    exact operation ownership, grouped alternatives, current-only funding,
    residual construction progress, residual worker and raider demand,
    repeated-cadence factory use, command acceptance, and unchanged Overseer
    hashes.
  - Boundary: migrate repeatable ordinary line, siege, anti-air, and Tender
    production without rewriting persistent operation execution or changing unit
    statistics. Keep post-bootstrap Harvester, Excavator, and Scuttler demand in
    the residual bridge for later domain migrations.
  - Preserve the construction ladder's existing next-technology reserve while an
    eligible worker exists and construction is not recovering; after the tree is
    complete, preserve the exact legal strategic Turret threshold. The
    current-only floor remains unclaimed for Utility, never spends forecast
    income, and disappears when its non-scrap premises are unavailable.
  - Fixed conditional operation ownership by deriving separate Standing
    inventories for Connected absence, minimum, and every cumulative marginal.
    Exact live units and paid producer occurrences now leave ordinary
    availability only in the contexts where they are genuinely free.
  - Preserved independent same-kind demand on disconnected fronts by keying
    Standing alternatives to a canonical service point or footprint and
    requiring inventory and producers to serve that target through public
    terrain and current blockers.
  - Made an infeasible active Connected revision downgrade atomically: release
    its typed obligation and selected-only Standing contexts, retain surviving
    units in bounded recovery, and rederive unconditional Standing against the
    remaining exact paid work.
  - Extend portfolio selection to every zero-or-one choice within Foundry,
    connected offense, and StandingForce. Keep one shared allocation proposal
    case, claim bundle, and trace model, with no proposal-count or machine-word
    cutoff; consolidate the remaining producer schedulers during Action 10.
  - Verified the full workspace, Clippy, rustdoc, Markdown, skills, and coverage
    gates. Unit line coverage is 90.27% and combined line coverage is 91.72%;
    StandingForce is 98.33% unit and 98.86% combined, frozen simulation and
    Overseer hashes remain unchanged, and only the approved player-facing
    fixture moved.
  - Evaluated the shipped path on a controlled Overseer matrix, a paired rich
    map, an eight-seat severed-ground map, and a repeated 2v2 team map. Standing
    production stayed active, its training commands were accepted, available
    higher tiers dominated late production, and the 2v2 command log repeated
    byte for byte; Prime strength calibration remains later work.
  - Closed three adversarial review gaps: delayed connected purchases stay
    operation-owned until the observed queue occurrence leaves; active revisions
    preserve only the current scrap guard left after mandatory work; and bounded
    higher-tier waits now claim current and forecast capital inside shared
    allocation while their affordable fallback remains selectable.
  - Restore an acceptable coverage runtime without weakening behavioral or
    coverage gates; keep the optimization behavior-neutral and retain the
    long-match oracles in the normal cross-platform test matrix.
  - Profiled the fresh 68-minute CI coverage run: three long-horizon behavior
    and hash oracles consumed 53 minutes while contributing about 0.07
    percentage points of line coverage.
  - Changed the residual Turret reserve to use the exact strategic-placement
    predicate but stop at the first valid site; actual defense construction
    still ranks every valid site globally.
  - Kept the long behavior and hash oracles in normal cross-platform tests,
    excluded them only from LLVM instrumentation, and serialized combined
    coverage. The complete instrumented scripted-bot suite fell from 174 to 54
    seconds locally; combined coverage completed in 180 seconds at 91.64% and
    unit coverage completed in 107 seconds at 90.29%.
- [x] 6. Migrate defensive spending to opportunity-scaled investment while
      preserving strategic placement.
  - Scale defense from exposed value, credible current threats, existing
    coverage, reinforcement time, and opportunity cost; preserve the established
    role-specific site scorer and keep emergency defense as a constraint.
  - Boundary: retain Turret, Bastion, Flak Turret, Scuttle Charge, Barricade,
    and Array placement geometry; replace only the decision about what defensive
    investment is worth funding and when.
  - Cover threatened expansions and production, approach-facing coverage,
    overlap and diminishing return, unfinished defenses as claims rather than
    firepower, builder and route safety, personality expression, emergency
    exceptions, and competing offensive or economic proposals.
  - Replaced voluntary defense admission with typed, current-funded alternatives
    for Turret, Bastion, Flak Turret, Scuttle Charge, Barricade, and Array;
    accepted work retains its exact role, site, builder, claims, readiness,
    evidence, and marginal value through commitment.
  - Kept current-threat emergency defense as a survival obligation while
    voluntary defense competes with Foundry, Connected, and StandingForce work
    in the shared deterministic portfolio.
  - Scored exposed structures and active resource work against current,
    remembered, and public-prior approaches; accounted for live versus
    unfinished coverage, overlap, reinforcement and construction time, builder
    danger, resource access, producer egress, and exact footprint compatibility.
  - Preserved every defensive role at every personality setting, used
    fortification, support, and guile only as bounded ordering signals, and kept
    Array valuation on usable novel radar coverage rather than weapon coverage.
  - Closed adversarial gaps in reflected builder routes, diagonal path
    companions, unfinished producer egress, Connected reinforcement ownership,
    and accepted-defense retention in bounded traces; focused and shipped-path
    regressions pass.
  - Profiled repeated candidate route searches and added exact endpoint caching,
    baseline-cost pruning, and Array row-prefix coverage. Terminal Basin at
    3,400 ticks fell from 31.22 to 10.15 seconds with identical outputs, versus
    5.85 seconds at HEAD; the full player-facing hash oracle fell from 655.69 to
    176.45 seconds. The remaining runtime overhead is a calibration and
    performance follow-up.
  - Passed workspace tests, Clippy, rustdoc, Rust and Markdown formatting,
    canonical skill validation, and both coverage gates. Unit line coverage is
    90.54% and combined is 91.88%; defensive investment reaches 99.17% unit and
    99.25% combined.
  - Refreshed only the approved player-facing state and command hashes at 0.16.0
    and verified them unblessed in the full workspace suite. Frozen simulation
    and Overseer behavior remain unchanged. Human play and final strength
    calibration remain Action 10 work.
- [x] 7. Migrate economy, technology, and production capacity away from
      arbitrary ceilings.
  - Choose workers, Extractors, Reclaimers, Foundries, factories, and upgrades
    from saturation, payback, bottlenecks, demand, and reachable opportunity;
    remove remaining arbitrary policy ceilings.
  - Boundary: reuse the merged Foundry opportunity model and convert the
    remaining economic, technology, and producer decisions incrementally rather
    than inventing a second economy controller.
  - Cover safe work capacity, replacement pressure, renewable income, haul
    savings, completed producer demand, upgrade capability value, construction
    delay, diminishing return, rich-map growth, and removal of worker,
    specialist, factory, and upgrade ceilings that lack game-world
    justification.
  - Implement the accepted economic-investment migration: safe work and
    capability demand, exact typed capital and worker choices, upgrade downtime,
    shared allocation, and bounded saving. Preserve opening recovery and frozen
    Overseer; leave raid, reconnaissance, and Repair Bay admission for Action 8.
  - Implemented typed economic proposals from finite safe work, pre-eligibility
    capability demand, exact capital/builders/sites/upgrade ids, refit income
    loss, and shared capacity. Added construction-backlog replacement valuation
    and capacity-only Foundry quotes through existing expansion security and
    saving. Retained opening recovery and the frozen Overseer; residual Repair
    Bay, scouting, and raid admission remain Action 8.
  - Regressions cover fixed-deadline saving and deferred foundation ownership,
    paid-work preservation, core loss and expiry, changed site occupation,
    retained Foundry footprint alternatives, three-foundation conflicts,
    prerequisite chains, paid/deferred capacity, and exact five-domain dispatch.
    Fixed a live cancellation loop caused by shallow Sentinel recovery revoking
    accepted construction, and prevented prior-based income demand from
    borrowing evidence from already-funded current work. Doctrine fixtures
    retain orphan recovery, scout ownership, and repair cancellation assertions.
  - Large-map review retains calibration limits: Terminal Basin remained
    undecided at ten minutes with late and uneven technology, while Skyhook
    produced air forces, transport loads, and multi-Foundry growth but seats 2,
    3, and 7 stopped exploring after early scout losses and banked substantial
    scrap. These observations do not constitute whole-match promotion;
    reconnaissance and outcome recovery remain Actions 8-10.
  - Cheap behavior-preserving optimization is complete: skip defense roles that
    cannot fit imported fixed capital, and skip standing-force demand projection
    when revalidating only deferred foundation travel. The isolated 3400-tick
    Terminal Basin benchmark improved from 22.50 to 13.64 seconds, versus the
    recorded 10.15-second baseline; final state 0x28883c0a2567af60 and command
    hash fnv1a64:f0135e5840cbe199 are unchanged, with zero rejections or stalls.
    Remaining defense-routing overhead is accepted under the user-approved scope
    boundary; broad geometry/cache redesign is deferred.
  - Reviewed complete Scrapheap/Turtle, Standard/Balanced, and
    Veteran/Aggressive Skirmish games, plus the six-leg Prime/Balanced batch
    across Skirmish, Terminal Basin, and Skyhook with personality seeds starting
    at 9000. All Skirmish games reached decisions without rejections or stalls.
    Four large-map legs reached the 12000-tick review limit; one Terminal seed
    exposed the allied-frame defect recorded in Findings, while both Skyhook
    legs had zero rejections and exercised transport and air capacity. Native
    Skyhook playback at tick 8953 confirmed the Load checkpoint. These are
    execution and review evidence, not human fun approval.
  - Included the approved allied-frame recovery fix. The regression failed
    before the selector change and now covers unfinished/completed allied
    Extractors, repeated decisions without a phantom capital hold, and
    eligibility after the frame becomes vacant. The exact Terminal Basin seed
    20260814 with personality seeds 9008-9015 now runs 12000 ticks with zero
    command rejections across all eight seats, versus 143 rejected restoration
    commands before the fix. Allied Extractor 109 remains visible on (13,23) at
    ticks 8568 and 11976; seat 1 never retries after its original tick-zero
    build. Replay reconstruction matches final hash 0x71078290e083a001.
  - Final validation passes: full workspace tests including unblessed
    player-facing oracles and frozen state hashes, Clippy, cargo check, rustdoc
    with warnings denied, rustfmt, Markdown formatting, all eight canonical
    skill validators, unit coverage at 90.49%, and combined coverage at 91.89%.
    The previously approved player-facing fixture refresh remains the only
    golden change; no further bless was needed for the allied-frame fix.
    Workspace and simulation versions remain unchanged.
  - Repeated the isolated 3400-tick benchmark after the allied-frame fix: 13.31
    seconds with the same state/command hashes and zero rejections or stalls,
    confirming the cheap optimization remains intact on the final tree.
  - The published-slice review reproduced shared Airworks time being reused
    across simultaneous air/lift demand and distant live harvesters entering
    work with zero travel. Previously green single-demand tests missed both
    cases. The follow-up regressions now cover them. The review also raised hash
    approval, already satisfied by the stack-wide same-version decision recorded
    before this slice.
  - Fixed shared Airworks accounting with deadline/readiness/route-bound joint
    time allocation. Live harvest workers now pay initial public-route travel to
    visible safe work, using one lazily built distance field per used region.
    Regressions cover both reproduced defects, restricted customers, residual
    reassignment, paid readiness, expired deadlines, and partial-unit returns.
  - Follow-up validation passes all workspace tests, Clippy, type checking,
    rustdoc, rustfmt, Markdown formatting, unit coverage (90.50%), and combined
    coverage (91.90%). Player-facing and frozen simulation hashes remain
    unchanged; no bless or version change was needed. The 12000-tick Skyhook
    replay preserves prior command/state hashes, eight Load commands (first tick
    8952), zero rejections, and prior sparse stalls; reconstruction matches
    0x841e2003b70e81e5. The isolated Terminal Basin 3400-tick benchmark takes
    13.54 seconds versus 13.31 before the fixes, with identical hashes and zero
    rejections or stalls.
- [x] 8. Migrate reconnaissance and support to information value and concrete
      operational demand.
  - Scout when resolving uncertainty could change a decision, and build repair,
    anti-air, escort, or relief support for concrete forces and threats;
    preserve the distinction among public priors, current sight, and memory.
  - Boundary: preserve existing scouting, recovery, raid, relief, and support
    execution where sound; replace quota and first-available admission with
    explicit information or operational value.
  - Cover questions whose answers can change investment, safe ways to answer
    them, stale and invalidated evidence, public-map priors, current threats,
    remembered uncertainty, repair demand, escorts, anti-air, allied relief,
    cancellation, and bounded retry.
  - Implemented seven-domain allocation with exact question and support
    ownership, owner-only repair-target observation schema 16, trace schema 10,
    cadence-funded exact-worker repairs, finite shared repair service, marginal
    Repair Bays, complete selected-layout validation, protective deployment,
    allocated pressure-driven relief, and objective-bound Scuttler procurement.
    Removed the migrated residual purchasers while preserving core protection,
    ordinary simulation rules, frozen Overseer, and operation execution.
  - Closed replay-discovered ownership and funding gaps: refresh outgoing claims
    after commit and rollback, exclude exact paid scout occurrences across
    operational and question planners, retain late living observers for recall,
    preserve fixed question deadlines, mature forecast funding beside older
    current-capital claims, and recover genuinely unfundable Lift schedules
    without releasing surviving payload membership. Concurrent questions retain
    independent loss, retry, recall, and safe-return evidence.
  - Reviewed fourteen cells: two Prime seeds each on Skirmish, Twin Forges,
    Skyhook, and Terminal Basin, plus two Skirmish seeds each for
    Scrapheap/Turtle, Standard/Balanced, and Veteran/Aggressive. All final cells
    have zero rejected commands, with at most three repeats of one stall reason
    on any unit. The corrected Twin Forges and Terminal Basin reruns remove
    exactly 196 and six post-elimination commands while preserving every other
    command and final hashes 0xc503daaa73583f91 and 0x37b81f7c6b0ea251. The
    guard covers Foundry loss and surrender at every difficulty, suppresses
    traced/untraced planner advancement, and leaves allies and frozen Overseer
    intact.
  - Preserved review evidence in /tmp/oxide-pr8.eWmKwE. Exact fog-seat snapshots
    and native playback cover post-loss observer reuse and recall, one Tender
    repair order taking an Avalanche from 180 to 300 HP, and exact relief
    deployment/withdrawal. The two Skyhook legs recorded 47 and 30 Load
    commands; the reviewed scout roster stabilized and reused survivors rather
    than endlessly purchasing replacements. Selected Standard, Skyhook, Twin
    Forges, and Terminal Basin replays reconstruct to their recorded hashes.
    Large-map outcome, late-tier and defense-routing limitations remain recorded
    under Action 10; no automated result establishes human fun or final
    difficulty calibration.
  - Passed final full workspace tests, focused and composed regressions, Clippy,
    cargo check, warnings-denied rustdoc, rustfmt, Markdown, all eight canonical
    skill validators, and sequential coverage: 90.45% unit and 91.94% combined
    lines. The approved player-facing fixture refresh passes unblessed,
    including after the elimination guard; frozen state hashes and
    workspace/simulation version 0.16.0 remain unchanged. The approved
    defense-test migration removes only pre-contact timing, retaining the
    six-minute bound, exact ownership, legality, and forward placement.
  - Repeated the isolated debug Terminal Basin 3400-tick benchmark after all
    code changes: 7.83 seconds versus the verified 13.54-second prior baseline,
    with zero rejected commands or stalls. Late Skyhook profiling still
    attributes most sampled bot time to existing defensive approach pathfinding;
    broader routing/cache changes remain a measured performance follow-up rather
    than part of this admission migration.
- [x] 9. Feed bounded deterministic outcome evidence back into future strategic
      decisions.
  - Record operation, route, harvest, and defense outcomes with confidence,
    decay, and invalidation; use them to reconsider future proposals without ML,
    cross-match state, omniscience, or oscillation.
  - Boundary: retain only controller-local evidence reconstructable from the
    recorded match prefix; do not add training, persistent opponent models,
    hidden state, or ambient randomness.
  - Cover success, partial success, abort, loss, unsafe routes, repeated harvest
    danger, effective defenses, observed counters, confidence decay, explicit
    invalidation, cooldown release, and deterministic reconsideration without
    permanent fear or rapid thrashing.
  - Started from clean main e01a449 on cjl/feat/adaptive-battlefield-strategy.
    Baseline artifacts: /tmp/oxide-pr9.HuHJ8c. Isolated debug Terminal Basin
    samples: 7.88, 7.70, 7.67 seconds; median 7.70 seconds. Action stays open
    through composed tests, full gates, replay review, and performance
    comparison.
  - Added owner-only carried identities, shared battlefield/movement assessment,
    bounded experience and explicit outcome journals, effective-return
    allocation plumbing, battlefield reconnaissance consumers, and strategic
    Array coverage. Ground mission and exact-member lowering integration is in
    progress. Initial experience (7) and battlefield (4) unit tests passed
    before later integration edits; full gates and behavior review are not yet
    complete. Baseline match batch is still running; do not rebuild or clean the
    release driver until it finishes.
  - Implemented and tested exact mission membership, bounded return behavior,
    two-front response selection, owner-only cargo accounting, and recoverable
    dispatched harvest/build attempts. The latest complete simulation unit
    checkpoint passed 1,413 tests; subsequent focused mission tests passed, but
    newer integration edits still require a fresh full run. Array approach
    weighting and post-delivery objective watches are now wired. Remaining work
    includes composed adaptation scenarios, broader ownership/learning
    regressions, documentation, all required gates, candidate replay/native
    review, and isolated performance comparison. Baseline release evaluation
    remains active in /tmp/oxide-pr9.HuHJ8c; do not overwrite its executable
    before the batch completes.
  - Fixed a returned-army lifecycle dead end that prevented reinforcement,
    preserving the existing decisive-mirror bound. Added exact home-guard
    splits, explicit mission lowering receipts, regional reconnaissance
    partial-evidence handling, shared lift/ground outcome credit, and foundation
    follow-through after worker release. The latest complete unit checkpoint
    passed 1,426 tests; earlier focused integration checkpoints passed. New
    composed two-front and outcome tests, all final gates, candidate behavior
    review, and isolated performance remain in progress. Baseline Skyhook
    evaluation is still running against the preserved main executable.
  - Profiled the 31% debug checkpoint regression and removed repeated
    route-predicate work and exhausted-component searches without changing route
    selection. Isolated follow-up samples were 8.53, 8.07, and 8.05 seconds,
    median 8.07 seconds versus baseline 7.70 seconds (4.8% overhead). Defensive
    routing tests passed; final gates, candidate replay review, and native
    sequence inspection remain pending. Action 9 stays open.
  - Passed the 1,433-test simulation unit checkpoint and 90.54% unit coverage;
    all six focused integration suites, Clippy, all-target checking, rustdoc,
    Markdown formatting, and eight canonical skill validations passed. Frozen
    state hashes stayed unchanged. The approved player-facing-only fixture
    refresh and unblessed rerun are in progress, followed by a fresh full
    workspace gate. Native final-candidate Skirmish captures cover first contact
    and post-loss regrouping. At tick 12660, repeated grounded experience
    explicitly selects a Lancer over a Sentinel; repeated failed Extractor work
    lowers that sites effective return by one band without changing its raw
    evidence. The full long-horizon matrices, combined coverage, final
    performance repetition, and remaining replay checks are not complete.
  - Replay review exposed overly broad credit for pre-contact ground withdrawals
    and concurrent armies in one assault. Added distinct abort versus
    executed-loss outcomes, live phase reporting, shared physical ground-episode
    credit, conservative mixed-force attribution, and explicit deadline
    outcomes. Explicitly cancelled foundations now report preemption rather than
    destruction. Strengthened replay reconstruction to require nonneutral
    experience in its prefix. These follow-up fixes supersede the earlier
    Lancer-selection evidence and require fresh candidate replays and gates.
    Combined coverage also exposed a legacy fog-route regression; restored that
    legacy contract and passed all 59 doctrine tests. The earlier candidate
    checkpoints remain preserved; the clean-main baseline batch is still
    running.
  - Late-game review found that exact ground drafts repeatedly selected
    exhausted rear-line veterans and failed atomically, hiding 34 healthy
    Sentinels farther from the rally. Added Executive muster exclusions to
    ground-only proposal inputs, preserving repair access and lowering
    validation; a planner-to-Executive regression exercises rear expiry and
    healthy-member admission. The prior full workspace and both coverage gates
    passed, but this replay-driven fix requires fresh gates and candidate
    review.
  - The rear-exclusion regression passed. Both latest Prime Skirmish seeds
    finished (44,071 and 13,384 ticks) with zero command rejects; the longer
    seed reconstructed to its recorded 0x61ce02a9b9822cb8 final hash. Native
    captures covered first-contact defense, post-loss return, successor
    movement, and the repaired draft at ticks 13,440–14,040. Four one-off
    NoRoute events on the winning seat occurred during late tactical returns at
    ticks 43,160–43,400; none repeated for the same unit. The long unchanged
    baseline is still in Skyhook; sampling attributes most of its decision cost
    to existing defensive-approach routing. Full matrix review and the final
    isolated performance comparison remain pending.
  - Latest source passed fmt, Clippy, all-target check, the full workspace
    suite, rustdoc, both sequential coverage gates (90.57% unit and 92.09%
    combined), and all eight canonical skill validations. The unblessed
    player-facing oracle still passes after the draft fix; frozen fixtures and
    versions remain untouched. Final isolated debug samples were 8.43, 8.17, and
    8.10 seconds (median 8.17, 6.1% above clean-main 7.70), with zero
    rejects/stalls and identical candidate hashes. Native Twin Forges review at
    ticks 8,160–8,760 showed allied-asset defense alongside a retained home
    reserve. Full 60,000-tick large-map baseline/candidate runs remain open and
    expensive; asked whether to continue the full horizon or explicitly record a
    shorter matched review limitation, continuing the full runs by default.
  - Subsequent Skyhook inspection found two terminal-accounting defects:
    unexecuted air preparation timeouts were labeled ineffective execution, and
    an intact transport returning after bounded landing failure was labeled unit
    loss. Corrected both and passed owning-planner regressions, including sealed
    live cargo. Added an outcome-journal guard that treats carried-participant
    unavailability without any actual casualty as preemption rather than
    loss-based doctrine evidence; its regression passed. These final accounting
    fixes supersede the preceding gate/performance checkpoint. Fresh
    source-consistent candidate artifacts use the attribution prefix; validation
    is running again while the clean-main baseline continues.
  - Final accounting source passed formatting, Clippy, check, the complete
    workspace suite (including the unblessed player-facing oracle and frozen
    hashes), and rustdoc. Full command-log SHA-256 comparisons matched both
    previously reviewed Skirmish runs and the first Twin Forges run against the
    latest attribution build, preserving those native-review conclusions. Final
    sequential coverage and the full baseline/candidate large-map matrix remain
    in progress; Action 9 stays open.
  - Final-source sequential coverage passed: unit line coverage 90.58%, combined
    92.09%. The three isolated attribution performance samples were 8.43, 8.16,
    and 8.16 seconds, median 8.16 versus clean-main 7.70 (+6.0%). Builds and
    coverage were stopped/completed and only this task's two evaluation
    processes were temporarily paused; both resumed afterward. All three samples
    retained state hash 0x0c16212ab0d35627 and command hash
    fnv1a64:2bd4f2ea388926fe with no rejects or stalls. Latest Skirmish replays
    also reconstructed against recorded final hashes after verifying identical
    setup and command logs. The remaining completion work is the full
    long-map/difficulty matrix and its final replay review, not code gates or
    timing.
  - Resumed the remaining full-horizon review without waiting for approval.
    Corrected two stale skill descriptions (trace schema 11 and Array sites
    around relevant owned assets), then passed all eight canonical skill
    validations and Markdown checks. A five-second candidate island profile
    attributed 2,803 of 3,941 samples to existing defensive projection,
    consistent with the baseline hotspot. Preserved native baseline
    transport-loss frames at ticks 8,800-9,040 and owner fog at 9,040 for
    candidate comparison. Island traces demonstrate separate delivery/assault
    credit, nonpunitive unexecuted preparation timeouts, and temporary Air
    doctrine penalties from two distinct attributable losses; successor behavior
    and full matrix review remain underway.
  - Verified a concrete experience-driven successor investment in the
    command-identical Skirmish replay: at tick 12,660, seat 0 ranked a 110-scrap
    Lancer preparation over an immediately affordable 90-scrap Sentinel. Raw
    value bands tied and personality slightly favored the Sentinel (145 versus
    144), but its -315 pressure experience reversed the comparison. The accepted
    alternative held 93 current scrap plus a 17-scrap forecast rather than
    issuing a train command. This proves a changed saving decision, not a
    fielded siege attack; later core needs can still preempt unpaid preparation.
    The previously watched 12,660-13,260 native sequence covers this interval.
  - Native Standard replay review exposed a remembered-building identity
    transition: at tick 7512 the forward mission recovered when the ghost
    placeholder became the live Foundry id, despite the same site remaining a
    valid objective. A focused planner regression reproduced the unwanted recall
    before the fix. Ground pressure now binds owner, kind, footprint, and any
    actually observed id separately from its movement goal; reacquisition
    preserves the commitment and deadline. Exact site matching also separates
    nearby ghost episodes and permits shared credit across remembered/current
    descriptions of the same objective. Validation and a fresh candidate matrix
    are required after this fix; the previous green gates and candidate
    artifacts are pre-fix evidence.
  - Completed all 14 clean-main matrix legs: Prime Skirmish 6983/9234 ticks,
    Twin Forges 8371/6768, Skyhook Anchorage 60000/60000 undecided, Terminal
    Basin 23119/25559, plus all six difficulty/stance legs. No baseline command
    rejections. The pre-fix six smaller candidate replays also reconstructed
    exactly; their Scrapheap worker DangerHold audit showed one evacuation and
    one subsequent Harvest per worker, not repeated bot reissuance, with all
    four workers returning to harvesting at unchanged HP. These remain
    diagnostic pre-fix evidence. Stopped the obsolete candidate evaluator after
    the site-identity regression and preserved its incomplete trace and
    completed staged replays. Compressed obsolete trace sidecars losslessly for
    disk headroom. The corrected full matrix is running with Skyhook isolated
    from the ground-map invocation; source inspection confirmed map argument
    index affects artifact names, not scenario or personality seeds.
  - Corrected-candidate native review: all seven frames at ticks 8232..8592 show
    the Prime assault leaving a separate reserve, advancing into observed
    defenders, and keeping its remembered Foundry mission through real-id
    reacquisition. Exact fog at 8400 shows enemy Foundry 1 at (34,18); trace
    retains admission 8232 and deadline 10032 with no replacement directive. The
    two corrected Standard replays ended at 13606/34859 ticks with zero rejects
    or stalls and reconstructed to 0xa8d05f7e78bd1e48 / 0x5e6e4bf9b5f87636. At
    Prime seat 0 tick 17640, two distinct costly assaults produced Pressure
    -457: equal raw proposal bands now favor saving 100 current plus 10 forecast
    for a Lancer over an affordable Sentinel despite its lower personality
    weight. A later Lancer purchase at 18948 instead wins on fresh urgency, so
    it is not attributed solely to learning. Site-identity hash drift affects
    Gatework, Terminal, Three Shifts, and Twin Forges only; Twin first diverges
    by omitting the false recall at 4956. Applied the already-approved narrow
    player-facing refresh and scheduled an unblessed oracle plus fresh gates;
    frozen hashes and versions remain untouched.
  - Refreshed site-identity validation passed the unblessed player-facing
    oracle, full workspace tests, Clippy, all-target check, and warning-clean
    rustdoc. Final coverage is running sequentially. Corrected Twin Forges
    native frames at 4980..5580 and exact owner fog show army 1 assigning six
    members to allied Extractor 5, then army 0 adding eleven as current pressure
    increases. The initial push is repelled and both seats retain their Foundry,
    Extractor, Fabricator, and Crucible. This is ground-mission allied defense,
    not evidence of a separate relief launch or universal home-garrison
    retention.
  - Watched all six corrected Standard native frames at 240..740 with owner fog
    and exact decision traces. After losing mobile contacts, army 0 retained
    Reserve while worker 2 performed the accepted approach sweep (432, fixed
    deadline 2232), inspected 79 tiles, recalled, and released only on observed
    safe return at 732. Its recorded outcome was Partial/ObservedProgress with
    confidence 500, not confirmed clearance; the worker returned at full 60 HP
    and relevant unanswered approach demand remained. This supplies native
    restraint, reconnaissance, partial-evidence, and recovery evidence without
    claiming an enemy kill or hidden trajectory.
  - Final source gates passed: full workspace tests, Clippy, all-target check,
    rustdoc, rustfmt, Markdown, and all eight canonical skills. Sequential
    coverage passed at 90.59% unit and 92.11% combined. Three isolated debug
    Terminal Basin samples measured 8.44, 8.24, 8.24 seconds (median 8.24, +7.0%
    over clean-main 7.70), all with zero rejects/stalls and identical state
    0x0c16212ab0d35627 and command fnv1a64:2bd4f2ea388926fe. Verified evaluator
    and replay processes were paused only for timing and automatically resumed;
    no concurrent builds or coverage. The short benchmark does not establish
    late crowded-island scalability, whose existing defensive-route hotspot
    remains separately profiled. Also verified positive Pressure +330 at Prime
    seat 1 tick 9468 selects and trains a Sentinel over a higher-personality
    Lancer alternative with tied raw bands, explicitly on experience.
  - Additional corrected Skyhook 12000-tick review replay reconstructed to
    0x65a89d6c46d91df4 with zero rejects and six isolated stall reports (audit
    pending). Watched all six Array frames at 8200..8800: accepted worker 2
    built Array 127 at (37,23), complete at 250 HP in owner fog by 8800, serving
    approach demand for forward asset 107. Watched all six delivery frames at
    9400..10000: seat 3 carrier 219 delivered riders 18/51/91 and returned with
    180 HP, but defenders defeated the assault. Owner fog, lift phase transition
    at 9792, and reports distinguish Partial delivery at 11376 from Ineffective
    assault at 11388 with the same Lift 6720 credit and 270 actual landed
    losses. This short replay is additional inspection evidence, not a
    substitute for the continuing full matrix; verify its exact command prefix
    against the full replay when emitted.
  - Early Skyhook stall audit found a separate pre-existing placement defect: at
    11592 worker 0 was still building Reclaimer 206 from (31,20), but a new
    Reclaimer commitment to worker 2 targeted that same work tile. The later
    foundation displaced the first builder, leaving its original scaffold to
    decay. The terrain regression failed before the fix. Player-facing work
    observation now indexes exact active-builder tiles once per decision and
    fresh blocking foundations exclude them; nonblocking charges, ordinary
    movement, completion/movement release, and legacy placement remain
    unchanged. The 17 owning terrain tests passed. Stopped the now-pre-fix full
    evaluators and preserved artifacts; gates, fixture inspection, timing, and a
    corrected full matrix must be refreshed again. Launch the new matrix as
    independent exact-seed legs so long maps do not serially delay other seeds,
    preserving the original consecutive-seat personality arithmetic.
  - Refreshed all ten Skirmish/Twin Forges legs after active-builder protection,
    with zero command rejects. Both Standard replays and the reviewed first
    Prime Skirmish/Twin command logs match the prior native-reviewed recordings
    exactly. Four independent large-map legs continue at the full 60000-tick
    ceiling with matched scenario and consecutive-seat personality seeds.
    Focused utility (480) and composed adaptation/recon suites pass; normal
    gates and sequential coverage are still running. Hash inspection isolates
    the new drift to the two Skyhook behavior cases (four rows); the approved
    same-version refresh and unblessed oracle are running, with frozen/state
    fixtures untouched.
  - The refreshed Skyhook 12500-tick inspection replay reconstructs to
    0x7605570afff746ee, with zero rejects. Watched all native construction
    frames 2100..2400: Reclaimer 27 completes and worker 15 returns to
    harvesting. Watched all Array frames 6100..6700: worker 22 completes Array
    83 at (102,60), with owner-visible anonymous contacts at (97,73) and
    (106,75), not composition. Watched all delivery frames 9300..9900: seat 0
    carrier 211 delivers four riders, then returns with 112 HP after defenders
    defeat them. Lift 6576 reports Partial delivery at 11532 and its bounded
    assault watch reports Ineffective at 11544 under the same credit, with 360
    actual landed losses and no duplicate delivery doctrine credit. Earlier
    pre-worktile Skyhook visual sequences are historical, because the refreshed
    command prefix first diverges at 2196; the new captures replace that
    evidence.
  - Refreshed source gates now pass after the builder fix: full workspace tests,
    focused utility and composed suites, Clippy, all-target check, and the
    approved Skyhook-only behavior refresh followed by an unblessed oracle.
    Sequential coverage passes at 90.60% unit and 92.11% combined. All ten
    emitted ground/profile replays reconstruct against their recorded final
    hashes. Native Veteran frames 33200..33800 show another contested Extractor
    attempt failing under pressure; at 33696 its contextual score -589 lowers
    Material to Incremental while broader Expansion doctrine stays neutral.
    Acceptance at that tick is an unpaid forecast-backed preparation, not
    immediate command credit or a proven successful expansion. This is
    recovery/penalty evidence, with repeated contested investment retained for
    calibration review, not a claim of fun or optimal play. Final large-map
    results and refreshed isolated timing remain open; timing must wait for
    unrelated oxide-ui-polish validation to leave a clean window.
  - Final isolated debug samples after all source changes: 8.63, 8.54, 8.53
    seconds, median 8.54 (+10.9% versus clean-main 7.70). All three preserve
    state 0x0c16212ab0d35627 and command fnv1a64:2bd4f2ea388926fe with zero
    rejects/stalls. No concurrent builds/coverage ran; only verified task
    evaluators and viewer were temporarily paused and automatically resumed.
    Investigated the threshold overrun with an isolated debug sample: routing
    and allocation validation dominate, with no obvious remaining small hotspot
    in the new builder protection. A separate late-Skyhook sample attributes
    2913/3577 samples to existing defensive projection (2790 in shortest-path
    work). Earlier cheap route optimizations reduced the initial 31% regression;
    discussed retaining the remaining roughly 11% cost and existing late-island
    routing scalability as explicit limitations instead of adding a broad
    routing redesign to this slice.
  - Audited the final Skyhook short replay’s remaining seat-6 stalls: all five
    flagged workers resumed harvesting or construction by 12500, and surviving
    flagged fighters received later movement/deployment orders. These early
    reports are not permanent frozen-order evidence; the full horizon remains
    pending. A fixed complete Terminal Basin trace prefix through about 44900
    retains all six outcome categories, with every seat at or below 64 episodes
    and 92 contextual entries. Broad doctrine eligibility remains limited to
    attributable episodes. Artifacts: worktile-sky-seat6-stall-inspect.json and
    worktile-terminal-1-progress-outcomes.json in the existing review directory.
  - The full pre-site-classification-fix Terminal Basin seed 20260813 decided
    for team 0 at 48330, hash 0xf21d0195dc960955, with zero rejects and 12
    stalls (baseline 51); reconstruction passed. Worker 215 continued responding
    through late evacuation. Final armies remained mostly tier one and late
    fortification accumulated heavily, so duration and roster quality remain
    calibration limitations. Exact fog review found a narrow outcome bug: worker
    28 lost an Extractor race at world (84,54) to allied foundation 28 by tick
    3564, but the journal called it BlockedRoute. Added a failing regression and
    corrected current full-footprint occupation to Invalidated/SiteOccupied with
    no route exclusion or experience penalty. Current own/allied/enemy
    foundations and finished buildings are covered; ghosts, dead buildings, and
    nonoverlapping sites cannot establish occupation. Six work-experience unit
    tests pass after the fix. Gates and affected behavior evidence are being
    refreshed; worktile full runs remain explicitly pre-fix artifacts and Action
    9 stays open.
  - The corrected 4200-tick Terminal Basin replay records
    Invalidated/SiteOccupied at 3564 with no contextual entry for the occupied
    frame. Its setup and complete command prefix match the pre-fix replay, and
    reconstruction passes at 0x00960be876350482. Watched all seven native frames
    3300..4020: worker 28 leaves the allied-claimed frame, founds the alternate
    (78,54), and completes it by 3972. All canonical skills validate. The
    refreshed player-facing oracle passes unblessed without further fixture
    changes. Stopped only the three verified superseded worktile evaluations,
    preserving their partial trace files and every completed replay; final
    occupation-* replacements retain the full 60000-tick ceiling.
  - Final source gates pass after site-occupation classification: full workspace
    tests (including the unblessed player-facing oracle), Clippy, all-target
    check, warning-free rustdoc, formatting, and sequential coverage at 90.60%
    unit / 92.11% combined. All ten occupation-* ground/profile matches have
    identical setups and complete command logs to their worktile predecessors;
    every emitted replay reconstructs to its recorded final hash. Final isolated
    debug timings are 8.45, 8.15, 8.08 seconds, median 8.15 (+5.8% versus
    clean-main 7.70), identical state/command hashes and zero rejects/stalls.
    Verified there were no concurrent builds or coverage; only the four task
    evaluations and task viewer were paused, automatically resumed, and checked
    afterward. Retain the earlier 8.54 median as timing variability rather than
    attributing the difference to this narrow fix. Existing late-map defensive
    routing remains a separate scalability limitation. Four full final-source
    large-map legs remain running.
  - Both final-source Terminal Basin legs reconstruct: seed 20260813 finishes
    team 0 at 48330 / 0xf21d0195dc960955 with zero rejects and 12 stalls; its
    setup and entire command log match the pre-fix worktile run, preserving the
    detailed stall/recovery review. Seed 20260814 finishes team 0 at 41168 /
    0x17ad1734524760b4 with 33 stalls and one NotANode rejection. Traced that
    rejection exactly through native playback at 34152: seat 6 currently sees
    one wreck salvage at (106,24), but earlier in the same command list seat 5
    builds an Extractor at (105,23), whose ordinary foundation commit clears the
    wreck under its complete footprint. Seat 6 cannot observe that future allied
    order while deciding. Worker 973 is subsequently retasked; this is a
    cross-seat command race, not hidden-state access or a
    current-evidence/funding defect. Preserved exact fog snapshots and native
    step events under occupation-terminal-2-harvest-reject.json and
    occupation-reject-34152.json. Two final Skyhook legs remain open.
  - Publication is now authorized after final review: create a signed
    conventional commit, push this branch normally, and open a pull request with
    an empty description. Keep the implementation frozen while the two final
    full-horizon Skyhook legs finish; no additional evaluation matrix or broad
    routing redesign is part of closure.
  - Both final Skyhook legs completed the unchanged 60000-tick ceiling and
    reconstructed exactly: seed 20260821 hash 0x350cb0fb5fc31d78, seed 20260822
    hash 0xe3ce20f0f3276ae8. Both remain undecided, as did their baselines, and
    both have zero rejected commands. Aggregate stall events are 646 and 435
    versus baseline 886 and 625; the first leg still has an individual aircraft
    with 35 NoOpenGround events, so aggregate improvement is not a claim that
    landing congestion is solved. All 14 final match replays now reconstruct.
    The final first-Skyhook setup and all 2964 commands before tick 12500 match
    the previously watched construction, Array, transport-delivery, and
    outcome-attribution sequences exactly.
  - Final late-Skyhook review is complete. Native frames at 54480, 54600, and
    54720 show the seat-5 air cohort recalling around its crowded island after
    the exact 54492 transition from suppression to recovery. The next decision
    records Ineffective/RequiredUnitLost, not success, while two independent
    recon observers retain fixed deadlines. Exact fog snapshots through 60000
    show the worst-stalling Moths changing positions and orders, including
    completed landings; seed 20260822 Moth 2765 receives 24 later orders and is
    landed at the final snapshot. This rules out a single permanently frozen
    order for those examples, not landing congestion or wasteful repeated
    assaults. Artifacts are occupation-sky-*-late-recovery.json,
    occupation-sky-1-post-recovery-outcomes.json, and native-final-sky-recovery/
    in the preserved review directory.
  - Action 9 implementation, required gates, full baseline/candidate matrix,
    exact replay reconstruction, representative native review, performance
    comparison, and fixture decisions are complete. Final source remains at
    version 0.16.0 with frozen Overseer/state fixtures unchanged; the necessary
    player-facing behavior fixture refresh passed the unblessed oracle. Final
    debug median is 8.15 seconds (+5.8%); earlier 8.54-second variability and
    late-map routing cost remain recorded. Final personality/difficulty
    calibration, human fun, crowding, repeated defended landings, and broad
    routing scalability are not claimed solved and remain Action 10 work. All
    eight canonical skills and final formatting/whitespace checks passed.
- [ ] 10. Prove personality signatures, difficulty competence, and whole-match
      quality before promotion.
  - Verify identical legal repertoires but distinct allocation and within-domain
    behavior, structurally monotone fair difficulty, paired seats and factions,
    Prime versus frozen Overseer, long rich-map play, deterministic replays, and
    human judgment; finish by bumping 0.17.0 and blessing final hashes.
  - Boundary: finish migration cleanup and calibration without changing game
    balance; any proposed unit, building, or economy-stat adjustment remains a
    separate human decision.
  - Close only after focused and composed tests, repeated deterministic hashes,
    distinct same-difficulty personality signatures, fair difficulty competence,
    paired map and seat matrices, duplicate-seed detection,
    Prime-versus-Overseer evidence, watched full replays, human play, final
    documentation, final hashes, and the approved 0.17.0 version bump.
  - One paired control cell had Standard defeat Prime in all four legs across
    Cinder Steppe and Terrace Ledger; retain this as difficulty-calibration
    evidence rather than tuning it inside the force-package slice.
  - Finish the strangler exit rather than adding another coordination layer:
    retire remaining test-only player-facing facades as their domains migrate,
    replace the reduced-observation/raw-budget adapter, consolidate semantically
    identical producer schedulers and proposal-band/trace mirrors, and split
    oversized inline test modules. Preserve behavioral tripwires before deleting
    legacy tests; do not invent a generic operation framework or perform
    mechanical container rewrites without measured value.
  - Retain final large-map evidence for outcome learning and calibration:
    Skyhook scouts recover and transports launch, but repeated defended landings
    fail, idle paid observers accumulate after questions finish, and large
    defensive rosters persist. Terminal Basin still has late and uneven
    technology and mostly tier-one armies. Late Skyhook samples concentrate in
    existing defensive approach pathfinding. Revisit these measured outcomes and
    routing cost in Actions 9-10; automated legality and coverage do not
    establish human fun or final difficulty quality.
- [x] Address reconnaissance and support review findings: retain raid
      membership, objective, and paid production; correct Bay coverage and
      protector recall; validate and push a follow-up commit.
  - Retained the accepted raid pair, target, deadline, and exact paid queue
    occurrences across allocation and residual advancement. Queue completion
    binds observed births before connected-operation inventory; ambiguous
    missing births release the preparation conservatively without cancelling
    paid work. Bay coverage now uses both complete rectangles. Busy protectors
    recall with ordinary movement, retain ownership, and resume protection
    inside the service radius; a blocked return releases the assignment.
  - Reviewed exact raid and protector sequences in the native shell. A staged
    pair trained one missing Scuttler and dispatched the original member plus
    its birth at tick 96; its replay reconstructs to 0xc8672d5e57968771. Twin
    Forges unit 74 changed from an active chase to Move at tick 5748 and resumed
    AttackMove at 5796 without repeated recall orders. Skirmish and Twin Forges
    replay finals reconstruct exactly; all seats had zero rejected commands.
    Artifacts remain outside the repository at /tmp/oxide-pr51-followup.7GLACh;
    the staging scenario was removed and its embedded replay retained.
  - Focused raid (29), allocation (137), support (16), and recon_support (9)
    tests passed, including coordinator ownership against connected operations,
    mixed paid/live pairs, cancellation, missing births, frozen objectives,
    rectangle symmetry, delayed overlapping service, and busy protector
    recovery. Inspected and refreshed four player-facing hash rows under the
    existing same-version approval: Twin Forges state/commands and Terminal
    Basin/Three Shifts commands. The first unblessed rerun passed; version
    0.16.0 and frozen fixture files remain unchanged.
  - Final validation passed: cargo fmt --all --check; cargo clippy --workspace
    --all-targets --locked -- -D warnings; cargo check --workspace --all-targets
    --locked; cargo test --workspace --locked; warning-free workspace rustdoc;
    and repository-wide Prettier. Sequential cargo cov-unit and cargo
    cov-combined passed at 90.50% and 91.98% line coverage. The final full
    workspace rerun included the unblessed player-facing oracle and unchanged
    frozen Overseer/state compatibility gates. No skill guidance changed. Native
    review established the selected raid/recall transitions, not human fun or
    final calibration.
- [x] Correct battlefield-adaptation review findings: flight-domain defensive
      credit, age-preserving contextual decay, deadline-safe Array selection,
      and frozen failed-objective identity. Preserve conservative unit-raid
      outcomes; validate without publishing unless separately authorized.
  - Reproduced and corrected all four review boundaries: parked flight-capable
    providers, staggered contextual decay with exact shared-credit replacement,
    completion-time Array selection with lazy route bounds, and failed-objective
    identity through fog. Unit-target raid disappearance remains inconclusive.
    Focused regressions pass; full gates are running. No publication is
    authorized for this follow-up.
  - Full workspace tests, Clippy, type checking, rustdoc, formatting, and all
    eight canonical skill validators passed. The unblessed player-facing oracle
    and frozen Overseer/state checks passed without fixture or version changes.
    Sequential coverage remains pending; validation logs are in
    /tmp/oxide-pr52-review.wydw2b.
  - Completed all four fixes with pre-fix reproductions and passing focused
    regressions, including independent decay ages, exact shared-credit
    replacement, bounded saturation recovery, and the one-quote neutral Array
    fast path. Sequential coverage passed at 90.64% unit and 92.14% combined.
    All required gates are green; no fixture refresh, version change, or
    repeated match matrix was needed. Updated architecture and scripted-bot
    regression guidance. Changes remain uncommitted and unpushed.
  - The user subsequently authorized committing and pushing this validated
    follow-up to the existing branch. Publish as a new signed conventional
    commit without amending or force-pushing.

## Open Questions
