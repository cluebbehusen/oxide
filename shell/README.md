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
  and result report.
- `game` owns one live session, its recorder, and bots. `game::Presentation`
  holds camera, interpolation, effects, and UI state; rendering borrows the
  active live or replay world through `game::Scene`. Its checkpoint adapter
  restores the shared session, tutorial progress, concession report, and
  decorative boundary exploration. Restoration opens paused and rebuilds
  transient presentation at the current viewport.
- `input` and `action` form the single hardware and injected-input funnel.
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

Settings provides **Diagnostics**, **Open diagnostics folder**, and **Export
diagnostic report**. Detailed capture defaults Off; `--diagnostics` enables it
for one launch. Reports stay local and contain a replay plus available phase,
frame, and suspected-stall evidence. Diagnostic capture does not require the
debug server. After leaving a recorded playback, export retains that viewer's
report source until live play resumes. `diagnostic_report` runs exports and
folder operations off the frame thread; `kit::recovery` and `kit::diagnostics`
own bounded persistence.

See [the persistence contract](../docs/shell-architecture.md) for durability,
retention, compatibility, and timing semantics. Recovery warnings mean the
recording stopped at its last intact prefix, not that gameplay stopped.
