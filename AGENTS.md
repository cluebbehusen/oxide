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
| `oxide-sim` | `sim/` | All Oxide game rules. `State::tick(&[PlayerCommand])` is the only way anything happens. The skirmish bot lives here too, but *outside* the tick pipeline — it's just another command source. |
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
`.github/workflows/ci.yml` is authored and dormant until a remote exists.

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
```

`screenshots/` and `replays/` are gitignored scratch output; keep goldens
and test fixtures inside crate `tests/` directories.

## Conventions

- **Conventional commits** (`feat(sim): …`, `fix(shell): …`, `docs: …`),
  trunk-based on `main`. Commit signing is disabled repo-locally (the
  global signing key needs an interactive passphrase).
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
  ground; the byte is hashed but nothing else changes), `#` rock, `s` scrap
  node, `S` rich node (double salvage), `1`-`8` Foundry anchors (top-left
  of 2x2). Shipped maps are 180°-symmetric — author edits in mirrored
  pairs. `Scenario::skirmish()` embeds `scenarios/skirmish.json` at
  compile time.
- **Balance numbers** all live in `sim/src/stats.rs`; expect hash churn
  when touching them.
- Keep this file and README.md current when commands or behavior change.

## Design decisions worth knowing

- **`State` fields are private; `State::tick` is the only mutator.** Read
  through the accessors (`units()`, `buildings()`, `players()`, `map()`,
  `current_tick()`, `result()`, `vision(id)`, `hash()`). If new code needs
  a view the accessors can't give, add an accessor — never a `pub` field.
- **Ranged fire traces line of sight** (`chassis::path::line_blocked`, a
  fixed-point supercover walk): rock and non-target buildings block, scrap
  and units don't, endpoints never do. In range but blocked → keep
  approaching until range *and* line hold. Vision stays radius-based on
  purpose — cover is a firing rule, not a stealth system.
- **Movement feel is tuned, not emergent** (`sim/src/stats.rs`): waypoints
  accept within `WAYPOINT_ACCEPT` (corner-safe), arrival propagates through
  contact with settled neighbors near a shared goal (`ARRIVAL_NEAR`), group
  orders fan out over a deterministic ring of per-unit goals, anchored
  workers take `ANCHORED_PUSH_SHARE` of pair separation so crowds flow
  around them, and collision applies pairs Gauss-Seidel-style in id order —
  symmetric cancellation once froze the whole economy.
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
  Masterless machines finishing their last orders fit the fiction; in
  two-player games the question is moot (elimination ends the match), and
  if FFA maps ever ship, revisit deliberately.

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
