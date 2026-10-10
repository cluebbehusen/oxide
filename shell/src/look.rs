//! What each unit kind looks and sounds like in the shell, declared in one
//! place. [`unit()`] is one exhaustive match, so a new kind must state every
//! presentation fact here instead of drawing with another kind's defaults.
//! Facts the simulation already states (rotorcraft, large airframes,
//! scouts, demolition, braced siege) are read from its stats instead.

use crate::game::{FlakYokeDelay, ShotStyle, SoundKind};
use macroquad::prelude::{Vec2, vec2};
use oxide_sim::UnitKind;
use oxide_sim::stats::Role;

/// The usual sprite size against a unit's tile.
const DEFAULT_SCALE: f32 = 1.05;
/// The Warden's broad hull.
const HEAVY_SCALE: f32 = 1.4;
/// Large airframes and the heaviest walkers.
const LARGE_SCALE: f32 = 2.0;

/// A unit kind's presentation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct UnitLook {
    /// Sprite size against the unit's tile.
    pub(crate) scale: f32,
    /// How the body moves on screen.
    pub(crate) gait: Gait,
    /// Which atlas rows hold its two locomotion poses.
    pub(crate) move_rows: MoveRows,
    /// Exposed belt runs inside the track casings, in 128-px sprite space.
    pub(crate) belts: &'static [[f32; 4]],
    /// A separate weapon or rotor mount drawn over the hull.
    pub(crate) rig: bool,
    /// The work body a worker draws instead of its plain sprite.
    pub(crate) tool: Option<WorkerTool>,
    /// The shadow and lift of a large airframe; other flyers share
    /// [`SMALL_AIRFRAME`].
    pub(crate) airframe: Option<Airframe>,
    /// How its weapon reports, for every kind that fights.
    pub(crate) weapon: Option<WeaponLook>,
    /// Its strategic marker at far zoom.
    pub(crate) marker: MarkerRole,
}

/// How a body moves on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Gait {
    /// A tracked hull cycling tread frames.
    Treads,
    /// Legs stepping between two strides.
    Legs,
    /// Lift rotors; the hull turns toward travel at `hull_turn` radians a
    /// tick.
    Rotor { hull_turn: f32 },
    /// Alternating move frames, or the worker's own work poses.
    Plain,
}

/// Which atlas rows a kind's locomotion poses use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MoveRows {
    /// `_tread1`, `_tread2`.
    Tread,
    /// `_move1`, `_move2`.
    Move,
}

/// The work body a worker draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkerTool {
    /// The Harvester's scoop and cargo bay.
    Scoop,
    /// The Excavator's drum.
    Drum,
    /// The Scuttler's shears.
    Shears,
    /// The Tender's welding arm.
    Welder,
}

/// Shadow and lift of an airframe, in tiles at unit zoom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Airframe {
    pub(crate) shadow: Vec2,
    pub(crate) shadow_offset: Vec2,
    pub(crate) lift: f32,
}

/// Every flyer without a large airframe.
pub(crate) const SMALL_AIRFRAME: Airframe = Airframe {
    shadow: vec2(0.9, 0.9),
    shadow_offset: vec2(0.16, 0.26),
    lift: 0.18,
};

/// How a unit's weapon reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WeaponLook {
    /// The report heard when it fires.
    pub(crate) sound: SoundKind,
    /// The direct-fire report drawn, and where the muzzle sits along the
    /// shot in tiles. Shells and charges draw their own way.
    pub(crate) shot: Option<(ShotStyle, f32)>,
}

/// A unit's strategic marker at far zoom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkerRole {
    Worker,
    Gun,
    Siege,
    AntiAir,
    Scout,
    Support,
    Transport,
    Demolition,
}

/// Whether a unit sees buried charges, as the simulation's scouts do.
pub(crate) fn scout(kind: UnitKind) -> bool {
    kind.role() == Role::Scout
}

/// Whether a unit of `kind` launches `payload` from one of its weapons.
pub(crate) fn fires(kind: UnitKind, payload: oxide_sim::ProjectileKind) -> bool {
    kind.stats().weapons.iter().any(|weapon| {
        weapon
            .projectile
            .is_some_and(|projectile| projectile.payload == payload)
    })
}

fn weapon(sound: SoundKind, shot: ShotStyle, muzzle: f32) -> WeaponLook {
    WeaponLook {
        sound,
        shot: Some((shot, muzzle)),
    }
}

