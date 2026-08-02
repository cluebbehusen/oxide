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
    /// An artillery shell landing.
    Artillery,
    /// An order acknowledged.
    Ack,
    /// A Sentinel's compact cannon report.
    SentinelFire,
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

/// The visual family of a direct-fire shot — mapped from the exact
/// (shooter kind, weapon slot) the hit event names, so every weapon
/// reads as itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShotStyle {
    /// A contact tool: target sparks, never a ranged projectile.
    Contact,
    /// A short physical bullet tracer at the impact end of its path.
    Tracer,
    /// The Lancer's rail: heavy, bright, lingering.
    Rail,
    /// One logical anti-air attack shown as a paired ballistic burst.
    FlakBurst,
    /// A large direct-fire round, dark-bodied with a warm short tail.
    HeavyRound,
}

impl ShotStyle {
    /// Seconds the report stays on screen.
    pub fn life(self) -> f32 {
        match self {
            ShotStyle::Contact => 0.12,
            ShotStyle::Tracer => 0.14,
            ShotStyle::Rail => 0.24,
            ShotStyle::FlakBurst => 0.18,
            ShotStyle::HeavyRound => 0.18,
        }
    }
}

/// Which report family a unit's weapon slot fires.
fn unit_shot_style(kind: oxide_sim::UnitKind, weapon: usize) -> ShotStyle {
    use oxide_sim::UnitKind;
    match (kind, weapon) {
        (UnitKind::Scuttler, _) => ShotStyle::Contact,
        (UnitKind::Lancer, _) => ShotStyle::Rail,
        (UnitKind::Flakhound | UnitKind::Stinger, _) => ShotStyle::FlakBurst,
        (UnitKind::Buzzard, _) => ShotStyle::HeavyRound,
        _ => ShotStyle::Tracer,
    }
}

