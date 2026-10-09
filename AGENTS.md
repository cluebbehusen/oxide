# Agent instructions

Oxide is a 2D RTS and an experiment in agent-assisted game development. Keep the
architecture legible: a pure deterministic simulation, a thin native shell, and
a harness that can inspect the same game headlessly or through the real window.

## Start here

Read the README for the crate you are changing:

| Crate                                  | Responsibility                                                                       |
| -------------------------------------- | ------------------------------------------------------------------------------------ |
| [`chassis`](chassis/README.md)         | Reusable deterministic primitives. No game rules or engine dependencies.             |
| [`oxide-sim`](sim/README.md)           | Game rules and player-knowledge projection.                                          |
| [`oxide-opponent`](opponent/README.md) | Reactive, best-effort opponent controller.                                           |
| [`oxide-protocol`](protocol/README.md) | Debug wire types, framing, input events, and state views.                            |
| [`oxide-kit`](kit/README.md)           | Shared replay, statistics, fixture, and CPU-rendering services.                      |
| [`oxide-net`](net/README.md)           | Lockstep multiplayer: wire messages, session core, start barrier, and TCP transport. |
| [`oxide-shell`](shell/README.md)       | Macroquad input, UI, rendering, audio, persistence, and live session.                |
| [`oxide-driver`](driver/README.md)     | Headless runner, inspectors, map audit, live client, profiling, and smoke QA.        |

Implementation contracts live in `docs/simulation-architecture.md` and
`docs/shell-architecture.md`. Keep those descriptive. `oxide-opponent` is
specified in `docs/oxide-opponent.md`. Put repeatable procedures in a skill and
historical results in notes or version control.

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

## Architecture and Rust quality

For a substantive change, establish who owns the data and when it expires,
whether it can be derived, how its cost scales with seats/units/map size, which
observable contract proves it works, and which existing mechanism it replaces or
extends. Apply these questions proportionately; trivial edits need no design
report.

Use enums for mutually exclusive states and required fields for facts a state
always needs; keep independent facts separate. Keep mutation and public APIs
narrow; prefer concrete interfaces until multiple real consumers justify
abstraction. Borrow shared inputs where practical, and justify retained data and
costly clones through ownership or measured need. Clippy and coverage support
review; they do not establish architectural quality.

Consider deterministic parallelism for expensive independent work after removing
duplication: frozen inputs, private mutable scratch, fixed work allowances and
canonical result ordering. Admission and shared mutation remain ordered; measure
overhead and contention before adding workers. `oxide-opponent` keeps its work
cheap instead: beyond the existing per-seat execution, it adds parallelism only
where a profile shows a real win.

## Determinism contract

The target is strict: **the same scenario plus the same command log produces
bit-identical state on every run and platform.**

- `chassis`, `oxide-sim`, and `oxide-opponent` contain no floating-point
  arithmetic. Use `chassis::fx::Fx`; floats are presentation-only.
- Never depend on `HashMap` or `HashSet` iteration for an outcome. Use complete,
  stable ordering. Geometric ties must also preserve the documented symmetry:
  use the existing query-, footprint-, or owner-relative ranks instead of
  introducing absolute id or row-major preferences.
- Once a `Scenario` exists, all outcome-relevant randomness comes from
  `chassis::rng::Pcg32` and a recorded seed or documented stream. Never consult
  time, threads, the OS, or ambient entropy during a match. The shell may use
  ambient entropy only to choose fresh opponent personality seeds before
  constructing a New Match scenario; those exact values must be recorded in it.
- `State::tick(&[PlayerCommand])` is the only game-state transition. Mouse,
  touch, bot, replay, and debug input all stage recorded commands.
- Simulation time is ticks only. Rendering, audio, presentation caches, debug
  reads and frame timing are observational.
- Preserve documented parity when a pass alternates direction for fairness.

## State and session boundaries

- Keep `State` fields private. Add a narrow immutable accessor instead of
  exposing or mutating internal collections.
- `State::validate_invariants` is the deserialization trust boundary. For each
  new serialized field, decide its invariants and cover reachable round trips
  plus meaningful malformed states. Reuse tests of the owning invariant rather
  than adding one assertion per field or duplicating Serde's primitive checks.
- Rejected commands leave authoritative state unchanged. Preserve set semantics
  by sorting and deduplicating id lists at dispatch.
- Player saves restore validated session checkpoints without executing history.
  Replays retain world origins and commands. Tick `N` is the state before
  commands stamped `N` execute; restoration never adds a hidden mutation.
- Controller checkpoints hold exactly the non-derivable state the
  `oxide-opponent` specification lists; scenario-derived data rebuilds from the
  bound scenario. Controller validation rejects state that could panic or cause
  unbounded work; a forged value that only changes play is accepted. A new
  checkpoint field needs a design review.
- Player knowledge has two fog-honest projections: `oxide_protocol::FogView` for
  players and agents, and `oxide_sim::observation::ObservationData::fog_honest`
  for bots. A parity test in `protocol/src/view/tests.rs` keeps them in
  agreement; change both together. Omniscient QA views must never feed a bot or
  player decision.
