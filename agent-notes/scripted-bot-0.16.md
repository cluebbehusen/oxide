---
created: 2026-08-24T19:37:13
updated: 2026-08-28T17:52:29
---

# Oxide 0.16 Rules-Based Opponent

## Goal

Ship a fair, deterministic rules-based opponent whose seeded identities,
strategic plans, and four difficulty levels produce credible and enjoyable
complete matches.

## Decisions

- Preserve the level playing field: every bot reads only fog-honest knowledge
  and emits ordinary player commands under shared costs, prerequisites, queues,
  movement, combat, and economy.
- Keep Overseer frozen as the stable QA anchor; player-facing strategy evolves
  through the separately constructed scripted opponent.
- Store difficulty, stance, and one per-seat personality seed in the scenario
  and replay. Resolve the seed deterministically through stable independent
  streams rather than serializing internal tuning details.
- Expose Turtle, Balanced, and Aggressive stances. Stance governs tempo,
  commitment, and risk, while seeded traits vary only within a coherent stance
  envelope.
- Resolve six correlated traits: air, siege, support, fortification, greed, and
  guile. Deal each profile a primary specialty and secondary wrinkle instead of
  independently permitting every trait to become extreme.
- Use four difficulty levels: Scrapheap, Standard, Veteran, and Prime. Build
  coherent Prime behavior first, then derive lower levels by degrading reaction,
  attention, estimates, coordination, and discretionary timing rather than
  granting advantages or disabling strategy.
- New Match generates new personality seeds; Restart and Rematch preserve the
  exact opponent configuration so the same setup remains reproducible.
- Make strategy legible through persistent phased playbooks with explicit
  objectives, composition requirements, reservations, abort conditions,
  cooldowns, and evidence-based transitions.
- Use a combined air breakthrough as the first strategic vertical slice:
  reconnoiter, assemble, suppress observed anti-air with artillery, verify the
  opening under current sight, strike with reserved bombers, then recover or
  pivot.
- Treat guile as distinct from aggression. High-guile bots seek bounded
  hit-and-run attacks on economy, expansions, construction, and isolated
  infrastructure, then retreat under explicit loss and time budgets.
- Automated evaluation may reject stalls, command storms, unused forces, ignored
  observed counters, and failure to convert advantage. It cannot certify fun;
  complete human play and replay review remain the promotion gate.
- Controller-only difficulty and personality calibration is required 0.16 work.
  Connor sign-off is required before changing simulation-side unit, building,
  economy, or map balance, not before tuning fair bot policy.
- Replace global Extractor escalation with a fixed base yield and a visible,
  non-stacking benefit from a nearby completed same-owner Foundry.
- Keep the nearby derelict Extractor as an opening choice rather than starting
  it completed; defer the haul-from-Extractor model.
- Treat late-game air suppression and mass transport as independent strategic
  levers that may also synchronize: bomber or screen forces can win alone,
  transport waves can win alone, and a wealthy bot should combine them when the
  opportunity warrants it.
- Treat wealthy island stalemates as sufficient reason to consider strategic
  bombers even when air is not the bot's seeded specialty; personality controls
  emphasis and timing, not whether the only credible attack path exists.
- Froze the R23 gameplay candidate after final paired calibration; remaining
  controller-policy balance questions stay documented for Connor rather than
  being tuned through unit, building, economy, or map stats without sign-off.

## Findings

- The existing Observation, orientation, Intent, and Executive layers are strong
  reusable mechanics; the fixed sequential policy and shallow memory are the
  strategic bottlenecks.
- Observation exposes explored terrain but not the current visibility mask, so
  the bot cannot honestly distinguish recently verified weakness from stale
  negative evidence.
- Current air behavior throws every idle ground-attack flyer at one target or
  cancels when anti-air is known; it does not reserve strike groups, suppress
  defenses, verify results, or recover survivors.
- Player-facing baseline matches all concluded, but Severance produced 2,009
  pre-decision route stalls and Compass Grand produced 294 rejected build sites.
  Similar openings and compositions confirm weak strategic identity.
- Existing all-map liveness, sweep, pace, and hash instruments intentionally
  exercise Overseer. The changing player-facing bot needs a separate candidate
  evaluation surface.
- Paired complete-match evaluation exposed two behavioral faults hidden by
  aggregate progress: blind expansion placement could storm rejected sites, and
  permanently retired wounded units could make a large army mostly unusable.

## Actions

- [x] Preserve the requirements and reproducible baseline evidence.
  - Recorded the fairness, configuration, personality, difficulty, playbook,
    evaluation, and promotion requirements before implementation.
  - Copied the four complete baseline replays, summaries, final states, and logs
    into the gitignored replays/bot-0.16-baseline review directory.
  - Established signed checkpoints for the 0.16 version, renewable economy,
    mechanical fairness, strategic controller and setup UI, evaluation harness,
    deterministic fixtures, coverage floors, and maintained documentation before
    further work.
- [x] Add configuration provenance, deterministic profile resolution, and a
      player-bot evaluation runner.
  - Added strict scenario/replay provenance for four difficulties, three
    stances, and one exact per-seat personality seed while preserving the
    compact default controller wire.
  - Resolved six stance-bounded traits with a fixed preference budget, truthful
    ranked specialties, independent streams, full-width seed goldens, and guile
    kept orthogonal to stance.
  - Added a player-facing JSONL evaluation runner with exact profiles, early
    result stops, paired seat swaps, anomaly evidence, and optional create-only
    replay/JSONL publication. Reported publication failures roll the batch back;
    abrupt termination across arbitrary final paths is explicitly outside that
    guarantee.
- [x] Add current visibility, timestamped intelligence, persistent plans, and
      compatible candidate scheduling.
  - Added the canonical current-visibility mask, timestamped unit and building
    memory, current-versus-remembered anti-air evidence, and explicit confidence
    decay without turning fog disappearance into proof.
  - Added persistent operation state, exact-unit reservations, player-facing
    difficulty tuning, and prelude-aware multi-factory scheduling while
    preserving the profile-free Overseer path.
- [x] Implement and behaviorally verify the combined air breakthrough vertical
      slice.
  - The real Brain-to-State vertical now reserves one scout, two artillery
    pieces, and two bombers at commitment, destroys currently visible flak
    first, waits for refreshed corridor evidence, then releases the exact bomber
    wing; the scout survives outside known anti-air and every command is legal.
  - Closed a gap found by the behavioral harness: recon initially left future
    operation members available to the ordinary army draft. Operations now claim
    existing candidates immediately and persist their exact membership while
    filling only missing roles.
