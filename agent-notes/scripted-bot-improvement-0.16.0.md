---
created: 2026-08-29T08:00:10
updated: 2026-08-29T22:12:00
---

# Oxide 0.16.0 Scripted Bot Improvement

## Goal

Recover the 0.16.0 rules-based opponent by completing the renewable-economy map
contract, restoring fundamental macro competence, simplifying the policy
surface, and promoting only behavior proven against Overseer and human play.

## Decisions

- Treat the current 0.16.0 candidate as rejected rather than frozen or
  promotion-ready.
- Preserve the fair-opponent contract. Better worker saturation, spending,
  production uptime, expansion timing, reinforcement, and critical-mass judgment
  are ordinary competence, not cheating.
- Keep Overseer frozen as the external QA baseline. Prime must first beat it
  decisively before personality, difficulty, or advanced-strategy results can
  count as promotion evidence.
- Stop adding high-order strategic planners until the opening and ordinary macro
  game are demonstrably competent.
- Treat Condor movement and control feel as a separate simulation problem.
  Bot-policy work must not hide or work around broken player-facing flight
  behavior.
- Use coverage to protect meaningful behavior and invariants, never as evidence
  that a bot is strong, credible, or fun.
- Base cleanup on verified duplication and structural risk. Do not introduce
  generic abstractions or replace ordered vectors mechanically merely to satisfy
  source metrics.
- Every starting position must have one fully visible, builder-reachable
  Extractor frame within the starting Foundry support radius.
- Most starting positions should also have a second nearby frame that requires
  expansion beyond home support.
- Large, vast, and grand maps should place additional remote value in otherwise
  empty regions. Clusters of frames that one well-placed forward Foundry can
  enhance are explicitly desirable.

## Findings

- A direct read-only benchmark completed 21 games against frozen Overseer.
  Overseer won all 21, including 12 on Skirmish and nine completed Cinder Steppe
  cells; no scripted difficulty won.
- The latest human replay reached 7:00 with Prime on four Harvesters and three
  Sentinels while it prioritized infrastructure and banked usable scrap.
- Difficulty currently changes mostly reaction, memory, estimates, coordination,
  and hesitation. It does not govern worker saturation, army targets, expansion
  timing, or production uptime strongly enough to create a reliable ladder.
- The bot implementation contains roughly 16,500 production lines, several very
  large policy functions, dual-controller branching, loosely represented
  operation state, untyped budget plumbing, and tests coupled heavily to private
  phases.
- Independent review confirmed the broad maintainability problem but found some
  specific claims overstated: `raid.rs` and `team.rs` share lifecycle vocabulary
  rather than being near-copy duplicates, and ordered vectors are often
  intentional even though their invariants need better types.
- There is no simulation-level Harvester population cap. The adaptive
  player-facing policy instead holds ordinary worker production at a bootstrap
  target of four until renewable economy stands, then permits only the
  stance-and-greed target of four through seven live-or-queued Harvesters.
  Difficulty affects neither target. The latest Balanced profile resolved to
  four even after renewable income, so restoring its missing Extractor would
  improve income but would not make that Prime train a fifth worker.
- The complete roster now carries 491 Extractor frames, up from 77. This is an
  intentional economy-scale change, not merely a validity correction, so match
  pacing and secondary-claim fairness still require human play.
- Skyhook Anchorage cannot fit a nearby unsupported natural on each compact
  starting island without changing its topology. Its four additional claims
  remain transport-contested island expansions.
- Both bots stopped harvesting before four minutes and stopped construction by
  5:39. The aggressive bot never rebuilt its destroyed supported Extractor; both
  evacuated workers to distant corners after salvage danger and failed to resume
  ordinary economy.
- Prime carried a badly wounded combat core for minutes without repair and
  failed to answer an Avalanche and Bombard siege it could currently see, losing
  23 units and four buildings without inflicting a loss.
- Prime's fixed eight-tile Foundry-defense trigger did not respond to a visible
  Avalanche shelling from twelve tiles away even though the weapon's range is
  fourteen tiles.
