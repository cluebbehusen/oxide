---
name: simulation-performance
description:
  Measure, locate, and reduce Oxide's simulation cost per tick. Use for sim hot
  spots, tick profiling, benchmark workloads, before-and-after or cross-commit
  comparisons, proving that an optimization changes no outcome, and judging a
  performance change that alters behavior. Not for native frame timing or
  save/load transitions; use oxide-live-qa for those.
---

# Oxide simulation performance

Measure before changing anything, explain the cost before optimizing it, and
prove that an optimization changes nothing but cost. Most large wins come from
work the simulation repeats for nothing: rescans of every entity, searches whose
answer cannot have changed, and loops where units undo each other.

## Record the benchmark set

Work from a release build on macOS (`tick-profile` drives `sample`), and run the
built binary directly so no timing includes Cargo. Record the set once per
behavior change, then reuse it; it is gitignored.

```sh
cargo build --release --locked -p oxide-driver
driver=target/release/oxide-driver
mkdir -p replays/benchmark
for workload in duel skyhook mature-armies; do
  "$driver" bot-cost $workload --controller opponent --json \
    --save-replay replays/benchmark/$workload.json > replays/benchmark/$workload.cost.json
done
"$driver" bot-cost --scenario scenarios/compass-grand.json --ticks 20000 \
  --controller opponent --json --save-replay replays/benchmark/compass-grand.json \
  > replays/benchmark/compass-grand.cost.json
```

Each `.cost.json` holds the recording's `final_hash` and a `simulation` block
with average, p99 and maximum nanoseconds per tick. `driver bench --units 250`
and `--units 1000` add controller-free mass battles of 500 and 2000 units with
their own hash. Ask each command for `--help`; the CLI is canonical.

## Locate the cost

1. `driver tick-scan <replay>` ranks the costliest windows. Late, crowded
   windows and isolated spikes usually tell different stories; check both.
2. `driver tick-profile <replay> --from <tick> --ticks 50 --json` gives each
   function's inclusive and own share of the tick. `--focus <name>` breaks one
   function down. Rank by share, not time: shares hold steady on a busy machine.
3. When inlining hides the split, mark candidate functions `#[inline(never)]` in
   a throwaway build, or wrap a block in a local `#[inline(never)] fn`.
4. Explain the cost before fixing it. Count calls, outcomes and repeats with
   env-gated `eprintln!` in a throwaway build, filtered to the window's ticks,
   and summarize the lines with a short script. Ask how many calls run per tick,
   how many could have found anything, and whether the same unit repeats the
   same work tick after tick. Read the replay's command log before blaming the
   simulation: a controller may be ordering the pattern.
5. Remove only the instrumentation. Copy each file aside before instrumenting it
   and copy it back afterwards, so edits made before the instrumentation
   survive; never `git checkout` a file that also holds other work. Check the
   diff before the next measurement.

A cost that recurs per unit per tick with the same inputs is often a behavior
bug, such as units yielding to each other in a cycle, a route dropped and
replanned unchanged, or a retry with no backoff. Fixing the cause beats making
the repetition cheaper.

## Measure reliably

Other sessions load this machine, so wall time can swing by tens of percent.

- Count with `/usr/bin/time -l "$driver" ...`: instructions retired are nearly
  load-independent, and cycles elapsed follow real cost, including branch and
  cache effects that instruction counts miss. Report both when they disagree.
- To isolate one stretch's instructions, subtract two runs:
  `"$driver" replay <replay> --ticks <end>` minus the same with
  `--ticks <start>`. Instruction counts barely vary between runs, so the
  difference stays tight. For wall time over a stretch, use the window timings
  from `tick-scan` or `tick-profile` instead; subtracting wall times amplifies
  noise.
- Interleave candidate and control runs in three or more pairs, note `uptime`
  load, and report the pairs or their minimum, median and maximum, not only the
  best run. Never compare runs taken minutes apart under different load.
- Check every workload, small ones included. Fixed per-tick or per-snapshot
  overhead (a table sized to the map, an allocation per query) shows up first on
  the duel.
- Instruction counts cannot show a parallel gain; time those with interleaved
  wall and CPU totals (`/usr/bin/time -p`).

## Prove an optimization exact

An optimization that changes no outcome needs no version decision. Prove it on
every PR with evidence that does not rest on the final state alone:

