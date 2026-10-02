---
name: oxide-opponent
description:
  Build and evaluate oxide-opponent, Oxide's reactive best-effort opponent
  controller. Use for any work in the oxide-opponent crate or its host
  integration, its tests and staged scenarios, its CPU and line reports, its
  evaluation, and its difficulty, stance and personality mapping. Not for
  oxide-bot; use scripted-bot for that.
---

# oxide-opponent

Read [the specification](../../../docs/oxide-opponent.md) first. It is normative
for this crate. `docs/bot-strategy.md`, `docs/bot/` and the scripted-bot skill
describe `oxide-bot` and do not apply here; do not import their
exact-allocation, forecasting or planning-progress requirements.

## Rules

1. **Invariants come first:** fairness, fog honesty, determinism, and saving and
   resuming this bot's own games.
2. **Budgets are constraints.** Improve play within the CPU budget. When a
   single decision is too slow, simplify or drop the expensive feature; when
   total CPU is too high, decide less often. Immutable map preprocessing and
   scratch reused within one decision are fine. Retaining derived answers across
   changing observations (a cache with invalidation) needs Connor's approval.
3. **Follow the specification's limits.** Precise about now, approximate about
   the future; one running total per decision; no mission owns future
   production; only the allowed computation. A new checkpoint field needs a
   design review. A change that adds reservations, proofs or planning state
   across decisions needs Connor's approval; stop and ask rather than adding
   one. Missions read each other only for unit membership, held targets, whether
   a mission holds its units, and the current rival.
4. **Counts come from need.** What the bot owns, trains, builds or sends comes
   from a need it can state from what it knows: threat, known defenses,
   deliverability, wounds, stale points, harvestable scrap, spare producer time.
   A constant may bound computation or model a difficulty, stance or personality
   limit, and its doc comment says which; it never sets what the bot owns or
   sends. A computation bound must not bind in normal play.
5. **Planning never spans decisions.** Missions and their phases may. Background
   decision execution stays as it is.
6. **Measure performance** for every PR: average and p99 time per decision and
   total CPU on the defined workloads. The bots-to-simulation ratio is a
   diagnostic, not a gate.
7. **Behavior may change.** Simplifying code or improving play justifies a
   behavior change. Fixtures driven by this bot live in their own file, separate
   from simulation-only hashes. Rebless them yourself after checking the
   smoke-matrix comparison. Simulation rule changes still need Connor's
   approval.
8. **Tests check what the bot does.** Every reactive behavior gets a
   command-level acceptance test in a staged scenario. Focused tests of memory,
   ranking, geometry, symmetry and bookkeeping are welcome. Do not pin tuning
   constants, incidental ordering or intermediate plans. Fairness, determinism
   and save-resume are always tested.
9. **Size is reported, not gated.** Measure every PR's net production and test
   line change, including code this bot adds anywhere (kit, simulation, driver).
   Growth past about 15,000 production lines triggers a design review, not a
   failure.
10. **Guards need evidence:** a failure seen in evaluation, a replay, a playtest
    or prior evaluation evidence, or one directly demonstrable. All work is
    bounded: finite inputs and a bounded number of visits or expansions.
11. **Parallelism only where measured.** Seats already run in parallel.
12. **Plain Rust.** Plain functions and data over traits and generic frameworks;
    enums for mutually exclusive states; no abstraction without two real
    callers.
13. **Docs describe behavior and boundaries, not algorithms.**

## Run it

- **Shell:** Settings > **Opponent AI: New** seats `oxide-opponent` in every bot
  seat of the next New Match; **Classic** keeps `oxide-bot`. Rematches, saves
  and replays keep the choice recorded in their scenario.
- **Evaluation:**
  `cargo run --release -p oxide-driver -- bot-eval skirmish --controller opponent --opponent-controller scripted --ticks 6000`
  duels this bot against `oxide-bot`; `--controller` sets every seat and
  `--opponent-controller` overrides seat one. Add `--paired` for a second leg
  with the two configurations exchanged. The stub never attacks, so
  opponent-only runs end at `--ticks`.
- **Traces:** `--decision-trace-out <file>`, with `--out` and `--candidate`,
  writes one JSONL row per decision. Opponent seats' rows carry this crate's
  `Trace`: tick, player, bank, received own events, spent, purchases, unit-order
  count, allowance, saving target, protected scrap, and missions with their
  phases.

## Report at handoff

When handing off a PR, tell Connor its net production and test line change,
average and p99 time per decision with total CPU, the smoke-matrix comparison,
and the pressure scenarios that pass, including when they are unfavorable. Keep
them out of the PR description.

