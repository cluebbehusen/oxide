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
`Presentation` member holds camera, selection, interpolation, effects, and audio
cues. Statistics, recording, and bot execution stay with the live session.
Rendering and read-only UI queries receive a borrowed `Scene`: the active world,
scenario, pending commands, and presentation. Neither `Presentation` nor `Scene`
owns or advances a simulation. Each view prepares a small stack-resident
seat-style table from the current viewer, teams, factions, and colorblind
setting. World, minimap, and result rendering share those lookups; no per-entity
player scan or mutable identity cache is needed. The table supports the scenario
seat limit.

`Game::do_tick` is the only live-shell path that advances state. It collects
pending human/debug commands and bot commands, records them at the current tick,
calls `State::tick`, then updates statistics and presentation from the result.
Fast advancement can suppress intermediate presentation work but uses the same
recorded tick path.

Local input uses the first non-bot seat in scenario order, or seat zero for an
all-bot scene. This choice does not change the configured controllers. Live
scenarios, replay continuation and checkpoint restoration accept any valid bot
roster, including no bots; the New Match wizard still authors one local seat.
Sandbox completion rules belong to the simulation, so headless and native
sessions reproduce the same open-ended scene.

Live ticks and replay reconstruction use `oxide_kit::bot_execution`. Due bots
may think concurrently against the same immutable state; their work joins in
input seat order before commands are recorded. `State::tick` remains serial.

Between ordinary live ticks, `Game` may submit one background decision. It keeps
the pre-decision controllers and gives the worker a cloned working set plus an
`Arc<State>` shared with rendering. At consumption, world identity, tick and
seat order must match. The worker releases its world reference before publishing
complete controllers and commands; mutation requires unique world ownership. A
late decision is joined at its tick, never skipped or replaced by a timeout.
Bulk advancement and replay reconstruction collect synchronously when no job
exists. Both modes use the same bounded executor admission gate.

Save and controller inspection read the retained pre-decision controllers even
when the job has finished. Restored sessions recompute that decision once;
prepared work is not persisted and saving does not join it. Pause retains the
job; replacing the session or world discards it. Discarding its result does not
cancel the worker, which retains executor admission until computation finishes.
Outstanding work shares diagnostic recorder ownership with the live session.

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

Presentation (camera glides, held pans, effects, and music) reads frame time
clamped to a quarter second. The live and replay clocks read the unclamped time
and cap their own catch-up. A negative or non-finite clock reading counts as no
time. A live frame of two seconds or more means the app stopped presenting, as a
suspended iPad app or a sleeping Mac does; the match opens the pause menu with a
notice instead of resuming. Frame time arrives one frame late, so the rule
requires two consecutive live frames, which keeps startup, launches, and loads
from reading as suspensions. Debug-server sessions are exempt.

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

Building controls share one capability-based selection model for single and
grouped selections. Upgrade ladders, weapons, production rosters, and
construction state determine available actions without a building-kind
whitelist. Buildings of one kind remain a group across tiers; an action applies
only to eligible members. Upgrade advances each eligible building one rung,
skips max-tier or offline members, and allocates available scrap in building-id
order. The card shows the recipient count, total cost, and per-tier
destinations. Its activation rechecks pending commands, as do defense orders, so
an upgrade already staged while paused cannot be bought twice or invalidate
another selected defense's target command. Stop clears completed defenses first;
a separate Scrap sites card can abandon fresh sites in the same selection.
Committed upgrades cannot be cancelled.

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

Production panels reserve a fixed-width rally group with one shared flag beside
adjacent Set/Reset and Clear buttons. Clear stays visible but disabled until a
selected producer has a rally. Rally changes preserve the action band's height
and production-card positions. Narrow layouts widen the group to preserve
minimum touch targets; production wraps in the remaining columns. Card
rectangles and panel bounds share the same separator and inset calculations.

Coordinates are logical throughout the input/layout pipeline; the hardware
adapter applies DPI conversion once. Touches arrive through the same ordered
input stream as mouse events, one event per phase. Drawing publishes a shared
`LayoutModel` whose rectangles also drive hit testing. The HUD's supported
layout floor is 1280×800 at default UI scale. Smaller windows are overflow
stress cases.

Selections contain units of one allegiance or buildings of one owner, ordered by
id. Foreign entities can be inspected while visible, but commands remain gated
to the controlled seat. The simulation performs final ownership, fog, cost,
placement, and target validation. Dead entities and hostiles that leave sight
are removed from selection.

Double-clicks and touch double-taps select units or buildings of the picked
entity's kind and owner whose centers lie in the camera viewport. Building
groups span upgrade tiers and retain normal visibility and concealment checks.
Units retain picking priority over buildings, as with single clicks.

Selection panels and tooltips derive their facts from simulation accessors.
Static capabilities may be shown for foreign selections; current enemy orders,
loads, and building income remain private. Placement and support previews use
authoritative queries rather than duplicating game rules. Unknown concealed
mines cannot alter player-visible picking or placement feedback.

