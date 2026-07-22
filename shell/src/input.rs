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
use oxide_sim::{Command, Target, UnitId, UnitKind};

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
    groups: [Vec<UnitId>; 5],
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
            groups: Default::default(),
            last_click: None,
            last_recall: None,
            patrol_route: None,
            placing: None,
            build_menu: false,
            ui: 1.0,
            now: 0.0,
            camera_prefs: crate::config::CameraPrefs::default(),
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
        self.build_menu = false;
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
pub fn poll_events(input: &InputState) -> Vec<RawEvent> {
    let mut events = Vec::new();
    let (mx, my) = mq::mouse_position();
    if vec2(mx, my) != input.mouse {
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

/// Own harvesters with nothing to do, in id order — the cycle key and
/// the HUD badge both read this.
pub fn idle_harvesters(game: &Game) -> Vec<UnitId> {
    game.state
        .units()
        .iter()
        .filter(|u| {
            u.player == game.human
                && u.kind == UnitKind::Harvester
                && u.order == oxide_sim::Order::Idle
        })
        .map(|u| u.id)
        .collect()
}

/// Selects the next idle harvester after the current selection (id
/// order, wrapping) and centers the camera on it. Stateless: the
/// selection itself is the cursor.
fn cycle_idle_worker(game: &mut Game) {
    let idle = idle_harvesters(game);
    let Some(&first) = idle.first() else {
        game.toast("no idle harvesters");
        return;
    };
    let next = match game.selection.units.as_slice() {
        [current] => idle
            .iter()
            .copied()
            .find(|id| id > current)
            .unwrap_or(first),
        _ => first,
    };
    game.selection.units = vec![next];
    game.selection.building = None;
    let unit = game.state.unit(next).expect("listed above");
    game.camera.center = vec2(unit.pos.x.to_num::<f32>(), unit.pos.y.to_num::<f32>());
    game.camera.pan(Vec2::ZERO); // re-clamp
}

/// World-space pick radius around a unit: generous when zoomed out so
/// units never need tweezers (at least 10 logical px on screen).
fn pick_radius(game: &Game, ui: f32) -> f32 {
    (10.0 * ui / game.camera.zoom).max(0.6)
}

/// HUD chrome that swallows clicks: the top bar always; the bottom panel
/// only while it is actually shown — and as tall as it actually drew
/// (the packed palette wraps to several rows on narrow windows; clicks
/// on the upper rows must not fall through to the world).
fn click_on_hud(game: &mut Game, screen: Vec2) -> bool {
    game.layout.get().chrome_owns(screen)
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
                if let Some(kind) = input.placing {
                    // The minimap keeps its meaning while placing: jump
                    // the camera, never misread the click as world ground
                    // (that would spend scrap on a bogus tile).
                    if let Some(world) = crate::render::minimap_world_at(game, vec2(x, y)) {
                        game.camera.center = world;
                        game.camera.pan(Vec2::ZERO); // re-clamp
                    } else if !click_on_hud(game, vec2(x, y)) {
                        let world = game.camera.to_world(vec2(x, y));
                        let anchor = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
                        // The ghost already showed red; a misclick must
                        // not throw away the armed mode on top of it.
                        if !game.state.can_place(game.human, kind, anchor) {
                            game.toast("can't build there — needs open, visible ground");
                            game.sounds_pending.push(crate::game::SoundKind::Denied);
                            continue;
                        }
                        let units = game.selection.units.clone();
                        game.issue(Command::Build {
                            units,
                            kind,
                            anchor,
                        });
                        game.ping(world, PingKind::Rally);
                        // Shift keeps placing: walls go up one click at
                        // a time, not one arming at a time.
                        if !input.resolver.shift_held() {
                            input.placing = None;
                        }
                    }
                    continue;
                }
                // Panel slots are buttons: a click on a produce slot or
                // palette entry means exactly what its digit means.
                let slots = game.layout.get().panel_slots;
                if let Some(slot) = slots
                    .iter()
                    .position(|r| r.w > 0.0 && r.contains(vec2(x, y)))
                {
                    digit_action(game, input, slot);
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
                            game.toast("patrol is full — R to start it");
                        } else {
                            route.push(tile);
                            game.ping(vec2(world.x, world.y), PingKind::Rally);
                        }
                    } else {
                        let units = game.selection.units.clone();
                        if !units.is_empty() {
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
                            game.toast("patrol is full — R to start it");
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
            RawEvent::TouchDown { .. } | RawEvent::TouchMove { .. } | RawEvent::TouchUp { .. } => {}
        }
    }
}

/// Continuous per-frame input (held-key panning).
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

fn click_select(game: &mut Game, screen: Vec2, additive: bool, ui: f32) {
    let world = game.camera.to_world(screen);
    if !additive {
        game.selection.building = None;
    }
    // Nearest own unit within pick range wins…
    let radius = pick_radius(game, ui);
    let picked = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == game.human)
        .map(|u| {
            let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
            (p.distance(world), u.id)
        })
        .filter(|(d, _)| *d <= radius)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, id)) = picked {
        if additive {
            // Shift-click toggles membership.
            if let Some(index) = game.selection.units.iter().position(|u| *u == id) {
                game.selection.units.remove(index);
            } else {
                game.selection.units.push(id);
            }
        } else {
            game.selection.units = vec![id];
        }
        return;
    }
    if additive {
        return; // shift-miss leaves the selection alone
    }
    // …then an own building under the cursor…
    let tile = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
    if let Some(building) = game.state.building_at(tile)
        && building.player == game.human
    {
        game.selection.units.clear();
        game.selection.building = Some(building.id);
        return;
    }
    // …otherwise clear.
    game.selection.units.clear();
}