- **Lines:** `uv run tools/line_report.py <base>` prints the net production and
  test line change per top-level directory from the merge base; `--help` defines
  what counts. `--working-tree` includes uncommitted files.
- **CPU:**
  `cargo run --release --locked -p oxide-driver -- bot-cost <workload> --controller opponent`
  for each of `duel`, `skyhook` and `mature-armies` reports average and p99 time
  per decision and total CPU per seat and controller, with the observation build
  beside them. Run it on the branch and on its base, on one machine with no
  competing builds or benchmarks. `--controller scripted` gives `oxide-bot`'s
  figures for reference; `--json` gives the same report as data. The budget:
  `skyhook` at most 250 µs average and 1 ms p99 per decision; `duel` and
  `mature-armies` no worse than the base beyond noise.

Run the smoke matrix locally before handoff; CI does not run it:

```sh
cargo run --release -p oxide-driver -- bot-matrix driver/evaluation/smoke.json --out <new directory>
```

It plays Skirmish, The Deep Cut and Severance at Standard and Prime, Balanced
and Aggressive, three seed runs each. Each cell is a head-to-head pair, this bot
in seat zero and then seat one with one personality seed on both sides, plus one
`oxide-bot` mirror leg. Mirror rows are cached per user under the reference
digest of the `bot/`, `sim/` and `chassis/` sources, the `kit` code that hosts
`oxide-bot`, and `Cargo.lock` (`--baseline-cache` moves the cache), so they
rerun only when those inputs, a map, a seed or the tick limit change.
`bot-matrix-report <rows.jsonl>...` re-reads published rows; `--json` prints the
same report as JSON. `--replay-dir <dir>` saves a replay of every evaluated leg
with their compact rows in `legs.jsonl`; cached mirror legs have none.

A PR that touches a map family or mode also runs the matching subset:
`severed.json` (Severance and The Scattering at Scrapheap, Standard and Prime),
`free-for-all-smoke.json` (Salvage Triangle and Scramble Basin at Standard) and
`teams-smoke.json` (Open Quarry at Standard), each a few minutes to about ten.

Stage checkpoints also run `driver/evaluation/duels.json`, the full two-seat
matrix, `teams.json` and `free-for-all.json`. A team map adds a mixed pair to
its head-to-head pair: both bots on each team, alternating along its front so
that facing enemies run different bots, then every seat flipped. A free-for-all
is a mixed pair on alternating seats. Every seat is controlled, including the
authored human chair.

Read the report by match mode, overall and by difficulty, stance and map family:

- **Pairs**: this bot wins both legs, split, `oxide-bot` wins both, or undecided
  (at least one leg without a winner). A head-to-head leg goes to the winning
  side. A mixed leg goes to the bot whose seats outlast the other's more often:
  survivors tie for first and seats that fall on one tick tie; a leg where
  neither bot does better, or that a stall loop stopped, has no winner.
- **New share**: this bot's share of legs that had a winner, with a 95% Wilson
  interval. Small matrices give wide intervals; compare runs, not single cells.
- **Decided new-old** against **decided old-old**: compared legs should decide
  at least as often as the mirror.
- **Placement**: mean place (1 is last standing) and median survival tick of
  each bot's seats in mixed legs. It is a diagnostic: surviving longer can be
  passive play, so review replays before reading it as strength.
- **Failure incidents** and **income**, per controller with seat-legs for scale.
  `oxide-bot` numbers are the reference, not a target. Rows recorded before a
  detector existed do not count toward it, and the report shows how many
  seat-legs did; mirror rows cached before then stay unmeasured until the
  baseline cache is cleared.
- **Deliveries**, shown when any seat trained armed ground units on severed
  ground: their scrap delivered, lost and left at home.

## Failure detectors

Evaluation rows carry omniscient QA detectors, checked every 12 ticks for every
controlled seat. They never reach a controller.

- **Repeated orders:** one unit stalls with the same reason 5 times within 1,200
  ticks. The episode ends after a full window without that stall. Danger holds
  are exempt: a harvest line waiting out danger re-reports every 100 ticks by
  design. They still count in the row's stall evidence.
- **Abandoned sites:** a paid, visible, unbuilt base-tier site makes no
  construction progress for 1,200 ticks.
- **Starved production:** every built producer of the seat stays idle for 1,200
  ticks while the bank, less scrap this bot protects for a saving target, covers
  the cheapest unit any of them may legally train. Queueing anything ends the
  episode. Rows also list each producer's idle, affordable ticks as a
  diagnostic, not an incident.
- **Stuck missions:** one of this bot's missions stays in one phase for 1,200
  ticks past the timeout the bot gives that phase. The detector reads the
  missions the controller reports and never changes them.
