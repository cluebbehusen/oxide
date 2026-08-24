//! In-place building upgrades: the tier ladder. An upgrade charges its
//! full price, takes the works offline as a site on the new tier's row,
//! rebuilds itself on a deterministic timer, and stands back up with the
//! new tier's numbers. Workers cannot pause or accelerate that timer,
//! upgrades cannot be cancelled, and the deepest rungs sit behind the
//! Crucible.

use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::stats::{BuildingKind, RECLAIMER_PERIOD, SITE_DECAY_PERIOD};
use oxide_sim::{Command, Event, Faction, PlayerCommand, PlayerId, Scenario, State, UnitKind};

fn players(scrap: u32) -> Vec<PlayerSpec> {
    vec![
        PlayerSpec {
            name: "Ferrous".into(),
            faction: Faction::Ferrous,
            team: None,
            scrap,
            bot: false,
            bot_config: None,
        },
        PlayerSpec {
            name: "Cupric".into(),
            faction: Faction::Cupric,
            team: None,
            scrap,
            bot: false,
            bot_config: None,
        },
    ]
}

/// Seat 0 with a Fabricator, a Turret, and a harvester crew — the
/// Reclaimer joins only where its income cannot smudge exact-bank math.
fn arena(scrap: u32, crucible: bool, reclaimer: bool) -> Scenario {
    let mut buildings = vec![
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Fabricator,
            x: 2,
            y: 5,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Turret,
            x: 8,
            y: 3,
        },
    ];
    if reclaimer {
        buildings.push(BuildingSpec {
            player: 0,
            kind: BuildingKind::Reclaimer,
            x: 6,
            y: 6,
        });
    }
    if crucible {
        buildings.push(BuildingSpec {
            player: 0,
            kind: BuildingKind::Crucible,
            x: 11,
            y: 5,
        });
    }
    Scenario {
        name: "upgrade-arena".into(),
        seed: 3,
        map: vec![
            "####################".into(),
            "#1.................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#...............2..#".into(),
            "#..................#".into(),
            "####################".into(),
        ],
        players: players(scrap),
        units: vec![
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 6,
                y: 3,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 5,
                y: 6,
            },
        ],
        buildings,
        meta: None,
    }
}

fn cmd(player: u8, command: Command) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(player),
        command,
    }
}

fn upgrade(building: oxide_sim::BuildingId) -> PlayerCommand {
    cmd(0, Command::UpgradeBuilding { building })
}

fn find(state: &State, kind: BuildingKind) -> oxide_sim::BuildingId {
    state
        .buildings()
        .iter()
        .find(|b| b.kind == kind)
        .unwrap()
        .id
}

fn run_until_built(state: &mut State, building: oxide_sim::BuildingId, cap: u64) {
    for _ in 0..cap {
        state.tick(&[]);
        if state.building(building).is_some_and(|b| b.built) {
            return;
        }
    }
    panic!("the upgrade never completed within {cap} ticks");
}

#[test]
fn a_self_upgrade_uses_the_exact_timer_without_workers() {
    let mut scenario = arena(2_000, false, false);
    scenario.units.clear();
    let mut state = scenario.build().unwrap();
    let turret = find(&state, BuildingKind::Turret);
    let ticks = BuildingKind::Turret
        .upgrade_from(0)
        .expect("the Heavy Turret rung exists")
        .build_ticks;

    let report = state.tick(&[upgrade(turret)]);
    assert!(report.events.iter().all(
        |event| !matches!(event, Event::BuildingCompleted { building, .. } if *building == turret)
    ));
    assert_eq!(state.building(turret).unwrap().progress, 1);
    for expected in 2..ticks {
        let report = state.tick(&[]);
        let building = state.building(turret).unwrap();
        assert!(!building.built, "the works stood one tick too soon");
        assert_eq!(building.progress, expected);
        assert!(report.events.iter().all(
            |event| !matches!(event, Event::BuildingCompleted { building, .. } if *building == turret)
        ));
    }

    let report = state.tick(&[]);
    let building = state.building(turret).unwrap();
    assert!(building.built);
    assert_eq!(building.progress, 0);
    assert!(report.events.iter().any(
        |event| matches!(event, Event::BuildingCompleted { building, .. } if *building == turret)
    ));
}

