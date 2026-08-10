//! Presentation effects: the visual vocabulary (shots, shells, bursts,
//! pings), the sound kinds, and the event-to-effect mapping that turns
//! sim reports into transient visuals and positional audio. Nothing
//! here is sim-relevant; dropping it all is always safe.

use super::{Game, world_vec};

use macroquad::prelude::Vec2;
use oxide_sim::Event;

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
            EffectKind::DirectShot { completed_tick, .. } => {
                let whole = completed_ticks.saturating_sub(completed_tick) as f32;
                (whole + tick_fraction.clamp(0.0, 1.0)) * super::TICK_DT + self.age
            }
            _ => self.age,
        }
    }
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
    /// A bomber releasing its load.
    BombRelease,
    /// A buried charge or Sapper detonating.
    DemolitionBoom,
    /// A works coming back online one rung higher.
    UpgradeDone,
}

/// What an order-acknowledgment ping means (decides its color).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PingKind {
    /// Move / advance / attack-move destination.
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

/// The visual family of a direct-fire shot — mapped from the exact
/// (shooter kind, weapon slot) the hit event names, so every weapon
/// reads as itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShotStyle {
    /// A contact tool: target sparks, never a ranged projectile.
    Contact,
    /// The approved compact forge-bright orb with no persistent tracer.
    ForgeSpot,
    /// The Lancer's rail: heavy, bright, lingering.
    Rail,
    /// One logical anti-air attack shown as two physical rounds.
    FlakBurst {
        /// When the second visible yoke reports.
        yoke_delay: FlakYokeDelay,
    },
}

impl ShotStyle {
    /// Seconds the report stays on screen.
    pub fn life(self) -> f32 {
        match self {
            ShotStyle::Contact => 0.12,
            ShotStyle::ForgeSpot => 0.20,
            ShotStyle::Rail => 0.24,
            ShotStyle::FlakBurst {
                yoke_delay: FlakYokeDelay::None,
            } => 0.24,
            ShotStyle::FlakBurst { .. } => 0.30,
        }
    }
}

/// Which report family a unit's weapon slot fires.
fn unit_shot_style(kind: oxide_sim::UnitKind, weapon: usize) -> ShotStyle {
    use oxide_sim::UnitKind;
    match (kind, weapon) {
        (UnitKind::Scuttler, _) => ShotStyle::Contact,
        (UnitKind::Lancer, _) => ShotStyle::Rail,
        (UnitKind::Flakhound, _) => ShotStyle::FlakBurst {
            yoke_delay: FlakYokeDelay::OneTick,
        },
        (UnitKind::Stinger, _) => ShotStyle::FlakBurst {
            yoke_delay: FlakYokeDelay::None,
        },
        _ => ShotStyle::ForgeSpot,
    }
}

fn defense_shot_style(kind: oxide_sim::BuildingKind) -> ShotStyle {
    debug_assert!(
        kind.base_stats()
            .weapons
            .iter()
            .all(|weapon| !weapon.projectile),
        "real shell weapons must arrive through ShellLaunched"
    );
    match kind {
        oxide_sim::BuildingKind::FlakTurret => ShotStyle::FlakBurst {
            yoke_delay: FlakYokeDelay::OneAndHalfTicks,
        },
        _ => ShotStyle::ForgeSpot,
    }
}

fn visual_shot_origin(from: Vec2, to: Vec2, reach: f32) -> Vec2 {
    let direction = to - from;
    if direction.length_squared() <= f32::EPSILON {
        from
    } else {
        from + direction.normalize() * reach
    }
}

fn unit_muzzle_reach(kind: oxide_sim::UnitKind) -> f32 {
    match kind {
        // The Quad-Fan's forward gun extends well beyond its central hull.
        oxide_sim::UnitKind::Buzzard => 0.44,
        _ if kind.stats().domain == oxide_sim::stats::Domain::Ground => 0.38,
        _ => 0.32,
    }
}

fn defense_muzzle_reach(kind: oxide_sim::BuildingKind) -> f32 {
    match kind {
        oxide_sim::BuildingKind::Bastion => kind.base_stats().size.0 as f32 * 0.49,
        oxide_sim::BuildingKind::FlakTurret => 0.47,
        _ => 0.44,
    }
}

