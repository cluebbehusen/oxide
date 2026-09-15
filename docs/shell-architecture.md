# Shell architecture

`oxide-shell` presents a deterministic match through a native macroquad window.
It owns wall-clock pacing, input, screen flow, rendering, audio, and persistence
UX. Shared replay and diagnostic services live in `oxide-kit`; the live and
headless debug surfaces share `oxide-protocol`.

Crate entry points are in the [shell README](../shell/README.md). Live-driving
commands and review procedures belong in the
[oxide-live-qa skill](../.agents/skills/oxide-live-qa/SKILL.md).

## Session ownership

`Game` owns one live session: its starting `Scenario`, authoritative `State`,
bot controllers, pending commands, replay recorder, and presentation state. The
latter includes camera, selection, interpolation, effects, audio cues, and
statistics.

`Game::do_tick` is the only live-shell path that advances state. It collects
pending human/debug commands and bot commands, records them at the current tick,
calls `State::tick`, then updates statistics and presentation from the result.
Fast advancement can suppress intermediate presentation work but uses the same
recorded tick path.

Live ticks and replay reconstruction use `oxide_kit::bot_execution`. Due bots
may think concurrently against the same immutable state; their work joins in
input seat order before commands are recorded. `State::tick` remains serial.

The shell may use floats, frame time, and interpolation to interpret interaction
and present a match. Only the resulting tick-stamped `PlayerCommand` crosses the
simulation boundary. Camera, audio, selection, and presentation caches cannot
change authoritative state. Seeks clear or rebuild timeline-local presentation.

`App` owns resources that outlive a screen: the live `Game`, configuration,
input funnel, New Match draft, tutorial, assets, audio services, debug channels,
and profiling collectors. Each `Screen` variant owns its local interaction
state. Settings and the Codex retain their return screen; Playback retains an
explicit return destination. The live game supplies the backdrop for pause,
results, and final-map inspection.

Screen modules consume `RawEvent` values and return semantic outcomes.
`app/screen_flow.rs` applies those outcomes and draws the active screen.
`app.rs` owns frame orchestration and debug request handling. Screen update
logic accepts injected viewport/input state so navigation can be tested without
a GPU window.

The New Match draft records each seat's difficulty and stance. Successful launch
materializes distinct personality seeds into the `Scenario`. Ordinary launches
seed the shell's seed source from wall-clock and process identity; automation
uses a fixed source. This is the pre-scenario entropy boundary. Restart,
Rematch, and replay resume preserve the recorded setup without consulting it
again.

## Frame and input flow

The frame loop:

1. Drains debug requests, deferring screenshot replies until rendering
   completes.
2. Polls hardware input, appends injected input, and routes the events.
3. Advances the live or playback clock unless paused or seeking.
4. Renders the active screen from interpolated presentation state.
5. Captures requested screenshots, replies, and yields to presentation.

`oxide_protocol::RawEvent` is the common hardware and injected-input vocabulary.
`input::apply_events` maps gameplay events into camera/selection changes and
staged commands. Cross-frame gestures, held keys, touch state, control groups,
and bindings belong to the input layer. The debug protocol's `SendCommand`
stages an already-semantic command and does not exercise UI mapping.

`action::BindingMap` owns contextual primary and secondary bindings shared by
input dispatch, UI hints, and settings. `ActionResolver` pairs releases with the
original presses so aliases and modifier changes preserve held actions. Keyboard
card actions use the same availability checks as clicks. Configuration migration
preserves explicit remaps and unbindings without displacing custom chords.

Selecting multiple own production buildings of one kind exposes their shared
roster. Each activation stages one ordinary `Train` per available factory in
building-id order, skips full or offline factories, and spends only available
scrap. Partial batches name the skipped reason. A command-phase projection
includes earlier staged purchases, refunds, and other spending before accepting
another activation.

