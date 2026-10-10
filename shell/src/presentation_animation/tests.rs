use oxide_sim::TickReport;
use std::collections::VecDeque;

use chassis::fx::Vec2Fx;
use chassis::grid::TilePos;
use oxide_sim::scenario::{BuildingSpec, UnitSpec};
use oxide_sim::{Command, PlayerCommand, PlayerId, Scenario, Target};

use super::*;

fn unit_facts(kind: UnitKind) -> UnitAnimationFacts {
    UnitAnimationFacts {
        id: UnitId(7),
        kind,
        moved: false,
        work: UnitWorkFact::Idle,
        work_target: None,
        carrying: 0,
        demolition_contact: false,
        cooldowns: [0; MAX_WEAPONS],
    }
}

fn building_facts(kind: BuildingKind) -> BuildingAnimationFacts {
    BuildingAnimationFacts {
        id: BuildingId(9),
        kind,
        tier: 0,
        built: true,
        progress: 0,
        construction_total: kind
            .base_stats()
            .construction
            .map(|stats| stats.build_ticks),
        construction_active: false,
        production: None,
        cooldown: 0,
    }
}

fn point() -> Vec2Fx {
    TilePos::new(4, 4).center()
}

fn excavator_state(buildings: Vec<BuildingSpec>) -> State {
    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = 1_000;
    scenario.units = vec![UnitSpec {
        player: 0,
        kind: UnitKind::Excavator,
        x: 4,
        y: 6,
    }];
    scenario.buildings = buildings;
    scenario.build().expect("Excavator work scenario builds")
}

fn player_command(command: Command) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(0),
        command,
    }
}

fn unit_attack_report(tick: u64, attacker: UnitId, weapon: usize) -> TickReport {
    TickReport {
        tick,
        movement: Vec::new(),
        events: vec![Event::AttackHit {
            attacker,
            attacker_kind: UnitKind::Sentinel,
            weapon,
            target: Some(Target::Unit(UnitId(99))),
            attacker_pos: point(),
            target_pos: point(),
        }],
    }
}

#[test]
fn first_shot_is_ready_and_cooldown_prepares_only_the_next_shot() {
    let controller = AnimationController::default();
    let ready = controller.unit_state(
        unit_facts(UnitKind::Lancer),
        AnimationClock::new(0, 0.0),
        AnimationOptions::default(),
    );
    assert_eq!(ready.weapons[0], WeaponCycle::Ready);
    assert!(ready.attack.is_none());

    let mut cooling = unit_facts(UnitKind::Lancer);
    let total = UnitKind::Lancer.stats().weapons[0].cooldown_ticks;
    cooling.cooldowns[0] = total;
    let just_fired = controller.unit_state(
        cooling,
        AnimationClock::new(1, 0.0),
        AnimationOptions::default(),
    );
    assert_eq!(
        just_fired.weapons[0],
        WeaponCycle::Preparing { progress: 0.0 }
    );

    cooling.cooldowns[0] = total / 2;
    let halfway = controller.unit_state(
        cooling,
        AnimationClock::new(30, 0.0),
        AnimationOptions::default(),
    );
    assert!(matches!(
        halfway.weapons[0],
        WeaponCycle::Preparing { progress } if (progress - 0.5).abs() < 0.001
    ));
}

#[test]
fn damage_event_drives_one_report_then_recovery() {
    let mut controller = AnimationController::default();
    let report = &unit_attack_report(40, UnitId(7), 0);
    controller.observe_events(report.tick + 1, &report.events);
    let facts = unit_facts(UnitKind::Sentinel);

    let report = controller.unit_state(
        facts,
        AnimationClock::new(41, 0.5),
        AnimationOptions::default(),
    );
    assert!(matches!(
        report.attack,
        Some(AttackPhase::Report { weapon: 0, .. })
    ));

    let recovery = controller.unit_state(
        facts,
        AnimationClock::new(43, 0.5),
        AnimationOptions::default(),
    );
    assert!(matches!(
        recovery.attack,
        Some(AttackPhase::Recover { weapon: 0, .. })
    ));

    let settled = controller.unit_state(
        facts,
        AnimationClock::new(46, 0.0),
        AnimationOptions::default(),
    );
    assert!(settled.attack.is_none());
}