fn unit_fire_sound(kind: oxide_sim::UnitKind) -> SoundKind {
    use oxide_sim::UnitKind;
    match kind {
        UnitKind::Sentinel => SoundKind::SentinelFire,
        UnitKind::Scuttler => SoundKind::ScuttlerFire,
        UnitKind::Lancer => SoundKind::LancerFire,
        UnitKind::Bombard => SoundKind::BombardFire,
        UnitKind::Flakhound => SoundKind::FlakhoundFire,
        UnitKind::Stinger => SoundKind::StingerFire,
        UnitKind::Buzzard => SoundKind::BuzzardFire,
        UnitKind::Darter => SoundKind::DarterFire,
        UnitKind::Talon => SoundKind::TalonFire,
        UnitKind::Wisp => SoundKind::WispFire,
        UnitKind::Warden => SoundKind::WardenFire,
        // Interceptors share the air-superiority zap family on purpose.
        UnitKind::Shrike => SoundKind::TalonFire,
        UnitKind::Sylph => SoundKind::WispFire,
        UnitKind::Condor | UnitKind::Moth => SoundKind::BombRelease,
        UnitKind::Avalanche => SoundKind::AvalancheFire,
        UnitKind::Breaker => SoundKind::BreakerFire,
        UnitKind::Tender
        | UnitKind::Excavator
        | UnitKind::Kestrel
        | UnitKind::Gnat
        | UnitKind::Skyhook => SoundKind::Laser,
        UnitKind::Sapper => SoundKind::DemolitionBoom,
        UnitKind::Harvester => SoundKind::Laser,
    }
}