- Compare candidate and control binaries tick by tick: run
  `replay <replay> --hash-every 1` with each and diff the outputs. Every line
  carries that tick's state hash and a running digest of every tick report, so a
  difference that later converges, or one that only changes events, still shows.
  This is the proof, at roughly five times a plain replay's cost.
- Screen first with a larger interval such as `--hash-every 100`, which costs
  little more than a plain replay. It samples states only every N ticks, so
  differing lines prove a divergence and bracket where it starts, but matching
  lines prove nothing: a short-lived state difference can fall between samples.
  A matching final hash (`--expect-hash`) is a quicker screen still.
- Compare `bench` hashes against the control binary, and run
  `cargo test -p oxide-driver --locked` for the fixtures and goldens.
- For a nontrivial shortcut, such as a pruned search, a cache or a changed loop
  structure, add a differential test that keeps the replaced logic in the test
  module as the reference and compares it with the fast path across a staged
  sweep: every tile around a range edge, random segments over random terrain, or
  a scenario run tick by tick. A hoisted invariant or an allocation reused
  across iterations needs only the tick-by-tick comparison.
- Break the shortcut on purpose (shrink a bound, drop a filter, skip a
  bookkeeping step) and confirm the differential test fails. If the sweep cannot
  reach a path, add a staged case that does.
- Prune only with conservative supersets: a filter may admit extra candidates,
  never drop one the exact test would accept. Add a margin wherever fixed-point
  rounding meets a bound, and keep the exact test on whatever the bound cannot
  settle, such as orientation tests near collinearity.
- A snapshot built once per phase (the brain phase's unit index and team
  presence survey, a danger snapshot) is valid only while its inputs hold.
  Before reusing one, confirm nothing moves, spawns or changes owner in between,
  and say why in a comment.
- Preserve order where order decides outcomes. Sorting a subset keeps relative
  order, and a minimum or count over a pure predicate is order-free, but a new
  sort key can reorder ties.

## Changes that alter behavior

Bring them to the user with data before opening a PR. If state-hash fixtures
move, follow the version and bless rules in `AGENTS.md`.

- Replaying one build's command log on another diverges; compare fresh
  recordings, one per build, made with `bot-cost --save-replay`.
- Divergent matches differ in unit counts and economy, so total cost alone can
  mislead. Measure the targeted pattern directly (calls, repeats, stalls) and
  check `driver replay-summary <replay> --every 5000 --minimaps none` for
  income, army size and stalls.
- One match is one sample. A scenario's `seed` may not change controller play at
  all; vary `personality_seed` in each seat's `bot_config` to get more matches,
  and check whether the case fires anywhere else before claiming a general
  effect.
- Keep the responsiveness that motivated the old behavior: a backoff or a
  committed choice can cost the economy what it saves the CPU.

## Compare against another commit

Build the other commit in a scratch worktree with its own target directory, so
neither checkout reuses the other's crates (`$SCRATCH` is any scratch
directory):

```sh
git worktree add --detach "$SCRATCH/wt-base" <commit>
(cd "$SCRATCH/wt-base" && CARGO_TARGET_DIR="$SCRATCH/base-target" \
  cargo build --release --locked -p oxide-driver)
cp "$SCRATCH/base-target/release/oxide-driver" "$SCRATCH/driver-base"
git worktree remove --force "$SCRATCH/wt-base"
```

Run both binaries interleaved on the same workloads. Older drivers may lack
`--save-replay`, `--hash-every`, `tick-scan` or `tick-profile`;
`bot-cost --json` and `bench` compare across a long span. Delete the scratch
target directories afterwards.

## Questions before bigger levers

Answer these with measurements on the current code, and record the results with
their workloads and builds in the PR or a note rather than here.

- Parallelism inside a tick: is each piece of parallel work well above the cost
  of waking threads and moving data between cores? Time the phase's per-tick
  work before adding workers. Seats and whole matches are coarser candidates.
- Build settings such as LTO or codegen units: does every hash still match, and
  is the gain across the whole benchmark set worth the slower build?
- A second, finer rejection test: does the check itself cost less than the work
  it skips? Measure its own share, not only the walk it avoids.
- A table sized to the map: does its rebuild scale with map area rather than
  units? Measure the smallest and largest maps, and bound it by occupancy if it
  does.

## Hygiene

Keep instrumentation, recordings and timing output out of commits; put numbers
in the PR description. Run `cargo clean` after heavy cycles, and remove scratch
worktrees and target directories when done. Re-record the benchmark set after
any behavior change merges.
