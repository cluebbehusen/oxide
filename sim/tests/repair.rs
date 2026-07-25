//! Repair welding and Reclaimer trickle income. Headless scenarios
//! through the public API only, like `behavior.rs`.

use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::scenario::{PlayerSpec, UnitSpec};
use oxide_sim::stats::{BuildingKind, RECLAIMER_PERIOD};
use oxide_sim::{
    Command, Event, Faction, Order, PlayerCommand, PlayerId, Scenario, State, UnitKind,
};

fn arena(units: Vec<UnitSpec>) -> Scenario {
    Scenario {
        name: "repair-arena".into(),
        seed: 42,
        map: vec![
            "################".into(),
            "#1.............#".into(),
            "#..............#".into(),
            "#.....##.......#".into(),
            "#.....##...s...#".into(),
            "#..........s...#".into(),
            "#............2.#".into(),
            "#..............#".into(),
            "################".into(),
        ],
        players: vec![
            PlayerSpec {
                name: "Ferrous".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 200,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "Cupric".into(),
                faction: Faction::Cupric,
                team: None,
                scrap: 200,
                bot: false,
                bot_config: None,
            },
        ],
        units,
        meta: None,
    }
}

fn unit(player: u8, kind: UnitKind, x: i32, y: i32) -> UnitSpec {
    UnitSpec { player, kind, x, y }
}

fn cmd(player: u8, command: Command) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(player),
        command,
    }
}

fn run_until(
    state: &mut State,
    max_ticks: u64,
    mut stop: impl FnMut(&State, &[Event]) -> bool,
) -> Vec<Event> {
    let mut all = Vec::new();
    for _ in 0..max_ticks {
        let report = state.tick(&[]);
        let done = stop(state, &report.events);
        all.extend(report.events);
        if done {
            return all;
        }
    }
    panic!("condition not reached within {max_ticks} ticks");
}

/// Builds a turret at (3,3), lets an enemy scuttler raid wound it (the
/// builder is an accepted casualty; the turret wins), then trains a
/// fresh harvester to do the welding. Returns (turret, welder, hp after
/// the fight).
fn wounded_turret(
    state: &mut State,
    builder: oxide_sim::UnitId,
    raiders: Vec<oxide_sim::UnitId>,
) -> (oxide_sim::BuildingId, oxide_sim::UnitId, u32) {
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor: TilePos::new(3, 3),
            queue: false,
        },
    )]);
    run_until(state, 500, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::BuildingCompleted { .. }))
    });
    let turret = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Turret)
        .unwrap()
        .id;
    // The raid: the scuttler eats the defenseless builder, gnaws the
    // turret, and dies to it — leaving scars.
    state.tick(&[cmd(
        1,
        Command::AttackMove {
            units: raiders.clone(),
            goal: TilePos::new(3, 4),
            queue: false,
        },
    )]);
    let mut fallen = 0;
    run_until(state, 1200, |_, events| {
        fallen += events
            .iter()
            .filter(|e| matches!(e, Event::UnitDied { unit, .. } if raiders.contains(unit)))
            .count();
        fallen == raiders.len()
    });
    let hp = state.building(turret).unwrap().hp;
    assert!(
        hp < BuildingKind::Turret.stats().max_hp,
        "test premise: the raid must leave scars (hp {hp})"
    );
    // Train the welder now that the field is quiet.
    let foundry = state
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(0) && b.kind == BuildingKind::Foundry)
        .unwrap()
        .id;
    state.tick(&[cmd(
        0,
        Command::Train {
            building: foundry,
            kind: UnitKind::Harvester,
        },
    )]);
    let mut welder = None;
    run_until(state, 200, |_, events| {
        events.iter().any(|e| {
            if let Event::UnitTrained { unit, .. } = e {
                welder = Some(*unit);
                true
            } else {
                false
            }
        })
    });
    (turret, welder.expect("trained"), hp)
}

#[test]
fn harvesters_weld_wounds_shut_for_a_price() {
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 4, 2),
        unit(1, UnitKind::Scuttler, 12, 6),
        unit(1, UnitKind::Scuttler, 12, 7),
    ])
    .build()
    .unwrap();
    let builder = state.units()[0].id;
    let raiders = vec![state.units()[1].id, state.units()[2].id];
    let (turret, welder, wounded_hp) = wounded_turret(&mut state, builder, raiders);
    let bank_before = state.player(PlayerId(0)).scrap;
    state.tick(&[cmd(
        0,
        Command::Repair {
            units: vec![welder],
            building: turret,
            queue: false,
        },
    )]);
    run_until(&mut state, 600, |s, _| {
        s.building(turret).unwrap().hp == BuildingKind::Turret.stats().max_hp
    });
    let spent = bank_before - state.player(PlayerId(0)).scrap;
    assert!(spent > 0, "welding is never free");
    let healed = BuildingKind::Turret.stats().max_hp - wounded_hp;
    assert!(
        spent < healed,
        "but far cheaper than the damage was worth (spent {spent} for {healed} hp)"
    );
    // The job wraps up on its own.
    run_until(&mut state, 20, |s, _| {
        s.unit(welder).unwrap().order == Order::Idle
    });
}

