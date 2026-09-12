# Shell architecture

This document describes the presentation and session architecture of
`oxide-shell` and its protocol/kit dependencies. Live-driving commands and
review procedures belong in the oxide-live-qa skill.

## The shell boundary

The shell is a macroquad window over deterministic simulation state. It owns
wall-clock pacing, screen flow, camera, input, selection, interpolation,
effects, audio, persistence UX, and the live debug endpoint. None of those may
change a match except by staging a `PlayerCommand` for a recorded tick.

`Game` is the live-session boundary. It groups:

- the immutable starting `Scenario` and authoritative `State`;
- bot command sources, pending human/debug commands, and the replay recorder;
- the controlled human seat and presentation camera/selection;
- interpolation, facing, aim, animation, effects, sounds, alerts, and toasts;
- live and final statistics plus autosave and tutorial bookkeeping.

`Game::do_tick` is the only live-shell path that advances state. It drains
pending commands, asks bots for their commands, records every command at the
current tick, calls `State::tick`, updates statistics, and then lets
presentation caches observe the resulting report. Fast advancement may suppress
intermediate presentation work, but it still bottoms out in the same recorded
tick path.

Live ticks and replay resume use `oxide_kit::bot_execution` to collect bot
commands. Due seats may think concurrently against the same immutable state; all
work joins in input seat order before recording commands and ticking.
`State::tick` remains serial.

The shell may use floats, hash maps, frame time, and interpolation while
interpreting input and presenting a match. Those values can affect which
semantic command the shell stages, but only the resulting `PlayerCommand`,
recorded for a specific tick, crosses the simulation boundary. Once staged,
camera, selection, interpolation, and other shell state never feed
`State::tick`. Selection is pruned when entities die or hostiles leave sight;
timeline-local aim and effects are cleared after jumps.

## App and screen ownership

`App` owns resources and state that outlive an individual screen: the live
`Game`, configuration, input funnel, cross-screen scenario draft, tutorial,
atlas, sounds, soundtrack, debug channels, and frame profiler. The `Screen` enum
pairs each active mode with its screen-local state, so the variant and its
payload cannot disagree. Cross-screen state such as the draft and live session
remains in `App` rather than being duplicated across variants.

The screen graph includes Home, Settings/Controls, the Codex (the roster read
from `stats.rs`), the New Match wizard, Playing, Playback, the Saves & Replays
shelf, Results, Final Map, and Pause. Settings and the Codex retain the
displaced screen they must return to. Playback records an explicit return
destination. The live `Game` remains available as the backdrop for pause,
results, and final-map inspection.

Individual screen modules own their menu cursor, dialogs, transient interaction
state, and windowless update logic. They accept `RawEvent` values and return
semantic transition results. `app/screen_flow.rs` turns those results into
cross-screen transitions and draws the active screen, while `app.rs` owns frame
orchestration, debug requests, audio, capture, and persistence. This separation
lets the navigation graph and destructive confirmations be tested without a GPU
window.

The New Match draft carries each chair's visible bot difficulty and stance. Each
opponent row exposes those as two direct controls while the map preview remains
visible. When a small window cannot fit the roster at 44 logical pixels per row,
setup pages the seat list and Start button instead of overlapping touch targets.
Only a successful New Match launch asks `App` to advance its shell-only
personality-seed source. Ordinary sessions seed that source from wall-clock and
process identity; automation uses a fixed test seed. This is the sole
pre-scenario entropy boundary, not in-match randomness. Launch materializes one
exact, distinct seed per opponent into the `Scenario` before `Game` is created,
and the source is not consulted again. The seed becomes replay provenance;
hidden personality traits are derived from it rather than serialized separately
or shown in ordinary UI labels.

The frame loop has a fixed shape:

1. Drain debug requests; defer screenshot replies until after rendering.
2. Poll hardware input, append injected input, and route all events.
3. Advance the live or playback clock unless paused or seeking.
4. Render the active world and screen using interpolated presentation state.
5. Capture requested screenshots from the completed frame and reply.

