use oxide_sim::stats::MAX_WEAPONS;

use super::*;
use crate::presentation_animation::{ConstructionState, PropulsionState};

#[test]
fn every_production_pose_resolves_to_authored_work_art() {
    use crate::presentation_animation::{BuildingActivity, BuildingAnimationState};
    let atlas: serde_json::Value =
        serde_json::from_str(include_str!("../../../../assets/sprites/atlas.json")).unwrap();
    for kind in BuildingKind::ALL {
        assert_eq!(
            production_frames(kind).is_some(),
            !kind.base_stats().produces.is_empty(),
            "{kind:?}"
        );
        if production_frames(kind).is_none() {
            continue;
        }
        for step in 0..=120 {
            let state = BuildingAnimationState {
                construction: None,
                attack: None,
                weapon: None,
                activity: BuildingActivity::Production {
                    unit: UnitKind::Sentinel,
                    progress: 0.5,
                    cycle: step as f32 / 120.0,
                },
            };
            let BuildingBodyFrame::Work(frame) = building_frame(kind, state).body else {
                panic!("producer has no work pose")
            };
            assert!(
                atlas
                    .get(format!("{}_work{}", kind.name(), frame + 1))
                    .is_some(),
                "{kind:?} selects missing work pose {frame}"
            );
        }
    }
}

#[test]
fn walking_legs_pass_through_neutral_between_opposing_steps() {
    for kind in [UnitKind::Scuttler, UnitKind::Sapper] {
        let mut state = unit_state();
        let frames = [0.0, 0.25, 0.5, 0.75, 1.0].map(|cycle| {
            state.locomotion = LocomotionState::Moving { cycle };
            unit_frame(kind, state)
        });
        assert_eq!(
            frames,
            [
                UnitFrame::Moving(0),
                UnitFrame::Idle,
                UnitFrame::Moving(1),
                UnitFrame::Idle,
                UnitFrame::Idle
            ]
        );
    }
}

#[test]
fn tread_loop_includes_the_base_phase_instead_of_reversing_between_two_frames() {
    for kind in [
        UnitKind::Sentinel,
        UnitKind::Warden,
        UnitKind::Lancer,
        UnitKind::Breaker,
        UnitKind::Avalanche,
        UnitKind::Bombard,
        UnitKind::Flakhound,
        UnitKind::Tender,
    ] {
        let mut state = unit_state();
        let frames = [0.0, 0.34, 0.67, 0.99, 0.0].map(|cycle| {
            state.locomotion = LocomotionState::Moving { cycle };
            unit_frame(kind, state)
        });
        assert_eq!(
            frames,
            [
                UnitFrame::Idle,
                UnitFrame::Moving(0),
                UnitFrame::Moving(1),
                UnitFrame::Moving(1),
                UnitFrame::Idle,
            ],
            "{kind:?}",
        );
    }
    for (cycle, expected) in [(0.0, 0), (0.34, 1), (0.67, 2)] {
        let mut state = unit_state();
        state.locomotion = LocomotionState::Moving { cycle };
        assert_eq!(tread_phase(cycle), expected);
        assert_eq!(
            unit_frame(UnitKind::Harvester, state),
            UnitFrame::Harvester {
                cargo: 0,
                pose: if expected == 0 {
                    HarvesterPose::Idle
                } else {
                    HarvesterPose::Moving(expected - 1)
                },
            },
        );
        assert_eq!(
            unit_frame(UnitKind::Excavator, state),
            UnitFrame::Excavator {
                cargo: 0,
                pose: if expected == 0 {
                    ExcavatorPose::Idle
                } else {
                    ExcavatorPose::Moving(expected - 1)
                },
            },
        );
    }
}

