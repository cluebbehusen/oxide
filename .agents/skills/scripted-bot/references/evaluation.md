# Evaluation procedure

Then run complete seeded matches on representative shapes: a normal duel, an
island or severed-ground map, a team map, and a long or grand map. Ask the
driver for its current syntax rather than copying stale flags:

```sh
cargo run -p oxide-driver -- run --help
cargo run -p oxide-driver -- replay-summary --help
```

The complete-match path is `run <scenario> --all-bots`; ordinary `--bots` honors
the scenario's configured chairs and therefore leaves its human chair under
human control. Add `--save-replay <path>` for review evidence.

Sample every difficulty and stance across the review set, plus multiple
personality seeds. Lower difficulty is not required to lose every paired match,
but its cognitive limits should remain visible and internally consistent.

Use `bot-eval` for reproducible player-facing profile cells. It stops when the
match decides, emits one compact JSONL row per leg, and can preserve the replay:

```sh
cargo run -p oxide-driver -- bot-eval skirmish \
  --difficulty prime --stance balanced \
  --scenario-seed-base 7000 --personality-seed-base 9000 \
  --paired --candidate candidate-a --out replays/bot-eval.jsonl \
  --replay-dir replays/bot-eval
```

When a compact row or replay shows suspicious behavior, capture the
player-facing controller's runtime decisions with
`--decision-trace-out replays/bot-eval-trace.jsonl`. The trace sidecar requires
`--out` and `--candidate`, joins each record to its exact evaluation leg, and
contains only fog-honest facts the current coordinator can state directly. The
schema is defined by the trace types and described in
[Bot architecture](../../../../docs/bot-architecture.md). It does not
reconstruct explanations from a replay or infer reasons from absent planner
output. Treat the sidecar as disposable diagnostic evidence and keep it out of
production commits.

For controlled current-controller comparisons, run a paired block across both
faction assignments and both map-end geometries:

```sh
cargo run -p oxide-driver -- bot-eval skirmish \
  --difficulty prime --stance balanced --opponent-difficulty standard --paired \
  --ticks 60000 --scenario-seeds 7000,7001 \
  --personality-seeds 9000,9001 --faction-cells fc,cf \
  --geometries authored,rot180 --candidate prime-standard-a \
  --out replays/prime-standard-a.jsonl \
  --replay-dir replays/prime-standard-a
```

`--paired` exchanges complete profiles while holding the transformed world and
faction rosters fixed. Cross independent `--scenario-seeds` and
`--personality-seeds`; simulation randomness and personality are separate
factors. Each personality value is the primary seat's seed, with ordinary
opponent seed assignment unless `--same-personality-seed` is selected. Use
`--runs N` for consecutive cells without explicit axes. Refuse nominal cells
that resolve to the same executable matchup.

Use `sweep`, `pace-sweep`, `sweep-factorial`, or `bench --scenario` to measure
one configured controller interacting with the simulation. Select
`--difficulty`, `--stance`, and `--personality-seed` explicitly for comparisons;
defaults are Standard/Balanced/zero. Keep the complete profile identical in
symmetric seats and fixed while varying simulation seeds. Preserve the exact
profile and simulation version in reports. Symmetric bot matchups do not isolate
engine or map fairness. Keep synthetic simulation benchmarks separate from bot
timing.

Use the per-unit stall breakdown to distinguish one blocked order from a broad
command failure. A leg ends as `termination: stall_loop` once one unit stalls
the same way `--stall-loop-limit` times (200 by default, 0 disables); that row
names the seat, unit, reason, count, and tick, and is an anomaly to inspect, not
a result. Treat rejections, stalls, and outcomes as diagnostic evidence, not a
quality score. Persisted evidence requires an explicit stable `--candidate`;
replay evidence also requires its JSONL `--out` sidecar. Rows record the
complete scenario and execution fingerprints, exact controller profiles, a
seed-independent command-stream hash, and the requested tick limit. Use repeated
command hashes to identify seed cells that generated the same play rather than
counting them as independent samples. The driver stages the whole invocation,
rolls back normal publication errors, and refuses to replace an existing JSONL
or replay. This is not a cross-path crash transaction: abrupt process
termination can leave hidden staging files or a partial replay set. Inspect and
remove the incomplete batch, then rerun it under a fresh candidate rather than
treating those files as complete evidence.

For each candidate, preserve the scenario, seed, replay, final hash, result,
duration, and a short behavioral verdict. Compare repeated identical runs for
exact hashes. Check that the controller:

- keeps an economy alive and replaces losses;
- builds and uses the reachable tech tree rather than merely owning it;
- scouts, reacts to discovered threats, and attacks through legal knowledge;
- escapes or changes plans after a failed route or site;
- behaves coherently after the opening and through the match's end;
- remains active on every seat, faction, and team shape in scope.

When paired results follow the physical seat, reduce the divergence to a
half-turned scenario before tuning policy. Compare authoritative state after
each relevant tick phase and audit equal-cost A* ties, footprint doorsteps,
production spawns, blocked group-goal snapping and spreading, signed fixed-point
vector scaling, and perfectly stacked collision separation. Outcome-relevant
tie-breaks belong in a query-, local-, or map-relative frame; an absolute
row-major or compass preference can turn a mechanical asymmetry into a false
personality or difficulty signal.

Use `replay-summary` to find long silences, nonsense loops, missed tech,
one-sided non-participation, and suspicious endings. Then watch the suspicious
and representative replays. Metrics are a triage tool; they do not certify
credible play.
