# Lineage-critical float checkpoints

The torch parents of shipped artifacts, committed so a future
continuation never repeats the 0.14 loss (that era's float parent
lived only in the gitignored `runs/` and vanished, forcing the 0.15
from-scratch restart).

- `r17-distilled.pt` — the float parent of the promoted 0.15 actor
  (`sim/src/bot/ladder_weights.json`, digest `320706eb6eb5882e`).
  Fine-tunes and continuations initialize from this.
- `prior-v9.pt` — the campaign's BC prior and the constant `--anchor`
  of every league phase; continuations keep anchoring here.

Rejected runs, pool checkpoints, and experiments stay out of the
repository; only artifacts a shipped actor's lineage depends on
belong here.