fn box_select(game: &mut Game, a_screen: Vec2, b_screen: Vec2, additive: bool) {
    let a = game.camera.to_world(a_screen);
    let b = game.camera.to_world(b_screen);
    let (lo, hi) = (a.min(b), a.max(b));
    game.selection.building = None;
    let mut boxed: Vec<UnitId> = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == game.human)
        .filter(|u| {
            let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
            p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y
        })
        .map(|u| u.id)
        .collect();
    if additive {
        boxed.extend(game.selection.units.iter().copied());
        boxed.sort_unstable();
        boxed.dedup();
    }
    game.selection.units = boxed;
}

/// Double-click: everyone of the clicked unit's kind currently on screen.
fn select_all_of_kind_on_screen(game: &mut Game, screen: Vec2, ui: f32) {
    let world = game.camera.to_world(screen);
    let radius = pick_radius(game, ui);
    let kind = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == game.human)
        .map(|u| {
            let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
            (p.distance(world), u.kind)
        })
        .filter(|(d, _)| *d <= radius)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, k)| k);
    let Some(kind) = kind else {
        return;
    };
    let (lo, hi) = game.camera.world_rect();
    game.selection.building = None;
    game.selection.units = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == game.human && u.kind == kind)
        .filter(|u| {
            let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
            p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y
        })
        .map(|u| u.id)
        .collect();
}

/// Digits are contextual: an open build palette spends them on
/// structures, a selected own factory spends them on production, and
/// otherwise the first five are control groups.
fn digit_action(game: &mut Game, input: &mut InputState, slot: usize) {
    if input.build_menu {
        if let Some(&kind) = BUILD_PALETTE.get(slot) {
            input.build_menu = false;
            input.placing = Some(kind);
            let cost = kind.stats().construction.map(|c| c.cost).unwrap_or(0);
            game.toast(format!(
                "placing {} ({} scrap): click to build, Esc to cancel",
                kind.name(),
                cost
            ));
        }
        return;
    }
    let producing = game.selection.building.is_some_and(|id| {
        game.state.building(id).is_some_and(|b| {
            b.player == game.human && b.built && !b.kind.stats().produces.is_empty()
        })
    });
    if producing {
        train(game, slot);
        return;
    }
    if slot < 5 {
        group_action(game, input, slot);
    }
}

