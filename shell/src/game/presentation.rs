//! Presentation shared by live play and replay playback. World state is borrowed.
use super::{
    Effect, PingKind, SoundKind, TICK_DT, Toast, angle_delta, fx, projectiles,
    rotor_hull_turn_rate, world_vec,
};
use super::{EffectKind, Selection};
use crate::camera::Camera;
use macroquad::prelude::{Vec2, vec2};
use oxide_sim::{
    Building, Event, PlayerCommand, PlayerId, Scenario, State, Target, UnitId, UnitKind,
};
use std::collections::HashMap;

pub struct Presentation {
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
    /// Compass headings at the previous tick, for angular interpolation.
    pub prev_heading: HashMap<u32, u8>,
    /// Sprite rotation per unit (radians; 0 = up).
    pub facing: HashMap<u32, f32>,
    pub(super) hull_heading: HashMap<u32, (f32, f32)>,
    /// Combat aim overrides: unit id -> (angle, fx-clock stamp). A shot
    /// turns the shooter toward its victim and holds briefly; movement
    /// facing resumes when the hold expires. Presentation only.
    pub aim_units: HashMap<u32, (f32, f32)>,
    pub(crate) aim_unit_targets: HashMap<u32, Target>,
    /// Same for buildings (turret mounts track their last victim).
    pub aim_buildings: HashMap<u32, (f32, f32)>,
    /// The live victims defense mounts follow between reports. The last
    /// legal angle remains in `aim_buildings` after a target disappears.
    pub(crate) aim_building_targets: HashMap<u32, Target>,
    /// Action-driven authored sprite state. This remembers only transient
    /// output events; clearing it never changes simulation truth.
    pub(crate) animations: crate::presentation_animation::AnimationController,
    pub(crate) track_motion: HashMap<u32, crate::track_motion::TrackMotion>,
    /// Eased collision slides, kept only while a body is drawn off its
    /// simulation pose.
    pub(crate) slide_motion: HashMap<u32, crate::slide_motion::SlideMotion>,
    pub(crate) projectile_releases: projectiles::ProjectileReleases,
    pub(super) fx_previous: fx::PreviousEffects,
    /// Live effects.
    pub fx: Vec<Effect>,
    /// Retains projectile identity across the impact tick.
    pub(crate) audio_timeline: crate::audio_timeline::AudioTimeline,
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
    pub(super) alert_gate: HashMap<(i32, i32), f32>,
    /// Presentation clock: seconds of fx time since session start.
    pub(super) fx_clock: f32,
    /// When each remembered tile (ghost anchors, scrap, wrecks) was
    /// last actually seen, on the fx clock — presentation state behind
    /// the staleness ramp. A `RefCell` because drawing borrows presentation.
    pub last_seen: std::cell::RefCell<HashMap<(i32, i32), f32>>,
    /// The minimap's cached terrain-and-fog texture layer. Presentation
    /// only, lazily created by the first minimap draw (headless sessions
    /// never touch the GPU). A `RefCell` because drawing borrows presentation.
    pub minimap_layer: std::cell::RefCell<Option<crate::render::MinimapLayer>>,
    pub boundary_fog: crate::boundary_fog::BoundaryFog,
    /// The chrome geometry the renderer computed last frame — the one
    /// model hit-testing reads, so drawn and clickable can never
    /// disagree. A `Cell` because drawing borrows presentation.
    pub layout: std::cell::Cell<crate::layout::LayoutModel>,
    /// The frame's command panel, built once in draw_hud and read by
    /// the tooltip pass — building it twice per frame was pure waste.
    pub panel_model: std::cell::RefCell<Option<crate::panel::Panel>>,
    /// Whether the surrender overlay (banner + concede stats + the
    /// Esc-to-menu exit) is up. Presentation only; opening the pause
    /// menu dismisses it so spectating the ally stays unobstructed.
    pub conceded_banner: bool,
    /// Fog-free viewing without the debug chrome — the playback
    /// viewer's stance. `overlay` remains the developer's F1 (grid,
    /// ids, camera internals) and implies this.
    pub spectate: bool,
    pub(super) accum: f32,
}