- Live, playback, and headless sessions share `oxide_protocol::DebugSession`.
  Explicitly refuse unsupported capabilities instead of faking them.
- Hardware and injected input enter through the same semantic event funnel and
  use logical coordinates. Apply DPI conversion only at the hardware adapter.

## Fair opponent contract

A bot is a command source, not an alternate ruleset. It receives fog-honest
information and shares human costs, prerequisites, queues, caps, build times,
movement, combat, and economy. Never hide bot-only income, vision, stats, legal
actions, or construction privileges behind controller code.

[`docs/oxide-opponent.md`](docs/oxide-opponent.md) is normative for
`oxide-opponent`, the reactive best-effort opponent. Work on that crate follows
its specification and the oxide-opponent skill. It must not introduce exact
cross-domain allocation, production forecasts, plan search or planning state
that spans decisions. Personality influences both cross-domain investment and
execution within a funded domain, but never access to information, strategies,
units, commands, or rules. Its counts come from need: a constant may bound
computation or model a difficulty, stance or personality limit, never what the
bot owns or sends.

Every bot seat of a normal match runs `oxide-opponent`. Scrapheap, Standard,
Veteran, and Prime alter fair macro competence plus cognitive and execution
limits: reaction time, attention, the margin an attack must bring, estimate
accuracy, and how well a seat attacks, focuses fire, and lifts. Turtle,
Balanced, and Aggressive bound its strategic posture. A deterministic per-seat
seed varies air, siege, support, fortification, greed, and guile priorities; it
never changes capabilities or unit strength.

Every difficulty retains the complete strategic repertoire. Automated metrics
surface candidates and failures; human play and replay judgment decide whether
behavior is credible or fun.

## Validation

Run focused tests while developing. The full validation gates run locally or
through the PR's CI:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
npx --yes prettier@3.9.6 --check "**/*.md"
.github/coverage.sh
```

Do not poll GitHub PR checks unless the user explicitly asks. While the PR's CI
is running a check, do not run the same check locally unless the user explicitly
asks. Stop any duplicate local run and leave that validation to CI. Report
pending checks as pending; do not wait for or repeatedly fetch their status.

Do not weaken a gate to pass it. Fix the implementation or discuss why the
contract is wrong.

The coverage script builds once for both gates: the unit gate counts library and
binary unit tests, and the combined gate adds the integration suites. The
combined gate skips the representative-map integrity soak because LLVM
instrumentation makes it expensive while adding little line coverage.
`cargo test --workspace --locked` runs it; the exhaustive all-map integrity soak
is opt-in. Compact controller contracts run under combined coverage.

Both floors measure production code. A module's unit tests live in a child
`tests.rs` or `*_tests.rs` file, never an inline `mod tests { … }`;
cargo-llvm-cov's default filter skips those files and everything under `tests/`.
A driver test fails on an inline or counted test module.

The `oxide-sim` integration suite compiles into one test binary: modules of
`tests/integration/main.rs`, whose guard test fails on an undeclared file. Add
new suites there rather than as separate files under `tests/`; each extra binary
recompiles and relinks against the workspace.

Hash fixtures supplement explicit behavior assertions in small, staged
scenarios. Do not pin autonomous match histories or require particular
personalities to win by a deadline. Stage the prerequisites for controller
decisions directly, and retain longer tests when the lifecycle, integrity, or
symmetry relationship itself needs that history.

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
uv run --with 'pillow==12.3.0' \
  -m unittest tools.test_gen_sprites tools.test_production_sprite_sources \
  tools.test_gen_icon
uv run tools/gen_sounds.py --check
uv run --with 'numpy==2.5.1' --with 'scipy==1.18.0' \
  -m unittest tools.test_gen_sounds
```

Use the real native shell for UI, input, animation, sound, and visual claims.
CPU screenshots prove schematic state, not presentation quality.

## Hashes, goldens, and versions

Oxide is pre-launch: saves, replays, settings and fixtures from other builds are
not supported. Never add code that reads an older format, and never change a
version number. [`docs/versioning.md`](docs/versioning.md) lists each version,
what it gates, and what changes at launch.

- Keep `driver/tests/goldens/state-hashes.json` as the cheap sim-drift tripwire.
  When a change moves its rows, inspect the drift, re-bless at the same version
  with `BLESS=1 BLESS_SAME_VERSION=1 cargo test -p oxide-driver --locked`, and
  name the moved rows and the reason in the PR.
- Fixtures driven only by `oxide-opponent` live in their own file and re-bless
  with `BLESS=1` alone; the PR includes a ladder-smoke comparison.
- Inspect changed PNGs. A green golden test cannot prove that art or layout is
  good.
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
- Update documentation when a change makes an existing claim inaccurate. Add
  architectural documentation only for significant boundaries, contracts, or
  responsibilities. Keep small implementation and visual details in code and
  tests.

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
