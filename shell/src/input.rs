//! The input funnel.
//!
//! One path, no exceptions: macroquad's polled state becomes [`RawEvent`]s,
//! injected events from the debug socket are appended to the same list, and
//! [`apply_events`] is the only code that turns events into camera motion,
//! selection, or sim commands. If input behavior ever bypasses this module,
//! injected tests stop meaning anything — don't.

use crate::action::{Action, ActionEvent, ActionResolver, BindingMap};
use crate::game::{Game, PingKind};
use chassis::grid::TilePos;
use macroquad::prelude::{self as mq, Vec2, vec2};
use oxide_protocol::{Key, MouseButton, RawEvent};
use oxide_sim::{Command, UnitId};

/// Logical pixels of mouse travel under which a press+release counts as a
/// click (scaled by dpi at use).
const CLICK_SLOP: f32 = 6.0;

fn click_slop(ui: f32) -> f32 {
    CLICK_SLOP * ui
}

/// Shared with the drag-rectangle renderer, so what draws as a drag is
/// exactly what selects as one.
pub fn drag_threshold(ui: f32) -> f32 {
    click_slop(ui)
}

/// World-unit pick radius around a unit's center.
const PICK_RADIUS: f32 = 0.6;
/// Camera pan speed in screen pixels per second (converted by zoom).
const PAN_PX_PER_SEC: f32 = 900.0;

/// One live finger on the screen.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TouchPoint {
    /// Where it landed.
    pub origin: Vec2,
    /// Where it is now.
    pub at: Vec2,
    /// Wall clock at touch-down (the injected `now`).
    pub down_at: f64,
    /// Whether it ever left the slop circle — a moved finger is a
    /// drag, never a tap or a long-press.
    pub moved: bool,
    /// Whether it LANDED on chrome (minimap or HUD). Chrome-born
    /// fingers never drive world gestures: a swipe starting on the
    /// command panel must not pan the camera behind it, and a
    /// two-finger box with a chrome-born corner must not select.
    pub chrome: bool,
    /// Whether its long-press already fired (fire once per touch).
    pub fired: bool,
}

/// Cross-frame input state (cursor, held keys, drag origin).
pub struct InputState {
    /// Last known cursor position, window pixels.
    pub mouse: Vec2,
    /// Where the current left-drag started, if any.
    pub drag_origin: Option<Vec2>,
    /// Whether a left-drag is steering the camera via the minimap —
    /// clicks there never start a selection box, they drive the view.
    pub(crate) minimap_drag: bool,
    /// Middle-drag pan anchor: the world follows the hand.
    pub(crate) mmb_anchor: Option<Vec2>,
    /// Control groups 1..=5 (assigned with Ctrl+N, recalled with N).
    groups: [Vec<UnitId>; crate::action::CONTROL_GROUPS],
    /// Previous click, for double-click detection.
    last_click: Option<(f64, Vec2)>,
    /// Previous group recall, for double-tap camera centering.
    last_recall: Option<(usize, f64)>,
    /// Camera bookmarks (Ctrl+F5..F8 set, F5..F8 recall). Session
    /// state: a new match's coordinates mean different ground.
    pub(crate) bookmarks: [Option<Vec2>; 4],
    /// Waypoints collected while arming a patrol (`R`), if any.
    pub(crate) patrol_route: Option<Vec<TilePos>>,
    /// Building kind armed for placement, if any.
    pub(crate) placing: Option<oxide_sim::BuildingKind>,
    /// The live drag-to-place stroke, while the button stays down.
    /// `None` when no stroke is live.
    pub(crate) placing_stroke: Option<PlacingStroke>,
    /// Armed salvage: the next left-click on an own built building
    /// sends the selected harvesters to strip it.
    pub(crate) salvaging: bool,
    /// Armed run: the next ground click sends the selection walking
    /// obliviously — no engaging, no auto-acquire en route.
    pub(crate) running: bool,
    /// Whether the build palette is open (`B`; digits pick a structure).
    pub(crate) build_menu: bool,
    /// This frame's chrome scale (dpi x user), injected by the frame
    /// loop so hit math never queries the window.
    pub(crate) ui: f32,
    /// This frame's wall clock, injected likewise (double-click and
    /// double-tap timing).
    pub(crate) now: f64,
    /// Camera feel from settings, injected per frame.
    pub(crate) camera_prefs: crate::config::CameraPrefs,
    /// Touch timing windows, injected from config each frame.
    pub(crate) touch_prefs: crate::config::TouchPrefs,
    /// Live fingers, in touch-down order (deterministic; two at most
    /// matter). Each carries its origin, latest point, down time, and
    /// whether it moved past the slop or already fired a long-press.
    pub(crate) touches: Vec<(u64, TouchPoint)>,
    /// Wall-clock stamp of the last completed tap, for double-taps.
    pub(crate) last_tap: Option<(f64, macroquad::prelude::Vec2)>,
    /// A two-finger pair that spread or squeezed reads as a pinch for
    /// its whole lifetime — lifting one finger of a pinch must not
    /// commit a box-select.
    pub(crate) pinching: bool,
    /// The pair's spread when it formed: pinch detection compares the
    /// CUMULATIVE change against this, so a slow pinch (under a pixel
    /// per event) still reads as one instead of committing a box.
    pub(crate) pair_dist: Option<f32>,
    /// The active binding profile (Classic until settings can edit it).
    pub(crate) bindings: BindingMap,
    /// Chord state: modifier truth and held actions.
    pub(crate) resolver: ActionResolver,
}

/// A live drag-to-place stroke: the anchors stamped so far (the
/// overlap guard) and the predicted `queue.len()` of the builder the
/// sim will pick. `commands::assign` appends while
/// `queue.len() < ORDER_QUEUE_CAP` and the ACTIVE order sits OUTSIDE
/// the queue, so a replacing stroke owns the cap plus that slot while
/// a Shift stroke inherits whatever the builder already carries.
/// Predicted forward because staged stamps have not reached the unit
/// yet — re-reading live state mid-stroke would count them twice.
pub(crate) struct PlacingStroke {
    anchors: Vec<TilePos>,
    queued: usize,
}

/// Scrap the staged-but-undrained Build commands will actually charge
/// when the tick takes them — summed per command, never count-times-
/// current-kind: a paused shell accumulates strokes of DIFFERENT
/// kinds across frames, and a Build aimed at an own unfinished site
/// is a free resume, not a purchase.
fn pending_build_bill(game: &Game) -> u32 {
    game.pending
        .iter()
        .filter_map(|pc| match &pc.command {
            Command::Build { kind, anchor, .. } => {
                let resume = game
                    .state
                    .building_at(*anchor)
                    .is_some_and(|b| b.player == game.human && !b.built);
                if resume {
                    None
                } else {
                    kind.stats().construction.map(|c| c.cost)
                }
            }
            _ => None,
        })
        .sum()
}

