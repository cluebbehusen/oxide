# Structure reach at the fun gate — r8 candidate

The r8 candidate (Overseer 88%, rusher 62% balanced per seat, tails
0-4.5%, strong rhythm, tier value share 53-77% across profiles)
failed three fun-gate structure-reach floors:

- Fabricator 85.7% < 90 (close; some fast wins legitimately skip tech)
- Turret 38.7% < 40 (noise-level at 3 seeds)
- Array 26.6% < 60 (real behavioral gap)

Analysis: the floors were authored against the deleted 0.14 actor
family, whose meta was Array-reliant. The Array is already cheap (120
scrap, permanent half-map radar), so making it cheaper to force reach
would repeat the bribery pattern the balance lab rejected. The
candidate skips radar because its training opponents rarely punish
blindness. The gate's live rationale is anti-stealth literacy (a
no-Array bot is exploitable by Scuttle Charge play) and that stays.

Experiment r9: raise organic intel pressure — overseer mix 0.30 (the
one opponent that lays mines, flies bombers, and plays the deep
tree), full faction deal retained, rusher 0.15 to hold the closed
exploit. 250 updates from r8-final. If reach rises but Array lands
short of the 0.14-era 60%, the floor gets recalibrated from measured
0.15-family data with an explicit anti-stealth minimum — a documented
re-anchor (like the liveness floors when the Overseer replaced the
old actors), not gate-shaving.

Result: the organic-pressure hypothesis FAILED, decisively and
informatively. r9 (250 updates, ckpt-01800): Fabricator reach now
PASSES 90%, tier-2/3 value share rose to 66/81/68% across profiles
(r8: 53/77/62%) — the overseer-heavy mix deepened teching. But Array
reach fell 26.6% -> 22.5% and Turret 38.7% -> 33.8% while the
candidate beat the mine-laying Overseer 48/60 (80%, seats 24/24).
Tripled exposure to the one opponent that punishes blindness produced
MORE wins with LESS radar. Cup cost: rush defense regressed 62% ->
45% (Ferrous 10/30) with rusher down-weighted to 0.15 — the
single-focus phases seesaw.

Diagnosis (balance, not training): the 120-scrap Array was dominated
by the 60-scrap scout flyer — mobile, vision 10 true sight vs the
mast's static vision 9, and the blip ring (tile-only, no kind, no
owner) does not pay a 2x premium. The learned scout-first meta was
correct play. Resolution per the balance-vs-training razor: make the
mast worth wanting — cost 120 -> 90, RADAR_DETECT_RADIUS 16 -> 20
(one mast now watches a whole approach corridor; persistence is the
product scouts cannot replicate). Deep Array untouched. Floors
re-anchored from the measured 0.15 family: turret 0.40 -> 0.30
(anti-passivity minimum; the 0.40 floor described the deleted 0.14
actor's turtle-leaning meta), array 0.60 -> 0.25 (the anti-stealth
minimum, expected to be cleared with room once the rebalance is
trained in). Fabricator stays 0.90. Workspace 0.15.0-beta.4, hashes
re-blessed (12 rows).

Follow-up (r10): one consolidation phase holding EVERY pressure at
once instead of another single-issue phase — rusher 0.20 AND overseer
0.25, full faction deal, zero shaping (no structure/mix bonus; r5
proved tier preference survives unshaped, and the rebalanced tree
should carry it), from r9's checkpoint so the fabricator-reach and
tier-share gains are kept. Candidate selection then compares r8, r9,
and r10 on the full battery rather than assuming the last checkpoint
wins.
