# Autopilot auto-3 — the drip-removal retrain that found the fitness gap

Experiment: can a 60-update-per-generation population fine-tune adapt
the shipped auto-2 champion to the 0.15.3 economy (Foundry drip
removed, Foundry/Crucible/Tender/Bastion price cuts)? Population 4,
three generations, seeded from the champion's own checkpoint AND
config, fun gate as hard constraint, failure-count-first fitness.

- Command: `uv run autopilot.py --name auto-3 --initialize-from
  lineage-checkpoints/auto2-g1m3.pt --seed-config
  runs/g1m3-config.json --population 4 --updates 60 --generations 3
  --cup-seeds 30` (workspace 0.15.3, commit b367fbe).

Result: the search worked, the battery caught trunk-level style
erosion, and NO ACTOR WAS PROMOTED. The incumbent ships on.

- The founder config failed the 0.15.3 fun gate outright when
  retrained (g0m0/g0m1: value-entropy spam floors, array reach,
  industrial-attrition mix entropy). Removing the drip changed what
  fun costs; the incumbent's recipe no longer reproduces itself.
- The search recovered inside generation 0: g0m3's perturbation
  (grand 0.368, ffa 0.127, rusher trimmed to 0.186) passed the gate
  at Overseer 54/60. Generations 1 and 2 never beat it — 7 of 12
  members failed gates under the tighter economy. Global-best
  tracking kept g0m3 (fitness (True, 0, 85, 54) vs the auto-2
  champion's (True, 0, 84, 50)).
- g0m3's driver battery was strong: mixed cup 54/60 (90%, up from
  83%), factions 57/50/54/51 (a tighter band than 80-100), repair
  8/8, determinism exact, ladder landscape fitting the shipped pins
  unchanged (20/29/37/40, strictly falling ticks).
- The profile battery then caught what the fitness cannot see:
  development and force style signatures at 0/7 (fortification and
  mobile pressure intact). Diagnosis matrix: the incumbent still
  scores 7/7/7/7 under 0.15.3 (the rules change is innocent), and
  every auto-3 gate-passer eroded (g1m1 development+force, g2m0
  fortification+force). The 0.15.3 economy delta is the largest the
  fine-tune era has trained across, and the trunk drifted with it —
  auto-1/auto-2's column survival was luck, not a property.
- Style re-distillation (style_distill.py, columns-only, freeze
  verified) did NOT repair it: development and force stayed 0/7 —
  both signatures are trunk-expressed, exactly the case columns
  cannot reach — and the distilled columns cost cup strength
  (mixed 54->44, ff 57->48). The distilled candidate is rejected.
- The incumbent re-measured under 0.15.3: 51/60 (85%) mixed, rusher
  24/60, profiles 7/7/7/7, liveness green on all 31 maps. The
  shipped actor is healthy under the new rules; promotion pressure
  is zero.

Verdict: promotion refused per the battery contract. The durable fix
is a fitness gap repair, not more search: the autopilot's hard
constraints see the fun gate but not the profile battery, so it
optimizes straight through personality identity. Auto-4 needs the
style-signature battery (or a cheap proxy of it) as a second hard
constraint before any member can win a generation — then the
population pays for trunk drift the moment it happens, instead of at
promotion time.

Artifacts kept local: runs/auto-3/ (journal, members, batteries),
runs/auto-3/g0m3-distilled.pt and candidate-0.15.3.json (the rejected
distillation), runs/g1m3-config.json (seed). Nothing embedded;
lineage-checkpoints and artifact-lineage.md unchanged.