/// Recall (or with Ctrl, assign) a control group; a quick double-tap on
/// the same slot centers the camera on the group.
fn group_action(game: &mut Game, input: &mut InputState, slot: usize) {
    // Ownership, not mere existence: after a session change a stale id
    // could name anyone's unit (belt to reset_session's suspenders).
    let alive: Vec<UnitId> = input.groups[slot]
        .iter()
        .copied()
        .filter(|id| game.state.unit(*id).is_some_and(|u| u.player == game.human))
        .collect();
    input.groups[slot] = alive.clone();
    if alive.is_empty() {
        return;
    }
    game.selection.units = alive.clone();
    game.selection.building = None;
    let now = input.now;
    if input
        .last_recall
        .is_some_and(|(s, t)| s == slot && now - t < 0.4)
    {
        let mut sum = vec2(0.0, 0.0);
        for id in &alive {
            let u = game.state.unit(*id).expect("pruned above");
            sum += vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
        }
        game.camera.center = sum / alive.len() as f32;
        game.camera.pan(Vec2::ZERO); // re-clamp
    }
    input.last_recall = Some((slot, now));
}

/// Right-click: order the selection by what's under the cursor — enemy →
/// attack, scrap → harvest, ground → move. The sim re-validates everything;
/// this is only intent.
fn context_order(game: &mut Game, screen: Vec2, queue: bool) {
    let world = game.camera.to_world(screen);
    let tile = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
    if game.selection.units.is_empty() {
        // A selected own building takes right-clicks as its rally point.
        if let Some(building) = game.selection.building
            && game
                .state
                .building(building)
                .is_some_and(|b| b.player == game.human)
        {
            game.issue(Command::SetRally {
                building,
                rally: Some(tile),
            });
            game.ping(world, PingKind::Rally);
        }
        return;
    }
    let units = game.selection.units.clone();

    // Fog rules what right-click may target: unseen enemies aren't there
    // as far as the player is concerned (the sim enforces this too).
    let enemy_unit = game
        .state
        .units()
        .iter()
        .filter(|u| game.state.hostile(game.human, u.player) && game.my_vision().visible(u.tile()))
        .map(|u| {
            let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
            (p.distance(world), u.id)
        })
        .filter(|(d, _)| *d <= PICK_RADIUS)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, target)) = enemy_unit {
        let at = game
            .state
            .unit(target)
            .map(|u| vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>()))
            .unwrap_or(world);
        game.issue(Command::Attack {
            units,
            target: Target::Unit(target),
            queue,
        });
        game.ping(at, PingKind::Attack);
        return;
    }
    if let Some(building) = game.state.building_at(tile)
        && game.state.hostile(game.human, building.player)
        && building.tiles().any(|t| game.my_vision().visible(t))
    {
        let target = Target::Building(building.id);
        game.issue(Command::Attack {
            units,
            target,
            queue,
        });
        game.ping(world, PingKind::Attack);
        return;
    }
    let has_harvester = units.iter().any(|id| {
        game.state
            .unit(*id)
            .is_some_and(|u| u.kind == UnitKind::Harvester)
    });
    // A wounded own building under the cursor puts harvesters to welding.
    if has_harvester
        && let Some(building) = game.state.building_at(tile)
        && building.player == game.human
        && building.built
        && building.hp < building.kind.stats().max_hp
    {
        game.issue(Command::Repair {
            units,
            building: building.id,
        });
        game.ping(world, PingKind::Harvest);
        return;
    }
    // The harvest check reads the player's *memory*, not the live map —
    // probing fog with right-clicks must not reveal hidden scrap. Wreck
    // memory counts the same as node memory.
    if (game.my_vision().remembered_scrap(tile) > 0 || game.my_vision().remembered_wreck(tile) > 0)
        && has_harvester
    {
        game.issue(Command::Harvest {
            units,
            node: tile,
            queue,
        });
        game.ping(world, PingKind::Harvest);
        return;
    }
    // Fire at will: ground orders engage whatever shows up on the way.
    // Combat units attack-move; the sim degrades harvesters to a plain
    // walk. There is no hold-fire stance (yet — nothing to hide from).
    game.issue(Command::AttackMove {
        units,
        goal: tile,
        queue,
    });
    game.ping(world, PingKind::Move);
}