#[test]
fn boarding_and_unloading_events_drive_only_the_named_transport() {
    let mut controller = AnimationController::default();
    controller.observe_events(
        21,
        &[Event::UnitBoarded {
            transport: UnitId(7),
            unit: UnitId(8),
            player: PlayerId(0),
        }],
    );
    let skyhook = unit_facts(UnitKind::Skyhook);
    let boarding = controller.unit_state(
        skyhook,
        AnimationClock::new(24, 0.0),
        AnimationOptions::default(),
    );
    assert!(matches!(
        boarding.transport,
        Some(TransportActionState::Boarding { progress }) if (progress - 0.375).abs() < 0.001
    ));

    controller.observe_events(
        30,
        &[Event::UnitUnloaded {
            transport: UnitId(7),
            unit: UnitId(8),
            player: PlayerId(0),
            at: TilePos::new(4, 5),
        }],
    );
    let unloading = controller.unit_state(
        skyhook,
        AnimationClock::new(31, 0.0),
        AnimationOptions::default(),
    );
    assert!(matches!(
        unloading.transport,
        Some(TransportActionState::Unloading { progress }) if (progress - 0.125).abs() < 0.001
    ));
    let settled = controller.unit_state(
        skyhook,
        AnimationClock::new(38, 0.0),
        AnimationOptions::default(),
    );
    assert!(settled.transport.is_none());
}

#[test]
fn projectile_launch_drives_bombard_and_bastion_reports() {
    let mut controller = AnimationController::default();
    let report = &TickReport {
        tick: 8,
        movement: Vec::new(),
        events: vec![
            Event::ShellLaunched {
                unit_pose: None,
                shooter: Target::Unit(UnitId(7)),
                target: Some(Target::Unit(UnitId(8))),
                player: PlayerId(0),
                from: point(),
                to: point(),
                flight: 10,
            },
            Event::ShellLaunched {
                unit_pose: None,
                shooter: Target::Building(BuildingId(9)),
                target: Some(Target::Unit(UnitId(8))),
                player: PlayerId(0),
                from: point(),
                to: point(),
                flight: 10,
            },
        ],
    };
    controller.observe_events(report.tick + 1, &report.events);
    let clock = AnimationClock::new(9, 0.0);
    assert!(matches!(
        controller
            .unit_state(
                unit_facts(UnitKind::Bombard),
                clock,
                AnimationOptions::default()
            )
            .attack,
        Some(AttackPhase::Report { .. })
    ));
    assert!(matches!(
        controller
            .building_state(
                building_facts(BuildingKind::Bastion),
                clock,
                AnimationOptions::default()
            )
            .attack,
        Some(AttackPhase::Report { .. })
    ));
}

#[test]
fn bastion_report_is_a_single_hard_recoil_then_a_short_settle() {
    let timing = building_attack_timing(BuildingKind::Bastion);
    assert_eq!(timing.report_ticks, 1.0);
    assert_eq!(timing.recover_ticks, 3.0);
    assert!(matches!(
        attack_phase(0.99, 0, timing),
        Some(AttackPhase::Report { .. })
    ));
    assert!(matches!(
        attack_phase(1.0, 0, timing),
        Some(AttackPhase::Recover { progress: 0.0, .. })
    ));
    assert_eq!(attack_phase(4.0, 0, timing), None);
}

#[test]
fn paused_clock_holds_motion_and_lift_rotors_run_while_idle() {
    let controller = AnimationController::default();
    let clock = AnimationClock::new(77, 0.35);
    let options = AnimationOptions::default();
    for kind in [UnitKind::Buzzard, UnitKind::Wisp, UnitKind::Skyhook] {
        let a = controller.unit_state(unit_facts(kind), clock, options);
        let b = controller.unit_state(unit_facts(kind), clock, options);
        assert_eq!(a, b);
        assert_eq!(a.locomotion, LocomotionState::Rest);
        assert!(matches!(a.propulsion, PropulsionState::LiftRotors { .. }));

        let held = controller.unit_state(
            unit_facts(kind),
            AnimationClock::new(200, 0.9),
            AnimationOptions {
                reduced_motion: true,
            },
        );
        assert_eq!(held.propulsion, PropulsionState::LiftRotors { cycle: 0.0 });
    }
}

#[test]
fn array_sweep_preserves_fractional_motion_and_holds_the_paused_clock() {
    let controller = AnimationController::default();
    let facts = building_facts(BuildingKind::Array);
    let options = AnimationOptions::default();
    let activity = |fraction| {
        controller
            .building_state(facts, AnimationClock::new(12, fraction), options)
            .activity
    };
    let BuildingActivity::ArraySweep { cycle: first } = activity(0.0) else {
        panic!("built Array must sweep");
    };
    let BuildingActivity::ArraySweep { cycle: later } = activity(0.5) else {
        panic!("built Array must sweep");
    };
    assert!((later - first - 0.5 / ARRAY_SWEEP_PERIOD as f32).abs() < 0.00001);
    assert_eq!(activity(0.5), activity(0.5));
    assert_eq!(
        controller
            .building_state(
                facts,
                AnimationClock::new(99, 0.75),
                AnimationOptions {
                    reduced_motion: true
                }
            )
            .activity,
        BuildingActivity::ArraySweep { cycle: 0.0 }
    );
}

