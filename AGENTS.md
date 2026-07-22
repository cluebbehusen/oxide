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
| `chassis` | `chassis/` | Reusable deterministic-sim toolkit: Q32.32 fixed point (`fx`), PCG32 (`rng`), FNV-1a state hashing over postcard bytes (`hash`), tile grid (`grid`), 8-dir A* (`path`), tick-stamped replay format (`replay`). No game rules, no engine deps. |
| `oxide-sim` | `sim/` | All Oxide game rules. `State::tick(&[PlayerCommand])` is the only way anything happens. The bots live here too, but *outside* the tick pipeline — command sources like the mouse: the shipped **neural ladder** (`bot::NeuralBot`, embedded quantized weights, Easy/Medium/Hard/Expert + a personality knob), the scripted `bot::Brain` tiers (fog-honest, training anchors and benchmarks), and the classic 0.6 `bot::Bot` (what replays without a `bot_config` reproduce). |
| `oxide-protocol` | `protocol/` | Debug-protocol types: JSON-lines envelope, tagged requests/replies, `RawEvent` input events (touch included for the future mobile shell), and `StateView` (floats + ASCII map — legible, not exact; exactness is the hash's job). |
| `oxide-shell` | `shell/` | macroquad renderer, the single input funnel, HUD, debug server. Nothing here may affect game outcomes except by staging tick-stamped commands. |
| `oxide-driver` | `driver/` | CLI harness: headless scenario runs, replay verification, byte-exact golden images (tiny-skia, CPU-only), live-game client, automated smoke test. Also a library (`runner`/`render`/`client`/`smoke`). |

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
in the commit message.

## Build, test, bless

```sh
cargo test --workspace               # all tests, headless, ~seconds
cargo clippy --workspace --all-targets
cargo fmt --all
BLESS=1 cargo test -p oxide-driver   # regenerate goldens after intended change
```

Before committing: fmt + clippy clean, tests green. Golden files live in
`driver/tests/goldens/`: byte-exact PNGs (a mismatch writes the actual
under `target/` for side-by-side inspection) plus `state-hashes.json`,
bot-vs-bot hashes at tick 2,000 for every shipped scenario — the cheap
tripwire that flags sim drift without image churn, and the fixture CI
re-derives per-OS as the cross-platform determinism proof.
`.github/workflows/ci.yml` runs the suite on three OSes plus an MSRV
job on every push and PR; the hash-fixture step re-derives per-OS as the
cross-platform determinism proof.

## Running and driving the game

```sh
cargo run -p oxide-shell                            # play (human vs bot)
cargo run -p oxide-shell -- --debug-server          # + socket on 127.0.0.1:4123
cargo run -p oxide-shell -- --debug-server --paused # driven mode: sim time
                                                    # moves only on request
cargo run -p oxide-driver -- smoke --spawn          # automated live check
```

A typical agent session against a running shell:

```sh
driver() { cargo run -q -p oxide-driver -- "$@"; }
driver live status
driver live state --map            # ASCII map with entities overlaid
driver live harvest 0 --units 0,1,2 --node 7,2
driver live attack-move 0 --units 3 --to 34,18
driver live rally 0 --building 0 --tile 7,2   # or --clear
driver live advance 300            # exactly 300 ticks, replies with hash
driver live screenshot -o screenshots/check.png   # then READ the png
driver live inject-wheel 2.0       # events enter the real input funnel
driver live inject-key escape      # opens the pause menu — menus share
driver live inject-key enter       # the input funnel too
driver live save-replay replays/session.json
driver replay replays/session.json # must print the same hash as live
driver live load-replay replays/session.json      # resume = load a save
```

Save states are replays, by design: `load_replay` rebuilds the scenario,
re-runs the recorded ticks headless-fast, and keeps recording on the same
log — no snapshot format, no way for a save to desync from its history.
The cost is version-pinning (replays reproduce only on the sim that wrote
them) and load time proportional to session length, which at thousands of
ticks per second is noise. If sessions ever get long enough to hurt,
revisit with a snapshot+suffix-log hybrid — and keep the recorder valid.

Headless, no window needed:

```sh
driver run skirmish --ticks 2000 --bots --map     # summary + ASCII map
driver render skirmish --ticks 1200 --bots -o out.png
driver replay-stats replays/session.json          # per-seat series + losses
driver map-audit scenarios/basalt-spine.json      # routes, fairness, pressure
```

Replay UX in the shell: cold launches land on Home; `Replays` browses
autosaves and `replays/` (watch, delete, honest version badges);
`oxide-shell --watch <replay>` opens the read-only playback viewer
(pause, seek, speed — no recorder; backward seek restores an
in-memory checkpoint and re-simulates). The pause menu's Watch
Replay plays the live session so far. `sh tools/package_macos.sh`
builds `dist/Oxide.app` (resources resolve executable-relative when
bundled, cwd otherwise).

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
  appear in `driver/src/render.rs` and `shell/src/render.rs` — keep them
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
  180°-symmetric — author edits in mirrored pairs, and on 4-player maps
  every seat's unit list must be the exact image-transform of seat 0's,
  entry by entry (the 0.7 seat-fairness rule generalized). Faction
  convention: even seats Ferrous, odd seats Cupric.
  `Scenario::skirmish()` embeds `scenarios/skirmish.json` at compile
  time.
- **Balance numbers** all live in `sim/src/stats.rs`; expect hash churn
  when touching them.
- Keep this file and README.md current when commands or behavior change.

## The bots and the training loop

The shipped opponent is a neural policy embedded in `oxide-sim`
(`sim/src/bot/ladder_weights.json`, Q12 integers evaluated in pure
`i64` — no floats, so neural matches replay bit-identically and the
hash fixtures pin the weights like any other rule). Difficulty is a
dial into one mind: `bot::Level` (Easy/Medium/Hard/Expert) sets a
skill knob whose degradation the network *trained under*; a second
knob picks the personality (turtle → aggressive), dealt from the
scenario seed when unset; a third carries the seat's faction (by map
convention, even seats run Ferrous — every shipped and generated map
follows it, and the knob is honest, never sampled). Scenario seats opt
in via `PlayerSpec.bot_config`; a seat without one gets the legacy
rule-cascade bot, which is what keeps pre-0.7 replays reproducing
(that bot is team-blind by design — team seats must set a config).