- [x] Broaden strategy across siege, support, fortification, raiding,
      multi-factory production, and team roles.
  - Player-facing construction now defers claims outside current sight, while
    Overseer remains unchanged; the same paired seeds fell from as many as 66
    rejected sites to zero.
  - Added repair-point construction, mobile Tender assignments, bounded guile
    raids, and release of genuinely repaired rear-line units without changing
    Overseer's permanent rear ledger.
  - Added current-sight allied-base relief with stable exact groups, a
    home-defense floor, reaction and commitment budgets, withdrawal reasons,
    cooldowns, and Brain-level proof that emergency reservations coexist with
    the ordinary army draft.
- [ ] Make the six correlated seeded traits produce recognizable, bounded
      strategic identities.
  - Final R7 quartiles make air, support, fortification, and greed directionally
    legible across both physical seats; stance signatures are stronger still.
  - Siege currently reads through Bastion construction but not mobile siege
    production, while guile does not yet create a reliable raid or harassment
    signature. Keep this task open.
  - All 96 final personality legs decided: 22 paired cells were won twice by one
    personality and 26 followed the same physical side after swapping profiles.
    Most matches were short and decisive rather than consistently close.
  - Turtle and Aggressive are much clearer than the hidden traits: Aggressive
    produces more air, siege, raids, and direct attacks, while Turtle produces
    more defenses and support. Eleven of 12 stance pairs still followed the
    physical side.
  - Audited 20,000 live resolved profiles per stance. Siege targets only ever
    resolve to two or three units, never one or four; on Balanced, 87.5 percent
    choose two. Support targets only ever resolve to one or two, never three; on
    Balanced, 88.1 percent choose one. One quarter of primary Siege profiles and
    one fifth of primary Support profiles therefore share the ordinary baseline
    target instead of gaining a roster signature.
  - Traced the weak signatures to policy shape rather than missing trait
    provenance. Every profile owns a one-unit Bombard, Avalanche, and Tender
    floor; support is otherwise reactive or team-only in duel evaluations, and
    every profile receives a Repair Bay by tick 6,000. Guile disables the raid
    planner entirely below 55, while only about 4.5 percent of resolved profiles
    reach the three-raider threshold, so it acts as a sparse capability switch
    rather than a broad propensity.
  - Implemented all six bounded identity signatures with real-seed
    counterfactuals: Guile uses an exact two-raider muster and preservation
    cadence; Siege substitutes artillery within the shared force envelope;
    Support changes Repair Bay, Tender, and repair-budget behavior; Air,
    Fortification, and Greed retain their existing tested signatures.
  - Reran 144 same-difficulty personality legs. Physical-seat wins were 49/50,
    but Standard, Veteran, and Prime each produced 12 profile sweeps and no
    ground-map splits. Profiles 1,616,300, 1,616,303, and 1,616,305 dominated
    their paired opponents; replay commands show small trait differences
    crossing into large early Harvester, turret, and Sentinel investment
    differences. Identity is legible but not yet bounded closely enough.
  - Removed the early-investment cliff that made small trait differences decide
    whole openings: every adaptive profile now erects one pre-contact perimeter
    turret, the remaining fortification cap opens only after an actual raid, and
    player-facing Flak requires confirmed air rather than anonymous blips. Exact
    counterfactuals stopped following the personality, exposing a separate
    physical-seat bias instead.
  - Exact adjacent personality seeds isolated two controller cliffs. Expansion
    reserve could hoard 370 scrap for an unsafe or unplaceable third Foundry,
    while an unconditional second Tender entered the first symmetric fight and
    won seven of eight paired legs. Keep the traits and targets, but make
    expansion reserve share construction feasibility and make extra support
    demand-driven.
  - Final Prime personality calibration ran 32 paired legs across four scenarios
    and four seed pairs. Every match decided with zero command rejections;
    physical seats split 15/17, seven paired cells were personality sweeps, and
    seed 1,616,300 beat seed 1,616,301 in all eight legs.
  - Bounded replay forensics found a concrete controller-policy cliff, not a
    liveness bug: the Guile/Support profile kept a flexible line, while the
    Guile/Siege profile crossed the combined-operation gate, reserved extra
    Bombards and a scout, and usually lost the first meaningful battle on
    connected maps. Keep personality calibration open until that specialization
    tradeoff is changed with Connor's sign-off and reviewed through human play.
