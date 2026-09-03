---
created: 2026-09-02T05:47:32
updated: 2026-09-03T05:45:12
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
  - Added an exact deterministic producer scheduler with no policy count cutoff;
    independent brute-force oracles matched 215,000 generated queue,
    eligibility, release-order, and lane-allocation cases.
  - Aligned reconnaissance, target-cluster liveness, artillery firing stands,
    group spreading, producer egress, and attack routes with public terrain and
    the authoritative command geometry; an infeasible preferred target now
    yields to the next viable current objective.
  - Evaluated 14 persisted ordinary- and rich-map matches with 34,338 trace
    rows. Connected packages ranged from the shared three-unit minimum to 15
    units, scaled suppression through Bombards and Avalanche against observed
    AA, substituted Buzzards and Condors at higher opportunity, and reproduced
    the same command and terminal hashes under an identical seed.
  - Closed adversarial review gaps: remembered frozen targets remain inside AA clearance until their full footprints are re-scouted, and admission now proves a distinct reachable firing stand for every suppression provider rather than extrapolating from one artillery piece.
  - Revalidated active hidden-target preparation against the latest spendable bank and surviving completed-income forecast, so a destroyed income source releases an infeasible package instead of holding resources until timeout.
- [ ] 4. Admit compatible investments through a deterministic cross-domain
      allocator.
  - Boundary: compare structured proposals and claims while leaving each domain
    responsible for its own target, placement, composition, phases, and micro;
    adapt unmigrated work through explicit legacy obligations.
  - Cover hard survival constraints, already-paid work, compatible concurrency,
    mutually exclusive scrap, builders, factories and units, stable tie-breaks,
    personality weighting without zeroing a domain, and traceable approval or
    rejection.
  - First slice: compare exactly two fresh proposals, one currently safe and command-legal Foundry expansion and one connected-operation minimum. Treat saved Foundry plans, active operations, paid work, opening recovery, bootstrap work, emergency survival, and the shallow Sentinel as obligations; leave other fresh unmigrated channels on residual resources.
  - Select among the four possible proposal subsets with no search cutoff. Apply current and forecast scrap, exact builders, sites, units, and shared producer timing as one atomic claim bundle so higher-order conflicts cannot double-spend a forecast or factory lane.
  - Store exact proposal payloads and commit only the accepted site, builder, target, minimum package, and production evidence without rerunning domain ranking. Scale connected marginal capability only from resources left after the accepted minimum and expansion.
  - Verify an independent four-mask oracle, atomic rollback, current and forecast conflicts, compatible concurrency, producer hyperedges, persistent obligations, deterministic lowering and tie-breaks, a real personality near-tie, and a composed state-accepted expansion-plus-offense case.
  - Implemented the pure two-domain allocator with exhaustive four-mask selection, named semantic bands, exact current and deadline-scoped forecast capital, actor and site ownership, producer FIFO scheduling, atomic rollback, deterministic ties, and additions-only connected scaling.
  - Integration is not complete until accepted future jobs retain both money and exact producer timing without fabricating queued inventory; due commands spend exactly once; saved Foundries dispatch after accrual; active legacy planners roll back atomically on failure; and current-reserve clamps use the observation tick rather than an owner's acceptance tick.
  - Keep Action 4 open through focused adversarial regressions, composed State acceptance, deterministic behavior hashes, representative headless evaluation, maintainability extraction, and full repository gates.
  - Rejected a producer abstraction that conflated command enqueue and payment with production start; allocation must retain decision-tick admission, FIFO start, slot reopening, strict readiness, and post-income spendability.
- [ ] 5. Replace default unit sinks with tech-aware standing-army and
      ordinary-production demand.
  - Derive standing force from known counters, strategic plans, technology,
    capacity, and personality; remove Sentinel and Lancer as automatic fallbacks
    while retaining tier-one units as useful screens and counters.
  - Boundary: migrate repeatable ordinary combat production and specialist
    demand without rewriting persistent operation execution or changing unit
    statistics.
  - Cover live, queued, reserved, and same-think capability accounting; current
    and remembered enemy counters; production bottlenecks; useful higher-tier
    substitution; home-defense floors; reinforcement; and avoidance of idle
    factories, hoarding, or partial strategic cohorts.
- [ ] 6. Migrate defensive spending to opportunity-scaled investment while
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
- [ ] 7. Migrate economy, technology, and production capacity away from
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
- [ ] 8. Migrate reconnaissance and support to information value and concrete
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
- [ ] 9. Feed bounded deterministic outcome evidence back into future strategic
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

## Open Questions
