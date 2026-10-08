//! Pure selection of authored sprite frames from presentation state.
//!
//! The controller derives semantic activity from deterministic simulation
//! facts. This module maps those facts onto the approved atlas rows without
//! consulting wall time or changing gameplay state.

use crate::numeric;
use oxide_sim::{BuildingKind, UnitKind};

use crate::presentation_animation::{
    AttackPhase, BuildingActivity, BuildingAnimationState, CargoState, ExcavatorTool,
    LocomotionState, PropulsionState, TransportActionState, UnitAnimationState, UnitWorkState,
    WeaponCycle,
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
    if kind == UnitKind::Skyhook
        && let Some(action) = state.transport
    {
        return skyhook_transport_frame(action);
    }

    if let Some(attack) = state.attack {
        return UnitFrame::Action(unit_attack_frame(kind, attack));
    }
    if matches!(kind, UnitKind::Avalanche | UnitKind::Moth)
        && let Some(progress) = preparation_progress(&state.weapons)
    {
        return UnitFrame::Action(unit_preparation_frame(kind, progress));
    }

    if kind == UnitKind::Sapper
        && let Some(progress) = state.demolition_preparation
    {
        return UnitFrame::Action(cycle_index(progress, 3));
    }

    if kind == UnitKind::Harvester {
        let cargo = state.cargo.map_or(0, cargo_bucket);
        let pose = match state.work {
            UnitWorkState::Harvesting { cycle, .. }
            | UnitWorkState::Constructing { cycle, .. }
            | UnitWorkState::Repairing { cycle, .. }
            | UnitWorkState::Salvaging { cycle, .. } => harvester_work_frame(cycle),
            UnitWorkState::Unloading { .. } => harvester_work_frame(0.75),
            UnitWorkState::Idle => match state.locomotion {
                LocomotionState::Moving { cycle } => match tread_phase(cycle) {
                    0 => HarvesterPose::Idle,
                    phase => HarvesterPose::Moving(phase - 1),
                },
                LocomotionState::Rest => HarvesterPose::Idle,
            },
        };
        return UnitFrame::Harvester { cargo, pose };
    }

    if kind == UnitKind::Excavator {
        let cargo = state.cargo.map_or(0, cargo_bucket);
        let pose = match state.work.excavator_tool() {
            ExcavatorTool::Drum { cycle } => excavator_work_frame(cycle),
            ExcavatorTool::WeldingArm { .. } | ExcavatorTool::Stowed => match state.locomotion {
                LocomotionState::Moving { cycle } => match tread_phase(cycle) {
                    0 => ExcavatorPose::Idle,
                    phase => ExcavatorPose::Moving(phase - 1),
                },
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
            PropulsionState::LiftRotors { cycle } => lift_rotor_frame(cycle),
            PropulsionState::None if has_treads(kind) => match tread_phase(cycle) {
                0 => UnitFrame::Idle,
                phase => UnitFrame::Moving(phase - 1),
            },
            PropulsionState::None if matches!(kind, UnitKind::Scuttler | UnitKind::Sapper) => {
                match cycle_index(cycle, 4) {
                    0 => UnitFrame::Moving(0),
                    2 => UnitFrame::Moving(1),
                    _ => UnitFrame::Idle,
                }
            }
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
        return lift_rotor_frame(cycle);
    }
    preparation.map_or(UnitFrame::Idle, |progress| {
        UnitFrame::Action(unit_preparation_frame(kind, progress))
    })
}

fn has_treads(kind: UnitKind) -> bool {
    matches!(
        kind,
        UnitKind::Sentinel
            | UnitKind::Warden
            | UnitKind::Lancer
            | UnitKind::Breaker
            | UnitKind::Avalanche
            | UnitKind::Bombard
            | UnitKind::Flakhound
            | UnitKind::Stinger
            | UnitKind::Tender
    )
}

/// Base, tread one, tread two form one forward belt loop.
pub(super) fn tread_phase(cycle: f32) -> usize {
    cycle_index(cycle, 3)
}

fn lift_rotor_frame(cycle: f32) -> UnitFrame {
    match cycle_index(cycle, 3) {
        0 => UnitFrame::Idle,
        phase => UnitFrame::Moving(phase - 1),
    }
}

/// Independent weapon rows retain their cycle while the chassis travels.
pub(crate) fn unit_mount_frame(kind: UnitKind, mut state: UnitAnimationState) -> UnitFrame {
    state.locomotion = LocomotionState::Rest;
    unit_frame(kind, state)
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
                let frames = match kind {
                    BuildingKind::Airworks => 2,
                    BuildingKind::Foundry => 12,
                    _ => 4,
                };
                BuildingBodyFrame::Work(cycle_index(cycle, frames))
            }
            BuildingActivity::AirworksLaunch { progress } => {
                BuildingBodyFrame::Work(2 + cycle_index(progress, 2))
            }
            BuildingActivity::ArraySweep { cycle } => {
                BuildingBodyFrame::Work(cycle_index(cycle, 6))
            }
            BuildingActivity::Extracting { cycle } => {
                BuildingBodyFrame::Work(cycle_index(cycle, 4))
            }
            BuildingActivity::Reclaiming { cycle } => {
                BuildingBodyFrame::Work(cycle_index(cycle, 12))
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
    numeric::to_usize((cargo.fill.clamp(0.0, 1.0) * 4.0).round()).min(4)
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

fn skyhook_transport_frame(action: TransportActionState) -> UnitFrame {
    let action = match action {
        TransportActionState::Boarding { progress } => cycle_index(progress, 4),
        TransportActionState::Unloading { progress } => 3 - cycle_index(progress, 4),
    };
    UnitFrame::Action(action)
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
        UnitKind::Moth => {
            if progress < 0.84 {
                2
            } else if progress < 0.90 {
                3
            } else if progress < 0.96 {
                4
            } else {
                5
            }
        }
        UnitKind::Avalanche => {
            if progress < 0.78 {
                2
            } else if progress < 0.95 {
                3
            } else {
                0
            }
        }
        UnitKind::Lancer => {
            if progress < 0.82 {
                0
            } else if progress < 0.94 {
                1
            } else {
                2
            }
        }
        UnitKind::Bombard => cycle_index(progress, 3),
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
        | UnitKind::Breaker => 0,
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
            UnitKind::Condor => {
                if progress < 1.0 / 3.0 {
                    3
                } else {
                    1
                }
            }
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
            | UnitKind::Breaker
            | UnitKind::Avalanche => 1,
            UnitKind::Moth => 0,
            UnitKind::Harvester
            | UnitKind::Tender
            | UnitKind::Excavator
            | UnitKind::Kestrel
            | UnitKind::Gnat
            | UnitKind::Skyhook
            | UnitKind::Sapper => 0,
        },
        AttackPhase::Recover { progress, .. } => match kind {
            UnitKind::Avalanche => 2,
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
            | UnitKind::Breaker => 2 + cycle_index(progress, 2),
            UnitKind::Moth => 1,
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
    numeric::to_usize((progress.clamp(0.0, 1.0) * count as f32).floor()).min(count - 1)
}

#[cfg(test)]
mod tests;
