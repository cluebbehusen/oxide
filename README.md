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
Canyon, Verdigris Fields, Derelict Yard, Slagline, Open Circuit, Meridian
Scar — then asks two questions: how hard should the opponent think
(**Easy, Medium, Hard, Expert**), and who is it (**turtle, balanced,
aggressive**, or let the map decide). Every answer is the same trained
neural commander with different dials: it sees only what its units see,
plays by exactly your rules, and its mistakes at lower settings are
misjudgments, not lobotomies. You're Ferrous; the machine is Cupric,
and it is not asleep.

Four machines and three buildings. **Harvesters** feed the economy and
build; **Sentinels** hold the line; **Scuttlers** (fast, cheap, fragile)
eat undefended harvest lines; **Lancers** outrange everything including
turrets, and melt if anything reaches them. The **Foundry** trains the
basics and anchors your defeat condition; the **Fabricator** (built by a
harvester) unlocks the advanced pair; **Turrets** hold ground on their
own. Construction sites are attackable from the first tick, and
cancelling one refunds only what's still standing — damage burns salvage.

| Input | Action |
|---|---|
| Left click / drag | Select your units (click a Foundry to select it) |
| Shift + click / drag | Add to (or remove from) the selection |
| Double-click a unit | Select all visible units of that kind |
| Ctrl + `1`-`5` | Assign the selection to a control group |
| `1`-`5` | Recall the group — tap again to center the camera on it |
| Left click on minimap | Jump the camera there |
| Right click | Contextual order: enemy → attack, scrap → harvest, ground → **move engaging everything on the way** (fire at will is the only stance; combat units always defend themselves) |
| Shift + right click | Queue the order behind the current one |
| `R` | Arm a patrol: right-click waypoints, `R` again to start the loop — patrollers engage everything met and never settle |
| `B` / `N` | With a harvester selected: place a Turret / Fabricator (ghost shows validity on ground you can currently see; click commits, Esc cancels) |
| `X` | Units selected: stop in place. Construction site selected: scrap it for a partial refund |
| Right click on minimap | Send the selection there, fighting through |
| Right click (Foundry selected) | Set the rally point — rally a scrap node and fresh harvesters mine it; fresh Sentinels attack-move to it |
| Mouse wheel | Zoom (toward the cursor) |
| Arrow keys | Pan |
| `H` / `S` | Train the selected factory's first / second unit (Foundry: harvester 50 / sentinel 75; Fabricator: scuttler 40 / lancer 110) |
| `Space` | Jump to your Foundry |
| `P` | Quick pause |
| `Esc` | Deselect, then the pause menu (resume / restart / main menu / quit) |
| `F1` | Debug overlay (grid, ids, paths — and no fog) |

Ranged fire needs a clear line: rock (and buildings) block shots, so a
Sentinel behind cover must step out to fire — and so must the one shooting
at it. Every order answers back — a ground ping where it landed, a toast
when it couldn't be done. Rich scrap nodes (the taller, denser piles)
hold double the salvage and are usually worth fighting over.

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

Four layers: sim unit tests plus a seeded command fuzzer (hostile input
must never panic or diverge); headless scenario/determinism tests
(identical runs, mid-run serde roundtrips, replay reproduction, bot-vs-bot
to a decisive end); golden images rendered by a CPU rasterizer and
compared byte-for-byte, alongside fixed state-hash fixtures for every
shipped map; and the live smoke drive. A full bot match simulates in well
under a second. CI (`.github/workflows/ci.yml`) runs the suite on
Linux/macOS/Windows and re-checks the hash fixtures on each — the
cross-platform determinism proof.

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
war and ghost memory, a four-unit roster behind a build-your-tech gate,
harvester-built turrets and factories, order queues and patrols, solid
units that crowd without gridlocking, attack-move with line-of-sight
fire, damage retaliation, rally points, control groups, shift-select,
order feedback, a fog-aware minimap, sound, eight maps, menus, a trained
neural opponent with four difficulty levels and selectable personalities,
save/resume via replays, and the agent tooling described above.

Not yet: expansions (Foundries aren't buildable), formations, teams and
free-for-all lobbies (the sim supports up to eight players; the menus
don't, yet), and the mobile ports —
macroquad makes iOS/Android plausible, and `RawEvent` already carries
touch variants, but nothing is wired. The sim freezes at game end; the
pause menu's Restart is the rematch.

Built with [macroquad](https://macroquad.rs/); simulation math on the
[`fixed`](https://crates.io/crates/fixed) crate; goldens via
[tiny-skia](https://crates.io/crates/tiny-skia).
