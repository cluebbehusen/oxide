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

The front door offers Play, Tutorial, Replays, Settings, and Quit
(plus Continue when an autosave waits). The pause menu carries Save
Game — name the save inline (Enter accepts the suggested name) and
load it back any time from the Replays shelf, which shelves saves
and finished-match replays in their own sections. Settings holds live volume
buses, UI scale, camera feel, accessibility switches (reduced motion,
colorblind-safe accents, a left-handed preset), and full key
remapping — every change applies immediately and persists, explicit
unbindings included. The pause menu carries the same Settings, so a
live match can be retuned mid-game (the match waits, and Back returns
to the pause menu); a refused rebind says which verb already holds
the key. Mid-match the pause menu also offers Surrender (confirmed,
Cancel preselected, like every destructive choice): a 1v1 concession
ends the match on the spot and the normal stats and Watch Replay
flow takes over, while in a team game only your seat resigns — the
overlay shows your match-so-far numbers with Esc as the exit to the
menu, and dismissing it leaves you spectating while your ally plays
on. Play opens a thumbnail grid of every map,
sectioned by format, each card carrying a fog-free, theme-graded
preview — the selected map's blurb names its pace (with a measured
typical-duration band on the 1v1 maps) and scrap richness —
and every choice you make on the way to a
match survives backing up a screen. Selected
machines draw their weapon ranges (and radar rings); stalls and
rejections say why in words. Clicking anything opens its command
panel: portrait, sprite cards for everything it can do (costs,
hotkeys in the corner, reasons in red when a card refuses), and the
queue along the bottom — production ghosts with progress you can
click to cancel, or a unit's order program, where every chip wears
what it acts on: the turret it is raising, the works it is welding,
the machine it is chasing. Hovering a card tells
you what the machine is and exactly how it fights. A six-step
tutorial (Home → Tutorial) teaches by watching you actually do each
thing; guns aim at what they shoot, turrets track, downed flyers
fall, and battle sound sits in space — launches thump at the gun,
booms land at the impact, and distance dims both. Treads cycle while
machines move, building machinery works inside the sprite, construction rises
through visible stages, themed debris dresses each map, and an adaptive score
moves from the menus through calm industry into combat. When a match ends, a
full report separates units and buildings built and lost, peak army,
scrap collected, and every player's army curve, with actions to rematch,
watch the replay, or go home.

```sh
cargo run -p oxide-shell
```

A menu lists the shipped maps — the classic duels, the quick 2v2s
Twin Forges and Open Quarry, the big fields: Basalt Spine
(a plated barrier splits the map; two ground passes, one air-only door),
Ferric Reach (three lanes, long logistics), Parallel Works and
Paired Claims (large 2v2s), Continental Divide (a vast
plated barrier where the doors decide it), and the team-war fields —
Trident Plateau and Causeway Verdict (3v3), Compass Grand and
Gatework Array (4v4), lane wars where the
ridge doors carry the fight sideways — then opens one setup screen
for every map size: pick your chair from the seat cards (grouped
under team headings when the map has teams) and tune every
opponent's difficulty (**Easy, Medium, Hard, Expert**), personality
(**turtle, balanced, aggressive**, or let the map decide), and
faction in place beside a who-is-where preview. Start is
preselected, so Enter-Enter from the map grid still launches the
classic matchup — and the chips can now arrange what the old quick
questions never could: a mirror match, or your seat on the other
side of the map. Every
opponent is the same trained neural commander with different dials. Each
named personality has several seeded strategic variants, and teammates take
complementary jobs instead of following the same build in lockstep. The commander
sees only what its units see, plays by exactly your rules, and its
mistakes at lower settings are misjudgments, not lobotomies. On the
2v2 maps your teammate is that same mind, fighting beside you with
shared sight.