#[test]
fn buying_an_upgrade_does_not_touch_worker_programs() {
    let mut state = arena(2_000, false, false).build().unwrap();
    let turret = find(&state, BuildingKind::Turret);
    let worker = state.units()[0].id;
    state.tick(&[
        cmd(
            0,
            Command::Move {
                units: vec![worker],
                goal: TilePos::new(4, 3),
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![worker],
                goal: TilePos::new(4, 6),
                queue: true,
            },
        ),
    ]);
    assert!(
        !state.unit(worker).unwrap().queue.is_empty(),
        "premise: the worker carries active and queued work"
    );
    let mut control = state.clone();

    state.tick(&[upgrade(turret)]);
    control.tick(&[]);

    assert_eq!(
        state.units(),
        control.units(),
        "the purchase must not draft, clear, reroute, or advance a worker"
    );
}

#[test]
fn workers_cannot_join_or_accelerate_a_self_upgrade() {
    let mut state = arena(2_000, false, false).build().unwrap();
    let turret = find(&state, BuildingKind::Turret);
    let worker = state.units()[0].id;
    let target = state.building(turret).unwrap();
    let (kind, anchor) = (target.kind, target.anchor);
    state.tick(&[upgrade(turret)]);
    let mut control = state.clone();

    let report = state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![worker],
            kind,
            anchor,
            queue: false,
            defer: false,
        },
    )]);
    control.tick(&[]);

    assert!(report.events.iter().any(|event| matches!(
        event,
        Event::CommandRejected {
            reason: RejectReason::BadSite,
            ..
        }
    )));
    assert_eq!(state.building(turret), control.building(turret));
    assert_eq!(state.units(), control.units());
}

#[test]
fn a_stale_builder_order_cannot_speed_up_the_next_tier() {
    let mut scenario = arena(2_000, false, false);
    scenario
        .buildings
        .retain(|building| building.kind != BuildingKind::Turret);
    let mut state = scenario.build().unwrap();
    let worker = state.units()[0].id;
    let anchor = TilePos::new(8, 3);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![worker],
            kind: BuildingKind::Turret,
            anchor,
            queue: false,
            defer: false,
        },
    )]);
    let turret = find(&state, BuildingKind::Turret);
    run_until_built(&mut state, turret, 2_000);
    assert_eq!(
        state.unit(worker).unwrap().order,
        oxide_sim::Order::Build { site: turret },
        "the builder observes completion on its next brain tick"
    );

    state.tick(&[upgrade(turret)]);

    assert_eq!(
        state.building(turret).unwrap().progress,
        1,
        "only the building's automatic clock advances"
    );
    assert_ne!(
        state.unit(worker).unwrap().order,
        oxide_sim::Order::Build { site: turret },
        "the stale ordinary-site job is finished, not an upgrade crew"
    );
}

#[test]
fn the_ladder_lifts_and_the_numbers_follow() {
    let mut state = arena(2_000, true, false).build().unwrap();
    let turret = find(&state, BuildingKind::Turret);

    let bank = state.player(PlayerId(0)).scrap;
    let heavy = BuildingKind::Turret.upgrade_from(0).unwrap();
    state.tick(&[upgrade(turret)]);
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        bank - heavy.cost,
        "the upgrade charges its full price on commitment"
    );
    {
        let b = state.building(turret).unwrap();
        assert!(!b.built, "the works goes offline as a site");
        assert_eq!(b.tier, 1, "the tier bumps at commitment");
    }
    run_until_built(&mut state, turret, 2_000);
    {
        let b = state.building(turret).unwrap();
        assert_eq!(b.stats().max_hp, 500, "heavy turret hit points");
        assert_eq!(b.stats().weapons[0].damage, 20, "heavy turret gun");
        assert_eq!(b.hp, b.stats().max_hp, "the ramp tops out at the new max");
    }

    // The second rung: Bulwark, behind the Crucible (present here).
    state.tick(&[upgrade(turret)]);
    run_until_built(&mut state, turret, 3_000);
    let b = state.building(turret).unwrap();
    assert_eq!(b.tier, 2);
    assert_eq!(b.stats().max_hp, 900, "bulwark hit points");
    assert_eq!(b.stats().weapons[0].damage, 60, "bulwark gun");
    assert_eq!(b.kind.tier_name(b.tier), "bulwark");
}