fn unit_state() -> UnitAnimationState {
    UnitAnimationState {
        locomotion: LocomotionState::Rest,
        work: UnitWorkState::Idle,
        work_target: None,
        welding_arm: None,
        cargo: None,
        attack: None,
        weapons: [WeaponCycle::Unavailable; MAX_WEAPONS],
        propulsion: PropulsionState::None,
        scanner: None,
        transport: None,
        demolition_preparation: None,
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
fn articulated_mount_keeps_late_charge_and_recoil_while_tracks_move() {
    let mut state = unit_state();
    state.locomotion = LocomotionState::Moving { cycle: 0.75 };
    for (progress, frame) in [(0.1, 0), (0.8, 0), (0.9, 1), (0.98, 2)] {
        state.weapons[0] = WeaponCycle::Preparing { progress };
        assert_eq!(unit_frame(UnitKind::Lancer, state), UnitFrame::Moving(1));
        assert_eq!(
            unit_mount_frame(UnitKind::Lancer, state),
            UnitFrame::Action(frame)
        );
    }
    state.attack = Some(AttackPhase::Report {
        progress: 0.0,
        weapon: 0,
    });
    assert_eq!(
        unit_mount_frame(UnitKind::Lancer, state),
        UnitFrame::Action(3)
    );
    state.attack = Some(AttackPhase::Recover {
        progress: 0.1,
        weapon: 0,
    });
    assert_eq!(
        unit_mount_frame(UnitKind::Lancer, state),
        UnitFrame::Action(4)
    );
    state.attack = None;
    state.weapons[0] = WeaponCycle::Ready;
    assert_eq!(unit_mount_frame(UnitKind::Lancer, state), UnitFrame::Idle);
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

    for work in [
        UnitWorkState::Unloading {
            target: chassis::fx::Vec2Fx::ZERO,
            progress: 0.5,
        },
        UnitWorkState::Constructing {
            site: oxide_sim::BuildingId(7),
            target: chassis::fx::Vec2Fx::ZERO,
            cycle: 0.7,
        },
        UnitWorkState::Repairing {
            target: chassis::fx::Vec2Fx::ZERO,
            cycle: 0.7,
        },
    ] {
        state.work = work;
        assert_eq!(
            unit_frame(UnitKind::Excavator, state),
            UnitFrame::Excavator {
                cargo: 2,
                pose: ExcavatorPose::Idle
            },
            "the drum rests while {work:?}"
        );
    }

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
    for kind in [UnitKind::Buzzard, UnitKind::Skyhook] {
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
    assert_eq!(unit_frame(UnitKind::Skyhook, state), UnitFrame::Idle);
}

#[test]
fn skyhook_boarding_and_unloading_run_opposite_clamp_sequences() {
    let mut state = unit_state();
    for (progress, expected) in [
        (0.0, UnitFrame::Action(0)),
        (0.26, UnitFrame::Action(1)),
        (0.51, UnitFrame::Action(2)),
        (0.76, UnitFrame::Action(3)),
    ] {
        state.transport = Some(TransportActionState::Boarding { progress });
        assert_eq!(unit_frame(UnitKind::Skyhook, state), expected);
    }
    for (progress, expected) in [
        (0.0, UnitFrame::Action(3)),
        (0.26, UnitFrame::Action(2)),
        (0.51, UnitFrame::Action(1)),
        (0.76, UnitFrame::Action(0)),
    ] {
        state.transport = Some(TransportActionState::Unloading { progress });
        assert_eq!(unit_frame(UnitKind::Skyhook, state), expected);
    }
}

#[test]
fn sapper_braces_and_arms_only_after_reaching_contact() {
    let mut state = unit_state();
    state.demolition_preparation = Some(0.25);
    assert_eq!(unit_frame(UnitKind::Sapper, state), UnitFrame::Action(0));
    state.demolition_preparation = Some(0.5);
    assert_eq!(unit_frame(UnitKind::Sapper, state), UnitFrame::Action(1));
    state.demolition_preparation = Some(0.9);
    assert_eq!(unit_frame(UnitKind::Sapper, state), UnitFrame::Action(2));
    state.demolition_preparation = None;
    state.locomotion = LocomotionState::Moving { cycle: 0.6 };
    assert_eq!(unit_frame(UnitKind::Sapper, state), UnitFrame::Moving(1));
}

#[test]
fn lift_rotors_use_the_complete_three_phase_loop_at_rest_and_in_motion() {
    let mut state = unit_state();
    for (cycle, expected) in [
        (0.0, UnitFrame::Idle),
        (0.34, UnitFrame::Moving(0)),
        (0.67, UnitFrame::Moving(1)),
    ] {
        for kind in [UnitKind::Buzzard, UnitKind::Skyhook] {
            state.propulsion = PropulsionState::LiftRotors { cycle };
            assert_eq!(unit_frame(kind, state), expected);
            state.locomotion = LocomotionState::Moving { cycle: 0.99 };
            assert_eq!(unit_frame(kind, state), expected);
            state.locomotion = LocomotionState::Rest;
        }
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
    let action_counts = UnitKind::ALL
        .into_iter()
        .map(|kind| (kind, crate::assets::unit_action_frames(kind)))
        .filter(|(_, count)| *count > 0);
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
        BuildingBodyFrame::Work(9)
    );
    assert_eq!(
        building_frame(BuildingKind::Crucible, state).body,
        BuildingBodyFrame::Work(3)
    );
    assert_eq!(
        building_frame(BuildingKind::Airworks, state).body,
        BuildingBodyFrame::Work(1)
    );
    state.activity = BuildingActivity::AirworksLaunch { progress: 0.0 };
    assert_eq!(
        building_frame(BuildingKind::Airworks, state).body,
        BuildingBodyFrame::Work(2)
    );
    state.activity = BuildingActivity::AirworksLaunch { progress: 0.5 };
    assert_eq!(
        building_frame(BuildingKind::Airworks, state).body,
        BuildingBodyFrame::Work(3)
    );
    state.activity = BuildingActivity::ArraySweep { cycle: 0.99 };
    assert_eq!(
        building_frame(BuildingKind::Array, state).body,
        BuildingBodyFrame::Work(5)
    );
    state.activity = BuildingActivity::Extracting { cycle: 0.99 };
    assert_eq!(
        building_frame(BuildingKind::Extractor, state).body,
        BuildingBodyFrame::Work(3)
    );
    state.activity = BuildingActivity::Reclaiming { cycle: 0.99 };
    assert_eq!(
        building_frame(BuildingKind::Reclaimer, state).body,
        BuildingBodyFrame::Work(11)
    );
}

#[test]
fn idle_producers_and_repair_bays_hold_their_base() {
    for kind in [
        BuildingKind::Foundry,
        BuildingKind::Fabricator,
        BuildingKind::Crucible,
        BuildingKind::Airworks,
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
    use crate::assets::{building_stem, rung_stem, shipped_frames};
    for kind in BuildingKind::ALL {
        let Some(look) = crate::look::defense(kind) else {
            assert_eq!(defense_preparation_frame(kind, 0.5), None, "{kind:?}");
            continue;
        };
        // Every rung's mount, and a charge rack on the hull, must hold
        // every frame the gun selects.
        let mut counts: Vec<usize> = (0..kind.tiers().len())
            .map(|tier| shipped_frames(&rung_stem(look.mount, tier), "action"))
            .collect();
        if look.charge_rack {
            counts.push(shipped_frames(building_stem(kind), "action"));
        }
        let count = counts.into_iter().min().unwrap();
        for progress in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let frames = [
                defense_preparation_frame(kind, progress),
                defense_attack_frame(
                    kind,
                    AttackPhase::Report {
                        weapon: 0,
                        progress,
                    },
                ),
                defense_attack_frame(
                    kind,
                    AttackPhase::Recover {
                        weapon: 0,
                        progress,
                    },
                ),
            ];
            for frame in frames {
                assert!(frame.unwrap() < count, "{kind:?} at {progress}");
            }
        }
    }
}

#[test]
fn avalanche_stays_empty_through_most_of_reload_even_while_moving() {
    let mut state = unit_state();
    state.locomotion = LocomotionState::Moving { cycle: 0.5 };
    for (progress, frame) in [(0.1, 2), (0.77, 2), (0.8, 3), (0.96, 0)] {
        state.weapons[0] = WeaponCycle::Preparing { progress };
        assert_eq!(
            unit_frame(UnitKind::Avalanche, state),
            UnitFrame::Action(frame)
        );
    }
}