## Input and layout

`oxide_protocol::RawEvent` is the common input vocabulary for mouse, keyboard,
touch, wheel, and text. Macroquad hardware polling emits these events; debug
injection appends the same type; `input::apply_events` is the only mapper from
player-facing gameplay interaction to camera, selection, and commands. New UI
interaction that bypasses this funnel cannot be exercised honestly through
injected-input QA. The protocol's `SendCommand` is intentionally separate: it
stages an already-semantic `PlayerCommand` for deterministic control and does
not pretend to exercise the UI path.

Input state owns cross-frame gestures and modes: drag selection, minimap drag,
middle-button pan, control groups, camera bookmarks, patrol routes, placement
strokes, rally targeting, touch tracking, pinch state, and resolved bindings.
World clicks stage commands rather than touching `State`. The simulation still
performs final ownership, fog, cost, placement, and target validation.

Selections contain either units of one allegiance or buildings of one owner,
stored in id order. Hostile and allied entities may be inspected while visible,
but order generation remains gated to the controlled seat. Multi-select and card
actions must preserve set semantics when they become commands.

HUD drawing publishes one `LayoutModel` for the frame. The same rectangles drive
hit testing for the top bar, panel band, order dock, minimap, roster, cards,
queue, idle-worker badge, and armed-mode ribbon. Drawing and interaction must
not recalculate competing geometry. Logical input coordinates are used
throughout; platform DPI conversion occurs once at the hardware adapter.

Construction uses one 13-card catalog, grouped by economy, production, defense,
and utility when space permits. Visual grouping preserves each building's digit
shortcut. Hover details accompany compact icon, name, and price cards. Opening
it replaces the selection's ordinary action cards, and choosing a building
leaves construction visible. Armed world commands yield clicks and taps to HUD
controls so another card can replace the placement kind directly. The minimap
retains its existing command and camera routing. The HUD shows banked scrap and
current recurring income; individual own income buildings show their rate
without requiring a tooltip. Harvest deliveries and temporary recovery grants
are separate from that rate. Building portraits use the selected tier's hull and
weapon mount.

The optional `OXIDE_REVIEW_FONT` path loads one TTF for both body and display
text at startup. An unreadable or invalid font refuses startup; leaving it unset
uses the embedded Chakra Petch Medium font. This permits native typography
comparisons without replacing production font files or altering text sizes.

## Persistence and replay modes

The recorder is always active. A save is a replay containing the starting
scenario, tick-stamped commands, simulation version, and metadata. There is no
independent mutable snapshot format to drift away from command history.

Autosaves represent resumable live sessions. Explicit saves are player-named
records whose display name lives in metadata rather than the filename. Decided
matches are retained as watchable records. Metadata records kind, save time,
description, and duration; filename prefixes are an older-file fallback.
Autosaves and finished matches rotate separately, while explicit saves persist
until the player deletes them.

Disk publication reserves a collision-free destination and delegates the actual
replay write to the chassis atomic-write path. A failed save is a reported UX
outcome rather than a silent quit. Shelf discovery skips malformed records and
labels version-incompatible records instead of trying to guess.

`Game::from_replay` resumes a live record by rebuilding the scenario and
replaying every recorded command. Bots are allowed to observe the reconstruction
only to restore their controller-local memory; their regenerated commands are
discarded because the log is authoritative. The resumed `Game` continues
recording onto that same command history.

Restart and Rematch rebuild from `Game::scenario`, while replay and save loading
rebuild from the recorded setup. Consequently all of those paths preserve the
exact opponent difficulty, stance, and personality seed. They never consult the
New Match seed source; only another successful wizard launch creates new
opponent identities.

Read-only viewing uses `oxide_kit::playback::Playback`: no bots, no recorder,
and no new commands. Forward playback feeds recorded commands directly to
`State::tick`. Seeking restores the best prior in-memory checkpoint and
re-simulates the suffix; checkpoint cadence stretches to retain at most 64 state
clones. The shell slices long seeks across frames, replaces the render vehicle's
state after each budgeted slice, and drops effects from the abandoned timeline.

