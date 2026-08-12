# Agent instructions

Working contract for AI agents and humans developing Oxide. Keep this file
short and global: it defines invariants, required gates, and where specialized
procedures live. Historical experiments, release narratives, and one-off
measurements belong in research notes or version control, not here.

## What this is

**Oxide** is a small 2D RTS in Rust about self-replicating machines salvaging
an exhausted mining world. Ferrous (rust orange) and Cupric (teal patina)
harvest scrap, train units, and try to demolish each other's Foundry. See
`README.md` for the player-facing story and controls.

The repository is also a testbed for agent-driven game development. Its shape
is deliberate: a pure deterministic simulation, a thin renderer, and a driver
that can inspect the simulation headlessly or operate the real shell through a
debug socket.

## Crate map

| Crate | Path | Responsibility |
|---|---|---|
| `chassis` | `chassis/` | Reusable deterministic-simulation primitives: fixed point, PCG32, hashing, grids, pathfinding, replays, and durable writes. No game rules or engine dependencies. |
| `oxide-sim` | `sim/` | All game rules and bots. `State::tick(&[PlayerCommand])` is the only state transition. Bots are command sources outside the tick pipeline. |
| `oxide-protocol` | `protocol/` | JSON-lines wire contract, input events, state and fog views, framing, and the shared debug-session request surface. |
| `oxide-shell` | `shell/` | macroquad renderer, input funnel, HUD, audio, persistence UI, and live debug server. It may affect outcomes only by staging recorded commands. |
| `oxide-kit` | `kit/` | Shared replay, scenario, statistics, and CPU-rendering machinery used by the shell and driver. |
| `oxide-driver` | `driver/` | Headless runner, replay and map inspection, live client, windowless session server, profiling, goldens, and smoke automation. |

`chassis`, the protocol envelope, and the driver patterns are intended for
reuse. Oxide's rules, balance, sprites, and shell UI are game-specific.

## Route specialized work first

Read the matching skill before changing that area. The canonical copies live
under `.agents/skills/`; `.claude/skills/` contains relative symlinks so both
agent environments use the same contract.

- Live play, debug driving, screenshots, replay forensics, profiling, or native
  shell QA: `.agents/skills/oxide-live-qa/SKILL.md`.
- Scenario design, terrain, symmetry, seat pairing, or map gates:
  `.agents/skills/map-authoring/SKILL.md`.
- Neural policy, gym, training, widening, export, evaluation, or promotion:
  `.agents/skills/bot-training/SKILL.md`.
- Sprites, animation, projectiles, terrain art, scenery, or the atlas:
  `.agents/skills/visual-assets/SKILL.md`.
- Sound synthesis, audition, mixer behavior, alerts, or production audio:
  `.agents/skills/sound-design/SKILL.md`.

Skills contain procedures and current command inventories. This file remains
the authority for cross-cutting invariants if a skill drifts.

Current implementation contracts live in `docs/simulation-architecture.md`
and `docs/shell-architecture.md`. Keep those documents descriptive and keep
repeatable procedures in skills.

## Determinism and simulation invariants

The target is strict: **same seed plus same command log produces bit-identical
state on every run and every platform.** Replays, hashes, and cross-platform CI
enforce it.

1. `chassis` and `oxide-sim` use no floating-point arithmetic. The workspace
   denies it there. Simulation math is `chassis::fx::Fx` (Q32.32); floats are
   presentation-only in the shell, driver, and protocol views.
2. Never make simulation outcomes depend on `HashMap` or `HashSet` iteration.
   Iterate entities in id order and end every choice's explicit tie-break key
   with an id or `(y, x)`.
3. All randomness comes from `chassis::rng::Pcg32`, seeded by the scenario.
   Never use wall time, thread ids, the OS, or nondeterministic entropy.
4. `State::tick(&[PlayerCommand])` is the only game-state transition. Every
   command source, including mouse, touch, bot, and debug socket, must stage a
   tick-stamped command through the recorder before execution.
