//! The input funnel.
//!
//! One path, no exceptions: macroquad's polled state becomes [`RawEvent`]s,
//! injected events from the debug socket are appended to the same list, and
//! [`apply_events`] is the only code that turns events into camera motion,
//! selection, or sim commands. If input behavior ever bypasses this module,
//! injected tests stop meaning anything — don't.

use crate::game::{Game, PingKind};
use chassis::grid::TilePos;
use macroquad::prelude::{self as mq, Vec2, vec2};
use oxide_protocol::{Key, MouseButton, RawEvent};
use oxide_sim::{Command, Target, UnitId, UnitKind};
use std::collections::HashSet;

/// Logical pixels of mouse travel under which a press+release counts as a
/// click (scaled by dpi at use).
const CLICK_SLOP: f32 = 6.0;

fn click_slop() -> f32 {
    CLICK_SLOP * crate::render::ui_scale()
}

/// Shared with the drag-rectangle renderer, so what draws as a drag is
/// exactly what selects as one.
pub fn drag_threshold() -> f32 {
    click_slop()
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
    /// Control groups 1..=5 (assigned with Ctrl+N, recalled with N).
    groups: [Vec<UnitId>; 5],
    /// Previous click, for double-click detection.
    last_click: Option<(f64, Vec2)>,
    /// Previous group recall, for double-tap camera centering.
    last_recall: Option<(usize, f64)>,
    /// Waypoints collected while arming a patrol (`R`), if any.
    pub(crate) patrol_route: Option<Vec<TilePos>>,
    /// Building kind armed for placement (`B`/`N`), if any.
    pub(crate) placing: Option<oxide_sim::BuildingKind>,
    held: HashSet<KeyOrd>,
}

/// `Key` wrapped for `HashSet` (the protocol enum keeps no Hash to stay
/// serde-minimal).
#[derive(PartialEq, Eq, Hash, Clone, Copy)]
struct KeyOrd(u8);

fn key_ord(key: Key) -> KeyOrd {
    KeyOrd(match key {
        Key::Up => 0,
        Key::Down => 1,
        Key::Left => 2,
        Key::Right => 3,
        Key::H => 4,
        Key::S => 5,
        Key::P => 6,
        Key::R => 19,
        Key::B => 20,
        Key::N => 21,
        Key::Escape => 7,
        Key::Space => 8,
        Key::F1 => 9,
        Key::A => 10,
        Key::Enter => 11,
        Key::Shift => 12,
        Key::Ctrl => 13,
        Key::Num1 => 14,
        Key::Num2 => 15,
        Key::Num3 => 16,
        Key::Num4 => 17,
        Key::Num5 => 18,
    })
}

impl InputState {
    /// Fresh input state.
    pub fn new() -> Self {
        Self {
            mouse: vec2(0.0, 0.0),
            drag_origin: None,
            groups: Default::default(),
            last_click: None,
            last_recall: None,
            patrol_route: None,
            placing: None,
            held: HashSet::new(),
        }
    }

    fn is_held(&self, key: Key) -> bool {
        self.held.contains(&key_ord(key))
    }

    /// Drops everything that assumes continuity — held keys and any open
    /// drag — keeping only the cursor position. Called on every mode
    /// transition: a menu eats the matching release events, and stale
    /// held-state otherwise pans the camera forever (or fires a phantom
    /// box-select) after resuming.
    pub fn reset_transient(&mut self) {
        self.held.clear();
        self.drag_origin = None;
        self.patrol_route = None;
        self.placing = None;
    }

    /// Everything `reset_transient` drops, plus state that assumes the
    /// *match* continues: control groups, double-click memory, recall
    /// timing. Called whenever the `Game` is replaced (restart, new map,
    /// replay load) — unit ids restart from zero there, and a stale group
    /// would resolve to unrelated units in the new world.
    pub fn reset_session(&mut self) {
        self.reset_transient();
        self.groups = Default::default();
        self.last_click = None;
        self.last_recall = None;
    }
}