fn launcher(sound: SoundKind) -> WeaponLook {
    WeaponLook { sound, shot: None }
}

fn rotor(hull_turn: f32) -> Gait {
    Gait::Rotor { hull_turn }
}

/// A light rotorcraft's hull turn, scaled by its speed against the
/// Buzzard's.
fn light_rotor(kind: UnitKind) -> Gait {
    rotor(
        0.3 * kind.stats().speed.to_num::<f32>() / UnitKind::Buzzard.stats().speed.to_num::<f32>(),
    )
}

/// The muzzle reach of a direct-fire weapon on a plain ground hull.
const GROUND_MUZZLE: f32 = 0.38;
/// The muzzle reach of a direct-fire weapon on a plain airframe.
const AIR_MUZZLE: f32 = 0.32;

/// The presentation of `kind`.
pub(crate) fn unit(kind: UnitKind) -> UnitLook {
    use MarkerRole as M;
    let plain = UnitLook {
        scale: DEFAULT_SCALE,
        gait: Gait::Plain,
        move_rows: MoveRows::Move,
        belts: &[],
        rig: false,
        tool: None,
        airframe: None,
        weapon: None,
        marker: M::Gun,
    };
    let kinetic = ShotStyle::Kinetic { heavy: false };
    let heavy = ShotStyle::Kinetic { heavy: true };
    match kind {
        UnitKind::Harvester => UnitLook {
            move_rows: MoveRows::Tread,
            belts: &[[20., 48., 33., 107.], [95., 48., 108., 107.]],
            tool: Some(WorkerTool::Scoop),
            marker: M::Worker,
            ..plain
        },
        UnitKind::Sentinel => UnitLook {
            gait: Gait::Treads,
            belts: &[[18., 52., 33., 111.], [95., 52., 110., 111.]],
            rig: true,
            weapon: Some(weapon(SoundKind::SentinelFire, kinetic, 0.35)),
            ..plain
        },
        UnitKind::Scuttler => UnitLook {
            gait: Gait::Legs,
            tool: Some(WorkerTool::Shears),
            weapon: Some(weapon(
                SoundKind::ScuttlerFire,
                ShotStyle::Contact,
                GROUND_MUZZLE,
            )),
            marker: M::Demolition,
            ..plain
        },
        UnitKind::Lancer => UnitLook {
            gait: Gait::Treads,
            belts: &[[16., 74., 32., 111.], [96., 74., 112., 111.]],
            rig: true,
            weapon: Some(weapon(
                SoundKind::LancerFire,
                ShotStyle::Rail,
                GROUND_MUZZLE,
            )),
            ..plain
        },
        UnitKind::Bombard => UnitLook {
            gait: Gait::Treads,
            belts: &[[16., 58., 31., 107.], [97., 58., 112., 107.]],
            weapon: Some(launcher(SoundKind::BombardFire)),
            marker: M::Siege,
            ..plain
        },
        UnitKind::Flakhound => UnitLook {
            gait: Gait::Treads,
            move_rows: MoveRows::Tread,
            belts: &[[14., 73., 31., 111.], [97., 73., 114., 111.]],
            weapon: Some(weapon(
                SoundKind::FlakhoundFire,
                ShotStyle::FlakBurst {
                    yoke_delay: FlakYokeDelay::OneTick,
                    rounds_per_yoke: 2,
                },
                42.0 / 128.0 * DEFAULT_SCALE,
            )),
            marker: M::AntiAir,
            ..plain
        },
        UnitKind::Stinger => UnitLook {
            gait: Gait::Treads,
            weapon: Some(weapon(
                SoundKind::StingerFire,
                ShotStyle::FlakBurst {
                    yoke_delay: FlakYokeDelay::None,
                    rounds_per_yoke: 1,
                },
                35.0 / 128.0 * DEFAULT_SCALE,
            )),
            marker: M::AntiAir,
            ..plain
        },
        UnitKind::Buzzard => UnitLook {
            gait: light_rotor(kind),
            rig: true,
            weapon: Some(weapon(
                SoundKind::BuzzardFire,
                heavy,
                38.0 / 128.0 * DEFAULT_SCALE,
            )),
            ..plain
        },
        UnitKind::Darter => UnitLook {
            weapon: Some(weapon(
                SoundKind::DarterFire,
                ShotStyle::ForgeSpot,
                AIR_MUZZLE,
            )),
            ..plain
        },
        UnitKind::Talon => UnitLook {
            weapon: Some(weapon(
                SoundKind::TalonFire,
                ShotStyle::ForgeSpot,
                AIR_MUZZLE,
            )),
            marker: M::AntiAir,
            ..plain
        },
        UnitKind::Wisp => UnitLook {
            gait: light_rotor(kind),
            rig: true,
            weapon: Some(weapon(
                SoundKind::WispFire,
                ShotStyle::ForgeSpot,
                AIR_MUZZLE,
            )),
            marker: M::AntiAir,
            ..plain
        },
        UnitKind::Warden => UnitLook {
            scale: HEAVY_SCALE,
            gait: Gait::Treads,
            belts: &[
                [11., 37., 30., 57.],
                [98., 37., 117., 57.],
                [11., 88., 30., 110.],
                [98., 88., 117., 110.],
            ],
            rig: true,
            weapon: Some(weapon(SoundKind::WardenFire, heavy, 0.36 * HEAVY_SCALE)),
            ..plain
        },
        UnitKind::Tender => UnitLook {
            gait: Gait::Treads,
            belts: &[[20., 52., 33., 108.], [95., 52., 108., 108.]],
            tool: Some(WorkerTool::Welder),
            marker: M::Support,
            ..plain
        },
        UnitKind::Excavator => UnitLook {
            scale: 1.3,
            belts: &[[17., 50., 32., 109.], [96., 50., 111., 109.]],
            tool: Some(WorkerTool::Drum),
            marker: M::Worker,
            ..plain
        },
        UnitKind::Kestrel | UnitKind::Gnat => UnitLook {
            marker: M::Scout,
            ..plain
        },
        // Interceptors share the air-superiority report family on purpose.
        UnitKind::Shrike => UnitLook {
            scale: 1.3,
            weapon: Some(weapon(
                SoundKind::TalonFire,
                ShotStyle::ForgeSpot,
                AIR_MUZZLE,
            )),
            marker: M::AntiAir,
            ..plain
        },
        UnitKind::Sylph => UnitLook {
            scale: 1.2,
            weapon: Some(weapon(
                SoundKind::WispFire,
                ShotStyle::ForgeSpot,
                AIR_MUZZLE,
            )),
            marker: M::AntiAir,
            ..plain
        },
        UnitKind::Condor => UnitLook {
            scale: LARGE_SCALE,
            airframe: Some(Airframe {
                shadow: vec2(1.75, 1.1875),
                shadow_offset: vec2(0.125, 0.1875),
                lift: 0.0625,
            }),
            weapon: Some(launcher(SoundKind::BombRelease)),
            marker: M::Siege,
            ..plain
        },
        UnitKind::Moth => UnitLook {
            scale: LARGE_SCALE,
            airframe: Some(Airframe {
                shadow: vec2(1.55, 1.0),
                shadow_offset: vec2(0.11, 0.17),
                lift: 0.08,
            }),
            weapon: Some(launcher(SoundKind::BombRelease)),
            marker: M::Siege,
            ..plain
        },
        UnitKind::Breaker => UnitLook {
            scale: LARGE_SCALE,
            gait: Gait::Treads,
            belts: &[[23., 42., 38., 107.], [90., 42., 105., 107.]],
            weapon: Some(weapon(SoundKind::BreakerFire, ShotStyle::Mortar, 0.625)),
            ..plain
        },
        UnitKind::Avalanche => UnitLook {
            scale: LARGE_SCALE,
            gait: Gait::Treads,
            belts: &[[27., 38., 36., 102.], [93., 38., 102., 102.]],
            weapon: Some(launcher(SoundKind::AvalancheFire)),
            marker: M::Siege,
            ..plain
        },
        UnitKind::Skyhook => UnitLook {
            scale: LARGE_SCALE,
            gait: rotor(0.25),
            rig: true,
            airframe: Some(Airframe {
                shadow: vec2(1.78, 1.52),
                shadow_offset: vec2(0.13, 0.20),
                lift: 0.07,
            }),
            marker: M::Transport,
            ..plain
        },
        UnitKind::Sapper => UnitLook {
            gait: Gait::Legs,
            weapon: Some(launcher(SoundKind::DemolitionBoom)),
            marker: M::Demolition,
            ..plain
        },
    }
}

#[cfg(test)]
mod tests;