#[test]
fn scout_scanners_run_at_rest_without_restarting_with_movement() {
    let controller = AnimationController::default();
    let options = AnimationOptions::default();
    for kind in [UnitKind::Kestrel, UnitKind::Gnat] {
        let mut facts = unit_facts(kind);
        let clock = AnimationClock::new(12, 0.5);
        let idle = controller.unit_state(facts, clock, options);
        assert_eq!(idle.locomotion, LocomotionState::Rest);
        assert!(idle.scanner.is_some());
        let later = controller.unit_state(facts, AnimationClock::new(24, 0.5), options);
        assert_ne!(idle.scanner, later.scanner);
        facts.moved = true;
        assert_eq!(
            controller.unit_state(facts, clock, options).scanner,
            idle.scanner
        );
        assert_eq!(
            controller
                .unit_state(
                    facts,
                    clock,
                    AnimationOptions {
                        reduced_motion: true
                    }
                )
                .scanner,
            Some(0.0)
        );
    }
    assert_eq!(
        controller
            .unit_state(
                unit_facts(UnitKind::Sentinel),
                AnimationClock::new(12, 0.5),
                options
            )
            .scanner,
        None
    );
}

#[test]
fn lift_rotor_cadence_does_not_change_when_an_aircraft_starts_moving() {
    assert_eq!(unit_move_period(UnitKind::Buzzard), BUZZARD_ROTOR_PERIOD);
    assert_eq!(unit_move_period(UnitKind::Wisp), WISP_ROTOR_PERIOD);
    assert_eq!(unit_move_period(UnitKind::Skyhook), SKYHOOK_ROTOR_PERIOD);
}

#[test]
fn excavator_roller_ignores_scrap_meter_resets() {
    let controller = AnimationController::default();
    let mut facts = unit_facts(UnitKind::Excavator);
    let options = AnimationOptions::default();
    let target = TilePos::new(10, 10).center();
    let clock = AnimationClock::new(100, 0.5);
    facts.work = UnitWorkFact::Harvesting(target, 4);
    let before = controller.unit_state(facts, clock, options).work;
    facts.work = UnitWorkFact::Harvesting(target, 0);
    assert_eq!(before, controller.unit_state(facts, clock, options).work);
    assert_eq!(
        before,
        controller
            .unit_state(facts, AnimationClock::new(120, 0.5), options)
            .work
    );
    assert_ne!(
        before,
        controller
            .unit_state(facts, AnimationClock::new(105, 0.5), options)
            .work
    );
}

#[test]
fn welding_arm_history_does_not_survive_a_missing_worker_or_seek() {
    let state = Scenario::skirmish().build().unwrap();
    let mut controller = AnimationController::default();
    let mut facts = unit_facts(UnitKind::Excavator);
    facts.id = UnitId(u32::MAX);
    facts.work = UnitWorkFact::Repairing(TilePos::new(10, 10).center());
    facts.work_target = Some(WorkTarget::Building(BuildingId(4)));
    controller.observe_worker(facts, 0);
    assert!(controller.welding_arms.contains_key(&facts.id));
    controller.observe_workers(&state);
    assert!(!controller.welding_arms.contains_key(&facts.id));
    controller.observe_worker(facts, 0);
    controller.reset_workers(&state);
    assert!(!controller.welding_arms.contains_key(&facts.id));
}

