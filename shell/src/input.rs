//! The input funnel.
//!
//! One path, no exceptions: macroquad's polled state becomes [`RawEvent`]s,
//! injected events from the debug socket are appended to the same list, and
//! [`apply_events`] is the only code that turns events into camera motion,
//! selection, or sim commands. If input behavior ever bypasses this module,
//! injected tests stop meaning anything — don't.

use crate::game::Game;
use chassis::grid::TilePos;
use macroquad::prelude::{self as mq, Vec2, vec2};
use oxide_protocol::{Key, MouseButton, RawEvent};
use oxide_sim::{Command, Target, UnitKind};
use std::collections::HashSet;

/// Pixels of mouse travel under which a press+release counts as a click.
const CLICK_SLOP: f32 = 6.0;
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
    /// `A` was pressed: the next left-click issues an attack-move instead
    /// of selecting.
    pub armed_attack_move: bool,
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
        Key::Escape => 7,
        Key::Space => 8,
        Key::F1 => 9,
        Key::A => 10,
        Key::Enter => 11,
    })
}

impl InputState {
    /// Fresh input state.
    pub fn new() -> Self {
        Self {
            mouse: vec2(0.0, 0.0),
            drag_origin: None,
            armed_attack_move: false,
            held: HashSet::new(),
        }
    }

    fn is_held(&self, key: Key) -> bool {
        self.held.contains(&key_ord(key))
    }
}

const KEY_MAP: [(Key, mq::KeyCode); 12] = [
    (Key::Up, mq::KeyCode::Up),
    (Key::Down, mq::KeyCode::Down),
    (Key::Left, mq::KeyCode::Left),
    (Key::Right, mq::KeyCode::Right),
    (Key::H, mq::KeyCode::H),
    (Key::S, mq::KeyCode::S),
    (Key::P, mq::KeyCode::P),
    (Key::Escape, mq::KeyCode::Escape),
    (Key::Space, mq::KeyCode::Space),
    (Key::F1, mq::KeyCode::F1),
    (Key::A, mq::KeyCode::A),
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
        // Normalize platform wheel scales into modest notch counts.
        events.push(RawEvent::Wheel {
            delta: (wheel / 60.0).clamp(-3.0, 3.0),
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
    events
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
                if input.armed_attack_move {
                    input.armed_attack_move = false;
                    let world = game.camera.to_world(vec2(x, y));
                    let units = game.selection.units.clone();
                    if !units.is_empty() {
                        game.issue(Command::AttackMove {
                            units,
                            goal: TilePos::new(world.x.floor() as i32, world.y.floor() as i32),
                        });
                    }
                } else {
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
                    if origin.distance(release) <= CLICK_SLOP {
                        click_select(game, release);
                    } else {
                        box_select(game, origin, release);
                    }
                }
            }
            RawEvent::MouseDown {
                button: MouseButton::Right,
                x,
                y,
            } => {
                input.mouse = vec2(x, y);
                input.armed_attack_move = false; // a direct order overrides
                context_order(game, vec2(x, y));
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
                    Key::A if !game.selection.units.is_empty() => {
                        input.armed_attack_move = true;
                    }
                    Key::Escape => input.armed_attack_move = false,
                    _ => {}
                }
                key_action(game, key);
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
        let world_per_sec = PAN_PX_PER_SEC / game.camera.zoom;
        game.camera.pan(dir.normalize() * world_per_sec * dt);
    }
}

fn click_select(game: &mut Game, screen: Vec2) {
    let world = game.camera.to_world(screen);
    game.selection.building = None;
    // Nearest own unit within pick range wins…
    let picked = game
        .state
        .units
        .iter()
        .filter(|u| u.player == game.human)
        .map(|u| {
            let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
            (p.distance(world), u.id)
        })
        .filter(|(d, _)| *d <= PICK_RADIUS)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, id)) = picked {
        game.selection.units = vec![id];
        return;
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

fn box_select(game: &mut Game, a_screen: Vec2, b_screen: Vec2) {
    let a = game.camera.to_world(a_screen);
    let b = game.camera.to_world(b_screen);
    let (lo, hi) = (a.min(b), a.max(b));
    game.selection.building = None;
    game.selection.units = game
        .state
        .units
        .iter()
        .filter(|u| u.player == game.human)
        .filter(|u| {
            let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
            p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y
        })
        .map(|u| u.id)
        .collect();
}

/// Right-click: order the selection by what's under the cursor — enemy →
/// attack, scrap → harvest, ground → move. The sim re-validates everything;
/// this is only intent.
fn context_order(game: &mut Game, screen: Vec2) {
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
        }
        return;
    }
    let units = game.selection.units.clone();

    // Fog rules what right-click may target: unseen enemies aren't there
    // as far as the player is concerned (the sim enforces this too).
    let enemy_unit = game
        .state
        .units
        .iter()
        .filter(|u| u.player != game.human && game.my_vision().visible(u.tile()))
        .map(|u| {
            let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
            (p.distance(world), u.id)
        })
        .filter(|(d, _)| *d <= PICK_RADIUS)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, target)) = enemy_unit {
        game.issue(Command::Attack {
            units,
            target: Target::Unit(target),
        });
        return;
    }
    if let Some(building) = game.state.building_at(tile)
        && building.player != game.human
        && building.tiles().any(|t| game.my_vision().visible(t))
    {
        let target = Target::Building(building.id);
        game.issue(Command::Attack { units, target });
        return;
    }
    let has_harvester = units.iter().any(|id| {
        game.state
            .unit(*id)
            .is_some_and(|u| u.kind == UnitKind::Harvester)
    });
    if game.state.map.scrap_at(tile) > 0 && has_harvester {
        game.issue(Command::Harvest { units, node: tile });
        return;
    }
    game.issue(Command::Move { units, goal: tile });
}

fn key_action(game: &mut Game, key: Key) {
    match key {
        Key::H => train(game, UnitKind::Harvester),
        Key::S => train(game, UnitKind::Sentinel),
        Key::P => game.paused = !game.paused,
        Key::F1 => game.overlay = !game.overlay,
        Key::Escape => {
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
        // Pan keys are continuous (update_held); A is handled in
        // apply_events where the arming state lives; Enter is menu-only.
        Key::Up | Key::Down | Key::Left | Key::Right | Key::A | Key::Enter => {}
    }
}

/// Train at the selected Foundry, falling back to the home one — so H/S
/// work without fiddly building selection.
fn train(game: &mut Game, kind: UnitKind) {
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
        game.issue(Command::Train { building, kind });
    }
}