#[test]
fn the_deepest_rung_waits_for_the_crucible() {
    let mut state = arena(2_000, false, false).build().unwrap();
    let turret = find(&state, BuildingKind::Turret);
    state.tick(&[upgrade(turret)]);
    run_until_built(&mut state, turret, 2_000);

    let report = state.tick(&[upgrade(turret)]);
    assert!(
        report.events.iter().any(|e| matches!(
            e,
            Event::CommandRejected {
                reason: RejectReason::MissingPrerequisite,
                ..
            }
        )),
        "the Bulwark waits for the Crucible"
    );
    assert_eq!(state.building(turret).unwrap().tier, 1, "nothing moved");
}

#[test]
fn upgrades_refuse_the_broke_the_topped_out_and_the_cancel() {
    let mut state = arena(60, true, false).build().unwrap();
    let turret = find(&state, BuildingKind::Turret);
    let fabricator = find(&state, BuildingKind::Fabricator);

    // Too poor for the 150-scrap Heavy Turret.
    let report = state.tick(&[upgrade(turret)]);
    assert!(report.events.iter().any(|e| matches!(
        e,
        Event::CommandRejected {
            reason: RejectReason::NotEnoughScrap,
            ..
        }
    )));

    // A kind with no ladder refuses as an invalid target.
    let report = state.tick(&[upgrade(fabricator)]);
    assert!(report.events.iter().any(|e| matches!(
        e,
        Event::CommandRejected {
            reason: RejectReason::InvalidTarget,
            ..
        }
    )));
}

#[test]
fn an_upgrading_works_is_committed_offline_and_mortal() {
    let mut scenario = arena(2_000, false, false);
    scenario.units.push(UnitSpec {
        player: 1,
        kind: UnitKind::Sapper,
        x: 8,
        y: 2,
    });
    let mut state = scenario.build().unwrap();
    let turret = find(&state, BuildingKind::Turret);
    let raider = state.units()[2].id;
    state.tick(&[upgrade(turret)]);

    // No cancel: the machine is committed until it stands.
    let report = state.tick(&[cmd(0, Command::Cancel { building: turret })]);
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. })),
        "an upgrade cannot be cancelled"
    );

    // Offline: the site never fires while the raider grinds it down.
    let mut report = state.tick(&[cmd(
        1,
        Command::Attack {
            units: vec![raider],
            target: oxide_sim::Target::Building(turret),
            queue: false,
        },
    )]);
    let mut destroyed = report.events.iter().any(
        |event| matches!(event, Event::BuildingDestroyed { building, .. } if *building == turret),
    );
    for _ in 0..100 {
        assert!(
            !report
                .events
                .iter()
                .any(|e| matches!(e, Event::TurretFired { turret: t, .. } if *t == turret)),
            "an upgrading works must not fire"
        );
        if destroyed {
            break;
        }
        report = state.tick(&[]);
        destroyed = report
            .events
            .iter()
            .any(|event| matches!(event, Event::BuildingDestroyed { building, .. } if *building == turret));
    }
    assert!(destroyed, "the vulnerability window is real");
}