5. Simulation time advances only by ticks. The shell accumulator and driven
   clock must bottom out in the same tick path; the sim never reads a clock.
6. Bots are ordinary, fog-honest command sources. Omniscient QA views such as
   `query_state` must never feed player or bot decisions.
7. Presentation is observational. Rendering, audio, UI caches, profiling, and
   debug reads must not alter state, command order, randomness, or timing.
8. When iteration direction alternates for fairness, preserve its documented
   parity and all explicit within-pass ordering. Do not replace an order with
   an unordered collection even when current tests happen to pass.

### State, replay, and protocol boundaries

- `State` fields remain private. Add a narrow accessor when a consumer needs a
  new view; do not expose mutable state.
- `State::validate_invariants` is the deserialization trust boundary. Every new
  serialized field needs validation and adversarial coverage in
  `sim/tests/state_integrity.rs`.
- Saves are replays. Loading reconstructs the scenario from its recorded
  commands and resumes recording the same log. Do not add an unrecorded resume
  mutation or silently accept an incompatible simulation version.
- Replay tick `N` means the state before commands stamped `N` execute. Preserve
  that convention in inspectors, viewers, and tests.
- The live shell, replay viewer, and headless session share protocol behavior
  through `oxide_protocol::DebugSession`. Capability-specific requests should
  be explicitly refused where unavailable, never simulated dishonestly.
- `FogView` is the shared source of fog-honest knowledge. Ghosts, radar
  contacts, and remembered salvage must have canonical ordering.
- Input enters through one semantic event funnel. New hardware paths must stage
  the same commands as existing mouse, keyboard, and touch paths.

## Required validation

Run the relevant focused tests while developing, then run the complete Rust
gates before considering a code change finished:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
```

When training Python changes, run its own lint, format, type, and test suite:

```sh
cd tools/train
uv run ruff check .
uv run ruff format --check .
uv run ty check
uv run pytest -q
```

When assets or their generators change, also run:

```sh
uv run tools/gen_sprites.py --check
uv run --python 3.14 --with 'pillow==12.3.0' \
  -m unittest tools.test_gen_sprites tools.test_production_sprite_sources
uv run tools/gen_sounds.py --check
uv run --with 'numpy==2.5.1' --with 'scipy==1.18.0' \
  -m unittest tools.test_gen_sounds
