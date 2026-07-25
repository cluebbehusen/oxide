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
use oxide_sim::bot::{SeatBot, seat_bots};
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

pub use oxide_kit::GameReplay;

/// What the player currently has selected.
#[derive(Default)]
pub struct Selection {
    /// Selected own units.
    pub units: Vec<UnitId>,
    /// Selected own building (mutually exclusive with units in practice).
    pub building: Option<BuildingId>,
}

/// A transient visual effect (never sim-relevant).
mod fx;

pub use fx::{BoltStyle, Effect, EffectKind, PingKind, SoundKind};

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
    pub bots: Vec<SeatBot>,
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
    /// Combat aim overrides: unit id -> (angle, fx-clock stamp). A shot
    /// turns the shooter toward its victim and holds briefly; movement
    /// facing resumes when the hold expires. Presentation only.
    pub aim_units: HashMap<u32, (f32, f32)>,
    /// Same for buildings (turret mounts track their last victim).
    pub aim_buildings: HashMap<u32, (f32, f32)>,
    /// Live effects.
    pub fx: Vec<Effect>,
    /// Clips queued by this frame's ticks; the main loop drains and plays.
    pub sounds_pending: Vec<(SoundKind, Option<Vec2>)>,
    /// Transient HUD messages, newest last.
    pub toasts: Vec<Toast>,
    /// Scorch decals where buildings died: (world pos, seconds old).
    pub scorches: Vec<(Vec2, f32)>,
    /// Live under-attack alerts: world position and seconds of age.
    /// Pulsed on the minimap, jumpable, aged out by update_fx.
    pub alerts: Vec<(Vec2, f32)>,
    /// Where trouble last landed — the jump key's target.
    pub last_alert: Option<Vec2>,
    /// Per-region rate limiter for alerts (8-tile cells -> last raise
    /// time in fx-seconds), so a running battle nags once, not per hit.
    alert_gate: HashMap<(i32, i32), f32>,
    /// Presentation clock: seconds of fx time since session start.
    fx_clock: f32,
    /// Whether the current session content is already autosaved; a new
    /// tick makes it stale again. Guards double-writes when Main Menu
    /// saves and the same game then quits as the Home backdrop.
    pub autosave_done: bool,
    /// When each remembered tile (ghost anchors, scrap, wrecks) was
    /// last actually seen, on the fx clock — presentation state behind
    /// the staleness ramp. A `RefCell` because drawing holds `&Game`.
    pub last_seen: std::cell::RefCell<HashMap<(i32, i32), f32>>,
    /// The chrome geometry the renderer computed last frame — the one
    /// model hit-testing reads, so drawn and clickable can never
    /// disagree. A `Cell` because drawing holds `&Game`.
    pub layout: std::cell::Cell<crate::layout::LayoutModel>,
    /// The frame's command panel, built once in draw_hud and read by
    /// the tooltip pass — building it twice per frame was pure waste.
    pub panel_model: std::cell::RefCell<Option<crate::panel::Panel>>,
    /// End-of-match statistics, computed once from the recorder when
    /// the result lands (the record IS the match — a re-execution).
    pub end_stats: Option<oxide_kit::stats::MatchStats>,
    /// What the player has demonstrably done — the tutorial's evidence.
    pub demo: crate::tutorial::Demo,
    /// Fog-free viewing without the debug chrome — the playback
    /// viewer's stance. `overlay` remains the developer's F1 (grid,
    /// ids, camera internals) and implies this.
    pub spectate: bool,
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
    /// Starts a session from a scenario, at the injected window size
    /// (headless callers get the default without a window).
    pub fn new(scenario: Scenario) -> Result<Self> {
        Self::with_viewport(scenario, crate::render::viewport())
    }

    /// `new` with the window injected — the only constructor tests use,
    /// because it never touches macroquad.
    pub fn with_viewport(scenario: Scenario, viewport: Vec2) -> Result<Self> {
        let state = scenario.build()?;
        let bots = seat_bots(&scenario);
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
        let camera = Camera::new(focus, state.map().width(), state.map().height(), viewport);
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
            aim_units: HashMap::new(),
            aim_buildings: HashMap::new(),
            fx: Vec::new(),
            sounds_pending: Vec::new(),
            autosave_done: false,
            last_seen: std::cell::RefCell::new(HashMap::new()),
            toasts: Vec::new(),
            scorches: Vec::new(),
            alerts: Vec::new(),
            last_alert: None,
            alert_gate: HashMap::new(),
            fx_clock: 0.0,
            layout: std::cell::Cell::new(crate::layout::LayoutModel::default()),
            panel_model: std::cell::RefCell::new(None),
            end_stats: None,
            demo: crate::tutorial::Demo::default(),
            spectate: false,
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
        const MAX_LOAD_TICKS: u64 = oxide_kit::MAX_REPLAY_TICKS;
        anyhow::ensure!(
            total <= MAX_LOAD_TICKS,
            "replay spans {total} ticks, beyond the {MAX_LOAD_TICKS}-tick interactive load limit \
             (the headless driver replays without one)"
        );
        // Bots carry memory since 0.5 (raid flags, node blacklists), so
        // the fast-forward must let them *watch* the session back: act()
        // runs against every tick to rebuild that memory, and its outputs
        // are discarded — the recorded commands are the truth. A resumed
        // session then continues exactly as the unsaved one would have.
        let mut bots = seat_bots(&scenario);
        let mut cursor = replay.cursor();
        for _ in 0..total {
            for bot in &mut bots {
                let _ = bot.act(&state);
            }
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
        game.bots = bots;
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
        // New ticks make any earlier autosave stale.
        self.autosave_done = false;
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
        let human_commands: Vec<Command> = commands
            .iter()
            .filter(|pc| pc.player == self.human)
            .map(|pc| pc.command.clone())
            .collect();
        for bot in &mut self.bots {
            commands.extend(bot.act(&self.state));
        }
        for command in &commands {
            self.recorder
                .record(self.state.current_tick(), command.clone());
        }
        let report = self.state.tick(&commands);

        // The tutorial's evidence: what the human actually asked for
        // AND the sim accepted. A tick carrying any rejection for the
        // human grades nothing — the deliberately-illegal placement
        // the building lesson invites must not graduate it.
        let human_rejected = report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { player, .. } if *player == self.human));
        if !human_rejected {
            for command in &human_commands {
                match command {
                    Command::Train { kind, .. } => {
                        self.demo.trained = true;
                        if kind.stats().can_fight() {
                            self.demo.trained_fighter = true;
                        }
                    }
                    Command::Harvest { .. } => self.demo.harvested = true,
                    Command::Build { .. } => self.demo.built = true,
                    // The march lesson teaches attack-move specifically;
                    // a targeted attack is a different verb.
                    Command::AttackMove { .. } => self.demo.attack_moved = true,
                    _ => {}
                }
            }
        }

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
        self.aim_units
            .retain(|id, _| state.unit(UnitId(*id)).is_some());
        self.aim_buildings
            .retain(|id, _| state.building(oxide_sim::BuildingId(*id)).is_some());
        if let Some(b) = self.selection.building
            && self.state.building(b).is_none()
        {
            self.selection.building = None;
        }
    }

    /// Whether rendering should ignore fog: the debug overlay or a
    /// spectator stance (playback). Chrome decides separately.
    pub fn all_seeing(&self) -> bool {
        self.overlay || self.spectate
    }

    /// Absorbs one batch of replayed ticks for presentation: the world
    /// the engine produced plus the events it emitted on the way —
    /// bolts, deaths, aim, and sound work in playback exactly as live.
    pub fn playback_present(&mut self, state: &oxide_sim::State, events: &[Event]) {
        self.prev_pos = self
            .state
            .units()
            .iter()
            .map(|u| (u.id.0, world_vec(u.pos)))
            .collect();
        self.state = state.clone();
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
        self.spawn_fx(events);
        let state = &self.state;
        self.facing
            .retain(|id, _| state.unit(UnitId(*id)).is_some());
        self.aim_units
            .retain(|id, _| state.unit(UnitId(*id)).is_some());
        self.aim_buildings
            .retain(|id, _| state.building(oxide_sim::BuildingId(*id)).is_some());
    }

    /// Drops queued transient presentation — what a bulk jump (a seek)
    /// must not replay as a burst of noise.
    pub fn drop_presentation(&mut self) {
        self.fx.clear();
        self.sounds_pending.clear();
        self.toasts.clear();
        // Aim holds and recoil stamps are per-timeline: after a seek,
        // an id that exists at the destination must not face or flash
        // for a shot fired on the timeline we just left.
        self.aim_units.clear();
        self.aim_buildings.clear();
    }

    /// The effect clock — what aim holds and recoil age against.
    pub fn fx_time(&self) -> f32 {
        self.fx_clock
    }

    /// How far the presentation clock sits between the last executed
    /// tick and the next, 0..1 — frozen while paused. Interpolation
    /// fuel for anything that must move on sim time, not wall time.
    pub fn tick_fraction(&self) -> f32 {
        (self.accum / TICK_DT).clamp(0.0, 1.0)
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
        self.pending.push(PlayerCommand {
            player: self.human,
            command,
        });
    }

    /// Drops an order-acknowledgment ping at a world point.
    pub fn ping(&mut self, at: Vec2, kind: PingKind) {
        // An order the sim accepted deserves an answer in the ear as
        // well as the eye (the mixer rate-limits volley spam).
        self.sounds_pending.push((SoundKind::Ack, None));
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
            .find(|b| b.player == self.human && b.kind == oxide_sim::BuildingKind::Foundry)
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
    /// Raises an under-attack alert, rate-limited per 8-tile region —
    /// a running battle nags once, not once per hit.
    fn raise_alert(&mut self, world: Vec2) {
        let cell = ((world.x / 8.0) as i32, (world.y / 8.0) as i32);
        let now = self.fx_clock;
        if self
            .alert_gate
            .get(&cell)
            .is_some_and(|&last| now - last < 6.0)
        {
            return;
        }
        self.alert_gate.insert(cell, now);
        self.alerts.push((world, 0.0));
        self.last_alert = Some(world);
        self.toast("under attack");
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

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_sim::{Command, Scenario, UnitKind};

    #[test]
    fn demo_flags_read_only_the_humans_commands() {
        let mut game = Game::with_viewport(
            Scenario::skirmish(),
            macroquad::prelude::vec2(1280.0, 800.0),
        )
        .expect("skirmish builds");
        assert!(!game.demo.trained);
        let foundry = game
            .state
            .buildings()
            .iter()
            .find(|b| b.player == game.human)
            .unwrap()
            .id;
        game.issue(Command::Train {
            building: foundry,
            kind: UnitKind::Harvester,
        });
        game.do_tick();
        assert!(game.demo.trained, "the human trained");
        assert!(!game.demo.trained_fighter, "a harvester is not a fighter");
        game.issue(Command::Train {
            building: foundry,
            kind: UnitKind::Sentinel,
        });
        game.do_tick();
        assert!(game.demo.trained_fighter);
        // The opponent bot issues commands every think; none of them
        // may grade the human's homework (flags above already proved
        // the human path; run a few bot-only ticks and check the
        // unrelated flags stay cold).
        for _ in 0..20 {
            game.do_tick();
        }
        assert!(!game.demo.attack_moved);
        assert!(!game.demo.built);
    }
}
