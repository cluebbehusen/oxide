# The experiment log

The working lab notebook of Oxide's bot-training and balance
campaigns, split by era, kept as it was written — hypotheses, failed
runs, corrections, and verdicts included. This is the *why* behind
the shipped weights and balance numbers; AGENTS.md carries only the
distilled rules.

Newest entries at the bottom of each file. All win rates are
seat-swapped (each seed played from both seats) unless noted. "Cup" =
native Rust tournament of the quantized artifact
(`oxide-driver neural-cup`); "tournament" = torch-side eval
(`tools/train/tournament.py`).

| Era | File | Headline |
|---|---|---|
| 0.7 (2026-07-20/21) | [the learned bot](2026-07-20-the-0.7-learned-bot.md) | BC warm starts, the anchored league, conditioning knobs, the first shipped artifact |
| 0.8 (2026-07-21/22) | [the full roster](2026-07-21-the-0.8-full-roster.md) | gym v3, air/teams/artillery-era retrain, the 2v2 studies, drift doctrine at scale |
| 0.9 (2026-07-22) | [the artillery era](2026-07-22-the-0.9-artillery-era.md) | training throughput, real shells, the v4 schema freeze, the consolidation lineage |
| 0.10 (2026-07-23/24) | [the pacing campaign](2026-07-23-the-0.10-pacing-campaign.md) | balance instruments, ten rounds against the spam equilibrium, the fun gate opens |
| 0.11 (2026-07-25/26) | [the salvage campaign](2026-07-25-the-0.11-salvage-campaign.md) | gym v5, the salvage verb's consolidation lineage, the flipped-faction probe |
