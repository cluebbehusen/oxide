---
name: scripted-bot
description:
  Design, change, debug, and evaluate Oxide's fair rules-based opponent. Use for
  Brain, UtilityPolicy, Dials, Observation, Intent, Executive, SeatBot,
  BotConfig, scripted openings, economy, scouting, tech, combat, expansion, team
  conduct, liveness, bot replays, bot difficulty proposals, or whether a match
  looks credible and fun.
---

# Oxide scripted bot

Build a credible opponent under ordinary game rules. Automated metrics identify
failures and candidates; human play and replay review decide whether it is fun.

## Establish the owning contract

Read [Bot architecture](../../../docs/bot-architecture.md) for the maintained
controller, knowledge and ownership boundaries. For allocation, forecasting,
personality, opportunity scaling or cross-domain coordination, also read the
normative [Bot strategy](../../../docs/bot-strategy.md). Desired behavior is not
proof that the current implementation already provides it.

State the observable problem and the layer that owns it: observation, memory,
proposal derivation, admission, persistent execution, or command lowering.
Capture a deterministic scenario and seed before changing policy. For a
structural refactor, name the old mechanism being removed and preserve the
commands, state hashes, events and deterministic planning progress it produced.
A justified behavior correction instead needs a focused before/after regression;
historical whole-match hashes do not define correctness. Apply the repository's
hash/version rules using any existing session authorization.

The bot receives fog-honest observations and an immutable public briefing.
Authored starts, initial resources and terrain are priors, never current enemy
contacts or live resource amounts. Hidden QA state and replay diagnostics cannot
feed decisions. Add honest evidence with tests when needed; do not infer it from
an omniscient view.

Domains own evidence, ranking and tactics. Shared admission owns exact capital,
units, queues, workers and sites. Commit the accepted payload without reranking
it or reconstructing a second budget. Retained work, release and recovery must
remain explicit. Current scrap funds commands; forecast income proves future
feasibility only. Personality ranks legal choices and never grants or removes a
capability. Difficulty changes the documented cognitive limits, not game rules.

Policy lives in `oxide-bot`; authoritative observation filtering lives in
`oxide-sim`. Keep the dependency one-way. `Brain` consumes observations;
`SeatBot` adapts state on the existing host worker. Do not introduce simulation
mutation APIs for bot fixtures or serial observation preprocessing before
parallel seat execution.

## Choose evidence by boundary

Read only the relevant sections of
[Domain regression guide](references/domain-regressions.md). It identifies
cross-domain cases that small planner tests miss; it is not a checklist to run
in full for every edit.

- Pure-domain tests prove ranking, route, schedule and fixed-point boundaries.
- Coordinated-controller tests prove ownership transfer, funding, recovery and
  actual commands across domains. Use the maintained planner configuration.
- Long-horizon scenarios prove liveness and reconstruction over time; they do
  not replace the smaller regression explaining the failure.

Keep fixture builders, fault injection, deterministic work counters and small
independent exhaustive oracles when they prove a real contract. Test-only
execution modes, impossible planner states and bookkeeping with no production
consumer need a specific justification. Assert the production output rather than
maintaining a second result solely for tests. Do not ban `cfg(test)` or expose
invalid default observations to make fixtures easier.

## Measure architecture and execution together

Remove duplicate preparation before adding concurrency. Assess ownership, shared
inputs, scratch lifetime, invalidation and memory as well as function size. One
proposal should not require unrelated domains to recreate its claims.

For runtime parallelism, identify independent frozen inputs and worker-owned
scratch. Fix work allowances before dispatch and join results in canonical
order; completion order must not change choices, tie breaks or planning
progress. Admission and mutation stay ordered. Measure small and large workloads
plus nested-seat and concurrent-match contention. Preserve negative experiments.
Distinguish authoritative tick latency, controller latency, frame cost and batch
throughput; improvement in one does not prove improvement in the others.

Compare isolated repeated before/after runs without competing builds or
coverage. Investigate overhead above 10%, but report absolute costs and variance
rather than treating a percentage threshold as a verdict.

## Evaluate in layers

Start with the affected module and integration suite. Common controller seams:

```sh
cargo test -p oxide-bot --test bot_brain --locked
cargo test -p oxide-bot --lib utility::policy_tests --locked
cargo test -p oxide-bot --test scripted_bot --locked
cargo test -p oxide-bot --test bot_frames --locked
```

For battlefield or reconnaissance ownership, include `battlefield_adaptation`,
`recon_support` and the owning planner tests. Required gates and CI duplication
rules live in `AGENTS.md`.

Use the [evaluation procedure](references/evaluation.md) to select a match
matrix for the changed behavior. Cross-domain policy changes need broad
representative play; a local refactor starts with the affected contracts. Select
difficulties, stances and seeds that exercise the changed decisions. A lower
difficulty need not lose every paired match; its cognitive limits must remain
explainable and monotone.

Finish player-facing changes with replay review and the native QA path in
`oxide-live-qa`. Watch beyond the opening: economy, replacement, tech use,
scouting, legal reaction, failed-route recovery and meaningful late activity.
Report exact evidence, limitations and what still needs human judgment. Do not
claim credible play from a higher win rate or a green suite alone.