The collective production dock groups paid units by kind, including active
heads, and shows total count, active count, and the next head's completion time.
A ready head may still await an open exit. Narrow windows use counted sprite
tiles with these details in the tooltip. Clicking a collective tile cancels one
waiting unit first, choosing the back of the lowest-id factory's queue; if only
active heads remain, it cancels the least-progressed head with building id
breaking ties. The tooltip identifies that target, and each click resolves
against pending commands again. Selecting one factory retains the exact ordered
queue and its per-slot cancellation controls. Mixed building kinds retain rally
controls and the first compatible producer's training shortcuts.

Production panels reserve a fixed-width rally group with adjacent Set/Reset and
Clear buttons. Clear stays visible but disabled until a selected producer has a
rally. Rally changes preserve the action band's height and production-card
positions. Narrow layouts widen the group to preserve minimum touch targets;
production wraps in the remaining columns. Card rectangles and panel bounds
share the same separator and inset calculations.

Coordinates are logical throughout the input/layout pipeline; the hardware
adapter applies DPI conversion once. Drawing publishes a shared `LayoutModel`
whose rectangles also drive hit testing. The HUD's supported layout floor is
1280×800 at default UI scale. Smaller windows are overflow stress cases.

Selections contain units of one allegiance or buildings of one owner, ordered by
id. Foreign entities can be inspected while visible, but commands remain gated
to the controlled seat. The simulation performs final ownership, fog, cost,
placement, and target validation. Dead entities and hostiles that leave sight
are removed from selection.

Selection panels and tooltips derive their facts from simulation accessors.
Static capabilities may be shown for foreign selections; current enemy orders,
loads, and building income remain private. Placement and support previews use
authoritative queries rather than duplicating game rules. Unknown concealed
mines cannot alter player-visible picking or placement feedback.

## Persistence and replay

A save is a replay: starting scenario, tick-stamped commands, simulation
version, and metadata. There is no independent mutable snapshot format. The live
replay recorder is always active. Autosaves represent resumable sessions;
decided matches are watchable records. Named saves persist until explicitly
deleted, while autosaves and finished matches rotate separately.

Save publication reserves a collision-free destination and uses the chassis
atomic-write path. Failures are reported to the player. Shelf discovery skips
malformed files and labels incompatible versions.

`Game::from_replay` reconstructs state from the recorded commands. Bots observe
reconstruction to restore their controller-local memory, but their regenerated
commands are discarded. The recorded log is authoritative, and the resumed game
continues recording onto it.

Ordinary play also keeps an incremental recovery journal through
`kit::recovery`. A bounded queue sends prepared commands and completed-tick
boundaries to a persistence worker. Only a contiguous, validated completed
prefix can be recovered; an unfinished tick is diagnostic evidence. Recovered
matches start paused.

The worker publishes durable progress separately from live progress. Storage
failure or queue exhaustion stops capture with a visible warning while gameplay
continues. Flush cadence is a target, not a guaranteed loss bound. Clean exit
requires a durable completion marker; force quit can preserve only bytes already
written. Recovery retires the interrupted source only after its replacement
baseline is durable and the original diagnostic evidence has been preserved. An
exclusive source claim prevents concurrent or stale callers from recovering an
already-retired record.

Filesystem leases protect active writers and report readers from retention.
Managed sessions have bounded counts and storage; active records and malformed
evidence are not silently deleted to admit a new recording. Explicit exports and
named saves are outside automatic recovery retention. Limits and journal framing
are defined in `kit::recovery`.

### Read-only playback

`oxide_kit::playback::Playback` owns replay-viewer state. It has no bots,
recorder, or new commands. Forward playback feeds the log into `State::tick`;
seeks restore an earlier in-memory checkpoint and replay the suffix. Checkpoint
storage is bounded, and the shell slices seeks across frames.

While Playback is visible, `App` retains a hidden live game. `PlaybackSession`
owns both the playback engine and a `Game` used as its render vehicle. Debug
state and clock requests target the playback engine; camera and overlay requests
target its render vehicle. UI, profiling, and diagnostic context describe the
visible session. Authoritative session mutations are refused.

