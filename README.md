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

You're Ferrous, top-left. The bot is Cupric, and it is not asleep.

| Input | Action |
|---|---|
| Left click / drag | Select your units (click a Foundry to select it) |
| Right click | Contextual order: enemy → attack, scrap → harvest, ground → move |
| Mouse wheel | Zoom (toward the cursor) |
| Arrow keys | Pan |
| `H` / `S` | Train a Harvester (50) / Sentinel (75) |
| `Space` | Jump to your Foundry |
| `P` | Pause |
| `F1` | Debug overlay (grid, ids, paths) |
| `Esc` | Deselect |

Keep harvesters on scrap, keep the Foundry queue warm, and don't let your
army idle at home while the other swarm grows. Everything is visible — no
fog of war yet.

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
tools/      sprite generator (Python — uv run tools/gen_sprites.py)
assets/     the generated sprites, committed
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
cargo run -p oxide-driver -- live advance 300         # exactly 300 ticks
cargo run -p oxide-driver -- live screenshot -o screenshots/now.png
cargo run -p oxide-driver -- live inject-wheel 2      # real input funnel
cargo run -p oxide-driver -- live save-replay replays/session.json
cargo run -p oxide-driver -- replay replays/session.json   # → same hash
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

## Status and road ahead

Working today: the full loop (harvest → train → fight → win), a competent
skirmish bot, replays, goldens, and the agent tooling described above.

Not yet: attack-move and unit collision (units overlap softly), fog of war,
minimap, sound, more maps, and the mobile ports — macroquad makes iOS/Android
plausible, and `RawEvent` already carries touch variants, but nothing is
wired. The sim freezes at game end rather than offering a rematch. Replays
are only guaranteed against the sim version that recorded them.

Built with [macroquad](https://macroquad.rs/); simulation math on the
[`fixed`](https://crates.io/crates/fixed) crate; goldens via
[tiny-skia](https://crates.io/crates/tiny-skia).
