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
/// Ticks a single frame may run before we let rendering catch up. Sized
/// so the advertised 64x speed cap is real at 60 fps (64 × 20 tps ÷ 60);
/// ticks are cheap enough that a full frame of them costs well under 1 ms.
const MAX_TICKS_PER_FRAME: u32 = 24;

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

/// A clip the shell should play (queued by sim events, drained per frame).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SoundKind {
    /// An attack landed somewhere you can see.
    Laser,
    /// A unit died somewhere you can see.
    UnitDeath,
    /// A building fell (yours are always audible).
    BuildingBoom,
    /// Your harvester delivered.
    Deposit,
    /// Your Foundry finished a unit.
    TrainDone,
    /// Menu activation.
    Click,
    /// An order was rejected.
    Denied,
    /// The match ended in your favor.
    Victory,
    /// It did not.
    Defeat,
}

/// What an order-acknowledgment ping means (decides its color).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PingKind {
    /// Move / attack-move destination.
    Move,
    /// Attack target.
    Attack,
    /// Harvest node.
    Harvest,
    /// Rally point.
    Rally,
    /// A unit left the Foundry.
    Spawn,
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
    /// An order acknowledgment: a ring collapsing onto the ordered point.
    Ping {
        /// Center, world coords.
        at: Vec2,
        /// Color class.
        kind: PingKind,
    },
}