Playback diagnostics retain the complete watched replay in a separate recording.
Its kind identifies viewer evidence, so it can be exported after a force quit
without appearing as a resumable live match. The hidden live game's recovery
history remains separate.

The ordinary shelf resumes unfinished records, preventing fog-free viewing from
scouting a live match. Completed records can be watched. Developer `--watch` may
inspect any compatible replay. Final Map is camera-only inspection of the final
live state.

## Performance and diagnostics

The player-facing Performance display is independent of diagnostic recording and
debug profiling. It observes completed frame measurements without changing fog,
pausing a match, or arming capture. Off adds no timing samples. FPS measures
frame-start intervals, including presentation waits; Detailed also measures CPU
work after debug handling and before `next_frame().await`. These are not GPU
timestamps. Screen/session changes reset the display's bounded history.

Detailed diagnostics are opt-in through Settings or `--diagnostics`. They use
`kit::diagnostics` to observe nested shell phases and per-seat bot work on the
normal executor. Input spans cover polling and event handling, ending before
clock advancement and drawing. Simulation callbacks are clock-free and
observational; diagnostic output never becomes replay input.

A worker retains bounded recent timings and slow operations, with dropped detail
and eviction reported explicitly. An independent watchdog reads atomic progress
for the main thread and bot workers, recording changes in the stalled worker set
and subsequent resumption. A presentation wait can include OS sleep or driver
delay; a suspected stall is not proof of deadlock. Panic metadata is queued
without waiting, and its persistence before process termination is best effort.

Report export runs off the frame thread. Reports contain a standard recovered
replay, unfinished-tick evidence, available diagnostics, and build provenance.
The completion manifest is published last and binds the replay digest; the
inspector rejects incomplete exports. Diagnostic files are independent
observations and may have different timestamps. Build differences remain visible
even when simulation versions permit playback. Reports stay local, and capture
does not automatically take screenshots or enable the debug server.

## Debug protocol

`oxide-protocol` owns JSON-lines envelopes, bounded TCP framing, input events,
and state views. The live server binds loopback. Socket threads parse requests
and pass them to the main thread; they never access the game. Requests execute
between frames, with screenshot replies deferred until GPU readback.

`DebugSession` and `dispatch_shared` define the common state and clock surface
for live play, read-only playback, and the driven headless session. Each
supplies its own clock semantics. Camera, input injection, native profiling, and
other window capabilities are supported only where they are real. Unsupported
operations are explicitly refused.

`StateView` is an omniscient QA representation, not an exact serialized state;
use `State::hash` for deterministic identity. `FogView::capture` is the shared
player-knowledge surface. It hides hostile intent and economy, exposes enemies
under current sight, and otherwise supplies only recorded ghosts, remembered
salvage, and anonymous radar contacts. Player-facing commands and effects must
respect that knowledge boundary.

## Rendering and audio

The GPU renderer owns camera clipping, fog composition, sprites, effects, and
screen chrome. Interpolation smooths fixed simulation ticks without moving
simulation entities. Presentation derives from authoritative events and state;
seeks reset interpolation and rebuild any persistent effects needed at the new
tick. Pause, speed, and reduced-motion behavior follow those presentation
clocks.

Destruction and projectile caches retain the pre-removal identity, pose, and
visibility needed to present an event after its entity has gone. Cosmetic
effects cannot reveal unseen events. Authoritative crash trajectories and impact
timing remain simulation-owned; rendering observes them without adding damage
rules.

`assets` loads the generated sprite atlas. Its manifest covers every resolved
sprite key; the renderer does not load individual sprite textures. Optional rigs
may fall back to composite sprites, but incomplete rig families are rejected.
Procedural quarry boundaries and pits derive from map geometry with fog-aware
visibility. Animation, heading, and weapon effects use the relevant simulation
state rather than inventing movement or firing delays.

