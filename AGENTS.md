# Agent instructions

Working notes for AI agents (and curious humans) developing this repository.

## What this is

**Oxide** — a small 2D RTS in Rust about self-replicating machines salvaging
a derelict world. Two factions, **Ferrous** (rust orange) and **Cupric**
(teal patina), harvest scrap, train units, and try to demolish each other's
Foundry. See README.md for the player-facing story and controls.

The repo doubles as a testbed for agent-driven development. The architecture
is optimized for *agent legibility*: a pure, deterministic, headless
simulation; a thin disposable renderer; and a driver CLI that can test the
sim headless or drive the live game over a debug socket — including
screenshots you read back and judge with your own eyes.

## Crate map

| Crate | Path | Purpose |
|---|---|---|
| `chassis` | `chassis/` | Reusable deterministic-sim toolkit: Q32.32 fixed point (`fx`), PCG32 (`rng`), FNV-1a state hashing over postcard bytes (`hash`), tile grid (`grid`), 8-dir A* (`path`), tick-stamped replay format (`replay`), atomic durable file writes (`fsx`). No game rules, no engine deps. |
| `oxide-sim` | `sim/` | All Oxide game rules. `State::tick(&[PlayerCommand])` is the only way anything happens. The bots live here too, but *outside* the tick pipeline — command sources like the mouse: the shipped **neural ladder** (`bot::NeuralBot`, embedded quantized weights, Easy/Medium/Hard/Expert + a personality knob), the scripted `bot::Brain` tiers (fog-honest, training anchors and benchmarks), and the classic 0.6 `bot::Bot` (what replays without a `bot_config` reproduce). |
| `oxide-protocol` | `protocol/` | The whole wire contract: JSON-lines envelope, tagged requests/replies, `RawEvent` input events (touch included for the future mobile shell), `StateView` (floats + ASCII map — legible, not exact; exactness is the hash's job), the fog-honest `FogView`, the framed TCP transport loop (`framing`) both servers run, and the `DebugSession` trait whose `dispatch_shared` is the ONE implementation of the shared request surface (state reads + the driven clock) every session kind — live game, replay viewer, headless session — answers through. |
| `oxide-shell` | `shell/` | macroquad renderer, the single input funnel, HUD, debug server. Nothing here may affect game outcomes except by staging tick-stamped commands. |
| `oxide-kit` | `kit/` | Shared engine-side toolkit: the headless scenario/replay `runner`, the replay `playback` engine (viewer and CLI), `stats` extraction (post-match screens, `replay-stats`), and the CPU software `render`er (tiny-skia) behind goldens and map previews. Exists so the shell never depends on the dev harness. |
| `oxide-driver` | `driver/` | CLI harness: headless scenario runs, replay verification, byte-exact golden images, live-game client, the windowless `session` server, automated smoke test. A library too (`client`/`session`/`smoke`/`audit` plus re-exports of the kit modules). |

Built for reuse in a later, bigger game: `chassis` wholesale, the protocol's
envelope/raw-event design, the driver's harness patterns. Game-specific and
disposable: `oxide-sim` rules, sprites, shell HUD.

## Determinism invariants — do not break these

Target: **same seed + same command log ⇒ bit-identical state on every run
and every platform.** Replays, regression hashes, and the smoke test all
assert this.

1. **No float arithmetic in `chassis` or `oxide-sim`** — enforced by
   `clippy::float_arithmetic = deny` in those crates. Sim math is
   `chassis::fx::Fx` (Q32.32); `sqrt` is integer-based, no libm.
   Floats are fine in shell/driver/protocol — presentation only.
2. **Never iterate a `HashMap`/`HashSet` in sim logic** — the workspace
   `clippy.toml` warns everywhere and `chassis`/`oxide-sim` deny it;
   the shell explicitly allows it for presentation caches.
3. **All randomness through `chassis::rng::Pcg32`**, seeded from the
   scenario. Never from time, thread ids, or the OS.
4. **Commands are tick-stamped; the replay is the only input.** Every
   command source (mouse, bot, debug socket) funnels into
   `Game::do_tick`, which records before it executes. A code path that
   mutates `State` without a recorded command breaks replays.
5. **Iterate entities in id order; tie-break every selection with an
   explicit key** ending in an id or (y, x) — see the brains for examples.
6. **No wall clock in the sim.** `State::tick` is the only time step. The
   shell's accumulator and `advance_ticks` both bottom out there.

If a change legitimately alters sim behavior, hashes and goldens move.
Re-bless (below), *look at* the regenerated goldens, and explain the change
in the commit message. A branch that blesses `state-hashes.json` must
already carry its cycle's workspace version: SIM_VERSION stamps every
replay and autosave, and a behavior change wearing last release's
number lets an old binary silently reconstruct a different world.
Since 0.13 the fixture enforces this mechanically — it carries the
`sim_version` it was blessed under, and `BLESS=1` refuses same-version
hash movement on an existing row (bump the version first;
`BLESS_SAME_VERSION=1` overrides for a justified exception, and new or
removed rows never block).

## Build, test, bless

```sh
cargo test --workspace               # all tests, headless, ~20s
cargo clippy --workspace --all-targets
cargo fmt --all
BLESS=1 cargo test -p oxide-driver   # regenerate goldens after intended change
```

The three map-sweep suites (every-shipped-map bot play-through, the
neural ladder yardstick, the hash fixtures) fan their independent
deterministic sims across threads — grown-shelf sweeps stay a
dozen-seconds affair, and a new sweep over per-map runs should spawn
the same way. The play-through
(`driver/tests/headless.rs`) is the **liveness gate**: 12k bot-vs-bot
ticks per map, extended to 24k on vast maps whose 100+ tile routes delay
first contact, and tallied per seat out of the `TickReport`s — units
produced, deliveries, buildings completed, shots fired, last-progress
tick. Every seat still holding a Foundry owes its format's production
and delivery floor; eliminated seats owe nothing (the
autonomous-remnants rule); the match owes recent progress, and a shot
below the 6/8-seat formats. The floors are authored at roughly a third
of the measured minimum by `liveness_floors_calibration_table` — an
`#[ignore]`d diagnostic in the same file that prints the whole
per-seat table. Re-run it and re-author after anything that moves bot
behavior. (Until 0.13 the gate was a unit-id proxy that a 32-unit
starting roster satisfied at tick zero.)

`sim/tests/fuzz.rs` fans the same way: seeded garbage down the command
surface, generated through a tag enum the compiler holds against
`Command`'s variant list (a new verb stops that file building until it
is fuzzed), with `State::validate_invariants` sampled along every run
and a set of reach premises asserting the garbage still lands — a
fuzzer that quietly stops reaching the deep shapes is the failure mode.
`FUZZ_SEEDS=<n> cargo test -p oxide-sim --test fuzz` soaks it past the
default sweep; a failure names its seed, which is the whole repro.

Before committing: fmt + clippy clean, tests green. Golden files live in
`driver/tests/goldens/`: byte-exact PNGs (a mismatch writes the actual
under `target/` for side-by-side inspection) plus `state-hashes.json`,
bot-vs-bot hashes at tick 2,000 for every shipped scenario — the cheap
tripwire that flags sim drift without image churn. Three PNGs,
not two: skirmish at t0 and t1200, plus `showcase.png` (0.13), a
scenario built in `driver/tests/golden.rs` — never under `scenarios/`,
which ships to players — and driven through a scripted program until
its final state holds every branch the CPU renderer can take: four
terrains, rubble, scrap at full/rich/depleted, a wreck, standing and
half-built structures of every kind, wounded machines of every kind on
both rosters, a laden harvester, a same-faction hostile pair. A
companion test names those features, so a showcase that stops covering
one fails with a sentence before the pixels disagree. Assets get the
same treatment without a GPU: `shell/src/assets.rs` holds
`assets/sprites/atlas.json` to a bijection against the keys the shell
resolves, so a regenerated atlas that dropped a row — or a new kind
whose art was never generated — fails by name in the ordinary test run
instead of at somebody's window.
`.github/workflows/ci.yml` runs the suite on three OSes plus an MSRV
job on every push and PR; the hash-fixture step re-derives per-OS as the
cross-platform determinism proof. Since 0.13 every cargo invocation is
`--locked`, the Linux leg gates strict rustdoc, the python job launches
a real `oxide-driver gym` worker (the Rust/Python contract handshake
CI was missing), and a `deny` job enforces `deny.toml` — advisories,
licenses, and source policy, with a reasoned triage ledger for
macroquad's stale corners.

## Running and driving the game

```sh
cargo run -p oxide-shell                            # play (human vs bot)
cargo run -p oxide-shell -- --debug-server          # + socket on 127.0.0.1:4123
cargo run -p oxide-shell -- --debug-server --paused # driven mode: sim time
                                                    # moves only on request
cargo run -p oxide-driver -- smoke --spawn          # automated live check
```

`--trace-startup` (or `OXIDE_TRACE_STARTUP=1`, which reaches the
packaged .app where flags are awkward) prints a stderr timeline:
prologue milestones with ms-since-entry, then per-frame gap and
hardware-event counts for the first 200 frames. Off by default, and
the env var alone never activates it under `--automation` — the
harnesses capture spawned shells' stderr.

A typical agent session against a running shell:

```sh
driver() { cargo run -q -p oxide-driver -- "$@"; }
driver live status
driver live state --map            # ASCII map with entities overlaid
driver live harvest 0 --units 0,1,2 --node 7,2
driver live advance-units 0 --units 3 --to 34,18
driver live attack-move 0 --units 3 --to 34,18
driver live rally 0 --building 0 --tile 7,2   # or --clear
driver live step 1                 # presented tick + exact sim events
driver live advance 300            # exactly 300 ticks, replies with hash
driver live screenshot -o screenshots/check.png   # then READ the png
driver live capture-sequence --present --out screenshots/motion
driver live inject-wheel 2.0       # events enter the real input funnel
driver live inject-key escape      # opens the pause menu — menus share
driver live inject-key enter       # the input funnel too
driver live inject-text "my save"  # types into the save-name field
driver live fog 0                  # seat 0's honest world: mask, ghosts,
                                   # remembered salvage, radar contacts
driver live save-replay replays/session.json
driver replay replays/session.json # must print the same hash as live
driver live load-replay replays/session.json      # resume = load a save
```