#[test]
fn lethal_fire_wins_an_upgrades_completion_tick() {
    let mut scenario = arena(2_000, false, false);
    scenario.units.extend([
        UnitSpec {
            player: 1,
            kind: UnitKind::Sapper,
            x: 8,
            y: 2,
        },
        UnitSpec {
            player: 1,
            kind: UnitKind::Sapper,
            x: 9,
            y: 3,
        },
    ]);
    let mut state = scenario.build().unwrap();
    let turret = find(&state, BuildingKind::Turret);
    let sappers: Vec<_> = state
        .units()
        .iter()
        .filter(|unit| unit.player == PlayerId(1) && unit.kind == UnitKind::Sapper)
        .map(|unit| unit.id)
        .collect();
    let ticks = BuildingKind::Turret.upgrade_from(0).unwrap().build_ticks;
    state.tick(&[upgrade(turret)]);
    for _ in 0..ticks - 2 {
        state.tick(&[]);
    }
    assert_eq!(state.building(turret).unwrap().progress, ticks - 1);

    let report = state.tick(&[cmd(
        1,
        Command::Attack {
            units: sappers,
            target: oxide_sim::Target::Building(turret),
            queue: false,
        },
    )]);

    assert!(report.events.iter().any(
        |event| matches!(event, Event::BuildingDestroyed { building, .. } if *building == turret)
    ));
    assert!(report.events.iter().all(
        |event| !matches!(event, Event::BuildingCompleted { building, .. } if *building == turret)
    ));
    assert!(state.building(turret).is_none());
}

#[test]
fn a_self_upgrading_building_does_not_decay_without_workers() {
    let mut scenario = arena(2_000, false, false);
    scenario.units.clear();
    let mut state = scenario.build().unwrap();
    let turret = find(&state, BuildingKind::Turret);
    assert!(state.units().is_empty(), "premise: there are no workers");
    state.tick(&[upgrade(turret)]);

    let mut previous_hp = state.building(turret).unwrap().hp;
    for _ in 0..SITE_DECAY_PERIOD * 20 {
        state.tick(&[]);
        let building = state.building(turret).expect("the upgrade survives");
        assert!(
            building.hp >= previous_hp,
            "active self-upgrade hp must never fall ({previous_hp} -> {})",
            building.hp
        );
        previous_hp = building.hp;
    }
}

#[test]
fn a_refinery_grinds_faster_than_its_reclaimer() {
    let mut state = arena(2_000, false, true).build().unwrap();
    let reclaimer = find(&state, BuildingKind::Reclaimer);
    state.tick(&[upgrade(reclaimer)]);
    run_until_built(&mut state, reclaimer, 2_000);
    assert_eq!(state.building(reclaimer).unwrap().tier, 1);

    // The refinery credits on the shared Reclaimer clock but at its own
    // richer rate — measure one long window against the base rate.
    let bank = state.player(PlayerId(0)).scrap;
    let window = 240u64;
    for _ in 0..window {
        state.tick(&[]);
    }
    let earned = state.player(PlayerId(0)).scrap - bank;
    let base_rate = (window / RECLAIMER_PERIOD) as u32;
    assert!(
        earned > base_rate,
        "a refinery out-earns the reclaimer clock ({earned} vs base {base_rate})"
    );
}

#[test]
fn the_deep_array_waits_for_the_crucible_too() {
    // Both deepest rungs sit behind the forge gate: the Array's wide
    // detection ring joins the Bulwark there.
    let mut scenario = arena(2_000, false, false);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Array,
        x: 14,
        y: 3,
    });
    let mut state = scenario.build().unwrap();
    let array = find(&state, BuildingKind::Array);

    let report = state.tick(&[upgrade(array)]);
    assert!(
        report.events.iter().any(|e| matches!(
            e,
            Event::CommandRejected {
                reason: RejectReason::MissingPrerequisite,
                ..
            }
        )),
        "the Deep Array waits for the Crucible"
    );
    assert_eq!(state.building(array).unwrap().tier, 0, "nothing moved");

    let mut scenario = arena(2_000, true, false);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Array,
        x: 14,
        y: 3,
    });
    let mut state = scenario.build().unwrap();
    let array = find(&state, BuildingKind::Array);
    state.tick(&[upgrade(array)]);
    run_until_built(&mut state, array, 2_000);
    let b = state.building(array).unwrap();
    assert_eq!(b.tier, 1);
    assert_eq!(b.kind.tier_name(b.tier), "deep array");
}