While Playback is visible, `App` retains its hidden live `Game` and the
`PlaybackSession` owns two more pieces: the playback engine's authoritative
state and a `Game` used only as its render vehicle. Debug state and clock
requests must route to the visible playback engine. Camera and overlay requests
target its render vehicle, while UI and profiling describe the visible window;
authoritative session mutations are refused. This prevents a viewer-bound
request from silently advancing the hidden live match.

The player-facing shelf loads resumable records rather than playing them
fog-free, preventing ordinary shelf use from scouting a live match. It offers
completed records for viewing. The explicit developer `--watch` launch may open
any compatible replay path and is therefore a fog-free inspection tool. Final
Map is a camera-only view over the already-final live state, not a replay
fast-forward.

## Debug protocol and capabilities

`oxide-protocol` defines JSON-lines request/reply envelopes, input events,
readable state views, fog-honest views, and the framed TCP transport. The live
shell and the driver session use the same bounded framing loop.

The shell server binds loopback and parses connections on socket threads, but
those threads never touch the game. Parsed requests cross a channel to the
macroquad main loop and are answered between frames against settled state.
Screenshot replies alone remain pending until the requested GPU frame has been
rendered and read back.

Three session shapes expose the protocol:

- the mutable live shell with a real window and wall clock;
- the read-only replay viewer inside that window;
- the mutable, permanently driven, windowless driver session.

`DebugSession` and `dispatch_shared` own the common state/clock surface: status,
omniscient state, fog view, state hash, hidden or presented tick advance,
pause/resume, and speed. Each implementation supplies its honest clock
semantics. Requests are capped centrally.

Other requests are capability-specific. Camera, UI, input injection, overlay,
and native frame profiling are window-shaped. Screenshots are implemented by
both servers but have deliberately different meanings: the shell captures the
completed GPU frame, while the headless session renders a whole-map CPU
schematic. Commands and scenario/replay swaps mutate a session. The shell
answers capabilities its current screen supports; replay playback refuses
authoritative mutations; the headless session refuses genuinely window-only
requests in words rather than fabricating results.

`StateView` is an omniscient, float-based QA view and may include an ASCII map.
It is legible, not exact; compare `State::hash` for deterministic identity.
`FogView::capture` is the canonical player-knowledge view shared by live and
headless servers. It redacts hostile intent and economy, exposes live enemies
only under true sight, and carries only the sim's ghosts, remembered salvage,
and anonymous radar contacts.

## Rendering and assets

The shell GPU renderer draws the actual player-facing frame: camera-clipped
world, fog, sprites, action animation, projectiles, effects, minimap, HUD, and
screen chrome. Render interpolation and presentation clocks smooth the fixed 20
Hz simulation without changing its state.

Airworks doors begin opening during the last twelve production ticks. The
authoritative training event holds the bay open while the aircraft emerges, then
closes it over the remainder of a sixteen-tick launch cycle. The newborn
aircraft is always drawn, selected, observed, and targeted at its simulation
position above the roof bay.

All production sprite regions come from generated atlas pages, each at most 4096
pixels per side, loaded by `shell/src/assets.rs`. Exact duplicate frames share
space. Manifest coordinates address vertically stacked pages; drawing resolves
the page without changing a sprite’s canvas or placement. The loader also
accepts older single-page review banks. The manifest must match every key the
shell resolves, and the renderer must not load per-sprite textures. The quarry
boundary and pit terraces are the exception: `render/environment.rs` and
`render/pits.rs` draw them procedurally as one riser, bench, and lip vocabulary,
the boundary rising outside the map rect and pits stepping down from a
fog-honest distance field over explored pit tiles. Both use shared strata and
material shading. The production floor preserves tile centers with feathered
edges; twelve `quarry_dressing_*` sprites scatter stains, grates, and cables
independently of the floor waves. Placement depends only on the scenario seed
and coordinates, preserves authored wrecks, and avoids resource sites. Older
resource banks may omit the complete dressing family to retain their original
terrain treatment. Animation state is driven by simulation events and current
actions, then discarded or rebuilt after a timeline jump. Fog rendering reads
the controlled seat's `Vision` unless an explicit spectator/debug mode is
active.