/// Whether a footprint of `kind` at `anchor` intersects any
/// staged-but-undrained Build's footprint. Live state cannot see
/// those sites while the shell is paused, so without this a stamp
/// overlapping an earlier stroke's site would acknowledge with a
/// ping and then die rejected when that site claims the ground.
/// Footprints are compared kind-by-kind — paused strokes can stack
/// different building sizes.
fn overlaps_pending_site(game: &Game, kind: oxide_sim::BuildingKind, anchor: TilePos) -> bool {
    let (w, h) = kind.stats().size;
    game.pending.iter().any(|pc| match &pc.command {
        Command::Build {
            kind: staged,
            anchor: a,
            ..
        } => {
            let (sw, sh) = staged.stats().size;
            anchor.x < a.x + sw && a.x < anchor.x + w && anchor.y < a.y + sh && a.y < anchor.y + h
        }
        _ => false,
    })
}

/// The founder's queue depth once this stroke's FIRST stamp has
/// landed — a Build drafts every own harvester in the selection, and
/// the lowest-id one (`accepted_units` sorts) is the founder whose
/// full queue rejects the whole command, so it is the unit whose
/// depth gates the stroke. (Another crew member arriving with a
/// deeper program can individually hit QueueFull while this proxy
/// says there was room — that drops the one hand, never the
/// command.) Without Shift the stamp
/// wipes the program; with Shift it appends, except onto an idle
/// builder, where it takes the free active slot. Live state alone is
/// not enough: a paused shell stages whole strokes — and any other
/// queued orders — without the sim ever draining them, so every
/// pending command that will mutate this builder's program is
/// replayed on top: appends deepen, replacements reset to the active
/// slot, Stop clears, Patrol installs its whole circuit.
fn stroke_queued(game: &Game, shift: bool) -> usize {
    if !shift {
        return 0;
    }
    let chosen_builder = |units: &[oxide_sim::UnitId]| {
        units
            .iter()
            .copied()
            .filter(|id| {
                game.state
                    .unit(*id)
                    .is_some_and(|u| u.player == game.human && u.kind.stats().harvest.is_some())
            })
            .min()
    };
    let Some(builder) = chosen_builder(&game.selection.units).and_then(|id| game.state.unit(id))
    else {
        return 0;
    };
    let mut depth =
        builder.queue.len() + usize::from(!matches!(builder.order, oxide_sim::Order::Idle));
    for pc in &game.pending {
        // Build drafts every listed harvester — the depth tracked
        // here is the founder's, the hand that gates the command;
        // every other unit order lands on each listed unit (an
        // Attack degrades to a walk for a pacifist harvester, but it
        // still occupies the program). Appends saturate at the cap
        // the way assign drops them.
        let mine = |units: &[oxide_sim::UnitId]| units.contains(&builder.id);
        match &pc.command {
            Command::Build { units, queue, .. } if chosen_builder(units) == Some(builder.id) => {
                depth = if *queue {
                    (depth + 1).min(oxide_sim::stats::ORDER_QUEUE_CAP + 1)
                } else {
                    1
                };
            }
            Command::Move { units, queue, .. }
            | Command::AttackMove { units, queue, .. }
            | Command::Attack { units, queue, .. }
            | Command::Harvest { units, queue, .. }
            | Command::Repair { units, queue, .. }
            | Command::Salvage { units, queue, .. }
                if mine(units) =>
            {
                depth = if *queue {
                    (depth + 1).min(oxide_sim::stats::ORDER_QUEUE_CAP + 1)
                } else {
                    1
                };
            }
            Command::Stop { units } if mine(units) => depth = 0,
            Command::Patrol { units, waypoints } if mine(units) => depth = waypoints.len(),
            _ => {}
        }
    }
    depth
}

/// Everything a harvester can put in the ground, in palette order — the
/// digit keys index straight into this.
pub(crate) const BUILD_PALETTE: [oxide_sim::BuildingKind; 6] = [
    oxide_sim::BuildingKind::Turret,
    oxide_sim::BuildingKind::FlakTurret,
    oxide_sim::BuildingKind::Bastion,
    oxide_sim::BuildingKind::Array,
    oxide_sim::BuildingKind::Reclaimer,
    oxide_sim::BuildingKind::Fabricator,
];

impl InputState {
    /// Fresh input state.
    pub fn new() -> Self {
        Self {
            mouse: vec2(0.0, 0.0),
            drag_origin: None,
            minimap_drag: false,
            mmb_anchor: None,
            groups: Default::default(),
            last_click: None,
            last_recall: None,
            patrol_route: None,
            placing: None,
            placing_stroke: None,
            salvaging: false,
            running: false,
            build_menu: false,
            ui: 1.0,
            now: 0.0,
            camera_prefs: crate::config::CameraPrefs::default(),
            touch_prefs: crate::config::TouchPrefs::default(),
            touches: Vec::new(),
            last_tap: None,
            pinching: false,
            pair_dist: None,
            bookmarks: [None; 4],
            bindings: crate::config::Config::load().bindings,
            resolver: ActionResolver::default(),
        }
    }

    /// Feeds a key edge through the binding map.
    fn key_edge(&mut self, key: Key, down: bool) -> Option<ActionEvent> {
        self.resolver.key_edge(&self.bindings, key, down)
    }

    /// Drops everything that assumes continuity — held keys and any open
    /// drag — keeping only the cursor position. Called on every mode
    /// transition: a menu eats the matching release events, and stale
    /// held-state otherwise pans the camera forever (or fires a phantom
    /// box-select) after resuming.
    /// One armed left-click verb at a time: arming placement, salvage,
    /// or run stands the others down. `armed_click` resolves modes in
    /// a fixed priority order, so two live at once would make the next
    /// click do something other than what the toast promised — press M
    /// while placing and the click would still stamp a building.
    pub(crate) fn disarm_click_verbs(&mut self) {
        self.placing = None;
        self.placing_stroke = None;
        self.salvaging = false;
        self.running = false;
    }

    pub fn reset_transient(&mut self) {
        self.resolver.clear();
        self.drag_origin = None;
        self.minimap_drag = false;
        self.mmb_anchor = None;
        self.patrol_route = None;
        self.placing = None;
        self.placing_stroke = None;
        self.salvaging = false;
        self.running = false;
        self.build_menu = false;
        self.touches.clear();
        self.last_tap = None;
        self.pinching = false;
        self.pair_dist = None;
    }