Loaded Harvesters and Excavators expose a Return Cargo card and shortcut (`U` by
default). Worker selections use Return Cargo, a single selected transport uses
Unload, and selected buildings retain Upgrade on the same key. Mixed unit
selections containing workers use Return Cargo. The action replaces current and
queued work, deposits at a reachable owned Foundry, and leaves the worker there.
A right-click on a completed owned Foundry sends loaded workers to that specific
building; a damaged Foundry also receives repair after delivery. Empty welders
retain the existing repair click, and unfinished sites retain construction.
Cargo returns replace work even when Shift is held; they never resume an
interrupted harvest job.

### Touch-only builds

`platform::TOUCH_ONLY` is true on iOS, where only touches and on-screen keyboard
characters arrive: no hardware keys, mouse, hover, or wheel. Code branches on
the constant, and pure helpers take it as a parameter, so both variants compile
and test on every platform.

Every screen offers a pointer path for what Escape does, on every platform. The
top bar ends in a menu button that opens the pause menu, and its clock or PAUSED
status toggles pause; both are `LayoutModel` chrome that `UiView` reports. The
New Match steps, replay playback, and the final map draw corner Back buttons,
and playback adds Play/Pause. These ride `Press::feed`, which also tells the
screen when a button claimed an event. Menu lists scroll by touch drag, and the
read-only viewers pan and pinch through `ViewerTouch`. The save-name field has
Save and Cancel; on touch-only builds the frame loop raises and hides the
on-screen keyboard to follow it.

Touch-only builds hide rows they cannot use: Controls and the left-handed preset
(key rebinding), edge pan (no hovering pointer), Open diagnostics folder (no
file manager), and Quit (the platform closes apps). A match the platform
terminates in the background returns through recovery.

## Persistence and replay

`oxide_kit::checkpoint::SessionCheckpoint` is the internal continuation
boundary: scenario, world, controller memory, pending commands, and live
statistics at a completed tick. Capture borrows the host without draining inputs
or running bots. Restore validates the pieces before installation and executes
no historical ticks. Controller and session format revisions are independent of
`SIM_VERSION`; the initial implementation accepts only matching revisions and
simulation versions.

The session envelope fingerprints the captured scenario and world together.
Restoration rejects changes to either side of that pairing, including a seed
changed in both the session and its companion recorder. Capture trusts the host
to supply the world's original scenario; this consistency fingerprint is not
authentication or proof that historical commands produced the snapshot. Session
revision 2 requires the fingerprint and rejects revision 1 checkpoints.
Simulation serialization and hashes are unchanged.

The headless session serde adapter uses `RecordedCheckpoint`, retaining its
recorder as a companion. Its setup and end tick must agree with the core, but
loading does not verify the entire history against the snapshot. The shell
adapter instead captures the core without a recorder. It additionally retains
tutorial progress, concession statistics, and decorative boundary exploration.
Camera, selection, effects, and interpolation rebuild, the wall clock starts
paused, and recovery/diagnostic workers are not serialized.

Player saves use `.oxsave`: eight-byte `OXIDESAV` magic, a little-endian 32-bit
header length, a JSON metadata header, and one checksummed Zstandard frame
containing a CBOR shell checkpoint. Headers are bounded to 64 KiB; both file
size and decoded payload are bounded to 256 MiB. The loader requires exact
lengths and no trailing frames or checkpoint data, checks revisions, and
validates metadata against the restored session. Controller payloads are CBOR
byte strings. No old player-save import or format migration is provided.

Restoration runs no historical ticks and starts a fresh world-origin recording
at the saved tick. Pending input remains pending until that tick executes. A
save is self-contained. Named saves persist until explicitly deleted; autosaves
and finished-match recordings rotate separately. Finished matches remain JSON
recordings, containing the available history since the current segment began.

One app-owned worker performs save encoding, disk I/O, decompression,
restoration, catalog scans, and recovery preparation. It admits one operation at
a time; a foreground screen can hold one pending intent while a cancelled
catalog or load finishes. Cancellation does not release admission early. Jobs
have unique IDs so a stale result cannot replace a newer session. Restoration
produces CPU data; presentation is installed only on the frame thread. Replaced
session data and recovery-close waits are retired on the worker.

A loading frame is presented before dispatch. Loads can be cancelled until
recovery publication begins; cancellation retains the previous session and never
supersedes a recovery source. Named saves keep the match paused until
completion. Save-before-leaving failures preserve Retry, Cancel, and Leave
Without Saving. A window-close request during a save waits for that operation
and the required exit save. The debug protocol reports `loading`/`saving` and
refuses session mutations while those screens own the boundary.

Recordings can additionally start from a versioned world checkpoint. Its
scenario and world fingerprint are validated, and commands cannot precede its
absolute start tick. Playback, inspection, and statistics execute only the
available suffix. Home and backward seeks stop at the recording origin; the
scrub bar spans that origin through the absolute end tick. Segment statistics
cannot reconstruct earlier events and explicitly describe only available
history. World-only recordings cannot resume live play because they lack
controller memory.