- [ ] Derive and calibrate the four fair difficulty levels from Prime.
  - The exact-continuation adjacent ladder finished 30 wins for the higher rung
    and 42 for the lower rung. Only three of 36 paired cells were higher-rung
    sweeps, while nine were lower-rung sweeps.
  - Twenty-four of 36 adjacent paired cells followed the same physical seat and
    faction after the configurations swapped. Difficulty needs calibration, and
    the next experiment must separate policy quality from the faction, seat, and
    map bundle.
  - The first human Prime promotion match exposed a shared strategic failure
    before difficulty could matter: the bot held only three Sentinels while
    reserving for infrastructure, founded inside remembered Bastion range, did
    not defend its own expansion, and reactively replaced workers into the same
    kill zone. Reclaimer income amplified the failure: the human earned 7,242
    passive scrap from Reclaimers versus Prime's 1,168, while the profile capped
    Prime at two. Treat controller defects separately from the economy question
    Connor wants to discuss.
  - The first post-transport Prime-versus-Standard cohort exposed controller
    inversions rather than balance evidence: non-nested decision ticks,
    independently re-rolled estimate error, lower-rung operation patience,
    primary-lift preemption by optional raids, and rapid player-facing
    focus/retreat oscillation under Prime's cadence. Nested the cadence and
    estimate contracts, made operation deadlines neutral, prioritized the lift,
    and made routed armies hold a contested fallback instead of re-entering the
    loop.
  - Fresh paired Scattering calibration produced five Prime wins, two Standard
    wins, and one 60,000-tick cutoff with zero rejected commands. The cutoff was
    an active fortified-island assault, not a deadlock: exact continuation ended
    in a Prime win at tick 64,107.
  - Replay forensics separated the two remaining Prime losses from stat balance:
    one lift attacked the home island while its air screen struck the expansion,
    and one early overseas payload left too little ground-capable defense for a
    counterdrop. Kept controller calibration open and queued narrow coordination
    and reserve fixes.
  - Audited the live ladder after the lift and air-coordination work.
    Independently seeded signed own and enemy strength errors could make a
    lower-rung push read strictly more favorable than Prime: personality seed 71
    made Scrapheap and Standard open an early five-versus-three gate that
    Veteran and Prime correctly held.
  - Replaced the signed lottery with deterministic conservative uncertainty.
    Lower rungs now only underestimate their own force and overestimate visible
    opposition; exhaustive seed and strength-ratio tests prove that a lower-rung
    commitment implies every higher rung also commits. Current sightings now
    outrank higher-value remembered buildings, so longer Prime memory cannot
    displace a live target with a fog ghost.
  - Kept calibration open. Remaining controller-only inversion risks are
    attention slots gating only raids rather than all new strategic concerns,
    reaction delay doubling as useful threat debounce, larger relief groups
    weakening home defense, and faster cadence freezing air or lift force
    targets from an earlier and smaller roster snapshot.
  - Traced the permanent Cinder timeout to Prime enclosing its own ground
    producers with individually legal buildings. Added a cumulative same-think
    placement invariant for every ground producer, backed by deterministic route
    certificates over the exact fog-known blocking layout.
  - Rejected two correct but unusably slow egress implementations. The apparent
    guard regression was actually an older expansion cliff: after a remote
    Extractor completed, support-Foundry search pathfound every candidate from
    every worker before checking site legality. Reordered the equivalent
    canonical search so geometry and egress reject cheap candidates before one
    route check. Exact Cinder seed 7,301 improved from failing to finish 2,000
    ticks after 18 seconds to 0.86 seconds; 20,000 ticks completes in 26.46
    seconds with zero rejections.
  - Audited the remaining late-match controller curve after the egress fix. The
    largest active cost is an eager full-map harvest-danger projection that
    repeatedly scans all fog-honest hostile memory even when no worker needs an
    order; repeated army-to-roster joins are the next largest growth shape.
    Treat both as 0.16 calibration blockers, preserve exact decisions with
    differential tests, and leave broader projection/allocation cleanup for
    later profiling.
  - Made strategic admission difficulty-neutral on one absolute 24-tick
    boundary. Air, lift, raid, and relief freeze the same evidence and exact
    roster before rung-specific reaction or hesitation, while mandatory
    construction is budgeted before residual difficulty-dependent production.
  - Ran the controlled 108-row adjacent ladder across rotational Open, Choke,
    and Island maps for both factions. Higher rungs won 42 of 95 decisions
    versus 53 for lower rungs; 13 rows hit 60,000 ticks and no command was
    rejected. The ladder is not calibrated.
  - Replay forensics separated two causes. Twenty-one of 24 split pairs followed
    the same physical seat, with divergence beginning after mirrored commands
    entered movement. Separately, Scrapheap swept Standard on Island because
    higher-rung optional Repair Bay and anti-air spending delayed the mandatory
    transport by 312 to 612 ticks.
  - Extended every limit-affected paired cell to 240,000 ticks. Seventeen of 36
    rows still failed to decide, all on Island. The exact Veteran versus Prime
    deadlock endlessly trained and lost one Kestrel about every 360 ticks while
    idle Skyhooks and ground beachheads never resumed a strategic operation.
  - Fixed the island Kestrel conveyor at both controller layers: a dispatched
    strategic scout now recovers without replacement, and the separate
    solo-scout channel releases production after one loss until actionable
    current enemy sight first goes dark and later returns. Persistent sight,
    remembered ghosts, and opposing dedicated-scout cross-sight cannot rearm it.
    The exact paired regression fell from 88 Kestrel trains per leg to three,
    with none after tick 6,000.
  - Reran the final 108-row adjacent ladder after the mechanical symmetry and
    scout fixes. Higher rungs won 47 of 74 decisions versus 27 for lower rungs,
    with zero rejected commands and an almost even 35/39 physical-seat split.
    Standard strongly beat Scrapheap and Prime modestly beat Veteran, but
    Standard still beat Veteran 14/11. Thirty-four rows hit 60,000 ticks, mostly
    on Island, so calibration and long-run liveness remain open.
  - Extended every remaining R2 adjacent-difficulty cutoff to 240,000 ticks.
    Eleven of 34 cutoff legs decided; all Open and Choke games resolved, while
    23 Island legs remained active strategic stalemates with hundreds to
    thousands of additional accepted commands rather than frozen simulations.
    The new decisions split five for the higher rung and six for the lower, so
    duration alone does not repair the ladder.
  - Traced paired-seat divergence to global UnitId fanout and absolute collision
    ordering. Replaced those choices with owner-local ranks and half-turn-local
    geometry, then locked the fix with a 600-tick mirrored opening and a
    different-ID perfect-stack regression.
  - Traced the higher-rung force fragmentation to optional specialties running
    before an ordinary fighting line existed. Adaptive profiles now fill an
    unreserved Sentinel-equivalent core before discretionary work, while
    persistent operations own bomber and ground-attack-air cohorts; focused
    tests cover full queues, partial banks, same-think accounting,
    multi-Airworks ownership, difficulty prefixes, and the profile-free control.
  - Reduced the remaining Standard-versus-Veteran paired sweep to a simulation
    seam rather than bot policy: mirrored commands and authoritative state stay
    rotation-equivalent through tick 2,220, then an unrelated Harvester takes a
    different collision-resolved movement step after the mirrored mine site is
    placed. Preserve the exact replay and add a minimal half-turn-equivariance
    regression before recalibrating the ladder.
  - Added a fair execution rung: Veteran and Prime coordinate an engaged army
    onto one legal focus target, while Scrapheap and Standard rely on ordinary
    auto-acquisition. The exact R16 Standard-versus-Veteran affected cohort then
    moved from an inversion to 18 Veteran wins in 18 paired legs across Open and
    Choke for both factions.
  - Corrected base reconnaissance so dispatch is not sight, Foundries outrank
    nearer Extractors, dedicated air scouts inspect the defended rear side, and
    only current visibility over that rear sample marks defense intelligence
    fresh. Keep the ladder open until the fresh complete-match matrix confirms
    this behavior.
  - The R20 controlled ladder put Standard over Scrapheap 28/4 and Prime over
    Veteran 42/21, but Standard still edged Veteran 17/15. Exact replay
    comparison found three semantic inversions: strategic attention changed
    production composition, longer enemy-force memory only extended the
    voluntary push veto, and coordinated focus pulled an entire mixed army after
    harmless anti-air.
  - Decoupled competent-rung production breadth from strategic attention,
    limited the voluntary attack-risk horizon while retaining longer
    intelligence, and restricted focus orders to compatible front-line units
    threatening the army. Added repeated queue-lifecycle, all-boundary
    monotonicity, target-selection, stale-focus recovery, and empty-command
    regressions.
  - Ran the R21 adjacent ladder after production and personality corrections.
    Standard beat Scrapheap 31-1, but Standard-Veteran split 17-15 and Veteran
    beat Prime 18-14; all 96 matches decided with zero rejected commands, so the
    upper ladder remained uncalibrated.
  - Replay forensics found three controller inversions behind the upper ladder:
    coordinated focus pulled whole front lines off attack-move into brittle
    target chases, Prime advanced air reconnaissance before its required scout
    existed, and longer Prime memory held paid operation members after Veteran
    released them.
  - Corrected those semantics with exact regressions. Focus fire now requires
    the complete compatible front line to already be in range; Recon cannot
    advance without a real scout dispatch; active paid operations use one shared
    540-tick evidence horizon while Prime retains longer uncommitted target
    memory.
  - R22 exact replay cells improved Standard-Veteran from two Standard sweeps to
    three Veteran wins and one active 120,000-tick Choke leg. Prime no longer
    loses both replay-derived cells, but Veteran-Prime still followed the
    physical seat in three of four legs, so Prime needs a real tactical edge
    before broad calibration.
  - Final adjacent-difficulty calibration ran 96 paired legs. Standard beat
    Scrapheap 19/13, Veteran beat Standard 17/15, and Prime beat Veteran 18/14;
    all matches decided with zero command rejections or tick-limit outcomes.
  - The ordering is aggregate, not absolute. Many cells still split by physical
    seat, including a 25/7 seat skew in Standard versus Veteran, so keep
    calibration open for human promotion play and map/seat disentangling rather
    than claiming that every higher-rung bot beats every lower one.
