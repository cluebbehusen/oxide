---
name: scripted-bot
description:
  Design, change, debug, and evaluate Oxide's fair rules-based opponent. Use for
  Brain, UtilityPolicy, Dials, Observation, Intent, Executive, SeatBot,
  BotConfig, scripted openings, economy, scouting, tech, combat, expansion, team
  conduct, liveness, bot replays, bot difficulty proposals, or whether a match
  looks credible and fun.
---

# Oxide scripted bot

Build one opponent that plays a recognizable, complete game under the same
constraints as a person. Do not turn aggregate success into a claim that its
matches are sensible or fun.

## Preserve the level playing field

The bot is an ordinary command source. It receives a fog-honest observation and
may issue only shared `PlayerCommand` values. It gets no hidden income, vision,
stats, prerequisites, queue space, build privileges, movement, or combat rules.

Public map knowledge such as authored starting anchors may be passed explicitly.
Current enemy state, unobserved terrain facts, and omniscient driver views may
not influence a decision. When behavior needs information the observation does
not contain, either add a fog-honest observation with tests or design behavior
that does not require it.

## Know the controller stack

The maintained path is:

```text
fog-honest Observation -> UtilityPolicy Intent -> Executive -> PlayerCommand[]
```

- `Brain::balanced` is the player-facing rules-based controller.
- `Dials::balanced` names its complete strategic surface.
- `Brain::overseer` is a separate stable QA anchor. Do not silently change it
  while tuning the playable opponent.
- `seat_bots` constructs controllers requested by scenario `BotConfig`.
- Authored bot seats use `BotConfig::Scripted`. Replays preserve emitted
  commands, so playback does not rerun the controller.

Keep policy memory controller-local and deterministic. A resumed replay rebuilds
controller memory by observing the authoritative recorded prefix; never add an
unrecorded state mutation to make resume convenient.

## Change one behavior at a time

State the player-visible problem before tuning. Good targets are concrete:

- leaves harvesters idle while known safe scrap exists;
- repeats an impossible build forever;
- never reaches a named tech rung on a map where it can;
- sends an army through a visible losing fight;
- hoards through a winning window;
- stops issuing meaningful commands for a long interval.

Capture the smallest deterministic scenario and seed that exhibits the problem.
Test the observation, intent, or lowering layer that owns it. Avoid adding a
special case in a later layer to hide an earlier bad decision.

Use explicit stable tie-breakers and dedicated RNG streams for genuine seeded
variation. Do not use randomness to make a broken policy harder to diagnose.

## Evaluate from cheap to expensive

Run focused bot tests first:

```sh
cargo test -p oxide-sim --test bot_brain --locked
cargo test -p oxide-sim --test bot_policy --locked
cargo test -p oxide-sim --test scripted_bot --locked
cargo test -p oxide-sim --test overseer_015 --locked
```

Then run complete seeded matches on representative shapes: a normal duel, an
island or severed-ground map, a team map, and a long or grand map. Ask the
driver for its current syntax rather than copying stale flags:

```sh
cargo run -p oxide-driver -- run --help
cargo run -p oxide-driver -- replay-summary --help
```

The complete-match path is `run <scenario> --all-bots`; ordinary `--bots` honors
the scenario's configured chairs and therefore leaves its human chair under
human control. Add `--save-replay <path>` for review evidence.

For each candidate, preserve the scenario, seed, replay, final hash, result,
duration, and a short behavioral verdict. Compare repeated identical runs for
exact hashes. Check that the controller:

- keeps an economy alive and replaces losses;
- builds and uses the reachable tech tree rather than merely owning it;
- scouts, reacts to discovered threats, and attacks through legal knowledge;
- escapes or changes plans after a failed route or site;
- behaves coherently after the opening and through the match's end;
- remains active on every seat, faction, and team shape in scope.

Use `replay-summary` to find long silences, nonsense loops, missed tech,
one-sided non-participation, and suspicious endings. Then watch the suspicious
and representative replays. Metrics are a triage tool; they do not certify
credible play.

## Promote by human judgment

A scripted change is not done because it wins, decides more games, or improves
an average. Play against it and watch full matches beyond the opening. Record
what the bot appeared to be trying to do, where that intention became legible,
and where it behaved nonsensically.

Keep one Balanced opponent until that baseline is worth preserving. Do not add
difficulty levels by granting advantages or by arbitrarily disabling whole
strategic channels. A future ladder should alter decision quality or execution
in a measured, explainable way and must be reviewed as a separate design.

Finish with the full Rust gates from `AGENTS.md` and the native QA path from the
`oxide-live-qa` skill whenever setup UI or player-facing behavior changed.