fn defense_shot_style(kind: oxide_sim::BuildingKind) -> ShotStyle {
    debug_assert!(
        kind.stats().weapons.iter().all(|weapon| !weapon.projectile),
        "real shell weapons must arrive through ShellLaunched"
    );
    match kind {
        oxide_sim::BuildingKind::FlakTurret => ShotStyle::FlakBurst,
        oxide_sim::BuildingKind::Bastion => ShotStyle::HeavyRound,
        _ => ShotStyle::Tracer,
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
    match kind.stats().domain {
        oxide_sim::stats::Domain::Ground => 0.38,
        oxide_sim::stats::Domain::Air => 0.32,
    }
}

fn defense_muzzle_reach(kind: oxide_sim::BuildingKind) -> f32 {
    match kind {
        oxide_sim::BuildingKind::Bastion => kind.stats().size.0 as f32 * 0.49,
        oxide_sim::BuildingKind::FlakTurret => 0.47,
        _ => 0.44,
    }
}

fn unit_fire_sound(kind: oxide_sim::UnitKind) -> SoundKind {
    use oxide_sim::UnitKind;
    match kind {
        UnitKind::Sentinel => SoundKind::SentinelFire,
        UnitKind::Lancer => SoundKind::LancerFire,
        UnitKind::Bombard => SoundKind::BombardFire,
        UnitKind::Flakhound => SoundKind::FlakhoundFire,
        UnitKind::Stinger => SoundKind::StingerFire,
        UnitKind::Buzzard => SoundKind::BuzzardFire,
        UnitKind::Darter => SoundKind::DarterFire,
        UnitKind::Talon => SoundKind::TalonFire,
        UnitKind::Wisp => SoundKind::WispFire,
        UnitKind::Harvester | UnitKind::Scuttler => SoundKind::Laser,
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

/// Effect shapes.
pub enum EffectKind {
    /// A direct-fire shot, styled by the weapon family that spoke.
    DirectShot {
        /// Visual family (contact, tracer, rail, flak, heavy round).
        style: ShotStyle,
        /// Muzzle, world coords.
        from: Vec2,
        /// Impact, world coords.
        to: Vec2,
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
) {
    // The area bloom stays behind the weapon report. In particular, an
    // opaque flak burst must not paint over the paired terminal tracers.
    if let Some(radius) = splash {
        effects.push(Effect {
            kind: EffectKind::Burst { at: to, radius },
            age: 0.0,
        });
    }
    effects.push(Effect {
        kind: EffectKind::DirectShot { style, from, to },
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
        for fx in &mut self.fx {
            fx.age += dt;
        }
        self.fx.retain(|fx| {
            fx.age
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
                        .stats()
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
                    );
                }
                Event::BuildingCompleted { player, kind, .. } if *player == self.human => {
                    self.sounds_pending.push((SoundKind::TrainDone, None));
                    self.toast(format!("{} online", kind.name()));
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
                    };
                    self.toast(why);
                    self.sounds_pending.push((SoundKind::Denied, None));
                }
                Event::ShellLaunched {
                    shooter,
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
                    let muzzle_seen = sees(self, *from);
                    let impact_seen = sees(self, *to);
                    let sound = shell_fire_sound(*shooter);
                    if muzzle_seen || *player == self.human {
                        self.sounds_pending.push((sound, Some(world_vec(*from))));
                    } else if impact_seen {
                        self.sounds_pending.push((sound, Some(world_vec(*to))));
                    }
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_sim::{BuildingId, BuildingKind, Target, UnitId, UnitKind};

    #[test]
    fn every_weapon_family_uses_its_physical_report() {
        assert_eq!(unit_shot_style(UnitKind::Scuttler, 0), ShotStyle::Contact);
        assert_eq!(unit_shot_style(UnitKind::Lancer, 0), ShotStyle::Rail);
        assert_eq!(
            unit_shot_style(UnitKind::Flakhound, 0),
            ShotStyle::FlakBurst
        );
        assert_eq!(unit_shot_style(UnitKind::Stinger, 0), ShotStyle::FlakBurst);
        // Both Sentinel slots speak through its one physical barrel;
        // the second is a weaker skyward poke, not a paired flak gun.
        assert_eq!(unit_shot_style(UnitKind::Sentinel, 0), ShotStyle::Tracer);
        assert_eq!(unit_shot_style(UnitKind::Sentinel, 1), ShotStyle::Tracer);
        assert_eq!(unit_shot_style(UnitKind::Buzzard, 0), ShotStyle::HeavyRound);
        assert_eq!(unit_shot_style(UnitKind::Darter, 0), ShotStyle::Tracer);
        assert_eq!(unit_shot_style(UnitKind::Talon, 0), ShotStyle::Tracer);
        assert_eq!(unit_shot_style(UnitKind::Wisp, 0), ShotStyle::Tracer);
        assert_eq!(
            defense_shot_style(BuildingKind::FlakTurret),
            ShotStyle::FlakBurst
        );
        assert_eq!(defense_shot_style(BuildingKind::Turret), ShotStyle::Tracer);
    }

    #[test]
    fn splash_bloom_draws_behind_the_direct_report() {
        let mut effects = Vec::new();
        push_direct_report(
            &mut effects,
            ShotStyle::FlakBurst,
            Vec2::ZERO,
            Vec2::ONE,
            Some(1.25),
        );

        assert!(matches!(effects[0].kind, EffectKind::Burst { .. }));
        assert!(matches!(
            effects[1].kind,
            EffectKind::DirectShot {
                style: ShotStyle::FlakBurst,
                ..
            }
        ));
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
            .filter(|kind| kind.stats().weapons.iter().any(|weapon| weapon.projectile))
            .collect();
        assert_eq!(building_shells, vec![BuildingKind::Bastion]);
    }

    #[test]
    fn approved_combatants_use_their_own_reports() {
        assert_eq!(unit_fire_sound(UnitKind::Sentinel), SoundKind::SentinelFire);
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
    fn unfinished_combat_audio_keeps_the_generic_report() {
        assert_eq!(unit_fire_sound(UnitKind::Scuttler), SoundKind::Laser);
        assert_eq!(defense_fire_sound(BuildingKind::Turret), SoundKind::Laser);
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
        assert!(
            defense_muzzle_reach(BuildingKind::Bastion)
                > defense_muzzle_reach(BuildingKind::Turret)
        );
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
