---
name: simulation-development
description:
  Change, debug, refactor, and validate Oxide's deterministic Rust simulation.
  Use for State, ticks, commands, entities, economy, construction, movement,
  combat, fog, teams, stats, serialization invariants, replay compatibility,
  state hashes, simulation performance, or splitting large sim modules.
---

# Oxide simulation development

Protect behavior at the command boundary. A clean abstraction is useful only if
the same scenario and command log still produce the intended exact state.

## Establish the contract

Read `sim/README.md`, `docs/simulation-architecture.md`, and the focused tests
for the subsystem before editing. Reduce a bug to one observable contract:

- the command or tick phase that owns the behavior;
- the state visible immediately before and after it;
- the expected event or refusal;
- whether a rejection must leave the hash unchanged;
- whether the change intentionally alters serialized state or simulation
  outcomes.

For repair behavior, cover limited-bank source priority, ordinary billing and
recovery reserves, active and queued salvage exclusion, and lethal same-tick
damage. Automatic and commanded work must preserve the same repair-versus-
salvage contract.

Add the smallest behavioral regression test that fails for the reported case.
For fog-sensitive behavior, pair the positive case with an unseen or
remembered-world case so the fix cannot become an information oracle.

## Keep state deterministic

- Use integers or `chassis::fx::Fx`; floats do not enter `chassis` or
  `oxide-sim`.
- Use `chassis::rng::Pcg32` with an explicit stream derived from the scenario.
- Iterate entities in id order. Sort other choices by a complete stable key
  ending in an id or `(y, x)`.
- Do not make outcomes depend on a hash collection's iteration order.
- Route every mutation through `State::tick`; add a narrow read accessor when a
  consumer needs more information.
- Validate new serialized fields in `State::validate_invariants` and add both a
  well-formed round trip and an adversarial forgery.
- Preserve the fixed tick phase order unless changing that order is the stated
  gameplay change.

Command lists may preserve original replay bytes while having set semantics at
dispatch. Sort and deduplicate ids where the command contract says duplicates
cannot multiply an effect. A rejected command should emit a useful reason and
otherwise be hash-inert.

## Refactor without hiding behavior

Split a large file only along an existing responsibility boundary. Prefer a
private child module that keeps the parent type's API and field privacy intact.
Move tests with the behavior when that improves navigation; do not combine a
mechanical move with unrelated rule changes.

Before and after a structural-only refactor, run the focused suite and compare
the relevant deterministic fixture. New public helpers deserve a real consumer;
remove compatibility wrappers and orphaned exports that no maintained path uses.

Comments explain an invariant, a non-obvious ordering dependency, or a known
failure mode. Remove comments that narrate implementation history, an obsolete
experiment, or what the next line already says.

## Validate in layers

During development:

```sh
cargo fmt --all --check
cargo test -p oxide-sim --locked <focused filter>
cargo clippy -p oxide-sim --all-targets --locked -- -D warnings
```

When commands or serialized state change, include these focused surfaces:

```sh
cargo test -p oxide-sim --test state_integrity --locked
cargo test -p oxide-sim --test command_canonicalization --locked
cargo test -p oxide-sim --test fuzz --locked
cargo test -p oxide-sim --test determinism --locked
```

Then run the workspace gates in `AGENTS.md`.

An intended rules change normally moves bot behavior or state hashes. Bump the
workspace simulation version before blessing changed existing rows, regenerate
with `BLESS=1 cargo test -p oxide-driver --locked`, and inspect every changed
fixture. Never use `BLESS_SAME_VERSION=1` merely to get a green run.

## Verify the real report

Headless tests prove rule behavior, not that the shell exposes it clearly. If a
bug was reported through the playable game, finish by using the `oxide-live-qa`
skill to repeat the actual interaction through the native input funnel and
inspect the result at normal play scale.
