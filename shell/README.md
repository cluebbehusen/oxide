# oxide-shell

`oxide-shell` is the playable macroquad application. It turns input into
recorded simulation commands, advances the deterministic state, and presents the
result through rendering, sound, menus, saves, and replay playback.

The shell is deliberately a presentation layer. It may stage commands, but it
must never change game outcomes through camera state, frame timing, animation,
audio, or UI caches. The crate is a binary, and this README is also its
crate-level rustdoc.

## Main pieces

- `main` handles CLI arguments, window configuration, and startup.
- `app` owns frame orchestration and debug requests; `app/screen_flow` owns
  cross-screen transitions and draws one active screen.
- `screens/wizard` owns New Match seat, team, faction, and opponent choices;
  `bot_label` keeps configured opponent names consistent across the wizard, HUD,
  and result report. The Settings **Opponent AI** row picks the controller for
  every bot seat of a new match, Classic (`oxide-bot`) or New
  (`oxide-opponent`); rematches, saves and replays keep their recorded choice.
- `game` owns one live session, its recorder, and bots. `game::Presentation`
  holds camera, interpolation, effects, and UI state; rendering borrows the
  active live or replay world through `game::Scene`. Its checkpoint adapter
  restores the shared session, tutorial progress, concession report, and
  decorative boundary exploration. Restoration opens paused and rebuilds
  transient presentation at the current viewport.
- `input` and `action` form the single hardware and injected-input funnel.
  `press` and `button` give full-screen surfaces one press gesture and button
  style; `viewer_touch` pans and pinches the read-only viewers.
- `platform` states whether the build is touch-only (iOS).
- `building_actions` derives single and grouped building controls from their
  capabilities, using projected pending orders for eligibility and spending.
- `production` shares selected-factory purchases and collective queue
  cancellation across panel cards and shortcuts, including commands awaiting the
  next tick.
- `render`, `panel`, and `layout` draw the world, expose owner-safe selection
  feedback, and share hit-test geometry.
- `entity_lod` derives filtered entity textures for world rendering and UI
  portraits; `strategic_markers` draws role and allegiance cues at distant zoom.
- `assets`, `typography`, `audio_mix`, and `soundtrack` own presentation
  resources.
- `debug_server` connects the frame loop to `oxide-protocol`.
- `netplay` gathers LAN machines in a lobby and carries a running match between
  them over `oxide-net`; `screens/lobby` asks for a host address and shows the
  lobby's status. `text_field` is the one-line field it shares with the save
  name.
- `saved_game` owns compact checkpoint files with independently readable
  metadata. `autosave` owns atomic publication and retention; `saves` classifies
  checkpoints and recordings for the shelf. `app/persistence` runs capture
  encoding, restoration, catalog discovery, deletion, and recovery preparation
  on one bounded worker. Playback screens accept recordings.

## Development

The executable's build script captures its Git revision and dirty status from
the shell and shared dependency package trees, workspace manifests, Cargo
configuration, assets, scenarios, and shared build support. Private workspace
notes and the driver package do not contribute. Source archives report unknown
provenance. This metadata is observational and never enters simulation hashes.

Run commands from the workspace root:

```sh
cargo run -p oxide-shell --release
cargo test -p oxide-shell --locked
cargo run -p oxide-driver -- smoke --spawn
```

To play over a LAN or Tailscale, build the same commit on every machine. In the
game, the host sets other seats to Remote in match setup and starts the match;
the others pick Join Match and enter the host's address (the port defaults to
4200). From the command line, the host passes a scenario whose players are the
seats with `"bot": false`; the host takes the first, and joining machines fill
the rest in order:

```sh
cargo run -p oxide-shell --release -- --host 0.0.0.0:4200 --scenario duel.json
cargo run -p oxide-shell --release -- --join 192.168.1.20:4200
```

## iPad build

`ios/` wraps the shell in an Xcode project with a script phase that runs Cargo
for the device or simulator and places the binary where Xcode signs and packages
it. Its resource phase copies `assets/` and `scenarios/` and compiles the
app-icon catalog. It needs full Xcode (not only the Command Line Tools), the
`aarch64-apple-ios` Rust target (plus `aarch64-apple-ios-sim` for the
simulator), and an iPad with Developer Mode on. Set your signing team in the
ignored `ios/Local.xcconfig`, never in Xcode's Signing pane, which writes it
into the shared project file. If you sign with your own team, also set a bundle
ID you control there (the example file shows how).

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
cp ios/Local.xcconfig.example ios/Local.xcconfig   # set DEVELOPMENT_TEAM
cargo ios
```

`cargo ios` builds the Release app, installs it, and launches it on a paired
iPad or an iPad simulator. With several devices it lists them and asks, offering
the last choice as the default; `--device <name or id>` skips the question and
`--list` only prints the devices. A locked iPad still receives the install; open
Oxide yourself once it is unlocked.

iPadOS 27 requires the UIScene lifecycle, so the workspace pins an unreleased
miniquad commit; see the workspace `Cargo.toml` for why that exact commit.

`uv run tools/gen_icon.py` reproduces the shared desktop and iOS icon. The iOS
catalog receives an opaque square master; desktop window icons and the macOS
bundle use the same artwork with transparent outer corners. Commit both sets of
generated files when changing the icon.

## Sandbox sessions

Launch an authored scene with `--scenario path/to/scene.json`. Set its `mode` to
`"sandbox"` for optional Foundries and open-ended play without automatic
elimination or victory. Bots are optional: `bot: false` is a passive seat unless
local or debug input commands it. Local input uses the first non-bot seat, or
seat zero when every seat is bot-controlled. Launching a scene does not enable
or disable any configured controller. Normal New Match setup still chooses one
local seat and its opponents.

Saves and replays retain sandbox rules. Ownership, terrain, costs and ordinary
command validation still apply; sandbox mode does not grant free production or
control of other players' units. The debug protocol can explicitly attribute
commands to any seat, as in ordinary sessions.

## Recovery and diagnostics

Ordinary play preserves a recent completed match prefix in the platform data
folder. After an abnormal exit, Home offers **Recover match** and opens it
paused. Ordinary Continue and named saves remain separate.

Settings provides **Open diagnostics folder** (not on iPad) and **Export
diagnostic report**. Crash and freeze diagnostics are always on and write only
when a panic or stall occurs. Reports stay local and contain a replay, panic and
stall incidents with recent frame timing, and how the session ended.
`diagnostic_report` runs exports and folder operations off the frame thread;
`kit::recovery` and `kit::diagnostics` own bounded persistence.

See [the persistence contract](../docs/shell-architecture.md) for durability,
retention, compatibility, and timing semantics. Recovery warnings mean the
recording stopped at its last intact prefix, not that gameplay stopped.
