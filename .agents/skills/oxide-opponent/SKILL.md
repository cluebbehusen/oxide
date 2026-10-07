---
name: oxide-opponent
description:
  Build and evaluate oxide-opponent, Oxide's reactive best-effort opponent
  controller. Use for any work in the oxide-opponent crate or its host
  integration, its tests and staged scenarios, its CPU and line reports, its
  evaluation, and its difficulty, stance and personality mapping.
---

# oxide-opponent

Read [the specification](../../../docs/oxide-opponent.md) first. It is normative
for this crate.

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
   from simulation-only hashes. Rebless them yourself after checking the ladder
   smoke. Simulation rule changes still need Connor's approval.
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

- **Shell:** every bot seat of a New Match runs this bot. Rematches, saves and
  replays keep the configuration recorded in their scenario.
- **Evaluation:**
  `cargo run --release -p oxide-driver -- bot-eval skirmish --ticks 6000` plays
  this bot in every seat; `--difficulty` and `--stance` set every seat and
  `--opponent-difficulty` and `--opponent-stance` override seat one. Add
  `--paired` for a second leg with the two configurations exchanged.
- **Traces:** `--decision-trace-out <file>`, with `--out` and `--candidate`,
  writes one JSONL row per decision. Opponent seats' rows carry this crate's
  `Trace`: tick, player, bank, received own events, spent, purchases, unit-order
  count, allowance, saving target, protected scrap, and missions with their
  phases.

## Report at handoff

When handing off a PR, tell Connor its net production and test line change,
average and p99 time per decision with total CPU, the ladder smoke against the
base, and the pressure scenarios that pass, including when they are unfavorable.
Keep them out of the PR description.

- **Lines:** `uv run tools/line_report.py <base>` prints the net production and
  test line change per top-level directory from the merge base; `--help` defines
  what counts. `--working-tree` includes uncommitted files.
- **CPU:** `cargo run --release --locked -p oxide-driver -- bot-cost <workload>`
  for each of `duel`, `skyhook` and `mature-armies` reports average and p99 time
  per decision and total CPU per seat, with the observation build beside them.
  Run it on the branch and on its base, on one machine with no competing builds
  or benchmarks; `--json` gives the same report as data. The budget: `skyhook`
  at most 250 µs average and 1 ms p99 per decision; `duel` and `mature-armies`
  no worse than the base beyond noise.

Run the ladder smoke locally on the branch and on its base before handoff; CI
does not run it:

```sh
cargo run --release -p oxide-driver -- bot-ladder driver/evaluation/ladder/ladder-smoke.json --out <new directory>
```

It plays every comparison of the ladder on Skirmish, The Deep Cut, Subsidence
and Severance, Balanced, two seed runs each. A PR that touches team or
free-for-all play also plays every seat of those maps at one rung and reads the
seats:

```sh
cargo run --release -p oxide-driver -- bot-eval scenarios/open-quarry.json scenarios/salvage-triangle.json scenarios/scramble-basin.json --runs 3 --candidate <name> --out <rows.jsonl>
cargo run --release -p oxide-driver -- bot-summary <rows.jsonl>
```

`bot-summary` pools the seats of any evaluation rows by team layout (`duel`,
`teams`, `free-for-all`, or team sizes such as `2v1`) and difficulty; `--json`
prints the same summary as JSON.

Read the ladder report per comparison, overall and by stance and map family:

- **Pairs**: the higher rung wins both legs, split, the lower rung wins both, or
  undecided (at least one leg without a winner).
- **Share**: the higher rung's share of decided legs, with a 95% Wilson
  interval, against the comparison's gate. Small runs give wide intervals;
  compare runs, not single cells.
- **Net worth share**: the higher rung's share of army, buildings and bank by
  pair at 6k, 12k, 18k and 24k ticks. It moves with far fewer legs than win
  share and shows when a game turns; a match that ended sooner carries its final
  worth.

Then each rung's seats, and in `bot-summary` each team layout and rung's:

