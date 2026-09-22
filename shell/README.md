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
  active live or replay world through `game::Scene`. Its internal serde
  checkpoint adapter restores the shared session, tutorial progress, concession
  report, and decorative boundary exploration. Restoration opens paused and
  rebuilds transient presentation at the current viewport.
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
- `autosave`, `saves`, and the playback screens manage replay-backed
  persistence.

## Development

Run commands from the workspace root:

```sh
cargo run -p oxide-shell --release
cargo test -p oxide-shell --locked
cargo run -p oxide-driver -- smoke --spawn
```

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