- [x] Expose setup controls and replay metadata, then assemble the human
      promotion review pack.
  - Added paged New Match controls for difficulty and stance, with deterministic
    automation seeds and entropy-backed ordinary sessions.
  - Preserved the exact opponent configuration through launch, Restart, Rematch,
    saves, replay playback, summaries, selection labels, and results.
  - Verified the setup layout at the minimum supported window, exact seed
    ownership, configuration labels, the 14-step native smoke path, and the
    four-test native menu transition battery.
  - Replaced the rejected large opponent editor with two direct Difficulty and
    Stance controls in each bot row. Kept the map preview visible and covered
    keyboard, mouse, touch, compact layout, field independence, protocol labels,
    and minimum-window native rendering.
- [x] Reconcile documentation and pass focused, workspace, coverage, Markdown,
      replay, and native QA gates.
  - Updated the root and crate documentation, both architecture documents, and
    the scripted-bot skill for the maintained configuration, planning,
    evaluation, setup, and replay contracts.
  - Passed rustfmt, workspace Clippy, the complete Rust suite, rustdoc, every
    canonical skill validator, and the all-Markdown Prettier gate.
  - Passed unit coverage at 73.00% against a 69.0% floor and combined coverage
    at 79.88% against a 76.5% floor, plus the 14-check native smoke and
    four-test native menu battery.
  - Ran cargo clean after the final evidence and gates; Cargo removed 90,863
    files totaling 13.8 GiB, and the build tree remains cold.
  - Added replay-shaped army lifecycle coverage for contact, focus fire, march
    refusal, contested withdrawal, and objective release; a fresh-intelligence
    difficulty matrix also exposed and fixed a precedence bug in the commitment
    floor.
  - Final R23 closeout passed rustfmt, Clippy with warnings denied, the full
    workspace suite including the shipped-map soak, rustdoc with warnings
    denied, every canonical skill validator, all-Markdown Prettier, and both
    raised coverage gates.
  - Reviewed and regenerated the deterministic CPU goldens and shipped-scenario
    state hashes after the intended simulation drift; repeated captures were
    byte-identical and the showcase's semantic feature contract remained
    complete.