#[test]
fn excavator_welder_folds_once_and_reverses_without_jumping() {
    let mut controller = AnimationController::default();
    let mut facts = unit_facts(UnitKind::Excavator);
    let target = WorkTarget::Building(BuildingId(4));
    facts.work = UnitWorkFact::Repairing(TilePos::new(10, 10).center());
    facts.work_target = Some(target);
    let options = AnimationOptions::default();
    controller.observe_worker(facts, 10);
    let arm = |controller: &AnimationController, facts, tick| {
        controller
            .unit_state(facts, AnimationClock::new(tick, 0.), options)
            .welding_arm
    };
    assert_eq!(arm(&controller, facts, 10).unwrap().deployment, 0.);
    assert_eq!(arm(&controller, facts, 16).unwrap().deployment, 0.5);
    controller.observe_worker(facts, 16);
    assert_eq!(arm(&controller, facts, 22).unwrap().deployment, 1.);
    assert_eq!(arm(&controller, facts, 30).unwrap().deployment, 1.);
    let working = facts;
    facts.work = UnitWorkFact::Idle;
    facts.work_target = None;
    controller.observe_worker(facts, 30);
    let retracting = arm(&controller, facts, 36).unwrap();
    assert_eq!(retracting.target, target);
    assert_eq!(retracting.deployment, 0.5);
    assert!(!retracting.active);
    assert_eq!(retracting, arm(&controller, facts, 36).unwrap());
    controller.observe_worker(working, 36);
    assert_eq!(arm(&controller, working, 36).unwrap().deployment, 0.5);
    assert_eq!(arm(&controller, working, 42).unwrap().deployment, 1.);
    controller.observe_worker(facts, 44);
    assert_eq!(arm(&controller, facts, 56), None);
    controller.reset_transients();
    assert_eq!(arm(&controller, facts, 15), None);
    assert_eq!(arm(&controller, working, 15).unwrap().deployment, 1.);
    assert_eq!(
        controller
            .unit_state(
                working,
                AnimationClock::new(15, 0.),
                AnimationOptions {
                    reduced_motion: true
                }
            )
            .welding_arm
            .unwrap()
            .deployment,
        1.
    );
}

#[test]
fn harvesting_requires_real_work_and_cargo_is_a_continuous_fill() {
    let state = Scenario::skirmish().build().expect("skirmish builds");
    let base = state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Harvester)
        .expect("skirmish starts a harvester")
        .clone();
    let node = (0..state.map().height())
        .flat_map(|y| (0..state.map().width()).map(move |x| TilePos::new(x, y)))
        .find(|tile| state.map().scrap_at(*tile) > 0)
        .expect("skirmish carries scrap");
    let mut unit = base;
    unit.order = Order::Harvest {
        node,
        anchor: node,
        retiring: false,
    };
    unit.pos = oxide_sim::geometry::work_approach_point(
        node.offset(-1, 0),
        node,
        (1, 1),
        unit.kind.stats().radius,
    );
    unit.worker
        .as_mut()
        .expect("a Harvester has harvest gear")
        .carrying = 5;
    unit.progress = 1;
    let facts = UnitAnimationFacts::capture(&state, &unit, false);
    assert_eq!(
        facts.work,
        UnitWorkFact::Harvesting(node.center(), unit.progress)
    );
    let animation = AnimationController::default().unit_state(
        facts,
        AnimationClock::new(5, 0.0),
        AnimationOptions::default(),
    );
    assert!(matches!(animation.work, UnitWorkState::Harvesting { .. }));
    assert!(matches!(
        animation.cargo,
        Some(CargoState { fill, .. }) if (fill - 0.5).abs() < 0.001
    ));

    unit.progress = 0;
    assert_eq!(
        UnitAnimationFacts::capture(&state, &unit, false).work,
        UnitWorkFact::Harvesting(node.center(), unit.progress),
        "a scoop boundary retains the physical work target and facing"
    );
    unit.progress = 1;

    unit.order = Order::Harvest {
        node,
        anchor: node,
        retiring: true,
    };
    assert_eq!(
        UnitAnimationFacts::capture(&state, &unit, false).work,
        UnitWorkFact::Idle
    );
}

#[test]
fn unloading_pose_and_cargo_follow_simulation_ticks() {
    let controller = AnimationController::default();
    let mut facts = unit_facts(UnitKind::Harvester);
    facts.carrying = 7;
    facts.work = UnitWorkFact::Unloading(point(), 6);
    facts.work_target = Some(WorkTarget::Building(BuildingId(3)));
    let clock = AnimationClock::new(20, 0.25);
    let state = controller.unit_state(facts, clock, AnimationOptions::default());
    assert!(matches!(state.work, UnitWorkState::Unloading { progress, .. } if progress == 0.625));
    assert_eq!(state.cargo.unwrap().amount, 7);
    facts.carrying = 0;
    facts.work = UnitWorkFact::Idle;
    let released = controller.unit_state(facts, clock, AnimationOptions::default());
    assert_eq!(released.cargo.unwrap().amount, 0);
    assert_eq!(released.work, UnitWorkState::Idle);
    assert_eq!(
        released,
        controller.unit_state(facts, clock, AnimationOptions::default())
    );
}