Unit minification is owned by `unit_lod`. At startup it derives half-, quarter-,
and eighth-resolution images independently from every unit sprite, including
rigs, accent masks, cargo and animation frames. Alpha-weighted RGB and averaged
coverage preserve thin details without importing transparent pixel colors or
neighboring atlas sprites. Each reduced region has its own extruded border. The
original atlas remains unchanged. Physical destination size, including DPI,
chooses the nearest reduced level; short premultiplied-alpha shader blends
smooth level boundaries. Normal rendering is retained at 32 logical pixels per
tile and above, with filtering introduced below that scale. The
`OXIDE_SPRITE_FILTER` override controls the original atlas; reduced levels use
linear sampling.

`strategic_markers` fades in role and seat-identity markers below 14 logical
pixels per tile, replacing unit sprites at 10. Ground bodies use squares and
airborne bodies diamonds. Markers retain player visibility, selection and
damaged health feedback, with separate allied and hostile cues. Selected markers
draw last in crowds. Markers remain centered on the same unit positions used for
picking; they do not cluster or displace units. Camera presentation never
changes simulation positions, commands or visibility.

An atlas may supply separate Array foundations and aerials through the complete
`rig_array_t{0,1}_{base,rotor}` faction and accent families. The world renderer
rotates the aerial about its bearing using the fractional presentation clock;
pause and reduced motion hold it still. Construction, placement, and icons use
the composite sprites. Banks without the rig retain the frame-based sweep, and
partial rigs are rejected at startup.

Buzzard's optional articulated rig separates the continuous rotor cycle and
movement-facing chassis from its gun. The gun interpolates the simulation's
authoritative turret bearing, including before its first shot and during reload;
attack effects do not override that bearing. The composite sprite remains the
fallback when a bank does not contain the separate rig.

Buzzard, Skyhook, and Wisp ease their hulls toward movement facing without
gating translation. Wisp's angular speed scales with movement speed, using
Buzzard's 0.3 radians per tick as the reference. Skyhook turns more slowly at
0.25 radians per tick to give the large transport more weight. Wisp also eases
toward its recent firing angle while hovering; attack frames cannot snap its
hull to the target.

Most units draw on one tile-sized canvas; Excavator and Shrike use 1.3-tile
canvases, Sylph uses 1.2, and Warden uses 1.4. Condor, Breaker, and Avalanche
use centered two-tile canvases with matching selection and health-bar geometry;
Condor also uses a larger air shadow. Tender's tread and welding-arm rows are
selected from real locomotion and active repair state. At demolition contact,
Sapper faces its visible target's nearest physical point before disappearing on
the authoritative attack tick. Breaker and Avalanche select their large tread
and weapon rows from real locomotion and attack state. Heavy ground units and
aircraft with committed or hover-capable cruise steering interpolate their
authoritative heading across the shortest angular interval between ticks. Attack
effects do not override that orientation. Avalanche launch reports show an empty
rail, and its cooldown holds that pose until the final reload interval.
Serialized projectile kind distinguishes shells, missiles, and belly-released
bombs even after the shooter dies. Launch reports retain unit kind and heading
from the firing phase, before egress steering or movement. Turret reports
likewise retain the firing tier so same-tick destruction cannot change the final
volley's presentation.

Avalanche missiles draw as compact finned payloads, 0.375 tiles long, with a
short motor flame and a trailing smoke segment. Their visual origin is ahead of
the vehicle center at the launch rail. A brief unpowered ejection clears the
rail before motor ignition and acceleration along a straight path. That
presentation integrates to the authoritative impact tick; only artillery shells
use a ballistic arc.

