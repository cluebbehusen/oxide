//! Pure selection of authored sprite frames from presentation state.
//!
//! The controller derives semantic activity from deterministic simulation
//! facts. This module maps those facts onto the approved atlas rows without
//! consulting wall time or changing gameplay state.

use oxide_sim::{BuildingKind, UnitKind};

use crate::presentation_animation::{
    AttackPhase, BuildingActivity, BuildingAnimationState, CargoState, LocomotionState,
    PropulsionState, UnitAnimationState, UnitWorkState, WeaponCycle,
};

const BUZZARD_CHARGE_THRESHOLD: f32 = 0.94;

/// A Harvester pose within one cargo-specific row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HarvesterPose {
    /// Resting chassis and scoop.
    Idle,
    /// One of the two tread phases.
    Moving(usize),
    /// One of the two lowered-scoop phases.
    Scoop(usize),
}

/// An Excavator chassis pose beneath its independent cargo meter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExcavatorPose {
    /// Resting chassis and raised milling drum.
    Idle,
    /// One of the two tread phases.
    Moving(usize),
    /// One of the four milling-drum work phases.
    Working(usize),
}

/// The atlas row selected for a unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnitFrame {
    /// Ordinary resting art.
    Idle,
    /// One of the two authored locomotion frames.
    Moving(usize),
    /// Zero-based `_actionN` frame.
    Action(usize),
    /// Cargo-aware Harvester art.
    Harvester {
        /// One of five load levels.
        cargo: usize,
        /// Scoop or tread mechanism state.
        pose: HarvesterPose,
    },
    /// Cargo-aware Excavator art.
    Excavator {
        /// One of five authoritative load-fraction levels.
        cargo: usize,
        /// Milling-drum or tread mechanism state.
        pose: ExcavatorPose,
    },
}

/// The atlas rows selected for a building and its optional rotating mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BuildingFrame {
    /// Main footprint art.
    pub(crate) body: BuildingBodyFrame,
    /// Zero-based defense-mount `_actionN`; `None` uses its base row.
    pub(crate) mount_action: Option<usize>,
}

/// Main footprint art for a building.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuildingBodyFrame {
    /// Completed, inactive base art.
    Idle,
    /// Zero-based `_workN` frame.
    Work(usize),
    /// Construction stage and machinery phase.
    Construction { stage: usize, phase: usize },
    /// Zero-based `_actionN` frame. Bastion uses this for its charge rack.
    Action(usize),
}

/// Selects a unit frame with action transients taking precedence over every
/// concurrent state.
pub(crate) fn unit_frame(kind: UnitKind, state: UnitAnimationState) -> UnitFrame {
    if let Some(attack) = state.attack {
        return UnitFrame::Action(unit_attack_frame(kind, attack));
    }

    if kind == UnitKind::Harvester {
        let cargo = state.cargo.map_or(0, cargo_bucket);
        let pose = match state.work {
            UnitWorkState::Harvesting { cycle, .. }
            | UnitWorkState::Constructing { cycle, .. }
            | UnitWorkState::Repairing { cycle, .. }
            | UnitWorkState::Salvaging { cycle, .. } => harvester_work_frame(cycle),
            UnitWorkState::Idle => match state.locomotion {
                LocomotionState::Moving { cycle } => HarvesterPose::Moving(cycle_index(cycle, 2)),
                LocomotionState::Rest => HarvesterPose::Idle,
            },
        };
        return UnitFrame::Harvester { cargo, pose };
    }

    if kind == UnitKind::Excavator {
        let cargo = state.cargo.map_or(0, cargo_bucket);
        let pose = match state.work {
            UnitWorkState::Harvesting { cycle, .. }
            | UnitWorkState::Constructing { cycle, .. }
            | UnitWorkState::Repairing { cycle, .. }
            | UnitWorkState::Salvaging { cycle, .. } => excavator_work_frame(cycle),
            UnitWorkState::Idle => match state.locomotion {
                LocomotionState::Moving { cycle } => ExcavatorPose::Moving(cycle_index(cycle, 2)),
                LocomotionState::Rest => ExcavatorPose::Idle,
            },
        };
        return UnitFrame::Excavator { cargo, pose };
    }

    if kind == UnitKind::Tender
        && let UnitWorkState::Repairing { cycle, .. } = state.work
    {
        return tender_work_frame(cycle);
    }

    if let LocomotionState::Moving { cycle } = state.locomotion {
        return match state.propulsion {
            PropulsionState::LiftRotors { cycle } => lift_rotor_frame(kind, cycle),
            PropulsionState::None => UnitFrame::Moving(cycle_index(cycle, 2)),
        };
    }
    let preparation = preparation_progress(&state.weapons);
    if kind == UnitKind::Buzzard
        && preparation.is_some_and(|progress| progress >= BUZZARD_CHARGE_THRESHOLD)
    {
        return UnitFrame::Action(0);
    }
    if let PropulsionState::LiftRotors { cycle } = state.propulsion {
        return lift_rotor_frame(kind, cycle);
    }
    preparation.map_or(UnitFrame::Idle, |progress| {
        UnitFrame::Action(unit_preparation_frame(kind, progress))
    })
}

