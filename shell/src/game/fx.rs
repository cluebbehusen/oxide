//! Presentation effects: the visual vocabulary (shots, shells, bursts,
//! pings), the sound kinds, and the event-to-effect mapping that turns
//! sim reports into transient visuals and positional audio. Nothing
//! here is sim-relevant; dropping it all is always safe.

use super::{Presentation, world_vec};
use crate::seat_style::AllegianceCue;
use oxide_sim::State;

use macroquad::prelude::Vec2;
use oxide_sim::Event;

#[derive(Clone, Copy)]
pub(crate) struct CollapseBody {
    pub kind: oxide_sim::BuildingKind,
    pub tier: u8,
    pub player: oxide_sim::PlayerId,
    pub faction: oxide_sim::Faction,
    pub rotation: f32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct UnitBody {
    pub kind: oxide_sim::UnitKind,
    pub player: oxide_sim::PlayerId,
    pub faction: oxide_sim::Faction,
    pub rotation: f32,
    pub velocity: Vec2,
}

impl UnitBody {
    fn capture(
        game: &Presentation,
        state: &State,
        unit: &oxide_sim::state::Unit,
        reduced_motion: bool,
    ) -> Self {
        let kind = unit.kind;
        let rotation = if kind.has_ground_turret() || super::rotor_hull_turn_rate(kind).is_some() {
            game.draw_hull_heading(state, unit.id, 1.0)
        } else if kind.stats().turn_rate > 0
            || kind.ground_turn_rate() > 0
            || kind.stats().cruise_turn_rate > 0
            || kind.stats().turret_turn_rate > 0
        {
            game.draw_heading(unit.id, unit.weapon_heading(), 1.0)
        } else {
            game.aim_units
                .get(&unit.id.0)
                .map(|pose| pose.0)
                .or_else(|| game.facing.get(&unit.id.0).copied())
                .unwrap_or(0.0)
        };
        let rotation = if super::rotor_hull_turn_rate(kind).is_some() {
            rotation
        } else {
            rotation + game.slide_yaw(unit.id, 1.0, reduced_motion)
        };
        let velocity = game
            .prev_pos
            .get(&unit.id.0)
            .map_or(Vec2::ZERO, |previous| {
                (world_vec(unit.pos) - *previous) / super::TICK_DT
            });
        Self {
            kind,
            player: unit.player,
            faction: state.player(unit.player).faction,
            rotation,
            velocity: velocity
                .clamp_length_max(kind.stats().speed.to_num::<f32>() / super::TICK_DT),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct BuildingHit {
    pub id: oxide_sim::BuildingId,
    pub kind: oxide_sim::BuildingKind,
    pub tier: u8,
    pub faction: oxide_sim::Faction,
    pub anchor: Vec2,
    pub facts: crate::presentation_animation::BuildingAnimationFacts,
}

impl BuildingHit {
    pub(crate) fn capture(state: &State, building: &oxide_sim::Building) -> Self {
        Self {
            id: building.id,
            kind: building.kind,
            tier: building.tier,
            faction: state.player(building.player).faction,
            anchor: Vec2::new(building.anchor.x as f32, building.anchor.y as f32),
            facts: crate::presentation_animation::BuildingAnimationFacts::capture(state, building),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct UnitHit {
    pub id: oxide_sim::UnitId,
    pub body: UnitBody,
    pub frame: crate::render::UnitSpriteFrame,
    pub center: Vec2,
    pub airborne: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum HitSurface {
    Building(BuildingHit),
    Unit(UnitHit),
}

impl HitSurface {
    pub(crate) fn covers(self, at: Vec2) -> bool {
        match self {
            Self::Building(hit) => {
                let (w, h) = hit.kind.size();
                at.x >= hit.anchor.x
                    && at.y >= hit.anchor.y
                    && at.x <= hit.anchor.x + w as f32
                    && at.y <= hit.anchor.y + h as f32
            }
            Self::Unit(hit) => {
                hit.center.distance(at) <= hit.body.kind.stats().radius.to_num::<f32>()
            }
        }
    }
}

#[derive(Default)]
pub(super) struct PreviousEffects {
    buildings: Vec<(oxide_sim::BuildingId, Option<CollapseBody>, BuildingHit)>,
    shells: Vec<oxide_sim::state::Shell>,
    units: Vec<(
        oxide_sim::UnitId,
        UnitBody,
        Vec2,
        bool,
        crate::render::UnitSpriteFrame,
    )>,
    visible_crash_contacts: Vec<oxide_sim::UnitId>,
}

impl PreviousEffects {
    pub(super) fn capture(game: &Presentation, state: &State) -> Self {
        Self {
            visible_crash_contacts: state
                .aircraft_crashes()
                .iter()
                .filter(|crash| {
                    crash.arrival == state.current_tick()
                        && (crash.player == game.human
                            || game.all_seeing()
                            || state
                                .vision(game.human)
                                .visible(chassis::grid::TilePos::containing(crash.impact)))
                })
                .map(|crash| crash.unit)
                .collect(),
            buildings: state
                .buildings()
                .iter()
                .filter(|b| {
                    !b.provisional
                        && (b.player == game.human
                            || game.all_seeing()
                            || (b.tiles().any(|t| state.vision(game.human).visible(t))
                                && state.building_apparent(game.human, b)))
                })
                .map(|b| {
                    (
                        b.id,
                        b.built.then_some(CollapseBody {
                            kind: b.kind,
                            tier: b.tier,
                            player: b.player,
                            faction: state.player(b.player).faction,
                            rotation: game.aim_buildings.get(&b.id.0).map_or(0.0, |pose| pose.0),
                        }),
                        BuildingHit::capture(state, b),
                    )
                })
                .collect(),
            shells: state.shells().to_vec(),
            units: state
                .units()
                .iter()
                .filter(|unit| {
                    unit.player == game.human
                        || game.all_seeing()
                        || state.vision(game.human).visible(unit.tile())
                })
                .map(|unit| {
                    (
                        unit.id,
                        UnitBody::capture(game, state, unit, crate::render::reduced_motion()),
                        game.draw_pos(unit.id, unit.pos, 1.0),
                        unit.domain() == oxide_sim::stats::Domain::Air,
                        crate::render::UnitSpriteFrame::capture(
                            unit.kind,
                            crate::render::unit_animation(game, state, unit),
                        ),
                    )
                })
                .collect(),
        }
    }
}

pub struct Effect {
    /// What to draw.
    pub kind: EffectKind,
    /// Wall seconds alive for effects that do not ride the simulation clock.
    pub age: f32,
}

impl Effect {
    /// Age at one simulation-timeline instant. Direct-fire reports follow
    /// sim time so their rounds stay attached to authored muzzle frames at
    /// every game and replay speed. Their wall-age field is only allowed to
    /// drain a terminal battlefield after simulation time has stopped.
    pub(crate) fn age_at(&self, completed_ticks: u64, tick_fraction: f32) -> f32 {
        match self.kind {
            EffectKind::DirectShot { completed_tick, .. }
            | EffectKind::SapperDetonation { completed_tick, .. }
            | EffectKind::Impact { completed_tick, .. } => {
                let whole = completed_ticks.saturating_sub(completed_tick) as f32;
                (whole + tick_fraction.clamp(0.0, 1.0)) * super::TICK_DT + self.age
            }
            EffectKind::Falling {
                crash: Some(crash), ..
            } => {
                let whole = completed_ticks.saturating_sub(crash.started + 1) as f32;
                (whole + tick_fraction.clamp(0.0, 1.0)) * super::TICK_DT + self.age
            }
            _ => self.age,
        }
    }
}

chassis::listed_enum! {
    /// A clip the shell should play (queued by sim events, drained per frame).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum SoundKind {
        /// An attack landed somewhere you can see.
        Laser,
        /// A unit died somewhere you can see.
        UnitDeath,
        /// A building fell or a heavy airframe hit the ground.
        BuildingBoom,
        /// Your harvester delivered.
        Deposit,
        /// Your Foundry finished a unit.
        TrainDone,
        /// Menu activation.
        Click,
        /// An order was rejected.
        Denied,
        /// High-priority warning that the local player is under attack.
        Alert,
        /// The match ended in your favor.
        Victory,
        /// It did not.
        Defeat,
        /// An artillery shell landing.
        Artillery,
        /// A hostile artillery launch heard from a visible impact warning.
        ArtilleryLaunch,
        /// An order acknowledged.
        Ack,
        /// A Sentinel's compact cannon report.
        SentinelFire,
        /// A Scuttler's paired mechanical shear.
        ScuttlerFire,
        /// A Lancer's charged rail report.
        LancerFire,
        /// A Bombard's heavy artillery report.
        BombardFire,
        /// A Flakhound's paired anti-air burst.
        FlakhoundFire,
        /// A Stinger's light anti-air burst.
        StingerFire,
        /// A Buzzard's heavy strike.
        BuzzardFire,
        /// A Darter's fast strike.
        DarterFire,
        /// A Talon's interceptor burst.
        TalonFire,
        /// A Wisp's compact interceptor burst.
        WispFire,
        /// A Bastion's emplaced artillery report.
        BastionFire,
        /// A Flak Turret's paired-yoke burst.
        FlakTurretFire,
        /// The Warden's fork cannon report.
        WardenFire,
        /// The Breaker's siege mortar.
        BreakerFire,
        /// The Avalanche bank launching.
        AvalancheFire,
        /// Missile motor ignition after launcher ejection.
        RocketMotor,
        /// A missile warhead reaching its impact point.
        RocketImpact,
        /// A bomber releasing its load.
        BombRelease,
        /// A buried charge or Sapper detonating.
        DemolitionBoom,
        /// A works coming back online one rung higher.
        UpgradeDone,
    }
}

/// What an order-acknowledgment ping means (decides its color).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PingKind {
    /// Move / advance / hunt destination.
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

/// Delay between the two visible rounds of one logical flak hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlakYokeDelay {
    /// Both barrels fire together.
    None,
    /// The second yoke fires one simulation tick after the first.
    OneTick,
    /// The second yoke fires halfway through a three-tick report.
    OneAndHalfTicks,
}

impl FlakYokeDelay {
    /// Authored delay in simulation ticks.
    pub(crate) fn ticks(self) -> f32 {
        match self {
            Self::None => 0.0,
            Self::OneTick => 1.0,
            Self::OneAndHalfTicks => 1.5,
        }
    }

    /// Authored delay in seconds on the simulation timeline.
    pub(crate) fn seconds(self) -> f32 {
        self.ticks() * super::TICK_DT
    }
}

/// The visual family of a direct-fire shot, mapped from the exact
/// (shooter kind, weapon slot) the hit event names so every weapon reads
/// as itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShotStyle {
    /// A contact tool: target sparks, never a ranged projectile.
    Contact,
    /// A compact forge-bright orb with no persistent tracer.
    ForgeSpot,
    /// A short metal round, followed by a compact impact burst.
    Kinetic { heavy: bool },
    /// A short cosmetic mortar report with a rupturing contact.
    Mortar,
    /// The Lancer's brief discharge and fading rail trace.
    Rail,
    /// One logical anti-air burst, with one to three rounds from each yoke.
    FlakBurst {
        /// When the second visible yoke reports.
        yoke_delay: FlakYokeDelay,
        /// Barrels firing together on each side.
        rounds_per_yoke: u8,
    },
}

impl ShotStyle {
    /// Seconds the report stays on screen.
    pub fn life(self) -> f32 {
        match self {
            ShotStyle::Contact => 0.12,
            ShotStyle::ForgeSpot => 0.32,
            ShotStyle::Kinetic { heavy: false } => 0.18,
            ShotStyle::Kinetic { heavy: true } => 0.24,
            ShotStyle::Mortar => 0.64,
            ShotStyle::Rail => 0.24,
            ShotStyle::FlakBurst {
                yoke_delay: FlakYokeDelay::None,
                rounds_per_yoke: 1,
            } => 0.24,
            ShotStyle::FlakBurst { .. } => 0.30,
        }
    }
}

/// Which report family a unit's weapon slot fires: every slot speaks
/// through the kind's one physical barrel. None for a kind that never draws
/// a direct report.
fn unit_shot_style(kind: oxide_sim::UnitKind, _weapon: usize) -> Option<ShotStyle> {
    crate::look::unit(kind)
        .weapon
        .and_then(|weapon| weapon.shot)
        .map(|(style, _)| style)
}

fn visual_shot_origin(from: Vec2, to: Vec2, reach: f32) -> Vec2 {
    let direction = to - from;
    if direction.length_squared() <= f32::EPSILON {
        from
    } else {
        from + direction.normalize() * reach
    }
}

fn unit_muzzle_reach(kind: oxide_sim::UnitKind) -> Option<f32> {
    crate::look::unit(kind)
        .weapon
        .and_then(|weapon| weapon.shot)
        .map(|(_, reach)| reach)
}

fn unit_shot_origin(kind: oxide_sim::UnitKind, from: Vec2, to: Vec2) -> Vec2 {
    if kind.stats().contact_reach.is_some() {
        return from;
    }
    let Some(reach) = unit_muzzle_reach(kind) else {
        return from;
    };
    let mut origin = visual_shot_origin(from, to, reach);
    if kind.stats().domain == oxide_sim::stats::Domain::Air {
        origin.y -= crate::render::air_presentation(kind, 1.0).2;
    }
    origin
}

/// The report a unit's weapon makes; an unarmed kind makes none.
fn unit_fire_sound(kind: oxide_sim::UnitKind) -> Option<SoundKind> {
    crate::look::unit(kind).weapon.map(|weapon| weapon.sound)
}

fn shell_fire_sound(shooter: oxide_sim::Target) -> SoundKind {
    match shooter {
        oxide_sim::Target::Unit(_) => SoundKind::BombardFire,
        oxide_sim::Target::Building(_) => SoundKind::BastionFire,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellSoundAnchor {
    Muzzle,
    Impact,
}

/// A launch heard at the muzzle is the shooting unit's own report; only a
/// shooter without a unit pose falls back to its family's.
fn shell_launch_audio(
    shooter: oxide_sim::Target,
    shooter_kind: Option<oxide_sim::UnitKind>,
    allegiance: AllegianceCue,
    muzzle_seen: bool,
    impact_seen: bool,
) -> Option<(SoundKind, ShellSoundAnchor)> {
    if muzzle_seen || allegiance == AllegianceCue::Mine {
        let report = shooter_kind
            .and_then(unit_fire_sound)
            .unwrap_or_else(|| shell_fire_sound(shooter));
        Some((report, ShellSoundAnchor::Muzzle))
    } else if allegiance == AllegianceCue::Hostile && impact_seen {
        Some((SoundKind::ArtilleryLaunch, ShellSoundAnchor::Impact))
    } else {
        None
    }
}

/// Effect shapes.
pub enum EffectKind {
    /// A direct-fire shot, styled by the weapon family that spoke.
    DirectShot {
        /// Visual family (contact, kinetic, rail, or flak).
        style: ShotStyle,
        /// Muzzle, world coords.
        from: Vec2,
        /// Impact, world coords.
        to: Vec2,
        /// Splash radius, if this logical hit has one.
        splash: Option<f32>,
        /// Unit that issued this report, when applicable.
        attacker: Option<oxide_sim::UnitId>,
        /// Visible target geometry survives a lethal hit for this report.
        surface: Option<HitSurface>,
        /// Simulation tick immediately after the hit was reported.
        completed_tick: u64,
    },
    /// A Sapper's decisive action pose, retained after its carrier dies.
    SapperDetonation {
        /// Exact firing position.
        at: Vec2,
        /// Exact center used by the simulation's splash ring.
        blast_at: Vec2,
        /// Direction toward the contacted target.
        rotation: f32,
        /// Owning seat, for the same identity tint the live unit used.
        player: oxide_sim::PlayerId,
        /// Faction-specific base art.
        faction: oxide_sim::Faction,
        /// The source position was legitimately known when the report arrived.
        source_witnessed: bool,
        /// The impact position was legitimately known when the report arrived.
        impact_witnessed: bool,
        /// Simulation tick immediately after the hit was reported.
        completed_tick: u64,
    },
    /// An airborne casualty: heavy airframes crash; smaller ones burst in flight.
    Falling {
        at: Vec2,
        body: UnitBody,
        seed: u32,
        crash: Option<oxide_sim::state::AircraftCrash>,
        impact_witnessed: bool,
    },
    /// A retained structural silhouette collapsing into low wreckage.
    Collapse {
        at: Vec2,
        body: CollapseBody,
        seed: u32,
    },
    /// A payload-specific impact.
    Impact {
        at: Vec2,
        radius: f32,
        payload: oxide_sim::ProjectileKind,
        from: Vec2,
        surface: Option<HitSurface>,
        completed_tick: u64,
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
        /// Whether the order joined the unit's program rather than
        /// replacing it.
        queued: bool,
    },
    /// A splash detonation blooming over its radius.
    Burst {
        /// Impact center, world coords.
        at: Vec2,
        /// Splash radius, tiles.
        radius: f32,
    },
    /// A ruptured ground chassis and rigid fragments settling on the floor.
    Debris {
        /// Death point, world coords.
        at: Vec2,
        body: UnitBody,
        /// Deterministic scatter seed (the casualty's id).
        seed: u32,
    },
}

fn push_direct_report(
    effects: &mut Vec<Effect>,
    style: ShotStyle,
    from: Vec2,
    to: Vec2,
    splash: Option<f32>,
    completed_tick: u64,
    surface: Option<HitSurface>,
) -> &mut Effect {
    effects.push(Effect {
        kind: EffectKind::DirectShot {
            style,
            from,
            to,
            splash,
            completed_tick,
            surface,
            attacker: None,
        },
        age: 0.0,
    });
    effects.last_mut().expect("report was inserted")
}

fn event_target_owner(
    state: &oxide_sim::State,
    events: &[Event],
    target: oxide_sim::Target,
) -> Option<oxide_sim::PlayerId> {
    match target {
        oxide_sim::Target::Unit(id) => state.unit(id).map(|unit| unit.player).or_else(|| {
            events.iter().find_map(|event| match event {
                Event::UnitDied { unit, player, .. } if *unit == id => Some(*player),
                _ => None,
            })
        }),
        oxide_sim::Target::Building(id) => state
            .building(id)
            .map(|building| building.player)
            .or_else(|| {
                events.iter().find_map(|event| match event {
                    Event::BuildingDestroyed {
                        building, player, ..
                    } if *building == id => Some(*player),
                    _ => None,
                })
            }),
    }
}

impl Presentation {
    pub(super) fn restore_pending_crashes(&mut self, state: &State) {
        for crash in state.aircraft_crashes().to_vec() {
            self.restore_crash_effect(state, crash, false);
        }
    }

    fn restore_crash_effect(
        &mut self,
        state: &State,
        crash: oxide_sim::state::AircraftCrash,
        witnessed: bool,
    ) {
        for effect in &mut self.fx {
            if let EffectKind::Falling {
                crash: Some(existing),
                impact_witnessed,
                ..
            } = &mut effect.kind
                && existing.unit == crash.unit
            {
                *impact_witnessed |= witnessed;
                return;
            }
        }
        let visible = witnessed
            || self.all_seeing()
            || crash.player == self.human
            || state
                .vision(self.human)
                .visible(chassis::grid::TilePos::containing(crash.launch))
            || state
                .vision(self.human)
                .visible(chassis::grid::TilePos::containing(crash.impact));
        if !visible {
            return;
        }
        self.fx.push(Effect {
            kind: EffectKind::Falling {
                at: world_vec(crash.launch),
                seed: crash.unit.0,
                crash: Some(crash),
                impact_witnessed: witnessed,
                body: UnitBody {
                    kind: crash.kind,
                    player: crash.player,
                    faction: state.player(crash.player).faction,
                    rotation: f32::from(crash.heading) * std::f32::consts::TAU / 256.0
                        + std::f32::consts::FRAC_PI_2,
                    velocity: Vec2::ZERO,
                },
            },
            age: 0.0,
        });
    }

    /// Advances the effect clock and ages and prunes effects, alerts, and
    /// toasts.
    pub fn update_fx(&mut self, state: &State, dt: f32) {
        self.fx_clock += dt;
        for (_, age) in &mut self.alerts {
            *age += dt;
        }
        self.alerts.retain(|(_, age)| *age < 6.0);
        let terminal = state.result().is_some();
        for fx in &mut self.fx {
            match fx.kind {
                EffectKind::DirectShot { .. }
                | EffectKind::SapperDetonation { .. }
                | EffectKind::Impact { .. }
                | EffectKind::Falling { crash: Some(_), .. }
                    if terminal =>
                {
                    fx.age += dt;
                }
                EffectKind::DirectShot { .. }
                | EffectKind::SapperDetonation { .. }
                | EffectKind::Impact { .. }
                | EffectKind::Falling { crash: Some(_), .. } => {}
                _ => fx.age += dt,
            }
        }
        let completed_ticks = state.current_tick();
        let tick_fraction = self.tick_fraction();
        self.fx.retain(|fx| {
            fx.age_at(completed_ticks, tick_fraction)
                < match fx.kind {
                    EffectKind::DirectShot { style, .. } => style.life(),
                    EffectKind::SapperDetonation { .. } => super::TICK_DT * 2.0,
                    EffectKind::Collapse { .. } => 4.5,
                    EffectKind::Impact { .. } => 1.4,
                    EffectKind::Puff { .. } => 0.4,
                    EffectKind::Falling { .. } => 4.0,
                    EffectKind::Ping { .. } => 0.5,
                    EffectKind::Burst { .. } => 0.35,
                    EffectKind::Debris { .. } => 3.0,
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

    fn building_hit(
        &self,
        state: &State,
        target: Option<oxide_sim::Target>,
    ) -> Option<BuildingHit> {
        let oxide_sim::Target::Building(id) = target? else {
            return None;
        };
        state
            .building(id)
            .filter(|b| {
                !b.provisional
                    && (self.all_seeing()
                        || b.player == self.human
                        || b.tiles().any(|t| state.vision(self.human).visible(t))
                            && state.building_apparent(self.human, b))
            })
            .map(|b| BuildingHit::capture(state, b))
            .or_else(|| {
                self.fx_previous
                    .buildings
                    .iter()
                    .find(|(bid, _, _)| *bid == id)
                    .map(|(_, _, hit)| *hit)
            })
    }

    pub(crate) fn hit_surface(
        &self,
        state: &State,
        target: Option<oxide_sim::Target>,
    ) -> Option<HitSurface> {
        let target = target?;
        if let oxide_sim::Target::Building(_) = target {
            return self
                .building_hit(state, Some(target))
                .map(HitSurface::Building);
        }
        let oxide_sim::Target::Unit(id) = target else {
            return None;
        };
        let (body, mut center, airborne, frame) = if let Some(unit) = state.unit(id) {
            if !self.all_seeing()
                && unit.player != self.human
                && !state.vision(self.human).visible(unit.tile())
            {
                return None;
            }
            (
                UnitBody::capture(self, state, unit, crate::render::reduced_motion()),
                self.draw_pos(id, unit.pos, 1.0),
                unit.domain() == oxide_sim::stats::Domain::Air,
                crate::render::UnitSpriteFrame::capture(
                    unit.kind,
                    crate::render::unit_animation(self, state, unit),
                ),
            )
        } else {
            let (_, body, center, airborne, frame) =
                self.fx_previous.units.iter().find(|(uid, ..)| *uid == id)?;
            (*body, *center, *airborne, *frame)
        };
        if airborne {
            center.y -= crate::render::air_presentation(body.kind, 1.0).2;
        }
        Some(HitSurface::Unit(UnitHit {
            id,
            body,
            frame,
            center,
            airborne,
        }))
    }

    pub(crate) fn payload_surface(
        &self,
        state: &State,
        target: Option<oxide_sim::Target>,
        player: oxide_sim::PlayerId,
        at: Vec2,
        targets: oxide_sim::stats::DomainMask,
    ) -> Option<HitSurface> {
        let covers = |surface: HitSurface| surface.covers(at);
        let known = self
            .hit_surface(state, target)
            .filter(|surface| covers(*surface));
        if target.is_some() {
            return known;
        }
        known
            .or_else(|| {
                let mut units = state
                    .units()
                    .iter()
                    .filter(|unit| {
                        if !state.hostile(player, unit.player) || !targets.covers(unit.domain()) {
                            return false;
                        }
                        let mut center = self.draw_pos(unit.id, unit.pos, 1.0);
                        if unit.domain() == oxide_sim::stats::Domain::Air {
                            center.y -= crate::render::air_presentation(unit.kind, 1.0).2;
                        }
                        center.distance(at) <= unit.kind.stats().radius.to_num::<f32>()
                    })
                    .map(|unit| unit.id)
                    .chain(
                        self.fx_previous
                            .units
                            .iter()
                            .filter(|(id, body, _, airborne, _)| {
                                state.unit(*id).is_none()
                                    && state.hostile(player, body.player)
                                    && if *airborne {
                                        targets.air
                                    } else {
                                        targets.ground
                                    }
                            })
                            .map(|(id, ..)| *id),
                    );
                units.find_map(|id| {
                    self.hit_surface(state, Some(oxide_sim::Target::Unit(id)))
                        .filter(|surface| covers(*surface))
                })
            })
            .or_else(|| {
                if !targets.ground {
                    return None;
                }
                state
                    .buildings()
                    .iter()
                    .filter(|building| state.hostile(player, building.player))
                    .map(|building| building.id)
                    .chain(
                        self.fx_previous
                            .buildings
                            .iter()
                            .filter(|(id, body, _)| {
                                state.building(*id).is_none()
                                    && body.is_some_and(|body| state.hostile(player, body.player))
                            })
                            .map(|(id, ..)| *id),
                    )
                    .find_map(|id| {
                        self.hit_surface(state, Some(oxide_sim::Target::Building(id)))
                            .filter(|surface| covers(*surface))
                    })
            })
    }

    /// Turns a tick's events into flashes and queued clips. Explosions can be
    /// heard through fog; the camera mixer bounds their audible distance.
    /// Visual effects retain their independent sight rules.
    #[expect(clippy::too_many_lines, reason = "one arm per simulation event")]
    pub(super) fn spawn_fx(&mut self, state: &State, events: &[Event]) {
        let sees = |game: &Self, pos: chassis::fx::Vec2Fx| {
            state
                .vision(game.human)
                .visible(chassis::grid::TilePos::containing(pos))
        };
        for event in events {
            match event {
                Event::DamageTaken { player, pos } if *player == self.human => {
                    self.raise_alert(world_vec(*pos));
                }
                Event::AttackHit {
                    attacker,
                    attacker_kind,
                    weapon,
                    attacker_pos,
                    target,
                    target_pos,
                    ..
                } => {
                    // The shooter turns to its work: aim overrides
                    // movement facing for a beat, and recoil ages off
                    // the same stamp.
                    let d = world_vec(*target_pos) - world_vec(*attacker_pos);
                    if d.length_squared() > 1e-6 {
                        if let Some(target) = target {
                            self.aim_unit_targets.insert(attacker.0, *target);
                        } else {
                            self.aim_unit_targets.remove(&attacker.0);
                        }
                        self.aim_units.insert(
                            attacker.0,
                            (d.y.atan2(d.x) + std::f32::consts::FRAC_PI_2, self.fx_clock),
                        );
                    }
                    let target_owner =
                        target.and_then(|target| event_target_owner(state, events, target));
                    // The kind comes from the event because the attacker
                    // may have died later this same tick. The weapon decides
                    // the report and whether the impact blooms.
                    let sapper_owner = attacker_kind
                        .stats()
                        .demolition
                        .is_some()
                        .then(|| {
                            events.iter().find_map(|event| match event {
                                Event::UnitDied { unit, player, .. } if unit == attacker => {
                                    Some(*player)
                                }
                                _ => None,
                            })
                        })
                        .flatten();
                    let source_witnessed =
                        sees(self, *attacker_pos) || sapper_owner == Some(self.human);
                    let impact_witnessed = sees(self, *target_pos)
                        || target_owner.is_some_and(|player| !state.hostile(self.human, player));
                    let heard = source_witnessed || impact_witnessed;
                    let sound = unit_fire_sound(*attacker_kind);
                    // The burst radius comes from the weapon slot the event
                    // names, so the drawn area matches the damage the sim
                    // deals.
                    let splash = attacker_kind
                        .stats()
                        .weapons
                        .get(*weapon)
                        .and_then(|w| w.splash)
                        .map(|s| s.to_num::<f32>());
                    if let Some(sound) = sound
                        && (heard || crate::mixer::spec(sound).explosion)
                    {
                        let at = if crate::mixer::spec(sound).explosion {
                            *target_pos
                        } else if sees(self, *attacker_pos) {
                            *attacker_pos
                        } else {
                            *target_pos
                        };
                        self.sounds_pending.push((sound, Some(world_vec(at))));
                    }
                    if attacker_kind.stats().demolition.is_some() {
                        if let Some(player) = sapper_owner {
                            let direction = world_vec(*target_pos) - world_vec(*attacker_pos);
                            let rotation = if direction.length_squared() > 1e-6 {
                                direction.y.atan2(direction.x) + std::f32::consts::FRAC_PI_2
                            } else {
                                0.0
                            };
                            self.fx.push(Effect {
                                kind: EffectKind::SapperDetonation {
                                    at: world_vec(*attacker_pos),
                                    blast_at: world_vec(*target_pos),
                                    rotation,
                                    player,
                                    faction: state.player(player).faction,
                                    source_witnessed,
                                    impact_witnessed,
                                    completed_tick: state.current_tick(),
                                },
                                age: 0.0,
                            });
                        }
                    } else {
                        let surface = self.hit_surface(state, *target);
                        let mut origin = unit_shot_origin(
                            *attacker_kind,
                            world_vec(*attacker_pos),
                            world_vec(*target_pos),
                        );
                        let airborne = state
                            .unit(*attacker)
                            .map(|unit| unit.domain() == oxide_sim::stats::Domain::Air)
                            .or_else(|| {
                                self.fx_previous
                                    .units
                                    .iter()
                                    .find(|(id, ..)| id == attacker)
                                    .map(|(_, _, _, airborne, _)| *airborne)
                            })
                            .unwrap_or(
                                attacker_kind.stats().domain == oxide_sim::stats::Domain::Air,
                            );
                        if attacker_kind.stats().domain == oxide_sim::stats::Domain::Air
                            && !airborne
                        {
                            origin.y += crate::render::air_presentation(*attacker_kind, 1.0).2;
                        }
                        let Some(style) = unit_shot_style(*attacker_kind, *weapon) else {
                            continue;
                        };
                        let report = push_direct_report(
                            &mut self.fx,
                            style,
                            origin,
                            world_vec(*target_pos),
                            splash,
                            state.current_tick(),
                            surface,
                        );
                        if let EffectKind::DirectShot {
                            attacker: source, ..
                        } = &mut report.kind
                        {
                            *source = Some(*attacker);
                        }
                    }
                }
                Event::TurretFired {
                    kind,
                    tier,
                    turret,
                    turret_pos,
                    target_pos,
                    target,
                    ..
                } => {
                    if let Some(target) = target {
                        self.aim_building_targets.insert(turret.0, *target);
                    } else {
                        self.aim_building_targets.remove(&turret.0);
                    }
                    let d = world_vec(*target_pos) - world_vec(*turret_pos);
                    if d.length_squared() > 1e-6 {
                        self.aim_buildings.insert(
                            turret.0,
                            (d.y.atan2(d.x) + std::f32::consts::FRAC_PI_2, self.fx_clock),
                        );
                    }
                    // The kind comes from the event because the turret may
                    // have been destroyed the tick it fired; its shot still
                    // gets the right report and burst. Only a direct-fire
                    // defense fires this way; shells launch as their own
                    // event.
                    let Some(report) = crate::look::defense(*kind).and_then(|look| look.report)
                    else {
                        continue;
                    };
                    let splash = kind
                        .tier_stats(*tier)
                        .weapons
                        .iter()
                        .find_map(|w| w.splash)
                        .map(|s| s.to_num::<f32>());
                    if sees(self, *turret_pos) || sees(self, *target_pos) {
                        let at = if sees(self, *turret_pos) {
                            *turret_pos
                        } else {
                            *target_pos
                        };
                        self.sounds_pending
                            .push((report.sound, Some(world_vec(at))));
                    }
                    let surface = self.hit_surface(state, *target);
                    push_direct_report(
                        &mut self.fx,
                        report.shot(*tier),
                        visual_shot_origin(
                            world_vec(*turret_pos),
                            world_vec(*target_pos),
                            report.muzzle,
                        ),
                        world_vec(*target_pos),
                        splash,
                        state.current_tick(),
                        surface,
                    );
                }
                Event::BuildingCompleted {
                    building,
                    player,
                    kind,
                } if *player == self.human => {
                    // A completion at a nonzero tier is an upgrade
                    // finishing: its own cue, its own name.
                    let tier = state.building(*building).map_or(0, |b| b.tier);
                    if tier > 0 {
                        self.sounds_pending.push((SoundKind::UpgradeDone, None));
                        self.toast(format!(
                            "{} online",
                            crate::typography::entity_name(kind.tier_name(tier))
                        ));
                    } else {
                        self.sounds_pending.push((SoundKind::TrainDone, None));
                        self.toast(format!(
                            "{} online",
                            crate::typography::entity_name(kind.name())
                        ));
                    }
                }
                Event::BuildCancelled { player, refund, .. } if *player == self.human => {
                    self.toast(format!("Site salvaged (+{refund} scrap)"));
                }
                Event::UnitDied {
                    unit,
                    pos,
                    player,
                    kind,
                    grounded,
                } => {
                    if *player == self.human {
                        self.raise_alert(world_vec(*pos));
                    }
                    let prior = self
                        .fx_previous
                        .units
                        .iter()
                        .find(|(id, ..)| id == unit)
                        .map(|(_, body, at, ..)| (*body, *at));
                    let witnessed = *player == self.human
                        || sees(self, *pos)
                        || self.all_seeing()
                        || prior.is_some();
                    if !witnessed {
                        continue;
                    }
                    self.sounds_pending
                        .push((SoundKind::UnitDeath, Some(world_vec(*pos))));
                    let body = prior.map_or(
                        UnitBody {
                            kind: *kind,
                            player: *player,
                            faction: state.player(*player).faction,
                            rotation: self.facing.get(&unit.0).copied().unwrap_or(0.0),
                            velocity: Vec2::ZERO,
                        },
                        |(body, _)| body,
                    );
                    let airborne =
                        kind.stats().domain == oxide_sim::stats::Domain::Air && !grounded;
                    self.fx.push(Effect {
                        kind: if airborne {
                            EffectKind::Falling {
                                at: world_vec(*pos),
                                body,
                                seed: unit.0,
                                impact_witnessed: false,
                                crash: state
                                    .aircraft_crashes()
                                    .iter()
                                    .find(|crash| crash.unit == *unit)
                                    .copied(),
                            }
                        } else {
                            EffectKind::Debris {
                                at: prior.map_or_else(|| world_vec(*pos), |(_, at)| at),
                                body,
                                seed: unit.0,
                            }
                        },
                        age: 0.0,
                    });
                }
                Event::AircraftImpacted { crash } => {
                    let witnessed = crash.player == self.human
                        || sees(self, crash.impact)
                        || self.all_seeing()
                        || self
                            .fx_previous
                            .visible_crash_contacts
                            .contains(&crash.unit);
                    self.restore_crash_effect(state, *crash, witnessed);
                    if state
                        .map()
                        .tile(chassis::grid::TilePos::containing(crash.impact))
                        .is_some_and(|tile| tile.terrain != oxide_sim::map::Terrain::Pit)
                    {
                        self.sounds_pending
                            .push((SoundKind::BuildingBoom, Some(world_vec(crash.impact))));
                    }
                }
                Event::BuildingDestroyed {
                    building,
                    pos,
                    player,
                    ..
                } => {
                    if *player == self.human {
                        self.raise_alert(world_vec(*pos));
                    }
                    let detonated = events.iter().any(|event| {
                        matches!(event, Event::ChargeDetonated { building: charge, .. } if charge == building)
                    });
                    if !detonated {
                        self.sounds_pending
                            .push((SoundKind::BuildingBoom, Some(world_vec(*pos))));
                    }
                    let body = self
                        .fx_previous
                        .buildings
                        .iter()
                        .find(|(id, _, _)| id == building)
                        .and_then(|(_, body, _)| *body);
                    self.fx.push(Effect {
                        kind: body.map_or(
                            EffectKind::Puff {
                                at: world_vec(*pos),
                            },
                            |body| EffectKind::Collapse {
                                at: world_vec(*pos),
                                body,
                                seed: building.0,
                            },
                        ),
                        age: 0.0,
                    });
                    // A fading scorch decal; capped, oldest dropped first.
                    self.scorches.push((world_vec(*pos), 0.0));
                    if self.scorches.len() > 16 {
                        self.scorches.remove(0);
                    }
                }
                Event::UnitTrained { unit, player, .. } if *player == self.human => {
                    self.sounds_pending.push((SoundKind::TrainDone, None));
                    if let Some(u) = state.unit(*unit) {
                        self.fx.push(Effect {
                            kind: EffectKind::Ping {
                                at: world_vec(u.pos),
                                kind: PingKind::Spawn,
                                queued: false,
                            },
                            age: 0.0,
                        });
                    }
                }
                Event::ScrapDeposited { player, .. } if *player == self.human => {
                    self.sounds_pending.push((SoundKind::Deposit, None));
                }
                Event::CommandRejected { player, reason } if *player == self.human => {
                    let why = match reason {
                        oxide_sim::command::RejectReason::NotEnoughScrap => "Not enough scrap",
                        oxide_sim::command::RejectReason::WrongFaction => {
                            "That machine belongs to the other faction"
                        }
                        oxide_sim::command::RejectReason::QueueFull => "Queue is full",
                        oxide_sim::command::RejectReason::UnreachableGoal => "Can't reach that",
                        oxide_sim::command::RejectReason::InvalidTarget => "Can't target that",
                        oxide_sim::command::RejectReason::NotANode => "Nothing to mine there",
                        oxide_sim::command::RejectReason::NotYourBuilding => "Not your building",
                        oxide_sim::command::RejectReason::CannotProduce => {
                            "That factory can't make those"
                        }
                        oxide_sim::command::RejectReason::BadSite => "Can't build there",
                        oxide_sim::command::RejectReason::NoValidUnits => {
                            "Nothing selected can do that"
                        }
                        oxide_sim::command::RejectReason::OutOfBounds => "Outside the map",
                        oxide_sim::command::RejectReason::Eliminated => "You are eliminated",
                        oxide_sim::command::RejectReason::MissingPrerequisite => {
                            "Needs its tech building first"
                        }
                    };
                    self.toast(why);
                    self.sounds_pending.push((SoundKind::Denied, None));
                }
                Event::ShellLaunched {
                    shooter,
                    target,
                    player,
                    from,
                    to,
                    unit_pose,
                    ..
                } => {
                    // The gun turns toward its target: a Bastion's mount as
                    // well as a Bombard's chassis.
                    let d = world_vec(*to) - world_vec(*from);
                    if d.length_squared() > 1e-6 {
                        let angle = d.y.atan2(d.x) + std::f32::consts::FRAC_PI_2;
                        match shooter {
                            oxide_sim::Target::Unit(uid) => {
                                self.aim_units.insert(uid.0, (angle, self.fx_clock));
                            }
                            oxide_sim::Target::Building(bid) => {
                                if let Some(target) = target {
                                    self.aim_building_targets.insert(bid.0, *target);
                                } else {
                                    self.aim_building_targets.remove(&bid.0);
                                }
                                self.aim_buildings.insert(bid.0, (angle, self.fx_clock));
                            }
                        }
                    }
                    // No effect spawned: in-flight shells render from
                    // `state.shells()` directly, aged by sim ticks, so pause,
                    // replay loads, and speed changes stay consistent.
                    // Sound follows sight, except that a hostile launch
                    // whose muzzle is fogged plays anchored at its impact:
                    // the same information the sim's incoming-shell sense
                    // grants (impact tile visible), without revealing the
                    // gun.
                    if let Some((sound, anchor)) = shell_launch_audio(
                        *shooter,
                        unit_pose.as_ref().map(|pose| pose.kind),
                        AllegianceCue::of(state, self.human, *player),
                        sees(self, *from),
                        sees(self, *to),
                    ) {
                        let at = match anchor {
                            ShellSoundAnchor::Muzzle => *from,
                            ShellSoundAnchor::Impact => *to,
                        };
                        self.sounds_pending.push((sound, Some(world_vec(at))));
                    }
                }
                Event::ChargeDetonated { at, .. } => {
                    // A mine going off is loud and unmistakable whoever
                    // owned it.
                    self.fx.push(Effect {
                        kind: EffectKind::Burst {
                            at: world_vec(*at),
                            radius: oxide_sim::stats::CHARGE_BLAST_RADIUS.to_num::<f32>(),
                        },
                        age: 0.0,
                    });
                    self.sounds_pending
                        .push((SoundKind::DemolitionBoom, Some(world_vec(*at))));
                }
                Event::ShellLanded {
                    player,
                    targets,
                    at,
                    splash,
                } => {
                    // The event names no victim (a shell in flight chooses
                    // nothing), so ask the post-tick world whether the
                    // blast reached anything of ours. Survivors alert here;
                    // the dead alert through their own events.
                    let impact_sound = self.audio_timeline.landed(*player, *at);
                    let reach = splash.map_or(1.0, |r| r.to_num::<f32>().max(1.0));
                    let world = world_vec(*at);
                    let hostile_shell = state.hostile(self.human, *player);
                    // Parenthesized deliberately: && binds tighter than ||,
                    // and without the grouping the building branch would
                    // alert on the player's own artillery.
                    let own_hurt = hostile_shell
                        && (state
                            .units()
                            .iter()
                            .filter(|u| u.player == self.human && targets.covers(u.domain()))
                            .any(|u| world_vec(u.pos).distance(world) <= reach)
                            || (targets.covers(oxide_sim::stats::Domain::Ground)
                                && state
                                    .buildings()
                                    .iter()
                                    .filter(|b| b.player == self.human)
                                    .any(|b| {
                                        let c = world_vec(b.center());
                                        c.distance(world) <= reach + 1.5
                                    })));
                    if own_hurt {
                        self.raise_alert(world);
                    }
                    self.sounds_pending
                        .push((impact_sound, Some(world_vec(*at))));
                    let arrived = self
                        .fx_previous
                        .shells
                        .iter()
                        .position(|shell| {
                            shell.player == *player
                                && shell.impact == *at
                                && shell.targets == *targets
                                && shell.splash == *splash
                                && shell.arrival < state.current_tick()
                        })
                        .map(|index| {
                            let target = self
                                .projectile_releases
                                .flight(&self.fx_previous.shells, index)
                                .and_then(|flight| flight.target);
                            let shell = self.fx_previous.shells.remove(index);
                            (shell, target)
                        });
                    let payload = arrived
                        .as_ref()
                        .map_or(oxide_sim::ProjectileKind::Shell, |(shell, _)| shell.kind);
                    let from = arrived
                        .as_ref()
                        .map_or(world, |(shell, _)| world_vec(shell.launch));
                    let surface = self.payload_surface(
                        state,
                        arrived.and_then(|(_, target)| target),
                        *player,
                        world,
                        *targets,
                    );
                    self.fx.push(Effect {
                        kind: EffectKind::Impact {
                            at: world_vec(*at),
                            radius: splash.map_or(0.8, |r| r.to_num::<f32>()),
                            payload,
                            from,
                            surface,
                            completed_tick: state.current_tick(),
                        },
                        age: 0.0,
                    });
                }
                // A spectator inherits a nominal seat but owns no
                // orders; a bot's private stall feedback must not read
                // as "your order failed" in the replay viewer.
                Event::OrderStalled {
                    player,
                    pos,
                    reason,
                    ..
                } if *player == self.human && !self.spectate => {
                    // Own-state facts only: a stall reason must never reveal
                    // what fog hides.
                    self.toast(match reason {
                        oxide_sim::StallReason::NoRoute => "Can't reach that",
                        oxide_sim::StallReason::NoFiringPosition => "No ground to fire from there",
                        oxide_sim::StallReason::InsufficientScrap => "Out of scrap",
                        oxide_sim::StallReason::GroundTaken => {
                            "That ground was taken before the founder arrived"
                        }
                        oxide_sim::StallReason::TransportFull => "The transport is full",
                        oxide_sim::StallReason::NoOpenGround => "No open ground to unload there",
                        oxide_sim::StallReason::DangerHold => {
                            "Worker waiting for a safe route home"
                        }
                    });
                    self.fx.push(Effect {
                        kind: EffectKind::Ping {
                            at: world_vec(*pos),
                            kind: PingKind::Attack,
                            queued: false,
                        },
                        age: 0.0,
                    });
                }
                Event::GameOver { result } => {
                    let won = matches!(
                        result,
                        oxide_sim::GameResult::Victory { team }
                            if *team == state.player(self.human).team
                    );
                    self.sounds_pending.push((
                        if won {
                            SoundKind::Victory
                        } else {
                            SoundKind::Defeat
                        },
                        None,
                    ));
                }
                _ => {}
            }
        }
        self.refresh_defense_aim(state);
    }

    fn refresh_defense_aim(&mut self, state: &State) {
        let updates: Vec<_> = self
            .aim_building_targets
            .iter()
            .filter_map(|(&building_id, &target)| {
                if self
                    .aim_buildings
                    .get(&building_id)
                    .is_some_and(|(_, fired_at)| *fired_at == self.fx_clock)
                {
                    return None;
                }
                let building = state.building(oxide_sim::BuildingId(building_id))?;
                if building.cooldown == 0 {
                    return None;
                }
                let target_pos = match target {
                    oxide_sim::Target::Unit(id) => {
                        let unit = state.unit(id)?;
                        let visible = self.all_seeing()
                            || !state.hostile(self.human, unit.player)
                            || state.vision(self.human).visible(unit.tile());
                        visible.then(|| world_vec(unit.pos))?
                    }
                    oxide_sim::Target::Building(id) => {
                        let target = state.building(id)?;
                        let visible = self.all_seeing()
                            || !state.hostile(self.human, target.player)
                            || target
                                .tiles()
                                .any(|tile| state.vision(self.human).visible(tile));
                        visible.then(|| world_vec(target.center()))?
                    }
                };
                let from = world_vec(building.center());
                let delta = target_pos - from;
                (delta.length_squared() > 1e-6).then(|| {
                    (
                        building_id,
                        delta.y.atan2(delta.x) + std::f32::consts::FRAC_PI_2,
                    )
                })
            })
            .collect();
        for (building_id, angle) in updates {
            self.aim_buildings
                .entry(building_id)
                .and_modify(|aim| aim.0 = angle)
                .or_insert((angle, self.fx_clock));
        }
    }
}

#[cfg(test)]
mod tests;
