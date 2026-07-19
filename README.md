# Oxide

A small 2D real-time strategy game about machines eating a dead world.

Two robot swarms — **Ferrous**, bleeding rust orange, and **Cupric**, crusted
in teal patina — wake up in the wreckage of some forgotten industry. Scrap is
food. Harvesters haul it home, Foundries smelt it into more machines, and
Sentinels make sure the other swarm doesn't get to. It ends when one side's
Foundry is a smoking crater.

Oxide is also an experiment: it is being built almost entirely by an AI agent,
and the architecture is shaped by that. The whole game is a pure, deterministic
simulation that runs headless at thousands of ticks per second; the renderer is
a thin shell over it; and a driver CLI can play, test, screenshot, and replay
the game without a human at the keyboard. The same properties that make an RTS
netcode-friendly (lockstep, command streams, fixed-point math) make it
machine-testable, which is the bet this repo is exploring.

*Status: under construction. This README grows as the game does.*

## Layout

```
chassis/    reusable deterministic-sim toolkit (fixed point, RNG, hashing,
            grid + A*, replays) — no game rules
sim/        oxide-sim: all game rules, pure and headless
protocol/   debug-protocol types shared by shell and driver
shell/      macroquad renderer + input + debug server
driver/     CLI: headless runs, replay verification, goldens, live driving
scenarios/  ASCII-map scenario files
tools/      sprite generator (Python, run with uv)
assets/     generated sprites (committed)
```

## Building

Rust stable (1.97+). `cargo test --workspace` runs everything headless — no
window, no GPU. `cargo run -p oxide-shell` starts the game proper.

More detail, including determinism rules and the debug protocol, in
[AGENTS.md](AGENTS.md).