- [x] Run a broad personality and difficulty match evaluation before promotion.
  - Run paired same-difficulty matches across many distinct personality seeds;
    evaluate whether different identities produce credible, close contests
    rather than merely different metrics.
  - Run cross-difficulty matches with seat swaps; the higher rung should ideally
    win consistently, and any reversal must be investigated for an
    implementation bug before it is treated as balance evidence.
  - Do not alter unit, economy, map, or broader game balance without Connor's
    sign-off. Record non-bug tuning candidates and the match evidence instead.
  - Completed an initial 90-match deterministic matrix across difficulties,
    stances, personalities, and five duel maps; preserved compact rows plus
    targeted replay evidence.
  - Invalidated the first difficulty ladder for calibration after replay review
    found cadence-sensitive duplicate expansion founders and a builder/scout
    ownership collision; treat these as controller bugs, fix them, then rerun
    the ladder before drawing balance conclusions.
  - Found large-map pathologies separately: island scenarios accumulated
    unreachable commands and tens of thousands of no-route stalls; preserve
    those replays and fix only concrete routing defects before the final map
    matrix.
  - Independent review found the deferred Foundry reserve must be policy-wide
    and precede strategic scheduling; a local production reserve still lets
    other purchases starve an already-issued claim.
  - Independent route review found three remaining liveness gaps before rerun:
    an exact-position Load livelock, known-object walls missing from army
    preflight, and loaded ferry cargo stranded when its target disappears.
  - Audited the profile-free path before calibration: player-facing defer, scout
    ownership, economy-boundary, and capital logic must not silently flow into
    the frozen Overseer QA controller; shared route correctness fixes will be
    rebaselined explicitly.
  - Reserved every unique deferred construction claim and cancel voluntary
    Repair/RepairUnit programs while one is unpaid; automatic Repair Bay aura
    remains a simulation-side debit against visible scrap. Deliberately did not
    add engine escrow or alter human-facing economics without Connor's sign-off.
  - Closed the controller calibration blockers without tuning: every unique
    deferred construction claim now reserves policy-wide scrap, pending sites
    count toward caps, active voluntary repair yields to unpaid construction,
    construction/scout ownership is deterministic, and forward defenses no
    longer redefine the enemy home half. The exact previously failing paired
    seed reached its Foundry with zero rejections or insufficient-scrap stalls;
    profile-free Overseer retained its legacy path.
  - Closed the final pre-matrix controller bugs: active air, relief, raid, and
    executive ownership now share one canonical reservation ledger; ferry
    loading cannot replace an earlier exact order or claim reserved units; and
    the frozen Overseer retains its legacy ferry and army-route policy while
    only the player-facing bot uses the new fog-honest route checks.
  - Invalidated the first 256-leg calibration pass after exact replay review
    exposed controller defects rather than balance evidence: unreachable
    worker/source choices, optimistic island routing, a repeated ferry Load
    loop, permanent wounded-unit retirement, and fragmented staged armies.
  - Fixed each controller defect without changing unit, economy, map, or
    difficulty constants; the formerly stuck cleanup seeds all decided, and the
    exact Scattering cohort fell from 205 repeated Loads and 831 NoRoute stalls
    to distinct ferry trips with no concentrated stall cohort.
  - Preserved the frozen Overseer command path during the liveness work,
    including legacy ferry rider ordering, and prevented nearby staged bodies
    from consolidating across a fog-honestly known wall.
  - Cleaned 15.4 GiB of accumulated Cargo artifacts before the final cold build
    and fresh matrix.
  - Traced the remaining Scattering loop to route-blind muster after a ferry
    landing: a remote Sentinel and local Flakhound became one army, then
    withdrawal re-staged them at an unreachable arithmetic centroid and cycled
    every 18 ticks. Player-facing FormArmy now drafts and reinforces only across
    explored shared ground, while Overseer remains unchanged; the exact failing
    seeds decided with zero concentrated NoRoute stalls.
  - Traced the apparent singleton attacks to FormArmy reinforcements targeting
    an obsolete forward rally, not to desperation pushes; player-facing
    completed and route-failed armies now relinquish their members while live
    objectives remain held and Overseer stays frozen.
  - The frozen-tree r13 cleanup probes eliminated the former loop: both exact
    personality legs decided in 14,007 and 15,395 ticks, the formerly stuck
    stance swap decided in 29,194 ticks, commands had zero rejections, and the
    remaining stance timeout was still an active two-base match rather than
    inert cleanup.
  - Found and fixed a separate air-operation defect before the final matrix:
    reconnaissance repeatedly replaced an identical scout move and spent its
    phase timer while waiting for the scout to be trained. Scout dispatches are
    now persistent and a new assignment starts its real flight window.
  - Removed seat-dependent scenario jitter from player-facing scripted dials so
    identical profiles compare symmetrically; the profile-free Overseer keeps
    its frozen jitter.
  - Closed final command-liveness gaps before freezing the matrix: air scouting,
    bomber holds, artillery staging, relief, and raids now retain stable
    dispatched orders; strategic reservations cannot accidentally complete a
    utility raid wing.
  - Made player-facing orphan-site relief require a route-capable free builder,
    so one paid site in a disconnected component cannot suppress reachable
    construction until it decays.
  - Re-ran the exact Prime air-identity seed after the source freeze: both
    paired legs decided at ticks 14,007 and 15,395 with zero command rejections,
    and the Kestrel emitted one ingress move rather than replacing it every
    think.
  - Re-run every final-r5 tick-limit leg with a much larger deterministic
    ceiling; record its decision tick or classify the surviving strategic
    stalemate from replay evidence before drawing balance conclusions.
  - Froze final R7 to driver SHA
    daa5545bb80adc7f80d9b6bde85b60ec895db2ea73a22eeb9e0933d4eb480900, ran 256
    paired legs twice, and obtained byte-identical normalized evidence across
    every leg.
  - The normal ceiling produced 240 decisions, 16 tick limits, zero command
    rejections, and no concentrated no-route cohort. Replayed all 16 cutoffs
    from their exact hashes at larger ceilings; every match eventually decided.
  - Fourteen cutoff matches resolved within four times the original ceiling.
    Prime seed 963001 forward and Veteran seed 962006 swapped resolved before
    five times, at ticks 235,947 and 217,131.
  - Distinguished long but active attrition from three controller concerns:
    Scattering disconnected-island recovery, Prime seed 963001 leaving 142
    combat units idle behind a six-unit attack, and Scrapheap seed 960007
    delaying a won endgame for tens of thousands of ticks.
  - The same-difficulty personality cohort was often decisive rather than close,
    and map results showed strong physical-side effects. Preserved these as
    calibration evidence and made no unit, economy, map, or balance changes.
  - All 48 map legs decided, but Cupric seat 1 won 33 to 15 and 13 of 24 pairs
    followed the physical side. Faction, roster, spawn, and seat remain bound,
    so the matrix cannot attribute the effect.
- [x] Close the final replay-proven endgame liveness defects without balance
      tuning.
  - Reinforce or recall the undersized Prime attack while a large combat reserve
    waits at home.
  - Make a dominant Scrapheap army convert a won endgame instead of remaining
    idle for tens of thousands of ticks.
  - Fix Scattering's disconnected-island recovery and repeated-loss loop without
    adding map-specific omniscience.
  - Reproduce each exact seed, repeat affected cohorts, and rerun required gates
    before replacing the frozen R7 evidence.
  - Fixed the three replay-proven endgame liveness failures without changing
    unit, economy, map, or difficulty values: forward-army ownership no longer
    masks a fresh home muster, remembered defenses decay by confidence while
    preserving their observed tier, and dispatched harvest work persists long
    enough to quarantine a source after a worker loss.
  - The exact Prime seed 963001 forward repro fell from 235,947 ticks to 15,028;
    Scrapheap seed 960007 swapped fell from 125,978 to 70,336; Scattering seed
    973000 forward fell from 225,462 to 31,911. All three retained their winner
    and emitted zero rejected commands.
  - Closed one final fog-memory edge found in review: revisiting salvage beside
    a remembered upgraded turret cannot clear quarantine by observing only the
    structure's base-tier ghost. Added an adversarial regression and documented
    the clearance rule.
  - Froze R9 to driver SHA
    ce3a5ced06cdeabb157868a5bcc15773556d0cfc8b75f0de17764ba0e45b3fca and ran the
    complete 256-leg personality, ladder, stance, and shipped-map matrix twice.
    Both passes produced normalized hash
    5c0839cfbf4214876566f6990a2b5bad3c5b96c80db122333a0199068742efa,
    byte-identical replay banks, 246 decisions, 10 normal-ceiling cutoffs, and
    zero command rejections.
  - R9 matched R8 behavior in every matrix cell after removing only the expected
    candidate and replay labels, so the last fog-memory hardening is a covered
    regression but was not encountered by the broad sample.
  - Extended every one of the ten normal-ceiling cutoffs from its exact replay
    prefix. All eventually decided: nine between ticks 51,032 and 72,114, and
    Basalt Spine seed 971000 swapped at tick 136,470. None is a permanent hard
    deadlock.
  - Kept personality and difficulty calibration open. Air and greed are clearly
    legible, fortification is weaker, and siege, support, and guile still need
    better behavioral signatures. Standard lost badly to Scrapheap, Prime lost
    its adjacent pairing to Veteran, and physical seat, faction, and map
    geometry remain heavily confounded. Made no balance changes.
  - Passed rustfmt, workspace Clippy, the full Rust suite, rustdoc, every
    canonical skill validator, unit coverage at 73.36%, and combined coverage at
    80.15%.
  - Ran the requested final Cargo cleanup after validation; Cargo removed 58,769
    files totaling 10.1 GiB and returned the worktree to a cold build.
