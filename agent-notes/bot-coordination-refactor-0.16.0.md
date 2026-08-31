---
created: 2026-08-31T06:45:14
updated: 2026-08-31T06:45:38
---

# Oxide 0.16.0 Bot Coordination Refactor

## Goal

Make bot arbitration and budget features cheap to land by giving the
coordination layer real types and whole-behavior tripwires, without moving
frozen Overseer behavior.

## Decisions

- Work happens on `0.16.0-refactor`, branched from the unshipped `0.16.0`. Do
  not merge back into `0.16.0` without explicit direction from Connor.
- No `SIM_VERSION` or workspace version bump on this branch: 0.16.0 has never
  shipped, so intended behavior changes fold into the already-deferred 0.16.0
  bump. Reblessing goldens is licensed.
- Frozen Overseer behavior must not move. The existing 33-map 2,000-tick
  state-hash rows stay bit-identical through every commit; new rows are
  additional coverage, never replacements.
- Every slice is verified against both hash fixtures before commit. Hash-neutral
  slices leave both goldens untouched; the explicitly listed behavior-changing
  slices rebless only the player-facing golden, with the movement explained in
  the commit message.
- Approved floor: (1) a player-facing whole-behavior hash fixture plus a
  6,000-tick Overseer horizon; (2) one component flood for the extractor claim
  instead of per-candidate BFS; (3) `Dials::overseer()` written as a complete
  literal instead of inheriting from the test fixture `Dials::full()`; (4)
  tripwire restorations, meaning `lift::enter()` over every phase transition,
  exhaustive matches replacing the two production `unreachable!()` sites, the
  danger-layout `PartialEq` derive, a `Reserve` enum replacing `Option<u32>`
  scrap guards, and the pure strength-formula dedup; (5) two defect fixes,
  meaning the zero-guard reserve bypass and the wedged-withdrawal target leak.
- Approved coordination types, each a hash-neutral commit confined to the
  player-facing path: a `Controller` enum carrying a `PlayerFacingMind`; a
  `ScrapLedger` with named `Hold` variants that preserves every reserve
  computation with its exact current inputs; a `ClaimLedger` with `Owner`
  selectors reproducing the exact current claim subsets, including the divergent
  lift predicate verbatim; and a `Speculation` helper replacing the two
  identical planner rollback blocks.
- Approved additional scope: the salvo fix (after the pure dedup, the shared
  strength formula gains the `weapon.salvo` multiplier and `cooldown.max(1)`
  guard, with a before/after evaluation delta recorded); collapsing the strategy
  air operation and its plan into one paired field, deleting the silent
  plan-substitution fallback; and three ride-alongs, meaning a named queue-depth
  constant in economy, binary-search consistency for the two remaining linear
  unit lookups, and a `UnitIdSet` newtype for the lift payload.
- Explicitly out of scope: reconciling the four known budget reserve
  disagreements (airworks tick mismatch, stale desperation read, three-way
  bootstrap divergence, all-claims versus retained-claims commitment), which are
  separate behavioral judgments to make one at a time after the ledger names
  them; unifying the drifted raid and team return-leg constants (raid literals
  get names, values stay); difficulty reaching lift execution; the
  utility-policy memory split; converting the 77 `player_facing` branch sites;
  container sweeps and constants modules.

## Findings

- The profile-free Overseer path leaves `Brain::act` before any coordination
  code runs: its planners are `None` and its core floor is zero, so the four
  coordination types are unreachable on the frozen path by construction.
- The 2,000-tick hash fixture is blind to combat-phase drift: it caught a
  harvest tie-break flip but missed a contact-radius change and reversed
  producer selection, because Overseer-versus-Overseer decisions land between
  roughly 5,600 and 36,000 ticks.
- Bot think work on shipped seven-seat maps measures 49-61 ms on shared
  cadence-12 think ticks, over the 50 ms tick budget at 20 Hz. The
  extractor-claim per-candidate BFS is about 58 percent of profile samples; the
  single-component-flood fix measured hash-identical across seven scenarios at
  6,000 ticks.
- The strength formula exists three times and has diverged: the intelligence
  copy multiplies by `weapon.salvo` and guards `cooldown_ticks.max(1)` while the
  executive and production copies do neither, undervaluing the Moth ground
  contribution about sixfold in hold and withdraw decisions.
- A `Some(0)` voluntary scrap guard silently disables the 70-scrap tech reserve
  at eleven construction gates, and one economy function defaults the same
  `Option` two different ways. The zero-disable is treated as an accidental
  bypass; the fix restores the ordinary floor.
- The lift Landing-to-Recover transition is the one phase transition missing its
  `phase_started_at` stamp; it is currently unobservable because Recover is
  terminal, so routing every transition through an `enter` helper is
  hash-neutral.
- `Dials::overseer()` inherits 32 fields from `Dials::full()`, which is
  documented as a focused-test fixture and constructed 61 times in tests;
  editing the fixture silently redefines the frozen yardstick.
- The brain re-derives lift claims from manifest fields with predicates that
  diverge from the lift planner reservations method. The claim ledger must
  reproduce the brain predicate verbatim and record the divergence in a comment;
  reconciling it is roadmap work.
- About fifteen brain test sites null individual planners on a scripted brain,
  so `PlayerFacingMind` keeps the four planner fields optional; the profile,
  intelligence, and public map briefing become non-optional.

## Actions

- [ ] Land the verification floor: the Overseer dials literal, the 6,000-tick
      Overseer horizon, and the player-facing hash fixture.
- [ ] Land the extractor-claim component-flood perf fix with both fixtures
      unmoved.
- [ ] Restore the lost tripwires: lift enter helper, exhaustive matches,
      danger-layout equality derive, Reserve enum, pure strength dedup, and the
      hash-neutral ride-alongs.
- [ ] Fix the reserve bypass, the wedged-withdrawal target leak, and the salvo
      divergence, reblessing only the player-facing golden with each movement
      explained.
- [ ] Introduce the four coordination types as one hash-neutral commit each:
      Controller, ScrapLedger, ClaimLedger, Speculation.
- [ ] Collapse the strategy air operation and plan into one paired field.
- [ ] Pass the full repository gate battery on the finished branch and reconcile
      this note.

## Open Questions

- Which maps and bot-config cells give the player-facing fixture the best
  coverage of the lift, team, and raid paths? The implementer should verify at
  least one lift phase transition occurs within the fixture horizon.
