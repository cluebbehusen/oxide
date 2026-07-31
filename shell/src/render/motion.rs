//! Pure presentation pose math.
//!
//! Rendering consumes these values, but the functions know nothing about
//! macroquad or simulation mutation. Keeping the clock-to-pose mapping here
//! makes reduced-motion behavior and deterministic offsets cheap to test.

use oxide_sim::BuildingKind;

/// Procedural pose applied to one moving unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct UnitPose {
    /// Sideways offset in fractions of one sprite width.
    pub lateral: f32,
    /// Vertical offset in fractions of one sprite height.
    pub lift: f32,
    /// Horizontal squash, used as a readable top-down bank.
    pub width_scale: f32,
    /// Vertical squash/stretch, used for ground weight.
    pub height_scale: f32,
    /// Strength of the ground wake.
    pub dust: f32,
    /// Strength of the air exhaust.
    pub thruster: f32,
}

impl UnitPose {
    const REST: Self = Self {
        lateral: 0.0,
        lift: 0.0,
        width_scale: 1.0,
        height_scale: 1.0,
        dust: 0.0,
        thruster: 0.0,
    };
}

/// Presentation pose for one unit.
///
/// Entity id offsets prevent a formation from bobbing in lockstep. Reduced
/// motion removes travel oscillation and particles but keeps a steady air
/// exhaust, so an active flyer still reads as powered rather than parked.
pub(crate) fn unit_pose(
    time: f32,
    id: u32,
    moving: bool,
    airborne: bool,
    reduced: bool,
) -> UnitPose {
    if !moving {
        return UnitPose::REST;
    }
    if reduced {
        return UnitPose {
            thruster: if airborne { 0.55 } else { 0.0 },
            ..UnitPose::REST
        };
    }
    let phase = time * if airborne { 5.2 } else { 8.4 } + id as f32 * 1.618_034;
    let wave = phase.sin();
    let stride = phase.cos();
    if airborne {
        UnitPose {
            lateral: wave * 0.025,
            lift: stride * 0.025,
            width_scale: 1.0 - wave.abs() * 0.065,
            height_scale: 1.0 + wave.abs() * 0.025,
            dust: 0.0,
            thruster: 0.62 + 0.28 * (stride * 0.5 + 0.5),
        }
    } else {
        UnitPose {
            lateral: wave * 0.014,
            lift: -stride.abs() * 0.014,
            width_scale: 1.0 + wave * 0.012,
            height_scale: 1.0 - wave * 0.009,
            dust: (stride * 0.5 + 0.5).powi(2),
            thruster: 0.0,
        }
    }
}

/// A stable 0..1 activity cycle, offset per entity.
pub(crate) fn activity_phase(time: f32, id: u32, speed: f32, reduced: bool) -> f32 {
    if reduced {
        0.5
    } else {
        ((time * speed + id as f32 * 0.754_878).sin() * 0.5 + 0.5).clamp(0.0, 1.0)
    }
}

/// Pose for a defense's separately drawn mount.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MountPose {
    /// Aim survives reduced motion because it communicates target state.
    pub angle: f32,
    /// Backward displacement as a fraction of footprint width.
    pub recoil: f32,
    /// Short launch flash.
    pub flash: f32,
}

pub(crate) fn mount_pose(
    kind: BuildingKind,
    angle: f32,
    shot_age: f32,
    reduced: bool,
) -> MountPose {
    let (duration, distance, flash_duration) = match kind {
        BuildingKind::Turret => (0.12, 0.050, 0.10),
        BuildingKind::FlakTurret => (0.18, 0.035, 0.16),
        BuildingKind::Bastion => (0.34, 0.060, 0.24),
        _ => (0.0, 0.0, 0.0),
    };
    let recoil = if reduced || duration == 0.0 || shot_age >= duration {
        0.0
    } else {
        distance * (1.0 - shot_age / duration)
    };
    let flash = if reduced || flash_duration == 0.0 || shot_age >= flash_duration {
        0.0
    } else {
        1.0 - shot_age / flash_duration
    };
    MountPose {
        angle,
        recoil,
        flash,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_motion_is_repeatable_and_formation_offsets_do_not_lock_step() {
        let a = unit_pose(2.25, 7, true, false, false);
        assert_eq!(a, unit_pose(2.25, 7, true, false, false));
        assert_ne!(a, unit_pose(2.25, 8, true, false, false));
        assert!(a.dust > 0.0);
        assert_eq!(a.thruster, 0.0);
    }

    #[test]
    fn reduced_motion_holds_pose_but_keeps_powered_flyer_state() {
        let ground = unit_pose(8.0, 3, true, false, true);
        assert_eq!(ground, UnitPose::REST);

        let air = unit_pose(8.0, 3, true, true, true);
        assert_eq!(air.lateral, 0.0);
        assert_eq!(air.lift, 0.0);
        assert_eq!(air.width_scale, 1.0);
        assert_eq!(air.dust, 0.0);
        assert!(air.thruster > 0.0);
        assert_eq!(activity_phase(1.0, 1, 9.0, true), 0.5);
        assert_eq!(activity_phase(99.0, 91, 0.2, true), 0.5);
    }

    #[test]
    fn air_and_ground_use_distinct_activity_cues() {
        let ground = unit_pose(0.4, 11, true, false, false);
        let air = unit_pose(0.4, 11, true, true, false);
        assert!(ground.dust > 0.0);
        assert_eq!(ground.thruster, 0.0);
        assert_eq!(air.dust, 0.0);
        assert!(air.thruster > 0.0);
        assert_ne!(air.width_scale, ground.width_scale);
        assert_eq!(unit_pose(0.4, 11, false, true, false), UnitPose::REST);
    }

    #[test]
    fn reduced_mount_keeps_aim_and_drops_recoil() {
        let live = mount_pose(BuildingKind::Bastion, 1.25, 0.05, false);
        assert_eq!(live.angle, 1.25);
        assert!(live.recoil > 0.0);
        assert!(live.flash > 0.0);

        let held = mount_pose(BuildingKind::Bastion, 1.25, 0.05, true);
        assert_eq!(held.angle, 1.25);
        assert_eq!(held.recoil, 0.0);
        assert_eq!(held.flash, 0.0);
        assert_eq!(
            mount_pose(BuildingKind::Bastion, 1.25, 2.0, false).recoil,
            0.0
        );
    }
}