Eleven machines and eight buildings now. The shared core: **Harvesters**
feed the economy, build, salvage battlefield wrecks, and weld wounded
buildings and ground machines alike; **Sentinels** hold the line (and carry a weak anti-air
poke); **Scuttlers** eat undefended harvest lines; **Lancers**
outrange turrets and melt in reach; the **Bombard** shells beyond its
own eyes — someone must spot for it — and its blasts hurt everything
in the radius. The factions split on the sky: Ferrous flies the heavy
**Buzzard**, hunts with the **Talon**, and guards with the tanky
**Flakhound**; Cupric answers with the darting **Darter**, the swarm
**Wisp**, and the cheap **Stinger**. Air ignores terrain almost
entirely — only **peaks** (`^` on the map, plated exclusion barriers on screen) wall
the sky, block every shot across them, and break artillery arcs; only
anti-air weapons can touch a flyer. Bombard and Bastion shells are
real projectiles now: they fly, they can be dodged, and they land
where the target _was_.

Buildings: the **Foundry** trains the basics and anchors your defeat
condition; the **Fabricator** unlocks everything advanced including
the air wing; **Turrets** hold ground; **Flak Turrets** hold sky;
the **Bastion** is artillery in a fortress shell — full reach needs a
spotter; the **Array** is radar (true sight in close, unidentified
blips out to its ring); the **Reclaimer** grinds an early long-war scrap
trickle; the **Repair Bay** is a field
workshop — an unarmed ring that welds your wounded machines, ground
and air alike, billed per hp from your bank at the same rate a
harvester's torch charges. After a very long war, every surviving
Foundry also smelts a slow baseline trickle so an exhausted map cannot
lock the game forever; a Reclaimer starts earlier and works two and a
half times faster. If your last Harvester is destroyed, the Foundry
switches to a much faster emergency flow for one recovery package: a
replacement plus a cheap guard when no ground-fighting screen survives. The
reserve resets only after a Harvester brings salvage home; automatic Repair Bays
leave it untouched.
Deaths leave wreck salvage where
machines fall — winning a fight and holding the ground pays twice,
and throwing an army away literally funds the enemy. A Harvest order
anchors a local work zone: the named source stays authoritative, its route
avoids known danger when possible, and the crew autonomously clears only
safe remembered nodes and wrecks nearby. It returns to the same zone after
each delivery and retires beside its Foundry instead of drifting across the map. Construction
sites are attackable from the first tick, and cancelling one refunds
only what's still standing — damage burns salvage.

