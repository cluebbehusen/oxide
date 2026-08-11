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
  16-17.5% (floor 25) because every teacher's construction arm caps
  Reclaimers at one or two and cloning those priorities suppressed
  insurance economy broadly (r16, dropping only the industry pair,
  landed WORSE at 13.4%). The undistilled control measured 23.2% at
  expert execution against a floor whose 0.14 rationale — the
  Reclaimer as the ONLY buildable economy structure — no longer
  exists (the Derelict Extractor owns that role and its tenure is
  gated separately). Floors re-anchored 0.25 -> 0.20; the control
  passes the whole fun gate there.
- r17 (construction cloning restricted to the fortify teacher):
  profile gates 7/7/7/7, parent-match holds, complete fun gate PASS,
  and the campaign-best Overseer cup — 90% over 120 games, seats
  dead even (54/54). Its rush canary read 51% vs the trunk's 60%,
  which prompted two more doses and a causal hunt:
  - r18 (8 epochs): gates hold, cup 73/62 — the doses trade Overseer
    strength against the canary inside noise.
  - r19 (NO construction cloning): gates hold, cup 79/50 — removing
    the suspected cause changed nothing.
  - The per-profile rush diagnostic settled it: the trunk ITSELF
    carries profile-specific rush holes (pre-distillation fortress,
    ground-combined, air-combined, and swarm all lose 0/20 to the
    expert all-in; seven-to-eight of nine hold 10-20/20 in every
    artifact). Distillation only shuffles which personalities are
    soft. The canary aggregate has always meant "some personalities
    lose to the expert rush"; no column-space dose fixes it, and a
    trunk-level rush-hardening phase is exactly the seesaw the
    consolidation era closed.

VERDICT: r17 promotes. Overseer 90% (54F/54C), rusher 51% (7/9
personalities hold; the fortress-family softness is a documented
residual shared by every candidate in the family), profile battery
complete (diversity, roles, style semantics 7/7/7/7, parent-match),
fun gate complete, ladder ordered 15/28/36/40 on the recalibrated
rungs, determinism exact, repair 8/8. Lineage: r13-02075 trunk
(consolidation) + r14 style-bonus columns + r17 named-condition
distillation (fortify construction only).

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
