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
    /// Strength of the air exhaust.
    pub thruster: f32,
}

impl UnitPose {
    const REST: Self = Self {
        lateral: 0.0,
        lift: 0.0,
        width_scale: 1.0,
        height_scale: 1.0,
        thruster: 0.0,
    };
}

/// Presentation pose for one unit.
///
/// Ground locomotion lives in authored sprite frames, keeping tread, wheel,
/// leg, and chassis motion inside the machine art. Air units retain a small
/// bank and exhaust; entity id offsets keep a formation out of lockstep.
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
    if !airborne {
        return UnitPose::REST;
    }
    let phase = time * 5.2 + id as f32 * 1.618_034;
    let wave = phase.sin();
    let stride = phase.cos();
    UnitPose {
        lateral: wave * 0.025,
        lift: stride * 0.025,
        width_scale: 1.0 - wave.abs() * 0.065,
        height_scale: 1.0 + wave.abs() * 0.025,
        thruster: 0.62 + 0.28 * (stride * 0.5 + 0.5),
    }
}

/// A stable authored-frame loop with a deterministic per-entity offset.
/// Reduced motion always holds the base frame.
pub(crate) fn loop_frame(
    time: f32,
    id: u32,
    frames_per_second: f32,
    frame_count: usize,
    reduced: bool,
) -> usize {
    assert!(frame_count > 0, "an authored loop needs at least one frame");
    if reduced || frame_count == 1 {
        return 0;
    }
    let tick = (time.max(0.0) * frames_per_second.max(0.0)).floor() as u64;
    ((tick + u64::from(id).wrapping_mul(7)) % frame_count as u64) as usize
}

/// Selects a construction sprite's progress stage and ambient machinery phase.
/// Public site progress chooses the stage; no builder order or other private
/// activity can leak through presentation. Reduced motion holds the crane.
pub(crate) fn construction_frame(
    progress: u32,
    total: u32,
    time: f32,
    id: u32,
    reduced: bool,
) -> (usize, usize) {
    let stage = ((u64::from(progress) * 3) / u64::from(total.max(1))).min(2) as usize;
    let phase = loop_frame(time, id, 3.0, 2, reduced);
    (stage, phase)
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
    // A presentation stamp should never sit ahead of the presentation
    // clock, but clamping the envelope here keeps a seek/reset race from
    // exaggerating recoil or producing a flash brighter than authored.
    let shot_age = shot_age.max(0.0);
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
        assert_eq!(a, unit_pose(2.25, 8, true, false, false));
        assert_eq!(a, UnitPose::REST);
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
        assert!(air.thruster > 0.0);
    }

    #[test]
    fn air_and_ground_use_distinct_activity_cues() {
        let ground = unit_pose(0.4, 11, true, false, false);
        let air = unit_pose(0.4, 11, true, true, false);
        assert_eq!(ground, UnitPose::REST);
        assert_eq!(ground.thruster, 0.0);
        assert!(air.thruster > 0.0);
        assert_ne!(air.width_scale, ground.width_scale);
        assert_eq!(unit_pose(0.4, 11, false, true, false), UnitPose::REST);
    }

    #[test]
    fn pose_envelopes_stay_finite_and_inside_authored_bounds() {
        for id in [0, 1, 17, u16::MAX as u32] {
            for step in 0..=240 {
                let time = step as f32 / 24.0;
                for airborne in [false, true] {
                    let pose = unit_pose(time, id, true, airborne, false);
                    for value in [
                        pose.lateral,
                        pose.lift,
                        pose.width_scale,
                        pose.height_scale,
                        pose.thruster,
                    ] {
                        assert!(value.is_finite());
                    }
                    assert!((0.9..=1.1).contains(&pose.width_scale));
                    assert!((0.9..=1.1).contains(&pose.height_scale));
                    assert!((0.0..=1.0).contains(&pose.thruster));
                }
            }
        }
    }

    #[test]
    fn authored_loops_are_repeatable_offset_and_reduced_motion_safe() {
        assert_eq!(loop_frame(1.25, 7, 8.0, 3, false), 2);
        assert_eq!(loop_frame(1.25, 7, 8.0, 3, false), 2);
        assert_ne!(
            loop_frame(1.25, 7, 8.0, 3, false),
            loop_frame(1.25, 8, 8.0, 3, false)
        );
        assert_eq!(loop_frame(99.0, 42, 8.0, 3, true), 0);
        assert_eq!(loop_frame(-2.0, 0, 8.0, 3, false), 0);
    }

    #[test]
    fn construction_frames_advance_by_progress_but_freeze_the_machine_when_needed() {
        assert_eq!(construction_frame(0, 300, 1.0, 0, false).0, 0);
        assert_eq!(construction_frame(100, 300, 1.0, 0, false).0, 1);
        assert_eq!(construction_frame(200, 300, 1.0, 0, false).0, 2);
        assert_eq!(construction_frame(299, 300, 1.0, 0, false).0, 2);
        assert_ne!(
            construction_frame(100, 300, 9.0, 4, false).1,
            construction_frame(100, 300, 9.4, 4, false).1,
            "every visible site carries the ambient crane loop"
        );
        assert_eq!(construction_frame(100, 300, 9.0, 4, true), (1, 0));
    }

    #[test]
    fn reduced_mount_keeps_aim_and_shot_envelopes_are_bounded() {
        let live = mount_pose(BuildingKind::Bastion, 1.25, 0.05, false);
        assert_eq!(live.angle, 1.25);
        assert!(live.recoil > 0.0);
        assert!(live.flash > 0.0);

        let early = mount_pose(BuildingKind::Bastion, 1.25, -0.05, false);
        assert_eq!(early.recoil, 0.060);
        assert_eq!(early.flash, 1.0);

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