    /// Everything `reset_transient` drops, plus state that assumes the
    /// *match* continues: control groups, double-click memory, recall
    /// timing. Called whenever the `Game` is replaced (restart, new map,
    /// replay load) — unit ids restart from zero there, and a stale group
    /// would resolve to unrelated units in the new world.
    pub fn reset_session(&mut self) {
        self.reset_transient();
        self.groups = Default::default();
        self.bookmarks = [None; 4];
        self.last_click = None;
        self.last_recall = None;
    }
}

const KEY_MAP: [(Key, mq::KeyCode); 43] = [
    (Key::Up, mq::KeyCode::Up),
    (Key::Down, mq::KeyCode::Down),
    (Key::Left, mq::KeyCode::Left),
    (Key::Right, mq::KeyCode::Right),
    (Key::H, mq::KeyCode::H),
    (Key::S, mq::KeyCode::S),
    (Key::P, mq::KeyCode::P),
    (Key::R, mq::KeyCode::R),
    (Key::B, mq::KeyCode::B),
    (Key::N, mq::KeyCode::N),
    (Key::X, mq::KeyCode::X),
    (Key::Escape, mq::KeyCode::Escape),
    (Key::Space, mq::KeyCode::Space),
    (Key::F1, mq::KeyCode::F1),
    (Key::Enter, mq::KeyCode::Enter),
    (Key::PageUp, mq::KeyCode::PageUp),
    (Key::PageDown, mq::KeyCode::PageDown),
    (Key::Home, mq::KeyCode::Home),
    (Key::End, mq::KeyCode::End),
    (Key::F5, mq::KeyCode::F5),
    (Key::F6, mq::KeyCode::F6),
    (Key::F7, mq::KeyCode::F7),
    (Key::F8, mq::KeyCode::F8),
    (Key::A, mq::KeyCode::A),
    (Key::C, mq::KeyCode::C),
    (Key::D, mq::KeyCode::D),
    (Key::E, mq::KeyCode::E),
    (Key::F, mq::KeyCode::F),
    (Key::G, mq::KeyCode::G),
    (Key::I, mq::KeyCode::I),
    (Key::J, mq::KeyCode::J),
    (Key::K, mq::KeyCode::K),
    (Key::L, mq::KeyCode::L),
    (Key::M, mq::KeyCode::M),
    (Key::O, mq::KeyCode::O),
    (Key::Q, mq::KeyCode::Q),
    (Key::T, mq::KeyCode::T),
    (Key::U, mq::KeyCode::U),
    (Key::V, mq::KeyCode::V),
    (Key::W, mq::KeyCode::W),
    (Key::Y, mq::KeyCode::Y),
    (Key::Z, mq::KeyCode::Z),
    (Key::Backspace, mq::KeyCode::Backspace),
];

/// Converts this frame's hardware input into events. Purely a poll→event
/// adapter; interpretation happens in [`apply_events`].
/// Translates one hardware touch phase into the raw event the funnel
/// speaks — the same vocabulary the debug harness injects, so a real
/// fingertip and an injected one walk identical code. `Stationary`
/// maps to nothing: a resting finger emits no event (the long-press
/// timer rides the frame loop, not the event stream).
fn touch_event(phase: mq::TouchPhase, id: u64, x: f32, y: f32) -> Option<RawEvent> {
    match phase {
        mq::TouchPhase::Started => Some(RawEvent::TouchDown { id, x, y }),
        mq::TouchPhase::Moved => Some(RawEvent::TouchMove { id, x, y }),
        // A cancelled touch (palm rejection, app switch) lifts like any
        // other: the gesture state must not wait for a finger the OS
        // already took away.
        mq::TouchPhase::Ended | mq::TouchPhase::Cancelled => Some(RawEvent::TouchUp { id, x, y }),
        mq::TouchPhase::Stationary => None,
    }
}

/// The platform's ordered pointer stream, translated event by event
/// into [`RawEvent`]s that keep their OWN coordinates.
///
/// macroquad's polled surface cannot express a press: it keeps the
/// frame's LAST cursor position and a "was pressed this frame" flag, so
/// a button that lands while the pointer is moving was stamped wherever
/// the pointer ENDED the frame. The drag box then anchored ahead of the
/// click (everything between the press and the frame boundary silently
/// dropped out of the selection), and could never draw on the press
/// frame at all — `MouseDown` set `mouse` and `drag_origin` to the same
/// point by construction, and the frame's motion had already been
/// collapsed into one MouseMove ahead of it.
///
/// Raw events arrive in backing-store pixels; the dpi factor is
/// injected (never queried) so the whole adapter runs headless.
pub(crate) struct PointerStream {
    dpi: f32,
    /// Translated events, in arrival order.
    pub(crate) events: Vec<RawEvent>,
}

impl PointerStream {
    pub(crate) fn new(dpi: f32) -> Self {
        Self {
            dpi: if dpi > 0.0 { dpi } else { 1.0 },
            events: Vec::new(),
        }
    }

    /// Backing-store pixels to the logical space every other coordinate
    /// in the shell speaks (what `screen_width()` reports).
    fn logical(&self, x: f32, y: f32) -> (f32, f32) {
        (x / self.dpi, y / self.dpi)
    }
}

fn mouse_button(button: mq::MouseButton) -> Option<MouseButton> {
    match button {
        mq::MouseButton::Left => Some(MouseButton::Left),
        mq::MouseButton::Right => Some(MouseButton::Right),
        mq::MouseButton::Middle => Some(MouseButton::Middle),
        mq::MouseButton::Unknown => None,
    }
}

impl macroquad::miniquad::EventHandler for PointerStream {
    // The collector never runs a frame; it only replays input.
    fn update(&mut self) {}
    fn draw(&mut self) {}

    fn mouse_motion_event(&mut self, x: f32, y: f32) {
        let (x, y) = self.logical(x, y);
        self.events.push(RawEvent::MouseMove { x, y });
    }

    fn mouse_button_down_event(&mut self, button: mq::MouseButton, x: f32, y: f32) {
        let (x, y) = self.logical(x, y);
        if let Some(button) = mouse_button(button) {
            self.events.push(RawEvent::MouseDown { button, x, y });
        }
    }

    fn mouse_button_up_event(&mut self, button: mq::MouseButton, x: f32, y: f32) {
        let (x, y) = self.logical(x, y);
        if let Some(button) = mouse_button(button) {
            self.events.push(RawEvent::MouseUp { button, x, y });
        }
    }

    /// A fingertip must arrive ONCE, as a touch: the trait's DEFAULT
    /// `touch_event` emulates mouse clicks, which would give every
    /// finger a second life as a press. Touches come from `touches()`
    /// below.
    fn touch_event(&mut self, _phase: macroquad::miniquad::TouchPhase, _id: u64, _x: f32, _y: f32) {
    }