Condor bombs start at the nose and draw beneath airborne bodies, so an authored
opening can conceal the payload until it clears the aircraft. A short forward
release follows the launch heading and blends into the fixed impact point;
height only decreases. The shell retains that heading for the payload's
lifetime, including through saved-match reconstruction and shooter death. After
a spectator seek without launch history, an identifiable Condor uses the fixed
impact line; an unidentified dead shooter retains the generic bomb fallback.

Moth uses the same launch-pose record with a distinct slot for each of its six
authoritative bombs, including salvos whose map-clamped impacts share an arrival
tick. Payloads emerge beneath two three-position racks, inherit the launch
heading, and descend to their original impact points and ticks. The authored
rack stays spent during movement and recovery, then refills in pairs during the
final 16 percent of the actual cooldown. It adds no decorative projectile or
release flash. Neither bomber's simulation flight or attack-run rules change.

The production atlas supplies complete `rig_<unit>_hull` and `rig_<unit>_mount`
families for Sentinel, Warden, and Lancer, including faction and allegiance
masks. Each hull has idle and two movement rows; Sentinel and Warden mounts have
idle and four action rows, and Lancer has idle and six. The shell draws each
mount independently of its hull, whose cosmetic heading eases across movement
ticks. Weapon cycles persist while the tracks move; Lancer's charge builds
during the final 18 percent of cooldown. Weapon recoil and firing direction
follow actual attack reports, with no new firing delay. Missing rigs use the
ordinary combined sprite; partial rigs are a startup error. The production bank
pairs these rows with authored muzzle positions, compact metal rounds for
Sentinel, Warden, and Breaker, and a thin fading Lancer trace. Rendering
interpolates authoritative ground body and independent gun bearings. Locomotion
frames remain active during a stationary chassis pivot.

Bombard retains its launch heading in the observational projectile-pose cache.
Its metal shell leaves the mortar muzzle on a straight, constant-speed course
and retains its launch bearing. A parabolic height cue slightly enlarges the
shell and separates its ground shadow around mid-flight, then closes them at
impact; height never steers the shell across the floor. Bastion uses the same
metal-shell treatment on its fixed launch-to-impact bearing. Its ammunition rack
is part of the foundation below the traversing carriage. Flak Turret effects
emit four visible rounds, or six after upgrading, in two offset banks matched to
the weapon. This remains one logical damage report. Flakhound reports four
visible rounds: both barrels on one yoke fire together, followed one tick later
by the other pair. Stinger retains its existing paired report. These effects
preserve projectile arrival ticks and existing damage resolution.

An optional neutral `scout_radar` atlas layer rotates over Gnat and Kestrel's
scanner mounts. Their independent 96-tick sensing cycle runs at rest and during
movement, holds with the simulation clock when paused, and freezes under reduced
motion. Flight and propulsion do not drive or restart it. Banks without the
layer retain their authored scout sprites.

An optional five-frame `bombard_spades_0` through `bombard_spades_4` atlas layer
follows authoritative spade deployment independently of reload and recoil
frames. The mortar body follows its interpolated simulation heading, including
before the first shot and after replay restoration; firing effects cannot snap
it toward a target. Partial spade banks are a startup error.

Sprite atlases use nearest-neighbor sampling by default.
`OXIDE_SPRITE_FILTER=linear` enables linear sampling for native comparisons. The
setting affects presentation only and rejects unknown values at startup.

Selection feedback may describe public simulation rules, but dynamic economy
state remains owner-only. A selected own Extractor names its authoritative
remote or supported rate, and selected own Extractors and Foundries draw the
exact square support footprint plus current endpoint links. Hostile selections
may show the static support radius without revealing whether an unseen enemy
Foundry currently supplies the bonus. The panel and renderer both ask `State`
for support rather than duplicating its footprint calculation.

`oxide-kit` also contains a tiny-skia CPU renderer. It is a deliberately plain,
whole-map schematic used for deterministic goldens and headless screenshots. It
does not share the GPU renderer's atlas, camera, fog composition, HUD,
animation, or visual polish. Screenshot replies label their producer as `"gpu"`
or `"cpu"` so tests and reviewers cannot compare the wrong surface.

