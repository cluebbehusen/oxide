---
created: 2026-09-25T17:14:02
updated: 2026-09-27T13:15:21
---

# Connected Air Offense Redesign

## Goal

Make the bot's connected air offense easier to reason about by redesigning its
operation model around a fixed target-cluster commitment, accepting behavior
changes that play at least as well. Tracked by
[CL-46](https://linear.app/cluebbehusen/issue/CL-46).

## Decisions

- Behavior and controller checkpoint layout may change. Judge the result by bot
  evaluation and native replay review; state hashes do not cover this code.
- Keep type refactors separate from behavior changes, so a refactor is proven by
  identical command and final hashes on every baseline leg.
- Fix the first-Airworks defect before the redesign and leave valuation tuning
  to the planned post-playtest rebalance. An Airworks before the Crucible is
  acceptable because it matches a normal human opening.
- Keep revising an admitted package during Recon and Assemble, and keep moving
  unpaid purchases to an earlier or different factory, so the bot adapts instead
  of abandoning half-built attacks. Revision never extends the deadline, and
  membership freezes on entering SuppressAa.
- Commit to the admitted target cluster instead of one building: freeze the
  enemy seat, admitted anchor set, minimum capability, admission tick and
  deadline, and let a sticky focus inside the set drive scouting, staging and
  launch. This replaces mid-preparation retargeting.
- Size revisions against every live member of the frozen set: remembered members
  with positive confidence count toward target value and hit points, while
  anti-air stays current-only. A cluster is cleared when every admitted member
  has been observed gone: before the strike that is a lost objective, after it a
  completion.
- Keep the `ConnectedOffenseKey` shape and define it as the frozen admitted
  primary and scope anchor, stable before and after admission.
- When no admitted member is visible during the strike, strike aircraft
  attack-move toward the best remembered member as long as no anti-air is known
  along the route or approach.
- Model island, connected and remembered-reconnaissance operations as distinct
  plan types.
- Drive the planner through one observe, propose and apply turn per decision.
  Mid-prepare recoveries stay in place because later reads in the same pass must
  see the recovery.
- Keep the persisted paid-purchase ledger; unpaid provider jobs are rederived
  each decision.
- Keep the small typed inputs `CapitalReserve`, `ProducerLanes`,
  `RecoveryReturnContext`, `EconomyEmergencyRecovery` and `LiftSupportRequest`:
  each carries one distinct input, and emergency recovery deliberately runs
  without intelligence.
- Defer the one-decision lag in the operational reconnaissance view to
  reconnaissance work, and the return order lost on think's claimed-elsewhere
  early return to later lifecycle work.

## Findings

- State-hash goldens never reach connected air offense. The evidence is a
  36-cell bot evaluation: Skirmish, Cinder Steppe, The Deep Cut, Basalt Spine,
  Terrace Ledger, Subsidence, Smelter Basin, Broad Front and Crosswinds at Prime
  and Standard, Balanced and Turtle; `bot-eval --runs 4` from scenario seed 7000
  and personality seed 9000, paired on two-seat maps, for 256 legs. Runs are
  deterministic; summaries, replay digests and the harness sit untracked in
  `replays/airworks-bootstrap-eval/`.
- Only 9% of seats built the first Airworks because its bootstrap needed live
  enemy sight, 200 banked scrap and no other held saving on the same quote.
  Funding it from forecast and valuing confidence-discounted memory raised that
  to 96%, with a median first Airworks near tick 7,400.
- The baseline on that fix produced 5,020 connected operations: 22% reached a
  strike and 8% completed; infeasible preparation ended 29%, required-unit loss
  22% and a lost objective 15%. The primary objective changed 356 times and the
  sized anchor set changed in 971 operations.
- The finished redesign against that baseline: operations fell 5% to 4,765; the
  strike rate held at 21.5% and completions at 381 against 379. Retargeting fell
  to none, sized anchor-set changes to 131 operations, new-anti-air aborts from
  237 to 154 and timeouts from 294 to 257; median time to strike fell from 684
  to 642 ticks. Airworks and Crucible timing, expansion and game length did not
  move, and decided games rose from 90% to 93%. All of the change came from the
  cluster commitment; every later refactor and fix reproduced identical legs.
- Infeasible preparation (29%) and required-unit loss (22%) remain the dominant
  failures; target identity was not the problem.
- The strike phase gives up after a fixed 1,200 ticks that include the flight
  and ignore observed progress. In a reviewed replay a single-Darter strike on
  an Extractor was recalled about 30 ticks from destroying it. Bot feedback,
  including attacks backing off too readily and an active revision that cannot
  be adopted on its exact deadline tick, is tracked in
  [CL-61](https://linear.app/cluebbehusen/issue/CL-61).
- The emergency Lift handoff and the carrier-hold saving gate fix situations
  that never occur in the matrix; their regression tests are the evidence.
- `strategy.rs` was one 18,724-line file with its tests inline. The air planner
  is now a 381-line root and ten modules (roster, geometry, operation,
  targeting, air defense, connected, resources, stages, lifecycle and turn) with
  tests in their own files; deduplication removed about 390 lines of real
  production code and the type trim about 205 more.

## Actions

- [x] Record the redesign baseline on the first-Airworks fix build with decision
      traces and replay digests.
- [x] Make the first Airworks reachable from forecast funding and remembered
      targets.
- [x] Split air operations into reacquire, island and connected kinds and
      validate the planner at checkpoint restore.
- [x] Commit connected operations to their admitted cluster, evaluate against
      the baseline, and review a native replay: playable, but attacks back off
      too readily.
- [x] Deduplicate air planning and drive it through one observe, propose and
      apply turn, with strategy tests running that production-order turn.
- [x] Hand emergency air aborts to Lift and tighten the carrier-hold predicates.
- [x] Trim the planner's input and identity types, split `strategy.rs` into
      modules, refresh the bot docs, and rerun the baseline as a controlled
      comparison.
- [ ] Later, as its own measured experiment: simplify the connected package
      search in `force_package.rs` (width-eight beam, growth ladder, funded
      refinement) only if a controlled comparison shows play holds at lower
      cost.

## Open Questions

- Whether the width-eight package beam and growth ladder earn their cost. The
  baseline shows how often they change a package; judging their value needs a
  controlled comparison.