- The first post-layout Salvage Triangle match lasted 18:39 and demonstrated the
  intended recovery loop: renewable home and central Extractors let a nearly
  destroyed player rebuild, expand, and pivot into siege.
- After separating Overseer policy identity from physical seat and simulation
  seed, the corrected 24-leg Prime-versus-Overseer matrix produced zero Prime
  wins, 16 Overseer wins, and eight 60,000-tick cutoffs.
- Clean Skirmish replays reproduced the opening failure on both seats: Prime
  trained only two additional Sentinels, left its Foundry without core
  production from 0:36 until after first contact, and spent on remote Extractors
  plus six infrastructure projects while Overseer reached twenty Sentinels by
  3:20.
- Crossing simulation seeds 7100 and 7101 with Prime personalities 9100 and 9101
  produced identical command streams for each corresponding Skirmish and Cinder
  Steppe leg. Those 16 rows represent eight effective games, so scenario-seed
  count cannot be treated as sample count for this opening.
- The corrected authored-FC Severance pair was still unresolved at 120,000
  ticks. Overseer accumulated 61,924 and 71,609 stalls, almost entirely
  no_route, while Prime recorded two stalls per leg; this is a routing liveness
  failure, not parity.
- Prime underproduction is a deterministic budget-ordering inversion, not a cap
  or seed effect. At exactly three fighters, voluntary construction runs before
  production and the next capital reserve is protected; each purchase leaves too
  little unreserved scrap for the missing core unit, so a serial capital chain
  starves the Foundry.

## Actions

- [x] Complete and validate the all-map Extractor layout contract.
  - Audit all 33 shipped maps seat by seat, add one supported home frame for
    every start, add a nearby expansion claim on most maps, and use grouped
    remote claims to make large empty regions strategically valuable.
  - Reworked 31 maps; Cinder Steppe and Skirmish already met the contract.
  - Added a maintained all-map gate for distinct home assignment, full
    visibility, builder access, overlap safety, legal forward Foundry room,
    transport-island viability, and grouped remote value.
  - Confirmed every seat has a distinct supported home claim, 32 maps cover
    every seat with a nearby natural, and the Skyhook topology exception
    supplies remote transport claims instead.
  - Filled large empty regions with supportable two- or three-frame expansion
    districts, including physically large maps whose pace label is only
    standard.
  - Independently reviewed all 33 layouts through CPU renders and high-contrast
    sheets; split Benchwork and Floodline into distinct three-frame districts,
    centered Salvage Triangle's shared claim, and preserved deterministic
    scenery on Open Quarry and Twin Forges.
  - Passed the complete map, workspace, coverage, Markdown, and skill gates.
- [x] Establish a promotion harness that directly compares Prime with frozen
      Overseer while separating physical seat, faction, spawn geometry, scenario
      seed, and personality.
  - Added an evaluation-only controller plan so frozen Overseer can use the
    canonical headless runner without becoming a BotConfig or player-facing
    match option.
  - Separated the fixed Overseer policy seed from simulation randomness and
    moved that exact identity across seats; recorded exact controller provenance
    in rows, replay descriptions, and replay sidecars.
  - Added paired FC/CF faction cells and authored/rotated geometry cells,
    independent crossed seed lists, execution and command fingerprints,
    effective-cell duplicate refusal, create-only evidence, and end-to-end
    coverage.
  - Ran the corrected 24-leg promotion block, a 16-leg two-personality seed
    cross, and a 120,000-tick Severance extension; preserved only four
    representative Skirmish and Cinder Steppe replays in temporary storage.
