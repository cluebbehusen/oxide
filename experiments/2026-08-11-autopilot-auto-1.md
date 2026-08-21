# Autopilot auto-1 — first population-based fine-tune campaign

Experiment: validate the training autopilot end-to-end while
producing the fine-tuned actor that lets the 0.15.1 harvester rule
promote. Population 4, three generations of 60 league updates each,
founders initialized from the promoted actor's float parent
(lineage-checkpoints/r17-distilled.pt), anchor prior-v9, cup 30
seeds, fun gate as the hard constraint.

- Command: `uv run autopilot.py --name auto-1 --initialize-from
  lineage-checkpoints/r17-distilled.pt --population 4 --updates 60
  --generations 3 --cup-seeds 30` (workspace 0.15.1, commit b79da2e;
  crash-resume from a47539d exercised twice for real — a cargo clean
  deleted the driver mid-run and a power outage shut the machine
  down — both relaunches reused every completed phase).

Result: the generations traced a clean dose-response curve for
fine-tuning under a changed sim rule.

- Generation 0 (60 updates): the adaptation dip — every member
  failed 2-4 gate floors while cup strength already exceeded the raw
  actor under the new rule (74 -> 86). Config signal was immediate
  and matched the r6-r8 era's lesson: starving the rusher share
  (m1, 0.145) cratered composition (reclaimer reach 8.4%); the
  rusher-rich perturbation (m2, 0.2516) held it best.
- Generation 1 (120 cumulative): the sweet spot. The rusher-rich
  lineage's g1m1 reached cup 90/120 (Overseer 51/60, rusher 39/60 —
  65% rush defense under the new rule) with a SINGLE floor short:
  array reach 14.7% vs 25%. Fabricator and reclaimer floors
  recovered on their own.
- Generation 2 (180 cumulative): the overshoot. Arrays eroded
  monotonically in every lineage (6-11%) and lower-quartile spam
  re-emerged in two members (p25 1.18) — continued fine-tuning past
  ~120 updates narrows the policy, the r-series seesaw reproduced
  inside one campaign.

Selection behaved correctly both generations without the
failure-count tiebreak (the cup and gate health happened to
correlate); the tiebreak plus battery-score persistence are patched
in for future runs regardless. Two autopilot lessons queued: track
the global best across generations (the final generation was the
worst; the campaign product was mid-run), and make the update dose a
searched knob.

Follow-up (array-recovery): arrays are the one habit fine-tuning
sheds rather than relearns — the same habit that originally required
the Array rebalance plus a 300-update consolidation to appear. From
g1m1's checkpoint: 60 updates under its own winning mix with the
transitional structure and reclaimer seeds (--structure-bonus 0.02
--reclaimer-bonus 0.02), then the UNSHAPED battery decides — only a
habit that survives without the crutch counts.

Result: the recovery FAILED, and the failure is the finding. Arrays
fell further (14.7% -> 8.2%) despite the seed — during the shaped
phase itself the policy built only ~2 arrays per update against the
r10 era's 3.5-5.4 under the same bonus — and the extra 60 updates
(180 cumulative, generation-2 territory) re-opened the spam floors
and dropped the cups (Overseer 85% -> 67.5%, rusher 65% -> 39%).
Meanwhile the shipped r17 still EXPRESSES arrays above the floor
under the new rule when evaluated statically. Synthesis: PPO is not
forgetting the habit, it is measuring it — the 0.15.1 harvester
replan stagger reduced blip-driven rerouting, which was a large part
of the Array's practical value to a working economy, so optimization
now sheds radar as genuinely underpaying. The same razor that
rebalanced the Array in the first place applies: this is a balance
question (revalue the mast's remaining products, or accept lower
reach as correct play under the new rule) and a design call, not a
knob for search or a floor to shave a third time.

Campaign disposition: g1m1 (runs/auto-1/g1m1/pool/ckpt-02275.pt,
export candidate available) stands as the measured best fine-tune —
cup-dominant over the shipped actor under the new rule with a single
floor short — preserved for whichever design resolution is chosen.
The autopilot itself is validated: it reproduced the campaign's
hand-won lessons (rusher-share protects composition, the fine-tune
dose curve) autonomously in one overnight-scale run, surviving a
deleted binary and a power outage on its crash-resume.
