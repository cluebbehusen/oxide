# es-1 — selection on the shipped artifact, gates inside the loop

Experiment: can weight-space evolution adapt the shipped Q12 champion
under the final 0.15.3+ rules without spending its personalities — the
trade two PPO campaigns (auto-3, auto-4) could not buy? Mutation is
integer perturbation on the artifact the sim loads; fitness is the
native neural-cup on paired seeds; the style gate sits inside the loop
as a per-generation trust region (a center step that worsens the
signature is rejected and the search narrows). No torch, no export
path: the thing being trained is the thing that ships.

- Command: `uv run es.py --name es-1 --hours 9` (tools/train), cups on
  a frozen 0.15.3 driver binary, style gates in an isolated target
  dir. Founder: the shipped champion (`ladder_weights.json`).
- The founder starts gate-RED under final rules: the drip restore
  collapsed Turtle-led development to a 16-16 tie with Balanced (the
  aggregate metric counts harvesters + development builds; Turtle's
  builds lead 11-6 underneath). Full bisection in the daily log.

Result: the run recovered the gate immediately and then climbed far
past the founder while holding every family.

- Generation 0's first accepted step took development from 0/7 to 7/7;
  all four families and the fun gate held at every confirm thereafter.
- Confirm suite (48 seeds x both seats x both opponents, 192 games):
  founder 113/192 (59%) -> 146 by g25 -> 161/192 (84%) by g50, held
  through g350. Center suite: 60/96 -> 82/96, last new high at g89.
- 483 generations: 123 accepted steps of 355 scored, 129 flat
  (mutation below the sim's decision granularity). The endgame is a
  limit cycle at the sigma floor — flat fitness widens the step, the
  next real step breaks a family and is rejected, the step narrows
  again. Selection polishes and defends; it does not discover.
- Incident: the loop's style gate compiles from the working tree while
  its cups run a frozen binary. Mid-run sim fixes (the review sweep)
  shifted the gate under a center whose clean record predated them,
  wedging the run on systematic rejections around g200. A stop/resume
  re-measures the center and unwedged it at g221 — under the NEW
  rules the center still held 7/7/7/7. Lesson: freeze the entire gate
  context for an overnight run, not just the fitness binary.

Reading: the auto-3/auto-4 failure mode is structurally absent when
the gate is part of acceptance — adaptation never had to spend the
personalities, and the +48-game climb says the style manifold has
plenty of room to improve inside it. The plateau is honest too: at
the boundary, minimal integer steps either change nothing or cross
it, so further capability (unused units, new behaviors) must come
from demonstration and gradient work, with selection as the last
mile. Battery-grade validation (factions, random maps, liveness,
ladder) deliberately deferred to the promotion pipeline.

Artifacts kept local: runs/es-1/ (journal, center checkpoints,
best.json — the 161/192 gate-green candidate). Nothing embedded;
lineage-checkpoints and artifact-lineage.md unchanged.
