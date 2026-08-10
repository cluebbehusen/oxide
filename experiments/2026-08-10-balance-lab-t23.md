# Balance lab — the tree was not worth climbing

Question (Connor's steer): is tier decay a training failure or a
balance failure? Never force a unit; make it worthwhile — the bot
should tech because teching wins.

Method: cost-normalized paired arena duels (`driver matchup`), both
seats, at workspace 0.15.0-beta.2 stats.

Before (the learner's tier-1 convergence was CORRECT play):
- warden:3 (840) vs sentinel:9 (810): warden wipes (works)
- warden:2 (560) vs lancer:5 (550): 0-550 WIPE both seats — the T1
  rail deleted the T2 line unit at equal cost (and carries 2.6x the
  damage-per-scrap with 2.5 tiles more range)
- breaker:1 (900) vs lancer:8 (880): verdict FLIPPED on seat swap —
  the tier-crusher coin-flips tier one at equal cost, after a
  500-scrap Crucible
- avalanche:1 (700) vs bombard:4 (680): 0-800 loss both seats — two
  7-second shots per Bombard kill; T1 artillery obsoleted its own
  successor
- avalanche:1 vs scuttler:9: loses (the intended rush counter)

Changes (sim/src/stats.rs, provenance in the stat comments):
- Warden 240hp/24dmg -> 260hp/32dmg
- Breaker 90dmg/4.0rng/1.2spl -> 115dmg/4.5rng/1.5spl
- Avalanche 70dmg/140t -> 110dmg/120t

After:
- warden:2 vs lancer:5: still loses (counter preserved) but 385
  surviving, a trade instead of a wipe
- breaker:1 vs lancer:8: 900-0 decisive, no seat flip
- avalanche:1 vs bombard:4: 700-0 decisive both seats
- avalanche:1 vs scuttler:9: still loses (counter preserved)
- warden:3 vs sentinel:9: still decisive (hp-weighted survival up)

All sim suites, liveness, and map gates green; hashes re-blessed at
0.15.0-beta.3. League r3 (still training on the old stats) stopped;
r4 relaunches on the new economics. The verification that matters
comes from r4: does the policy tech because it wins, with shaping as
transitional exploration only.