- [ ] Rebuild Prime around a strong ordinary opening and macro loop: responsive
      worker saturation, continuous useful spending, adequate defense,
      production uptime, expansion, reinforcement, and critical-mass attacks.
  - [x] Keep converting recurring income into useful capital when the bot is
        wealthy and builders are idle.
    - Reproduced the 12,840-tick stall: an unactionable Extractor route consumed
      the construction budget, shadowed the ready Crucible, and was then
      discarded by final worker safety binding.
    - Required one exact available Harvester, a safe fog-honest route, and
      preserved producer egress before charging an Extractor frame. Unsafe
      frames now yield to the next useful capital rung.
    - Covered remembered danger, persistent loss quarantine, a safe worker
      beyond the choke, and the shared Standard/Veteran opening. A fresh Salvage
      Triangle run kept spending without rejected commands.
    - Unified construction reserves with exact actionable Extractor and Foundry
      claims, including builder availability, remembered danger, route safety,
      and egress, so rejected capital projects cannot freeze the bank.
    - Made harvest chores preemptible and removed same-tick harvest orders whose
      resource is consumed by an accepted footprint.
    - Verified a fresh 23,627-tick Salvage Triangle match ended decisively with
      no rejected commands and did not reproduce the 12,840-tick bank freeze.
  - [x] Rebuild a safely recoverable lost home Extractor after its contested
        region has genuinely cleared.
    - Confirmed the replay never supplied legal negative evidence: its scout had
      died, a hidden Avalanche remained nearby, and the complete quarantine
      region was never currently visible.
    - Preserved the fail-closed rule: incident expiry and darkness cannot clear
      a loss. Recovery requires 300 uninterrupted ticks of full current sight
      with no projected danger.
    - Added one whole-chain regression from active incident through prolonged
      partial sight and 299 clear ticks to an exact ordinary Extractor build on
      the 300-tick boundary.
    - Required the same exact safe-builder preflight for both the 150-scrap
      restoration reserve and the eventual build; unavailable crews or unsafe
      remembered routes release the bank to core production.
  - [x] Make visible long-range siege a defensive threat beyond the fixed
        Foundry-defense radius.
    - Restricted the extended trigger to completed, living owned Foundries and
      currently visible enemy ground weapons whose Euclidean firing annulus can
      intersect the Foundry footprint; allied Foundries and frozen Overseer
      retain the ordinary eight-tile defense rule.
    - Covered axis-aligned and diagonal maximum range, the Avalanche blind ring,
      visibility, building completion and destruction, allied ownership,
      observation-order stability, and a full Brain-to-State response with
      ordinary accepted commands.
  - [ ] Protect a difficulty-scaled opening core from voluntary capital
        reservations and construction-first ordering.
    - Preserve the supported home Extractor and fourth-worker opening, but
      escrow missing-core scrap ahead of remote Extractors, deep tech, proactive
      defenses, and the first expansion; actual emergency defense may bypass it.
    - Give difficulty its own macro core floor and require the Prime floor live
      or queued before optional capital work, then keep at least one shallow
      Foundry queue funded so serial construction cannot idle production.
- [ ] Redefine and calibrate difficulty primarily through fair macro competence,
      with cognitive and execution differences layered on top.
- [ ] Fix confirmed correctness defects, beginning with orientation-dependent
      withdrawal targeting.
- [ ] Re-evaluate personality and stance only after the shared Prime foundation
      is competent, making style legible without turning it into a strength
      axis.
- [ ] Look into bot defense placement. The bot tends to place turrets on map
      edges and clusters mines instead of spreading them. It also places mines
      suboptimally close to its own buildings instead of out further where they
      could actually hit advancing enemies.
- [ ] Run complete human matches against every difficulty and representative
      stance before promotion.
- [ ] Perform a dedicated bot code-quality cleanup: identify and consolidate
      real duplication, replace brittle state and budget representations, split
      oversized modules and functions, reduce dual-controller branching and
      private-test coupling, remove dead code, and preserve behavior with
      focused regressions. (See exact Fable comments in References below.)