const KEY_MAP: [(Key, mq::KeyCode); 14] = [
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
    (Key::Escape, mq::KeyCode::Escape),
    (Key::Space, mq::KeyCode::Space),
    (Key::F1, mq::KeyCode::F1),
    (Key::Enter, mq::KeyCode::Enter),
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
    for (key, code) in KEY_MAP {
        if mq::is_key_pressed(code) {
            events.push(RawEvent::KeyDown { key });
        }
        if mq::is_key_released(code) {
            events.push(RawEvent::KeyUp { key });
        }
    }
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
    for (key, code) in [
        (Key::Num1, mq::KeyCode::Key1),
        (Key::Num2, mq::KeyCode::Key2),
        (Key::Num3, mq::KeyCode::Key3),
        (Key::Num4, mq::KeyCode::Key4),
        (Key::Num5, mq::KeyCode::Key5),
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

/// World-space pick radius around a unit: generous when zoomed out so
/// units never need tweezers (at least 10 logical px on screen).
fn pick_radius(game: &Game) -> f32 {
    (10.0 * crate::render::ui_scale() / game.camera.zoom).max(0.6)
}

/// HUD chrome that swallows clicks: the top bar always; the bottom panel
/// only while it is actually shown.
fn click_on_hud(game: &mut Game, screen: Vec2) -> bool {
    let s = crate::render::ui_scale();
    let viewport = game.camera.viewport();
    if screen.y <= 32.0 * s {
        return true;
    }
    let panel_shown = game.selection.building.is_some() || !game.selection.units.is_empty();
    panel_shown && screen.y >= viewport.y - 36.0 * s
}

/// Applies a frame's events — hardware and injected alike — to the game.
pub fn apply_events(game: &mut Game, input: &mut InputState, events: &[RawEvent]) {
    for event in events {
        match *event {
            RawEvent::MouseMove { x, y } => input.mouse = vec2(x, y),
            RawEvent::Wheel { delta } => game.camera.zoom_at(input.mouse, delta),
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x,
                y,
            } => {
                input.mouse = vec2(x, y);
                if let Some(kind) = input.placing {
                    if !click_on_hud(game, vec2(x, y)) {
                        let world = game.camera.to_world(vec2(x, y));
                        let anchor = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
                        let units = game.selection.units.clone();
                        game.issue(Command::Build {
                            units,
                            kind,
                            anchor,
                        });
                        game.ping(world, PingKind::Rally);
                        input.placing = None;
                    }
                    continue;
                }
                // The minimap owns clicks landing on it: jump the camera,
                // never start a drag-select there. HUD chrome swallows
                // clicks outright.
                if let Some(world) = crate::render::minimap_world_at(game, vec2(x, y)) {
                    game.camera.center = world;
                    game.camera.pan(Vec2::ZERO); // re-clamp
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
                if let Some(origin) = input.drag_origin.take() {
                    let release = vec2(x, y);
                    let additive = input.is_held(Key::Shift);
                    if origin.distance(release) <= click_slop() {
                        let now = mq::get_time();
                        let double = !additive
                            && input.last_click.take().is_some_and(|(t, p)| {
                                now - t < 0.35
                                    && p.distance(release) <= 12.0 * crate::render::ui_scale()
                            });
                        if double {
                            select_all_of_kind_on_screen(game, release);
                        } else {
                            click_select(game, release, additive);
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
                let queue = input.is_held(Key::Shift);
                if let Some(world) = crate::render::minimap_world_at(game, vec2(x, y)) {
                    let tile = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
                    if let Some(route) = &mut input.patrol_route {
                        route.push(tile);
                        game.ping(vec2(world.x, world.y), PingKind::Rally);
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
                        route.push(TilePos::new(world.x.floor() as i32, world.y.floor() as i32));
                        game.ping(world, PingKind::Rally);
                    } else {
                        context_order(game, vec2(x, y), queue);
                    }
                }
            }
            RawEvent::MouseUp {
                button: MouseButton::Right,
                ..
            }
            | RawEvent::MouseDown {
                button: MouseButton::Middle,
                ..
            }
            | RawEvent::MouseUp {
                button: MouseButton::Middle,
                ..
            } => {}
            RawEvent::KeyDown { key } => {
                input.held.insert(key_ord(key));
                match key {
                    Key::Num1 => group_action(game, input, 0),
                    Key::Num2 => group_action(game, input, 1),
                    Key::Num3 => group_action(game, input, 2),
                    Key::Num4 => group_action(game, input, 3),
                    Key::Num5 => group_action(game, input, 4),
                    _ => key_action(game, input, key),
                }
            }
            RawEvent::KeyUp { key } => {
                input.held.remove(&key_ord(key));
            }
            // Desktop shell; the mobile shell will map these.
            RawEvent::TouchDown { .. } | RawEvent::TouchMove { .. } | RawEvent::TouchUp { .. } => {}
        }
    }
}

/// Continuous per-frame input (held-key panning).
pub fn update_held(game: &mut Game, input: &InputState, dt: f32) {
    let mut dir = vec2(0.0, 0.0);
    if input.is_held(Key::Up) {
        dir.y -= 1.0;
    }
    if input.is_held(Key::Down) {
        dir.y += 1.0;
    }
    if input.is_held(Key::Left) {
        dir.x -= 1.0;
    }
    if input.is_held(Key::Right) {
        dir.x += 1.0;
    }
    if dir != vec2(0.0, 0.0) {
        let world_per_sec = PAN_PX_PER_SEC * crate::render::ui_scale() / game.camera.zoom;
        game.camera.pan(dir.normalize() * world_per_sec * dt);
    }
}

fn click_select(game: &mut Game, screen: Vec2, additive: bool) {
    let world = game.camera.to_world(screen);
    if !additive {
        game.selection.building = None;
    }
    // Nearest own unit within pick range wins…
    let radius = pick_radius(game);
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
fn select_all_of_kind_on_screen(game: &mut Game, screen: Vec2) {
    let world = game.camera.to_world(screen);
    let radius = pick_radius(game);
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

/// Recall (or with Ctrl, assign) a control group; a quick double-tap on
/// the same slot centers the camera on the group.
fn group_action(game: &mut Game, input: &mut InputState, slot: usize) {
    if input.is_held(Key::Ctrl) {
        input.groups[slot] = game.selection.units.clone();
        return;
    }
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
    let now = mq::get_time();
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
        .filter(|u| u.player != game.human && game.my_vision().visible(u.tile()))
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
        && building.player != game.human
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
    // The harvest check reads the player's *memory*, not the live map —
    // probing fog with right-clicks must not reveal hidden scrap.
    if game.my_vision().remembered_scrap(tile) > 0 && has_harvester {
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

fn building_name(kind: oxide_sim::BuildingKind) -> &'static str {
    match kind {
        oxide_sim::BuildingKind::Foundry => "foundry",
        oxide_sim::BuildingKind::Turret => "turret",
        oxide_sim::BuildingKind::Fabricator => "fabricator",
    }
}

fn key_action(game: &mut Game, input: &mut InputState, key: Key) {
    match key {
        Key::H => train(game, 0),
        Key::S => train(game, 1),
        Key::P => game.paused = !game.paused,
        Key::B | Key::N => {
            let kind = if key == Key::B {
                oxide_sim::BuildingKind::Turret
            } else {
                oxide_sim::BuildingKind::Fabricator
            };
            let has_builder = game.selection.units.iter().any(|id| {
                game.state
                    .unit(*id)
                    .is_some_and(|u| u.kind == UnitKind::Harvester)
            });
            if input.placing == Some(kind) {
                input.placing = None;
            } else if has_builder {
                input.placing = Some(kind);
                let cost = kind.stats().construction.map(|c| c.cost).unwrap_or(0);
                game.toast(format!(
                    "placing {} ({} scrap): click to build, Esc to cancel",
                    building_name(kind),
                    cost
                ));
            } else {
                game.toast("select a harvester to build");
            }
        }
        Key::R => {
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
        Key::F1 => game.overlay = !game.overlay,
        Key::Escape => {
            // Arming something? Escape abandons that first.
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
        Key::Space => {
            if let Some(center) = game.home_foundry().map(|b| b.center()) {
                let target = vec2(center.x.to_num::<f32>(), center.y.to_num::<f32>());
                game.camera.center = target;
                game.camera.pan(vec2(0.0, 0.0)); // re-clamp
            }
        }
        // Pan keys are continuous (update_held); Enter is menu-only;
        // modifiers and group digits are handled in apply_events; A is
        // reserved.
        Key::Up
        | Key::Down
        | Key::Left
        | Key::Right
        | Key::A
        | Key::Enter
        | Key::Shift
        | Key::Ctrl
        | Key::Num1
        | Key::Num2
        | Key::Num3
        | Key::Num4
        | Key::Num5 => {}
    }
}

/// Train at the selected Foundry, falling back to the home one — so H/S
/// work without fiddly building selection.
/// `H`/`S` train the selected factory's first/second product — Foundry:
/// harvester/sentinel, Fabricator: scuttler/lancer. No factory selected
/// falls back to the home Foundry.
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
        let Some(&kind) = game
            .state
            .building(building)
            .and_then(|b| b.kind.stats().produces.get(slot))
        else {
            return;
        };
        game.issue(Command::Train { building, kind });
    }
}

/// Normalizes a raw wheel reading toward gentle notch counts. Trackpads
/// report small continuous deltas, discrete wheels big notchy ones
/// (±120-ish); both should zoom at a comparable, capped rate. Heuristic —
/// revisit if a device feels off. TODO: X11-style ±1 detents land in the
/// trackpad branch and zoom 10× weaker than ±120 detents; needs tuning
/// on real Linux hardware before it's worth guessing at.
fn normalize_wheel(raw: f32) -> f32 {
    let delta = if raw.abs() >= 40.0 {
        raw / 120.0
    } else {
        raw / 10.0
    };
    delta.clamp(-3.0, 3.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_notches_and_trackpad_swipes_land_in_the_same_range() {
        // One mouse notch and a firm trackpad swipe both read as ~1 step.
        assert_eq!(normalize_wheel(120.0), 1.0);
        assert_eq!(normalize_wheel(-120.0), -1.0);
        assert_eq!(normalize_wheel(10.0), 1.0);
        assert!(normalize_wheel(2.0) > 0.0 && normalize_wheel(2.0) < 0.5);
    }

    #[test]
    fn wheel_bursts_are_capped() {
        assert_eq!(normalize_wheel(1200.0), 3.0);
        assert_eq!(normalize_wheel(-1200.0), -3.0);
        // The cap also catches fast trackpad flicks below the notch cutoff.
        assert_eq!(normalize_wheel(39.9), 3.0);
    }
}
