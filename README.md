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

The front door offers Play, Replays, Settings, and Quit (plus
Continue when an autosave waits). Settings holds live volume buses,
UI scale, camera feel, and full key remapping — every change applies
immediately and persists. The map list shows each map's hook and pace
badges with a fog-free, theme-graded preview, and every choice you
make on the way to a match survives backing up a screen. Selected
machines draw their weapon ranges (and radar rings); stalls and
rejections say why in words. Clicking anything opens its command
panel: portrait, sprite cards for everything it can do (costs,
hotkeys in the corner, reasons in red when a card refuses), and the
queue along the bottom — production ghosts with progress you can
click to cancel, or a unit's order program. Hovering a card tells
you what the machine is and exactly how it fights. A six-step
tutorial (Home → Tutorial) teaches by watching you actually do each
thing; guns aim at what they shoot, turrets track, downed flyers
fall, and battle sound sits in space — launches thump at the gun,
booms land at the impact, and distance dims both. When a match ends, the banner carries
the numbers: losses, peak army, closing scrap, and each side's army
curve over the whole fight.

```sh
cargo run -p oxide-shell
```

A menu lists the shipped maps — the classic duels, the quick 2v2s
Twin Forges and Open Quarry, and the new big fields: Basalt Spine
(a peak ridge splits the map; two ground passes, one air-only door),
Ferric Reach (three lanes, long logistics), and Parallel Works (a
large 2v2 built on quadrant symmetry) — then asks three questions: how hard
should the opponent think (**Easy, Medium, Hard, Expert**), who is it
(**turtle, balanced, aggressive**, or let the map decide), and which
faction you run (**Ferrous, Cupric**, or let the seed decide). Every
opponent is the same trained neural commander with different dials: it
sees only what its units see, plays by exactly your rules, and its
mistakes at lower settings are misjudgments, not lobotomies. On the
2v2 maps your teammate is that same mind, fighting beside you with
shared sight.

Eleven machines and seven buildings now. The shared core: **Harvesters**
feed the economy, build, salvage battlefield wrecks, and weld wounded
buildings; **Sentinels** hold the line (and carry a weak anti-air
poke); **Scuttlers** eat undefended harvest lines; **Lancers**
outrange turrets and melt in reach; the **Bombard** shells beyond its
own eyes — someone must spot for it — and its blasts hurt everything
in the radius. The factions split on the sky: Ferrous flies the heavy
**Buzzard**, hunts with the **Talon**, and guards with the tanky
**Flakhound**; Cupric answers with the darting **Darter**, the swarm
**Wisp**, and the cheap **Stinger**. Air ignores terrain almost
entirely — only **peaks** (`^` on the map, mountains on screen) wall
the sky, block every shot across them, and break artillery arcs; only
anti-air weapons can touch a flyer. Bombard and Bastion shells are
real projectiles now: they fly, they can be dodged, and they land
where the target _was_.

Buildings: the **Foundry** trains the basics and anchors your defeat
condition; the **Fabricator** unlocks everything advanced including
the air wing; **Turrets** hold ground; **Flak Turrets** hold sky;
the **Bastion** is artillery in a fortress shell — full reach needs a
spotter; the **Array** is radar (true sight in close, unidentified
blips out to its ring); the **Reclaimer** grinds a slow scrap trickle
so a long war never fully starves. Deaths leave wreck salvage where
machines fall — winning a fight and holding the ground pays twice,
and throwing an army away literally funds the enemy. Construction
sites are attackable from the first tick, and cancelling one refunds
only what's still standing — damage burns salvage.

| Input                              | Action                                                                                                                                                                       |
| ---------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Left click / drag                  | Select your units (click a Foundry to select it)                                                                                                                             |
| Shift + click / drag               | Add to (or remove from) the selection                                                                                                                                        |
| Double-click a unit                | Select all visible units of that kind                                                                                                                                        |
| Ctrl + `1`-`5`                     | Assign the selection to a control group                                                                                                                                      |
| `1`-`5`                            | Recall the group — tap again to center the camera on it                                                                                                                      |
| Left click on minimap              | Jump the camera there                                                                                                                                                        |
| Right click                        | Contextual order: enemy → attack, scrap → harvest, ground → **move engaging everything on the way** (fire at will is the only stance; combat units always defend themselves) |
| Shift + right click                | Queue the order behind the current one                                                                                                                                       |
| `R`                                | Arm a patrol: right-click waypoints, `R` again to start the loop — patrollers engage everything met and never settle                                                         |
| `B`                                | With a harvester selected: open the build palette — digits pick the structure, the ghost shows validity on ground you can currently see, click commits, Esc cancels          |
| Right click a damaged own building | With harvesters selected: weld it (costs a scrap trickle)                                                                                                                    |
| `X`                                | Units selected: stop in place. Construction site selected: scrap it for a partial refund                                                                                     |
| Right click on minimap             | Send the selection there, fighting through                                                                                                                                   |
| Right click (Foundry selected)     | Set the rally point — rally a scrap node and fresh harvesters mine it; fresh Sentinels attack-move to it                                                                     |
| Mouse wheel                        | Zoom (toward the cursor)                                                                                                                                                     |
| Arrow keys                         | Pan                                                                                                                                                                          |
| `H` / `S`                          | Train the selected factory's first / second unit                                                                                                                             |
| `1`-`9` (factory selected)         | Train by slot — the panel lists your faction's roster with prices                                                                                                            |
| `Space`                            | Jump to your Foundry                                                                                                                                                         |
| `P`                                | Quick pause                                                                                                                                                                  |
| `Esc`                              | Deselect, then the pause menu (destructive choices ask first)                                                                                                                |
| `N`                                | Select and center the next idle harvester (the top bar counts them)                                                                                                          |
| `A`                                | Jump to the last under-attack alert                                                                                                                                          |
| Ctrl + `F5`-`F8` / `F5`-`F8`       | Set / recall camera bookmarks                                                                                                                                                |
| `F1`                               | Debug overlay (grid, ids, paths — and no fog)                                                                                                                                |