`entity_lod` derives full, half, quarter, and eighth-resolution entity textures
at startup without changing authored atlas bytes. Regions pack in descending
size order to avoid wasting full-height rows on small mips. Independent regions
have extruded borders; reduction, linear sampling, level blending, and
compositing retain premultiplied alpha. Physical destination size, including DPI
and both axes, selects levels with a fixed -0.4 detail bias. Secondary UVs and
blend weights travel in vertex data; immutable page materials preserve batching
without reordering translucent layers. The material also handles ordinary
straight-alpha 2D draws and remains active until the screen boundary. Terrain
and unrelated effects keep nearest-neighbor sampling. Panel and roster portraits
share this bank, frame visible alpha bounds, and align to physical pixels.
Layered portraits use union bounds to preserve the relative positions of bases
and mounts. Construction subjects and scaffolds share the authored canvas; verb
pictograms fill their destination without portrait cropping.

`strategic_markers` supplies role and allegiance cues between 24 and 16 logical
pixels per tile by default. Buildings retain subdued footprints beneath their
markers; known unclaimed Extractor frames use amber brackets. Resource summaries
use world-anchored cells and follow the transition slightly later. Each cell
averages its tiles' visibility and the same bounded memory-age fade as world
salvage. Marker visibility shares player exploration, known claims, apparent
buildings, and remembered salvage with world rendering. Ghosts remain distinct
from live observations. Entity positions, selection, picking, and simulation are
unchanged.

Settings persists marker timing (Standard 24/16, Earlier 30/22, Later 18/10) and
size (75–150%) in `Config::markers`, applying changes immediately. Older configs
adopt the defaults without resetting other preferences. Custom config endpoints
are clamped to finite, ordered values. Logical marker dimensions keep
readability consistent across display densities; sprite sampling independently
uses physical pixels.

Simulation events enqueue audio cues. The mixer applies user buses, repetition
limits, and camera-relative attenuation. Continuous positional sounds are owned
and stopped individually; pause and screen transitions release them, and resumed
presentation can reconstruct them. Soundtrack state controls music beds and
crossfades. Audio never feeds a simulation decision. Production sprite and sound
bytes remain owned by their generators and approval workflows.

The tiny-skia renderer in `oxide-kit` produces whole-map CPU schematics. It does
not share the native atlas, camera, HUD, animation, or visual polish. Screenshot
responses identify their producer. CPU goldens prove deterministic schematic
composition; presentation and input claims require the real shell.

## Source and test map

| Contract                      | Primary source                                                                 | Behavioral evidence                                                        |
| ----------------------------- | ------------------------------------------------------------------------------ | -------------------------------------------------------------------------- |
| App ownership and screen flow | `shell/src/app.rs`, `shell/src/app/screen_flow.rs`, `shell/src/screens/`       | Screen tests, `driver/tests/menu_ux.rs`                                    |
| Live tick and recording       | `shell/src/game.rs`                                                            | Game tests, `driver/src/smoke.rs`                                          |
| Input and shared geometry     | `shell/src/input.rs`, `shell/src/layout.rs`, `shell/src/panel.rs`              | Input and layout tests                                                     |
| Saves and recovery            | `shell/src/autosave.rs`, `shell/src/saves.rs`, `kit/src/recovery/`             | Module tests                                                               |
| Diagnostic persistence        | `kit/src/diagnostics.rs`, `shell/src/diagnostic_report.rs`                     | Module tests                                                               |
| Playback and seeking          | `kit/src/playback.rs`, `shell/src/screens/playback.rs`                         | Playback tests                                                             |
| Protocol capabilities and fog | `protocol/src/session.rs`, `protocol/src/view.rs`, `shell/src/debug_server.rs` | Protocol tests, `driver/tests/session_parity.rs`                           |
| Native presentation           | `shell/src/render.rs`, `shell/src/assets.rs`                                   | Asset tests, `shell/tests/presentation_animation.rs`, native capture tests |
| CPU schematics                | `kit/src/render.rs`                                                            | `driver/tests/golden.rs`                                                   |
| Audio                         | `shell/src/audio_mix.rs`, `shell/src/soundtrack.rs`                            | Module tests                                                               |