The gym contract (v4) is 63 named integer features and 21 masked
macro actions; training slots are role-indexed where the factions
differ, so one action space serves both rosters. Since v4 every
positional feature rides as relative 0-1000 against the actual map
dimensions (fixed scales broke on the large map classes), map dims
ride along (march timing is an absolute-distance skill), and two
fog-safe shell senses report incoming shells near the economy
(impact tile currently visible — the arc renderer's rule) and own
shells in flight. `FEATURE_NAMES` rides in the gym hello and the
Python wrapper asserts its own list against it — Rust/Python column
skew dies at handshake, not in a silently mistrained run.

The weights are a generated artifact with a regeneration ritual, like
the goldens. From `tools/train/` (uv + PyTorch):

```sh
uv run bc.py --arch deep --episodes 48 --out runs/prior.pt   # imitation warm start
uv run league.py --name run --resume runs/prior.pt --anchor runs/prior.pt     --maps random --mix "self=0.35,past=0.15,tier=0.15,rusher=0.10,ffa=0.25"
uv run tournament.py --ckpt runs/run/pool/ckpt-XXXXX.pt      # torch-side eval
uv run export.py --ckpt <winner> --out runs/candidate.json   # Q12 artifact
cargo run -p oxide-driver -- neural-cup --weights runs/candidate.json  # the gate
```

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
an already-strong parent (the 0.8 artifact's lineage: BC bridge →
league peak → anchored peak → team-consolidated peak, gated 300/300).
Team training runs two flavors — self-team (`team`: the learner holds
both chairs) and mixed-ally (`team2`: a scripted Brain drives the
teammate) — and per-seat episode truncation pads a dead learner's
lane on its frozen last view so batches stay rectangular while the
teammate plays on; padded rows are masked out of the PPO update (GAE
still spans them so the team payoff reaches the live prefix). The scripted `Brain` tiers and the rush teacher
stay in-tree as league anchors, benchmarks, and the ladder-integrity
yardstick (`sim/tests/neural_ladder.rs` enforces Easy < Medium <
Hard < Expert forever).

## Design decisions worth knowing

- **`State` fields are private; `State::tick` is the only mutator.** Read
  through the accessors (`units()`, `buildings()`, `players()`, `map()`,
  `current_tick()`, `result()`, `vision(id)`, `hash()`). If new code needs
  a view the accessors can't give, add an accessor — never a `pub` field.
- **Ranged fire traces line of sight** (`chassis::path::line_blocked`, a
  fixed-point supercover walk): rock and non-target buildings block, scrap
  and units don't, endpoints never do. In range but blocked → keep
  approaching until range *and* line hold. Vision stays radius-based on
  purpose — cover is a firing rule, not a stealth system. The trace
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
  around them, and collision applies pairs Gauss-Seidel-style in id order —
  symmetric cancellation once froze the whole economy.
- **Orders are programs since 0.5**: every unit carries a bounded queue
  plus a looping flag; completion pops (or rotates — that's patrol),
  stalls drop the whole program with `OrderStalled`, plain orders replace
  it wholesale — except that reissuing the unit's *exact current* order
  is a no-op past the queue wipe, keeping path and progress (the
  scripted tiers re-command every think; repair billing counts on the
  meter surviving). Patrol legs are attack-moves and never settle.
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
- **Construction claims ground instantly**: full price on placement
  (refused — and refunded nothing — if no doorstep is reachable), a
  fifth of max hp standing, blind and inert until built. Ground closing
  mid-walk is real: movement revalidates each waypoint and repaths
  around fresh sites. Progress needs an adjacent builder — **several
  adjacent builders stack**, each contributing a tick, so two roughly
  halve the build; deliberate, tested. Orphaned sites freeze and any own
  harvester can resume them; a site zeroed by fire is dead even if its
  builder acts the same tick — construction hp-gains buffer like damage
  and resolve after it, so fire wins ties. Cancel (`X`) refunds
  `cost × hp / max_hp`. One predicate — `State::can_place` — serves sim
  validation and the shell's ghost, and it requires the footprint
  *currently visible*: its occupancy checks read live state, and testing
  explored-but-unseen ground would leak hidden enemies through the red
  tint.
- **Fog of war enforces exactly one thing in the sim**: targeted attacks
  need the issuer to *see* the victim. Rendering honors fog fully
  (unexplored void, explored dim, unseen enemies culled) but the debug
  surface — `query_state`, the F1 overlay, the software renderer — is
  deliberately omniscient. The bot reads full state (classic cheating AI);
  its commands still pass normal validation.
- **Units are solid but never block tiles.** Collision is iterative pair
  relaxation after movement; pathfinding ignores units entirely, so crowds
  jostle but can't deadlock a corridor the way tile-reservation schemes do.
- **Fire at will is the only stance.** The shell's right-click issues
  `AttackMove` for ground orders: units engage in aggro range, fight via
  `Order::Attack { resume: Some(goal) }`, and pick the march back up. Idle
  units auto-acquire (attackers must close inside aggro to shoot, so
  standing units always retaliate). Plain `Move` stays oblivious and
  remains protocol/bot-only — it becomes a player verb again if stealth
  or hold-fire ever exist. If a future unit outranges aggro, add
  damage-triggered retaliation; today nothing does.
- **Ghost memory lives in `Vision`**: enemy-building records refresh while
  their ground is visible and freeze when sight is lost; seeing the ground
  empty erases them. Scrap amounts get the same treatment via a per-player
  remembered grid. Renderers draw live state on visible ground, memories
  elsewhere — same rule on the minimap.
- **Sound follows sight.** Positional clips require the event's tile to be
  visible to the human; own losses and milestones are always audible. The
  queue is dropped after `advance_ticks` bulk jumps, and a per-kind rate
  limiter keeps battles from clipping into noise.
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
  the two sites cross-reference each other on purpose).
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
- **Peaks (`^`) are the mountain nothing crosses.** A third terrain:
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
  rejects cross-faction kinds. Even seats run Ferrous on every shipped
  and generated map — the training stack's faction knob depends on it.
- **Wrecks are a second salvage layer, not nodes.** Deaths leave a
  fraction of cost as `Tile.wreck`: never blocks movement, stripped
  standing ON the tile, decays on a global cadence, buried by accepted
  foundations, skipped when a surviving building covers the tile.
  Vision remembers wreck amounts like scrap; stale memories resolve by
  walking and discovering.
- **Repair reuses construction's machinery.** Welding feeds buffered
  hp gains through the same resolve path as building (fire wins ties),
  costs a scrap trickle billed at each interval's *start* (chip
  repairs pay their coin; free healing was an exploit), stalls broke,
  and stacks across welders.