Ranged fire needs a clear line: rock (and buildings) block ground
shots, so a Sentinel behind cover must step out to fire — and so must
the one shooting at it. The air plays by different rules: nothing
blocks a shot to or from the sky, and indirect shells (Bombard,
Bastion) arc over everything. Guns that outrange their own eyes fire
on your team's sight — kill the spotter and the guns go quiet. Every order answers back — a ground ping where it landed, a toast
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
kit/        Shared toolkit: headless runner, replay playback + stats,
            the CPU software renderer behind goldens and previews
driver/     CLI harness: headless runs, replay verification, byte-exact
            golden images (CPU-rendered), live-game client, smoke test
scenarios/  match definitions with ASCII maps
tools/      sprite + sound generators (Python — uv run tools/gen_*.py)
assets/     the generated sprites and sounds, committed
```

The load-bearing rule: **same scenario + same command log ⇒ bit-identical
state, on every platform**. Commands are tick-stamped and everything that
issues them — mouse, bot, debug socket — goes through one funnel, so a
replay (`setup + commands`) _is_ the session. The determinism rules and the
tooling contract live in [AGENTS.md](AGENTS.md).

## Driving it without hands

Start the shell with a socket, then talk to it:

```sh
cargo run -p oxide-shell -- --debug-server --paused   # driven mode
cargo run -p oxide-driver -- balance-probe          # composition + entropy
cargo run -p oxide-driver -- matchup --a sentinel:8 --b bombard:2,sentinel:4
cargo run -p oxide-driver -- bench                  # 500-unit ticks/s
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
replay _is_ a save: `save-replay` writes the session's command log, and
loading it (`--replay file.json`, or `live load-replay`) rebuilds the
scenario and re-runs every tick — thousands per second, so "loading" a
long game takes well under a second — then keeps playing and recording
from exactly where you stood. Unlike a state snapshot, the save stays
replayable end-to-end and can never desync from its own history. The
trade-off: replays only reproduce on the sim version that wrote them.

The shell wraps all of this: quitting a live match autosaves it and
Home offers Continue; the Replays screen lists every autosave and
local record with honest version badges (watch, or delete with a
deliberate double-X); once a match is decided the pause
menu's Watch Replay plays it back (replays are an end-of-match
affair — mid-match playback would scout the enemy through the fog);
and `--watch file.json` opens any record in the
read-only viewer — pause, seek both directions, speed steps, free
camera. Seeking backward restores an in-memory checkpoint and
re-simulates, so the viewer can never diverge from the record.

## Status and road ahead

Working today: the full loop (harvest → train → fight → win) with fog of
war and ghost memory, the two-faction eleven-unit roster (ground, air,
artillery) behind a build-your-tech gate, the harvester-built structure
palette from turrets to radar to Reclaimers, wreck salvage and repair
welding, 2v2 teams with shared sight, order queues and patrols, solid
units that crowd without gridlocking, attack-move with line-of-sight
fire, damage retaliation, rally points, control groups, shift-select,
order feedback, a fog-aware minimap, sound, ten maps, menus, a trained
neural opponent with four difficulty levels and selectable personalities,
save/resume via replays, and the agent tooling described above.

Not yet: expansions (Foundries aren't buildable), formations,
free-for-all (the sim seats up to eight players and the menu lists any
scenario it can parse, but no shipped map plays FFA), and the mobile
ports — macroquad makes iOS/Android plausible, and `RawEvent` already
carries touch variants, but nothing is wired. The sim freezes at game
end; the pause menu's Restart is the rematch.

Built with [macroquad](https://macroquad.rs/); simulation math on the
[`fixed`](https://crates.io/crates/fixed) crate; goldens via
[tiny-skia](https://crates.io/crates/tiny-skia).