#[test]
fn excavator_cargo_uses_its_authoritative_thirty_scrap_capacity() {
    let mut facts = unit_facts(UnitKind::Excavator);
    facts.carrying = 15;
    let animation = AnimationController::default().unit_state(
        facts,
        AnimationClock::new(5, 0.0),
        AnimationOptions::default(),
    );
    assert_eq!(
        animation.cargo,
        Some(CargoState {
            amount: 15,
            capacity: 30,
            fill: 0.5,
        })
    );
}

#[test]
fn excavator_construction_activates_unit_and_site_machinery() {
    let mut state = excavator_state(Vec::new());
    let excavator = state.units()[0].id;
    let anchor = TilePos::new(5, 6);
    state.tick(&[player_command(Command::Build {
        units: vec![excavator],
        kind: BuildingKind::Turret,
        anchor,
        queue: false,
        defer: false,
    })]);
    for _ in 0..100 {
        if state
            .buildings()
            .iter()
            .any(|b| b.anchor == anchor && b.construction_progress().unwrap_or(0) > 0)
        {
            break;
        }
        state.tick(&[]);
    }

    let site = state
        .buildings()
        .iter()
        .find(|building| building.anchor == anchor)
        .expect("construction site exists");
    assert!(!site.built());
    assert!(site.construction_progress().unwrap_or(0) > 0);
    let unit = state.unit(excavator).expect("Excavator survives");
    let mut controller = AnimationController::default();
    controller.observe_workers(&state);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the fold lasts a whole number of ticks"
    )]
    let fold_ticks = WELD_ARM_FOLD_TICKS as u64;
    let animation = controller.unit_state(
        UnitAnimationFacts::capture(&state, unit, false),
        AnimationClock::new(state.current_tick() + fold_ticks, 0.0),
        AnimationOptions::default(),
    );
    assert!(matches!(
        animation.work,
        UnitWorkState::Constructing { site: active, .. } if active == site.id
    ));
    assert_eq!(
        animation.welding_arm,
        Some(WeldingArmState {
            target: WorkTarget::Building(site.id),
            deployment: 1.,
            active: true,
        }),
        "the Excavator welds a site with its arm"
    );
    assert!(BuildingAnimationFacts::capture(&state, site).construction_active);
}

#[test]
fn excavator_mills_with_the_drum_and_welds_with_the_arm() {
    let target = point();
    for (work, tool) in [
        (UnitWorkState::Idle, ExcavatorTool::Stowed),
        (
            UnitWorkState::Unloading {
                target,
                progress: 0.5,
            },
            ExcavatorTool::Stowed,
        ),
        (
            UnitWorkState::Harvesting { target, cycle: 0.5 },
            ExcavatorTool::Drum { cycle: 0.5 },
        ),
        (
            UnitWorkState::Salvaging { target, cycle: 0.5 },
            ExcavatorTool::Drum { cycle: 0.5 },
        ),
        (
            UnitWorkState::Constructing {
                site: BuildingId(4),
                target,
                cycle: 0.5,
            },
            ExcavatorTool::WeldingArm { cycle: 0.5 },
        ),
        (
            UnitWorkState::Repairing { target, cycle: 0.5 },
            ExcavatorTool::WeldingArm { cycle: 0.5 },
        ),
    ] {
        assert_eq!(work.excavator_tool(), tool, "{work:?}");
    }
}

#[test]
fn excavator_salvage_activates_unit_machinery() {
    let mut state = excavator_state(vec![BuildingSpec {
        player: 0,
        kind: BuildingKind::Turret,
        x: 5,
        y: 6,
    }]);
    let excavator = state.units()[0].id;
    let target = state
        .buildings()
        .iter()
        .find(|building| building.kind == BuildingKind::Turret)
        .expect("salvage target exists")
        .id;
    state.tick(&[player_command(Command::Salvage {
        units: vec![excavator],
        building: target,
        queue: false,
    })]);

    for _ in 0..20 {
        state.tick(&[]);
        let unit = state.unit(excavator).expect("Excavator survives");
        let animation = AnimationController::default().unit_state(
            UnitAnimationFacts::capture(&state, unit, false),
            AnimationClock::new(state.current_tick(), 0.0),
            AnimationOptions::default(),
        );
        if matches!(animation.work, UnitWorkState::Salvaging { .. }) {
            return;
        }
    }
    panic!("Excavator never exposed active salvage work");
}