fn defense_fire_sound(kind: oxide_sim::BuildingKind) -> SoundKind {
    match kind {
        oxide_sim::BuildingKind::Bastion => SoundKind::BastionFire,
        oxide_sim::BuildingKind::FlakTurret => SoundKind::FlakTurretFire,
        _ => SoundKind::Laser,
    }
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

fn shell_launch_audio(
    shooter: oxide_sim::Target,
    own: bool,
    hostile: bool,
    muzzle_seen: bool,
    impact_seen: bool,
) -> Option<(SoundKind, ShellSoundAnchor)> {
    if muzzle_seen || own {
        Some((shell_fire_sound(shooter), ShellSoundAnchor::Muzzle))
    } else if hostile && impact_seen {
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
        /// Simulation tick immediately after the hit was reported.
        completed_tick: u64,
    },
    /// A downed flyer: the sprite drops, spins, and shrinks out.
    Falling {
        /// Where it was hit, world coords.
        at: Vec2,
        /// What fell.
        unit: oxide_sim::UnitKind,
        /// Whose colors it wears.
        faction: oxide_sim::Faction,
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
    /// A splash detonation blooming over its radius.
    Burst {
        /// Impact center, world coords.
        at: Vec2,
        /// Splash radius, tiles.
        radius: f32,
    },
    /// Hull shards scattering from a ground kill. The renderer derives
    /// each shard's arc from the seed, so playback matches live.
    Debris {
        /// Death point, world coords.
        at: Vec2,
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
) {
    effects.push(Effect {
        kind: EffectKind::DirectShot {
            style,
            from,
            to,
            splash,
            completed_tick,
        },
        age: 0.0,
    });
}

impl Game {
    pub fn update_fx(&mut self, dt: f32) {
        self.fx_clock += dt;
        for (_, age) in &mut self.alerts {
            *age += dt;
        }
        self.alerts.retain(|(_, age)| *age < 6.0);
        let terminal = self.state.result().is_some();
        for fx in &mut self.fx {
            match fx.kind {
                EffectKind::DirectShot { .. } if terminal => fx.age += dt,
                EffectKind::DirectShot { .. } => {}
                _ => fx.age += dt,
            }
        }
        let completed_ticks = self.state.current_tick();
        let tick_fraction = self.tick_fraction();
        self.fx.retain(|fx| {
            fx.age_at(completed_ticks, tick_fraction)
                < match fx.kind {
                    EffectKind::DirectShot { style, .. } => style.life(),
                    EffectKind::Puff { .. } => 0.4,
                    EffectKind::Falling { .. } => 0.7,
                    EffectKind::Ping { .. } => 0.5,
                    EffectKind::Burst { .. } => 0.35,
                    EffectKind::Debris { .. } => 0.7,
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
    pub(super) fn spawn_fx(&mut self, events: &[Event]) {
        let sees = |game: &Self, pos: chassis::fx::Vec2Fx| {
            game.my_vision()
                .visible(chassis::grid::TilePos::containing(pos))
        };
        for event in events {
            match event {
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
                        self.aim_units.insert(
                            attacker.0,
                            (d.y.atan2(d.x) + std::f32::consts::FRAC_PI_2, self.fx_clock),
                        );
                    }
                    let own_target = match target {
                        oxide_sim::Target::Unit(uid) => self
                            .state
                            .unit(*uid)
                            .is_some_and(|u| u.player == self.human),
                        oxide_sim::Target::Building(bid) => self
                            .state
                            .building(*bid)
                            .is_some_and(|b| b.player == self.human),
                    };
                    if own_target {
                        self.raise_alert(world_vec(*target_pos));
                    }
                    // Kind rides in the event: the attacker itself may have
                    // died later this same tick, and a rail shot deserves
                    // its report either way. The weapon's character decides
                    // the report and whether the impact blooms.
                    let heard = sees(self, *attacker_pos) || sees(self, *target_pos);
                    let sound = unit_fire_sound(*attacker_kind);
                    // The burst radius comes from the exact weapon that
                    // fired — the event says which slot — so the
                    // telegraphed area never overstates (or hides) the
                    // damage the sim will deal.
                    let splash = attacker_kind
                        .stats()
                        .weapons
                        .get(*weapon)
                        .and_then(|w| w.splash)
                        .map(|s| s.to_num::<f32>());
                    if heard {
                        let at = if sees(self, *attacker_pos) {
                            *attacker_pos
                        } else {
                            *target_pos
                        };
                        self.sounds_pending.push((sound, Some(world_vec(at))));
                    }
                    push_direct_report(
                        &mut self.fx,
                        unit_shot_style(*attacker_kind, *weapon),
                        visual_shot_origin(
                            world_vec(*attacker_pos),
                            world_vec(*target_pos),
                            unit_muzzle_reach(*attacker_kind),
                        ),
                        world_vec(*target_pos),
                        splash,
                        self.state.current_tick(),
                    );
                }
                Event::TurretFired {
                    kind,
                    turret,
                    turret_pos,
                    target_pos,
                    target,
                    ..
                } => {
                    self.aim_building_targets.insert(turret.0, *target);
                    let d = world_vec(*target_pos) - world_vec(*turret_pos);
                    if d.length_squared() > 1e-6 {
                        self.aim_buildings.insert(
                            turret.0,
                            (d.y.atan2(d.x) + std::f32::consts::FRAC_PI_2, self.fx_clock),
                        );
                    }
                    // A defense chewing on one of our entities is an
                    // attack like any other; the death event raises its
                    // own alert too.
                    let own_target = match target {
                        oxide_sim::Target::Unit(id) => self
                            .state
                            .unit(*id)
                            .is_some_and(|unit| unit.player == self.human),
                        oxide_sim::Target::Building(id) => self
                            .state
                            .building(*id)
                            .is_some_and(|building| building.player == self.human),
                    };
                    if own_target {
                        self.raise_alert(world_vec(*target_pos));
                    }
                    // Kind rides in the event: the turret may be rubble by
                    // now (destroyed the tick it fired), and its shot still
                    // deserves the right report and burst.
                    let sound = defense_fire_sound(*kind);
                    let splash = kind
                        .base_stats()
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
                        self.sounds_pending.push((sound, Some(world_vec(at))));
                    }
                    push_direct_report(
                        &mut self.fx,
                        defense_shot_style(*kind),
                        visual_shot_origin(
                            world_vec(*turret_pos),
                            world_vec(*target_pos),
                            defense_muzzle_reach(*kind),
                        ),
                        world_vec(*target_pos),
                        splash,
                        self.state.current_tick(),
                    );
                }
                Event::BuildingCompleted {
                    building,
                    player,
                    kind,
                } if *player == self.human => {
                    // A completion at a nonzero tier is an upgrade
                    // finishing: its own cue, its own name.
                    let tier = self.state.building(*building).map_or(0, |b| b.tier);
                    if tier > 0 {
                        self.sounds_pending.push((SoundKind::UpgradeDone, None));
                        self.toast(format!("{} online", kind.tier_name(tier)));
                    } else {
                        self.sounds_pending.push((SoundKind::TrainDone, None));
                        self.toast(format!("{} online", kind.name()));
                    }
                }
                Event::BuildCancelled { player, refund, .. } if *player == self.human => {
                    self.toast(format!("site salvaged (+{refund} scrap)"));
                }
                Event::UnitDied {
                    unit,
                    pos,
                    player,
                    kind,
                } => {
                    if kind.stats().domain == oxide_sim::stats::Domain::Air
                        && !crate::render::reduced_motion()
                    {
                        self.fx.push(Effect {
                            kind: EffectKind::Falling {
                                at: world_vec(*pos),
                                unit: *kind,
                                faction: self.state.player(*player).faction,
                            },
                            age: 0.0,
                        });
                    } else if !crate::render::reduced_motion() {
                        // Ground kills scatter hull shards; flyers
                        // already tell their death with the fall.
                        self.fx.push(Effect {
                            kind: EffectKind::Debris {
                                at: world_vec(*pos),
                                seed: unit.0,
                            },
                            age: 0.0,
                        });
                    }
                    if *player == self.human {
                        self.raise_alert(world_vec(*pos));
                    }
                    if *player == self.human || sees(self, *pos) {
                        self.sounds_pending
                            .push((SoundKind::UnitDeath, Some(world_vec(*pos))));
                    }
                    self.fx.push(Effect {
                        kind: EffectKind::Puff {
                            at: world_vec(*pos),
                        },
                        age: 0.0,
                    });
                }
                Event::BuildingDestroyed { pos, player, .. } => {
                    if *player == self.human {
                        self.raise_alert(world_vec(*pos));
                    }
                    if *player == self.human || sees(self, *pos) {
                        self.sounds_pending
                            .push((SoundKind::BuildingBoom, Some(world_vec(*pos))));
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
                    self.sounds_pending.push((SoundKind::TrainDone, None));
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
                    self.sounds_pending.push((SoundKind::Deposit, None));
                }
                Event::CommandRejected { player, reason } if *player == self.human => {
                    let why = match reason {
                        oxide_sim::command::RejectReason::NotEnoughScrap => "not enough scrap",
                        oxide_sim::command::RejectReason::WrongFaction => {
                            "that machine belongs to the other faction"
                        }
                        oxide_sim::command::RejectReason::QueueFull => "queue is full",
                        oxide_sim::command::RejectReason::UnreachableGoal => "can't reach that",
                        oxide_sim::command::RejectReason::InvalidTarget => "can't target that",
                        oxide_sim::command::RejectReason::NotANode => "nothing to mine there",
                        oxide_sim::command::RejectReason::NotYourBuilding => "not your building",
                        oxide_sim::command::RejectReason::CannotProduce => {
                            "that factory can't make those"
                        }
                        oxide_sim::command::RejectReason::BadSite => "can't build there",
                        oxide_sim::command::RejectReason::NoValidUnits => {
                            "nothing selected can do that"
                        }
                        oxide_sim::command::RejectReason::OutOfBounds => "outside the map",
                        oxide_sim::command::RejectReason::Eliminated => "you are eliminated",
                        oxide_sim::command::RejectReason::MissingPrerequisite => {
                            "needs its tech building first"
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
                    ..
                } => {
                    // The gun turns to its work — a Bastion's mount as
                    // much as a Bombard's chassis.
                    let d = world_vec(*to) - world_vec(*from);
                    if d.length_squared() > 1e-6 {
                        let angle = d.y.atan2(d.x) + std::f32::consts::FRAC_PI_2;
                        match shooter {
                            oxide_sim::Target::Unit(uid) => {
                                self.aim_units.insert(uid.0, (angle, self.fx_clock));
                            }
                            oxide_sim::Target::Building(bid) => {
                                self.aim_building_targets.insert(bid.0, *target);
                                self.aim_buildings.insert(bid.0, (angle, self.fx_clock));
                            }
                        }
                    }
                    // No effect spawned: in-flight shells render from
                    // `state.shells()` directly, aged by sim ticks — a
                    // paused shell hangs in the air, a loaded replay
                    // restores its arc, and speed changes track.
                    // Sound follows sight — but an incoming shell is a
                    // warning worth keeping. A hostile launch whose
                    // muzzle is fogged plays anchored at its IMPACT:
                    // the same information the sim's incoming-shell
                    // sense grants (impact tile visible), loudest when
                    // it is falling on you, and nothing tracks the gun.
                    let own = *player == self.human;
                    let hostile = self.state.hostile(self.human, *player);
                    if let Some((sound, anchor)) = shell_launch_audio(
                        *shooter,
                        own,
                        hostile,
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
                    // The event names no victim on purpose (a shell in
                    // flight chooses nothing), so ask the post-tick world
                    // whether the blast reached anything of ours —
                    // survivors alert here, the dead through their own
                    // events.
                    let reach = splash.map_or(1.0, |r| r.to_num::<f32>().max(1.0));
                    let world = world_vec(*at);
                    let hostile_shell = self.state.hostile(self.human, *player);
                    // Parenthesized deliberately: && binds tighter than
                    // ||, and an unguarded building branch once alarmed
                    // on the player's own defensive artillery.
                    let own_hurt = hostile_shell
                        && (self
                            .state
                            .units()
                            .iter()
                            .filter(|u| {
                                u.player == self.human && targets.covers(u.kind.stats().domain)
                            })
                            .any(|u| world_vec(u.pos).distance(world) <= reach)
                            || (targets.covers(oxide_sim::stats::Domain::Ground)
                                && self
                                    .state
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
                    if sees(self, *at) {
                        self.sounds_pending
                            .push((SoundKind::Artillery, Some(world_vec(*at))));
                    }
                    self.fx.push(Effect {
                        kind: EffectKind::Burst {
                            at: world_vec(*at),
                            radius: splash.map_or(0.8, |r| r.to_num::<f32>()),
                        },
                        age: 0.0,
                    });
                }
                Event::OrderStalled {
                    player,
                    pos,
                    reason,
                    ..
                } if *player == self.human => {
                    // Own-state facts only — a stall reason must never
                    // whisper about what fog hides.
                    self.toast(match reason {
                        oxide_sim::StallReason::NoRoute => "no route to that order",
                        oxide_sim::StallReason::NoFiringPosition => "no ground to fire from there",
                        oxide_sim::StallReason::InsufficientScrap => "out of scrap",
                        oxide_sim::StallReason::GroundTaken => {
                            "that ground was taken before the founder arrived"
                        }
                        oxide_sim::StallReason::TransportFull => "the transport is full",
                        oxide_sim::StallReason::NoOpenGround => "no open ground to unload there",
                    });
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
                        oxide_sim::GameResult::Victory { team }
                            if *team == self.state.player(self.human).team
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
        self.refresh_defense_aim();
    }

    fn refresh_defense_aim(&mut self) {
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
                let building = self.state.building(oxide_sim::BuildingId(building_id))?;
                if building.cooldown == 0 {
                    return None;
                }
                let target_pos = match target {
                    oxide_sim::Target::Unit(id) => {
                        let unit = self.state.unit(id)?;
                        let visible = self.all_seeing()
                            || !self.state.hostile(self.human, unit.player)
                            || self.my_vision().visible(unit.tile());
                        visible.then(|| world_vec(unit.pos))?
                    }
                    oxide_sim::Target::Building(id) => {
                        let target = self.state.building(id)?;
                        let visible = self.all_seeing()
                            || !self.state.hostile(self.human, target.player)
                            || target.tiles().any(|tile| self.my_vision().visible(tile));
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
mod tests {
    use super::*;
    use oxide_sim::{BuildingId, BuildingKind, Target, UnitId, UnitKind};

    fn defense_tracking_game() -> (crate::game::Game, BuildingId, UnitId) {
        let mut scenario = oxide_sim::Scenario::skirmish();
        scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
            player: 0,
            kind: BuildingKind::Turret,
            x: 11,
            y: 10,
        });
        scenario.units.push(oxide_sim::scenario::UnitSpec {
            player: 1,
            kind: UnitKind::Harvester,
            x: 14,
            y: 10,
        });
        let game =
            crate::game::Game::with_viewport(scenario, macroquad::prelude::vec2(1280.0, 800.0))
                .expect("tracking scenario builds");
        let building = game
            .state
            .buildings()
            .iter()
            .find(|building| building.kind == BuildingKind::Turret)
            .unwrap()
            .id;
        let target = game
            .state
            .units()
            .iter()
            .find(|unit| unit.tile() == chassis::grid::TilePos::new(14, 10))
            .unwrap()
            .id;
        (game, building, target)
    }

    #[test]
    fn every_weapon_family_uses_its_physical_report() {
        assert_eq!(unit_shot_style(UnitKind::Scuttler, 0), ShotStyle::Contact);
        assert_eq!(unit_shot_style(UnitKind::Lancer, 0), ShotStyle::Rail);
        assert_eq!(
            unit_shot_style(UnitKind::Flakhound, 0),
            ShotStyle::FlakBurst {
                yoke_delay: FlakYokeDelay::OneTick,
            }
        );
        assert_eq!(
            unit_shot_style(UnitKind::Stinger, 0),
            ShotStyle::FlakBurst {
                yoke_delay: FlakYokeDelay::None,
            }
        );
        // Both Sentinel slots speak through its one physical barrel;
        // the second is a weaker skyward poke, not a paired flak gun.
        assert_eq!(unit_shot_style(UnitKind::Sentinel, 0), ShotStyle::ForgeSpot);
        assert_eq!(unit_shot_style(UnitKind::Sentinel, 1), ShotStyle::ForgeSpot);
        assert_eq!(unit_shot_style(UnitKind::Buzzard, 0), ShotStyle::ForgeSpot);
        assert_eq!(unit_shot_style(UnitKind::Darter, 0), ShotStyle::ForgeSpot);
        assert_eq!(unit_shot_style(UnitKind::Talon, 0), ShotStyle::ForgeSpot);
        assert_eq!(unit_shot_style(UnitKind::Wisp, 0), ShotStyle::ForgeSpot);
        assert_eq!(
            defense_shot_style(BuildingKind::FlakTurret),
            ShotStyle::FlakBurst {
                yoke_delay: FlakYokeDelay::OneAndHalfTicks,
            }
        );
        assert_eq!(
            defense_shot_style(BuildingKind::Turret),
            ShotStyle::ForgeSpot
        );
    }

    #[test]
    fn flak_yoke_rounds_switch_with_the_second_muzzle_frame() {
        assert_eq!(
            crate::presentation_animation::FLAKHOUND_REPORT_TICKS / 2.0,
            FlakYokeDelay::OneTick.ticks()
        );
        assert_eq!(
            crate::presentation_animation::FLAK_TURRET_REPORT_TICKS / 2.0,
            FlakYokeDelay::OneAndHalfTicks.ticks()
        );
    }

    #[test]
    fn splash_bloom_stays_with_the_one_direct_report_until_arrival() {
        let mut effects = Vec::new();
        push_direct_report(
            &mut effects,
            ShotStyle::FlakBurst {
                yoke_delay: FlakYokeDelay::OneTick,
            },
            Vec2::ZERO,
            Vec2::ONE,
            Some(1.25),
            42,
        );

        assert_eq!(effects.len(), 1, "one hit creates one flak report");
        assert!(matches!(
            effects[0].kind,
            EffectKind::DirectShot {
                style: ShotStyle::FlakBurst {
                    yoke_delay: FlakYokeDelay::OneTick,
                },
                splash: Some(1.25),
                completed_tick: 42,
                ..
            }
        ));
    }

    #[test]
    fn direct_reports_age_on_sim_time_instead_of_wall_time() {
        let shot = Effect {
            kind: EffectKind::DirectShot {
                style: ShotStyle::ForgeSpot,
                from: Vec2::ZERO,
                to: Vec2::ONE,
                splash: None,
                completed_tick: 100,
            },
            age: 0.0,
        };
        assert_eq!(shot.age_at(100, 0.0), 0.0);
        assert!((shot.age_at(102, 0.5) - 2.5 * crate::game::TICK_DT).abs() < 1.0e-6);
    }

    #[test]
    fn final_volley_drains_after_the_simulation_stops() {
        let mut game = crate::game::Game::with_viewport(
            oxide_sim::Scenario::skirmish(),
            macroquad::prelude::vec2(1280.0, 800.0),
        )
        .expect("skirmish builds");
        game.issue(oxide_sim::Command::Surrender);
        game.do_tick();
        assert!(game.state.result().is_some());

        let completed_tick = game.state.current_tick();
        game.fx.push(Effect {
            kind: EffectKind::DirectShot {
                style: ShotStyle::ForgeSpot,
                from: Vec2::ZERO,
                to: Vec2::ONE,
                splash: None,
                completed_tick,
            },
            age: 0.0,
        });
        game.update_fx(ShotStyle::ForgeSpot.life() + 0.01);
        assert!(game.fx.is_empty());
    }

    #[test]
    fn only_bombard_and_bastion_use_real_shell_entities() {
        let units = [
            UnitKind::Harvester,
            UnitKind::Sentinel,
            UnitKind::Scuttler,
            UnitKind::Lancer,
            UnitKind::Bombard,
            UnitKind::Flakhound,
            UnitKind::Stinger,
            UnitKind::Buzzard,
            UnitKind::Darter,
            UnitKind::Talon,
            UnitKind::Wisp,
        ];
        let unit_shells: Vec<_> = units
            .into_iter()
            .filter(|kind| kind.stats().weapons.iter().any(|weapon| weapon.projectile))
            .collect();
        assert_eq!(unit_shells, vec![UnitKind::Bombard]);

        let buildings = [
            BuildingKind::Foundry,
            BuildingKind::Turret,
            BuildingKind::Fabricator,
            BuildingKind::FlakTurret,
            BuildingKind::Bastion,
            BuildingKind::Array,
            BuildingKind::Reclaimer,
            BuildingKind::RepairBay,
        ];
        let building_shells: Vec<_> = buildings
            .into_iter()
            .filter(|kind| {
                kind.base_stats()
                    .weapons
                    .iter()
                    .any(|weapon| weapon.projectile)
            })
            .collect();
        assert_eq!(building_shells, vec![BuildingKind::Bastion]);
    }

    #[test]
    fn approved_combatants_use_their_own_reports() {
        assert_eq!(unit_fire_sound(UnitKind::Sentinel), SoundKind::SentinelFire);
        assert_eq!(unit_fire_sound(UnitKind::Scuttler), SoundKind::ScuttlerFire);
        assert_eq!(unit_fire_sound(UnitKind::Lancer), SoundKind::LancerFire);
        assert_eq!(
            unit_fire_sound(UnitKind::Flakhound),
            SoundKind::FlakhoundFire
        );
        assert_eq!(unit_fire_sound(UnitKind::Stinger), SoundKind::StingerFire);
        assert_eq!(unit_fire_sound(UnitKind::Buzzard), SoundKind::BuzzardFire);
        assert_eq!(unit_fire_sound(UnitKind::Darter), SoundKind::DarterFire);
        assert_eq!(unit_fire_sound(UnitKind::Talon), SoundKind::TalonFire);
        assert_eq!(unit_fire_sound(UnitKind::Wisp), SoundKind::WispFire);
        assert_eq!(
            defense_fire_sound(BuildingKind::FlakTurret),
            SoundKind::FlakTurretFire
        );
        assert_eq!(
            shell_fire_sound(Target::Unit(UnitId(4))),
            SoundKind::BombardFire
        );
        assert_eq!(
            shell_fire_sound(Target::Building(BuildingId(7))),
            SoundKind::BastionFire
        );
    }

    #[test]
    fn generic_combatants_keep_the_generic_report() {
        assert_eq!(unit_fire_sound(UnitKind::Harvester), SoundKind::Laser);
        assert_eq!(defense_fire_sound(BuildingKind::Turret), SoundKind::Laser);
    }

    #[test]
    fn artillery_launch_audio_respects_sight_and_allegiance() {
        let bombard = Target::Unit(UnitId(4));
        assert_eq!(
            shell_launch_audio(bombard, false, true, true, true),
            Some((SoundKind::BombardFire, ShellSoundAnchor::Muzzle))
        );
        assert_eq!(
            shell_launch_audio(bombard, false, true, false, true),
            Some((SoundKind::ArtilleryLaunch, ShellSoundAnchor::Impact))
        );
        assert_eq!(shell_launch_audio(bombard, false, true, false, false), None);
        assert_eq!(
            shell_launch_audio(bombard, false, false, false, true),
            None,
            "a fogged allied shell must not sound like an incoming threat"
        );
        assert_eq!(
            shell_launch_audio(bombard, true, false, false, false),
            Some((SoundKind::BombardFire, ShellSoundAnchor::Muzzle)),
            "the local gun remains audible without revealing another seat"
        );
    }

    #[test]
    fn shot_visuals_begin_at_the_authored_muzzle_not_chassis_center() {
        let from = macroquad::prelude::vec2(2.0, 3.0);
        let to = macroquad::prelude::vec2(12.0, 3.0);
        assert_eq!(
            visual_shot_origin(from, to, 0.38),
            macroquad::prelude::vec2(2.38, 3.0)
        );
        assert_eq!(visual_shot_origin(from, from, 0.38), from);
        assert_eq!(unit_muzzle_reach(UnitKind::Buzzard), 0.44);
        assert_eq!(unit_muzzle_reach(UnitKind::Darter), 0.32);
        assert!(
            defense_muzzle_reach(BuildingKind::Bastion)
                > defense_muzzle_reach(BuildingKind::Turret)
        );
    }

    #[test]
    fn defense_mount_tracks_only_a_target_the_viewer_may_see() {
        let (mut game, building, _) = defense_tracking_game();
        let report = game.state.tick(&[]);
        game.spawn_fx(&report.events);
        assert!(game.state.building(building).unwrap().cooldown > 0);
        let hostile = game
            .state
            .units()
            .iter()
            .find(|unit| {
                game.state.hostile(game.human, unit.player)
                    && !game.my_vision().visible(unit.tile())
            })
            .expect("skirmish has a fogged hostile unit");
        assert!(!game.my_vision().visible(hostile.tile()));
        let target = Target::Unit(hostile.id);
        game.aim_building_targets.insert(building.0, target);
        game.aim_buildings.insert(building.0, (0.42, 0.0));
        game.update_fx(crate::game::TICK_DT);

        game.refresh_defense_aim();
        assert_eq!(game.aim_buildings[&building.0].0, 0.42);

        game.overlay = true;
        game.refresh_defense_aim();
        assert_ne!(game.aim_buildings[&building.0].0, 0.42);
    }

    #[test]
    fn defense_mount_follows_its_visible_target_during_reload() {
        let (mut game, building, target) = defense_tracking_game();
        let report = game.state.tick(&[]);
        game.spawn_fx(&report.events);
        let first_angle = game.aim_buildings[&building.0].0;
        let first_pos = game.state.unit(target).unwrap().pos;

        game.update_fx(crate::game::TICK_DT);
        let report = game.state.tick(&[oxide_sim::PlayerCommand {
            player: oxide_sim::PlayerId(1),
            command: oxide_sim::Command::Move {
                units: vec![target],
                goal: chassis::grid::TilePos::new(14, 14),
                queue: false,
            },
        }]);
        game.spawn_fx(&report.events);

        assert_ne!(game.state.unit(target).unwrap().pos, first_pos);
        assert_ne!(game.aim_buildings[&building.0].0, first_angle);
        assert!(game.state.building(building).unwrap().cooldown > 0);
    }

    #[test]
    fn shell_report_keeps_predicted_heading_on_the_launch_frame() {
        let mut scenario = oxide_sim::Scenario::skirmish();
        scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
            player: 0,
            kind: BuildingKind::Bastion,
            x: 11,
            y: 10,
        });
        scenario.units.push(oxide_sim::scenario::UnitSpec {
            player: 1,
            kind: UnitKind::Harvester,
            x: 16,
            y: 10,
        });
        let mut game =
            crate::game::Game::with_viewport(scenario, macroquad::prelude::vec2(1280.0, 800.0))
                .expect("tracking scenario builds");
        let shooter = game
            .state
            .buildings()
            .iter()
            .find(|building| building.kind == BuildingKind::Bastion)
            .unwrap()
            .id;
        let target = game
            .state
            .units()
            .iter()
            .find(|unit| unit.tile() == chassis::grid::TilePos::new(16, 10))
            .unwrap()
            .id;
        let report = game.state.tick(&[oxide_sim::PlayerCommand {
            player: oxide_sim::PlayerId(1),
            command: oxide_sim::Command::Move {
                units: vec![target],
                goal: chassis::grid::TilePos::new(16, 14),
                queue: false,
            },
        }]);
        let (from, to) = report
            .events
            .iter()
            .find_map(|event| match event {
                oxide_sim::Event::ShellLaunched {
                    shooter: Target::Building(id),
                    from,
                    to,
                    ..
                } if *id == shooter => Some((*from, *to)),
                _ => None,
            })
            .expect("the Bastion launches while its target begins moving");
        assert!(game.state.building(shooter).unwrap().cooldown > 0);
        assert!(
            game.my_vision()
                .visible(game.state.unit(target).unwrap().tile())
        );
        let expected = world_vec(to) - world_vec(from);
        let expected_angle = expected.y.atan2(expected.x) + std::f32::consts::FRAC_PI_2;
        let current = world_vec(game.state.unit(target).unwrap().pos) - world_vec(from);
        let current_angle = current.y.atan2(current.x) + std::f32::consts::FRAC_PI_2;
        assert!((expected_angle - current_angle).abs() > 1e-4);

        game.spawn_fx(&report.events);
        let angle = game.aim_buildings[&shooter.0].0;
        assert!((angle - expected_angle).abs() < 1e-6);
    }

    #[test]
    fn ground_kills_scatter_debris_and_air_kills_fall() {
        crate::render::set_reduced_motion(false);
        let mut game = crate::game::Game::with_viewport(
            oxide_sim::Scenario::skirmish(),
            macroquad::prelude::vec2(1280.0, 800.0),
        )
        .expect("embedded skirmish builds");
        let at = chassis::fx::Vec2Fx {
            x: chassis::fx::Fx::from_num(5),
            y: chassis::fx::Fx::from_num(5),
        };
        game.spawn_fx(&[oxide_sim::Event::UnitDied {
            unit: oxide_sim::UnitId(7),
            kind: UnitKind::Sentinel,
            player: oxide_sim::PlayerId(1),
            pos: at,
        }]);
        assert!(
            game.fx
                .iter()
                .any(|e| matches!(e.kind, EffectKind::Debris { seed: 7, .. })),
            "a ground kill scatters shards seeded by the casualty"
        );
        game.fx.clear();
        game.spawn_fx(&[oxide_sim::Event::UnitDied {
            unit: oxide_sim::UnitId(8),
            kind: UnitKind::Buzzard,
            player: oxide_sim::PlayerId(1),
            pos: at,
        }]);
        assert!(
            game.fx
                .iter()
                .any(|e| matches!(e.kind, EffectKind::Falling { .. })),
            "a flyer tells its death with the fall"
        );
        assert!(
            !game
                .fx
                .iter()
                .any(|e| matches!(e.kind, EffectKind::Debris { .. })),
            "no double story for one death"
        );
    }
}
