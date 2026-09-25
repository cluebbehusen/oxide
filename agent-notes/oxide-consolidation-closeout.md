---
created: 2026-09-25T12:00:12
updated: 2026-09-25T12:00:12
---

# Oxide consolidation closeout

September 25, 2026. Implementation reviewed through `a9bcc697`.

## Outcome and stopping point

End the broad consolidation workstream and resume careful feature development.
The game has clearer ownership, stronger behavioral tests and measured
performance improvements. This does not certify the entire codebase as clean.
Several refactors added explicit state and regression coverage; the overall
outcome must not be described as a wholesale reduction in code.

Bot policy now lives in `oxide-bot`, outside the deterministic simulation. The
host owns bounded parallel seat execution and collects commands canonically.
Background decisions between live ticks improve frame headroom; a late batch
joins at its required tick. Saves read the retained pre-decision controllers,
without waiting for or persisting speculative work.

Shared allocation owns exact resources and producer schedules. Selected owner
updates validate before application; speculative rejection no longer restores
whole planner snapshots and erases observation or independent maintenance.
Connected procurement retains actual investments and deadlines without treating
old unpaid quotes as permanent debt. Army missions live with their armies;
experience has one owner, and required objective baselines are represented in
types. Navigation preparation, work budgets and cache lifetimes are explicit.

Tests increasingly stage prerequisites and exercise production admission,
rejection, ownership and recovery. Useful independent oracles, fault injection
and work counters remain. Coverage percentages and deterministic hashes support
these contracts; neither demonstrates enjoyable play or good architecture.

Player saves are compact checkpoints with independently readable metadata.
Completed navigation fields reconstruct from retained recipes and knowledge;
unfinished progress remains exact. One bounded persistence worker performs
encoding, restoration and session retirement. Native close, cancellation,
failed-save retry and session preservation were verified, including loads and
saves already in flight. Full errors remain in logs while dialogs show concise
player-facing reasons.

## Measured results and limits

These are historical release measurements on an Apple M4 Mac with 16 GiB RAM,
using a 1280×800 native window and isolated workloads. They are not lower-spec
hardware guarantees or GPU timings.

- Early dispatch, compared with `2d2ce517`: late Grand and Skyhook due-frame p99
  fell from 15.2/13.6 ms to 2.1/2.4 ms at normal speed. Three alternating pairs
  covered 480 ticks per window; the tails had few due-decision samples. At 64×,
  catch-up left little dispatch lead and frames still exceeded 16.7 ms.
- Compact persistence, compared with `3adb1afc`: a seven-bot Skyhook save fell
  from 113 MB to about 135 KB. Warm save/load operations took approximately
  28/110–129 ms elapsed, with measured frame work around 1 ms. A sixteen-save
  catalog populated in approximately 6–15 ms. One earlier 90.7 ms transition
  outlier was not explained; later trials did not reproduce it.
- Recovery admission, compared with `26f7931e`: inspecting each retained journal
  once removed repeated validation under the storage lock. In ten matched
  load/save cycles, immediate saves changed from seven roughly one-second waits
  to 23–34 ms. Immediate exit can still wait about 0.9 seconds for recovery
  initialization. A successful player save does not guarantee that the recovery
  journal finishes its clean-close marker before the bounded exit wait expires.

Do not credit fewer allocations, faster batch evaluation or different bot
histories as a native frame improvement. Lower-spec qualification and human
judgment of current opponent behavior remain separate work.

## Decisions worth retaining

A command-panel cache was rejected: roughly 3–17 microseconds of ordinary model
work did not justify its added invalidation contract. A team-vision pool was
also rejected: a representative four-team case became slower, while synthetic
gains largely disappeared under concurrent-match contention. Revisit either only
with new representative evidence. Ordered within-seat admission still shares
claims, attention and budgets; more threads are not automatically an
improvement.

## Remaining work and entry points

Deferred structural work is bounded in
[planner responsibilities](https://linear.app/cluebbehusen/issue/CL-46),
[allocation interfaces](https://linear.app/cluebbehusen/issue/CL-47), and
[lift lifecycle](https://linear.app/cluebbehusen/issue/CL-48). These are not
prerequisites for ordinary feature work.
[Within-bot parallel planning](https://linear.app/cluebbehusen/issue/CL-40)
remains a measured investigation, not a promised speedup.

Use [AGENTS.md](../AGENTS.md) for invariants and design principles,
[bot architecture](../docs/bot-architecture.md) for ownership,
[review guidance](../.agents/skills/oxide-review/SKILL.md) for evidence
standards, and the
[performance procedure](../.agents/skills/oxide-live-qa/references/performance.md)
for fresh-checkout workloads and portable safeguards.

The retained
[late Skyhook checkpoint](../driver/tests/fixtures/performance/skyhook-late.oxsave)
is a profiling input, not a compatibility golden: tick 21839, seven bots, 471
units and 300 buildings. Copy it into an isolated QA profile's saves folder and
load through the native shelf following that procedure. It loads paused at hash
`0xe82c1af44f4fbe07` and occupies 134,842 bytes. It was produced by the
compact-save writer from the historical qualification world after local recipe
conversion; it is not a legacy-import example. Intentional format changes may
replace it with a freshly generated representative checkpoint.