#[test]
fn construction_requires_the_assigned_harvester_at_the_site() {
    let site = Building {
        id: BuildingId(12),
        player: PlayerId(0),
        kind: BuildingKind::Fabricator,
        anchor: TilePos::new(10, 10),
        hp: 100,
        queue: VecDeque::new(),
        phase: BuildingPhase::Site { progress: 20 },
        rally: None,
        focus: None,
        tier: 0,
        cooldown: 0,
        salvage_drained: 0,
        salvage_credited: 0,
    };
    let mut builder = Unit {
        air_motion: Vec2Fx::ZERO,
        id: UnitId(2),
        player: PlayerId(0),
        kind: UnitKind::Harvester,
        pos: oxide_sim::geometry::work_approach_point(
            site.anchor.offset(-1, 0),
            site.anchor,
            site.kind.size(),
            UnitKind::Harvester.stats().radius,
        ),
        hp: UnitKind::Harvester.stats().max_hp,
        worker: Some(oxide_sim::Worker::default()),
        cooldowns: [0; MAX_WEAPONS],
        brace_ticks: 0,
        turret_heading: None,
        drive_speed: chassis::fx::Fx::ZERO,
        stall_ticks: 0,
        progress: 0,
        order: Order::Build { site: site.id },
        queue: VecDeque::new(),
        looping: false,
        path: None,
        leash: None,
        settled: 0,
        heading: 0,
        cargo: Vec::new(),
        landed: false,
    };
    assert!(builder.in_work_reach(site.anchor, site.kind.size()));
    assert!(matches!(builder.order, Order::Build { site: id } if id == site.id));
    builder.pos = TilePos::new(1, 1).center();
    assert!(!builder.in_work_reach(site.anchor, site.kind.size()));
}

#[test]
fn producers_animate_only_while_queue_progress_can_advance() {
    let controller = AnimationController::default();
    for (building, unit) in [
        (BuildingKind::Foundry, UnitKind::Sentinel),
        (BuildingKind::Airworks, UnitKind::Gnat),
    ] {
        let mut facts = building_facts(building);
        facts.production = Some((unit, 25, unit.stats().train_ticks));
        let working = controller.building_state(
            facts,
            AnimationClock::new(10, 0.0),
            AnimationOptions::default(),
        );
        assert!(matches!(
            working.activity,
            BuildingActivity::Production { unit: active, .. } if active == unit
        ));

        facts.production = None;
        let idle = controller.building_state(
            facts,
            AnimationClock::new(10, 0.0),
            AnimationOptions::default(),
        );
        assert_eq!(idle.activity, BuildingActivity::Idle);
    }
}

#[test]
fn airworks_opens_before_completion_and_closes_after_launch() {
    let mut controller = AnimationController::default();
    let mut facts = building_facts(BuildingKind::Airworks);
    let total = UnitKind::Buzzard.stats().train_ticks;
    for (remaining, expected) in [(12, 0.0), (6, 0.5), (1, 11.0 / 12.0)] {
        facts.production = Some((UnitKind::Buzzard, total - remaining, total));
        let activity = controller
            .building_state(
                facts,
                AnimationClock::new(100, 0.0),
                AnimationOptions::default(),
            )
            .activity;
        let BuildingActivity::AirworksLaunch { progress } = activity else {
            panic!("the bay must open before the aircraft appears");
        };
        assert!((progress - expected).abs() < 0.0001);
    }
    facts.production = None;
    controller.observe_events(
        100,
        &[Event::UnitTrained {
            building: facts.id,
            unit: UnitId(99),
            kind: UnitKind::Buzzard,
            player: PlayerId(0),
        }],
    );
    for (tick, expected) in [(100, 1.0), (108, 1.0), (112, 0.5)] {
        assert_eq!(
            controller
                .building_state(
                    facts,
                    AnimationClock::new(tick, 0.0),
                    AnimationOptions::default()
                )
                .activity,
            BuildingActivity::AirworksLaunch { progress: expected }
        );
    }
}

#[test]
fn aircraft_completion_holds_open_only_its_producer_bay() {
    let facts = building_facts(BuildingKind::Airworks);
    let mut controller = AnimationController::default();
    controller.observe_events(
        20,
        &[Event::UnitTrained {
            building: facts.id,
            unit: UnitId(43),
            kind: UnitKind::Sentinel,
            player: PlayerId(0),
        }],
    );
    assert_eq!(
        controller
            .building_state(
                facts,
                AnimationClock::new(20, 0.0),
                AnimationOptions::default()
            )
            .activity,
        BuildingActivity::Idle
    );

    controller.observe_events(
        21,
        &[Event::UnitTrained {
            building: facts.id,
            unit: UnitId(44),
            kind: UnitKind::Gnat,
            player: PlayerId(0),
        }],
    );

    let clock = AnimationClock::new(21, 0.0);
    assert_eq!(
        controller
            .building_state(facts, clock, AnimationOptions::default())
            .activity,
        BuildingActivity::AirworksLaunch { progress: 1.0 }
    );

    let finished = AnimationClock::new(37, 0.0);
    assert_eq!(
        controller
            .building_state(facts, finished, AnimationOptions::default())
            .activity,
        BuildingActivity::Idle
    );
}