- [x] Make the bot defend threatened owned expansion Foundries.
  - In the human Prime replay, five visible Scuttlers destroyed the expansion
    while three Sentinels stayed at the original base because defensive threat
    detection included only the home Foundry and allied Foundries. This is a
    controller bug independent of economy balance.
  - Added player-facing defense objectives for every completed, living owned
    Foundry under a currently visible threat; expansion and home Foundries now
    draw from the same available defenders while Overseer retains its frozen
    home-only policy.
  - Added a deterministic regression modeled on the human replay: five visible
    Scuttlers threatening an expansion now mobilize the three otherwise-idle
    home Sentinels.
- [x] Make harvesters stop feeding replacements into a known local kill zone.
  - Worker danger memory is reactive and keyed to one exact resource tile.
    Wrecks beside dead workers therefore appeared as fresh sources inside the
    same remembered Bastion envelope, and replacements were sent back. Make
    assignment risk-aware under current and remembered fog-honest evidence, and
    retain danger across the local threatened region.
  - Added anonymous, bounded, fog-honest regional salvage incidents to strategic
    observation. Recent worker losses suppress resource assignments across the
    local kill zone instead of quarantining only one exact pile.
  - Covered the replay failure at both decision boundaries: a replacement avoids
    nearby fresh wrecks after the attacker leaves sight, and the local region
    becomes eligible again after the bounded 15-second warning expires. Overseer
    remains unchanged.
  - Replay validation found that safe endpoints were insufficient: a harvester
    could leave quarantine, accept a source whose ordinary command route crossed
    the remembered kill zone, and alternate Harvest and evacuation every 21
    ticks. Player-facing work selection now verifies the exact deterministic
    command path in both directions; the same seed emitted no repeated worker
    command group through tick 10,000.
- [x] Establish the renewable Extractor and forward-Foundry economy.
  - Replaced global time escalation with fixed renewable income: a remote
    Extractor yields 120 scrap per minute and one completed same-owner Foundry
    within eight footprint tiles raises it to 180. Support does not stack.
  - Added one visible supported derelict and at least one unsupported expansion
    frame per starting side on Skirmish, Cinder Steppe, and The Scattering. The
    frame remains an explicit opening restoration choice.
  - Updated the player-facing bot to reserve a legal safe home restoration,
    consolidate remote owned Extractors with route-valid support Foundries after
    Fabricator tech, and project completed, upgrading, pending, and deferred
    Reclaimer income without a bot-only cap.
  - Added shell feedback for remote versus supported yield, the support radius,
    non-stacking behavior, support boundaries, and the exact supporting Foundry
    link. Verified the selected supported state through the native GPU shell.
  - Exact paired Prime runs decided all four Skirmish legs between ticks 9,013
    and 18,753 and all four Cinder Steppe legs between ticks 10,928 and 11,566,
    with zero rejected commands after the frame, rally, and support-site fixes.
  - The Scattering still exposes an island-endgame liveness issue under
    renewable income: one exact forward leg remained active at tick 120,000 with
    more than 230 mostly idle Sentinels per side while tiny ferry raids
    continued. Treat this as controller evidence, not permission to change unit,
    map, or economy values.
  - Closed an adjacent Scattering command bug exposed by the rollout: artillery
    now scouts an unexplored staging ring before treating optimistic ground as
    real. The exact former rejection seed fell from two unreachable goals to
    zero; known water aborts the operation without moving artillery.
  - Independently audited all three map layouts. Every new frame is rotationally
    symmetric and builder-reachable, and each natural has many legal reachable
    Foundry support anchors. Strengthened the shipped-map gate to prove frame
    symmetry and frame-perimeter access directly.
  - Final independent policy review caught and closed two edge cases:
    support-site search now includes the nearest legal non-overlapping ring, and
    an already-walking Foundry promise no longer banks a second expansion fund
    after its own cost is reserved.
  - Fixed the factorial geometry probe to rotate Extractor frame anchors as 2x2
    footprints. Its old point-marker rule rejected the correctly symmetric
    promoted maps; an explicit involution regression now pins the frame
    transform.
  - Re-ran the exact Scattering air-identity seed after the final support and
    reserve fixes. Both seats emitted zero rejected commands through tick
    60,000; the broader single-transport attrition loop remains an open strategy
    problem.
  - Tightened the Foundry reserve regression around the real caller boundary: a
    walking founder escrows its cost exactly once, uncommitted scrap may still
    fund production, and the next Foundry fund begins only after the current
    promise stands.
  - Passed rustfmt, workspace Clippy, the complete Rust suite, rustdoc, all
    canonical skill validators, the all-Markdown Prettier gate, unit coverage at
    73.51%, and combined coverage at 80.36%. Native smoke and selected-Extractor
    review had already passed against the same implementation.
  - Ran the requested final Cargo cleanup after the last coverage pass; Cargo
    removed 78,979 files totaling 12.3 GiB and left the repository on a cold
    build.
