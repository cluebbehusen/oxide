//! The shell's session state: one sim, its bots, the recorder, and every
//! piece of presentation state (camera, selection, interpolation, effects).
//!
//! The dividing line is absolute: `state` evolves only inside [`Game::do_tick`]
//! via tick-stamped commands, and everything else in this struct is allowed
//! to be as floaty and frame-dependent as it likes because none of it feeds
//! back into the sim.

use crate::camera::Camera;
use anyhow::Result;
use chassis::replay::Replay;
use macroquad::prelude::{Vec2, vec2};
use oxide_protocol::hash_hex;
use oxide_sim::bot::Bot;
use oxide_sim::{
    Building, BuildingId, Command, Event, PlayerCommand, PlayerId, SIM_VERSION, Scenario, State,
    TICKS_PER_SECOND, UnitId,
};
use std::collections::HashMap;

/// Seconds per sim tick.
pub const TICK_DT: f32 = 1.0 / TICKS_PER_SECOND as f32;
/// Ticks a single frame may run before we let rendering catch up.
const MAX_TICKS_PER_FRAME: u32 = 8;

/// The concrete session replay type.
pub type GameReplay = Replay<Scenario, PlayerCommand>;

/// What the player currently has selected.
#[derive(Default)]
pub struct Selection {
    /// Selected own units.
    pub units: Vec<UnitId>,
    /// Selected own building (mutually exclusive with units in practice).
    pub building: Option<BuildingId>,
}

/// A transient visual effect (never sim-relevant).
pub struct Effect {
    /// What to draw.
    pub kind: EffectKind,
    /// Seconds alive.
    pub age: f32,
}

/// Effect shapes.
pub enum EffectKind {
    /// An attack beam.
    Laser {
        /// Muzzle, world coords.
        from: Vec2,
        /// Impact, world coords.
        to: Vec2,
    },
    /// A death pop.
    Puff {
        /// Center, world coords.
        at: Vec2,
    },
}

/// One running session.
pub struct Game {
    /// The scenario this session started from.
    pub scenario: Scenario,
    /// The sim. Touch only through [`Game::do_tick`].
    pub state: State,
    /// Command sources for bot-flagged players.
    pub bots: Vec<Bot>,
    /// Every command of the session, tick-stamped — always recording.
    pub recorder: GameReplay,
    /// Commands staged for the next tick (human + debug socket).
    pub pending: Vec<PlayerCommand>,
    /// The seat local input controls.
    pub human: PlayerId,
    /// Presentation camera.
    pub camera: Camera,
    /// Current selection.
    pub selection: Selection,
    /// Wall clock stopped?
    pub paused: bool,
    /// Wall-clock multiplier.
    pub speed: f64,
    /// Debug overlay on?
    pub overlay: bool,
    /// Positions at the previous tick, for render interpolation.
    pub prev_pos: HashMap<u32, Vec2>,
    /// Sprite rotation per unit (radians; 0 = up).
    pub facing: HashMap<u32, f32>,
    /// Live effects.
    pub fx: Vec<Effect>,
    accum: f32,
}

fn world_vec(pos: chassis::fx::Vec2Fx) -> Vec2 {
    vec2(pos.x.to_num::<f32>(), pos.y.to_num::<f32>())
}

impl Game {
    /// Starts a session from a scenario.
    pub fn new(scenario: Scenario) -> Result<Self> {
        let state = scenario.build()?;
        let bots = Bot::for_scenario(&scenario);
        let recorder = Replay::new(SIM_VERSION, scenario.clone());
        let human = PlayerId(0);
        let focus = state
            .buildings
            .iter()
            .find(|b| b.player == human)
            .map(|b| world_vec(b.center()))
            .unwrap_or_else(|| {
                vec2(
                    state.map.width() as f32 * 0.5,
                    state.map.height() as f32 * 0.5,
                )
            });
        let camera = Camera::new(focus, state.map.width(), state.map.height());
        Ok(Self {
            scenario,
            state,
            bots,
            recorder,
            pending: Vec::new(),
            human,
            camera,
            selection: Selection::default(),
            paused: false,
            speed: 1.0,
            overlay: false,
            prev_pos: HashMap::new(),
            facing: HashMap::new(),
            fx: Vec::new(),
            accum: 0.0,
        })
    }

    /// Resumes a session from a recorded replay: rebuild its scenario,
    /// re-execute every recorded tick (headless-fast), and keep recording
    /// onto the same log. In a deterministic sim a replay *is* a save file
    /// — this is "load game".
    pub fn from_replay(replay: GameReplay) -> Result<Self> {
        let scenario = replay.setup.clone();
        let mut state = scenario.build()?;
        let total = replay
            .meta
            .ticks
            .unwrap_or_else(|| replay.commands.last().map_or(0, |c| c.tick + 1));
        let mut cursor = replay.cursor();
        for _ in 0..total {
            let commands: Vec<PlayerCommand> = cursor
                .take_tick(state.tick)
                .iter()
                .map(|t| t.command.clone())
                .collect();
            state.tick(&commands);
        }
        let mut game = Self::new(scenario)?;
        game.state = state;
        game.recorder = replay;
        if let Some(focus) = game
            .state
            .buildings
            .iter()
            .find(|b| b.player == game.human)
            .map(|b| world_vec(b.center()))
        {
            game.camera.center = focus;
            game.camera.pan(Vec2::ZERO); // re-clamp
        }
        Ok(game)
    }

