---
name: oxide-opponent
description:
  Build and evaluate oxide-opponent, Oxide's reactive best-effort opponent
  controller. Use for any work in the oxide-opponent crate or its host
  integration, its tests and staged scenarios, its CPU and line reports, its
  evaluation, and its difficulty, stance and personality mapping. Not for
  oxide-bot; use scripted-bot for that.
---

# oxide-opponent

Read [the specification](../../../docs/oxide-opponent.md) first. It is normative
for this crate. `docs/bot-strategy.md`, `docs/bot/` and the scripted-bot skill
describe `oxide-bot` and do not apply here; do not import their
exact-allocation, forecasting or planning-progress requirements.

## Rules

1. **Invariants come first:** fairness, fog honesty, determinism, and saving and
   resuming this bot's own games.
2. **Budgets are constraints.** Improve play within the CPU budget. When a
   single decision is too slow, simplify or drop the expensive feature; when
   total CPU is too high, decide less often. Immutable map preprocessing and
   scratch reused within one decision are fine. Retaining derived answers across
   changing observations (a cache with invalidation) needs Connor's approval.
3. **Follow the specification's limits.** Precise about now, approximate about
   the future; one running total per decision; no mission owns future
   production; only the allowed computation. A new checkpoint field needs a
   design review. A change that adds reservations, proofs or planning state
   across decisions needs Connor's approval; stop and ask rather than adding
   one.
4. **Planning never spans decisions.** Missions and their phases may. Background
   decision execution stays as it is.
5. **Measure performance** for every PR: average and p99 time per decision and
   total CPU on the defined workloads. The bots-to-simulation ratio is a
   diagnostic, not a gate.
6. **Behavior may change.** Simplifying code or improving play justifies a
   behavior change. Fixtures driven by this bot live in their own file, separate
   from simulation-only hashes. Rebless them yourself after checking the
   smoke-matrix comparison. Simulation rule changes still need Connor's
   approval.
7. **Tests check what the bot does.** Every reactive behavior gets a
   command-level acceptance test in a staged scenario. Focused tests of memory,
   ranking, geometry, symmetry and bookkeeping are welcome. Do not pin tuning
   constants, incidental ordering or intermediate plans. Fairness, determinism
   and save-resume are always tested.
8. **Size is reported, not gated.** Measure every PR's net production and test
   line change, including code this bot adds anywhere (kit, simulation, driver).
   Growth past about 15,000 production lines triggers a design review, not a
   failure.
9. **Guards need evidence:** a failure seen in evaluation, a replay, a playtest
   or prior evaluation evidence, or one directly demonstrable. All work is
   bounded: finite inputs and a bounded number of visits or expansions.
10. **Parallelism only where measured.** Seats already run in parallel.
11. **Plain Rust.** Plain functions and data over traits and generic frameworks;
    enums for mutually exclusive states; no abstraction without two real
    callers.
12. **Docs describe behavior and boundaries, not algorithms.**

## Run it

- **Shell:** Settings > **Opponent AI: New** seats `oxide-opponent` in every bot
  seat of the next New Match; **Classic** keeps `oxide-bot`. Rematches, saves
  and replays keep the choice recorded in their scenario.
- **Evaluation:**
  `cargo run --release -p oxide-driver -- bot-eval skirmish --controller opponent --opponent-controller scripted --ticks 6000`
  duels this bot against `oxide-bot`; `--controller` sets every seat and
  `--opponent-controller` overrides seat one. Add `--paired` for a second leg
  with the two configurations exchanged. The stub never attacks, so
  opponent-only runs end at `--ticks`.
- **Traces:** `--decision-trace-out <file>`, with `--out` and `--candidate`,
  writes one JSONL row per decision. Opponent seats' rows carry this crate's
  `Trace`: tick, player, bank, spent, purchases and unit-order count.

## Report at handoff

When handing off a PR, tell Connor its net production and test line change,
average and p99 time per decision with total CPU, and the smoke-matrix
comparison, including when they are unfavorable. Keep them out of the PR
description.

Run the smoke matrix locally before handoff; CI does not run it:

```sh
cargo run --release -p oxide-driver -- bot-matrix driver/evaluation/smoke.json --out <new directory>
```

It plays Skirmish, The Deep Cut and Severance at Standard and Prime, Balanced
and Aggressive, three seed runs each. Each cell is a head-to-head pair, this bot
in seat zero and then seat one with one personality seed on both sides, plus one
`oxide-bot` mirror leg. Mirror rows are cached per user under the reference
digest of the `bot/`, `sim/` and `chassis/` sources, the `kit` code that hosts
`oxide-bot`, and `Cargo.lock` (`--baseline-cache` moves the cache), so they
rerun only when those inputs, a map, a seed or the tick limit change.
`driver/evaluation/duels.json` is the full two-seat matrix.
`bot-matrix-report <rows.jsonl>...` re-reads published rows; `--json` prints the
same report as JSON.

Read the report overall and by difficulty, stance and map family:

- **Pairs**: this bot wins both legs, split, `oxide-bot` wins both, or undecided
  (at least one leg without a winner).
- **New share**: this bot's share of head-to-head legs that had a winner, with a
  95% Wilson interval. Small matrices give wide intervals; compare runs, not
  single cells.
- **Decided new-old** against **decided old-old**: head-to-head legs should
  decide at least as often as the mirror.
- **Failure incidents** and **income**, per controller with seat-legs for scale.
  `oxide-bot` numbers are the reference, not a target.

## Failure detectors

Evaluation rows carry omniscient QA detectors, checked every 12 ticks for every
controlled seat. They never reach a controller.

- **Repeated orders:** one unit stalls with the same reason 5 times within 1,200
  ticks. The episode ends after a full window without that stall. Danger holds
  are exempt: a harvest line waiting out danger re-reports every 100 ticks by
  design. They still count in the row's stall evidence.
- **Abandoned sites:** a paid, visible, unbuilt base-tier site makes no
  construction progress for 1,200 ticks.
- **Starved production:** every built producer of the seat stays idle for 1,200
  ticks while the bank, less scrap this bot protects for a saving target, covers
  the cheapest unit any of them may legally train. Queueing anything ends the
  episode. The trace does not report protected scrap yet, so the whole bank
  counts. Rows also list each producer's idle, affordable ticks as a diagnostic,
  not an incident.

Income compares scrap earned in the minute before ticks 6,000, 12,000 and 24,000
(deliveries plus Reclaimer, Extractor and Foundry credits) with a saturation
estimate: two Harvesters on each of the four nearest scrap nodes that still hold
scrap for every completed Foundry, at their straight-line round trip, plus those
credits. No node counts for two Foundries.

## Review play

Use `replay-summary` and the native shell through the oxide-live-qa skill. Give
tick ranges for notable moments. Automated metrics find candidates and failures;
human play and replay review decide whether the bot is credible and fun.