    /// Typed characters, layout- and shift-resolved by the OS — a
    /// Key-to-character table would get every non-US layout wrong.
    /// Printable ASCII only, filtered AT INGEST: the menu font is
    /// Latin-1 and UI strings stay ASCII, so nothing downstream ever
    /// needs its own filter.
    fn char_event(
        &mut self,
        character: char,
        _keymods: macroquad::miniquad::KeyMods,
        _repeat: bool,
    ) {
        if ('\u{20}'..='\u{7e}').contains(&character) {
            self.events.push(RawEvent::Text { ch: character });
        }
    }
}

// A fingertip must arrive ONCE, as a touch — macroquad otherwise
// mirrors every touch into synthetic mouse events and the same
// finger would both pan the camera and drag a box.
static TOUCH_SETUP: std::sync::Once = std::sync::Once::new();
// The subscriber is registered on demand so an automation shell —
// which never polls hardware — never accumulates a queue it won't
// drain. Hardware shells must arm BEFORE the prologue instead:
// macroquad's register_input_subscriber starts an empty queue, so
// every event dispatched earlier is fanned out to no one and gone —
// with lazy-only arming, anything clicked or typed while assets and
// the autosave scan ran inside frame 1 was silently discarded.
static POINTER_SUB: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

/// Subscribes to the hardware input stream now, so events arriving
/// before the first [`poll_events`] queue instead of vanishing. Call
/// at the top of the frame future, before any prologue work; never
/// call from an automation shell. `poll_events` still self-arms as a
/// fallback and keeps its first-poll cursor seed either way.
pub fn arm_hardware() {
    TOUCH_SETUP.call_once(|| mq::simulate_mouse_with_touch(false));
    POINTER_SUB.get_or_init(mq::utils::register_input_subscriber);
}

pub fn poll_events() -> Vec<RawEvent> {
    TOUCH_SETUP.call_once(|| mq::simulate_mouse_with_touch(false));
    let mut events = Vec::new();
    for touch in mq::touches() {
        if let Some(event) = touch_event(touch.phase, touch.id, touch.position.x, touch.position.y)
        {
            events.push(event);
        }
    }
    // Pointer events in true arrival order, each with its own position.
    let sub = *POINTER_SUB.get_or_init(mq::utils::register_input_subscriber);
    static FIRST_POLL: std::sync::Once = std::sync::Once::new();
    FIRST_POLL.call_once(|| {
        // However early the subscriber was armed, a stationary cursor
        // never queues a baseline: seed the stream with the position
        // the pointer already holds, or edge pan and wheel zoom anchor
        // at (0, 0) until the first real motion. mouse_position() is
        // already logical (macroquad divides its dpi out).
        let (x, y) = mq::mouse_position();
        events.push(RawEvent::MouseMove { x, y });
    });
    let mut stream = PointerStream::new(macroquad::miniquad::window::dpi_scale());
    mq::utils::repeat_all_miniquad_input(&mut stream, sub);
    events.append(&mut stream.events);
    // macroquad's mouse_leave_event marks held buttons released in the
    // POLLED state without queuing any subscriber event — a button let
    // go outside the window would never reach the stream, leaving
    // drags, pans, and placement strokes latched (a stroke would even
    // resume stamping when the pointer wandered back). Synthesize the
    // missing MouseUp at the last known logical position; releases the
    // stream DID carry are left alone, and a rare duplicate MouseUp is
    // harmless — every release handler tolerates an idle repeat.
    for (mq_btn, btn) in [
        (mq::MouseButton::Left, MouseButton::Left),
        (mq::MouseButton::Middle, MouseButton::Middle),
        (mq::MouseButton::Right, MouseButton::Right),
    ] {
        if mq::is_mouse_button_released(mq_btn)
            && !events
                .iter()
                .any(|e| matches!(e, RawEvent::MouseUp { button, .. } if *button == btn))
        {
            let (x, y) = mq::mouse_position();
            events.push(RawEvent::MouseUp { button: btn, x, y });
        }
    }
    let wheel = mq::mouse_wheel().1;
    if wheel != 0.0 {
        events.push(RawEvent::Wheel {
            delta: normalize_wheel(wheel),
        });
    }
    // Modifier edges land BEFORE ordinary key edges: a chord pressed
    // whole within one frame (Ctrl and F5 together) must resolve as
    // Ctrl+F5, not as F5 followed by a late Ctrl.
    // Modifiers map two physical keys onto one logical one. (Releasing one
    // of a simultaneously-held pair releases the logical key — an edge case
    // nobody plays with.)
    for (key, a, b) in [
        (Key::Shift, mq::KeyCode::LeftShift, mq::KeyCode::RightShift),
        (
            Key::Ctrl,
            mq::KeyCode::LeftControl,
            mq::KeyCode::RightControl,
        ),
    ] {
        if mq::is_key_pressed(a) || mq::is_key_pressed(b) {
            events.push(RawEvent::KeyDown { key });
        }
        if mq::is_key_released(a) || mq::is_key_released(b) {
            events.push(RawEvent::KeyUp { key });
        }
    }
    for (key, code) in KEY_MAP {
        if mq::is_key_pressed(code) {
            events.push(RawEvent::KeyDown { key });
        }
        if mq::is_key_released(code) {
            events.push(RawEvent::KeyUp { key });
        }
    }
    for (key, code) in [
        (Key::Num1, mq::KeyCode::Key1),
        (Key::Num2, mq::KeyCode::Key2),
        (Key::Num3, mq::KeyCode::Key3),
        (Key::Num4, mq::KeyCode::Key4),
        (Key::Num5, mq::KeyCode::Key5),
        (Key::Num6, mq::KeyCode::Key6),
        (Key::Num7, mq::KeyCode::Key7),
        (Key::Num8, mq::KeyCode::Key8),
        (Key::Num9, mq::KeyCode::Key9),
    ] {
        if mq::is_key_pressed(code) {
            events.push(RawEvent::KeyDown { key });
        }
        if mq::is_key_released(code) {
            events.push(RawEvent::KeyUp { key });
        }
    }
    events
}

mod dispatch;
mod orders;
mod select;
#[cfg(test)]
mod tests;

use dispatch::dispatch_action;
use orders::context_order;
pub use select::idle_harvesters;
use select::{
    box_select, click_on_hud, click_select, cycle_idle_worker, select_all_of_kind_on_screen,
};