| Input                              | Action                                                                                                                                                                       |
| ---------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Left click / drag                  | Select units or buildings                                                                                                                                                    |
| Shift + click / drag               | Add to (or remove from) the selection                                                                                                                                        |
| Double-click a unit                | Select all visible units of that kind                                                                                                                                        |
| Ctrl + `1`-`5`                     | Assign the selection to a control group                                                                                                                                      |
| `1`-`5`                            | Recall the group — tap again to center the camera on it                                                                                                                      |
| Left click on minimap              | Jump the camera there                                                                                                                                                        |
| Right click                        | Contextual order: enemy → attack, scrap → harvest, ground → **advance** (keep moving while the primary weapon fires at in-range targets; never chase)                       |
| Shift + right click                | Queue the order behind the current one                                                                                                                                       |
| `M`                                | Units selected: arm run — click ground to move without firing or engaging, the strict disengage order (Esc cancels)                                                         |
| `F`                                | Units selected: arm attack-move — click ground to engage and chase enemies along the route (Esc cancels; Shift chains)                                                       |
| `R`                                | Arm a patrol: right-click waypoints, `R` again to start the loop — patrollers engage everything met and never settle                                                         |
| `B`                                | With harvesters selected: open the build palette — digits pick the structure, the ghost shows validity on ground you have seen (green claims now, amber on remembered ground sends the crew to found on arrival), click commits the whole selected crew, Esc cancels |
| Right click a damaged own building | With harvesters selected: weld it (billed per hp — pricier than building, cheaper than losing it)                                                                            |
| Right click a damaged own unit     | With harvesters selected: weld the machine (ground only; billed per hp against its cost). The torch holds only while welder and patient both stand still — a fleeing machine is chased, not healed |
| `W`                                | With harvesters selected: arm weld — click a damaged own ground unit, even one in your selection (Esc cancels; Shift chains)                                                 |
| Right click an own unfinished site | With harvesters selected: resume construction — several builders stack                                                                                                       |
| `V`                                | With harvesters selected: arm salvage — click an own built building to strip it for a partial refund (Foundries refuse; Shift chains teardowns)                              |
| `X`                                | Units selected: stop in place. Construction site selected: scrap it for a partial refund                                                                                     |
| Right click on minimap             | Advance the selection there without stopping to chase                                                                                                                        |
| Touch                              | Tap selects, drag pans, pinch zooms, two fingers box-select, and a still long-press performs the same contextual order as right click                                        |
| Right click (producers selected)   | Set the same rally for every selected producer; a scrap rally sends fresh Harvesters straight to work                                                                       |
| Right click enemy (defenses selected) | Focus every compatible selected Turret, Flak Turret, or Bastion on that visible target                                                                                  |
| Mouse wheel                        | Zoom (toward the cursor)                                                                                                                                                     |
| Arrow keys                         | Pan                                                                                                                                                                          |
| `H` / `S`                          | Train the first / second unit from the first selected producer that offers it                                                                                                |
| `1`-`9` (producers selected)       | Train by slot — the first compatible selected producer takes the order                                                                                                      |
| `Space`                            | Jump to your Foundry                                                                                                                                                         |
| `P`                                | Quick pause                                                                                                                                                                  |
| `Esc`                              | Deselect, then the pause menu (destructive choices ask first)                                                                                                                |
| `N`                                | Select and center the next idle harvester (the top bar counts them)                                                                                                          |
| `A`                                | Jump to the last under-attack alert                                                                                                                                          |
| Ctrl + `F5`-`F8` / `F5`-`F8`       | Set / recall camera bookmarks                                                                                                                                                |
| `F1`                               | Debug overlay (grid, ids, paths — and no fog)                                                                                                                                |

Ranged fire needs a clear line: rock blocks ground shots, so a
Sentinel behind cover must step out to fire — and so must the one
shooting at it. Buildings are not cover: they block movement, never
bullets. The air plays by different rules: the sky is
clear of everything except peaks, which wall it, block every shot
across them in any pairing, and break the indirect arcs (Bombard,
Bastion) that sail over mere rock. Guns that outrange their own eyes fire
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
keep the Foundry queue warm, advance through light resistance, and use
attack-move (`F`) when you need the army to clear territory instead of
holding its route.

## How it's put together

```
chassis/    reusable deterministic-sim toolkit: Q32.32 fixed point, PCG32,
            FNV state hashing, tile grid + A*, replay format. No game rules.
sim/        oxide-sim — every game rule, pure and headless. One entry point:
            State::tick(commands). No floats, no clocks, no hash maps.
protocol/   debug-protocol types (JSON lines) + agent-readable state views
            + the framed TCP transport both servers share
shell/      macroquad renderer, single input funnel, debug server. Disposable.
kit/        Shared toolkit: headless runner, replay playback + stats,
            the CPU software renderer behind goldens and previews
driver/     CLI harness: headless runs, replay verification, byte-exact
            golden images (CPU-rendered), live-game client, the
            windowless session server, smoke test
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
cargo run -p oxide-driver -- balance-probe          # value/body-time composition + entropy
cargo run -p oxide-driver -- repair-probe --weights sim/src/bot/ladder_weights.json
cargo run -p oxide-driver -- matchup --a sentinel:8 --b bombard:2,sentinel:4
cargo run -p oxide-driver -- bench                  # 500-unit ticks/s
cargo run -p oxide-driver -- live status
cargo run -p oxide-driver -- live harvest 0 --units 0,1,2 --node 7,2
cargo run -p oxide-driver -- live advance-units 0 --units 3 --to 34,18
cargo run -p oxide-driver -- live attack-move 0 --units 3 --to 34,18
cargo run -p oxide-driver -- live step 1              # effects + sim events
cargo run -p oxide-driver -- live advance 300         # exactly 300 ticks
cargo run -p oxide-driver -- live screenshot -o screenshots/now.png
cargo run -p oxide-driver -- live capture-sequence --present --out screenshots/motion
cargo run -p oxide-driver -- live inject-wheel 2      # real input funnel
cargo run -p oxide-driver -- live save-replay replays/session.json
cargo run -p oxide-driver -- replay replays/session.json   # → same hash
cargo run -p oxide-driver -- live load-replay replays/session.json  # resume
```

