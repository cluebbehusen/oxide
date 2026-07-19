# Oxide

A small 2D real-time strategy game about machines eating a dead world.

Two robot swarms — **Ferrous**, bleeding rust orange, and **Cupric**, crusted
in teal patina — wake up in the wreckage of some forgotten industry. Scrap is
food. Harvesters haul it home, Foundries smelt it into more machines, and
Sentinels make sure the other swarm doesn't get to. It ends when one side's
Foundry is a smoking crater.

Oxide is also an experiment: it was built almost entirely by an AI agent, and
the architecture is shaped by that. The entire game is a pure, deterministic
simulation that runs headless at thousands of ticks per second; the renderer
is a thin shell over it; and a driver CLI can play, test, screenshot, and
replay the game with nobody at the keyboard. The properties that make an RTS
netcode-friendly — lockstep ticks, command streams, fixed-point math — are
the same ones that make it machine-testable. That's the bet this repo
explores, and so far it holds: any live session, human or agent, saves as a
replay that re-executes headless to a bit-identical state hash.

## Playing

```sh
cargo run -p oxide-shell
```

A menu lists the shipped maps — Skirmish Basin, Scrapyard Brawl, Rustbelt
Canyon, Verdigris Fields. You're Ferrous; the bot is Cupric, and it is not
asleep.

| Input | Action |
|---|---|
| Left click / drag | Select your units (click a Foundry to select it) |
| Left click on minimap | Jump the camera there |
| Right click | Contextual order: enemy → attack, scrap → harvest, ground → **move engaging everything on the way** (fire at will is the only stance; combat units always defend themselves) |
| Right click on minimap | Send the selection there, fighting through |
| Right click (Foundry selected) | Set the rally point — rally a scrap node and fresh harvesters mine it; fresh Sentinels attack-move to it |
| Mouse wheel | Zoom (toward the cursor) |
| Arrow keys | Pan |
| `H` / `S` | Train a Harvester (50) / Sentinel (75) |
| `Space` | Jump to your Foundry |
| `P` | Quick pause |
| `Esc` | Deselect, then the pause menu (resume / restart / main menu / quit) |
| `F1` | Debug overlay (grid, ids, paths — and no fog) |

Fog of war is real: you see what your machines see, explored ground stays
dimly remembered, and you cannot target what nobody is looking at. Enemy
buildings you've scouted linger as gray ghosts until someone sees that
ground again — a ghost is a belief, and beliefs go stale. The minimap
(bottom-right) follows the same rules. Units are solid — a chokepoint held
by a wall of Sentinels is actually held. Sound follows sight: you hear
fights you can see, and your own losses always. Scout early, set a rally,
keep the Foundry queue warm, and attack-move (never plain move) into
territory you don't control.

## How it's put together

```
chassis/    reusable deterministic-sim toolkit: Q32.32 fixed point, PCG32,
            FNV state hashing, tile grid + A*, replay format. No game rules.
sim/        oxide-sim — every game rule, pure and headless. One entry point:
            State::tick(commands). No floats, no clocks, no hash maps.
protocol/   debug-protocol types (JSON lines) + agent-readable state views
shell/      macroquad renderer, single input funnel, debug server. Disposable.
driver/     CLI harness: headless runs, replay verification, byte-exact
            golden images (CPU-rendered), live-game client, smoke test
scenarios/  match definitions with ASCII maps
tools/      sprite + sound generators (Python — uv run tools/gen_*.py)
assets/     the generated sprites and sounds, committed
```

The load-bearing rule: **same scenario + same command log ⇒ bit-identical
state, on every platform**. Commands are tick-stamped and everything that
issues them — mouse, bot, debug socket — goes through one funnel, so a
replay (`setup + commands`) *is* the session. The determinism rules and the
tooling contract live in [AGENTS.md](AGENTS.md).

## Driving it without hands

Start the shell with a socket, then talk to it:

```sh
cargo run -p oxide-shell -- --debug-server --paused   # driven mode
cargo run -p oxide-driver -- live status
cargo run -p oxide-driver -- live harvest 0 --units 0,1,2 --node 7,2
cargo run -p oxide-driver -- live attack-move 0 --units 3 --to 34,18
cargo run -p oxide-driver -- live advance 300         # exactly 300 ticks
cargo run -p oxide-driver -- live screenshot -o screenshots/now.png
cargo run -p oxide-driver -- live inject-wheel 2      # real input funnel
cargo run -p oxide-driver -- live save-replay replays/session.json
cargo run -p oxide-driver -- replay replays/session.json   # → same hash
cargo run -p oxide-driver -- live load-replay replays/session.json  # resume
```

`live --help` lists the rest (state queries with ASCII maps, key/click
injection, camera, overlay, scenario loading). `smoke --spawn` runs the
whole sequence as an automated check.

## Testing

```sh
cargo test --workspace              # everything below, headless, no GPU
BLESS=1 cargo test -p oxide-driver  # re-bless goldens after intended changes
cargo run -p oxide-driver -- smoke --spawn   # live end-to-end (opens a window)
```

Four layers: sim unit tests; headless scenario/determinism tests (identical
runs, mid-run serde roundtrips, replay reproduction, bot-vs-bot to a
decisive end); golden images rendered by a CPU rasterizer and compared
byte-for-byte; and the live smoke drive. A full bot match simulates in well
under a second.

## Saving games

There is no separate save format, on purpose. In a deterministic sim a
replay *is* a save: `save-replay` writes the session's command log, and
loading it (`--replay file.json`, or `live load-replay`) rebuilds the
scenario and re-runs every tick — thousands per second, so "loading" a
long game takes well under a second — then keeps playing and recording
from exactly where you stood. Unlike a state snapshot, the save stays
replayable end-to-end and can never desync from its own history. The
trade-off: replays only reproduce on the sim version that wrote them.

## Status and road ahead

Working today: the full loop (harvest → train → fight → win) with fog of
war and ghost memory, solid units, attack-move, rally points, a fog-aware
minimap, sound, four maps, menus, a competent skirmish bot, save/resume
via replays, and the agent tooling described above.

Not yet: more unit and building types (the roster is deliberately tiny),
formations and control groups, and the mobile ports — macroquad makes
iOS/Android plausible, and `RawEvent` already carries touch variants, but
nothing is wired. The sim freezes at game end; the pause menu's Restart
is the rematch.

Built with [macroquad](https://macroquad.rs/); simulation math on the
[`fixed`](https://crates.io/crates/fixed) crate; goldens via
[tiny-skia](https://crates.io/crates/tiny-skia).