/// The cursor shape the current intent deserves: crosshair while
/// placing a building or plotting a patrol, a pointer over clickable
/// chrome, the arrow otherwise. Pure — the loop applies it.
pub fn desired_cursor(game: &Game, input: &InputState) -> macroquad::miniquad::CursorIcon {
    use macroquad::miniquad::CursorIcon;
    if input.placing.is_some() || input.patrol_route.is_some() || input.salvaging || input.running {
        return CursorIcon::Crosshair;
    }
    let layout = game.layout.get();
    let p = input.mouse;
    if layout.minimap.contains(p)
        || layout.chrome_owns(p)
        || (layout.idle_badge.w > 0.0 && layout.idle_badge.contains(p))
    {
        return CursorIcon::Pointer;
    }
    CursorIcon::Default
}

/// Applies a frame's events — hardware and injected alike — to the game.
pub fn apply_events(game: &mut Game, input: &mut InputState, events: &[RawEvent]) {
    for event in events {
        match *event {
            RawEvent::MouseMove { x, y } => {
                input.mouse = vec2(x, y);
                // A held minimap press keeps steering: clamp the cursor
                // into the minimap so sliding off its edge doesn't stall
                // the pan mid-gesture.
                if input.minimap_drag {
                    let rect = crate::render::minimap_rect(game);
                    let clamped = vec2(
                        x.clamp(rect.x, rect.x + rect.w - 1.0),
                        y.clamp(rect.y, rect.y + rect.h - 1.0),
                    );
                    if let Some(world) = crate::render::minimap_world_at(game, clamped) {
                        game.camera.center = world;
                        game.camera.pan(Vec2::ZERO);
                    }
                }
                // Middle-drag: the world follows the hand, so the pan
                // moves against the cursor delta, scaled out of screen
                // space by the zoom.
                if let Some(anchor) = input.mmb_anchor {
                    let delta = vec2(x, y) - anchor;
                    game.camera.pan(-delta / game.camera.zoom);
                    input.mmb_anchor = Some(vec2(x, y));
                }
                // Drag-to-place: while the button stays down in
                // placement mode, every new valid, non-overlapping
                // anchor stamps another build, queued behind the
                // builder's program — a wall in one stroke. Invalid
                // cells skip silently (the ghost tells that story),
                // and the stroke stops stamping at the order-queue
                // cap instead of firing doomed commands.
                if let (Some(kind), Some(stroke)) = (input.placing, input.placing_stroke.as_mut())
                    && !click_on_hud(game, vec2(x, y))
                    && crate::render::minimap_world_at(game, vec2(x, y)).is_none()
                {
                    let world = game.camera.to_world(vec2(x, y));
                    let anchor = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
                    let (w, h) = kind.stats().size;
                    let overlaps = stroke
                        .anchors
                        .iter()
                        .any(|a| (a.x - anchor.x).abs() < w && (a.y - anchor.y).abs() < h);
                    let cost = kind.stats().construction.map(|c| c.cost).unwrap_or(0);
                    // Reserve only the stamps the tick hasn't CHARGED
                    // yet: the frame loop drains pending into the sim
                    // mid-drag, and the bank already reflects
                    // everything drained. Reserving the whole stroke
                    // billed those stamps twice and cut a funded wall
                    // to half its length (the re-review's measured
                    // catch); reserving nothing let a fast drag stage
                    // a wall the seat can't pay for.
                    let reserved = pending_build_bill(game);
                    let affordable =
                        game.state.player(game.human).scrap >= cost.saturating_add(reserved);
                    // The cap is the BUILDER's remaining headroom, not
                    // a flat count: a Shift stroke inherits the queue
                    // it appends behind.
                    if !overlaps
                        && !overlaps_pending_site(game, kind, anchor)
                        && affordable
                        && stroke.queued < oxide_sim::stats::ORDER_QUEUE_CAP
                        && game.state.can_place(game.human, kind, anchor)
                    {
                        let units = game.selection.units.clone();
                        game.issue(Command::Build {
                            units,
                            kind,
                            anchor,
                            queue: true,
                        });
                        game.ping(world, PingKind::Rally);
                        stroke.anchors.push(anchor);
                        stroke.queued += 1;
                    }
                }
            }
            RawEvent::Wheel { delta } => {
                let delta = if input.camera_prefs.zoom_inverted {
                    -delta
                } else {
                    delta
                };
                game.camera.zoom_at(input.mouse, delta);
            }
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x,
                y,
            } => {
                input.mouse = vec2(x, y);
                if armed_click(game, input, vec2(x, y)) {
                    continue;
                }
                // Panel cards are buttons: each carries the exact action
                // its click performs — the same action its hotkey routes.
                let layout = game.layout.get();
                let card_hit = layout.cards[..layout.card_count]
                    .iter()
                    .chain(layout.queue_slots[..layout.queue_count].iter())
                    .find(|(r, _)| r.w > 0.0 && r.contains(vec2(x, y)))
                    .map(|(_, a)| *a);
                if let Some(action) = card_hit {
                    activate_card(game, input, action);
                    continue;
                }
                // The idle badge cycles workers on click.
                let badge = game.layout.get().idle_badge;
                if badge.w > 0.0 && badge.contains(vec2(x, y)) {
                    cycle_idle_worker(game);
                    continue;
                }
                // The minimap owns clicks landing on it: jump the camera,
                // never start a drag-select there. HUD chrome swallows
                // clicks outright.
                if let Some(world) = crate::render::minimap_world_at(game, vec2(x, y)) {
                    game.camera.center = world;
                    game.camera.pan(Vec2::ZERO); // re-clamp
                    input.minimap_drag = true;
                } else if !click_on_hud(game, vec2(x, y)) {
                    input.drag_origin = Some(vec2(x, y));
                }
            }
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x,
                y,
            } => {
                input.mouse = vec2(x, y);
                input.minimap_drag = false;
                // The placement stroke ends at release; Shift decides
                // whether the MODE stays armed, exactly as the old
                // one-click-per-wall rule did.
                if input.placing_stroke.take().is_some() && !input.resolver.shift_held() {
                    input.placing = None;
                }
                if let Some(origin) = input.drag_origin.take() {
                    let release = vec2(x, y);
                    let additive = input.resolver.shift_held();
                    if origin.distance(release) <= click_slop(input.ui) {
                        let now = input.now;
                        let double = !additive
                            && input.last_click.take().is_some_and(|(t, p)| {
                                now - t < 0.35 && p.distance(release) <= 12.0 * input.ui
                            });
                        if double {
                            select_all_of_kind_on_screen(game, release, input.ui);
                        } else {
                            click_select(game, release, additive, input.ui);
                        }
                        input.last_click = Some((now, release));
                    } else {
                        box_select(game, origin, release, additive);
                    }
                }
            }
            RawEvent::MouseDown {
                button: MouseButton::Right,
                x,
                y,
            } => {
                input.mouse = vec2(x, y);
                // A right-click on the minimap orders to that world tile
                // (ground semantics — entities can't be picked at that
                // scale); anywhere else, full context ordering. HUD chrome
                // swallows the click.
                let queue = input.resolver.shift_held();
                if let Some(world) = crate::render::minimap_world_at(game, vec2(x, y)) {
                    let tile = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
                    if let Some(route) = &mut input.patrol_route {
                        if route.len() >= oxide_sim::stats::ORDER_QUEUE_CAP {
                            game.toast("patrol is full: R starts it");
                        } else {
                            route.push(tile);
                            game.ping(vec2(world.x, world.y), PingKind::Rally);
                        }
                    } else {
                        let units = game.selection.units.clone();
                        // The same commandability gate the world path
                        // applies: an inspected ally or enemy takes no
                        // orders from the minimap either.
                        if !units.is_empty() && game.selection_commandable() {
                            game.issue(Command::AttackMove {
                                units,
                                goal: tile,
                                queue,
                            });
                            game.ping(vec2(world.x, world.y), PingKind::Move);
                        }
                    }
                } else if !click_on_hud(game, vec2(x, y)) {
                    let world = game.camera.to_world(vec2(x, y));
                    if let Some(route) = &mut input.patrol_route {
                        if route.len() >= oxide_sim::stats::ORDER_QUEUE_CAP {
                            game.toast("patrol is full: R starts it");
                        } else {
                            route
                                .push(TilePos::new(world.x.floor() as i32, world.y.floor() as i32));
                            game.ping(world, PingKind::Rally);
                        }
                    } else {
                        context_order(game, vec2(x, y), queue);
                    }
                }
            }
            RawEvent::MouseUp {
                button: MouseButton::Right,
                ..
            } => {}
            RawEvent::MouseDown {
                button: MouseButton::Middle,
                x,
                y,
            } => {
                input.mmb_anchor = Some(vec2(x, y));
            }
            RawEvent::MouseUp {
                button: MouseButton::Middle,
                ..
            } => {
                input.mmb_anchor = None;
            }
            RawEvent::KeyDown { key } => {
                if let Some(ActionEvent::Pressed(action)) = input.key_edge(key, true) {
                    dispatch_action(game, input, action);
                }
            }
            RawEvent::KeyUp { key } => {
                let _ = input.key_edge(key, false);
            }
            // Desktop shell; the mobile shell will map these.
            RawEvent::TouchDown { id, x, y } => {
                let p = vec2(x, y);
                input.touches.retain(|(tid, _)| *tid != id);
                let chrome =
                    crate::render::minimap_world_at(game, p).is_some() || click_on_hud(game, p);
                input.touches.push((
                    id,
                    TouchPoint {
                        origin: p,
                        at: p,
                        down_at: input.now,
                        moved: false,
                        fired: false,
                        chrome,
                    },
                ));
                if input.touches.len() > 2 {
                    // Three fingers mean nothing yet; the oldest yields.
                    input.touches.remove(0);
                }
                if input.touches.len() == 2 {
                    // A fresh pair starts undecided, whatever the last
                    // pair was doing — a pinch must not outlive its
                    // fingers and swallow the next pair's box.
                    input.pinching = false;
                    input.pair_dist =
                        Some((input.touches[0].1.at - input.touches[1].1.at).length());
                } else {
                    input.pair_dist = None;
                }
            }
            RawEvent::TouchMove { id, x, y } => {
                let p = vec2(x, y);
                let slop = click_slop(input.ui) * 2.0;
                let two = input.touches.len() == 2;
                let old_dist =
                    two.then(|| (input.touches[0].1.at - input.touches[1].1.at).length());
                let mut delta = Vec2::ZERO;
                if let Some((_, tp)) = input.touches.iter_mut().find(|(tid, _)| *tid == id) {
                    delta = p - tp.at;
                    tp.at = p;
                    if (p - tp.origin).length() > slop {
                        tp.moved = true;
                    }
                }
                match input.touches.len() {
                    // One moved finger drags the world under the hand —
                    // unless it landed on chrome, whose ground it keeps.
                    1 if input.touches[0].1.moved && !input.touches[0].1.chrome => {
                        game.camera.center -= delta / game.camera.zoom;
                        game.camera.pan(Vec2::ZERO); // re-clamp
                    }
                    // Two fingers: a spread that has CUMULATIVELY moved
                    // past the threshold is a pinch (zoom at the
                    // midpoint) — per-event deltas would miss a slow
                    // pinch entirely and mis-commit it as a box select.
                    2 => {
                        let new_dist = (input.touches[0].1.at - input.touches[1].1.at).length();
                        if !input.pinching
                            && let Some(start) = input.pair_dist
                            && (new_dist - start).abs() > 24.0 * input.ui
                        {
                            input.pinching = true;
                        }
                        if input.pinching
                            && let Some(old) = old_dist
                        {
                            let spread = new_dist - old;
                            if spread != 0.0 {
                                let mid = (input.touches[0].1.at + input.touches[1].1.at) * 0.5;
                                game.camera.zoom_at(mid, spread * 0.02);
                            }
                        }
                    }
                    _ => {}
                }
            }
            RawEvent::Text { .. } => {
                // Typed characters exist for menu text fields (the
                // save-name flow); gameplay deliberately has no text
                // consumer — letters reach the world as semantic keys.
            }
            RawEvent::TouchUp { id, x, y } => {
                let p = vec2(x, y);
                let Some(pos) = input.touches.iter().position(|(tid, _)| *tid == id) else {
                    continue;
                };
                let (_, lifted) = input.touches.remove(pos);
                match input.touches.len() {
                    // Second finger of a pair released: a pair that
                    // never pinched commits the box between the fingers
                    // — both corners world-born; a chrome-born finger
                    // boxes nothing behind its panel.
                    1 => {
                        if !input.pinching && !lifted.chrome && !input.touches[0].1.chrome {
                            let other = input.touches[0].1.at;
                            box_select(game, other, p, false);
                        }
                        // The survivor is spent EITHER way: after a box
                        // or a pinch, its own still release must not
                        // read as a tap and select whatever sits under
                        // the resting finger.
                        input.touches[0].1.moved = true;
                    }
                    0 => {
                        input.pinching = false;
                        input.pair_dist = None;
                        if !lifted.moved && !lifted.fired {
                            // A short still touch is a tap: select. Two
                            // taps inside the window sweep the kind,
                            // like a double-click.
                            let double = input.last_tap.is_some_and(|(t, at)| {
                                (input.now - t) * 1000.0
                                    < f64::from(input.touch_prefs.double_tap_ms)
                                    && (at - p).length() < click_slop(input.ui) * 2.0
                            });
                            // Armed modes first, exactly like the
                            // mouse: the tap that follows an armed
                            // Build or Salvage card completes the
                            // command instead of selecting under it.
                            // A tap is an atomic click — no drag can
                            // follow, so the stroke closes here and
                            // Shift decides the mode, like MouseUp.
                            if armed_click(game, input, p) {
                                if input.placing_stroke.take().is_some()
                                    && !input.resolver.shift_held()
                                {
                                    input.placing = None;
                                }
                                input.last_tap = None;
                                continue;
                            }
                            // The minimap owns its taps (jump the
                            // camera), and HUD chrome swallows the
                            // rest — same ownership order as clicks,
                            // or a tap behind the panel would select
                            // (and a minimap tap would grab) whatever
                            // world ground happens to sit under the
                            // chrome pixel.
                            if let Some(world) = crate::render::minimap_world_at(game, p) {
                                game.camera.center = world;
                                game.camera.pan(Vec2::ZERO); // re-clamp
                                continue;
                            }
                            // Chrome next, through the touch pad: a
                            // fingertip needs 44 logical px even where
                            // the drawn card is smaller.
                            let layout = game.layout.get();
                            let card = layout.cards[..layout.card_count]
                                .iter()
                                .chain(layout.queue_slots[..layout.queue_count].iter())
                                .find(|(r, _)| {
                                    r.w > 0.0 && crate::layout::touch_pad(*r, input.ui).contains(p)
                                })
                                .map(|(_, a)| *a);
                            let badge = layout.idle_badge;
                            if let Some(action) = card {
                                activate_card(game, input, action);
                            } else if badge.w > 0.0
                                && crate::layout::touch_pad(badge, input.ui).contains(p)
                            {
                                // The idle badge cycles workers by
                                // fingertip too — it sits in the top
                                // bar, which the bare-chrome swallow
                                // below would otherwise eat.
                                cycle_idle_worker(game);
                            } else if click_on_hud(game, p) {
                                // Bare chrome: the tap is swallowed.
                            } else if double {
                                select_all_of_kind_on_screen(game, p, input.ui);
                                input.last_tap = None;
                            } else {
                                click_select(game, p, false, input.ui);
                                input.last_tap = Some((input.now, p));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// One armed world click or tap — placement or salvage — at screen
/// point `p`. Returns whether an armed mode consumed the event
/// (whatever the outcome: issued, denied, or a minimap camera jump).
/// Mouse and touch route here identically: a fingertip that armed a
/// Build card completes the build with its next tap.
fn armed_click(game: &mut Game, input: &mut InputState, p: Vec2) -> bool {
    if let Some(kind) = input.placing {
        // The minimap keeps its meaning while placing: jump the
        // camera, never misread the click as world ground (that would
        // spend scrap on a bogus tile).
        if let Some(world) = crate::render::minimap_world_at(game, p) {
            game.camera.center = world;
            game.camera.pan(Vec2::ZERO); // re-clamp
        } else if !click_on_hud(game, p) {
            let world = game.camera.to_world(p);
            let anchor = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
            // The ghost already showed red; a misclick must not throw
            // away the armed mode on top of it. The toast names the
            // actual blocker — "needs open ground" while your own
            // harvester stands on the tile taught nobody anything.
            if let Some(refusal) = game.state.place_refusal(game.human, kind, anchor) {
                use oxide_sim::PlaceRefusal;
                game.toast(match refusal {
                    PlaceRefusal::Fog => "can't build there: ground not in sight",
                    PlaceRefusal::Terrain => "can't build there: impassable ground",
                    PlaceRefusal::Building => "can't build there: something already stands there",
                    PlaceRefusal::Unit => {
                        "can't build there: an enemy machine is holding that ground"
                    }
                    PlaceRefusal::NotConstructible => "that can't be built",
                });
                game.sounds_pending
                    .push((crate::game::SoundKind::Denied, None));
                return true;
            }
            // Ground already spoken for by a staged-but-undrained site
            // refuses like any other blocker — live state can't see it
            // while the shell is paused, and the sim would reject the
            // stamp the moment the earlier site claims the footprint.
            if overlaps_pending_site(game, kind, anchor) {
                game.toast("can't build there: ground already spoken for");
                game.sounds_pending
                    .push((crate::game::SoundKind::Denied, None));
                return true;
            }
            // The opening stamp affords itself (plus whatever staged
            // builds the tick hasn't charged) before it fires — a
            // broke click gets the honest toast, not an
            // acknowledgment ping followed by a sim rejection.
            let cost = kind.stats().construction.map(|c| c.cost).unwrap_or(0);
            let bill = pending_build_bill(game);
            if game.state.player(game.human).scrap < cost.saturating_add(bill) {
                game.toast(format!("not enough scrap for a {}", kind.name()));
                game.sounds_pending
                    .push((crate::game::SoundKind::Denied, None));
                return true;
            }
            // The opening stamp must also FIT the builder's program: a
            // Shift click onto a crew already at the order-queue cap
            // would ping and then die in the sim as QueueFull. Same
            // honest refusal as the broke click, mode stays armed.
            // (stroke_queued reads sim state, which this frame's issue
            // hasn't touched — before or after the stamp, same answer.)
            let depth = stroke_queued(game, input.resolver.shift_held());
            if depth > oxide_sim::stats::ORDER_QUEUE_CAP {
                game.toast("that builder's order queue is full");
                game.sounds_pending
                    .push((crate::game::SoundKind::Denied, None));
                return true;
            }
            let units = game.selection.units.clone();
            // Shift both keeps placing AND queues the build behind the
            // builder's current program — chained construction in one
            // gesture.
            game.issue(Command::Build {
                units,
                kind,
                anchor,
                queue: input.resolver.shift_held(),
            });
            game.ping(world, PingKind::Rally);
            // The stroke opens: dragging stamps more of the same kind,
            // queued. Whether the MODE survives the release is still
            // Shift's call, decided at MouseUp.
            input.placing_stroke = Some(PlacingStroke {
                anchors: vec![anchor],
                queued: depth,
            });
        }
        return true;
    }
    if input.salvaging {
        // The same manners placement keeps: minimap jumps the camera,
        // a misclick keeps the mode armed, and Shift chains teardowns
        // behind the crew's program.
        if let Some(world) = crate::render::minimap_world_at(game, p) {
            game.camera.center = world;
            game.camera.pan(Vec2::ZERO); // re-clamp
        } else if !click_on_hud(game, p) {
            let world = game.camera.to_world(p);
            let tile = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
            let target = game.state.building_at(tile).filter(|b| {
                b.player == game.human && b.built && b.kind != oxide_sim::BuildingKind::Foundry
            });
            let Some(building) = target.map(|b| b.id) else {
                game.toast("salvage wants an own built building (not a Foundry)");
                game.sounds_pending
                    .push((crate::game::SoundKind::Denied, None));
                return true;
            };
            let units = game.selection.units.clone();
            game.issue(Command::Salvage {
                units,
                building,
                queue: input.resolver.shift_held(),
            });
            game.ping(world, PingKind::Harvest);
            if !input.resolver.shift_held() {
                input.salvaging = false;
            }
        }
        return true;
    }
    if input.running {
        // Same manners again: minimap jumps the camera, HUD swallows,
        // Shift chains legs and keeps the verb armed.
        if let Some(world) = crate::render::minimap_world_at(game, p) {
            game.camera.center = world;
            game.camera.pan(Vec2::ZERO); // re-clamp
        } else if !click_on_hud(game, p) {
            let world = game.camera.to_world(p);
            let goal = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
            let units = game.selection.units.clone();
            game.issue(Command::Move {
                units,
                goal,
                queue: input.resolver.shift_held(),
            });
            game.ping(world, PingKind::Move);
            if !input.resolver.shift_held() {
                input.running = false;
            }
        }
        return true;
    }
    false
}

/// Continuous per-frame input (held-key panning).
/// One panel card pressed — by mouse or fingertip, the same act its
/// hotkey performs.
fn activate_card(game: &mut Game, input: &mut InputState, action: crate::panel::CardAction) {
    match action {
        crate::panel::CardAction::Dispatch(a) => {
            dispatch_action(game, input, a);
        }
        crate::panel::CardAction::ArmBuild(kind) => {
            input.build_menu = false;
            input.disarm_click_verbs();
            input.placing = Some(kind);
            let cost = kind.stats().construction.map(|c| c.cost).unwrap_or(0);
            game.toast(format!(
                "placing {} ({} scrap): click to build, Shift chains, Esc to cancel",
                kind.name(),
                cost
            ));
        }
        crate::panel::CardAction::CancelQueue(building, index) => {
            game.issue(Command::CancelTrain { building, index });
        }
        crate::panel::CardAction::ClearRally(building) => {
            game.issue(Command::SetRally {
                building,
                rally: None,
            });
        }
        crate::panel::CardAction::FilterKind(kind) => {
            // The cut is shell-side only: selections are presentation,
            // no command leaves here.
            let keep = !input.resolver.ctrl_held();
            game.selection.units.retain(|id| {
                game.state
                    .unit(*id)
                    .is_some_and(|u| (u.kind == kind) == keep)
            });
        }
        crate::panel::CardAction::None => {}
    }
}

/// The long-press carrier: a held finger emits no events, so its timer
/// rides the frame loop beside `update_held`. A single still touch
/// past the window fires the context gesture ONCE — on an entity it
/// inspects (tap-select), on ground it issues the context order for
/// the current selection, exactly like a right-click.
pub fn update_touch(game: &mut Game, input: &mut InputState) {
    if input.touches.len() != 1 {
        return;
    }
    let (_, tp) = input.touches[0];
    if tp.moved || tp.fired {
        return;
    }
    if (input.now - tp.down_at) * 1000.0 < f64::from(input.touch_prefs.long_press_ms) {
        return;
    }
    input.touches[0].1.fired = true;
    // Chrome owns its ground for the held finger too: a long-press on
    // the minimap or panel band must not order the army to the world
    // point hiding under the HUD.
    if crate::render::minimap_world_at(game, tp.at).is_some() || click_on_hud(game, tp.at) {
        return;
    }
    let world = game.camera.to_world(tp.at);
    let tile = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
    // Only entities the viewer can actually SEE steer the gesture — an
    // omniscient probe here let a hidden hostile under the fog flip a
    // rally into a select, making occupancy observable through touch.
    let sees = |t: TilePos| game.all_seeing() || game.my_vision().visible(t);
    let on_entity = game.state.units().iter().any(|u| {
        let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
        p.distance(world) <= PICK_RADIUS && (u.player == game.human || sees(u.tile()))
    }) || game
        .state
        .building_at(tile)
        .is_some_and(|b| b.player == game.human || sees(tile));
    if on_entity && game.selection.units.is_empty() {
        select::click_select(game, tp.at, false, input.ui);
    } else {
        orders::context_order(game, tp.at, false);
    }
}

pub fn update_held(game: &mut Game, input: &InputState, dt: f32) {
    let mut dir = vec2(0.0, 0.0);
    if input.resolver.is_held(Action::PanUp) {
        dir.y -= 1.0;
    }
    if input.resolver.is_held(Action::PanDown) {
        dir.y += 1.0;
    }
    if input.resolver.is_held(Action::PanLeft) {
        dir.x -= 1.0;
    }
    if input.resolver.is_held(Action::PanRight) {
        dir.x += 1.0;
    }
    if input.camera_prefs.edge_pan && dir == vec2(0.0, 0.0) {
        // The pointer at a window edge pans — opt-in, because it fights
        // windowed-mode mousing; keyboard panning always wins when both
        // speak.
        const EDGE: f32 = 8.0;
        let viewport = game.camera.viewport();
        if input.mouse.x <= EDGE {
            dir.x -= 1.0;
        } else if input.mouse.x >= viewport.x - EDGE {
            dir.x += 1.0;
        }
        if input.mouse.y <= EDGE {
            dir.y -= 1.0;
        } else if input.mouse.y >= viewport.y - EDGE {
            dir.y += 1.0;
        }
    }
    if dir != vec2(0.0, 0.0) {
        let world_per_sec = PAN_PX_PER_SEC * input.camera_prefs.pan_speed / game.camera.zoom;
        game.camera.pan(dir.normalize() * world_per_sec * dt);
    }
}

/// Normalizes a raw wheel reading toward gentle notch counts. Trackpads
/// report small continuous deltas, discrete wheels big notchy ones
/// (±120-ish); both should zoom at a comparable, capped rate. Heuristic —
/// revisit if a device feels off (small whole numbers — X11-style
/// detents — count as full notches; fractional deltas are trackpads).
fn normalize_wheel(raw: f32) -> f32 {
    let delta = if raw.abs() >= 40.0 {
        raw / 120.0
    } else if raw.abs() <= 3.0 && raw.fract() == 0.0 {
        // X11-style discrete detents arrive as small whole numbers;
        // trackpads produce fractional deltas. Exact integers are notches.
        raw
    } else {
        raw / 10.0
    };
    delta.clamp(-3.0, 3.0)
}