fn lift_rotor_frame(kind: UnitKind, cycle: f32) -> UnitFrame {
    if kind == UnitKind::Buzzard {
        match cycle_index(cycle, 3) {
            0 => UnitFrame::Idle,
            phase => UnitFrame::Moving(phase - 1),
        }
    } else {
        UnitFrame::Moving(cycle_index(cycle, 2))
    }
}

/// Selects the complete building frame, keeping Bastion's fixed charge rack
/// synchronized with its rotating mount.
pub(crate) fn building_frame(kind: BuildingKind, state: BuildingAnimationState) -> BuildingFrame {
    if let Some(site) = state.construction {
        return BuildingFrame {
            body: BuildingBodyFrame::Construction {
                stage: cycle_index(site.progress, 3),
                phase: if site.active {
                    cycle_index(site.machinery_cycle, 2)
                } else {
                    0
                },
            },
            mount_action: None,
        };
    }

    if kind.base_stats().weapons.is_empty() {
        let body = match state.activity {
            BuildingActivity::Idle => BuildingBodyFrame::Idle,
            BuildingActivity::Production { cycle, .. } => {
                BuildingBodyFrame::Work(cycle_index(cycle, 4))
            }
            BuildingActivity::ArraySweep { cycle } => {
                BuildingBodyFrame::Work(cycle_index(cycle, 6))
            }
            BuildingActivity::Reclaiming { cycle } => {
                BuildingBodyFrame::Work(cycle_index(cycle, 3))
            }
            BuildingActivity::RepairPulse { progress } => {
                BuildingBodyFrame::Work(cycle_index(progress, 4))
            }
        };
        return BuildingFrame {
            body,
            mount_action: None,
        };
    }

    let action = state
        .attack
        .map(|attack| defense_attack_frame(kind, attack))
        .or_else(|| match state.weapon {
            Some(WeaponCycle::Preparing { progress }) => {
                Some(defense_preparation_frame(kind, progress))
            }
            Some(WeaponCycle::Ready | WeaponCycle::Unavailable) | None => None,
        });
    BuildingFrame {
        body: match (kind, action) {
            (BuildingKind::Bastion, Some(frame)) => BuildingBodyFrame::Action(frame),
            _ => BuildingBodyFrame::Idle,
        },
        mount_action: action,
    }
}

fn cargo_bucket(cargo: CargoState) -> usize {
    ((cargo.fill.clamp(0.0, 1.0) * 4.0).round() as usize).min(4)
}

