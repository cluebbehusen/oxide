# Agent instructions

Working notes for AI agents (and curious humans) developing this repository.

## What this is

**Oxide** — a small 2D RTS in Rust about self-replicating machines salvaging a
derelict world. Two factions, **Ferrous** (rust orange) and **Cupric** (teal
patina), harvest scrap, train units, and try to demolish each other's Foundry.

The game exists partly for its own sake and partly as a testbed for
agent-driven development. The architecture is optimized for *agent legibility*:
a pure, deterministic, headless simulation; a thin disposable renderer; and a
driver CLI that can test the sim headlessly or drive the live game over a
debug socket, including screenshots you can read back and inspect.

## Crate map

| Crate | Path | Purpose |
|---|---|---|
| `chassis` | `chassis/` | Reusable deterministic-sim toolkit: fixed-point math, PCG32 RNG, FNV-1a state hashing, tile grid + A*, replay format. No game rules, no engine deps. |
| `oxide-sim` | `sim/` | All Oxide game rules. Pure, deterministic, headless. Depends only on `chassis` + serde. |
| `oxide-protocol` | `protocol/` | Serde types for the debug protocol: requests, replies, raw input events, and human/agent-readable state views. |
| `oxide-shell` | `shell/` | macroquad renderer, input funnel, HUD, debug server. Thin — nothing in here may matter to game outcomes. |
| `oxide-driver` | `driver/` | CLI harness: headless scenario runs, replay verification, software-rendered goldens (tiny-skia), live-game client, automated smoke test. |

Built for reuse in a later, bigger game: `chassis` wholesale, the protocol's
envelope/raw-event design, and the driver's harness patterns. Game-specific
and disposable: `oxide-sim` rules, sprites, shell HUD.

## Determinism invariants — do not break these

Target: **same seed + same command log ⇒ bit-identical state on every run and
every platform.** Replays and regression hashes depend on it.

1. **No float arithmetic in `chassis` or `oxide-sim`.** Enforced by
   `clippy::float_arithmetic = deny` in those crates. Sim math is Q32.32 fixed
   point (`chassis::fx::Fx`). Floats are fine in shell/driver/protocol — they
   are presentation.
2. **Never iterate a `HashMap`/`HashSet` in sim logic.** Enforced by
   `clippy::disallowed_types` (see `clippy.toml`). Use `Vec` + stable ids or
   `BTreeMap`.
3. **All randomness through `chassis::rng::Pcg32`,** seeded from the scenario.
   Never from time, thread ids, or the OS.
4. **Commands are tick-stamped.** `setup + Vec<TimedCommand>` (the replay) is
   the *only* input to the sim. Any input path that bypasses the command
   stream breaks replays.
5. **Iterate entities in id order; tie-break every selection deterministically**
   (nearest, then lowest id — documented at each site).
6. **No wall clock in the sim.** `State::tick` is the only way time advances.

If a change legitimately alters sim behavior, regression hashes change too.
Re-bless (below) and explain the behavior change in the commit message.

## Build, test, bless

```sh
cargo test --workspace          # all tests (sim unit, determinism, goldens)
cargo clippy --workspace --all-targets
cargo fmt --all
BLESS=1 cargo test -p oxide-driver   # re-bless golden images / hashes after
                                     # an intentional behavior change
```

Before committing: fmt + clippy clean, tests green. When blessing, eyeball the
regenerated goldens (they are PNGs — read them) and justify the diff.

## Running and driving the game

```sh
cargo run -p oxide-shell                          # play: human vs bot
cargo run -p oxide-shell -- --debug-server        # + debug socket on :4123
cargo run -p oxide-shell -- --debug-server --paused   # driven mode: time only
                                                      # advances via the socket
cargo run -p oxide-driver -- live status          # talk to a running shell
cargo run -p oxide-driver -- smoke                # spawn shell + scripted checks
```

The driver's `live` subcommands map 1:1 to protocol methods (state, hash,
advance, inject-*, screenshot, save-replay…). Screenshots land in
`screenshots/` — read the PNG to verify visuals. See README for the full
command list and controls.

## Conventions

- **Conventional commits** (`feat(sim): …`, `fix(shell): …`, `docs: …`).
  Trunk-based on `main`.
- **Idiomatic Rust.** rustfmt defaults; clippy clean; document public items
  (`missing_docs` warns in the library crates). Comments explain constraints,
  not narration.
- **Sprites are generated, then committed.** Edit `tools/gen_sprites.py`
  (palette lives at the top), run `uv run tools/gen_sprites.py`, commit script
  and PNGs together.
- **Scenarios** live in `scenarios/*.json` — ASCII maps (`#` rock, `.` ground,
  `s` scrap node, `1`/`2` Foundry anchors) plus player/unit specs.
- Keep this file and README.md current when commands or behavior change.