fn dispatch_action(game: &mut Game, input: &mut InputState, action: Action) {
    match action {
        // Continuous pans live in update_held; Confirm belongs to menus.
        Action::PanLeft | Action::PanRight | Action::PanUp | Action::PanDown => {}
        Action::Confirm => {}
        Action::Slot(n) => digit_action(game, input, (n - 1) as usize),
        Action::AssignGroup(n) => {
            // Groups 1-5, like the recall side; the classic layout never
            // had more.
            let slot = (n - 1) as usize;
            if slot < input.groups.len() {
                input.groups[slot] = game.selection.units.clone();
            }
        }
        Action::StopOrScrap => {
            // Contextual: units selected halt in place; a selected own
            // unfinished site is scrapped for its refund.
            if !game.selection.units.is_empty() {
                let units = game.selection.units.clone();
                game.issue(Command::Stop { units });
            } else if let Some(id) = game.selection.building
                && game
                    .state
                    .building(id)
                    .is_some_and(|b| b.player == game.human && !b.built)
            {
                game.issue(Command::Cancel { building: id });
                game.selection.building = None;
            }
        }
        Action::TrainSlot(n) => train(game, n as usize),
        Action::TogglePause => game.paused = !game.paused,
        Action::ToggleBuildPalette => {
            if input.build_menu {
                input.build_menu = false;
                return;
            }
            let has_builder = game.selection.units.iter().any(|id| {
                game.state
                    .unit(*id)
                    .is_some_and(|u| u.kind == UnitKind::Harvester)
            });
            if has_builder {
                input.build_menu = true;
                input.placing = None;
            } else {
                game.toast("select a harvester to build");
            }
        }
        Action::Patrol => {
            // First press arms a route; the second sends the circuit.
            match input.patrol_route.take() {
                None if !game.selection.units.is_empty() => {
                    input.patrol_route = Some(Vec::new());
                    game.toast("patrol: right-click waypoints, R to start");
                }
                None => {}
                Some(route) if route.is_empty() => {
                    game.toast("patrol cancelled");
                }
                Some(waypoints) => {
                    let units = game.selection.units.clone();
                    game.issue(Command::Patrol { units, waypoints });
                }
            }
        }
        Action::ToggleOverlay => game.overlay = !game.overlay,
        Action::Back => {
            // Arming something? Escape abandons that first.
            if input.build_menu {
                input.build_menu = false;
                return;
            }
            if input.placing.take().is_some() {
                game.toast("placement cancelled");
                return;
            }
            if input.patrol_route.take().is_some() {
                game.toast("patrol cancelled");
                return;
            }
            game.selection.units.clear();
            game.selection.building = None;
        }
        Action::SetBookmark(slot) => {
            input.bookmarks[slot as usize] = Some(game.camera.center);
            game.toast(format!("bookmark {} set", slot + 1));
        }
        Action::RecallBookmark(slot) => {
            if let Some(center) = input.bookmarks[slot as usize] {
                game.camera.center = center;
                game.camera.pan(Vec2::ZERO); // re-clamp
            }
        }
        Action::CycleIdleWorker => cycle_idle_worker(game),
        Action::JumpToLastAlert => {
            if let Some(world) = game.last_alert {
                game.camera.center = world;
                game.camera.pan(Vec2::ZERO); // re-clamp
            } else {
                game.toast("no recent alerts");
            }
        }
        Action::HomeCamera => {
            if let Some(center) = game.home_foundry().map(|b| b.center()) {
                let target = vec2(center.x.to_num::<f32>(), center.y.to_num::<f32>());
                game.camera.center = target;
                game.camera.pan(vec2(0.0, 0.0)); // re-clamp
            }
        }
    }
}

