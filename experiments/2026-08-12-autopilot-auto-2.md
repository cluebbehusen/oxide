# Autopilot auto-2 — the rebalance experiment that closed the loop

Experiment: does base-Array charge detection (0.15.2) give PPO a
value channel that re-teaches radar? Population 4, two generations of
60 updates, seeded from auto-1's champion checkpoint AND config
(--seed-config, g1m1's rusher-rich mix), fun gate as hard constraint,
failure-count-first fitness, on the rebalanced sim with the batched
collector.

- Command: `uv run autopilot.py --name auto-2 --initialize-from
  runs/auto-1/g1m1/pool/ckpt-02275.pt --seed-config
  runs/g1m1-config.json --population 4 --updates 60 --generations 2
  --cup-seeds 30` (workspace 0.15.2, commits 6581847/7137c8c/404745c
  + 74f7bdc).

Result: the rebalance worked and the campaign produced the fine-tune
era's FIRST full fun-gate passers.

- Generation 0: array reach jumped immediately where auto-1 had
  eroded monotonically (22.1% on the founder config vs the 6-15%
  slide before) — the policy re-adopts radar when detection pays.
  The failure-count-first fitness earned its keep visibly: the
  strongest cup (93) carried the worst gate profile and was ranked
  down; the gate-healthiest member led instead.
- Generation 1: TWO complete fun-gate passes. Champion g1m3
  (ckpt-02395): Overseer 83% mixed and 80/95/83/100 across ff/cc/fc/
  cf (the cf sweep is the campaign's first 60/60), rush canary 57%
  mixed, every composition floor green including arrays. Its config
  drifted toward more team games and fixed maps — the search found
  its own route to composition health.
- Per-profile rush structure improved over every prior artifact:
  fortress holds 10/20 (from 0/20 always), six personalities are
  perfect, and the vulnerability migrated to the air-opening family
  (air-combined, swarm, air-raider at 0/20) — committed air openings
  losing to a scripted expert ground all-in is characterful variance,
  recorded as the residual. Cupric-seat cells (cc 32%) carry more of
  the air-family deal; profile-mediated, not systemic.
- Profile battery passed 7/7/7/7 WITHOUT re-distillation — the
  personality columns survived two full-policy fine-tune campaigns.
- Ladder: the champion saturated the old Hard rung outright (40/40 at
  650 per mille); re-pinned from its own sweep to 800 per mille on
  the full-speed clock — 16/29/37/40 with strictly falling ticks.

Promoted 2026-08-12: digest 21d1018489498a8e, lineage
sha256:7c453c727ed8c2b13beee3a2d0a5c00a5137ea92575f4e9dde5849098b1bdc35,
float parent committed as lineage-checkpoints/auto2-g1m3.pt. From
Connor's rebalance directive to a promoted actor: one working day —
the cheap-training loop is real.