#[test]
fn an_empty_bank_stalls_the_torch() {
    let mut scenario = arena(vec![
        unit(0, UnitKind::Harvester, 4, 2),
        unit(1, UnitKind::Scuttler, 12, 6),
        unit(1, UnitKind::Scuttler, 12, 7),
    ]);
    // Turret (100) + welder (50) leave two coins for the torch — not
    // nearly enough to close the raid's scars.
    scenario.players[0].scrap = 152;
    let mut state = scenario.build().unwrap();
    let builder = state.units()[0].id;
    let raiders = vec![state.units()[1].id, state.units()[2].id];
    let (turret, welder, _) = wounded_turret(&mut state, builder, raiders);
    state.tick(&[cmd(
        0,
        Command::Repair {
            units: vec![welder],
            building: turret,
            queue: false,
        },
    )]);
    run_until(&mut state, 300, |_, events| {
        events.iter().any(|e| {
            matches!(
                e,
                Event::OrderStalled {
                    unit,
                    reason: oxide_sim::StallReason::InsufficientScrap,
                    ..
                } if *unit == welder
            )
        })
    });
    assert_eq!(state.player(PlayerId(0)).scrap, 0, "the last coin burned");
    assert!(
        state.building(turret).unwrap().hp < BuildingKind::Turret.stats().max_hp,
        "the weld never finished"
    );
    assert_eq!(state.unit(welder).unwrap().order, Order::Idle);
}

#[test]
fn repair_rejects_the_healthy_the_foreign_and_the_unfinished() {
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 4, 2)])
        .build()
        .unwrap();
    let builder = state.units()[0].id;
    let my_foundry = state
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(0))
        .unwrap()
        .id;
    let their_foundry = state
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(1))
        .unwrap()
        .id;
    for (building, expected) in [
        (my_foundry, RejectReason::InvalidTarget),
        (their_foundry, RejectReason::NotYourBuilding),
    ] {
        let report = state.tick(&[cmd(
            0,
            Command::Repair {
                units: vec![builder],
                building,
                queue: false,
            },
        )]);
        assert!(
            report
                .events
                .iter()
                .any(|e| matches!(e, Event::CommandRejected { reason, .. } if *reason == expected)),
            "expected {expected:?}"
        );
    }
    // A fresh site is Build's business, not Repair's.
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor: TilePos::new(6, 1),
            queue: false,
        },
    )]);
    let site = state
        .buildings()
        .iter()
        .find(|b| !b.built)
        .expect("site placed")
        .id;
    let report = state.tick(&[cmd(
        0,
        Command::Repair {
            units: vec![builder],
            building: site,
            queue: false,
        },
    )]);
    assert!(report.events.iter().any(|e| matches!(
        e,
        Event::CommandRejected {
            reason: RejectReason::InvalidTarget,
            ..
        }
    )));
}

#[test]
fn reclaimers_trickle_scrap_forever() {
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 4, 2)])
        .build()
        .unwrap();
    let builder = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Reclaimer,
            anchor: TilePos::new(5, 1),
            queue: false,
        },
    )]);
    run_until(&mut state, 500, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::BuildingCompleted { .. }))
    });
    let sample_a = state.player(PlayerId(0)).scrap;
    for _ in 0..10 * RECLAIMER_PERIOD {
        state.tick(&[]);
    }
    let sample_b = state.player(PlayerId(0)).scrap;
    assert_eq!(
        sample_b - sample_a,
        10,
        "one scrap per period, exactly, forever"
    );
    // The payback math that keeps it out of opening builds: minutes, not
    // seconds.
    let cost = u64::from(BuildingKind::Reclaimer.stats().construction.unwrap().cost);
    let payback_ticks = cost * RECLAIMER_PERIOD;
    assert!(
        payback_ticks >= 20 * 60 * 3,
        "a reclaimer must take at least three minutes to repay itself"
    );
}

#[test]
fn reissued_repairs_still_pay_for_the_welding() {
    // The scripted tiers re-command their welder every think; the
    // reissue must not reset the billing counter, or badly damaged
    // buildings heal for free.
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 4, 2),
        unit(1, UnitKind::Scuttler, 12, 6),
        unit(1, UnitKind::Scuttler, 12, 7),
    ])
    .build()
    .unwrap();
    let builder = state.units()[0].id;
    let raiders = vec![state.units()[1].id, state.units()[2].id];
    let (turret, welder, _) = wounded_turret(&mut state, builder, raiders);
    let bank_before = state.player(PlayerId(0)).scrap;
    // Reissue the identical repair every 4 ticks — the bot-think cadence
    // that used to reset the billing counter before it ever reached a
    // whole scrap.
    let mut healed = false;
    for _ in 0..150 {
        state.tick(&[cmd(
            0,
            Command::Repair {
                units: vec![welder],
                building: turret,
                queue: false,
            },
        )]);
        for _ in 0..3 {
            state.tick(&[]);
        }
        let b = state.building(turret).unwrap();
        if b.hp == BuildingKind::Turret.stats().max_hp {
            healed = true;
            break;
        }
    }
    assert!(healed, "the weld must finish under re-command");
    assert!(
        state.player(PlayerId(0)).scrap < bank_before,
        "and the torch must have been paid for ({} -> {})",
        bank_before,
        state.player(PlayerId(0)).scrap
    );
}

