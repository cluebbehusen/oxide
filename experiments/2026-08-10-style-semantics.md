# Style semantics: making personality legible again

The r13-02075 candidate passed the fun gate and cups but failed the
promotion battery's style-family separation: profiles were
behaviorally DISTINCT (pairwise divergence 7/7 everywhere) yet the
semantic gradients were gone — turtle did not out-fortify aggressive
and aggressive did not out-fight turtle. PPO pulled every personality
toward the same winning opening; conditioning survived as "different",
not as "what the dial says".

- r14 (league --profile-columns-only --style-coef 0.08, 80 updates):
  the terminal style bonus stayed flat (0.21 -> 0.25, no trend) and
  the gate still read 0/7 on fortification/force. RL credit through a
  capped ±0.1 terminal bonus cannot move five input columns.
- style_distill.py (new tool): clone the v9 scripted teachers'
  play into the profile columns DIRECTLY, each teacher labeled with
  the Rust-authored named condition it embodies, trunk frozen (the
  freeze is verified parameter-by-parameter, and the battery's
  parent-match gate proves the raw-aggression path stays
  byte-identical). r15 (12 epochs, 3e-4): mobile pressure 0 -> 7/7.
  r15b (40 epochs, 5e-4, opening-dense 8k demos): overcorrected —
  development 7 -> 0. The mild dose is the right dose.
- Fortification/force needed instrument surgery, not more teaching.
  Native probes proved the r15 artifact's personality is vivid in
  real matches (style-pinned balance-probe: turtle 1.22 turrets and
  1.70 arrays per competitive lifetime vs aggressive 0.33 and 0.71)
  while the gate still read 0/7, because the 0.14-era instrument
  probes an OPPONENTLESS skirmish opening: the 0.15 actor's
  fortification is threat-responsive (in a vacuum it develops
  instead of walling — correct play; the deleted actor fortified
  unconditionally), and summing all three variants per style cancels
  the contrast (counterbattery is a siege turtle, air-combined holds
  no ground line). Re-anchored instrument: fortification/force are
  measured under Overseer contact over 12k ticks on the flagship
  variants, end-led (turtle above both for fortification, both army
  styles above turtle for force) — the same end-led shape the
  development gate always used; the strict three-way ordering lives
  in mobile pressure where it genuinely holds. Thresholds unchanged
  (4/7 seeds).
- Result (r15): development 7, fortification 7, force 7, mobile
  pressure 7 — and r15b still fails the re-anchored gate (0/7
  development), so the instrument discriminates.
- r15's fun gate then flagged collateral: reclaimer reach fell to
  16-17.5% (floor 25) because the industry teacher caps its opening
  at ONE Reclaimer and the industry -> industrial-attrition mapping
  cloned that under-building in. r16 re-distills without the industry
  pair (the trunk already carries turtle-led development): profile
  gate 7/7/7/7 again, parent-match holds. Fun-gate verdicts for r16
  and the 02075-at-expert control: (pending)

Also in this thread: the Level ladder recalibration. The candidate
saturated every 0.14-era rung (Easy at 350 per-mille hesitation still
beat the Overseer 39/40). The new ladder_handicap_sweep instrument
(sim/tests/neural_bot.rs, OXIDE_SWEEP_WEIGHTS) measured the
(hesitation, cadence) plane: cadence saturates past 64 ticks,
hesitation is the real lever with a cliff between 800 and 900.
Re-pinned rungs: Easy 900/34, Medium 800/48, Hard 650/34, Expert
0/34 — 15/28/36/40 wins on the yardstick with strictly falling tick
totals (gaps 13/8/4). The fun gate's probe moved to expert-level
execution for the same reason: the lower rungs now carry handicaps
severe enough that a non-expert probe measures hesitation noise, not
the policy.