```

CI additionally proves hashes on Linux, macOS, and Windows, tests the declared
MSRV, checks the Rust/Python gym handshake, and runs `cargo deny`. Do not weaken
a gate to make a change pass. Fix the implementation or discuss why the rule is
wrong.

### Hashes, goldens, and versioning

- Intended simulation changes move state hashes. The branch must carry the new
  workspace and `SIM_VERSION` before blessing changed existing rows; replays
  and autosaves are version-pinned.
- Regenerate driver fixtures with
  `BLESS=1 cargo test -p oxide-driver --locked`. The fixture refuses same-version
  hash movement. `BLESS_SAME_VERSION=1` is an exceptional, explicitly
  justified override, not a convenience.
- Inspect regenerated PNGs with your eyes. A green blessing command does not
  establish that the visual result is good.
- `driver/tests/goldens/state-hashes.json` is the cheap sim-drift tripwire.
  `showcase.png` must continue to exercise every CPU-renderer branch named by
  its companion coverage test.
- `shell/src/assets.rs` and `assets/sprites/atlas.json` must remain a bijection
  over every sprite key the shell resolves.
- Bot behavior changes owe the liveness sweep and the bot-training skill's
  evaluation battery. Recalibrate authored liveness floors only from measured
  data, never merely to admit a regression.
- A new `Command` variant must be added to the fuzz generator's compiler-held
  tag surface and receive reach assertions.

## Repository workflow

- Never commit directly to `main`. Use the user's requested release branch when
  one is explicitly named; otherwise use `cjl/<type>/<brief-description>`.
- Use signed conventional commits such as `feat(sim): add retreat stance` or
  `fix(shell): align selection halo`. Do not add `Co-Authored-By` lines.
- Do not rewrite a pushed branch to answer review. Add a new commit and push
  normally unless the user explicitly authorizes history rewriting.
- Keep changes focused. Preserve unrelated tracked, staged, and untracked work
  in a dirty tree, and inspect both index and working tree before committing.
- Rust is rustfmt-formatted and Clippy-clean. Library documentation warnings
  matter; comments explain constraints rather than narrating code.
- Pin every GitHub Action to a full commit SHA and leave its release tag in a
  comment. Never rely on a mutable action tag.
- `screenshots/`, `replays/`, and review banks are scratch output. Permanent
  fixtures belong under crate test directories.
- Keep `README.md` and the relevant skill current when public behavior or a
  maintained workflow changes. Do not grow this router with procedural detail.

## Generated assets and approval boundaries

Sprites and sounds are generated source artifacts:

- `tools/gen_sprites.py` owns production sprites and packs the single
  `atlas.png`/`atlas.json` texture consumed by the shell.
- `tools/gen_sounds.py` owns production sound effects through deterministic,
  pinned synthesis.
- Run generators with `uv run`. Both accept `--out DIR` for review output and
  reserve `--check` for reproducibility; those modes are mutually exclusive.
- Commit a generator change with its production output. Never hand-edit the
  generated atlas or checked-in sound files.
- Production commits contain only assets the user explicitly finalized.
  Audition banks, alternatives, rejected attempts, and direction boards remain
  local until the user requests a separate cleanup or archive operation.
- Preserve approved assets byte-for-byte while experimenting. Generate variants
  into a review directory rather than overwriting production.
- The shell draws from the atlas, not per-sprite textures. Preserve the 1-pixel
  edge extrusion that prevents sampling bleed.
- Palette constants are duplicated in `tools/gen_sprites.py`,
  `kit/src/render.rs`, and `shell/src/render.rs`; keep them synchronized.

The visual-assets and sound-design skills define the detailed review ledger,
promotion, and verification workflows.

## Map and balance conventions

Scenario maps are JSON with an ASCII grid:

| Byte | Meaning |
|---|---|
| `.` | Ground |
| `,` | Cosmetic rubble ground |
| `#` | Rock; blocks ground only |
| `^` | Connected uncut-quarry mesa; blocks ground, air, and fire |
| `s` / `S` | Normal / rich scrap node |
| `1`–`8` | Foundry anchor, the top-left tile of its 2x2 footprint |

`w` is a rendered wreck marker and is not authorable scenario terrain.

- Shipped maps are 180-degree symmetric. Author terrain, resources, anchors,
  and starting units in mirrored pairs.
- Derive seat pairing from rotated Foundry anchors; never assume a numeric
  pairing. The relation must be an involution, paired seats must oppose one
  another, and paired starts must have equal scrap and role-equivalent mirrored
  unit lists.
- Team ids normalize densely by first appearance. An omitted team means a team
  of one; assigning every seat to one team is invalid.
- Even seats default to Ferrous and odd seats to Cupric, but launch-time retint
  is supported and must remap faction-derived names and unit roles together.
- Large team maps additionally require equal resources across every seat and
  unique player labels. The map gates and `map-audit` are the authority.
- All gameplay balance constants live in `sim/src/stats.rs`. Expect hashes and
  sometimes goldens to move when changing them.

Use `.agents/skills/map-authoring/SKILL.md` for the complete symmetry rules,
audit commands, terrain design, and large-map validation.

## World and art identity