- **Failure incidents** and **income**, with seat-legs for scale. Rows recorded
  before a detector existed do not count toward it, and the report shows how
  many seat-legs did.
- **Deliveries**, shown when any seat trained armed ground units on severed
  ground: their scrap delivered, lost and left at home.
- **Reactivity**: the situations below, how many arose, the share answered in
  time, missed and moot, and the mean ticks to an answer.
- **Ledger**:
  - **Attacks**, grouped by strength sent against the known defense at launch:
    how many withdrew, fought on or never met the enemy, and value dealt over
    value lost. A withdrawal rate that does not fall as the sent-to-known ratio
    rises means the estimate misses what the attack meets.
  - **Units and buildings**, most-bought first: value dealt per scrap paid, by
    victim and by place (near its own buildings, near the enemy's, or neither),
    enabled (damage friendly shooters dealt to targets only that kind saw),
    repair supplied, deaths and lifetime; for buildings, income and production
    per scrap. Front-line units absorb damage for ranged ones, so their own
    returns understate them; upgraded buildings survived to be upgraded, so
    compare tiers with care.

`replay-ledger <replay>...` gives the same ledger tables for any replay or match
recording, including human games.

## Difficulty ladder

The ladder measures the rungs against each other:

```sh
cargo run --release -p oxide-driver -- bot-ladder driver/evaluation/ladder/ladder.json --out <new directory>
```

Each comparison pits a higher rung against a lower one on duel maps, in pairs
with the rungs swapped between the seats, both seats sharing one personality
seed. A comparison passes when the higher rung wins at least its gate of decided
legs over at least the manifest's number of decided pairs: each rung against the
one two below it at 65%, and Prime against Scrapheap at 80%, over 40 decided
pairs. `ladder.json` covers the nine duel maps, every stance and four runs,
about 650 legs; run it for a lever's final numbers. `ladder-smoke.json` is the
quick check at handoff and while a difficulty lever is in progress.
`bot-ladder-report <rows.jsonl>...` re-reads published rows, and `--replay-dir`
saves a replay of every leg with their compact rows in `legs.jsonl`.

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
building's repair case opens once per spell under 75% health. `bot-ladder`
prints each rung's cases and `bot-summary` each team layout and rung's. Missed
cases keep their ticks for replay review with `--replay-dir`.

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

`cargo run --release -p oxide-driver -- bot-pressure` runs the staged scenarios
in `driver/evaluation/pressure/`; `--replay-dir` saves each run for
`replay-summary`. A scripted attacker seat issues tick-stamped commands for its
preset units while the bot defends. Its Foundry stands in a walled corner: for
early rush, air switch and siege the corner opens at its far end, so the
defender's ground reaches it only by a long route and the defender is not cut
off from its enemy; for lift drop it is an island no ground unit reaches, since
the drop must come by air.

- **Early rush:** Sentinels and Scuttlers attack the base; the Foundry must
  stand at the deadline.
- **Air switch:** two Buzzards harass the base; the bot must field anti-air (a
  dedicated anti-air unit or a Flak Turret) within 1,500 ticks of first seeing
  them.
- **Siege:** Bombards shell the base with a Kestrel spotting; by the deadline
  they must be destroyed or out of range. Killing only the spotter does not
  pass.
- **Lift drop:** a Skyhook loaded at the start sets four Sentinels down beside
  the harvest line at about tick 1,900, about as early as an enemy could field a
  Skyhook and its load after a Fabricator and an Airworks; its unload is ordered
  early enough for the flight to land then. Every landed unit must be destroyed,
  unless the Skyhook falls before setting anyone down.

Scenario files are JSON: the staged scenario, the defender and attacker seats, a
deadline, the script (unit ids follow scenario order) and the check.

## Review play

Use `replay-summary` and the native shell through the oxide-live-qa skill. Give
tick ranges for notable moments. Automated metrics find candidates and failures;
human play and replay review decide whether the bot is credible and fun.