- **Idle army:** for 1,200 ticks while a hostile player still plays, armed units
  worth at least half the seat's army, and at least 1,500 scrap, have each
  rested within 12 tiles of an own building for 2,400 ticks: within 2 tiles of
  where they settled, with no enemy inside their weapon range plus 4 tiles.

Rows also report **deliveries** as a diagnostic: the scrap of armed ground units
a seat trained while its ground touched no standing hostile building, split into
those that reached other ground, those that died, and those still at home 6,000
ticks after training.

## Reactivity detectors

Rows also record, for every controlled seat, the situations it met and how it
answered them. A case opens when a situation first holds and closes once:
answered when the response shows in time, moot when the situation ends first,
missed at its deadline. Situations a seat must see count only what it sees. A
seat that has lost its last Foundry opens and answers no more cases, and a
building's repair case opens once per spell under 75% health.
`bot-matrix-report` prints each controller's cases per mode, with `oxide-bot`'s
as the reference. Missed cases keep their ticks for replay review with
`--replay-dir`.

- **Anti-air:** the first armed enemy aircraft seen; the seat owns a dedicated
  anti-air unit or a built Flak Turret within 3,600 ticks.
- **Airworks:** an enemy Airworks seen before any armed aircraft; anti-air
  before the first one.
- **Ground and air defense:** a seen armed enemy within 8 tiles of an own
  Foundry; the seat's units or turrets hit one, or its guns fire at one, within
  600 ticks.
- **Artillery:** an enemy shell fired at the seat's units or buildings that
  lands within 8 tiles of its buildings; the seat hits the gun, or its guns fire
  at it, within 1,200 ticks of the launch, moot if the gun dies to something
  else.
- **Scouting:** a standing hostile start unseen for 3,600 ticks; seen again
  within 3,600.
- **Evacuation:** a worker more than 8 tiles from home in a seen armed enemy's
  reach; out of reach, having moved 2 tiles, within 240 ticks.
- **Repair:** a built building other than an obstacle under 75% health with no
  seen armed enemy within 10 tiles; its health rises within 1,200 ticks.
- **Restoration:** a destroyed Extractor; another on its site within 3,600
  ticks, moot if an armed enemy still stands near.
- **Relief:** a seen armed enemy within 8 tiles of an ally's Foundry; the seat
  hits one, or fires at one, within 1,200 ticks.
- **Withdrawal,** this bot only: an attack, strike or raid in its fight; it
  withdraws rather than vanishing with half its units lost.

The report also gives this bot's target switches per 10,000 ticks: how often a
new attack, strike or lift goes after a different player than the one before it.
Raids, which take the least guarded harvest line, are left out.

Income compares scrap earned in the minute before ticks 6,000, 12,000 and 24,000
(deliveries plus Reclaimer, Extractor and Foundry credits) with a saturation
estimate: two Harvesters on each of the four nearest scrap nodes that still hold
scrap for every completed Foundry, at their straight-line round trip, plus those
credits. No node counts for two Foundries. The estimate is a fixed yardstick for
comparing runs, not the bot's own staffing, so income can exceed it. A seat
stops sampling once it is eliminated, so a seat that is out while its team plays
on adds no empty samples.

## Pressure scenarios

`cargo run --release -p oxide-driver -- bot-pressure --controller opponent` runs
the staged scenarios in `driver/evaluation/pressure/`; `--replay-dir` saves each
run for `replay-summary`. A scripted attacker seat, whose Foundry sits on an
island no ground unit reaches, issues tick-stamped commands for its preset units
while the bot defends:

- **Early rush:** Sentinels and Scuttlers attack the base; the Foundry must
  stand at the deadline.
- **Air switch:** two Buzzards harass the base; the bot must field anti-air (a
  dedicated anti-air unit or a Flak Turret) within 1,500 ticks of first seeing
  them.
- **Siege:** Bombards shell the base with a Kestrel spotting; by the deadline
  they must be destroyed or out of range. Killing only the spotter does not
  pass.
- **Lift drop:** a Skyhook sets four Sentinels down beside the harvest line at
  tick 1,900, about as early as an enemy could field a Skyhook and its load
  after a Fabricator and an Airworks; every landed unit must be destroyed,
  unless the Skyhook falls before setting anyone down.

`oxide-bot` passes all four. Scenario files are JSON: the staged scenario, the
defender and attacker seats, a deadline, the script (unit ids follow scenario
order) and the check.

## Review play

Use `replay-summary` and the native shell through the oxide-live-qa skill. Give
tick ranges for notable moments. Automated metrics find candidates and failures;
human play and replay review decide whether the bot is credible and fun.