#[test]
fn the_torch_bills_its_first_scrap_the_tick_it_lights() {
    // Billing lands at each interval's start, not its end — a chip repair
    // that finishes in under an interval still pays one scrap, or repeated
    // small wounds would heal free forever.
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 4, 2),
        unit(1, UnitKind::Scuttler, 12, 6),
        unit(1, UnitKind::Scuttler, 12, 7),
    ])
    .build()
    .unwrap();
    let builder = state.units()[0].id;
    let raiders = vec![state.units()[1].id, state.units()[2].id];
    let (turret, welder, _) = wounded_turret(&mut state, builder, raiders);
    let bank_before = state.player(PlayerId(0)).scrap;
    state.tick(&[cmd(
        0,
        Command::Repair {
            units: vec![welder],
            building: turret,
            queue: false,
        },
    )]);
    // The welder trained adjacent to the foundry and must walk over; give
    // it until the first welding tick, which is when the first coin drops.
    run_until(&mut state, 100, |s, _| {
        s.player(PlayerId(0)).scrap < bank_before
    });
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        bank_before - 1,
        "exactly one scrap up front, not a free first interval"
    );
}

#[test]
fn the_last_coin_welds_its_whole_prepaid_interval() {
    // The coin spent at an interval's start buys all ten welding ticks:
    // broke may only stall the torch at the next billing boundary, never
    // mid-interval. One coin in the bank must therefore heal exactly the
    // ramp's first full interval before the stall.
    let mut scenario = arena(vec![
        unit(0, UnitKind::Harvester, 4, 2),
        unit(1, UnitKind::Scuttler, 12, 6),
        unit(1, UnitKind::Scuttler, 12, 7),
    ]);
    // Turret (100) + welder (50) leave exactly one coin.
    scenario.players[0].scrap = 151;
    let mut state = scenario.build().unwrap();
    let builder = state.units()[0].id;
    let raiders = vec![state.units()[1].id, state.units()[2].id];
    let (turret, welder, _) = wounded_turret(&mut state, builder, raiders);
    assert_eq!(state.player(PlayerId(0)).scrap, 1, "test premise: one coin");
    let hp_before = state.building(turret).unwrap().hp;

    state.tick(&[cmd(
        0,
        Command::Repair {
            units: vec![welder],
            building: turret,
            queue: false,
        },
    )]);
    run_until(&mut state, 300, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::OrderStalled { unit, .. } if *unit == welder))
    });

    let stats = BuildingKind::Turret.stats();
    let ramp = stats.max_hp - stats.max_hp / 5;
    let ramp_ticks = stats.construction.unwrap().build_ticks;
    // Ten ticks of the linear ramp from progress zero telescope to
    // exactly ramp * 10 / ramp_ticks hp.
    let interval = ramp * 10 / ramp_ticks;
    assert!(interval > 0, "test premise: an interval heals something");
    assert_eq!(
        state.building(turret).unwrap().hp,
        hp_before + interval,
        "one coin buys the full interval, not a single tick"
    );
    assert_eq!(state.player(PlayerId(0)).scrap, 0);
}

#[test]
fn a_queued_repair_waits_its_turn_then_welds() {
    // Shift-repair appends behind the current job instead of wiping it:
    // the welder finishes its walk first, then turns to the wound.
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 3, 2),
        unit(1, UnitKind::Scuttler, 12, 6),
        unit(1, UnitKind::Scuttler, 12, 7),
    ])
    .build()
    .unwrap();
    let (builder, r1, r2) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let (turret, welder, wounded_hp) = wounded_turret(&mut state, builder, vec![r1, r2]);

    // Send the welder marching, then queue the weld behind the march.
    let waypoint = TilePos::new(10, 2);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![welder],
            goal: waypoint,
            queue: false,
        },
    )]);
    state.tick(&[cmd(
        0,
        Command::Repair {
            units: vec![welder],
            building: turret,
            queue: true,
        },
    )]);
    assert!(
        matches!(state.unit(welder).unwrap().order, Order::Move { .. }),
        "the march survives the shift-repair"
    );
    assert_eq!(state.unit(welder).unwrap().queue.len(), 1);
    // The march lands, the weld begins, the wound closes.
    run_until(&mut state, 400, |s, _| {
        s.unit(welder).unwrap().tile() == waypoint
    });
    run_until(&mut state, 2_000, |s, _| {
        s.building(turret).unwrap().hp > wounded_hp
    });
}