- [ ] Replace the singleton ferry with scalable coordinated island assaults.
  - Replaced the player-facing singleton ferry with a persistent lift planner
    that freezes and reserves a canonical payload, scales Skyhooks from
    transport capacity, assigns disjoint manifests and landing sites, launches
    behind one shared boarding barrier, hands off only landed riders, and
    recovers every surviving carrier. The profile-free Overseer keeps its frozen
    legacy ferry.
  - Added bounded provisioning, boarding, support, landing, and recovery failure
    paths; support success and aborts persist across planner ticks, partial
    waves need at least half their carriers and payload, and aborted preflight
    operations enter a retry quarantine.
  - The first corrected 60,000-tick Scattering probe decided naturally at tick
    45,052 with zero rejected commands and both factions producing multiple
    Skyhooks. Replay inspection still found repeated home-side Load and Unload
    cycles after live objectives became fog ghosts, which aggregate match
    metrics did not expose; restricted new operations to current sightings,
    added retry quarantine, and queued a second exact replay probe.
  - Follow-up exact replays separated three issues: remembered objectives caused
    repeated home-side load cycles, executive ownership kept idle fighters out
    of wealthy lifts, and partial boarding could turn a planned wave into a
    trickle. Restricted uncoordinated starts to current sightings, quarantined
    aborted retries, required a shared launch quorum, and allowed only
    targetless or safely staged armies to transfer from the executive.
  - The final exact Scattering replay produced two synchronized three-carrier
    Ferrous waves: 12 riders landed together at tick 17,694 and 11 more boarded
    together at tick 21,762. Cupric also used paired-carrier waves; neither side
    repeated the former fog-ghost load loop, and all commands were accepted.
  - Replay forensics found that the former capped personality matches were not
    merely long: six later loaded waves of two to nine carriers returned home
    immediately when the paired air operation aborted. Bulk waves of at least
    three carriers now proceed independently after a matching abort, while one-
    and two-carrier probes still recover.
  - Bounded the coordinated support wait from the moment a fully boarded wave
    enters it. Repeated suppression signals no longer refresh the deadline; a
    prepared three-plus-carrier wave launches independently at expiry and an
    undersized wave recovers.
  - Re-ran the six formerly capped same-difficulty personality legs after the
    ownership and fallback fixes. All six decided by tick 56,915 with zero
    rejections; the exact former cutoff launched four Skyhooks carrying 13
    riders immediately behind a two-Buzzard/four-Condor strike.
  - Reordered new optional raids behind primary lift planning while preserving
    ownership for raids already under way. A full Brain regression now starts
    the non-Air bomber plan, freezes an eight-Skyhook/32-rider payload, and lets
    Prime draw a three-unit raid only from the leftovers; a companion proves an
    active raid cannot be stolen by the lift.
  - Exact replay review caught a state-machine regression behind the remaining
    home-side load loop: a matching air abort sent even seven fully loaded
    Skyhooks into recovery despite the intended independent bulk fallback.
    Prepared waves of at least three loaded carriers now launch after an abort
    or bounded support wait; one- and two-carrier probes still recover. The
    58-test lift suite covers both branches.
  - Exact private-state reproduction found why a wealthy natural match still
    formed undersized lifts: at tick 48,057 Prime had 33 transferable staged
    fighters and no ground strategic conflicts, but the first canonical pickup
    tile belonged to a tiny building-separated component, so only five riders
    entered a two-carrier payload. Select the strongest legal near-home pickup
    component and keep it stable through provisioning and boarding.
  - Fixed component selection so a wealthy lift reserves the strongest reachable
    home-side fighter group instead of the first canonical pickup pocket. The
    exact seed 6,000 replay then formed three-Skyhook waves carrying up to ten
    fighters.
  - A coordinated air abort no longer deletes a still-current lift during
    Provision. It unlatches and keeps building the independent transport wave; a
    lift admitted only against a remembered fog ghost still cancels and enters
    retry quarantine.
  - A lifecycle coverage probe exposed frozen bulk waves abandoning two fully
    loaded surviving carriers after a third carrier died. Closed only the
    missing manifest, released its riders, and let the surviving quorum launch,
    land, attack, and recover without provisioning a mid-wave replacement.
  - Extended 21 stratified Island cutoff cells from 60,000 to 240,000 ticks;
    none resolved. The simulations remained deterministic and active, but landed
    survivors went idle after their first objective vanished, solo scout
    recovery was circular, and unconditional production grew hundreds of
    undeployable Sentinels.
  - Kept landed survivors reserved through the far-shore assault, retargeted
    them deterministically to a current reachable building or reachable
    unexplored terrain, and released them only when no fog-honest job remains.
    Returned carriers release independently.
  - Replaced the unconditional player-facing Sentinel and Lancer stream with
    deployment demand. Reachable current or remembered ground objectives, a
    current disconnected objective with real transport capacity, or a known
    desperate mirror road admit recurring production; minimum core replacements,
    bounded specialists, and the frozen Overseer remain unchanged.