fn harvester_work_frame(cycle: f32) -> HarvesterPose {
    match cycle_index(cycle, 5) {
        0 | 4 => HarvesterPose::Idle,
        1 | 3 => HarvesterPose::Scoop(0),
        _ => HarvesterPose::Scoop(1),
    }
}

fn excavator_work_frame(cycle: f32) -> ExcavatorPose {
    match cycle_index(cycle, 5) {
        0 => ExcavatorPose::Idle,
        phase => ExcavatorPose::Working(phase - 1),
    }
}

fn tender_work_frame(cycle: f32) -> UnitFrame {
    match cycle_index(cycle, 5) {
        0 => UnitFrame::Idle,
        phase => UnitFrame::Action(phase - 1),
    }
}

fn preparation_progress(weapons: &[WeaponCycle]) -> Option<f32> {
    weapons
        .iter()
        .filter_map(|cycle| match cycle {
            WeaponCycle::Preparing { progress } => Some(*progress),
            WeaponCycle::Unavailable | WeaponCycle::Ready => None,
        })
        .min_by(f32::total_cmp)
}

fn unit_preparation_frame(kind: UnitKind, progress: f32) -> usize {
    match kind {
        UnitKind::Lancer | UnitKind::Bombard => cycle_index(progress, 3),
        UnitKind::Flakhound => cycle_index(progress, 5),
        UnitKind::Sentinel
        | UnitKind::Scuttler
        | UnitKind::Stinger
        | UnitKind::Buzzard
        | UnitKind::Darter
        | UnitKind::Talon
        | UnitKind::Wisp
        | UnitKind::Warden
        | UnitKind::Shrike
        | UnitKind::Sylph
        | UnitKind::Condor
        | UnitKind::Moth
        | UnitKind::Breaker
        | UnitKind::Avalanche => 0,
        UnitKind::Harvester
        | UnitKind::Tender
        | UnitKind::Excavator
        | UnitKind::Kestrel
        | UnitKind::Gnat
        | UnitKind::Skyhook
        | UnitKind::Sapper => 0,
    }
}

fn unit_attack_frame(kind: UnitKind, attack: AttackPhase) -> usize {
    match attack {
        AttackPhase::Report { progress, .. } => match kind {
            UnitKind::Lancer | UnitKind::Bombard => 3,
            UnitKind::Flakhound => 5 + cycle_index(progress, 2),
            UnitKind::Sentinel
            | UnitKind::Scuttler
            | UnitKind::Stinger
            | UnitKind::Buzzard
            | UnitKind::Darter
            | UnitKind::Talon
            | UnitKind::Wisp
            | UnitKind::Warden
            | UnitKind::Shrike
            | UnitKind::Sylph
            | UnitKind::Condor
            | UnitKind::Breaker
            | UnitKind::Avalanche => 1,
            UnitKind::Moth => cycle_index(progress, 3),
            UnitKind::Harvester
            | UnitKind::Tender
            | UnitKind::Excavator
            | UnitKind::Kestrel
            | UnitKind::Gnat
            | UnitKind::Skyhook
            | UnitKind::Sapper => 0,
        },
        AttackPhase::Recover { progress, .. } => match kind {
            UnitKind::Lancer | UnitKind::Bombard => 4 + cycle_index(progress, 2),
            UnitKind::Flakhound => 7 + cycle_index(progress, 2),
            UnitKind::Sentinel
            | UnitKind::Scuttler
            | UnitKind::Stinger
            | UnitKind::Buzzard
            | UnitKind::Darter
            | UnitKind::Talon
            | UnitKind::Wisp
            | UnitKind::Warden
            | UnitKind::Shrike
            | UnitKind::Sylph
            | UnitKind::Condor
            | UnitKind::Breaker
            | UnitKind::Avalanche => 2 + cycle_index(progress, 2),
            UnitKind::Moth => 3 + cycle_index(progress, 3),
            UnitKind::Harvester
            | UnitKind::Tender
            | UnitKind::Excavator
            | UnitKind::Kestrel
            | UnitKind::Gnat
            | UnitKind::Skyhook
            | UnitKind::Sapper => 0,
        },
    }
}