`live --help` lists the rest (state queries with ASCII maps, fog-honest
per-seat views, key/click injection, camera, overlay, scenario loading).
`smoke --spawn` runs the whole sequence as an automated check.

No window at all: `cargo run -p oxide-driver -- session` serves the same
protocol windowless (no GPU, sim time moves only on request), and every
`live` verb above works against it unchanged — screenshots come from the
CPU renderer, and a parity test holds the two servers to identical
answers.

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
Home offers Continue; while a match is running, the pause menu's Save
Game writes a named save (the name rides in the record's metadata, so
no filename rules to trip over); the Saves & Replays shelf lists every
record with honest version badges in two sections — saves load back
into a live session, finished matches watch — and any row deletes
with a deliberate double-X. Once a match is decided, Save Game gives
way to Watch Replay, which plays it back (replays are an end-of-match
affair — mid-match playback would scout the enemy through the fog);
and `--watch file.json` opens any record in the
read-only viewer — pause, seek both directions,
0.5x/1x/2x/4x/8x speed steps, free camera. Seeking backward restores
an in-memory checkpoint and re-simulates, so the viewer can never
diverge from the record.

Saves land atomically (a crash mid-write can never truncate a record),
and a save that fails — full disk, read-only folder — says so: quit
paths raise a Retry / Leave-without-saving dialog instead of exiting
silently, and an explicit save reports its verdict on the pause menu.
Autosave rotation keeps the newest five live sessions and
twenty finished matches, each pool on its own budget; named saves sit
in their own directory that rotation never touches — a save you asked
for is deleted only by you.

## Status and road ahead

Working today: the full loop (harvest → train → fight → win) with fog of
war and ghost memory, the two-faction eleven-unit roster (ground, air,
artillery) behind a build-your-tech gate, the harvester-built structure
palette from turrets to radar to Reclaimers, wreck salvage and repair
welding (buildings and ground machines alike), team games from 2v2 to
4v4 with shared sight, order queues
and patrols, solid
units that crowd without gridlocking, zero-chase advances and attack-move with line-of-sight
fire, damage retaliation, rally points, control groups, shift-select,
order feedback, a fog-aware minimap, sound, twenty-five maps in a
thumbnail-grid browser sectioned by format (sixteen duels, five 2v2s,
two 3v3s, two 4v4s), per-seat match setup on
team maps (team-grouped seat cards with inline difficulty,
personality, and faction dials beside a who-is-where map — every
seat's faction is free, yours included), building salvage as harvester labor, ally
inspection with visible orders and team-color accents on the machines
themselves (allied seats use distinct cool accents, hostile seats use distinct
warm accents, and your own keep pure faction paint), touch gestures (pan, tap, long-press,
pinch, two-finger box), menus, a trained neural opponent with four
difficulty levels and selectable personalities, save/resume via
replays, and the agent tooling described above.

Not yet: expansions (Foundries aren't buildable), formations,
free-for-all (the sim seats up to eight players and the menu lists any
scenario it can parse, but no shipped map plays FFA), and the mobile
ports — macroquad makes iOS/Android plausible, and the desktop shell
already resolves touch gestures, but no mobile build exists. The sim freezes at game
end; the pause menu's Restart is the rematch.

Built with [macroquad](https://macroquad.rs/); simulation math on the
[`fixed`](https://crates.io/crates/fixed) crate; goldens via
[tiny-skia](https://crates.io/crates/tiny-skia).
