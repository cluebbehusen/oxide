# Oxide

Oxide is a 2D real-time strategy game about autonomous machines salvaging an
exhausted mining world. Ferrous, stained rust orange, and Cupric, crusted in
teal patina, compete for the last useful scrap at the bottom of abandoned
open-pit quarries.

It is also an experiment in AI-assisted game development. The game is built
around a pure deterministic simulation, a thin graphical shell, and a driver
that can inspect or play matches without a person at the keyboard. That
architecture makes bugs reproducible and lets every live match become a replay.

## Run the game

Oxide requires a current Rust toolchain. From the repository root:

```sh
cargo run -p oxide-shell --release
```

The main menu includes the tutorial, skirmish setup, saved games and replays,
the complete unit roster, settings, and controls. Normal skirmishes use one
deterministic, rules-based **Balanced AI**. It receives no extra resources,
information, build access, or combat advantages.

## The game

Harvesters bring scrap back to a Foundry, construct buildings, salvage wrecks,
and repair damaged machines. Foundries and specialized works turn that scrap
into an army. Destroy every enemy Foundry to win.

The important strategic layers are already present:

- real fog of war, remembered buildings, radar contacts, and shared team sight;
- ground, air, direct-fire, artillery, stealth, transport, and repair roles;
- terrain that distinguishes ordinary rock, impassable peaks, and open pits;
- buildable expansions, derelict Extractor frames, upgrades, and late-game
  recovery income;
- persistent wreck salvage, so holding a battlefield has economic value;
- free-for-all and team maps as well as conventional duels.

The in-game Roster is the best reference for exact units, buildings, costs,
weapons, and prerequisites.

## Essential controls

Most actions are available as clickable cards in the selection panel. These
shortcuts cover the core loop:

| Input                            | Action                                                                                           |
| -------------------------------- | ------------------------------------------------------------------------------------------------ |
| Left click / drag                | Select a unit, building, or group.                                                               |
| Shift + click / drag             | Add to or remove from the current selection.                                                     |
| Right click                      | Give the contextual order: harvest scrap, attack an enemy, repair an ally, or advance to ground. |
| Shift + right click              | Queue the contextual order.                                                                      |
| `F`, then ground                 | Attack-move and engage along the route.                                                          |
| `M`, then ground                 | Move without engaging.                                                                           |
| `R`                              | Mark patrol points; press `R` again to start the loop.                                           |
| `B` with Harvesters selected     | Open basic builds. Press `B` again, or click **Advanced builds**, for the second page.           |
| `B`, digit                       | Choose a basic structure. Advanced structures use `B`, `B`, digit.                               |
| `W`, then a damaged machine      | Order selected Harvesters to repair it.                                                          |
| `V`, then an own building        | Salvage the building for a partial refund. Foundries cannot be salvaged.                         |
| `X`                              | Stop selected units, or cancel a selected new construction site.                                 |
| `1`–`9` with a producer selected | Train the unit in that card slot.                                                                |
| Ctrl + `1`–`5` / `1`–`5`         | Assign or recall a control group.                                                                |
| `N`                              | Select and center the next idle Harvester.                                                       |
| `Space`                          | Center the camera on your Foundry.                                                               |
| `A`                              | Jump to the last under-attack alert.                                                             |
| `P`                              | Pause immediately.                                                                               |
| `Esc`                            | Cancel the active action, clear selection, or open the pause menu.                               |

The build ghost explains why a site is invalid. A discovered Extractor frame is
one 2x2 site. Amber placement on remembered ground creates a deferred order; the
builders walk there and validate the site only when they can see it. Upgrades
rebuild themselves on a fixed timer: the building stays offline and vulnerable,
but Harvesters keep their existing work.

Rock blocks direct ground fire. Peaks block ground movement, aircraft, and all
fire across them. Pits block ground movement, but aircraft and fire cross the
gap. Some units can shoot beyond their own vision if another friendly machine
provides sight.

## Architecture

The workspace is intentionally split at game-state boundaries. Each crate has
its own README with its purpose, main modules, and focused development commands.

- [`chassis/`](chassis/README.md) is the reusable deterministic-simulation
  foundation: fixed-point geometry, stable randomness and hashing, pathfinding,
  replay records, and durable writes. It contains no Oxide rules or rendering.
- [`sim/`](sim/README.md) is `oxide-sim`, the pure headless game. It owns every
  rule and bot; `State::tick(&[PlayerCommand])` is its only state transition.
- [`protocol/`](protocol/README.md) defines the JSON-lines debug contract,
  hardware-neutral input events, state views, fog-honest views, and transport
  shared by live and windowless sessions.
- [`kit/`](kit/README.md) contains Oxide-specific services shared by the shell
  and driver, including headless running, replay playback, statistics, combat
  fixtures, and the deterministic CPU renderer.
- [`shell/`](shell/README.md) is the playable Macroquad application. It owns
  input, UI, rendering, audio, persistence, and the live debug server, but may
  affect a match only by staging recorded commands.
- [`driver/`](driver/README.md) is the headless and live QA harness. It runs and
  inspects matches and replays, audits maps, drives the real shell, profiles
  frames, and serves the same protocol without a window.

Supporting directories:

- [`scenarios/`](scenarios/) contains shipped match definitions and ASCII maps.
- [`assets/`](assets/) contains production sprites and sounds.
- [`tools/`](tools/) contains deterministic asset generators and review tools.
- [`.agents/skills/`](.agents/skills/) contains maintained procedures for
  simulation work, scripted bots, live QA, maps, visual assets, and sound.

The load-bearing rule is:

> The same scenario and command log must produce a bit-identical state on every
> run and every supported platform.

Simulation code uses fixed-point arithmetic, explicitly ordered choices, and a
seeded PCG32 stream. Humans, bots, replays, and the debug socket all submit the
same tick-stamped commands. Rendering and audio observe the result; they never
feed back into it.

The implementation contracts are documented in
[`docs/simulation-architecture.md`](docs/simulation-architecture.md) and
[`docs/shell-architecture.md`](docs/shell-architecture.md).

## Development

To run the complete Rust gates:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
```

The driver is the main inspection surface:

```sh
cargo run -p oxide-driver -- --help
cargo run -p oxide-driver -- run scenarios/skirmish.json --ticks 12000 --all-bots --map
cargo run -p oxide-driver -- replay-summary replays/match.json --minimaps sparse
cargo run -p oxide-driver -- smoke --spawn
```

Use the headless runner for exact simulation questions and the native shell for
input, layout, animation, sound, or visual judgment. Aggregate match numbers can
find suspicious games; they cannot establish that a match was fun. Read the
replay summary, watch representative replays, and play changes yourself before
promoting them.

Repository invariants, workflow rules, versioning, and the skill router live in
[`AGENTS.md`](AGENTS.md).

## Saves and replays

There is no separate mutable save-state format. A save contains the starting
scenario and every tick-stamped command. Loading reconstructs the match by
replaying that record, then continues recording from the same history. Finished
matches use the same format in a read-only viewer.

Replays are pinned to the simulation version that wrote them. This makes
incompatibility explicit when a rules change would otherwise reproduce a
different world.

## Project status

Oxide is playable and under active development. It has a broad RTS ruleset, many
maps, a native desktop shell, deterministic saves and replays, and a deep
automation harness. The current focus is making that existing game clearer,
cleaner, and more enjoyable before adding more scale.

Built with [Macroquad](https://macroquad.rs/),
[`fixed`](https://crates.io/crates/fixed), and
[`tiny-skia`](https://crates.io/crates/tiny-skia).