- [ ] Make wealthy bots independently field bomber waves on The Scattering and
      similar island stalemates.
  - Added a wealthy-island air playbook that is available independently of the
    air personality trait. It scales a fighter screen and bomber wing from the
    renewable economy and roster, spreads training across Airworks, reconnoiters
    remembered objectives honestly, and attacks currently visible flak anywhere
    along the known flight corridor before committing to the objective.
  - In the final exact Scattering replay, Ferrous committed an eight-aircraft
    package of two Buzzards and six Condors while three Skyhooks boarded the
    ground wave; Cupric independently fielded six Moths. Replay inspection found
    and fixed identical strike orders being reissued every think, reducing
    Ferrous commands from 376 to 177 without changing the final state hash or
    outcome.
  - Proved the map does not gate this behavior behind the Air identity: a Prime
    Greed/Guile profile with air trait 48 built two Buzzards and six Condors and
    launched them at tick 17,298. A later exact match showed one bot landing two
    Skyhooks while its five-Moth screen was already flying toward the same
    enemy.
  - The first exact Prime-versus-Standard regression cohort still exposed a
    difficulty inversion after the transport fixes: Standard won six of seven
    decisions and one leg reached tick 60,000. Preserved all eight replays and
    kept difficulty calibration open rather than changing unit, building, map,
    or economy values.
  - Made utility production account for strategic Airworks prelude orders, so a
    requested replacement scout cannot be duplicated ahead of the intended
    screen or bomber batch.
  - The fresh cohort confirmed bomber consideration on The Scattering across
    both factions and non-Air identities. Its clearest combined operation loaded
    four Skyhooks with 12 riders, launched a two-Buzzard/two-Condor screen, then
    sent all four transports to the same objective 165 ticks later.
  - Added a composed Brain-to-State regression independent of lift planning: a
    mature non-Air Support/Greed profile with no transport-capable ground
    payload emits a legal grouped strike of six Condors and three Buzzards
    before tick 10,000.
  - Independently re-audited the final dirty tree after the bulk-lift
    requirement changed. The controller and focused regressions prove
    bomber-only, transport-only, and shared-objective operations; the authored
    seed 6,000 replay contains two six-aircraft Cupric strikes, one
    four-aircraft Ferrous strike, and a separate three-Skyhook crossing.
  - A fresh four-seed Prime-versus-Standard Scattering cohort exposed two
    unrelated controller defects before it could serve as calibration evidence:
    repair work could pull an evacuated Harvester back into its quarantined kill
    zone every think, and ground armies repeatedly focus-fired swooping aircraft
    across unstandable water and buildings, producing an authoritative NoRoute
    storm. Keep both fixes controller-only and rerun the exact seeds.
  - Consolidated replay inspection produced the intended natural combination at
    tick 55,233: eight Moths and five Darters were attacking the Ferrous
    expansion while three Skyhooks boarded eleven ground units; all three
    transports crossed and unloaded beside the same objective.
  - Closed two controller defects exposed by the invalid calibration cohort.
    Repair can no longer recruit an evacuated worker back into visible or
    remembered danger; the exact 60,000-tick repro fell from 2,222 alternating
    repair and escape orders to none. Ground armies no longer explicitly pursue
    aircraft across unstandable terrain; the exact seed fell from 1,484 NoRoute
    stalls to one and decided at tick 29,179.
  - Extended the exact seed 6,000 forward leg to 120,000 ticks and found a true
    controller deadlock: Prime repeatedly sent one Harvester toward a Scuttle
    Charge site through its own remembered kill zone, evacuated the founder on
    the next think, let the site decay, and retried for the final 80,000 ticks.
    Bound construction to an exact worker whose canonical route avoids current
    and remembered danger; if no such route exists, the bot waits.
  - Private-state replay forensics found a separate coordinated-operation
    failure: Prime assembled ten Moths, five Darters, seven Skyhooks, and 26
    riders against one objective, then treated three ordinary Sentinels' weak
    skyward weapons as an unsuppressible hard AA gate. The air abort deleted the
    viable lift, after which utility sent the aircraft alone. Keep Flak a hard
    suppression gate, distinguish credible mobile AA engagement, and let the
    bulk lift continue independently after an air abort.
  - Exact replay-shaped regressions now distinguish mobile AA from hard static
    suppression. A frozen ten-Moth/five-Darter wing accepts twelve visible
    Sentinels and strikes, while seven dedicated Flakhounds still force
    recovery; current Flak remains a suppression target. The decision
    deduplicates visible sources and compares a conservative exposure budget
    with current wing HP.
  - Added full Brain lifecycle coverage for a wealthy same-objective operation:
    the mixed bomber/screen wing must release before any target unload, every
    three-plus-carrier manifest unloads, every landed rider enters the
    same-target assault, and every command remains legal. Strengthened the
    independent abort and mobile-AA-versus-Flak regressions at the same time.
- [x] Establish near-complete behavior-level coverage for the player-facing bot
      and raise the enforced coverage floors.
  - Added routing and intelligence regressions for canonical mixed-domain
    groups, fog-known versus optimistic paths, blockers and stealth,
    unreachable-preference fallback, deferred footprint escape, malformed maps,
    contact identity and replacement, resize/schema handling, idempotence, and
    exact mobile anti-air range.
  - Added construction and recurring-income regressions for Repair Bay, Bastion,
    Flak, ordinary turrets, exact builder-bound authoritative construction,
    Reclaimer and Refinery completion, Foundry warmup, supported Extractors, and
    deferred-income deduplication.
  - Keep the task open until real seeded Brain lifecycles, difficulty
    monotonicity, personality counterfactuals, failure exits, LLVM per-file
    review, and raised workspace floors are complete.
  - The full integration suite rejected an apparent income-projection fix and
    clarified the maintained contract: paid Reclaimer sites, offline automatic
    Refinery upgrades, and deferred claims count at eventual yield to prevent
    duplicate construction. Restored that behavior, documented the projection
    boundary, and rewrote the new test. Kept the valid defensive fix that a
    malformed same-anchor different-kind building contact cannot inherit stale
    identity, age, or tier.
  - Raised combined player-facing bot coverage to 25,781 of 26,121 executable
    lines, or 98.70 percent, with 2,254 of 2,327 functions covered. The 426
    library tests now guard finished-state immutability, real difficulty
    cadence, fog-honest recon, operation abort and recovery, construction and
    routing failure boundaries, relief loss and timeout limits, lift boarding
    loss, and exact command lowering.
  - Coverage probes exposed one real controller bug: an explicitly staged scout
    could receive an impossible move through fog-known peaks. Added a fog-honest
    air-route check and lifecycle regressions for Assemble, Suppress, Verify,
    and Strike.
  - Added behavioral construction regressions across turret caps two through
    four before and after a raid, ambiguous versus confirmed air evidence, and
    the frozen profile-free controller. A seeded four-difficulty ground-cohesion
    regression exposed that the fresh-intelligence floor multiplied only the
    stale branch by unit strength; corrected the precedence and now prove that
    no rung launches the brittle five-Sentinel body while reinforced commitments
    remain monotone.
  - A fresh LLVM audit measured 6,856 of 6,890 executable lines across the
    focused production bot files, or 99.51 percent. Difficulty and adaptive
    production were at 100 percent; the remaining misses were overwhelmingly
    defensive invariants or attribution artifacts rather than useful test
    targets.
  - Added exact behavioral contracts for every difficulty memory boundary,
    weaker and stronger sight updates, rear-defense reconnaissance, bounded
    solo-scout retry, demand-driven recurring production, finite core
    replacement under reservations, and every far-shore survivor handoff and
    release path.
  - Final focused LLVM coverage measured 34,661 of 35,015 player-facing bot
    lines, or 98.99 percent. The remaining misses are rare defensive and
    terminal paths rather than an untested subsystem.
  - Raised and passed the workspace floors from 69.0 to 83.5 percent for unit
    coverage and from 76.5 to 86.0 percent for combined coverage. Final
    measurements were 84.25 and 86.93 percent respectively.

## Open Questions

- Which generated profiles remain fun and legible after complete human play,
  rather than merely producing statistically different commands?
- How should the next calibration experiment unbind faction, physical seat,
  spawn geometry, and roster before difficulty parameters are tuned?
- How strongly should guile and mobile siege alter plans before those identities
  become brittle or predictable?
- Does Scattering need a dedicated map-aware ferry and island-endgame policy
  before it remains in bot calibration?