#[test]
fn completed_airworks_launches_are_pruned() {
    let mut state = Scenario::skirmish().build().expect("state");
    let building = state.buildings()[0].id;
    let mut controller = AnimationController::default();
    controller.observe_events(
        0,
        &[Event::UnitTrained {
            building,
            unit: UnitId(44),
            kind: UnitKind::Gnat,
            player: PlayerId(0),
        }],
    );
    controller.retain_live(&state);
    assert_eq!(controller.airworks_launches.len(), 1);

    for _ in 0..AIRWORKS_LAUNCH_TICKS {
        state.tick(&[]);
    }
    controller.retain_live(&state);
    assert!(controller.airworks_launches.is_empty());
}

#[test]
fn foundry_eye_pulses_more_slowly_than_fabricator_machinery() {
    let kind = UnitKind::Sentinel;
    let controller = AnimationController::default();
    let production_cycle = |building_kind, tick| {
        let mut facts = building_facts(building_kind);
        facts.production = Some((kind, 25, kind.stats().train_ticks));
        let state = controller.building_state(
            facts,
            AnimationClock::new(tick, 0.0),
            AnimationOptions::default(),
        );
        let BuildingActivity::Production { cycle, .. } = state.activity else {
            panic!("factory with a queue must expose production motion");
        };
        cycle
    };

    assert_eq!(
        production_cycle(BuildingKind::Foundry, 0),
        production_cycle(BuildingKind::Foundry, FOUNDRY_PRODUCTION_PERIOD)
    );
    assert_eq!(
        production_cycle(BuildingKind::Fabricator, 0),
        production_cycle(BuildingKind::Fabricator, FABRICATOR_PRODUCTION_PERIOD)
    );
    assert_eq!(
        production_cycle(BuildingKind::Crucible, 0),
        production_cycle(BuildingKind::Crucible, CRUCIBLE_PRODUCTION_PERIOD)
    );
    assert_ne!(
        production_cycle(BuildingKind::Foundry, FABRICATOR_PRODUCTION_PERIOD),
        production_cycle(BuildingKind::Foundry, 0)
    );
}

#[test]
fn repair_bay_moves_only_after_an_accepted_unit_or_building_repair_event() {
    let facts = building_facts(BuildingKind::RepairBay);
    for event in [
        Event::UnitRepaired {
            unit: UnitId(4),
            player: PlayerId(0),
            source: UnitRepairSource::RepairBay { building: facts.id },
            amount: 2,
        },
        Event::BuildingRepaired {
            building: BuildingId(9),
            player: PlayerId(0),
            repair_bay: facts.id,
            amount: 2,
        },
    ] {
        let mut controller = AnimationController::default();
        let before = controller.building_state(
            facts,
            AnimationClock::new(20, 0.0),
            AnimationOptions::default(),
        );
        assert_eq!(before.activity, BuildingActivity::Idle);

        let report = &TickReport {
            tick: 20,
            movement: Vec::new(),
            events: vec![event],
        };
        controller.observe_events(report.tick + 1, &report.events);
        let pulse = controller.building_state(
            facts,
            AnimationClock::new(21, 0.0),
            AnimationOptions::default(),
        );
        assert!(matches!(
            pulse.activity,
            BuildingActivity::RepairPulse { progress: 0.0 }
        ));
        let done = controller.building_state(
            facts,
            AnimationClock::new(27, 0.0),
            AnimationOptions::default(),
        );
        assert_eq!(done.activity, BuildingActivity::Idle);
    }
}