fn defense_preparation_frame(kind: BuildingKind, progress: f32) -> usize {
    match kind {
        BuildingKind::Turret => 2,
        BuildingKind::FlakTurret => cycle_index(progress, 4),
        BuildingKind::Bastion => cycle_index(progress, 5),
        _ => 0,
    }
}

fn defense_attack_frame(kind: BuildingKind, attack: AttackPhase) -> usize {
    match attack {
        AttackPhase::Report { progress, .. } => match kind {
            BuildingKind::Turret => 0,
            BuildingKind::FlakTurret => 4 + cycle_index(progress, 2),
            BuildingKind::Bastion => 5,
            _ => 0,
        },
        AttackPhase::Recover { progress, .. } => match kind {
            BuildingKind::Turret => 1,
            BuildingKind::FlakTurret => 6,
            BuildingKind::Bastion => 6 + cycle_index(progress, 2),
            _ => 0,
        },
    }
}

fn cycle_index(progress: f32, count: usize) -> usize {
    debug_assert!(count > 0);
    ((progress.clamp(0.0, 1.0) * count as f32).floor() as usize).min(count - 1)
}

#[cfg(test)]
mod tests {
    use oxide_sim::stats::MAX_WEAPONS;

    use super::*;
    use crate::presentation_animation::{ConstructionState, PropulsionState};

    fn unit_state() -> UnitAnimationState {
        UnitAnimationState {
            locomotion: LocomotionState::Rest,
            work: UnitWorkState::Idle,
            cargo: None,
            attack: None,
            weapons: [WeaponCycle::Unavailable; MAX_WEAPONS],
            propulsion: PropulsionState::None,
        }
    }

    fn building_state() -> BuildingAnimationState {
        BuildingAnimationState {
            construction: None,
            activity: BuildingActivity::Idle,
            attack: None,
            weapon: None,
        }
    }

    #[test]
    fn first_shot_ready_never_invents_a_windup() {
        let mut state = unit_state();
        state.weapons[0] = WeaponCycle::Ready;
        assert_eq!(unit_frame(UnitKind::Lancer, state), UnitFrame::Idle);

        let mut defense = building_state();
        defense.weapon = Some(WeaponCycle::Ready);
        assert_eq!(
            building_frame(BuildingKind::Bastion, defense),
            BuildingFrame {
                body: BuildingBodyFrame::Idle,
                mount_action: None,
            }
        );
    }

    #[test]
    fn attack_overrides_cooldown_and_locomotion() {
        let mut state = unit_state();
        state.locomotion = LocomotionState::Moving { cycle: 0.75 };
        state.weapons[0] = WeaponCycle::Preparing { progress: 0.8 };
        state.attack = Some(AttackPhase::Report {
            weapon: 0,
            progress: 0.0,
        });
        assert_eq!(unit_frame(UnitKind::Lancer, state), UnitFrame::Action(3));
    }

    #[test]
    fn real_locomotion_keeps_every_reload_from_freezing_its_treads() {
        let mut state = unit_state();
        state.locomotion = LocomotionState::Moving { cycle: 0.75 };
        state.weapons[0] = WeaponCycle::Preparing { progress: 0.8 };
        for kind in [
            UnitKind::Sentinel,
            UnitKind::Lancer,
            UnitKind::Bombard,
            UnitKind::Flakhound,
        ] {
            assert_eq!(unit_frame(kind, state), UnitFrame::Moving(1));
        }
    }

    #[test]
    fn newest_sentinel_weapon_preparation_is_selected() {
        let mut state = unit_state();
        state.weapons = [
            WeaponCycle::Preparing { progress: 0.8 },
            WeaponCycle::Preparing { progress: 0.1 },
        ];
        assert_eq!(unit_frame(UnitKind::Sentinel, state), UnitFrame::Action(0));
    }

