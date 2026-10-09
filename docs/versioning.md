# Versioning

Oxide is pre-launch. Files and peers from other builds are not supported, and
every version below stays at its initial value until launch. Each version
already has the job it will need afterwards.

| Version                       | Defined in                                                                               | Gates                                                                                                                 |
| ----------------------------- | ---------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| Package version               | `Cargo.toml`                                                                             | Nothing. Players see it, and the iOS and macOS bundles use it as their version string.                                |
| Bundle build number           | `git rev-list --count HEAD`, stamped by `ios/build_rust.sh` and `tools/package_macos.sh` | App Store Connect, which needs a higher build number for each upload.                                                 |
| `oxide_sim::SIM_VERSION`      | `sim/src/lib.rs`                                                                         | Replays, player saves and session checkpoints recorded by another sim are refused. Hash fixtures carry it as a stamp. |
| `oxide_net::PROTOCOL_VERSION` | `net/src/lib.rs`                                                                         | Netplay Hello.                                                                                                        |
| Save format                   | `shell/src/saved_game.rs`                                                                | `.oxsave` files.                                                                                                      |
| Session checkpoint revision   | `kit/src/checkpoint.rs`                                                                  | Session checkpoints inside saves and recovery bundles.                                                                |
| Settings format               | `shell/src/config.rs`                                                                    | The settings file; a mismatch falls back to defaults.                                                                 |
| `OBSERVATION_VERSION`         | `sim/src/observation.rs`                                                                 | Nothing yet. It labels bot observations for consumers outside the game.                                               |
| Driver report versions        | `driver/src/replay_inspect.rs`, `driver/src/replay_summary.rs`, `driver/src/bot_eval.rs` | Nothing. They label JSON reports for scripts.                                                                         |

Each file type carries one format number, owned by the code that writes it, and
one sim version check.

Only builds archived from `main` go to App Store Connect: the commit count rises
along `main` but can repeat across branches.

## Before launch

- Never change a version, and never add code that reads another build's files.
- When a change moves a row of `driver/tests/goldens/state-hashes.json`,
  re-bless at the same version with
  `BLESS=1 BLESS_SAME_VERSION=1 cargo test -p oxide-driver --locked` and name
  the moved rows and the reason in the PR. Opponent fixtures re-bless with
  `BLESS=1` alone.
- Netplay also requires both sides to run the same commit, because a frozen
  `SIM_VERSION` cannot tell two development builds apart.

## At launch

- Netplay compares `PROTOCOL_VERSION` and `SIM_VERSION` instead of the commit.
  Hello does not carry `SIM_VERSION` yet.
- A moved state-hash row requires a new `SIM_VERSION`, and the bless gate's
  refusal says so instead of offering the same-version override.
- Each format version moves with its file's shape, and the package version moves
  with each release.