/// A transient HUD message (rejected orders, stalled units).
pub struct Toast {
    /// What to say.
    pub text: String,
    /// Seconds since raised.
    pub age: f32,
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
    /// Clips queued by this frame's ticks; the main loop drains and plays.
    pub sounds_pending: Vec<SoundKind>,
    /// Transient HUD messages, newest last.
    pub toasts: Vec<Toast>,
    /// Scorch decals where buildings died: (world pos, seconds old).
    pub scorches: Vec<(Vec2, f32)>,
    /// Session flags for the starter hint strip: cleared once the player
    /// has trained something / sent fighters somewhere.
    pub hinted_train: bool,
    /// See [`Game::hinted_train`].
    pub hinted_fight: bool,
    accum: f32,
    /// True during bulk fast-forwards: presentation (fx, sounds, facing)
    /// is skipped entirely instead of accumulated-then-discarded — a
    /// million-tick advance must not buffer a million battles.
    suppress_presentation: bool,
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
            .buildings()
            .iter()
            .find(|b| b.player == human)
            .map(|b| world_vec(b.center()))
            .unwrap_or_else(|| {
                vec2(
                    state.map().width() as f32 * 0.5,
                    state.map().height() as f32 * 0.5,
                )
            });
        let camera = Camera::new(
            focus,
            state.map().width(),
            state.map().height(),
            macroquad::prelude::vec2(
                macroquad::prelude::screen_width(),
                macroquad::prelude::screen_height(),
            ),
            macroquad::miniquad::window::dpi_scale(),
        );
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
            sounds_pending: Vec::new(),
            toasts: Vec::new(),
            scorches: Vec::new(),
            hinted_train: false,
            hinted_fight: false,
            accum: 0.0,
            suppress_presentation: false,
        })
    }

    /// Resumes a session from a recorded replay: rebuild its scenario,
    /// re-execute every recorded tick (headless-fast), and keep recording
    /// onto the same log. In a deterministic sim a replay *is* a save file
    /// — this is "load game".
    pub fn from_replay(replay: GameReplay) -> Result<Self> {
        // Untrusted file: enforce the invariants recording guarantees, and
        // refuse cross-version saves outright — resuming one would keep
        // recording onto a log that can no longer reproduce.
        replay
            .validate(Some(SIM_VERSION))
            .map_err(|err| anyhow::anyhow!("{err}"))?;
        let scenario = replay.setup.clone();
        let mut state = scenario.build()?;
        let total = replay.meta.ticks.unwrap_or_else(|| {
            replay
                .commands
                .last()
                .map_or(0, |c| c.tick.saturating_add(1))
        });
        // Loading replays synchronously on the frame loop: a structurally
        // valid file can still claim an absurd duration and freeze the UI
        // for minutes. ~28 game-hours is beyond any honest session.
        const MAX_LOAD_TICKS: u64 = 2_000_000;
        anyhow::ensure!(
            total <= MAX_LOAD_TICKS,
            "replay spans {total} ticks — beyond the {MAX_LOAD_TICKS}-tick interactive load limit \
             (the headless driver replays without one)"
        );
        let mut cursor = replay.cursor();
        for _ in 0..total {
            let commands: Vec<PlayerCommand> = cursor
                .take_tick(state.current_tick())
                .iter()
                .map(|t| t.command.clone())
                .collect();
            state.tick(&commands);
        }
        anyhow::ensure!(
            cursor.is_finished(),
            "replay duration metadata does not cover its own commands"
        );
        let mut game = Self::new(scenario)?;
        game.state = state;
        game.recorder = replay;
        if let Some(focus) = game
            .state
            .buildings()
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
    /// is recorded, presentation caches update. The only place `state.current_tick()`
    /// is called.
    pub fn do_tick(&mut self) {
        // Interpolation cache; pointless during suppressed bulk advances
        // (advance_ticks rebuilds it once at the end).
        if !self.suppress_presentation {
            self.prev_pos = self
                .state
                .units()
                .iter()
                .map(|u| (u.id.0, world_vec(u.pos)))
                .collect();
        }

        let mut commands = std::mem::take(&mut self.pending);
        for bot in &mut self.bots {
            commands.extend(bot.act(&self.state));
        }
        for command in &commands {
            self.recorder
                .record(self.state.current_tick(), command.clone());
        }
        let report = self.state.tick(&commands);

        if !self.suppress_presentation {
            for unit in self.state.units() {
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
        }
        self.selection
            .units
            .retain(|id| self.state.unit(*id).is_some());
        let state = &self.state;
        self.facing
            .retain(|id, _| state.unit(UnitId(*id)).is_some());
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
        self.suppress_presentation = true;
        for _ in 0..n {
            self.do_tick();
        }
        self.suppress_presentation = false;
        // No cross-jump interpolation after a bulk advance — and whatever
        // presentation slipped in beforehand doesn't survive the jump.
        self.accum = 0.0;
        self.sounds_pending.clear();
        self.fx.clear();
        self.prev_pos = self
            .state
            .units()
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
        match &command {
            Command::Train { .. } => self.hinted_train = true,
            Command::AttackMove { .. } | Command::Attack { .. } => self.hinted_fight = true,
            _ => {}
        }
        self.pending.push(PlayerCommand {
            player: self.human,
            command,
        });
    }

    /// Drops an order-acknowledgment ping at a world point.
    pub fn ping(&mut self, at: Vec2, kind: PingKind) {
        self.fx.push(Effect {
            kind: EffectKind::Ping { at, kind },
            age: 0.0,
        });
    }

    /// Raises a transient HUD message (capped; oldest fall off).
    pub fn toast(&mut self, text: impl Into<String>) {
        self.toasts.push(Toast {
            text: text.into(),
            age: 0.0,
        });
        if self.toasts.len() > 3 {
            self.toasts.remove(0);
        }
    }

    /// The human's first Foundry (hotkey target, camera home).
    pub fn home_foundry(&self) -> Option<&Building> {
        self.state
            .buildings()
            .iter()
            .find(|b| b.player == self.human)
    }

    /// Current state fingerprint, protocol-formatted.
    pub fn hash_hex(&self) -> String {
        hash_hex(self.state.hash())
    }

    /// The local player's fog view (what rendering and targeting honor).
    pub fn my_vision(&self) -> &oxide_sim::Vision {
        self.state.vision(self.human)
    }

    /// Ages and prunes effects and toasts.
    pub fn update_fx(&mut self, dt: f32) {
        for fx in &mut self.fx {
            fx.age += dt;
        }
        self.fx.retain(|fx| {
            fx.age
                < match fx.kind {
                    EffectKind::Laser { .. } => 0.15,
                    EffectKind::Puff { .. } => 0.4,
                    EffectKind::Ping { .. } => 0.5,
                }
        });
        for toast in &mut self.toasts {
            toast.age += dt;
        }
        self.toasts.retain(|t| t.age < 2.5);
        for (_, age) in &mut self.scorches {
            *age += dt;
        }
        self.scorches.retain(|(_, age)| *age < 20.0);
    }

    /// Turns a tick's events into flashes and queued clips. Sight rules
    /// mirror rendering: positional sounds only play for ground the local
    /// player can see (own losses and own milestones are always audible).
    fn spawn_fx(&mut self, events: &[Event]) {
        let sees = |game: &Self, pos: chassis::fx::Vec2Fx| {
            game.my_vision()
                .visible(chassis::grid::TilePos::containing(pos))
        };
        for event in events {
            match event {
                Event::AttackHit {
                    attacker_pos,
                    target_pos,
                    ..
                } => {
                    // Positions come from the event itself: resolving ids
                    // here loses the beam whenever the hit was lethal.
                    if sees(self, *attacker_pos) || sees(self, *target_pos) {
                        self.sounds_pending.push(SoundKind::Laser);
                    }
                    self.fx.push(Effect {
                        kind: EffectKind::Laser {
                            from: world_vec(*attacker_pos),
                            to: world_vec(*target_pos),
                        },
                        age: 0.0,
                    });
                }
                Event::UnitDied { pos, player, .. } => {
                    if *player == self.human || sees(self, *pos) {
                        self.sounds_pending.push(SoundKind::UnitDeath);
                    }
                    self.fx.push(Effect {
                        kind: EffectKind::Puff {
                            at: world_vec(*pos),
                        },
                        age: 0.0,
                    });
                }
                Event::BuildingDestroyed { pos, player, .. } => {
                    if *player == self.human || sees(self, *pos) {
                        self.sounds_pending.push(SoundKind::BuildingBoom);
                    }
                    self.fx.push(Effect {
                        kind: EffectKind::Puff {
                            at: world_vec(*pos),
                        },
                        age: 0.0,
                    });
                    // A permanent-feeling scar (capped; oldest fall off).
                    self.scorches.push((world_vec(*pos), 0.0));
                    if self.scorches.len() > 16 {
                        self.scorches.remove(0);
                    }
                }
                Event::UnitTrained { unit, player, .. } if *player == self.human => {
                    self.sounds_pending.push(SoundKind::TrainDone);
                    if let Some(u) = self.state.unit(*unit) {
                        self.fx.push(Effect {
                            kind: EffectKind::Ping {
                                at: world_vec(u.pos),
                                kind: PingKind::Spawn,
                            },
                            age: 0.0,
                        });
                    }
                }
                Event::ScrapDeposited { player, .. } if *player == self.human => {
                    self.sounds_pending.push(SoundKind::Deposit);
                }
                Event::CommandRejected { player, reason } if *player == self.human => {
                    let why = match reason {
                        oxide_sim::command::RejectReason::NotEnoughScrap => "not enough scrap",
                        oxide_sim::command::RejectReason::QueueFull => "queue is full",
                        oxide_sim::command::RejectReason::UnreachableGoal => "can't reach that",
                        oxide_sim::command::RejectReason::InvalidTarget => "can't target that",
                        oxide_sim::command::RejectReason::NotANode => "nothing to mine there",
                        oxide_sim::command::RejectReason::NotYourBuilding => "not your building",
                        oxide_sim::command::RejectReason::NoValidUnits => {
                            "nothing selected can do that"
                        }
                        oxide_sim::command::RejectReason::OutOfBounds => "outside the map",
                        oxide_sim::command::RejectReason::Eliminated => "you are eliminated",
                    };
                    self.toast(why);
                    self.sounds_pending.push(SoundKind::Denied);
                }
                Event::OrderStalled { player, pos, .. } if *player == self.human => {
                    self.toast("a unit can't reach its order");
                    self.fx.push(Effect {
                        kind: EffectKind::Ping {
                            at: world_vec(*pos),
                            kind: PingKind::Attack,
                        },
                        age: 0.0,
                    });
                }
                Event::GameOver { result } => {
                    let won = matches!(
                        result,
                        oxide_sim::GameResult::Victory { winner } if *winner == self.human
                    );
                    self.sounds_pending.push(if won {
                        SoundKind::Victory
                    } else {
                        SoundKind::Defeat
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
