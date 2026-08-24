# Agent instructions

Oxide is a 2D RTS and an experiment in agent-assisted game development. Keep the
architecture legible: a pure deterministic simulation, a thin native shell, and
a harness that can inspect the same game headlessly or through the real window.

## Start here

Read the README for the crate you are changing:

| Crate                                  | Responsibility                                                                |
| -------------------------------------- | ----------------------------------------------------------------------------- |
| [`chassis`](chassis/README.md)         | Reusable deterministic primitives. No game rules or engine dependencies.      |
| [`oxide-sim`](sim/README.md)           | Game rules and command-producing bots.                                        |
| [`oxide-protocol`](protocol/README.md) | Debug wire types, framing, input events, and state views.                     |
| [`oxide-kit`](kit/README.md)           | Shared replay, statistics, fixture, and CPU-rendering services.               |
| [`oxide-shell`](shell/README.md)       | Macroquad input, UI, rendering, audio, persistence, and live session.         |
| [`oxide-driver`](driver/README.md)     | Headless runner, inspectors, map audit, live client, profiling, and smoke QA. |

Implementation contracts live in `docs/simulation-architecture.md` and
`docs/shell-architecture.md`. Keep those descriptive. Put repeatable procedures
in a skill and historical results in notes or version control.

## Keep instructions in their proper place

`AGENTS.md` is not a catch-all. Keep only repository-wide invariants,
boundaries, required gates, and workflow rules here.

Canonical skills live under `.agents/skills/`; `.claude/skills/` contains
relative links to the same directories. Skills describe their own scope and
trigger conditions. Read and follow the matching skill when a task calls for
one; do not duplicate a skill catalog here.

Put detailed or repeatable procedures in a focused skill. Put current factual
descriptions of the implementation in crate READMEs or architecture documents.
Track a multi-step workstream in a named note under `agent-notes/` only when the
user directs it. Once created, maintain that note through Kladde.

## Determinism contract

The target is strict: **same seed plus same command log produces bit-identical
state on every run and platform.**

- `chassis` and `oxide-sim` contain no floating-point arithmetic. Use
  `chassis::fx::Fx`; floats are presentation-only.
- Never depend on `HashMap` or `HashSet` iteration for an outcome. Iterate in a
  canonical order and finish tie-break keys with an id or `(y, x)`.
- All randomness comes from `chassis::rng::Pcg32` and a documented seed or
  stream. Never use time, threads, the OS, or ambient entropy.
- `State::tick(&[PlayerCommand])` is the only game-state transition. Mouse,
  touch, bot, replay, and debug input all stage recorded commands.
- Simulation time is ticks only. Rendering, audio, caches, debug reads, and
  frame timing are observational.
- Preserve documented parity when a pass alternates direction for fairness.

## State and session boundaries

- Keep `State` fields private. Add a narrow immutable accessor instead of
  exposing or mutating internal collections.
- `State::validate_invariants` is the deserialization trust boundary. Every new
  serialized field needs validation and an adversarial integrity test.
- Rejected commands leave authoritative state unchanged. Preserve set semantics
  by sorting and deduplicating id lists at dispatch.
- Saves are replays. Replay tick `N` is the state before commands stamped `N`
  execute. Never add a hidden resume mutation.
- `FogView` is the canonical player-knowledge surface. Omniscient QA views must
  never feed a bot or player decision.
- Live, playback, and headless sessions share `oxide_protocol::DebugSession`.
  Explicitly refuse unsupported capabilities instead of faking them.
- Hardware and injected input enter through the same semantic event funnel and
  use logical coordinates. Apply DPI conversion only at the hardware adapter.

## Fair opponent contract

A bot is a command source, not an alternate ruleset. It receives fog-honest
information and shares human costs, prerequisites, queues, caps, build times,
movement, combat, and economy. Never hide bot-only income, vision, stats, legal
actions, or construction privileges behind controller code.

Normal matches use one rules-based Balanced opponent. Improve that baseline
through observable behavior and complete-match review before inventing a
difficulty ladder. Automated metrics surface candidates and failures; human play
and replay judgment decide whether behavior is credible or fun.

## Validation

