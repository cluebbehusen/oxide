---
name: oxide-review
description:
  Review Oxide code, pull requests, architecture proposals, or external review
  findings against current implementation and observable contracts. Use for
  requested reviews and review follow-ups, not routine implementation alone.
---

# Oxide review

Review the actual implementation and the user's requested scope. An external
review is a set of claims to evaluate, not permission to implement its advice.
Respect existing authorization for edits or publication; otherwise keep the
review read-only.

## Establish the baseline

Identify the checkout, commit, comparison base and relevant uncommitted work.
Distinguish what is on the PR from what exists only locally. Read the affected
crate README and relevant architecture sections; `docs/bot-strategy.md`
describes intended behavior, not proof of current behavior. Follow changed data
through its producer, owner, consumers and retirement rather than reviewing
isolated functions.

For a reported bug, recover the exact trigger and expected result. Inspect the
current path before accepting a reviewer's severity or assuming that a nearby
fix addressed it. Keep pre-existing defects separate from new regressions.

## Assess the changed boundary

Apply the
[architecture and Rust questions](../../../AGENTS.md#architecture-and-rust-quality)
to the affected design. Prefer a concrete removal or clearer owner over a new
adapter layer or a file split that leaves the same coupling.

- For Rust state models, examine required data, mutually exclusive states,
  visibility and borrowing. Flag a clone or abstraction for its demonstrated
  ownership/scaling cost, not merely its presence.
- For persistence, trace capture, validation, reconstruction and installation,
  including failure/cancellation and the lifetime of the previous session.
  Distinguish derivable answers from deterministic progress and readiness. Check
  size and expansion bounds when retained data changes.
- For performance or concurrency, identify the actual expensive phase and unit
  of parallelism. Same-seat work, seat parallelism and concurrent matches have
  different costs. Check frozen inputs, bounded work, private scratch and
  canonical collection; worker completion order cannot choose outcomes.
- For tests, identify the observable contract protected by each affected case.
  Keep useful fixtures, fault injection, work counters and independent oracles.
  Do not delete `cfg(test)` wholesale or mistake coverage percentage for proof.
  Prefer a production-path regression over another test-only execution mode.

## Verify proportionately

Choose the smallest probe that can establish or refute the claim, then cover
relevant interactions. Pure refactors preserve observable behavior and planning
progress; justified behavior corrections need an explicit before/after
regression. Historical autonomous-match outcomes are not the specification.
Follow hash/version authorization and local-versus-CI rules in `AGENTS.md`.

Use the owning domain skill for focused tests and `oxide-live-qa` for native
presentation or timing claims. Its
[performance procedure](../oxide-live-qa/references/performance.md) provides
scoped workloads; compare fixed inputs on the same machine and report absolute
cost, variance and limits. A replay reproduces recorded commands; it does not by
itself evaluate changed bot decisions.

## Report what the evidence supports

Separate demonstrated defects, structural concerns, untested hypotheses and
test-maintenance suggestions. Give actionable findings their trigger, impact,
source location and evidence; do not manufacture findings to fill a quota. For
follow-ups, state fixed, partial, open or unverified with the reason. A green
suite is supporting evidence, not proof that the reported case was exercised.

For cleanup proposals, name the mechanism to remove, owning boundary and
completion criterion. Report production, test and documentation deltas
separately when size is part of the claim. Be explicit about what remains
unmeasured or requires human play; do not infer good gameplay from win rate or
determinism.
