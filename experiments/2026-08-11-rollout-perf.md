# Rollout performance: measure first, then cut where the time is

Experiment: before building the planned gym-wire batching, instrument
the whole rollout path and let the split pick the target.

- Instrumentation (8018a2b): driver gym server wall-clock counters
  (state ticking, in-process Overseer thinking, reply encoding, pipe,
  resets) behind a `stats` request; client-side wire counters in
  oxide_gym.py; gym_bench.py prints the merged split for
  league-identical episodes.
- Verdict on the wire: acquitted. Grand 4v4 (gatework-array), the
  league-representative case: 90-92% sim, ~2-8% wire. Skirmish steps
  ~92k ticks/s through the same JSON protocol while the grand map
  managed ~4.4k — map scale, not serialization. Wire batching
  cancelled.
- Sampling profile (macOS `sample`, line-tables release) of the grand
  workload: 94% of tick inside `brain::run`, 78% of the whole tick in
  `economy::deliver -> approach_safe_rect -> known_rect_route` — the
  harvester delivery pathfinding — plus 15% in the harvest arm and,
  at depth, the Overseer's `island_target` running one full-map BFS
  per known enemy building per think.

Fixes, all required to hold every hash fixture bit-identical (pure
perf: same commands, same states):

1. `known_rect_route` now prunes doorstep candidates with
   `last_route_reachability` after an exhausted flood — the identical
   pruning `source_route_avoiding_danger` already had; the delivery
   arm simply never received it and re-flooded up to the 20k expansion
   cap once per candidate.
2. `GroundSalvageDanger` stamps an incident-proximity grid at capture
   so `route_safe_from`'s origin-dependent incident rule (uncacheable
   per tile) short-circuits on the vast majority of expansions that
   sit near no incident at all.
3. `island_target` floods home's known-road component ONCE and
   answers every enemy site by membership. The per-site BFS it
   replaces proved a road exists to every known building separately —
   on any connected map that meant re-walking the same component once
   per site, per think, per Overseer seat.

Result: A/B on identical work (same seeds; both runs simulated
exactly 66,855 ticks and 3,730 decisions — behavior identity
confirmed at the workload level, and `hashes` passes before and
after): 982 -> 2,567 ticks/s on 30k-tick grand 4v4 episodes, a 2.6x.
State ticking 39.7s -> 19.1s; Overseer thinking 26.7s -> 5.3s (5x).
Post-fix profile for the next round, each now a modest slice:
harvest-arm `source_route_avoiding_danger` ~18% of samples, idle-army
`combat::acquire_target`/`building_apparent` scans ~11%, residual
delivery routing ~6%.