Run focused tests while developing, then all Rust gates before finishing:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
npx --yes prettier@3.9.6 --check "**/*.md"
cargo cov-unit
cargo cov-combined
```

Do not weaken a gate to pass it. Fix the implementation or discuss why the
contract is wrong.

The combined coverage gate skips the exhaustive shipped-map soak because LLVM
instrumentation turns that already-required normal test from seconds into tens
of minutes. `cargo test --workspace --locked` still runs the complete soak; the
coverage job measures the rest of the unit and integration surface.

Repeated builds, tests, and coverage runs accumulate incremental, profile, and
instrumented artifacts under `target/`; this can consume tens of gigabytes over
time. Run `cargo clean` at convenient checkpoints when disk space is tight or
after a heavy validation cycle, accepting that the next build will start cold.

Prettier covers every nonignored Markdown file, including agent notes and hidden
canonical skill files. Format Markdown with
`npx --yes prettier@3.9.6 --write "**/*.md"`; do not maintain wrapping by hand.

When a skill changes, validate every canonical skill directory. The
`.claude/skills/` entries are aliases and are not validated separately.

```sh
for skill_file in .agents/skills/*/SKILL.md; do
  skill_dir="${skill_file%/SKILL.md}"
  uvx --from skills-ref==0.1.1 agentskills validate "$skill_dir"
done
```

When assets or generators change, also run their deterministic checks:

```sh
uv run tools/gen_sprites.py --check
uv run --python 3.14 --with 'pillow==12.3.0' \
  -m unittest tools.test_gen_sprites tools.test_production_sprite_sources \
  tools.test_gen_icon
uv run tools/gen_sounds.py --check
uv run --with 'numpy==2.5.1' --with 'scipy==1.18.0' \
  -m unittest tools.test_gen_sounds
```

Use the real native shell for UI, input, animation, sound, and visual claims.
CPU screenshots prove schematic state, not presentation quality.

## Hashes, goldens, and versions

- Intended simulation changes move the workspace version and `SIM_VERSION`
  before existing state-hash rows are blessed.
- Regenerate driver fixtures with `BLESS=1 cargo test -p oxide-driver --locked`.
  `BLESS_SAME_VERSION=1` is an exceptional, explicitly justified override.
- Inspect changed PNGs. A green golden test cannot prove that art or layout is
  good.
- Keep `driver/tests/goldens/state-hashes.json` as the cheap sim-drift tripwire.
- A new `Command` variant must enter the fuzz generator's compiler-held tag
  surface and receive reach assertions.
- `shell/src/assets.rs` and `assets/sprites/atlas.json` remain a bijection over
  every sprite key the shell resolves.

## Repository workflow

- Never commit directly to `main`. Unless the user names a branch, use
  `cjl/<type>/<brief-description>`.
- Use signed conventional commits such as `fix(sim): validate extractor sites`.
  Do not add `Co-Authored-By` lines.
- Do not amend or force-push a branch that has been pushed for review. Add a new
  commit and push normally unless the user explicitly says otherwise.
- Preserve unrelated tracked, staged, and untracked work. Inspect the index and
  worktree before committing.
- Keep Rust rustfmt-formatted, Clippy-clean, and rustdoc-clean. Fix code rather
  than suppressing a useful lint.
- Comments explain constraints and non-obvious failure modes. They are not a
  diary, changelog, training narrative, or substitute for names and structure.
- Pin GitHub Actions to full commit SHAs and leave the release tag as a comment.
- Keep screenshots, replays, generated review banks, and experiments out of
  production commits unless the user explicitly promotes them.
- Update the root README, crate README, architecture document, and relevant
  skill when a maintained public behavior or workflow changes.

## Generated assets

`tools/gen_sprites.py` and `tools/gen_sounds.py` own production assets. Generate
experiments into a review directory, preserve approved files byte-for-byte, and
never hand-edit the generated atlas or checked-in sounds. Productionize only
assets the user explicitly approves. The visual and sound skills define the
detailed workflow.

## Cross-cutting pitfalls

- tiny-skia can assert on anti-aliased sub-pixel rectangles; keep AA off for
  tiny fills such as CPU-rendered health bars.
- A first attack may land on the command tick; event tests must retain that
  tick's `TickReport`.
- Fixed-point conversion to `i32` truncates toward zero. Use
  `TilePos::containing` for tile math.
- Macroquad windows on macOS run on the main thread. Socket work crosses into
  the frame loop by channel.
- Macroquad audio is feature-gated; removing the feature yields silent stubs.
- A paused shell stages socket commands for the next tick. Advance one tick
  before asserting their effects.
