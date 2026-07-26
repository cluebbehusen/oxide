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
    /// Last *hardware* cursor position the poll saw. Change detection
    /// must compare against this, not `mouse`: injected pointer events
    /// move `mouse`, and comparing the idle OS cursor against it once
    /// re-emitted a phantom MouseMove every frame — which fought every
    /// injected drag for the pointer.
    hw_mouse: Vec2,
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
    /// Armed salvage: the next left-click on an own built building
    /// sends the selected harvesters to strip it.
    pub(crate) salvaging: bool,
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
            hw_mouse: vec2(0.0, 0.0),
            groups: Default::default(),
            last_click: None,
            last_recall: None,
            patrol_route: None,
            placing: None,
            salvaging: false,
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
    pub fn reset_transient(&mut self) {
        self.resolver.clear();
        self.drag_origin = None;
        self.minimap_drag = false;
        self.mmb_anchor = None;
        self.patrol_route = None;
        self.placing = None;
        self.salvaging = false;
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

const KEY_MAP: [(Key, mq::KeyCode); 42] = [
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
];

/// Converts this frame's hardware input into events. Purely a poll→event
/// adapter; interpretation happens in [`apply_events`].
pub fn poll_events(input: &mut InputState) -> Vec<RawEvent> {
    let mut events = Vec::new();
    let (mx, my) = mq::mouse_position();
    if vec2(mx, my) != input.hw_mouse {
        input.hw_mouse = vec2(mx, my);
        events.push(RawEvent::MouseMove { x: mx, y: my });
    }
    let wheel = mq::mouse_wheel().1;
    if wheel != 0.0 {
        events.push(RawEvent::Wheel {
            delta: normalize_wheel(wheel),
        });
    }
    for (button, mq_button) in [
        (MouseButton::Left, mq::MouseButton::Left),
        (MouseButton::Right, mq::MouseButton::Right),
        (MouseButton::Middle, mq::MouseButton::Middle),
    ] {
        if mq::is_mouse_button_pressed(mq_button) {
            events.push(RawEvent::MouseDown {
                button,
                x: mx,
                y: my,
            });
        }
        if mq::is_mouse_button_released(mq_button) {
            events.push(RawEvent::MouseUp {
                button,
                x: mx,
                y: my,
            });
        }
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
    if input.placing.is_some() || input.patrol_route.is_some() || input.salvaging {
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
                input.touches.push((
                    id,
                    TouchPoint {
                        origin: p,
                        at: p,
                        down_at: input.now,
                        moved: false,
                        fired: false,
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
                    // One moved finger drags the world under the hand.
                    1 if input.touches[0].1.moved => {
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
            RawEvent::TouchUp { id, x, y } => {
                let p = vec2(x, y);
                let Some(pos) = input.touches.iter().position(|(tid, _)| *tid == id) else {
                    continue;
                };
                let (_, lifted) = input.touches.remove(pos);
                match input.touches.len() {
                    // Second finger of a pair released: a pair that
                    // never pinched commits the box between the fingers.
                    1 => {
                        if !input.pinching {
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
                            if armed_click(game, input, p) {
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
                            if let Some(action) = card {
                                activate_card(game, input, action);
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
            // away the armed mode on top of it.
            if !game.state.can_place(game.human, kind, anchor) {
                game.toast("can't build there: needs open, visible ground");
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
            // Shift keeps placing: walls go up one click at a time,
            // not one arming at a time.
            if !input.resolver.shift_held() {
                input.placing = None;
            }
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
            input.placing = Some(kind);
            let cost = kind.stats().construction.map(|c| c.cost).unwrap_or(0);
            game.toast(format!(
                "placing {} ({} scrap): click to build, Esc to cancel",
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