The same session, no window at all: `driver session` serves the
identical protocol windowless — a persistent headless match backed by
the kit runner, so every `driver live` verb above works against it
unchanged (it binds the shell's default port; `--port`, `--scenario`,
and `--idle-timeout` dial it). No GPU, no wall clock: the session is
permanently in driven mode, which is exactly what an agent wants —
`pause`/`resume`/`speed` and the window verbs (`camera`, `ui`,
`inject-*`, `overlay`) are refused in words rather than faked, and
everything else answers identically to a live shell, per-reply, which
`driver/tests/session_parity.rs` asserts (the headless half runs in
CI; the spawned-shell half runs with the #[ignore]d battery) — and
since 0.13 agreement is structural, not scripted: all three session
kinds (live game, replay viewer, headless session) answer the shared
surface through one `oxide_protocol::DebugSession` dispatch, with
window-shaped and mutating requests split off by capability and
refused in words where a session cannot honestly serve them.
`driver session` screenshots are the CPU schematic renderer, not the
shell's frame — the reply says `"renderer":"cpu"` so nobody judges
visual polish from the wrong picture. `query_state` stays
deliberately omniscient (QA view); `fog <seat>` is the first-class
fog-honest counterpart on both servers, built by one shared
`FogView::capture` so live and headless answers cannot drift.

```sh
cargo run -p oxide-driver -- session &              # windowless server on 4123
driver live status                                  # every live verb works
driver live advance 5000 && driver live fog 1
driver live screenshot -o screenshots/map.png       # CPU render, whole map
```

Save states are replays, by design: `load_replay` rebuilds the scenario,
re-runs the recorded ticks headless-fast, and keeps recording on the same
log — no snapshot format, no way for a save to desync from its history.
The cost is version-pinning (replays reproduce only on the sim that wrote
them) and load time proportional to session length, which at thousands of
ticks per second is noise. If sessions ever get long enough to hurt,
revisit with a snapshot+suffix-log hybrid — and keep the recorder valid.

The socket is bounded (`oxide_protocol::framing`, the one transport
both servers run): eight connections at once — a ninth is told so in an
error envelope and closed — request lines capped at
`oxide_protocol::MAX_FRAME_BYTES`, and a connection idle for half an
hour is dropped, which is deliberately far longer than a paused
driven-mode session ever parks (`--debug-idle-timeout` on the shell,
`--idle-timeout` on `driver session`, raises it for parked-overnight
agents). Undecodable bytes answer like any other bad request instead
of vanishing. Both sides budget synchronous advances from one figure,
`oxide_protocol::ADVANCE_TICKS_PER_BUDGET_SECOND`, so client deadline
and server reply deadline cannot disagree.

Headless, no window needed:

```sh
driver run skirmish --ticks 2000 --bots --map     # summary + ASCII map
driver render skirmish --ticks 1200 --bots -o out.png
driver replay-stats replays/session.json          # per-seat series + losses
driver map-audit scenarios/basalt-spine.json      # routes, fairness, pressure
```

Replay UX in the shell: cold launches land on Home; `Replays` opens
the Saves & Replays shelf — two header sections over one menu:
SAVES (autosaves and player-named saves; Enter loads one back into a
live session through the same loader as Home's Continue) and REPLAYS
(finished matches; Enter watches), with per-row delete and honest
version badges throughout. While a match is running, the pause menu's
Save Game writes a named save (the inline name field is the shell's
only text-entry surface: `RawEvent::Text` from the funnel,
printable-ASCII at ingest, static caret) into the saves dir rotation
never touches; the name lives in `ReplayMeta.description`, never the
filename, and records carry
`kind` + `saved_at` metadata — 0.12-era files classify by filename
prefix instead. `oxide-shell --watch <replay>` opens the read-only
playback viewer (pause, seek, 0.5x/1x/2x/4x/8x speed — no recorder;
backward seek restores an in-memory checkpoint and re-simulates;
checkpoint cadence stretches with record length so no replay retains
more than 64 state clones, and interactive loads cap at 2M claimed
ticks). Watch Replay replaces Save Game on the pause menu once the
match is decided — mid-match playback was a fog-free scout of the
enemy, which is also why the shelf's resumable records load instead
of watching. Surrender is its
mid-match complement (hidden again for a seat that already resigned
or lost its Foundry — no verb the sim would only reject), behind the
destructive-confirm dialog with its own subtitle: a 1v1 concession
decides on the next tick and the normal result flow takes over
(banner reads SURRENDERED); a team-game concession raises the
surrender overlay — concede-time stats, Esc to the menu as the exit
offer — and dismissing it leaves the human spectating under a
SURRENDERED - SPECTATING strip while the ally plays on. `sh tools/package_macos.sh`
builds `dist/Oxide.app` (resources resolve executable-relative when
bundled, cwd otherwise; `OXIDE_RESOURCE_ROOT` overrides both — the
harness's isolation seam, unvalidated by design: a wrong root fails
loudly at atlas load).

`screenshots/` and `replays/` are gitignored scratch output; keep goldens
and test fixtures inside crate `tests/` directories.

## Conventions

- **Conventional commits** (`feat(sim): …`, `fix(shell): …`, `docs: …`).
  Since 0.4.1 the repo lives at github.com/cluebbehusen/oxide and each
  version is developed on a branch (`0.5`, `0.6`, …) merged to `main` by
  PR — **main takes no direct commits**. Commits are signed (SSH key via
  ssh-agent — run `ssh-add --apple-use-keychain` after a reboot if
  signing starts failing). GitHub Actions are pinned to full commit SHAs
  with a version comment (`uses: owner/action@<sha> # vX.Y.Z`); resolve
  tags with `gh api repos/OWNER/REPO/commits/TAG --jq .sha`.
- **Idiomatic Rust.** rustfmt defaults, clippy clean, `missing_docs` warns
  in the library crates. Comments state constraints, not narration.
- **Assets are generated.** Sprites: `tools/gen_sprites.py` (palette at
  the top); sounds: `tools/gen_sounds.py` (stdlib-only synthesis). Run
  with `uv run`, commit script + output together. The sprite script also
  shelf-packs everything into `atlas.png` + `atlas.json`; the shell draws
  exclusively from that one texture (source rects, 1px edge extrusion
  against bleed) so the whole world batches into a handful of draw calls —
  never load per-sprite textures in the shell. The palette constants also
  appear in `kit/src/render.rs` and `shell/src/render.rs` — keep them
  in sync.
- **Scenarios** are JSON with ASCII maps: `.` ground, `,` rubble (cosmetic
  ground; the byte is hashed but nothing else changes), `#` rock, `^` peak
  (blocks ground, air, and fire — see the design bullet), `s` scrap
  node, `S` rich node (double salvage), `1`-`8` Foundry anchors (top-left
  of 2x2). (`w` appears in *rendered* ASCII for wreck tiles but is never
  authorable.) `PlayerSpec.team` groups seats; omitted means a team of
  one — genuinely: teams normalize to dense ids by first appearance,
  so an authored id can never alias an omitted seat, whatever number
  it picked — and every-seat-one-team is a build error. Shipped maps are
  180°-symmetric — author edits in mirrored pairs. The seat pairing is
  derived from the map, never assumed: a Foundry anchor rotated 180°
  lands on exactly one other anchor, and that relation is an involution
  over the seats — `{0<->1}` on duels, `{0<->3, 1<->2}` or
  `{0<->2, 1<->3}` on the 4-player maps, `{i <-> n-1-i}` on the lane
  stacks (so "every seat is an image of seat 0" is NOT the rule and is
  false on Open Quarry and Twin Forges). Paired seats must be
  cross-team, bank the same scrap, and carry unit lists that are exact
  entry-by-entry image transforms of each other — compared by
  `Role`, so a launch-time retint or a future faction-varied starting
  unit still reads as a mirror. `driver/tests/map_gates.rs` gates all
  of it, the 0.7 seat-fairness bug made permanently unrepeatable. The
  3v3/4v4 maps (Trident Plateau and Causeway Verdict; Compass Grand
  and Gatework Array) generalize further:
  stacks of identical 180°-self-symmetric lanes, and the
  map-gates fairness test holds all six/eight seats to strict scrap
  equality (team seats also need unique names — a headless sweep
  gates every shipped map, and launch gives colliding labels
  ordinals). Faction convention: even seats Ferrous, odd seats
  Cupric — the AUTHORED default. Since 0.11 any seat can retint at
  launch (`Scenario::retint_seat`: faction, faction-derived name,
  and starting units remapped through their roles) — the setup
  screen's faction chips land there (the retired 1v1 quick flow did
  too).
  `Scenario::skirmish()` embeds `scenarios/skirmish.json` at compile
  time.
- **Balance numbers** all live in `sim/src/stats.rs`; expect hash churn
  when touching them.
- Keep this file and README.md current when commands or behavior change.

## The bots and the training loop

The shipped opponent is a neural policy embedded in `oxide-sim`
(`sim/src/bot/ladder_weights.json`, Q12 integers evaluated in pure
`i64` — no floats, so neural matches replay bit-identically and the
hash fixtures pin the weights like any other rule). Difficulty is an
execution handicap around one strategic mind: Easy/Medium/Hard/Expert
use cadence 56/36/28/28 and exact hesitation 350/190/5/0 per mille.
Named difficulty no longer feeds the learned skill input. It changes
only cadence and hesitation, so the shipped ladder degrades execution
rather than asking one continuous neural input to order itself. Player-facing
personality resolves once into one of three deterministic variants inside
Turtle, Balanced, or Aggressive, plus a complementary team role. Surprise Me
deals the style and variant from disjoint PCG streams; mirrored hostile seats
share a variant and opposing teams receive the same multiset. Exact
`aggression` remains the zero-facet research/tooling override.

The 12 conditioning inputs are skill, aggression, the seat's actual faction,
a four-way strategy one-hot derived from the aggression quartile, and the
resolved economy/air/siege/support/commitment facets. Authored maps deal even
seats Ferrous; launch-time retints feed the faction knob the seat's ACTUAL
faction rather than sampling it. Scenario seats opt
in via `PlayerSpec.bot_config`; a seat without one gets the legacy
rule-cascade bot, which is what keeps pre-0.7 replays reproducing
(that bot is team-blind by design — team seats must set a config).

The gym contract is v8: 81 named integer features, 12 named conditions,
and 26 masked macro
action ids partitioned into three independent policy heads. Production
chooses one of nine actions, construction/maintenance one of eleven
(including `NoConstruction`), and military operations one of six
(including `NoOperation`). Training samples, PPO scores, and Q12
inference each select one masked action per head; the executive lowers
the resulting `ActionPlan` against one shared budget. Production,
capital investment or repair, and army control therefore no longer
erase each other by competing in one categorical draw.

Construction is an intention before it is a purchase. A selected
capital project persists, reserves its full cost from production, and
waits for both scrap and one free founder; unpaid founding claims keep
the same reservation. The original plan clock survives kind revisions,
and a plan or claim that has not become a paid site after 1,200 ticks is
cancelled. Every capital action then stays masked for another timeout
window, giving production an unreserved chance to spend; blocking only
the expired kind lets an alternating policy reserve the bank forever.
This makes saving observable and trainable without making it permanent.

Salvage remains the 0.11 reclaim-parity action: a cheapest-first lowering
that never touches the Fabricator or Foundry, with own-building value in
the observation and potential so liquidation is not scored as free
reward. The 0.13 weld pair remains in the construction/maintenance
head: `RepairUnit` picks the highest-value wounded own ground machine
with a free Harvester inside a 12-tile leash, while `BuildRepairBay`
uses the ordinary capital planner. The 81 features add the fog-honest
economy, queue, paid-site, health-value, threat, distance, and current
capital-plan state that the factorized policy needs.

The first 0.13 artifact was the v6 widen bridge of the 0.11 weights,
with zero feature columns and unreachable new-action floors. The
from-scratch weld campaign's two candidates were refused by the gate
battery; the incumbent-continuation campaign then recovered the exact
Q12 actor and promoted update 1450 (`fe2b97bdb7058372`) as the v6
baseline. That artifact proved the repair lifecycle and reduced
Scuttler body-time, but it is an ancestor and comparison point, not the
v7 promotion. Episode-aligned continuation first selected update 95,
then the combat-mix continuation selected update 105
(`ca3502efb0a4e845`) as the decisive parent. The promoted v7 source artifact
is that parent plus one selectively revived production row:
`revive.py --actions 3,8 --promote-actions 3` trains the Scuttler and
air-superiority rows together, then restores the air row after the
native cup showed it cost 40 industry-profile wins. That promoted v7
source artifact's digest is
`4473f3e795891915`; its content-addressed training lineage is
`sha256:41de14c644a34fa26597a717cc6f01883529b24325436651120682d55799fe70`.
It displaced its parent only after the complete native-Q12,
faction/seat, composition, repair, ladder, and determinism battery.
The 0.14 v8 bridge appended the five profile columns with exact zero
first-layer weights, so every pre-continuation logit and action stayed
unchanged. Removing those columns reconstructs the promoted v7 digest above
exactly. `tools/train/widen.py` is the explicit v7 artifact and checkpoint
migration; the v8 loader refuses an old external artifact with that command in
its error instead of guessing. The bridge is an ancestor, not the shipped
policy. A full-policy continuation over the Rust-authored named-profile
factorial promoted R2 update 140 (`c36fce50824b9fb5`); its content-addressed
training lineage is
`sha256:3e1df7598e5b1dd1438bc96bad326a4f732b88f416a87176bec4ece15af6090c`.
The current `ladder_weights.json` is that artifact's byte-exact exporter
output. Later profile-only continuations did not clear the promotion battery
and remain research artifacts.

Named-profile identity does not depend on learned logits alone. Strong
authored facets narrow the ordinary action mask around finite opening
milestones at the same decision boundary used by native inference and the
external gym. High economy commits five Harvesters and a Fabricator safety
screen, then a Scuttler, faction anti-air, and Lancer; it also commits a
Reclaimer once nearby salvage falls below 450 and the bank covers its cost
plus 70 scrap. High air commits a Fabricator, both faction air roles, and a
Lancer; high siege commits a Fabricator and Bombard; high support commits one
Turret. Progress latches one-way and counts live or queued units, completed or
paid sites, and deferred founding claims, so a lost commitment never becomes
a standing replacement quota. Recovery, forced home defense or finishing,
and an existing saved capital plan take precedence. Zero facets bypass this
doctrine exactly, preserving R2's raw mask, command, and state-hash path.
`sim/tests/profile_doctrine.rs` pins every finite package and precedence rule;
`sim/tests/bot_profiles.rs` requires every same-style variant pair to change
actual play across a majority of seven fixed seeds (the promoted combination
differs on all seven for every pair), and the driver fixtures hold external-gym
profile masks to the same native decision surface.
Training slots are role-indexed
where the factions differ, so one action space serves both rosters.
Since v4 every
positional feature rides as relative 0-1000 against the actual map
dimensions (fixed scales broke on the large map classes), map dims
ride along (march timing is an absolute-distance skill), and two
fog-safe shell senses report incoming shells near the economy
(impact tile currently visible — the arc renderer's rule) and own
shells in flight. `FEATURE_NAMES`, `CONDITION_NAMES`, and every canonical
style/variant/team-role vector ride in the gym hello. The Python wrapper
validates and consumes that Rust-authored catalog, so feature, condition, or
profile skew dies at handshake instead of in a silently mistrained run.

The weights are a generated artifact with a regeneration ritual, like
the goldens. From `tools/train/` (uv + PyTorch):

```sh
uv run bc.py --arch deep --episodes 48 --out runs/prior.pt   # imitation warm start
uv run league.py --name run --initialize-from runs/prior.pt --anchor runs/prior.pt \
    --collection episodes --maps grand \
    --mix "self=0.35,past=0.15,tier=0.15,rusher=0.10,ffa=0.25"
uv run tournament.py --ckpt runs/run/pool/ckpt-XXXXX.pt      # torch-side eval
uv run export.py --ckpt <winner> --out runs/candidate.json   # Q12 artifact
cargo run -p oxide-driver -- neural-cup --weights runs/candidate.json  # the gate
# ff and cc isolate each roster across both physical seats; fc/cf cross the order
cargo run -p oxide-driver -- neural-cup --weights runs/candidate.json --factions ff
# A narrowly gated row revival can retain only the rows that pass the cup:
uv run revive.py --initialize-from runs/parent.json --actions 3,8 \
    --promote-actions 3 --out runs/revived.pt
```

Checkpoints and exported artifacts created by the current tools carry a
validated, content-addressed lineage manifest. Input identities are SHA-256
digests rather than paths, and the lineage id covers the phase start, material
hyperparameters, trainer/model/gym-client code, the exact driver binary,
consumed scenario or generated-map inputs, and initializer/anchor/incumbent
contents.
Moving an input preserves the id; changing its bytes changes the id. Loaders
reject a tampered manifest, while legacy checkpoints and artifacts without one
remain readable. QuantNet's gameplay digest deliberately ignores the manifest,
so adding provenance cannot alter a replay.
Every league phase also owns a fresh, write-once `runs/<name>` directory.
Nonempty names are refused before workers launch, and the log is created
exclusively; continuing from a checkpoint means a new phase name, not appending
new settings and stale pool members under the parent's identity. Pool, eval,
and probe intervals run on the phase-relative clock, and the phase's final
checkpoint is always saved even when its length is not interval-aligned.
Generated-map caches are namespaced by the validator binary and re-check their
deterministic retry candidate before reuse. Checkpoints record whether their
critic is ready: behavior-cloning and exact-Q12 checkpoints warm the critic
before PPO moves the actor, selective revival preserves its source readiness,
and a trained league parent does not pay that warm-up again. Exact Q12 recovery
preserves a manifest because the actor is unchanged, while action unflooring
and v6→v7 widening derive new transformation lineages.

`--weights` loads a file the sim did not write, so `QuantNet::from_json`
is a trust boundary, not just a shape check: every tensor carries a
magnitude ceiling (recip in `1..=2^26`, `|w|`/`|b|` <= 2^20, `|lut|` <=
2^13, <=16 layers of <=4096 width), and those ceilings are what make the
`i64` kernel's accumulator bound provable — see the derivation in
`sim/src/bot/neural.rs`. They sit orders of magnitude above anything
`export.py` can structurally emit, which mirrors them so a drifting
architecture fails with the checkpoint in hand rather than at promotion
time. Every artifact also carries a `digest()` — FNV over its parsed
tensors and contract fields, blind to reformatting and to the
`arch`/`update` metadata — and `balance-probe` and `neural-cup` print it
on every result, so a composition table or a cup line pasted into
`experiments/` answers "which weights" on its own. The cup's optional
`--factions ff|fc|cf|cc` retints a two-seat scenario before build and
reports both the ordered pair and per-seat results; omission preserves
the scenario's authored rosters. Use `ff` and `cc` to measure one roster
across both physical seats, then `fc` and `cf` to expose roster-order
interactions.

Hard-won rules encoded in that stack: pick checkpoints by tournament,
never recency — leagues are checkpoint farms with a shelf life
(ladder-facing quality peaks, then league inbreeding takes over;
drift arrives 150-400 updates past the peak, and the rusher eval
going soft is the earliest canary; anchored mixes — scripted share
around 0.45, KL anchor-coef 0.1 — widen the harvest window but never
cure the drift); masks are part of the trained distribution (widening
one feeds untrained logits to the blunder picker) while action
*lowering* absorbs lifecycle races; `decision()` previews the
executive's reconciliation on a throwaway clone so observations match
what lowering will see; never build a reward out of what the agent
happens to know about the enemy — under fog, "known" is an
information artifact, and shaping on it teaches blindness; and
teammate skill is bought with duel sharpness unless consolidated from
an already-strong parent (the 0.9 artifact's lineage: v4 BC bridge →
league peak ckpt-750 → anchored team consolidation ckpt-875, gated
1200/1200 with zero draws; the 0.8 lineage read the same way). A
continuation's KL anchor anneals on the current training phase's clock,
starting at the requested coefficient even when the imported checkpoint
already has a large absolute update number. (`--anchor-decay 1.0` holds it
constant — the style-retention setting.) The 0.10 campaign added two more:
`--tech-bonus/--tech-anneal`
pay a fog-safe own-tech terminal bonus (annealed on the RUN's clock,
not the absolute one) that seeds the tech tree, and `--maps grand`
draws the 1v1 lanes from the large/vast classes only — the decisive
lever, because on mostly-small maps games end before a Fabricator
amortizes and PPO grinds imitation-taught tech back out the moment
the bonus fades; on the grand distribution the true objective
sustains it unaided. The 0.11 campaign added the general forms:
`widen.py` bridges an old artifact to a new contract twice over (the
shipped json gets zero feature columns and an UNREACHABLE new-action
floor so every fixture stays green; the float resume gets a
reachable zero bias, because a verb PPO can never sample is a verb
it never learns), `--salvage-bonus` seeds a new verb on the
tech-bonus schedule, and the decisive lesson: a long fresh league
from a converged parent only dilutes it — the working shape is a
SHORT consolidation resumed from and anchored to the intact parent
(coef 0.1 held flat), picked by tournament inside the anneal's
shadow before the rusher canary collapses. A seeded verb the true
objective still prices as lossy ships as trained runner-up logits,
not a usage quota — forcing usage past the game's own economics is
the "weird ML" line the campaign doctrine refuses to cross. The 0.13
contract stage extends both instruments: one `--repair-bonus` seeds
the weld pair introduced by v6 and retained in v7 on the tech-bonus
schedule (actual field-welded hp and a completed Repair Bay each earn
it once per episode — sampled actions and unfinished scaffolds earn
nothing). The gym carries those successful effects beside, never
inside, the 81 policy features, so the trainer can distinguish work
from intent without leaking reward bookkeeping into inference. Accepted unit
heals emit an output-only `UnitRepaired` event tagged `FieldWelder` or
`RepairBay`; the gym sums only the former for weld reward, instead of guessing
from static Bay coverage. Events do not enter state, saves, or replay hashes.
`--structure-bonus` applies the same annealed, once-per-kind
seeding to completed Turrets and Arrays, capped at 0.02 each; and
`--probe-every` plays
the fun gate's composition probe in-loop — every N updates the
league snapshots the checkpoint, exports it to Q12, and runs
`balance-probe` over the shipped slate, logging decisiveness,
per-seat entropy p10, and unit AND building shares beside the rusher
eval (observed, never rewarded; snapshot, artifact, and raw payload
land under `runs/<name>/probe/` for post-hoc reading). The 0.13
campaign wrote the doctrine's converse in numbers: a from-scratch
league under moved physics farms tier-sweeping STRENGTH in one
campaign, but decisiveness — the will to end games — has been
consolidated across every era since 0.7 and did not re-derive from
scratch (the candidate stalled 17 of 48 sweep games the incumbent
decides); the canaries called that verdict from the first in-loop
probe (composition p10 never near the shipped floor, the prime eval
pinned seat-sided), so read them as a forecast, not a formality,
and prefer a decisive parent's bridged lineage over a fresh prior
even when the physics gap argues for starting over. The endgame's
second round added the corollary: one more era DESCENDED from the
refused winner came back within noise on every gated axis —
descending from an indecisive lineage is not the bridged-lineage
lever. The incumbent-continuation campaign finally spent that lever:
recover the shipped Q12 actor exactly, give the float bridge reachable
new-action floors, warm only the reconstructed critic, then use a
short fixed-anchor consolidation with all faction/seat pairings. Pay
seed bonuses on successful effects, never sampled intent; save densely
around the first whole-gate boundary; and keep the unmodified
checkpoint when output-row or bias surgery merely moves failures
between gates. When a final executive correction changes what a
successful verb accomplishes, run one more short anchored continuation
under that exact lifecycle: the first repaired parent lost a Veteran
cup cell after healed units became draftable, while twelve conservative
updates restored it, closed repair reach to 8/8 cases, and improved FFA
and team play. The tier-heavier sibling was more diverse and twice as
strong against Prime, but its mixed-roster Ferrous marginal fell to
41.8% and it lost the paired FFA comparison; whole-game factors outrank
an isolated strength axis.

The v7 strategy audit found that the old single categorical action was
itself a bottleneck: a bot could produce, build, maintain, or operate an
army, but never express all three decisions in one think. The
factorized heads and persistent capital planner fix that representation
before asking PPO to rediscover strategy. Updates 80 and 95 came from
episode-aligned continuations of that bridge; update 95 won its
whole-game battery, not a recency comparison. Update 105 then improved
the combat mix without changing the model topology. Broad behavior
distillation and production-entropy continuation both collapsed useful
specialists. The narrow alternative collected corrected-teacher states
and optimized only two output rows with held-episode retention; the
native cup accepted the Scuttler row and rejected the air-superiority
row. Selective output work is therefore a downstream-gated surgical
tool, not a license to score action quotas. Checkpoint interpolation and
targeted hard-opponent continuation merely moved faction/seat failures
between gates and were rejected. The stall audit added one more rule: a
cap is not a dead
economy. Inspect live Harvesters, queued replacements, Reclaimers,
remaining map scrap, recent production, recent combat, and last roster
movement before prescribing more income.

The training distribution must match the named runtime wrapper explicitly.
The v7 ladder's learned skill response is not monotone, so sampling a scalar
skill and deriving hesitation from it trained a relationship the shipped bot
does not use. The v8 default curriculum consumes the Rust handshake catalog and
shuffles an exact 36-cell factorial: all nine named variants crossed with Easy,
Medium, Hard, and Expert execution profiles. Team lanes sample distinct
Rust-authored specialist-role vectors; solo and FFA lanes use Generalist.
`--style-coef` is the bounded seeding instrument for those new columns:
named profiles score only fog-safe own-state economy, air and siege mix,
support infrastructure, and army commitment against their five authored
facets, averaged over the episode and capped at 0.1 total reward. Raw
aggression experiments retain the old commitment-only posture signal.
This teaches a conditioning contract rather than an action quota; promotion
still requires the unshaped outcome and composition gates.
Custom aggression distributions stay zero-facet experiments with a separate
four-profile execution cycle. Every sampled cadence reaches `Worker.reset`,
and PPO receives the exact hesitation value rather than deriving one from
policy skill. Frozen past/incumbent opponents mirror the learner's named
vector under their own faction, and in-loop evaluation rotates through every
canonical variant. `tournament.py` defaults to a deterministic rotation through
the Rust canonical profile catalog; explicit `--skill` or `--aggression`
selects the zero-facet research path, while `--hesitation` and `--cadence`
control execution.
Passing skill as hesitation makes a full-skill checkpoint idle every window;
the native Q12 cup never shared that torch-side failure mode.

Team training runs two flavors — self-team (`team`: the learner holds
both chairs) and mixed-ally (`team2`: a scripted Brain drives the
teammate). Current PPO campaigns use `--collection episodes`: every
worker resets once, holds one policy for one complete variable-length
episode, and never splices a freshly reset match into the same rollout.
Short lanes pad to the longest lane with invalid rows. A dead learner's
lane likewise stays on its frozen last view while the teammate plays
on; all padded rows are masked out of the PPO loss, while GAE still
spans the frozen prefix so the team payoff reaches decisions made while
that seat was alive. An artificial tick cap arrives as
`truncated=true`: a living seat receives neutral outcome reward and
retains both the terminal potential and critic bootstrap before the
done mask cuts the reset. A true terminal remains absorbing, and an
eliminated seat receives no bootstrap. The scripted `Brain` tiers and the rush teacher
stay in-tree as league anchors, benchmarks, and the ladder-integrity
yardstick (`sim/tests/neural_ladder.rs` enforces Easy < Medium <
Hard < Expert forever).

### Balance instruments

`driver balance-probe` runs bot-vs-bot across the shipped maps
(optionally `--weights` for a candidate artifact — the fun gate's
mechanical form). Embedded and candidate policies consume the same
scenario-resolved named style, variant, and team role unless a raw override
is explicit. The probe reports both cost-weighted and body-time-weighted
composition, headed by the probed artifact's digest. The distinction is
load-bearing: cheap Scuttlers can dominate army presence over a match
while a few expensive specialists make army VALUE look varied. Since 0.13 every
record states the terms it was measured under — result, capped, winners,
last combat/economy/progress ticks, and final economy state (remaining
map salvage plus each seat's bank, live/queued Harvesters, Reclaimers,
Foundries, and resignation state). Match duration is not the verdict: a long active war is
healthy, while a cap with no meaningful activity for 2,000 ticks is an
unhealthy tail. Resource exhaustion is reported separately when that
inactive cap also has no map salvage and no active recovery-income route.

The historical all-unit value and body-time shares remain in every
record as all-time economy diagnostics, including Harvesters and
autonomous remnants. Parallel `combat_*` fields exclude units without
weapons and integrate a seat only while it is non-resigned and holds a
living completed Foundry. A losing seat's whole pre-defeat strategy
therefore counts, while its post-elimination remnants do not. Finished BUILDINGS ride beside the unit shares (distinct completed
buildings per kind per seat): a roster that never stands a Fabricator
never had the advanced kinds to decline. Both unit lenses publish
entropy-of-the-mean and the per-seat entropy distribution. The gate
uses the lower quartile plus direct rates from the raw seat arrays:
small-sample `p10` and `p90` jumps were too volatile to express the
actual catastrophic-tail limit. Everything folds through one seat-level cohort primitive
(`composition::aggregate_by` with ready-made faction / map-class /
decided-vs-capped / per-map keys), and the matches fan out over the
shared pool.

Schema 7's `--out` record carries `overall`, diagnostic `decided`, raw
matches with activity/economy evidence, the ordinary cohort tables, and the
explicitly selected named style and variant alongside the legacy aggression
field. The gate reads only `overall`'s competitive-lifetime `combat_*` fields;
the unprefixed all-unit fields are diagnostic compatibility data. A production
line is not a second combat kind. `tools/train/fun_gate.py` requires exactly
schema 7 and reads the payload by key; a driver test pins every field it
consumes.

Promotion runs at least three seeds over the shipped personality deal,
Turtle variant 1 (`industrial-attrition`), and Balanced variant 1
(`air-combined`). All three must clear the composition and tech gates. The
dealt profile also owes structure reach and cap health; Industrial Attrition
owes at least 25% competitive-lifetime Reclaimer reach, and Air Combined owes
at least 13% of competitive army value in faction-appropriate air units. More
than 10% unhealthy caps fails, but an active 40k war does not. Value/body
entropy must reach 2.00/1.95 bits
and their per-seat p25 values 1.35/1.25. At most 7.5% of seats may fall
below 0.75 value entropy or 0.65 body entropy, at most 10% may spend over
80% of body-time on one kind, and the leading mean body share is capped
at 50%. `--baseline-weights` optionally adds a same-map/seed regression
envelope: at most 0.10 mean-entropy loss, 0.15 p25 loss, or five-point
worsening in a catastrophic-tail rate or the leading body share.
Promotion tables captured under earlier winner-only or all-unit
contracts are not comparable and must be re-probed under schema 7.
The gate also enforces two tech rules with distinct
thresholds: `--min-tech-share` (0.45) on the SUM over the
Fabricator-gated kinds asks whether the tree was climbed at all, and
`--min-top-tech-share` (0.15) on the LARGEST single tech kind asks
whether anything on it was worth building — many individually
negligible kinds can clear the first bar and still fail the second.
Four independent competitive-lifetime reach floors keep a
no-construction policy from passing on unit mix alone: Fabricator 90%,
Turret 40%, Array 60%, and Reclaimer 25%. Repair Bay has no reach quota;
it is intentionally niche, and field repair remains the responsibility
of `repair-probe`.
`driver repair-probe --weights <artifact>` complements those broad
matches with eight deterministic wounded-state cases crossing seed,
physical seat, and faction. It reports RepairUnit commands, Repair Bay
attempts/completions, damaged purchase value, and observed healing,
alongside the artifact digest. This is a diagnostic, never a reward:
the A1450 actor's original exact-zero use and healing was the baseline
the v7 campaign had to move.
`driver matchup --a kind:n --b kind:n` fights
hand-picked armies twice on a clean arena, swapping their physical seats;
use comparable starting costs when testing counters. It reports each leg's
completion status and survivor purchase value, plus the paired mean.
Both seats wear ONE roster by default, so the leg swap exchanges seat,
geometry and initial ID range and nothing else; `--factions ff|cc|fc|cf`
(west then east) splits them when the roster is itself the experiment.
The arena trains nothing, so a seat's faction selects no unit stat —
same-faction seating removes a label from the swap's bundle without
moving a number. A wound-discounted survivor value (sum of
`cost · hp / max_hp`) rides beside the purchase value and never enters
the verdict: changing the verdict's input would silently restate every
arena conclusion already recorded.
`--b-structures turret:n` is defense mode: pre-built structures
stand in front of side B, priced into its verdict — scenarios grew a
serde-default `buildings` list of pre-built structures for exactly
this kind of harness work. `--garrison-pitch` (default 3) refills the
wall's fixed band more or less densely and must clear the widest
structure standing in it.
`driver sweep` (0.12) is the decisiveness instrument: N seeds of
bot-vs-bot on one 1v1 map at one level, each seed played in both
personality orientations (the dealt pair exchanged between the
seats), reporting decided/undecided counts, seat bias that survives
the exchange, and decision-tick medians — where balance-probe asks
what armies were made of, sweep asks whether games END. The 0.12 bot
phases gate on it. Its siblings: `driver duel --a <level> --b <level>`
fights two ladder profiles (candidate `--a-skill/--a-cadence` dials
included) seat-swapped across seeds, and `driver yardstick --level
<level> [--skill --cadence]` measures one profile against all four
scripted tiers over as many seeds as recalibration wants — the
doctrinal strength instrument, since neural head-to-heads reward
patience and stopped ordering the ladder in 0.10. The yardstick
reports pace, not just record: per tier, the median and p75 tick of
the profile's own victories beside the unresolved count, because two
rungs with the same count separate on how fast they close. `--dir
<scenarios>` (mutually exclusive with `--scenario`) runs the whole
1v1 slate on one pool and pools the tier records from the raw
matches — the ladder is gated on skirmish alone and duration
distributions vary by an order of magnitude across the roster. The
in-tree gate prints the same shape: `cargo test -p oxide-sim --test
neural_ladder -- --nocapture` lays out every rung's (wins, tick
total) instead of leaving it to an assertion message. All four share one
fan-out pool (`driver/src/pool.rs`) — job list in, results back in job
order, so no verdict depends on the thread count.
`driver sweep-factorial` (0.13) finishes what `sweep` starts:
everything the shipped game binds to the SEAT INDEX, permuted as a
full cross product on one seed set — the dealt personality pair, each
seat's roster (`Scenario::retint_seat`), which seat's units claim the
low unit-id range, which seat's commands land first in the tick's
command slice, whether the map plays as authored or rotated 180
degrees, and whether a seat's hesitation rng runs on its own stream
(`NeuralBot::with_profile_stream`, the additive constructor that
exists for exactly this and defaults every shipped path to
`DECISION_STREAM_BASE + seat`). Six factors, 128 cells; `--factors`
cuts the design down. The rotation asserts the map's terrain is
exactly 180-symmetric and refuses otherwise — a Foundry anchor names
the top-left of its footprint, so it moves a footprint in, not a tile,
and a silently mis-rotated map would make every verdict a verdict
about terrain. The report gives per-factor marginals on seat 0's share
of victories with 95% Wilson intervals, decision-tick quartiles, the
censored share per cell, and the WHOLE cell table, because the
interactions are the finding — the same-roster mirrors lean opposite
ways and any average erases that. The all-baseline cell is the shipped
game bit for bit on maps whose authored unit lists are seat-grouped
(pinned by test against `runner::step` + `seat_bots` on skirmish);
on the three 1v1 maps that author interleaved lists (meridian-scar,
open-circuit, slagline) BOTH spawn levels re-group ids, so the
comparison stays controlled but the baseline cell is not the authored
id order there. Intervals are unpaired and therefore conservative for
low-effect factors — the cell table and the hash-divergence counts
are the paired evidence; a paired statistic is a campaign-era
follow-up. First reading, skirmish Medium at 128 cells x 4 seeds: the
bundle is almost entirely ROSTER (ferrous 22.7% [18.0, 28.2] of
mixed-roster victories) plus a live geometry term (seat 0 takes 45.6%
authored, 73.2% rotated); id range, command order and rng stream all
sit inside their intervals, and command order changed 10 of 256 paired
matches bit-for-bit while flipping not one outcome. No balance-number
edit ships without a before/after run on the same seed set — the 0.13
roster rebalance was the first customer (ferrous 21.5% → 48.5% at 24
seeds on seed base 7000), and the geometry term outlived the bless on
both sides (44.8/59.6 before, 40.3/54.5 after): geometry is not
roster, and a probe reading must never bill one to the other.
`driver pace-sweep --dir scenarios --level <l> --seeds N` (0.13) is
the clock: `sweep` run over every 1v1 map in a directory and tabled
per map as decided/undecided, censored percent, and decision-tick
p25/median/p75 in ticks AND mm:ss, beside the map's declared `pace`
and its audited ground route. `pace` is a claim about map SCALE —
`map_gates.rs` bands it on Foundry-to-Foundry route length and
nothing else measured how long a match actually runs. It does now,
and the two are only loosely correlated: at Medium the 15-step
Scrapyard Brawl closes in 4:12 while the 31-step Skirmish Basin takes
9:40, and inside one label Ferric Reach (large, 65) reads 20:01
against Cinder Steppe (large, 85) at 12:23. Each row is exactly what
`driver sweep --scenario <map>` reports at the same dials — the rows
run one after another, each fanning its own matches, so a surprising
row is reproducible with one command. Measurement only: the medians
move with every artifact generation and every balance bless, so
nothing gates on them. The browser's duration badge is authored FROM
this instrument: `ScenarioMeta.duration` on every 1v1 map carries the
p25-p75 decision window (Medium, 12 seeds x 2 orientations, 60k cap,
seed base 7000) rounded outward to minutes, drawn beside the
geometric `pace` label it qualifies. The bands are artifact-stamped
measurements, never gates. The shipped v8 R2 update-140 slate on the final
0.14 simulation decided 318/384 matches; its 66 capped games remain censored
and do not enter the quartiles. Re-run and re-stamp the bands after any later
weights or balance bless that moves the clock. Meta never reaches
`State::assemble`, so the stamping moved no hash fixture — proven
unblessed.
`driver bench` times a 500-unit mass battle locally
(`--scenario scenarios/compass-grand.json` instead runs a shipped map
with EVERY chair converted to a thinking Expert — the heaviest honest
shape; the earlier 3,073 ticks/s figure benched seven minds around an
idle authored human seat, the 5,044 figure averaged free post-victory
ticks, and since 0.13 the timed loop stops at the decision — the
honest eight-mind live-game number is 4,530 ticks/s (~225x realtime)
since the tick's spatial index took over collision pair-finding and
target acquisition and vision's memory reconciliation fused to one
walk; the 500-unit mass battle reads 12,600 — so no perf window is
open). The refused optimizations are on the record too: the
tile-to-building index and the span-scoped vision clear were both
REFUSED by the 2026-07 profile — `State::passable` is ~8% of the
post-index tick with buildings peaking in the teens (the index pays
past ~60 standing buildings, where A* expansion is the real cost),
and vision's span stamping dwarfs the clear the scoped variant would
remove (revisit past 124x78 maps or more than 2 teams). Hit those
numbers before re-proposing either. CI asserts only hash-identity at scale. The 0.10 pacing findings and levers live in
`experiments/` (the per-era lab notebook — its README indexes the
campaigns); matches target tens of minutes (the `vast` map class
and the foundry-durability bless exist for this; the lancer's
damage bless is what made the tech tree worth climbing — the matchup
instrument condemned the old rail at true par cost).

`driver shots` is the perceptual-diff screenshot suite: twelve
canonical screens from a spawned automation shell (throwaway HOME, reduced
motion pinned so the Home backdrop can't drift), compared against
per-machine references in the gitignored `shots/` directory on a mean
per-channel metric. The default threshold is calibrated between font
AA jitter (<=0.003%) and a small UI element appearing (~0.02%).
`--bless` adopts the current captures after an intended visual change;
a compare run FAILS on a missing reference rather than silently
adopting it, so a fresh machine's first run is twelve failures and a
prompt to bless — expected, not drift. The automation shell runs from
a scratch cwd, so the replay-shelf capture is always an empty shelf
(local replays can no longer shift it). Local gate only — pixel
comparisons don't survive GPU churn, so CI never runs it.

## Design decisions worth knowing

- **`State` fields are private; `State::tick` is the only mutator.** Read
  through the accessors (`units()`, `buildings()`, `players()`, `map()`,
  `current_tick()`, `result()`, `vision(id)`, `hash()`). If new code needs
  a view the accessors can't give, add an accessor — never a `pub` field.
- **Deserialization is the sim's trust boundary.**
  `State::validate_invariants` runs inside `State`'s `Deserialize` impl
  and there is no unvalidated constructor, so a snapshot that parses is
  one the tick pipeline, the renderers, and the gym may assume whole:
  map bounds, entity owners and references, hp/meter/cooldown/queue
  bounds, the salvage ledger's exact arithmetic, canonical ghost and
  radar order, and a coordinate envelope — which is what *licenses*
  every unchecked `+` in `TilePos::offset` and `Building::contains`.
  Two rules stay deliberately permissive, because tightening them would
  refuse states the sim really produces: references are checked against
  the id counters, never the live tables (an order or a shell outliving
  its subject by a tick is ordinary), and the envelopes are generous
  sanity boxes rather than map-relative bounds (collision does shove a
  body a fraction past the border). Every new `State` field owes a row
  there and a fixture in `sim/tests/state_integrity.rs`, whose other
  half is the bring-up gate — every shipped map played out bot-vs-bot
  and round-tripped through the deserializer, plus a scripted verb run
  checked every tick, so a row tighter than reality fails there instead
  of eating a save.
- **Ranged fire traces line of sight** (`chassis::path::line_blocked`, a
  fixed-point supercover walk): rock blocks; buildings never do (since
  0.13 — they block movement, not bullets; built and unbuilt alike, so
  a dropped foundation buys no instant cover and a turret's own base
  can't blind it); scrap and units don't block, endpoints never do. In
  range but blocked → keep approaching until range *and* line hold.
  Vision stays radius-based on purpose — cover is a firing rule, not a
  stealth system, and the only cover is terrain. The trace
  saturates on hairline deltas (a 1-ulp segment once overflowed the
  1/Δ step math); direction symmetry A→B vs B→A is *not* promised
  (corner-graze rounding differs), mirror fairness *is* — a lattice
  test pins it, so don't "fix" the asymmetry by canonicalizing
  endpoints.
- **Movement feel is tuned, not emergent** (`sim/src/stats.rs`): waypoints
  accept within `WAYPOINT_ACCEPT` (corner-safe), arrival propagates through
  contact with settled neighbors near a shared goal (`ARRIVAL_NEAR`), group
  orders fan out over a deterministic ring of per-unit goals, anchored
  workers take `ANCHORED_PUSH_SHARE` of pair separation so crowds flow
  around them, and collision applies pairs Gauss-Seidel-style in id order
  under one per-unit correction budget shared by every relaxation pass in
  the tick — symmetric cancellation once froze the whole economy, while
  resetting the cap per pass made packed bodies visibly burst outward.
- **Moving bodies slide, parked bodies push (0.12).** `movement::run`
  hands its per-tick displacement to the collision resolver; a body
  that traveled INTO a contact takes its correction as
  `SLIDE_RADIAL_SHARE · away + SLIDE_LATERAL_SHARE · sideways`, the
  side picked toward its own travel (geometric, 180°-equivariant;
  head-on pairs provably pick opposite world sides). Pure radial push
  survives for parked, non-closing, and perfectly stacked bodies, so
  the settle probe holds by construction. A slide the terrain rejects
  drops the lateral against a head-on partner — never reverses into
  the partner's side, which would wall a corridor pair back into the
  freeze. Before the slide, a collinear head-on pair froze PERMANENTLY
  (radial pushback exactly cancels path speed) and army movement
  averaged 82% of nominal; after, the lab's head-on pair passes at
  exactly solo time and a 20-unit assault reaches 20/20 concurrent
  contact. `sim/tests/movement_lab.rs` is the instrument.
- **Orders are programs since 0.5**: every unit carries a bounded queue
  plus a looping flag; completion pops (or rotates — that's patrol),
  stalls drop the whole program with `OrderStalled`, plain orders replace
  it wholesale — except that reissuing the unit's *exact current* order
  is a no-op past the queue wipe, keeping path and progress (the
  scripted tiers re-command every think; repair billing counts on the
  meter surviving). Patrol legs are attack-moves and never settle.
  A command's unit list is read as a SET: dispatch sorts it and folds
  repeats away before any handler sees it, so a hand-forged
  `--units 3,3,3` queues one leg, not three. The recorded command keeps
  the client's bytes — the set is an interpretation rule, not a rewrite.
- **Combat resolves simultaneously since 0.6**: brains decide in id
  order (direction alternating by tick parity), but every shot is
  buffered and applied only after all brains and turrets have acted —
  everyone decides against the same start-of-tick world, identical
  opponents annihilate mutually, and no seat gets a reaction edge from
  id order. Bots think on the same tick for the same reason.
- **Damage answers back**: a hit unit that can fight and isn't already
  fighting turns on its attacker (units *and* turrets) — the counter to
  range beyond aggro. Inside aggro, auto-acquire already covered it.
  Retaliation resolves after all damage, in decision order: the earliest
  surviving attacker gets the answer.
- **Construction claims ground instantly — and bodies WALK off (0.13)**:
  full price on placement (refused — and refunded nothing — if no
  doorstep is reachable), a fifth of max hp standing, blind and inert
  until built. Since 0.12 friendly machines never block placement; a
  visible HOSTILE machine still denies its tile. Displaced bodies are
  no longer relocated instantly: the builder's own approach routes it
  to a doorstep (A* tolerates a blocked start), and every other
  friendly on the footprint (allies included) is walked off by a
  phase-5 eviction pre-pass — pathless ground bodies on claimed
  ground get a path to the nearest routable open tile each tick,
  path ONLY, so programs survive and an extracting harvester keeps
  its job while it clears the ground. Only a body with NO escape
  route takes the old instant perimeter deal (nothing may end up
  inside a finished building), and the crew draft plus that
  last-resort deal run strictly after the last rejection path so a
  refused command leaves no trace on the hash. Ground closing
  mid-walk is real: movement revalidates each waypoint and repaths
  around fresh sites. Progress needs an adjacent builder — **several
  adjacent builders stack**, each contributing a tick, so two roughly
  halve the build; deliberate, tested — and since 0.13 a
  multi-harvester Build commits the whole accepted crew, fresh and
  resume alike: the first accepted harvester founds (pays, proves the
  doorstep, and alone gates the command), the rest join best-effort.
  Orphaned sites freeze and any own
  harvester can resume them; a site zeroed by fire is dead even if its
  builder acts the same tick — construction hp-gains buffer like damage
  and resolve after it, so fire wins ties. Cancel (`X`) refunds
  `cost × hp / max_hp`. The placement doctrine (0.13): a placement
  verdict may only read facts the issuer knows — static terrain, own
  memory, own and allied entities. Current visibility is how the STRICT
  predicate (`State::can_place`, the final word on every actual ground
  claim) earns the right to read live occupancy; its sibling
  `State::place_intent_refusal` earns it differently — visible tiles
  take the live checks verbatim, explored-but-unseen tiles are judged
  from memory (static terrain, remembered scrap and ghosts, own/ally
  live buildings, own pending founds — never live hostiles), and the
  claim re-proves the strict predicate at arrival. That is the deferred
  build (`Command::Build { defer }` → `Order::Found`): nothing placed,
  nothing charged, no route demanded at accept; the crew walks out and
  the founder claims through the same `found_site` tail the instant
  path uses, in id order after the volley. Ground taken meanwhile
  stalls fog-safe (`StallReason::GroundTaken`, judged only on tiles the
  arriving founder sees — the arrival re-check also catches the one
  memory-proof collision, an allied scaffold on unseen ground, since
  unbuilt sites cast no vision); with nothing spent, Stop is the
  cancel. The shell emits `defer` for the human (amber ghost on
  remembered ground, tinted from the intent predicate, never live
  state), and since Part B the gym bot lowers a Build the same way —
  defer exactly when some footprint tile is out of current sight
  (`LoweringRules::gym`, the reclaim-parity rule). A walking founder
  is spoken for: the site audit waits on it instead of blacklisting
  its anchor, and the labor choosers and scout pick keep off it. The
  gym Scout arm also honors the think's claims (the labor-claims
  trap's other half). The scripted `Brain` tiers keep the strict
  instant claim and the unconditional Scout arm deliberately — they
  are the ladder's frozen anchors, and the one-line shared Scout
  guard measurably inverts both ladder gates — pinned byte-identical
  across the change. The bot-side mode leaves the tick-2000 hash
  fixtures untouched (proven unblessed); it shows at longer horizons
  on the fogged maps, where remembered-node turrets found instead of
  bouncing.
- **Fog of war enforces exactly one thing in the sim**: targeted attacks
  need the issuer to *see* the victim. Rendering honors fog fully
  (unexplored void, explored dim, unseen enemies culled) but the debug
  surface — `query_state`, the F1 overlay, the software renderer — is
  deliberately omniscient. The legacy Classic bot reads full state; the
  shipped neural/Gym path builds a fog-honest observation from team sight,
  memory, and radar. Every bot's commands still pass normal validation.
- **Units are solid but never block tiles.** Collision is iterative pair
  relaxation after movement; pathfinding ignores units entirely, so crowds
  jostle but can't deadlock a corridor the way tile-reservation schemes do.
- **Movement has three explicit stances (0.14), and stationed guards fight
  on a tether (0.12).** The shell's default ground right-click issues
  `Advance`: combat units keep their path and take primary-weapon shots
  only when a visible target is already in range and line of fire. They
  never stop, chase, or retaliate; pacifists degrade to `Move`. `F` (and
  the touchable panel card) arms the committed `AttackMove`: units engage
  in aggro range, fight via `Order::Attack { resume: Some(goal) }`, and
  pick the march back up.
  Idle units auto-acquire, and a machine that stood
  `LEASH_STATION_TICKS` first acquires on a leash: free hunting
  inside `LEASH_RADIUS` of its anchor (kept ≥ the Bombard's reach so
  siege stays answerable — pinned by test), a `LEASH_PATIENCE`
  warm-blood window beyond it that only a joined fight refreshes (a
  bait never in reach grants none — the kited picket breaks at the
  radius line), then the walk home and a re-acquire cooldown at the
  post. Victories stand their ground still stationed; only break-offs
  walk home (walking home mid-battle measurably lost rush defenses).
  A unit cycling through idle mid-battle hunts unleashed exactly as
  before — tethering those collapsed the scripted tier ladder to a
  seat-parity coin. Player commands are commitments: an explicit
  attack never tethers, and `assign` clears any leash unconditionally,
  no-op reissues included. Plain `Move` never fires and stays the player's
  **Run** verb (`M`, panel card beside Attack-move and Patrol). `Advance`
  is deliberately absent from retaliation's eligible orders, while
  `AttackMove` retains its destination as the retaliation resume.
  `sim/tests/behavior_leash.rs` pins the contract.
- **Ghost memory lives in `Vision`**: enemy-building records refresh while
  their ground is visible and freeze when sight is lost; seeing the ground
  empty erases them. Scrap amounts get the same treatment via a per-player
  remembered grid. Renderers draw live state on visible ground, memories
  elsewhere — same rule on the minimap.
- **Sound follows sight.** Positional clips require the event's tile to be
  visible to the human; own losses and milestones are always audible. The
  queue is dropped after `advance_ticks` bulk jumps, and a per-kind rate
  limiter keeps battles from clipping into noise. Since 0.9 sounds carry
  a world position and attenuate with camera distance (volume only —
  macroquad has no pan). One deliberate bend: a hostile artillery launch
  whose muzzle is fogged still plays, anchored at its IMPACT point — the
  warning survives, loudest when shells fall on you, and nothing about
  the sound tracks the hidden gun. That is the same information boundary
  the gym's incoming-shell sense draws (impact tile visible, never the
  launch), and the arc renderer clips hostile trails the same way.
- **Rally points are role-aware**: a rallied scrap node sends fresh
  harvesters straight to `Harvest`; combat units attack-move to the rally;
  the goal snaps at spawn time, not set time. Whether the rally counts as
  "a node" is judged by the owner's *remembered* scrap, like harvest
  validation — rallies can't probe unexplored ground.
- **Eliminated players leave autonomous remnants — by design.** Losing
  your last Foundry rejects your future commands, but units already in
  the world keep executing their brains (idle ones still auto-acquire).
  Masterless machines finishing their last orders fit the fiction; in a
  team game a foundry-less seat spectates while its team plays on (the
  victory check is team-scoped, the command gate stays player-scoped —
  the two sites cross-reference each other on purpose). Surrender
  (0.13) is the same doctrine chosen voluntarily: `Command::Surrender`
  sets `Player.resigned` — a first-class hashed fact, not a macro for
  razing the base — the seat's Foundries stop counting toward the
  victory check (a fully-resigned team is eliminated on the spot; a
  1v1 concession decides its own tick), its future commands reject as
  `Eliminated` (which makes a second Surrender a no-op), and its
  machines play out as remnants. Gym bots concede only from the terminal
  recovery state: a critically wounded Foundry under visible pressure with
  no viable economy or defensive package. Ordinary losing positions still
  play out.
- **Air is a second movement domain, not a special case.** Flyers take
  the straight line (no A*), ignore terrain, construction claims, and
  ground collision, collide only with each other, never block
  foundations, and accept any on-map tile as a goal — rock included,
  peaks excluded (goals ring-snap off them). Group orders split by
  domain so each half routes sensibly. Terrain cover (the rock LOS
  rule) is ground-vs-ground only; peaks are the exception below. A ground chaser
  whose flying victim parks over impassable ground marches to a
  stand-in instead: ring-scanned candidates filtered to the weapon's
  Euclidean reach (ring corners sit √2 past their Chebyshev radius),
  first routeable one wins — reaching weapon range is the job;
  occupying the victim's tile never was. No candidate in reach stalls
  the order honestly.
- **Peaks (`^`) are the plated exclusion barrier nothing crosses.** A third terrain:
  blocks ground movement (like rock), blocks air (the one thing the
  sky routes around — flyers A* over air passability when a peak
  breaks their straight line), blocks direct fire in every domain
  pairing, and breaks Bombard/Bastion arcs — siege-safe geography.
  Vision deliberately ignores peaks (cover is a firing rule, not a
  stealth system). Wrecks never land on them; `known_rock` in the bot
  observation includes them as known impassable terrain. Authoring:
  the map border is rock and rock is open sky, so a ridge meant to
  wall flyers must claim its border cells too; a rock plug inside a
  peak wall makes an air-only door (Basalt Spine's centerpiece).
- **Combat is a weapons matrix.** Every kind carries a weapon list
  (cap 2) with per-weapon cooldowns and target-domain masks; the weapon
  covering the ordered target is the primary, sidearms pick their own
  nearest coverable hostile without steering the chassis. Splash hits
  hostile *units* in radius (buildings take direct hits only), computed
  against the start-of-tick world; every victim retaliates at the
  shooter. Indirect weapons arc over terrain. The **fire gate**: a shot
  needs the owner's team to currently see the victim's tile — which is
  what turns beyond-vision guns (Bombard, Bastion) into spotter
  weapons. Splash deliberately skips the gate (a shell in flight
  chooses nothing) and leaks no information: no event names an unseen
  sufferer, and retaliation stays sight-gated.
- **Factions are one roster deal, not two stat tables.** Variant kinds
  are separate `UnitKind`s sharing kind-keyed static stats;
  `Role::unit_for(faction)` maps the varied slots and `apply_train`
  rejects cross-faction kinds. Even seats run Ferrous on every
  AUTHORED and generated map — the training curriculum's default —
  but faction is fully choosable at launch (`Scenario::retint_seat`),
  and the flipped-seat probe cleared the policy for it.
- **Wrecks are a second salvage layer, not nodes.** Deaths leave a
  fraction of cost as `Tile.wreck`: never blocks movement, stripped
  standing ON the tile, decays on a global cadence (slowed 7.5x in
  0.13 — a battlefield prize that outlives the battle, minutes not
  seconds, still never a bank), buried by accepted foundations,
  skipped when a surviving building covers the tile. Vision remembers
  wreck amounts like scrap; stale memories resolve by walking and
  discovering. An explicit Harvest creates a bounded work zone and may
  adopt remembered or visible wrecks inside it; the clicked source stays
  the anchor, so local cleanup can never chain from wreck to wreck across
  a battlefield.
- **Harvest is an anchored work contract (0.14).** The source the player
  clicked remains the center of a Chebyshev-radius-seven zone through
  extraction and every delivery. The explicit source remains authoritative,
  but its route avoids fog-honest known danger whenever a detour exists.
  Autonomous follow-ons deterministically choose only reachable known nodes
  and wrecks inside the fixed zone, refusing visible ground threats,
  remembered armed structures, or a nearby radar contact. Nearby friendly
  ground combat value screens equal or weaker visible mobile pressure, so a
  defended line keeps working instead of treating every battle as a rout.
  If no safe source remains, retirement is sticky: it deposits its cargo, returns beside a
  built own Foundry, advances one queued order, then idles. Losing sight
  cannot wake stale salvage memory back up, and a source just beyond the
  original anchor's radius never becomes eligible through hop drift.
- **A Reclaimer is paid insurance, not an opening build.** Each completed
  Reclaimer credits one scrap every 24 ticks. At 150 scrap it repays its
  purchase after 3,600 ticks, then remains the efficient answer to an
  exhausted map; the late Foundry floor below is deliberately slower.
- **A living Foundry prevents permanent economy death.** A non-resigned
  seat with a surviving completed Foundry, no live Harvester, and none in
  a live production queue receives the fast emergency credit: one scrap
  every 10 ticks against one finite recovery entitlement. That entitlement
  is the captured bank deficit for a Harvester plus one Sentinel, or only
  the Harvester when a paid ground screen already survives. Spending,
  cancelling, or losing the package never refills it; a real Harvester
  deposit re-arms the next cycle and a prepaid worker's death preserves
  only the old unspent remainder.
  Separately, once 12,000 ticks have completed, every non-resigned seat
  with a completed living Foundry receives the long-game floor of one
  scrap every 60 ticks even while its harvest line exists. Both credits
  are per seat, never per Foundry. Automatic Repair Bays preserve the
  recovery entitlement. The neural executive forms a screen-and-worker
  package when a known source is guarded; the replay-era classic bot
  remains untouched.
- **Repair reuses construction's machinery.** Welding feeds buffered
  hp gains through the same resolve path as building (fire wins ties),
  stalls broke, and stacks across welders. Since 0.11 the three
  economy verbs price per hp against building cost, strictly build
  (1000‰) > repair (850‰) > salvage (800‰): repair bills through a
  ceiling-prepaid milli-scrap meter derived from the welder's own
  tick counter (chip repairs pay their coin up front; free healing
  was an exploit), so welding back what salvage banked always loses.
  Stacked welders bill against the same start-of-tick reading, so
  resolution refunds a welder whose whole step the hp ceiling
  rejected — the crew bills the job, not the crew size. The live
  weld/salvage meters saturate one shy of the validator's
  `PROGRESS_ENVELOPE` (economy's `metered`), so a torch held on one
  job for millions of ticks never overflows the u32 step math.
- **Machines weld too since 0.13** (`Command::RepairUnit`, appended in
  that release without disturbing older postcard discriminants): harvesters chase a wounded own GROUND
  unit and weld it at body contact (`REPAIR_REACH`), billed per hp
  against the patient's cost at the same 850‰ through the same
  prepaid meter (ramp = max_hp over train_ticks). The torch holds
  only while welder and patient BOTH stand still — a walking patient
  is chased, never healed, so sustain can't ride along with a
  retreat — and the patient's own orders are untouched (no eviction:
  nothing else targets a friendly unit). Heals buffer as
  `PendingUnitHeal` and resolve through `resolve_unit_heals`, the ONE
  seam every future unit-healing source (a repair structure's aura)
  must feed — fire wins ties, the volley's dead forfeit, the
  ceiling-rejected welder is refunded. Air patients refuse in v1;
  shell surface is right-click a damaged own unit or the armed `W`
  verb. The v7 construction/maintenance action keeps another free
  Harvester for the economy
  and admits a patient only when the bank covers that unit kind's exact
  first paid weld step (derived from the same debit function the sim
  executes). The promoted v7 artifact reaches field welding in all
  eight capability-probe cases. A rear-line veteran
  becomes draftable again only at full health, avoiding pullback/re-draft
  oscillation.
- **The Repair Bay is an aura, not a crew** (0.13):
  `BuildingKind::RepairBay` (2x2, 200 scrap, unarmed — appended last,
  postcard discipline) welds own wounded units, ground AND air (the
  reason the harvester verb may refuse flyers), inside
  `REPAIR_BAY_RADIUS` of its footprint on the `REPAIR_BAY_PERIOD`
  cadence — a building-id-order pass in the brain phase feeding the
  same `resolve_unit_heals` seam as crewmate welds, so fire wins
  ties. Billing is per hp at the same 850‰ from the OWNER's bank
  with hp itself as the meter (ceiling-diff of the patient's
  milli-scrap value telescopes to within one scrap — no stored
  counter, nothing new in the hash); a bank that can't cover a
  patient's coin skips it and keeps scanning, so partial scrap heals
  the earliest ids deterministically and a broke owner heals nothing.
  The promoted v7 artifact reaches the build slot, though Bays remain
  rare in ordinary matches: one of 240 seats completed one in both the
  dealt and combined final probes, while none did in the industry probe.
  The matchup arena seats carry a bank now (only billed sustain can
  spend it), so `--b-structures repairbay:n` measures the aura; at
  the shipped dials a bay roughly pays for itself sustaining an
  equal-cost army inside its ring and loses to the same scrap spent
  on turrets or attackers — a sustain tool, not a turtle wall.
- **Salvage is labor, not a button** (0.11): `Command::Salvage` sends
  harvesters to strip an own BUILT non-Foundry building down the
  construction ramp backward. Drains buffer beside the gains
  (`PendingHpDrain`) and resolve after damage as one signed
  per-building delta clamped once — fire zeroing the target wins the
  tick and forfeits everything undrained. Refunds credit in
  resolution from hp *actually* removed through a cumulative
  per-building ledger (a full-health salvage totals exactly
  cost·800‰; a truncation never drifts), the deliberate end is
  `Event::BuildingSalvaged` (no wreck, no scorch, stat screens must
  not count it a loss), and a salvaged producer refunds its prepaid
  queue in full via the CancelTrain rule. Repair and salvage evict
  each other from a target — the two never coexist, or the bot's
  deepest-wound repair pick would re-crew every salvage. Unbuilt
  sites keep Cancel; Foundries refuse outright.
- **Radar blips detect without identifying.** The Array's outer ring
  surfaces hostile units as bare tiles in `Vision::contacts` — no kind,
  no owner, no memory, no license for a targeted attack. Team sight is
  shared by stamping every teammate's discs into each seat's view;
  `State::hostile` routes every allegiance decision.
- **Screens are objects since 0.10** (shell/src/screens/): each menu
  screen (home, wizard, shelf, pause+confirm, settings+controls,
  playback transport) owns its menus and state; `update` takes raw
  events and returns a transition, windowless by construction — the
  whole flow drives headless in unit tests. The main loop keeps only
  drawing and session wiring. The viewport is INJECTED once per frame
  (`render::set_viewport`); menus, chrome scale, and `Game::new` read
  the seam and never query the window (headless tests get 1280x800).
  Since 0.13 the coordinator (shell/src/app.rs) is an `App` struct
  (everything that outlives a screen: game, tutorial, draft, config,
  input) plus one payload-carrying `Screen` enum — a mode without its
  screen's state is unrepresentable, so the old guard-and-repair arms
  are gone rather than moved. The Settings variant carries the screen
  it displaced (`back`) and leaving restores it wholesale — the pause
  menu's payload waits intact, so the cursor comes back to the row
  that left. Pause rows are an enum (`pause::Row`), never shifted indices:
  a new conditional row is one variant, not an index audit. Settings
  complaints live on the screen's own notice line drawn above the
  veil (a HUD toast dies under it), persisting until the next action;
  a refused rebind names its holder ("M is already bound to Run") via
  `BindingMap::holder` + `Action::label`.
- **Memories admit their age**: remembered ghosts and salvage fade
  along a 90-second ramp after sight loss (presentation-only state on
  `Game::last_seen`; the sim's Vision carries no timestamps). They
  never vanish — they stop pretending to be news.
- **Replays are an end-of-match affair**: Watch Replay appears once
  the match is decided; `autosave-` records are Continue-only.
  Mid-match playback was a fog-free scout of the enemy. The viewer
  opens through `Game::spectator` — no command seat required, so
  all-bot records (driver benchmarks, bot-vs-bot spectacles) play
  back like any save.
- **The command panel is one grammar** (shell/src/panel.rs): a pure
  model (portrait, sprite cards carrying the exact Action their
  hotkey dispatches, queue thumbnails carrying CancelQueue) built
  from the selection, drawn by the renderer, hit-tested through the
  LayoutModel's card rects. Buildings and units share it; tooltips
  derive weapon lines from stats and name the live chord. The sim
  gained `CancelTrain` (full refund, head progress resets) for the
  queue ghosts. Since 0.13 an order chip carries its SUBJECT
  (`CardIcon::Order`): the target's own sprite in the target's own
  faction under a corner verb badge, ghosted beneath a scaffold
  while its site is still rising, with the kind in the title and a
  concrete line (percent raised, hp, scrap still in it) in the
  tooltip. The subject is resolved in the pure model, OWN programs
  only — an inspected ally's chips stay bare pictograms rather than
  resting the panel on a claim about what team sight shares, and an
  attack victim resolves through the breadcrumbs' own fog gate so
  chip and trail can never tell different stories. A chip whose
  subject is gone degrades to the plain verb. Every card also
  carries its own `progress`, so the drawn panel no longer peeks
  back into the state for the production head's bar. Tooltips anchor
  to the HOVERED rect through `layout::tooltip_origin` (a pure,
  headless-tested clamp): command cards above their card, dock chips
  right of the dock and centered on the chip, both boxed into the
  window between the top bar and the band.
- **The tutorial advances on demonstration** (shell/src/tutorial.rs):
  six cards watching `Game::demo` flags set from the human's own
  command stream — never a timer, never a "next" button — except the
  mining lesson, which graduates on the first *delivered* load
  (`Event::ScrapDeposited`), not on the accepted order. The match is
  `tutorial_scenario()`: the embedded skirmish with pushover bots and
  a tutorial-only raised bank (the authored 150 ran dry across the
  lessons' prepaid spends; the JSON stays untouched so every fixture
  and replay stands). Cost-bearing cards carry a live coach line —
  price, bank, hauling count — that becomes the press-N recovery
  nudge at an unaffordable lesson with nothing mining, and a
  literal-instructions playthrough test in `input::tests` pins every
  lesson affordable at shipped numbers. Dismissible; re-entry is
  another tutorial match from Home.
- **The build palette is data-driven.** `B` opens it; digits are
  contextual (palette first, then the first compatible selected producer's
  slots filtered to the seat's faction, then control groups). The old
  hardcoded B/N hotkeys are gone.
- **Input is semantic since 0.9.** `poll_events` is the only hardware
  reader; RawEvents resolve through a `BindingMap`
  (shell/src/action.rs) into Actions — "Oxide Classic" is the default
  profile, the Controls screen rebinds with conflict refusal, and
  chord matching grades exact → same-Ctrl → bare. The frame loop
  injects ui scale, wall clock, and camera + touch prefs into
  `InputState`, so the whole event path runs headless (input.rs has
  real integration tests against the sim).
- **Chrome geometry has one source.** The renderer computes a
  `LayoutModel` (top bar, panel band + clickable slots, minimap, idle
  badge) as it draws and publishes it on `Game`; hit-testing and
  QueryUi read the same model. Never hand-roll a second copy of any
  chrome rect — that class of bug (the 0.8 palette click-leak)
  is structurally extinct only while this holds. ui_scale() is the
  USER factor only: macroquad's coordinate space is logical, and
  multiplying dpi in is the double-scaling disease (fixed 0.9).
- **Chrome text color has one source too** (shell/src/theme.rs, 0.13):
  four tiers picked by what a line IS — primary, body (required
  reading: tutorial lessons, coaching, tooltip descriptions),
  secondary (hints, captions, off-cursor dials), disabled (the only
  legitimately dim one) — plus title/accent/danger and the surface
  fields. Unit tests pin body and secondary to >=4.5:1 (WCAG AA)
  composited on the house fields; pre-0.13 the shell carried four
  divergent constant sets and one alpha-90 token did triple duty.
  World decoration (order rings, rally lines, faction art) keeps the
  renderer's own palette — raising text must never retint the world.
- **Presentation config persists** (shell/src/config.rs): bindings
  (explicit unbindings survive via a tombstone list — a missing row
  alone reads as "verb newer than this config" and would re-adopt
  its classic chord), volumes, ui scale, camera feel, touch timing,
  window size, reduced motion, colorblind — platform config dir,
  versioned separately from replays, silent defaults on any trouble.
- **Persistence fails loudly and rotates narrowly (0.13).** Every
  record lands through `chassis::fsx::write_atomic` — parents created,
  temp + fsync + cross-platform atomic replace (std's rename replaces
  existing files on Windows too; the old remove-then-rename fallback
  was folklore), temp reaped on every error path, orphans swept by
  rotation. `autosave::save` reports `SaveOutcome`/`SaveError` and a
  quit path whose save fails raises a Retry / Leave-without-saving
  dialog (Cancel preselected) instead of exiting over data loss — the
  Leave row guarantees a full disk can never trap the player.
  Rotation is kind-scoped: `autosave-` keeps 5, `match-` keeps 20,
  and the explicit-saves directory (`<data>/saves`) is never
  rotation's to touch. `shell/src/paths.rs` is the one owner of the
  per-OS write roots (config/data/autosaves/saves/replays); a
  packaged bundle resolves `replays/` against the data dir instead of
  its unusable cwd, while workspace runs keep the documented
  cwd-relative `replays/`.
- **Screens are draft-driven.** The New Match wizard's choices live in
  a NewMatchDraft that survives Back; destructive pause choices
  confirm with Cancel preselected; menus scroll independently of
  selection and activate on release-inside (menu_ux tests spawn real
  windows and are #[ignore]d — run them explicitly, never in CI).
  The front door is a thumbnail-grid map browser sectioned by format
  (shell/src/screens/browser.rs, themed preview cards, remembers the
  pick by path); every map then lands on one inline setup screen —
  seat cards with difficulty/personality/faction chips edited in
  place beside a who-is-where preview (no sub-screen; the cell
  cursor moves with Left/Right and Enter takes a seat or cycles a
  dial; the human's card keeps its faction chip). Since 0.12 duels
  land there too — the 1v1 quick-question flow is gone, Start stays
  preselected so Enter-Enter still launches the classic matchup,
  team headings draw only when a team actually groups seats, and
  picking a DIFFERENT map resets the chair and dials while
  re-entering the same map keeps them. Small windows
  compress margins, chrome, then cards — every control stays on
  screen at every supported size, keyboard and pointer alike.
  Allegiance reads as team color ON the art (the RTS convention,
  semantic flavor): every faction-varied sprite carries a derived
  accent mask in the atlas — gen_sprites.py diffs the two faction
  variants; where they differ IS the faction-colored region — and the
  shell overlays it through `seat_identity_color`: own machines stay pure,
  allied seats receive distinct cool accents, and hostile seats receive
  distinct warm accents (colorblind mode keeps a friend/foe luminance split).
  Minimap dots use the same identities.
  Never a badge, bar, or ring around the silhouette — bars read as
  health. Runtime UI copy is ASCII-only; semantic icons carry meanings
  such as vision, radar, attack range, minimum range, and repair.
- **Stalls carry reasons** (`StallReason`): own-state facts only —
  routes, banks, footing. A reason must never derive from what fog
  hides; the enum doc enforces the principle on future variants.

## Known issues (tracked, deliberate)

- **The first click into an unfocused shell window is eaten by the
  engine layer** (macOS: miniquad's view answers neither
  `acceptsFirstMouse` nor a tracking area, so pointer motion and the
  focusing click go undelivered until the window is key — worst after
  a terminal launch, where activation lags a beat). Diagnosis
  recorded; the fix is upstream and deferred to a post-0.13 session.
  `dist/Oxide.app` activates normally, and `--trace-startup` shows
  the dead window as frames with `hw_events=0`.
- **The former A1450 all-neural Expert 2v2 could stall on open maps and lean west on
  Twin Forges** (measured: 12/12 thirty-k-tick draws on Open Quarry at
  Expert; 12-2 west in decisive Twin Forges games). Both are artifacts
  of near-deterministic symmetric self-play: each seat's
  enemy-strength reading doubles in 2v2 so trained push thresholds
  never fire, and the sim's residual id-order micro compounds without
  blunder noise (the learned bots think in seat-oriented coordinates
  — `bot::Orientation` — precisely so no tie-break favors a compass
  direction; what remains is the micro, and leans are
  artifact-specific: re-measure per artifact before blaming a map). At
  the former A1450 Medium baseline both effects vanished
  (12/12 decisive, no consistent lean), and a human in the match
  breaks symmetry at any level — bounded to bot-vs-bot spectacles.
  The parity-alternate `movement::run` candidate was run and retired
  in 0.13: path following has no cross-unit reads (each body consults
  only itself, terrain, and buildings), so reversing its iteration by
  tick parity is provably inert — measured bit-identical across the
  25-scenario fixture slate unblessed, a 120k-tick skirmish bot run,
  the 48-game Medium decisiveness sweep, and a 12-game Expert duel at
  120k cap. The id-order coupling in movement lives in the collision
  resolver's sequential Gauss-Seidel passes, and their direction has
  alternated by tick parity since 0.6; what remains fixed is
  pair-visit order within a pass — a future candidate would aim
  there, and THAT one is genuinely hash-moving.
- **Under the former A1450 policy, Causeway Verdict and Gatework Array
  fired no shot in 12k ticks**
  (all-Medium, the former A1450 baseline): not one casualty either, while
  every seat's economy runs at the roster's normal rate. Ten minutes of
  game time is march time on the biggest team maps — Trident Plateau
  and Compass Grand at the same seat counts do fight — so the liveness
  gate holds only the 2- and 4-seat formats to a combat floor. Whether
  that is honest scale or a marching problem is a pacing question, not
  a liveness one. The vast 1v1 maps earned the same reading under the
  0.13 roster prices: first contact on their 100+ tile routes lands
  after 12k ticks on maps whose decision medians are 18.6-21.9k, so
  the gate's horizon for the vast class is 24k — the floors themselves
  are unchanged.
- **The Cupric skirmish lean was the roster, and the 0.13 rebalance
  closed it** (Buzzard 160 -> 120, Darter 90 -> 100 — see the sim-batch
  notebook). The factorial probe's before/after on one seed set:
  Ferrous 21.5% [19.5, 23.6] of mixed-roster victories under the old
  prices, 48.5% [46.0, 51.1] shipped, FC/CF cells 44.7%/47.6% from
  23.2%/80.3%, same-faction mirrors still pacing apart (FF median
  7,763 vs CC 6,394 — par, not mirrors). What remains is
  artifact-specific residue at low blunder noise: the Expert
  seat-swapped skirmish sweep flipped its lean from 34-6 Cupric to
  35-1 Ferrous under the new prices — the near-deterministic mirror
  residual whipsawing as it does under any sim change, broken by any
  human in the match, re-measured per artifact. Two maps initially
  paid for par: Long Haul and Oxide Flats read 19/24 decided at a 60k
  cap (Long Haul's old 24/24 at median 4,859 was seat1 winning every
  game — its decisiveness WAS the imbalance). The first two 0.13 weld
  candidates stalled worse and were refused. The v6 A1450
  incumbent-continuation baseline closes that regression: its
  3,072-match factorial has zero censored games, and the 16-map pace
  slate decides 384/384 at 60k with all Long Haul and Oxide Flats
  games resolved. Its mixed-roster Ferrous marginal is 45.3%
  [42.8, 47.8], close to the frozen bridge's 48.5% rather than the
  refused candidates' reopened faction failure.
- **The A1450 policy was a middling teammate beside a scripted
  ally**, though its continuation improved every tier on the
  2v2 slate (48/96 versus the frozen bridge's 33/96). Shipped 2v2
  seats are all-neural, which is the configuration it trained; deeper
  mixed-ally training remains a known lever and has historically cost
  duel sharpness. R2 trained against both self-team and scripted
  mixed-ally lanes, but its final mixed-ally strength remains unmeasured;
  revisit when 2v2 becomes a headline mode.
- **Expert's outright yardstick sweep is on loan to the movement era.**
  The 0.12 movement overhaul first moved Expert from a sweep to 60/80;
  subsequent 0.13 economy and roster changes re-rolled every rung
  again. The ladder therefore asserts strict ordering and an outright
  Expert lead, not historical bars that no longer describe the same
  physics. The first 0.13 weld lineages did not ship: the from-scratch
  candidate decided only 31/48 skirmish games, its descendant recovered
  only to 44/48, and faction parity reopened. The
  A1450 incumbent-continuation artifact finally preserved decisiveness and
  the exact 150-seed native cup matrix, but embedding it made Hard at
  cadence 16 read below Medium. Those are historical v6 readings. The
  v7 policy instead conditioned every named rung on its trained style and
  expressed difficulty through cadence and hesitation; its 160-match
  yardstick read 101 < 116 < 135 < 140, and its native cup read 580/800
  with zero caps or draws. Those are historical v7 results, not permanent
  thresholds. The shipped v8 R2 update-140 policy retains the execution-only
  difficulty ladder while adding the resolved named-profile contract. Its raw
  zero-facet 160-match yardstick reads Easy 103 wins and 3,274,331 ticks,
  Medium 125 and 2,459,207, Hard 144 and 1,643,189, and Expert 147 and
  1,544,745: wins rise and the horizon-priced tick total falls at every rung.
  Its final canonical-profile native cup matrix reads FF 115 wins with 13
  caps, FC 133 with 7, CF 139 with 8, and CC 157 with 2, each over 300
  games, for 544/1,200 wins with 30 caps. Every cap remained active. Zero
  caps is therefore historical
  evidence, not a standing rule: the current gate rejects unhealthy tails
  while permitting active wars at the horizon.

## Gotchas learned the hard way

- tiny-skia's anti-aliased path asserts on sub-pixel rects — AA stays off
  for rect fills in the golden renderer (hp bars produce sub-pixel spans
  constantly).
- The first attack of a fight can land on the same tick as its command;
  tests that collect events must keep the command tick's report.
- `fixed`'s `to_num::<i32>()` truncates toward zero — always `floor()`
  first for tile math (already wrapped in `TilePos::containing`).
- A macroquad window on macOS must run on the main thread; the debug
  server therefore lives on socket threads and crosses to the frame loop
  by channel. Don't try to answer protocol requests off-thread.
- macroquad 0.4 ships with `default = []` — audio is a feature. Without
  `features = ["audio"]` every sound call is a silent stub that only
  logs a warning (this repo shipped four versions that way).
- Injected pointer events use *logical* coordinates (what
  `screen_width()` reports); screenshots come back in *physical* pixels
  (2× on retina). Don't dpi-scale injected clicks to match a screenshot
  — halve the screenshot's coordinates instead. The miniquad RAW input
  stream (the shell's hardware pointer source since 0.12) is physical
  too: `PointerStream` divides by the injected `dpi_scale` once at the
  adapter, and nothing downstream may scale again.
- A paused shell stages socket commands for the *next* tick; drive one
  `AdvanceTicks` before asserting on their effects, or the assert races
  the order.