/// One immutable world paired with its presentation and pending human commands.
#[derive(Clone, Copy)]
pub(crate) struct Scene<'a> {
    pub state: &'a State,
    pub scenario: &'a Scenario,
    pub pending: &'a [PlayerCommand],
    pub presentation: &'a Presentation,
    pub seat_styles: crate::seat_style::SeatStyles,
}
impl<'a> Scene<'a> {
    pub fn new(
        state: &'a State,
        scenario: &'a Scenario,
        pending: &'a [PlayerCommand],
        presentation: &'a Presentation,
    ) -> Self {
        Self {
            state,
            scenario,
            pending,
            presentation,
            seat_styles: crate::seat_style::SeatStyles::new(
                state,
                presentation.human,
                crate::render::colorblind(),
            ),
        }
    }

    /// Whether the current unit selection is the human's to command.
    /// Empty selections read as own (nothing to gate); a foreign
    /// selection is read-only everywhere a verb would act.
    pub fn selection_commandable(&self) -> bool {
        self.presentation
            .selection
            .units
            .first()
            .and_then(|id| self.state.unit(*id))
            .is_none_or(|u| u.player == self.presentation.human)
    }
    /// The human's first Foundry (hotkey target, camera home).
    pub fn home_foundry(&self) -> Option<&'a Building> {
        self.state.buildings().iter().find(|b| {
            b.player == self.presentation.human
                && !b.provisional
                && b.kind == oxide_sim::BuildingKind::Foundry
        })
    }
    /// The local player's fog view (what rendering and targeting honor).
    pub fn my_vision(&self) -> &oxide_sim::Vision {
        self.state.vision(self.presentation.human)
    }
    pub(crate) fn draw_hull_heading(&self, id: UnitId, alpha: f32) -> f32 {
        self.presentation.draw_hull_heading(self.state, id, alpha)
    }
}
impl Presentation {
    pub(crate) fn new(state: &State, human: PlayerId, viewport: Vec2) -> Self {
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
        let boundary_fog = crate::boundary_fog::BoundaryFog::new(state, human);
        Self {
            human,
            camera,
            selection: Selection::default(),
            paused: false,
            speed: 1.0,
            overlay: false,
            prev_pos: HashMap::new(),
            prev_heading: HashMap::new(),
            facing: HashMap::new(),
            hull_heading: HashMap::new(),
            aim_units: HashMap::new(),
            aim_unit_targets: HashMap::new(),
            aim_buildings: HashMap::new(),
            aim_building_targets: HashMap::new(),
            animations: crate::presentation_animation::AnimationController::default(),
            track_motion: HashMap::new(),
            slide_motion: HashMap::new(),
            projectile_releases: projectiles::ProjectileReleases::default(),
            fx_previous: fx::PreviousEffects::default(),
            fx: Vec::new(),
            audio_timeline: crate::audio_timeline::AudioTimeline::default(),
            sounds_pending: Vec::new(),
            last_seen: std::cell::RefCell::new(HashMap::new()),
            minimap_layer: std::cell::RefCell::new(None),
            boundary_fog,
            toasts: Vec::new(),
            scorches: Vec::new(),
            alerts: Vec::new(),
            last_alert: None,
            alert_gate: HashMap::new(),
            fx_clock: 0.0,
            layout: std::cell::Cell::new(crate::layout::LayoutModel::default()),
            panel_model: std::cell::RefCell::new(None),
            conceded_banner: false,
            spectate: false,
            accum: 0.0,
        }
    }
    /// Whether rendering should ignore fog: the debug overlay or a
    /// spectator stance (playback). Chrome decides separately.
    pub fn all_seeing(&self) -> bool {
        self.overlay || self.spectate
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

    /// Mirrors an externally owned replay clock into this render vehicle.
    /// Playback advances through its own engine, so this changes only the
    /// interpolation and authored-animation fraction, never simulation time.
    pub(crate) fn sync_external_tick_fraction(&mut self, fraction: f32) {
        self.accum = fraction.clamp(0.0, 1.0) * TICK_DT;
    }

    /// Interpolation factor for rendering between ticks.
    pub fn render_alpha(&self) -> f32 {
        if self.paused {
            1.0
        } else {
            (self.accum / TICK_DT).clamp(0.0, 1.0)
        }
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
        let text = text.into();
        self.toasts.retain(|toast| toast.text != text);
        self.toasts.push(Toast { text, age: 0.0 });
        if self.toasts.len() > 3 {
            self.toasts.remove(0);
        }
    }

    /// Ages and prunes effects and toasts.
    /// Raises an under-attack alert, rate-limited per 8-tile region —
    /// a running battle nags once, not once per hit.
    pub(super) fn raise_alert(&mut self, world: Vec2) {
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
        self.sounds_pending.push((SoundKind::Alert, None));
    }

    /// Interpolated draw position for a unit, trailing a collision slide by
    /// its eased lag.
    pub fn draw_pos(&self, id: UnitId, current: chassis::fx::Vec2Fx, alpha: f32) -> Vec2 {
        let now = world_vec(current);
        let lag = self
            .slide_motion
            .get(&id.0)
            .map_or(Vec2::ZERO, |slide| slide.lag(alpha));
        match self.prev_pos.get(&id.0) {
            Some(prev) => prev.lerp(now, alpha) - lag,
            None => now - lag,
        }
    }

    /// Drawn hull lean toward a collision slide, added to the body's rotation.
    pub(crate) fn slide_yaw(&self, id: UnitId, alpha: f32, reduced_motion: bool) -> f32 {
        if reduced_motion {
            return 0.0;
        }
        self.slide_motion
            .get(&id.0)
            .map_or(0.0, |slide| slide.yaw(alpha))
    }

    pub fn draw_heading(&self, id: UnitId, current: u8, alpha: f32) -> f32 {
        let previous = self.prev_heading.get(&id.0).copied().unwrap_or(current);
        let delta = current.wrapping_sub(previous).cast_signed();
        (f32::from(previous) + f32::from(delta) * alpha.clamp(0.0, 1.0)) * std::f32::consts::TAU
            / 256.0
            + std::f32::consts::FRAC_PI_2
    }

    pub(crate) fn chassis_turning(&self, unit: &oxide_sim::state::Unit) -> bool {
        if unit.kind.ground_turn_rate() == 0 {
            return false;
        }
        if unit.kind.has_ground_turret() {
            self.hull_heading
                .get(&unit.id.0)
                .is_some_and(|(previous, current)| angle_delta(*previous, *current).abs() > 1e-6)
        } else {
            self.prev_heading
                .get(&unit.id.0)
                .is_some_and(|previous| *previous != unit.heading)
        }
    }

    /// Drops queued transient presentation — what a bulk jump (a seek)
    /// must not replay as a burst of noise.
    pub fn drop_presentation(&mut self, state: &State) {
        self.fx.clear();
        self.restore_pending_crashes(state);
        self.sounds_pending.clear();
        self.toasts.clear();
        // Aim holds and recoil stamps are per-timeline: after a seek,
        // an id that exists at the destination must not face or flash
        // for a shot fired on the timeline we just left.
        self.aim_units.clear();
        self.aim_unit_targets.clear();
        self.hull_heading.clear();
        self.aim_buildings.clear();
        self.aim_building_targets.clear();
        self.animations.reset_transients();
        self.audio_timeline.clear();
        self.track_motion.clear();
        self.slide_motion.clear();
    }

    /// Sprite rotation for this tick: a heading-first airframe faces where
    /// the simulation says it does, parked or flying, and everything else
    /// faces the way it last moved. Live ticks, playback, and seeks all
    /// agree through this one rule.
    pub(super) fn refresh_facing(&mut self, state: &State, movement: &[oxide_sim::GroundMotion]) {
        let tick = state.current_tick();
        self.observe_slides(state, movement);
        self.track_motion
            .retain(|id, _| state.unit(UnitId(*id)).is_some());
        for unit in state
            .units()
            .iter()
            .filter(|unit| crate::render::tracks::supported(unit.kind))
        {
            let heading = f32::from(unit.heading) * std::f32::consts::TAU / 256.0
                + self
                    .slide_motion
                    .get(&unit.id.0)
                    .map_or(0.0, |slide| slide.current_yaw());
            self.track_motion
                .entry(unit.id.0)
                .or_insert_with(|| crate::track_motion::TrackMotion::new(tick, heading))
                .observe(
                    tick,
                    heading,
                    crate::render::tracks::gauge(
                        unit.kind,
                        crate::render::unit_draw_scale(unit.kind),
                    ),
                    // A shove along the hull rolls the belts like driving
                    // does; the part across the hull is a skid.
                    movement
                        .binary_search_by_key(&unit.id, |motion| motion.unit)
                        .ok()
                        .map_or(Vec2::ZERO, |index| {
                            world_vec(movement[index].propulsion + movement[index].correction)
                        }),
                );
        }

        self.aim_unit_targets
            .retain(|id, _| state.unit(UnitId(*id)).is_some());
        self.hull_heading
            .retain(|id, _| state.unit(UnitId(*id)).is_some());
        for unit in state.units() {
            if unit.kind.stats().turn_rate > 0
                || unit.kind.ground_turn_rate() > 0
                || unit.kind.cruise_turn_rate() > 0
            {
                let angle = f32::from(unit.heading) * std::f32::consts::TAU / 256.0;
                self.facing
                    .insert(unit.id.0, angle + std::f32::consts::FRAC_PI_2);
                if unit.kind.has_ground_turret() {
                    let target = angle + std::f32::consts::FRAC_PI_2;
                    let entry = self
                        .hull_heading
                        .entry(unit.id.0)
                        .or_insert((target, target));
                    *entry = (entry.1, target);
                }
                continue;
            }
            let now = world_vec(unit.pos);
            let mut moving = false;
            if let Some(prev) = self.prev_pos.get(&unit.id.0) {
                let delta = now - *prev;
                if delta.length_squared() > 1e-6 {
                    moving = true;
                    self.facing.insert(
                        unit.id.0,
                        delta.y.atan2(delta.x) + std::f32::consts::FRAC_PI_2,
                    );
                }
            }
            if let Some(turn) = rotor_hull_turn_rate(unit.kind) {
                let movement_facing = self.facing.get(&unit.id.0).copied().unwrap_or(0.0);
                let target = if unit.kind == UnitKind::Wisp && !moving {
                    self.aim_units
                        .get(&unit.id.0)
                        .filter(|(_, at)| self.fx_time() - at < 1.2)
                        .map_or(movement_facing, |(angle, _)| *angle)
                } else {
                    movement_facing
                };
                let (previous, current) = self.hull_heading.entry(unit.id.0).or_insert((0.0, 0.0));
                *previous = *current;
                *current += angle_delta(*current, target).clamp(-turn, turn);
            }
        }
    }

    /// Whether a body's drawn hull may lean away from its simulation heading.
    /// A fixed weapon's hull is its aim and a body at work faces its work;
    /// neither may be drawn pointing anywhere else.
    fn slide_lean_allowed(&self, unit: &oxide_sim::state::Unit, propulsion: Vec2) -> bool {
        let aiming = !unit.kind.has_ground_turret()
            && (unit.brace_ticks > 0
                || self
                    .aim_units
                    .get(&unit.id.0)
                    .is_some_and(|(_, at)| self.fx_clock - at < 1.2));
        !aiming && (propulsion != Vec2::ZERO || matches!(unit.order, oxide_sim::Order::Idle))
    }

    /// Eases this tick's collision slides. A body keeps an entry only while
    /// it is drawn off its simulation pose.
    fn observe_slides(&mut self, state: &State, movement: &[oxide_sim::GroundMotion]) {
        let tick = state.current_tick();
        let mut slides = std::mem::take(&mut self.slide_motion);
        slides.retain(|id, _| state.unit(UnitId(*id)).is_some());
        for motion in movement {
            // Newborns have no interpolation origin. Start their slide history
            // next tick so the first pose and the following tick meet.
            if !self.prev_pos.contains_key(&motion.unit.0) {
                continue;
            }
            let sliding = motion.correction != chassis::fx::Vec2Fx::ZERO;
            if !sliding && !slides.contains_key(&motion.unit.0) {
                continue;
            }
            let Some(unit) = state.unit(motion.unit) else {
                continue;
            };
            let propulsion = world_vec(motion.propulsion);
            slides
                .entry(unit.id.0)
                .or_insert_with(|| crate::slide_motion::SlideMotion::new(tick.wrapping_sub(1)))
                .observe(
                    tick,
                    f32::from(unit.heading) * std::f32::consts::TAU / 256.0,
                    propulsion,
                    world_vec(motion.correction),
                    unit.kind.stats().speed.to_num::<f32>(),
                    self.slide_lean_allowed(unit, propulsion),
                );
        }
        // A body at rest has no motion record; its lag still has to release.
        for (id, slide) in &mut slides {
            let unit = state.unit(UnitId(*id)).expect("retained live unit");
            slide.observe(
                tick,
                0.0,
                Vec2::ZERO,
                Vec2::ZERO,
                1.0,
                self.slide_lean_allowed(unit, Vec2::ZERO),
            );
        }
        slides.retain(|_, slide| !slide.settled());
        self.slide_motion = slides;
    }

    /// Records what the next tick's presentation needs to know about
    /// this one: every body's position for interpolation, and which
    /// airframes were parked for the death effect's fall-or-scatter
    /// choice.
    pub(crate) fn remember_previous_tick(&mut self, state: &State) {
        self.audio_timeline.remember_arrivals(state);
        self.fx_previous = fx::PreviousEffects::capture(self, state);
        self.prev_heading = state
            .units()
            .iter()
            .map(|unit| (unit.id.0, unit.weapon_heading()))
            .collect();
        self.prev_pos = state
            .units()
            .iter()
            .map(|unit| (unit.id.0, world_vec(unit.pos)))
            .collect();
    }

    /// Advances presentation from ordinary frame time while the match runs.
    /// Driven presentation steps use [`Self::update_fx`] directly because
    /// they represent sim time even when the wall clock is paused.
    pub fn update_wall_clock_fx(&mut self, state: &State, dt: f32) {
        if !self.paused {
            self.update_fx(state, dt);
        }
    }

    pub(crate) fn draw_hull_heading(&self, state: &State, id: UnitId, alpha: f32) -> f32 {
        self.hull_heading.get(&id.0).map_or_else(
            || {
                state.unit(id).map_or(0.0, |unit| {
                    if rotor_hull_turn_rate(unit.kind).is_some() {
                        return self.facing.get(&id.0).copied().unwrap_or(0.0);
                    }
                    f32::from(unit.heading) * std::f32::consts::TAU / 256.0
                        + std::f32::consts::FRAC_PI_2
                })
            },
            |(previous, current)| {
                previous + angle_delta(*previous, *current) * alpha.clamp(0.0, 1.0)
            },
        )
    }

    /// Updates shared presentation after one live or replay tick.
    pub(crate) fn observe_tick(
        &mut self,
        state: &oxide_sim::State,
        events: &[Event],
        movement: &[oxide_sim::GroundMotion],
    ) {
        self.projectile_releases.observe(state, events);
        self.animations.observe_events(state.current_tick(), events);
        self.spawn_fx(state, events);
        self.refresh_facing(state, movement);

        self.facing
            .retain(|id, _| state.unit(UnitId(*id)).is_some());
        self.aim_units
            .retain(|id, _| state.unit(UnitId(*id)).is_some());
        self.aim_buildings
            .retain(|id, _| state.building(oxide_sim::BuildingId(*id)).is_some());
        self.aim_building_targets.retain(|id, target| {
            state.building(oxide_sim::BuildingId(*id)).is_some()
                && match target {
                    Target::Unit(id) => state.unit(*id).is_some(),
                    Target::Building(id) => state.building(*id).is_some(),
                }
        });
        self.animations.retain_live(state);
    }

    /// Resets presentation after a seek or replay rebuild and establishes that
    /// destination as both interpolation endpoints. Timeline-local facing,
    /// aim, reports, and effects cannot survive across the jump.
    pub(crate) fn reset_after_jump(&mut self, state: &State) {
        self.boundary_fog = crate::boundary_fog::BoundaryFog::new(state, self.human);
        self.projectile_releases = projectiles::ProjectileReleases::default();
        self.drop_presentation(state);
        self.remember_previous_tick(state);
        self.facing.clear();
        // Nothing has moved across a jump, but a heading-first airframe
        // still has a heading to show, parked or flying.
        self.refresh_facing(state, &[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Game;
    use crate::slide_motion::SlideMotion;
    use oxide_sim::Command;

    fn production_game() -> Game {
        let mut scenario = Scenario::skirmish();
        for player in &mut scenario.players {
            player.bot_config = None;
        }
        scenario.units.clear();
        Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap()
    }

    #[test]
    fn newborn_collision_waits_for_an_interpolation_origin() {
        let mut game = production_game();
        let foundry = game
            .state
            .buildings()
            .iter()
            .find(|b| b.player == game.presentation.human)
            .unwrap()
            .id;
        for _ in 0..2 {
            game.issue(Command::Train {
                building: foundry,
                kind: UnitKind::Harvester,
            });
        }
        for _ in 0..600 {
            let report = game.do_tick();
            for event in report.events {
                let Event::UnitTrained { unit: id, .. } = event else {
                    continue;
                };
                if !report.movement.iter().any(|motion| {
                    motion.unit == id && motion.correction != chassis::fx::Vec2Fx::ZERO
                }) {
                    continue;
                }
                let unit = game.state.unit(id).unwrap();
                let born_at = world_vec(unit.pos);
                assert!(!game.presentation.prev_pos.contains_key(&id.0));
                assert!(!game.presentation.slide_motion.contains_key(&id.0));
                for alpha in [0.0, 0.5, 1.0] {
                    assert_eq!(game.presentation.draw_pos(id, unit.pos, alpha), born_at);
                }
                game.do_tick();
                let unit = game.state.unit(id).unwrap();
                assert_eq!(game.presentation.draw_pos(id, unit.pos, 0.0), born_at);
                assert!(game.presentation.slide_motion.contains_key(&id.0));
                return;
            }
        }
        panic!("production never spawned a colliding unit");
    }

    #[test]
    fn a_stationary_fixed_weapon_clears_lean_when_aiming_but_idle_lean_eases() {
        for aiming in [false, true] {
            let mut scenario = Scenario::skirmish();
            scenario.units = vec![oxide_sim::scenario::UnitSpec {
                player: 0,
                kind: UnitKind::Bombard,
                x: 10,
                y: 10,
            }];
            let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
            let id = game.state.units()[0].id;
            let tick = game.state.current_tick();
            let mut slide = SlideMotion::new(tick.wrapping_sub(2));
            slide.observe(
                tick.wrapping_sub(1),
                0.0,
                vec2(0.11, 0.0),
                vec2(0.0, 0.11),
                0.11,
                true,
            );
            game.presentation.slide_motion.insert(id.0, slide);
            if aiming {
                game.presentation
                    .aim_units
                    .insert(id.0, (0.0, game.presentation.fx_clock));
            }
            game.presentation.observe_slides(&game.state, &[]);
            for alpha in [0.0, 0.5] {
                let yaw = game.presentation.slide_yaw(id, alpha, false);
                if aiming {
                    assert_eq!(yaw, 0.0);
                } else {
                    assert!(yaw > 0.0);
                }
            }
            assert_eq!(game.presentation.slide_yaw(id, 1.0, false), 0.0);
        }
    }

    #[test]
    fn reduced_motion_suppresses_existing_lean_without_discarding_position_easing() {
        let mut game = production_game();
        let id = UnitId(99);
        let mut slide = SlideMotion::new(0);
        slide.observe(1, 0.0, vec2(0.11, 0.0), vec2(0.0, 0.11), 0.11, true);
        game.presentation.slide_motion.insert(id.0, slide);
        for alpha in [0.5, 1.0] {
            assert!(game.presentation.slide_yaw(id, alpha, false) > 0.0);
            assert_eq!(game.presentation.slide_yaw(id, alpha, true), 0.0);
            assert!(game.presentation.slide_motion[&id.0].lag(alpha).length() > 0.0);
        }
        assert!(game.presentation.slide_yaw(id, 1.0, false) > 0.0);
    }
}