    #[test]
    fn cargo_buckets_are_monotonic_and_fill_only_near_capacity() {
        let buckets = (0..=10)
            .map(|amount| {
                cargo_bucket(CargoState {
                    amount,
                    capacity: 10,
                    fill: amount as f32 / 10.0,
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(buckets[0], 0);
        assert_eq!(buckets[1], 0);
        assert_eq!(buckets[8], 3);
        assert_eq!(buckets[9], 4);
        assert_eq!(buckets[10], 4);
        assert!(buckets.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn harvester_work_and_motion_retain_the_cargo_bucket() {
        let mut state = unit_state();
        state.cargo = Some(CargoState {
            amount: 5,
            capacity: 10,
            fill: 0.5,
        });
        state.work = UnitWorkState::Harvesting {
            target: chassis::grid::TilePos::new(5, 5).center(),
            cycle: 0.5,
        };
        assert_eq!(
            unit_frame(UnitKind::Harvester, state),
            UnitFrame::Harvester {
                cargo: 2,
                pose: HarvesterPose::Scoop(1),
            }
        );
        state.work = UnitWorkState::Repairing {
            target: chassis::grid::TilePos::new(6, 5).center(),
            cycle: 0.25,
        };
        assert!(matches!(
            unit_frame(UnitKind::Harvester, state),
            UnitFrame::Harvester {
                cargo: 2,
                pose: HarvesterPose::Scoop(_),
            }
        ));
        state.work = UnitWorkState::Constructing {
            site: oxide_sim::BuildingId(7),
            target: chassis::grid::TilePos::new(7, 5).center(),
            cycle: 0.5,
        };
        assert_eq!(
            unit_frame(UnitKind::Harvester, state),
            UnitFrame::Harvester {
                cargo: 2,
                pose: HarvesterPose::Scoop(1),
            }
        );
        state.work = UnitWorkState::Idle;
        state.locomotion = LocomotionState::Moving { cycle: 0.75 };
        assert_eq!(
            unit_frame(UnitKind::Harvester, state),
            UnitFrame::Harvester {
                cargo: 2,
                pose: HarvesterPose::Moving(1),
            }
        );
    }

    #[test]
    fn excavator_work_and_motion_retain_the_authoritative_cargo_bucket() {
        let mut state = unit_state();
        state.cargo = Some(CargoState {
            amount: 15,
            capacity: 30,
            fill: 0.5,
        });
        state.work = UnitWorkState::Harvesting {
            target: chassis::grid::TilePos::new(5, 5).center(),
            cycle: 0.7,
        };
        assert_eq!(
            unit_frame(UnitKind::Excavator, state),
            UnitFrame::Excavator {
                cargo: 2,
                pose: ExcavatorPose::Working(2),
            }
        );

        state.work = UnitWorkState::Idle;
        state.locomotion = LocomotionState::Moving { cycle: 0.75 };
        assert_eq!(
            unit_frame(UnitKind::Excavator, state),
            UnitFrame::Excavator {
                cargo: 2,
                pose: ExcavatorPose::Moving(1),
            }
        );
    }

    #[test]
    fn tender_welds_only_while_real_repair_work_is_active() {
        let mut state = unit_state();
        state.work = UnitWorkState::Repairing {
            target: chassis::grid::TilePos::new(6, 5).center(),
            cycle: 0.7,
        };
        assert_eq!(unit_frame(UnitKind::Tender, state), UnitFrame::Action(2));

        state.work = UnitWorkState::Idle;
        assert_eq!(unit_frame(UnitKind::Tender, state), UnitFrame::Idle);

        state.locomotion = LocomotionState::Moving { cycle: 0.75 };
        assert_eq!(unit_frame(UnitKind::Tender, state), UnitFrame::Moving(1));
    }

    #[test]
    fn lift_rotors_run_at_rest_and_attacks_override_them() {
        let mut state = unit_state();
        state.propulsion = PropulsionState::LiftRotors { cycle: 0.75 };
        for kind in [UnitKind::Buzzard, UnitKind::Wisp] {
            assert_eq!(unit_frame(kind, state), UnitFrame::Moving(1));
        }

        state.attack = Some(AttackPhase::Report {
            weapon: 0,
            progress: 0.0,
        });
        assert_eq!(unit_frame(UnitKind::Buzzard, state), UnitFrame::Action(1));

        state.attack = None;
        state.propulsion = PropulsionState::LiftRotors { cycle: 0.0 };
        assert_eq!(unit_frame(UnitKind::Buzzard, state), UnitFrame::Idle);
        assert_eq!(unit_frame(UnitKind::Wisp, state), UnitFrame::Moving(0));
    }

    #[test]
    fn buzzard_rotors_use_the_approved_three_phase_loop_at_rest_and_in_motion() {
        let mut state = unit_state();
        for (cycle, expected) in [
            (0.0, UnitFrame::Idle),
            (0.34, UnitFrame::Moving(0)),
            (0.67, UnitFrame::Moving(1)),
        ] {
            state.propulsion = PropulsionState::LiftRotors { cycle };
            assert_eq!(unit_frame(UnitKind::Buzzard, state), expected);
            state.locomotion = LocomotionState::Moving { cycle: 0.99 };
            assert_eq!(unit_frame(UnitKind::Buzzard, state), expected);
            state.locomotion = LocomotionState::Rest;
        }
    }

    #[test]
    fn buzzard_only_holds_its_gun_ready_near_the_end_of_cooldown() {
        let mut state = unit_state();
        state.propulsion = PropulsionState::LiftRotors { cycle: 0.75 };
        state.weapons[0] = WeaponCycle::Preparing { progress: 0.93 };
        assert_eq!(unit_frame(UnitKind::Buzzard, state), UnitFrame::Moving(1));

        state.weapons[0] = WeaponCycle::Preparing { progress: 0.94 };
        assert_eq!(unit_frame(UnitKind::Buzzard, state), UnitFrame::Action(0));
    }

    #[test]
    fn every_unit_action_row_stays_inside_its_contract() {
        let action_counts = [
            (UnitKind::Sentinel, 4),
            (UnitKind::Scuttler, 4),
            (UnitKind::Lancer, 6),
            (UnitKind::Bombard, 6),
            (UnitKind::Flakhound, 9),
            (UnitKind::Stinger, 4),
            (UnitKind::Buzzard, 4),
            (UnitKind::Darter, 4),
            (UnitKind::Talon, 4),
            (UnitKind::Wisp, 4),
        ];
        for (kind, count) in action_counts {
            for progress in [0.0, 0.25, 0.5, 0.75, 1.0] {
                for attack in [
                    AttackPhase::Report {
                        weapon: 0,
                        progress,
                    },
                    AttackPhase::Recover {
                        weapon: 0,
                        progress,
                    },
                ] {
                    let frame = unit_attack_frame(kind, attack);
                    assert!(frame < count, "{kind:?} selected action {frame}");
                }
                assert!(unit_preparation_frame(kind, progress) < count);
            }
        }
    }

    #[test]
    fn moth_uses_all_six_payload_frames_across_report_and_recovery() {
        for (progress, expected) in [(0.0, 0), (0.34, 1), (0.67, 2)] {
            assert_eq!(
                unit_attack_frame(
                    UnitKind::Moth,
                    AttackPhase::Report {
                        weapon: 0,
                        progress,
                    },
                ),
                expected,
            );
        }
        for (progress, expected) in [(0.0, 3), (0.34, 4), (0.67, 5)] {
            assert_eq!(
                unit_attack_frame(
                    UnitKind::Moth,
                    AttackPhase::Recover {
                        weapon: 0,
                        progress,
                    },
                ),
                expected,
            );
        }
    }

    #[test]
    fn flakhound_cooldown_refills_before_the_paired_report_frames() {
        for (progress, expected) in [(0.0, 0), (0.2, 1), (0.4, 2), (0.6, 3), (0.8, 4), (1.0, 4)] {
            assert_eq!(
                unit_preparation_frame(UnitKind::Flakhound, progress),
                expected
            );
        }
        assert_eq!(
            unit_attack_frame(
                UnitKind::Flakhound,
                AttackPhase::Report {
                    weapon: 0,
                    progress: 0.0,
                },
            ),
            5
        );
        assert_eq!(
            unit_attack_frame(
                UnitKind::Flakhound,
                AttackPhase::Recover {
                    weapon: 0,
                    progress: 1.0,
                },
            ),
            8
        );
    }

    #[test]
    fn building_activity_uses_only_its_authored_row() {
        let mut state = building_state();
        state.activity = BuildingActivity::Production {
            unit: UnitKind::Sentinel,
            progress: 0.01,
            cycle: 0.76,
        };
        assert_eq!(
            building_frame(BuildingKind::Foundry, state).body,
            BuildingBodyFrame::Work(3)
        );
        state.activity = BuildingActivity::ArraySweep { cycle: 0.99 };
        assert_eq!(
            building_frame(BuildingKind::Array, state).body,
            BuildingBodyFrame::Work(5)
        );
        state.activity = BuildingActivity::Reclaiming { cycle: 0.99 };
        assert_eq!(
            building_frame(BuildingKind::Reclaimer, state).body,
            BuildingBodyFrame::Work(2)
        );
    }

    #[test]
    fn idle_producers_and_repair_bays_hold_their_base() {
        for kind in [
            BuildingKind::Foundry,
            BuildingKind::Fabricator,
            BuildingKind::RepairBay,
        ] {
            assert_eq!(
                building_frame(kind, building_state()).body,
                BuildingBodyFrame::Idle
            );
        }
    }

    #[test]
    fn construction_progress_and_real_activity_choose_the_site_frame() {
        let mut state = building_state();
        state.construction = Some(ConstructionState {
            progress: 0.7,
            active: true,
            machinery_cycle: 0.75,
        });
        assert_eq!(
            building_frame(BuildingKind::Fabricator, state).body,
            BuildingBodyFrame::Construction { stage: 2, phase: 1 }
        );
        state.construction.as_mut().expect("site").active = false;
        assert_eq!(
            building_frame(BuildingKind::Fabricator, state).body,
            BuildingBodyFrame::Construction { stage: 2, phase: 0 }
        );
    }

    #[test]
    fn bastion_body_and_mount_actions_never_drift() {
        for progress in [0.0, 0.2, 0.5, 0.8, 1.0] {
            let mut state = building_state();
            state.weapon = Some(WeaponCycle::Preparing { progress });
            let selected = building_frame(BuildingKind::Bastion, state);
            assert_eq!(
                selected.body,
                BuildingBodyFrame::Action(selected.mount_action.expect("Bastion mount"))
            );
        }
    }

    #[test]
    fn defense_rows_cover_report_recovery_and_charge_boundaries() {
        let counts = [
            (BuildingKind::Turret, 4),
            (BuildingKind::FlakTurret, 8),
            (BuildingKind::Bastion, 9),
        ];
        for (kind, count) in counts {
            for progress in [0.0, 0.25, 0.5, 0.75, 1.0] {
                assert!(defense_preparation_frame(kind, progress) < count);
                assert!(
                    defense_attack_frame(
                        kind,
                        AttackPhase::Report {
                            weapon: 0,
                            progress,
                        },
                    ) < count
                );
                assert!(
                    defense_attack_frame(
                        kind,
                        AttackPhase::Recover {
                            weapon: 0,
                            progress,
                        },
                    ) < count
                );
            }
        }
    }
}