- **Radar blips detect without identifying.** The Array's outer ring
  surfaces hostile units as bare tiles in `Vision::contacts` — no kind,
  no owner, no memory, no license for a targeted attack. Team sight is
  shared by stamping every teammate's discs into each seat's view;
  `State::hostile` routes every allegiance decision.
- **The build palette is data-driven.** `B` opens it; digits are
  contextual (palette first, then a selected factory's produce slots
  filtered to the seat's faction, then control groups). The old
  hardcoded B/N hotkeys are gone.
- **Input is semantic since 0.9.** `poll_events` is the only hardware
  reader; RawEvents resolve through a `BindingMap`
  (shell/src/action.rs) into Actions — "Oxide Classic" is the default
  profile, the Controls screen rebinds with conflict refusal, and
  chord matching grades exact → same-Ctrl → bare. The frame loop
  injects ui scale, wall clock, and camera prefs into `InputState`,
  so the whole event path runs headless (input.rs has real
  integration tests against the sim).
- **Chrome geometry has one source.** The renderer computes a
  `LayoutModel` (top bar, panel band + clickable slots, minimap, idle
  badge) as it draws and publishes it on `Game`; hit-testing and
  QueryUi read the same model. Never hand-roll a second copy of any
  chrome rect — that class of bug (the 0.8 palette click-leak)
  is structurally extinct only while this holds. ui_scale() is the
  USER factor only: macroquad's coordinate space is logical, and
  multiplying dpi in is the double-scaling disease (fixed 0.9).
- **Presentation config persists** (shell/src/config.rs): bindings,
  volumes, ui scale, camera feel, window size — platform config dir,
  versioned separately from replays, silent defaults on any trouble.
- **Screens are draft-driven.** The New Match wizard's choices live in
  a NewMatchDraft that survives Back; destructive pause choices
  confirm with Cancel preselected; menus scroll independently of
  selection and activate on release-inside (menu_ux tests spawn real
  windows and are #[ignore]d — run them explicitly, never in CI).
- **Stalls carry reasons** (`StallReason`): own-state facts only —
  routes, banks, footing. A reason must never derive from what fog
  hides; the enum doc enforces the principle on future variants.

## Known issues (tracked, deliberate)

- **The classic bot's 27/27 seat-1 mirror sweep: root-caused and fixed
  in 0.7.** The twin-simulation trace found symmetry breaking at tick 0
  because `skirmish.json`'s p1 spawn list wasn't the exact mirror-order
  of p0's — every id-order-sensitive decision then ran in a different
  logical order per seat. Two swapped JSON lines turned the sweep into
  a seed-decided coin flip; all other shipped maps were already
  mirror-ordered (authoring rule: p1's unit list must mirror p0's entry
  by entry). The learned bots additionally think in *seat-oriented*
  coordinates (`bot::Orientation`) so no `(y, x)` tie-break or ring
  scan favors a compass direction. Residual: the sim's id-order micro
  (movement first-mover, brain iteration) can still decide
  identical-dial neural mirror matches; win-rate gates neutralize it by
  scoring seat-swapped pairs, and shipped matches deal varied
  personalities, so no seat holds a standing edge in practice.
- **The 0.7 Standard-stall blemish is gone in 0.8** — the promoted
  artifact swept its gate 300/300 with zero draws; air harass gave the
  policy the anti-turtle tool the old roster lacked.
- **All-neural Expert 2v2 can stall on open maps and leans west on
  Twin Forges** (measured: 12/12 thirty-k-tick draws on Open Quarry at
  Expert; 12-2 west in decisive Twin Forges games). Both are artifacts
  of near-deterministic symmetric self-play: each seat's
  enemy-strength reading doubles in 2v2 so trained push thresholds
  never fire, and the sim's residual id-order micro (movement still
  iterates in fixed id order) compounds without blunder noise. At the
  shipped Medium default both effects vanish (12/12 decisive, no
  consistent lean), and a human in the match breaks symmetry at any
  level — bounded to bot-vs-bot spectacles. Candidate engine
  experiment for 0.9: parity-alternate `movement::run` like the brain
  loop (hash-moving; needs a bless and a re-measure).
- **The learned policy is a middling teammate beside a scripted ally**
  (25-31% on the mixed-ally 2v2 bracket vs scripted pairs, up 5x from
  pre-team-training). Shipped 2v2 seats are all-neural, which is the
  configuration it trained; deeper mixed-ally training is the known
  lever and costs duel sharpness — revisit when 2v2 becomes a
  headline mode.

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
  — halve the screenshot's coordinates instead.
- A paused shell stages socket commands for the *next* tick; drive one
  `AdvanceTicks` before asserting on their effects, or the assert races
  the order.