/// Train the selected factory's Nth product (the seat's own roster —
/// the other faction's variants are skipped). `H`/`S` alias the first
/// two slots; no factory selected falls back to the home Foundry.
fn train(game: &mut Game, slot: usize) {
    let building = game
        .selection
        .building
        .filter(|id| {
            game.state
                .building(*id)
                .is_some_and(|b| b.player == game.human)
        })
        .or_else(|| game.home_foundry().map(|b| b.id));
    if let Some(building) = building {
        let faction = game.state.player(game.human).faction;
        let Some(&kind) = game.state.building(building).and_then(|b| {
            b.kind
                .stats()
                .produces
                .iter()
                .filter(|k| k.faction().is_none_or(|f| f == faction))
                .nth(slot)
        }) else {
            return;
        };
        game.issue(Command::Train { building, kind });
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

#[cfg(test)]
mod tests {
    use super::*;

    fn headless_game() -> Game {
        Game::with_viewport(oxide_sim::Scenario::skirmish(), vec2(1280.0, 800.0), 1.0)
            .expect("embedded skirmish builds")
    }

    fn click(x: f32, y: f32) -> [RawEvent; 2] {
        [
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x,
                y,
            },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x,
                y,
            },
        ]
    }

    #[test]
    fn bookmarks_remember_and_recall_camera_ground() {
        let mut game = headless_game();
        let mut input = InputState::new();
        let saved = game.camera.center;
        let chord = |game: &mut Game, input: &mut InputState, ctrl: bool, key: Key| {
            let mut ev = Vec::new();
            if ctrl {
                ev.push(RawEvent::KeyDown { key: Key::Ctrl });
            }
            ev.push(RawEvent::KeyDown { key });
            ev.push(RawEvent::KeyUp { key });
            if ctrl {
                ev.push(RawEvent::KeyUp { key: Key::Ctrl });
            }
            apply_events(game, input, &ev);
        };
        chord(&mut game, &mut input, true, Key::F5);
        game.camera.center = saved + vec2(6.0, 4.0);
        chord(&mut game, &mut input, false, Key::F5);
        assert!(
            (game.camera.center - saved).length() < 1e-4,
            "recall returns to the remembered ground"
        );
        chord(&mut game, &mut input, false, Key::F6);
        assert!(
            (game.camera.center - saved).length() < 1e-4,
            "an empty slot recalls nothing"
        );
    }

    #[test]
    fn the_cycle_key_walks_idle_harvesters_in_id_order() {
        let mut game = headless_game();
        let mut input = InputState::new();
        let idle = idle_harvesters(&game);
        assert!(idle.len() >= 2, "premise: skirmish opens with idle workers");
        let press = |game: &mut Game, input: &mut InputState| {
            apply_events(
                game,
                input,
                &[
                    RawEvent::KeyDown { key: Key::N },
                    RawEvent::KeyUp { key: Key::N },
                ],
            );
        };
        press(&mut game, &mut input);
        assert_eq!(game.selection.units, vec![idle[0]]);
        press(&mut game, &mut input);
        assert_eq!(game.selection.units, vec![idle[1]], "id order, forward");
        for _ in 0..idle.len() - 1 {
            press(&mut game, &mut input);
        }
        assert_eq!(game.selection.units, vec![idle[0]], "and wraps");
    }

    #[test]
    fn a_misclick_keeps_placement_armed_and_a_shift_click_repeats() {
        let mut game = headless_game();
        let mut input = InputState::new();
        // Arm a turret with a harvester selected (the palette's path).
        let harvester = game
            .state
            .units()
            .iter()
            .find(|u| u.kind == UnitKind::Harvester && u.player == game.human)
            .unwrap()
            .id;
        game.selection.units = vec![harvester];
        input.placing = Some(oxide_sim::BuildingKind::Turret);

        // Skirmish's own foundry footprint is illegal ground: the
        // misclick toasts and stays armed, staging nothing.
        let foundry = game.state.buildings()[0].anchor;
        let bad = game
            .camera
            .to_screen(vec2(foundry.x as f32 + 0.5, foundry.y as f32 + 0.5));
        apply_events(
            &mut game,
            &mut input,
            &[RawEvent::MouseDown {
                button: MouseButton::Left,
                x: bad.x,
                y: bad.y,
            }],
        );
        assert!(input.placing.is_some(), "a misclick must not disarm");
        assert!(game.pending.is_empty(), "and must spend nothing");

        // Shift-click on open visible ground stages and stays armed.
        let open = game
            .camera
            .to_screen(vec2(foundry.x as f32 + 3.5, foundry.y as f32 + 3.5));
        apply_events(
            &mut game,
            &mut input,
            &[
                RawEvent::KeyDown { key: Key::Shift },
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x: open.x,
                    y: open.y,
                },
            ],
        );
        assert_eq!(game.pending.len(), 1, "legal ground stages the site");
        assert!(input.placing.is_some(), "shift keeps the wall going up");

        // A plain click disarms after staging.
        apply_events(
            &mut game,
            &mut input,
            &[
                RawEvent::KeyUp { key: Key::Shift },
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x: open.x + 96.0,
                    y: open.y,
                },
            ],
        );
        assert!(input.placing.is_none(), "a plain click finishes the job");
    }

    #[test]
    fn a_click_on_a_unit_selects_it_headlessly() {
        // The whole event path — resolver, hit-testing, selection —
        // exercised with no window: the C5 extraction's proof.
        let mut game = headless_game();
        let mut input = InputState::new();
        let unit = game.state.units()[0].id;
        let pos = game.state.units()[0].pos;
        let screen = game
            .camera
            .to_screen(vec2(pos.x.to_num::<f32>(), pos.y.to_num::<f32>()));
        apply_events(&mut game, &mut input, &click(screen.x, screen.y));
        assert_eq!(game.selection.units, vec![unit]);
    }

    #[test]
    fn a_right_click_on_ground_stages_an_attack_move() {
        let mut game = headless_game();
        let mut input = InputState::new();
        let pos = game.state.units()[0].pos;
        let screen = game
            .camera
            .to_screen(vec2(pos.x.to_num::<f32>(), pos.y.to_num::<f32>()));
        apply_events(&mut game, &mut input, &click(screen.x, screen.y));
        let mid = game.camera.to_screen(vec2(
            pos.x.to_num::<f32>() + 4.0,
            pos.y.to_num::<f32>() + 2.0,
        ));
        apply_events(
            &mut game,
            &mut input,
            &[RawEvent::MouseDown {
                button: MouseButton::Right,
                x: mid.x,
                y: mid.y,
            }],
        );
        assert!(
            game.pending
                .iter()
                .any(|c| matches!(c.command, Command::AttackMove { .. })),
            "fire-at-will ground order staged: {:?}",
            game.pending
        );
    }

    #[test]
    fn double_click_timing_obeys_the_injected_clock() {
        let mut game = headless_game();
        let mut input = InputState::new();
        let u = &game.state.units()[0];
        let (kind, pos) = (u.kind, u.pos);
        let same_kind_total = game
            .state
            .units()
            .iter()
            .filter(|o| o.kind == kind && o.player == game.human)
            .count();
        assert!(same_kind_total > 1, "premise: kin on screen to sweep up");
        let screen = game
            .camera
            .to_screen(vec2(pos.x.to_num::<f32>(), pos.y.to_num::<f32>()));
        input.now = 10.0;
        apply_events(&mut game, &mut input, &click(screen.x, screen.y));
        // A slow second click is just a click...
        input.now = 11.0;
        apply_events(&mut game, &mut input, &click(screen.x, screen.y));
        assert_eq!(game.selection.units.len(), 1, "1.0s apart is two clicks");
        // ...a fast one is a kind-sweep.
        input.now = 11.2;
        apply_events(&mut game, &mut input, &click(screen.x, screen.y));
        assert!(
            game.selection.units.len() > 1,
            "0.2s apart double-clicks into a kind sweep"
        );
    }

    #[test]
    fn wheel_notches_and_trackpad_swipes_land_in_the_same_range() {
        // Windows notches (±120), X11 detents (±1), and a firm trackpad
        // swipe all read as whole steps; small fractional trackpad deltas
        // stay gentle.
        assert_eq!(normalize_wheel(120.0), 1.0);
        assert_eq!(normalize_wheel(-120.0), -1.0);
        assert_eq!(normalize_wheel(1.0), 1.0);
        assert_eq!(normalize_wheel(-1.0), -1.0);
        assert_eq!(normalize_wheel(2.0), 2.0);
        assert_eq!(normalize_wheel(10.0), 1.0);
        assert!(normalize_wheel(0.4) > 0.0 && normalize_wheel(0.4) < 0.1);
    }

    #[test]
    fn wheel_bursts_are_capped() {
        assert_eq!(normalize_wheel(1200.0), 3.0);
        assert_eq!(normalize_wheel(-1200.0), -3.0);
        // The cap also catches fast trackpad flicks below the notch cutoff.
        assert_eq!(normalize_wheel(39.9), 3.0);
    }

    #[test]
    fn every_build_palette_entry_costs_scrap_to_raise() {
        // The palette is exactly what a harvester can place, so each entry
        // must carry construction stats with a real price. A `None` (a
        // Foundry-style scenario-only kind) or a zero cost would offer a
        // ghost the sim can never accept.
        for kind in BUILD_PALETTE {
            let cost = kind
                .stats()
                .construction
                .unwrap_or_else(|| {
                    panic!("{} is in the palette but not constructable", kind.name())
                })
                .cost;
            assert!(cost > 0, "{} is free to build", kind.name());
        }
    }

    #[test]
    fn the_build_palette_has_no_duplicate_structures() {
        // A repeated kind would burn a digit slot on a structure already
        // reachable by another digit.
        for (i, a) in BUILD_PALETTE.iter().enumerate() {
            for b in BUILD_PALETTE.iter().skip(i + 1) {
                assert_ne!(a, b, "{} appears twice", a.name());
            }
        }
    }

    #[test]
    fn the_build_palette_fits_the_digit_selectors() {
        // `digit_action` indexes the palette with slots 0..=8 (number keys
        // 1-9); an entry past the ninth could never be selected.
        assert!(
            BUILD_PALETTE.len() <= 9,
            "palette overflows the 1-9 digit range"
        );
    }

    #[test]
    fn key_map_binds_each_logical_key_at_most_once() {
        // Two rows sharing a logical Key would leave one keycode's binding
        // dead — whichever row `poll_events` reaches second is unreachable.
        for (i, a) in KEY_MAP.iter().enumerate() {
            for b in KEY_MAP.iter().skip(i + 1) {
                assert_ne!(a.0, b.0, "logical key bound twice: {:?}", a.0);
            }
        }
    }

    #[test]
    fn each_physical_key_drives_at_most_one_logical_key() {
        // A repeated keycode silently shadows: `poll_events` emits the first
        // row's logical key and the second row never fires.
        for (i, a) in KEY_MAP.iter().enumerate() {
            for b in KEY_MAP.iter().skip(i + 1) {
                assert_ne!(a.1, b.1, "keycode bound twice: {:?}", a.1);
            }
        }
    }
}