Save publication reserves a collision-free destination and uses the chassis
atomic-write path. Failures are reported to the player. Checkpoint discovery
reads metadata only: eligibility is not full validation. Continue tries eligible
autosaves newest first on the worker, skipping corrupt payloads and installing
the already restored result. Old player saves remain unavailable on disk. Home
recovery discovery uses bounded status metadata and inactive leases; selection
validates the journal and its completed prefix before installation.

`Game::from_replay` reconstructs state from the recorded commands. Bots observe
reconstruction to restore their controller-local memory, but their regenerated
commands are discarded. The recorded log is authoritative, and the resumed game
continues recording onto it.

Ordinary play also keeps an incremental recovery journal through
`kit::recovery`. A bounded queue sends prepared commands and completed-tick
boundaries to a persistence worker. Only a contiguous, validated completed
prefix can be recovered; an unfinished tick is diagnostic evidence. Recovered
matches start paused.

Recovery journals may pair a world-origin recording with a separate session
checkpoint. Restoration validates that both describe the same initial world,
restores controllers and live statistics, then observes only completed suffix
ticks. Recorded commands remain authoritative. Pending checkpoint inputs must
prefix the first journal batch: they survive when no tick completed and are
consumed exactly once when that batch completed. An unfinished prepared batch
remains diagnostic evidence. Exports retain the controller origin separately
from the watchable replay. Replacement journals must retain both origins before
retiring a recovered source. Ordinary new matches still start recovery from
their existing scenario-backed recorder. A loaded player save starts recovery
from its restored session checkpoint and new world-origin recording.

The worker publishes durable progress separately from live progress. Storage
failure or queue exhaustion stops capture with a visible warning while gameplay
continues. Flush cadence is a target, not a guaranteed loss bound. A recovery
journal is clean only after its completion marker is durable. After a successful
player save, the exit path waits at most one second for that marker; a timeout
or recovery error can leave an interrupted journal even after an ordinary exit.
Force quit preserves only bytes already written. Recovery retires the
interrupted source only after its replacement baseline is durable and the
original diagnostic evidence has been preserved. An exclusive source claim
prevents concurrent or stale callers from recovering an already-retired record.

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
owns the playback engine and its own `Presentation`. Rendering borrows the
engine state directly; stepping keeps only the previous positions, headings, and
effect metadata needed for interpolation and casualties. It neither clones the
world into a render vehicle nor constructs live bots or a live recorder. Live
and playback ticks use the same presentation update. Debug state and clock
requests target the engine; camera and overlay requests target presentation. UI,
profiling, and diagnostic context describe the visible session. Authoritative
session mutations are refused.

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
simulation entities. Collision slides are eased: a drawn ground body may trail
its simulation position and lean its hull off the simulation heading within
small fixed bounds, while selection, targeting, and turret aim keep reading
simulation truth. Presentation derives from authoritative events and state;
seeks reset interpolation and rebuild any persistent effects needed at the new
tick. Pause, speed, and reduced-motion behavior follow those presentation
clocks.

Destruction and projectile caches retain the pre-removal identity, pose, and
visibility needed to present an event after its entity has gone. Cosmetic
visuals cannot reveal unseen events. Authoritative crash trajectories and impact
timing remain simulation-owned; rendering observes them without adding damage
rules.

`assets` loads the generated sprite atlas. Its manifest covers every resolved
sprite key; the renderer does not load individual sprite textures. Optional rigs
may fall back to composite sprites, but incomplete rig families are rejected.
Procedural quarry boundaries and pits derive from map geometry with fog-aware
visibility. The shell extends allied unit sight discs and completed-building
footprint sight into a bounded off-map quarry margin. Its presentation-only
exploration cache updates on every tick, including bulk advances, and rebuilds
during explicit command-log reconstruction. Player checkpoints retain that cache
directly. Map tiles retain authoritative simulation fog; the replay viewer
remains fog-free. Animation, heading, and weapon effects use the relevant
simulation state rather than inventing movement or firing delays.

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
limits, and camera-relative attenuation. Missile, artillery, bomb, mine and
Sapper detonations, building destruction, and aircraft ground impacts are
audible through fog regardless of ownership. Their distance gain is full inside
the camera viewport and fades linearly to silence 24 tiles beyond its nearest
edge, with the same range at every zoom. Zoom weighting still reduces heavy
sounds to 72% at the widest view. Same-kind events coalesce to the loudest
emitter; inaudible events consume no voices and do not raise combat music. A
detonated charge uses only its demolition cue; other buildings destroyed in the
same tick retain their destruction cues. Visuals, target knowledge, launch
warnings, and missile motors retain their sight rules. Continuous positional
sounds are owned and stopped individually; pause and screen transitions release
them, and resumed presentation can reconstruct them. Soundtrack state controls
music beds and crossfades. Audio never feeds a simulation decision. Production
sprite and sound bytes remain owned by their generators and approval workflows.

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
| iPad build                    | `ios/`, `shell/src/platform.rs`                                                | iOS clippy in CI, device builds                                            |