    /// Runs exactly one tick: bots think, staged commands drain, everything
    /// is recorded, presentation caches update. The only place `state.tick`
    /// is called.
    pub fn do_tick(&mut self) {
        self.prev_pos = self
            .state
            .units
            .iter()
            .map(|u| (u.id.0, world_vec(u.pos)))
            .collect();

        let mut commands = std::mem::take(&mut self.pending);
        for bot in &mut self.bots {
            commands.extend(bot.act(&self.state));
        }
        for command in &commands {
            self.recorder.record(self.state.tick, command.clone());
        }
        let report = self.state.tick(&commands);

        for unit in &self.state.units {
            let now = world_vec(unit.pos);
            if let Some(prev) = self.prev_pos.get(&unit.id.0) {
                let delta = now - *prev;
                if delta.length_squared() > 1e-6 {
                    self.facing.insert(
                        unit.id.0,
                        delta.y.atan2(delta.x) + std::f32::consts::FRAC_PI_2,
                    );
                }
            }
        }
        self.spawn_fx(&report.events);
        self.selection
            .units
            .retain(|id| self.state.unit(*id).is_some());
        if let Some(b) = self.selection.building
            && self.state.building(b).is_none()
        {
            self.selection.building = None;
        }
    }

    /// Advances the sim from wall time (the normal play path).
    pub fn advance_wall_clock(&mut self, dt: f32) {
        if self.paused {
            return;
        }
        self.accum += dt * self.speed as f32;
        let mut ran = 0;
        while self.accum >= TICK_DT && ran < MAX_TICKS_PER_FRAME {
            self.accum -= TICK_DT;
            self.do_tick();
            ran += 1;
        }
        // Behind by more than a frame's worth of ticks? Drop the debt
        // rather than spiraling.
        if ran == MAX_TICKS_PER_FRAME {
            self.accum = self.accum.min(TICK_DT);
        }
    }

    /// Fast-forwards `n` ticks immediately, pause state notwithstanding
    /// (the debug socket's driven-clock mode).
    pub fn advance_ticks(&mut self, n: u64) {
        for _ in 0..n {
            self.do_tick();
        }
        // No cross-jump interpolation after a bulk advance.
        self.accum = 0.0;
        self.prev_pos = self
            .state
            .units
            .iter()
            .map(|u| (u.id.0, world_vec(u.pos)))
            .collect();
    }

    /// Interpolation factor for rendering between ticks.
    pub fn render_alpha(&self) -> f32 {
        if self.paused {
            1.0
        } else {
            (self.accum / TICK_DT).clamp(0.0, 1.0)
        }
    }

    /// Stages a command from the local player for the next tick.
    pub fn issue(&mut self, command: Command) {
        self.pending.push(PlayerCommand {
            player: self.human,
            command,
        });
    }

    /// The human's first Foundry (hotkey target, camera home).
    pub fn home_foundry(&self) -> Option<&Building> {
        self.state.buildings.iter().find(|b| b.player == self.human)
    }

    /// Current state fingerprint, protocol-formatted.
    pub fn hash_hex(&self) -> String {
        hash_hex(self.state.hash())
    }

    /// The local player's fog view (what rendering and targeting honor).
    pub fn my_vision(&self) -> &oxide_sim::Vision {
        self.state.vision(self.human)
    }

    /// Ages and prunes effects.
    pub fn update_fx(&mut self, dt: f32) {
        for fx in &mut self.fx {
            fx.age += dt;
        }
        self.fx.retain(|fx| {
            fx.age
                < match fx.kind {
                    EffectKind::Laser { .. } => 0.15,
                    EffectKind::Puff { .. } => 0.4,
                }
        });
    }

    fn spawn_fx(&mut self, events: &[Event]) {
        for event in events {
            match event {
                Event::AttackHit { attacker, target } => {
                    let Some(from) = self.state.unit(*attacker).map(|u| world_vec(u.pos)) else {
                        continue;
                    };
                    let to = match target {
                        oxide_sim::Target::Unit(id) => {
                            self.state.unit(*id).map(|u| world_vec(u.pos))
                        }
                        oxide_sim::Target::Building(id) => {
                            self.state.building(*id).map(|b| world_vec(b.center()))
                        }
                    };
                    if let Some(to) = to {
                        self.fx.push(Effect {
                            kind: EffectKind::Laser { from, to },
                            age: 0.0,
                        });
                    }
                }
                Event::UnitDied { pos, .. } => {
                    self.fx.push(Effect {
                        kind: EffectKind::Puff {
                            at: world_vec(*pos),
                        },
                        age: 0.0,
                    });
                }
                Event::BuildingDestroyed { pos, .. } => {
                    self.fx.push(Effect {
                        kind: EffectKind::Puff {
                            at: world_vec(*pos),
                        },
                        age: 0.0,
                    });
                }
                _ => {}
            }
        }
    }

    /// Interpolated draw position for a unit.
    pub fn draw_pos(&self, id: UnitId, current: chassis::fx::Vec2Fx, alpha: f32) -> Vec2 {
        let now = world_vec(current);
        match self.prev_pos.get(&id.0) {
            Some(prev) => prev.lerp(now, alpha),
            None => now,
        }
    }
}