## Audio as presentation

Simulation events enqueue sound kinds and optional world positions. The mixer
rate-limits repeated reports, applies user master/effects/UI buses, and uses a
pure per-frame camera mix to weight positional sounds. A close camera exposes
more machine detail; a wide camera admits fewer minor voices while protecting
heavy reports. The under-attack alert bypasses distance, zoom, and positional
voice limits.

The soundtrack owns continuous beds and pure crossfades for menu, match
pressure, pause, and results. Automation omits those long-lived sources during
deterministic UI capture. No audio behavior may create a simulation branch.

## CPU and GPU QA boundaries

Use the CPU surface to prove deterministic world composition and broad renderer
coverage. Driver goldens compare its PNG bytes exactly, and the showcase state
names every schematic branch it is expected to exercise. A CPU screenshot is
appropriate for map state and replay/session parity, not for judging sprites,
layout, animation, fog treatment, or native performance.

Use the real shell for presentation claims. The smoke test crosses input,
camera, commands, recording, and GPU screenshot capture. The menu UX suite
drives windowless screen objects and, in its ignored battery, the real window.
Native animation capture advances presentation by exact tick intervals through
the public protocol. Native shell profiling reports CPU frame work and
frame-start intervals around the real GPU presentation path; it does not expose
direct GPU timestamps.

Session parity tests compare the shared protocol structurally between headless
and live implementations. They intentionally permit honest capability
differences, especially CPU versus GPU screenshots and the absence of a window
clock in a headless session.

## Source and test map

| Contract                           | Primary source                                                             | Behavioral evidence                                                                                                                |
| ---------------------------------- | -------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| App ownership and screen graph     | `shell/src/app.rs`, `shell/src/app/screen_flow.rs`, `shell/src/screens/`   | `driver/tests/menu_ux.rs`, screen-module unit tests                                                                                |
| Live session and recorder boundary | `shell/src/game.rs`                                                        | `driver/src/smoke.rs`, `shell/src/game.rs` tests                                                                                   |
| Input and shared layout geometry   | `shell/src/input.rs`, `shell/src/layout.rs`, `shell/src/panel.rs`          | `shell/src/input/tests.rs`, `shell/src/layout.rs` tests                                                                            |
| Saves, shelf, and resume           | `shell/src/autosave.rs`, `shell/src/saves.rs`                              | module tests, `driver/tests/menu_ux.rs`                                                                                            |
| Read-only playback and seeking     | `kit/src/playback.rs`, `shell/src/screens/playback.rs`                     | unit tests in both playback modules                                                                                                |
| Protocol capability split          | `protocol/src/session.rs`, `shell/src/debug_server.rs`, `shell/src/app.rs` | `driver/tests/session_parity.rs`                                                                                                   |
| Readable and fog-honest views      | `protocol/src/view.rs`                                                     | protocol tests, `driver/tests/session_parity.rs`                                                                                   |
| GPU assets and rendering           | `shell/src/assets.rs`, `shell/src/render.rs`                               | asset-manifest tests in `shell/src/assets.rs`, `shell/tests/presentation_animation.rs`, `driver/tests/native_animation_capture.rs` |
| CPU schematic rendering            | `kit/src/render.rs`                                                        | `driver/tests/golden.rs`                                                                                                           |
| Audio mix and soundtrack           | `shell/src/audio_mix.rs`, `shell/src/soundtrack.rs`                        | module unit tests                                                                                                                  |

## Continuous ground tracks

Sentinel, Warden, Lancer, Harvester, Avalanche and Breaker retain their authored
hulls while exposed tread shoes scroll continuously. Each belt integrates signed
motor travel and differential hull rotation, so pivots counter-rotate the belts
and collision sliding does not count as driving. Motion reports reach both live
and replay presentation; seeks reset odometry and interpolation. Reduced motion
holds the shoes still. This presentation state never enters the simulation.