- [x] Reproduce and fix the Avalanche advancing toward an unseen enemy inside
      weapon range, which defeats its low-vision artillery role and forces
      repeated retreat orders.
  - Confirmed in combat acquisition: shared sight is special-cased to Bombard,
    so the Avalanche's 14-tile aggro and five-tile vision let it acquire an
    exact hidden target before the blind attack fallback chases that target's
    live position.
  - Cover hidden units and buildings remaining idle, stationary, and pathless,
    paired with an allied spotter making the same Avalanche acquire and fire
    from its original position.
  - Sight-gated autonomous acquisition whenever a unit's aggro range exceeds its
    own vision, so Avalanche and Bombard can use their full reach only through
    current team sight. Explicit Attack pursuit after sight loss is unchanged.
  - Added public unit and building regressions proving hidden long guns remain
    idle, pathless, stationary, and silent while allied sight unlocks exact
    acquisition and fire from the original position. Full workspace, hash,
    coverage, formatting, Clippy, and rustdoc gates pass.
- [x] Reproduce and fix Condor edge stalls and group-retarget resets.
  - Fixed unsafe edge arcs, reload-gap retarget loops, and pathless or
    dead-target hovering. Shared fixed-point flight geometry now keeps turns and
    departures recoverable within the map envelope.
  - Behavioral regressions, a five-Condor edge-and-corner lab, and the Deep Cut
    replay show no pinned or motionless ticks.
- [x] Review crowded-wing Condor control feel in native play.
- [x] Review automatic aircraft landings in native play: final unqueued ground
      destinations land turn-limited aircraft on open tiles, and idle aircraft
      auto-land.
  - Implemented and committed deterministic landing and takeoff across
    simulation, shell, protocol, driver, and bot behavior. Parked aircraft are
    ground bodies and weld patients, and the next order initiates takeoff.
  - Behavioral, symmetry, parity, and native-capture tests pass; parked-sprite
    and complete human-match feel still need Connor review.

## Open Questions

- Which observable economy signals should determine worker demand: active
  resource sites, safe work capacity, producer count, replacement pressure,
  expansion plans, or a bounded combination?
- How much of the current strategic planner surface should be retained after the
  competent macro foundation is established?
- What exact Overseer margin and human-play evidence should constitute the Prime
  promotion bar?
- Does human play support the 491-frame economy, especially Skyhook Anchorage's
  singleton transport claims, the dense perimeter economies on Pentangle Claim
  and Scramble Basin, and Open Quarry's compact central pair?

## References

Fable comments about quality and maintainability:

3. Is it maintainable?

No, not as-is. 16.5K production lines, files of 5–6.7K lines, apply_inner 484
lines, maintain_with_roster 417, production_with_air_demand 405 (10-level
nesting), Brain::act 363, LiftPlanner::think 271. raid.rs and team.rs are near
line-for-line clones with no shared abstraction. ~108 unnamed thresholds in the
planners alone. Five unreconciled "scrap budget" numbers flow through Brain::act
via three Observation clones. 610 tests at 99 % coverage, but a large fraction
pin private phases, exact reservation vectors, and cache build counts; reviewers
estimated 30–60 % breakage for a reasonable policy change. Ten checkpoint
commits with empty bodies, one of them 48K lines. This is maintainable by an
agent with unlimited patience, not by a person.

4. Is it high-quality, idiomatic Rust?

Middling: 6–7/10 idiom, 4–6/10 maintainability across five independent reviews.
Honoured without exception: no floats, no hash iteration, tie-breaks end in id
or (y, x), exhaustive matches as compile-time tripwires, zero lint suppressions,
unusually good why comments. Recurring faults: Vec + sort/dedup/contains as a
set at ~60 sites where BTreeSet is the type; Option<Planner> ×6 as a mode flag;
LiftManifest with five loose bools; phase machines as 16 scattered assignments,
one missing its phase_started_at stamp (lift.rs:438); unreachable!() inside
let-else on two-variant enums; a fabricated UnitId(u32::MAX) sentinel to measure
strength; #[cfg(test)] counters welded into production structs; 13 throwaway
RouteProjections per think; a linear unit() lookup over a documented-sorted vec
in hot loops. Two real correctness findings: withdrawal_threat uses an absolute
(y, x) tie-break on the unoriented observation (armies.rs:969), and Orientation
has four states while 12 shipped maps have 6–8 seats. The non-bot sim/engine
changes are the best code on the branch (8/10).
