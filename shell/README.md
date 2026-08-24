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
- `game` owns one live session, its recorder, bots, and presentation state.
- `input` and `action` form the single hardware and injected-input funnel.
- `render`, `panel`, and `layout` draw the world and share hit-test geometry.
- `assets`, `audio_mix`, and `soundtrack` own presentation resources.
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