Oxide is a lo-fi, top-down salvage RTS fought on the floors of exhausted
open-pit quarries that once drove a vibrant futuristic gold rush. Terraced cuts
and unmined shelves rise from the battlefield into darkness. The corporations
left after the resource rush, leaving autonomous fleets to dismantle the
abandoned operation and one another for the last recoverable value. The tone is
industrial, lonely, and faintly eerie rather than horrific or relentlessly
bleak. Bold silhouettes, restrained detail, readable mechanisms, charcoal,
rust, patina, and faded corporate paint matter more than realism, cheerful
saturation, or generic science-fiction glow. Detailed sprite, animation, and
audio principles live in the visual-assets and sound-design skills.

## Bots and the level-playing-field contract

The shipped opponent is a quantized neural policy evaluated with
deterministic integer math. Difficulty changes execution cadence and
hesitation around one strategic policy. Personality changes deterministic
preferences and team role; it does not grant information or legal actions.

The level-playing-field contract is non-negotiable: **a bot is a command source,
not an alternate ruleset.** It receives only fog-honest information available
to a player, and it shares player eligibility for counts, queues, costs,
prerequisites, commands, build times, movement, combat, and economy.

Controller architecture may use documented action abstractions, masks, opening
milestones, recovery overrides, or persistent plans to choose among shared-legal
actions. These mechanisms require liveness and effectiveness gates. They must
not become hidden income, stats, information, prerequisites, queue limits,
structure limits, or other bot-only rules. If a legal strategy is dominant or
pathological, fix shared balance or training rather than concealing it from the
bot. Difficulty can degrade execution; personality can alter preferences;
neither changes the strategy surface.

### 0.15 status: the promoted actor ships

The shipped opponent is the auto-2 autopilot champion (0.15.2 rules:
harvester replan stagger, base-Array charge detection), embedded at
`sim/src/bot/ladder_weights.json` and seated by `seat_bots` for every
configured bot seat. The gym v9 surface is parity-clean by
construction: the mask encodes shared legality only. `BotConfig`
(level, personality) is the authored scenario data it consumes; the
Level ladder's execution handicaps are re-measured for each promoted
actor with the `ladder_handicap_sweep` instrument. Provenance,
battery results, and known residuals live in
`.agents/skills/bot-training/references/artifact-lineage.md`.

The Overseer (`Brain::overseer`) remains the only scripted commander:
demonstration source, curriculum anchor, evaluation yardstick, and the
anchor for liveness, determinism, and hash gates. It is training and
QA infrastructure only, deliberately unreachable from any player-facing
surface (no scenario field, no wizard dial, no `SeatBot` arm). Keep it
that way.

A replacement actor must pass the complete native-Q12, faction/seat,
composition, profile, liveness, ladder, and determinism battery before
promotion. Never bless a new actor merely because it runs. See
`.agents/skills/bot-training/SKILL.md` for the gym contract, training
commands, and promotion procedure.

## Cross-cutting gotchas

- tiny-skia's anti-aliased path can assert on sub-pixel rectangles. Keep AA off
  for tiny rectangle fills such as CPU-rendered health bars.
- A fight's first attack can land on the same tick as its command. Event tests
  must retain the command tick's `TickReport`.
- `fixed` conversion to `i32` truncates toward zero. Floor first for tile math;
  use `TilePos::containing` instead of reproducing the conversion.
- A macroquad window on macOS must run on the main thread. Socket work crosses
  into the frame loop by channel; do not answer game requests from a window
  thread substitute.
- macroquad audio is feature-gated. Removing `features = ["audio"]` produces
  silent stubs rather than a useful build failure.
- Injected pointer events use logical coordinates, while Retina screenshots use
  physical pixels. The raw hardware stream is scaled once at the adapter; never
  apply DPI conversion twice.
- A paused shell stages socket commands for the next tick. Advance one tick
  before asserting their effects.
- Exact-order command payloads can have set semantics at dispatch. Preserve the
  recorded bytes, but sort and deduplicate where the command contract requires
  a set so duplicate ids cannot multiply effects.
