# Autopilot auto-4 — both gates enforced, and they pull apart

Experiment: with the style-signature gate as a second hard constraint
and the widened curriculum (island maps, the team4 kind, deeper
120-update members), can the population find an actor that adapts to
the 0.15.3 economy AND keeps its personalities? Population 4, three
generations, founded from the shipped champion's parent.

- Command: `uv run autopilot.py --name auto-4 --initialize-from
  lineage-checkpoints/auto2-g1m3.pt --seed-config runs/auto4-seed.json
  --population 4 --updates 120 --generations 3 --cup-seeds 30`
  (workspace 0.15.3, commits 52af198/6ba75e9/d10c388).
- First launch aborted in generation 1: a team job that drew 4v4
  broke the parent's lane accounting. Root cause and fix in d10c388 —
  the shape became the KIND (team4), lane geometry a pure function of
  the layout. The style gate's parser also learned to read stderr,
  where the test harness routes --nocapture output under pipes.
  Clean relaunch; the aborted run is archived locally.

Result: NO member passed both gates across all twelve, and the
failure pattern is the finding — the gates partition the population.

- Fun-gate passers eroded styles: g2m2 (fun PASS, Overseer 60/60)
  and g2m3 both lost development AND force to 0/7.
- Style keepers failed fun floors: g0m0 (style PASS, all four
  families) and g2m1 missed fabricator/array/reclaimer reach.
- Depth amplified drift: 120-update members lost up to three
  families (g0m3: development, fortification, force) where auto-3's
  60-update members lost two.
- Strength was never the bottleneck: g1m0 posted the fine-tune era's
  best cup (Overseer 60/60, rusher 43/60) while failing both gates.
- The search leaned INTO the new curriculum where it could: winning
  perturbations pushed team4 up (0.13 -> 0.18) while island drifted
  slightly down (0.10 -> 0.08). Confounded with everything else, but
  the 4v4 lanes did not price themselves out.
- The autopilot's honest exit: "no candidate passed the fun gate —
  review WARN lines before continuing," best fitness
  (False, -2, 86, 48) on g0m0. Nothing promotable; the incumbent
  ships on, unchanged.

Reading: fine-tuning under a large economy delta forces a CHOICE
between adaptation and personality because nothing in the training
loop defends personality — the constraint only selects after the
damage. The lever that changes the tradeoff is a style anchor inside
league training itself: a KL term toward the founder on
profile-conditioned decisions, so named-condition behavior stays
tethered while everything else adapts. Selection then works on
members that never had to spend their personalities to buy reach.
Post-hoc column distillation remains insufficient (proved in auto-3:
the eroded signatures are trunk-expressed).

Artifacts kept local: runs/auto-4/ and runs/auto-4-aborted/
(journals, members, batteries), runs/auto4-seed.json. Nothing
embedded; lineage-checkpoints and artifact-lineage.md unchanged.
