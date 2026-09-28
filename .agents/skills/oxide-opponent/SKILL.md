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
  `Trace`: tick, player, bank, received own events, spent, purchases and
  unit-order count.

## Report at handoff

When handing off a PR, tell Connor its net production and test line change,
average and p99 time per decision with total CPU, and the smoke-matrix
comparison, including when they are unfavorable. Keep them out of the PR
description.

- **Lines:** `uv run tools/line_report.py <base>` prints the net production and
  test line change per top-level directory from the merge base; `--help` defines
  what counts. `--working-tree` includes uncommitted files.
- **CPU:**
  `cargo run --release --locked -p oxide-driver -- bot-cost <workload> --controller opponent`
  for each of `duel`, `skyhook` and `mature-armies` reports average and p99 time
  per decision and total CPU per seat and controller, with the observation build
  beside them. Run it on the branch and on its base, on one machine with no
  competing builds or benchmarks. `--controller scripted` gives `oxide-bot`'s
  figures for reference; `--json` gives the same report as data.

## Review play

Use `replay-summary` and the native shell through the oxide-live-qa skill. Give
tick ranges for notable moments. Automated metrics find candidates and failures;
human play and replay review decide whether the bot is credible and fun.