#[test]
fn income_and_scan_buildings_use_continuous_activity_loops() {
    let controller = AnimationController::default();
    let clock = AnimationClock::new(10, 0.5);
    let options = AnimationOptions::default();
    assert!(matches!(
        controller
            .building_state(building_facts(BuildingKind::Array), clock, options)
            .activity,
        BuildingActivity::ArraySweep { .. }
    ));
    assert!(matches!(
        controller
            .building_state(building_facts(BuildingKind::Extractor), clock, options)
            .activity,
        BuildingActivity::Extracting { .. }
    ));
    assert!(matches!(
        controller
            .building_state(building_facts(BuildingKind::Reclaimer), clock, options)
            .activity,
        BuildingActivity::Reclaiming { .. }
    ));
    for kind in [
        BuildingKind::Foundry,
        BuildingKind::Fabricator,
        BuildingKind::Airworks,
        BuildingKind::RepairBay,
        BuildingKind::Turret,
        BuildingKind::FlakTurret,
        BuildingKind::Bastion,
    ] {
        assert_eq!(
            controller
                .building_state(building_facts(kind), clock, options)
                .activity,
            BuildingActivity::Idle,
            "{kind:?} must not receive a decorative idle loop"
        );
    }
}

#[test]
fn reset_for_seek_drops_reports_but_cooldowns_still_reconstruct() {
    let mut controller = AnimationController::default();
    let report = &unit_attack_report(4, UnitId(7), 0);
    controller.observe_events(report.tick + 1, &report.events);
    let mut facts = unit_facts(UnitKind::Lancer);
    facts.cooldowns[0] = 20;
    controller.reset_transients();
    let state = controller.unit_state(
        facts,
        AnimationClock::new(5, 0.0),
        AnimationOptions::default(),
    );
    assert!(state.attack.is_none());
    assert!(matches!(state.weapons[0], WeaponCycle::Preparing { .. }));
}

#[test]
fn construction_progress_freezes_machinery_when_inactive_or_reduced() {
    let mut facts = building_facts(BuildingKind::Fabricator);
    facts.built = false;
    facts.progress = 140;
    facts.construction_active = false;
    let controller = AnimationController::default();
    let inactive = controller.building_state(
        facts,
        AnimationClock::new(19, 0.5),
        AnimationOptions::default(),
    );
    assert!(matches!(
        inactive.construction,
        Some(ConstructionState {
            active: false,
            machinery_cycle: 0.0,
            ..
        })
    ));

    facts.construction_active = true;
    let reduced = controller.building_state(
        facts,
        AnimationClock::new(19, 0.5),
        AnimationOptions {
            reduced_motion: true,
        },
    );
    assert!(matches!(
        reduced.construction,
        Some(ConstructionState {
            active: true,
            machinery_cycle: 0.0,
            ..
        })
    ));
}

#[test]
fn an_automatic_upgrade_animates_without_a_builder() {
    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = 500;
    scenario.units.clear();
    scenario.buildings.extend([
        oxide_sim::scenario::BuildingSpec {
            player: 0,
            kind: BuildingKind::Fabricator,
            x: 9,
            y: 3,
        },
        oxide_sim::scenario::BuildingSpec {
            player: 0,
            kind: BuildingKind::Turret,
            x: 12,
            y: 3,
        },
    ]);
    let mut state = scenario.build().expect("upgrade fixture builds");
    let turret = state
        .buildings()
        .iter()
        .find(|building| building.kind == BuildingKind::Turret)
        .expect("fixture has a turret")
        .id;
    state.tick(&[oxide_sim::PlayerCommand {
        player: PlayerId(0),
        command: oxide_sim::Command::UpgradeBuilding { building: turret },
    }]);

    let building = state.building(turret).expect("upgrade lives");
    let facts = BuildingAnimationFacts::capture(&state, building);
    assert!(facts.construction_active);
    assert!(matches!(
        AnimationController::default()
            .building_state(
                facts,
                AnimationClock::new(state.current_tick(), 0.5),
                AnimationOptions::default(),
            )
            .construction,
        Some(ConstructionState { active: true, .. })
    ));
}

#[test]
fn capture_and_retention_entrypoints_follow_the_current_world() {
    let state = Scenario::skirmish().build().expect("skirmish builds");
    let clock = AnimationClock::from_state(&state, 0.25);
    assert_eq!(clock, AnimationClock::new(state.current_tick(), 0.25));

    let building = state.buildings().first().expect("skirmish has a Foundry");
    let facts = BuildingAnimationFacts::capture(&state, building);
    assert_eq!(facts.id, building.id);

    let mut controller = AnimationController::default();
    controller.observe_events(
        state.current_tick(),
        &[Event::TurretFired {
            turret: BuildingId(u32::MAX),
            kind: BuildingKind::Turret,
            tier: 0,
            target: Some(Target::Unit(UnitId(0))),
            turret_pos: point(),
            target_pos: point(),
        }],
    );
    assert_eq!(controller.building_attacks.len(), 1);
    controller.retain_live(&state);
    assert!(controller.building_attacks.is_empty());
}
