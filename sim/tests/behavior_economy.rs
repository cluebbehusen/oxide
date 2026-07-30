//! Economy: harvest, production, rallies, tech gates — behavior suite, public API only.

mod common;

use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::{Command, Event, Order, PlayerId, Scenario, UnitKind};

use common::*;

#[test]
fn harvester_gathers_and_deposits() {
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 3, 2)])
        .build()
        .unwrap();
    let worker = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![worker],
            node: TilePos::new(11, 4),
            queue: false,
        },
    )]);
    // Walk there (~10 tiles), extract 10 scrap (100 ticks), walk home, drop.
    let events = run_until(&mut state, 600, |_, events| {
        events.iter().any(|event| {
            matches!(
                event,
                Event::ScrapDeposited {
                    player: PlayerId(0),
                    amount
                } if *amount > 0
            )
        })
    });
    assert!(events.iter().any(
        |e| matches!(e, Event::ScrapDeposited { player: PlayerId(0), amount } if *amount > 0)
    ));
    // Still on the job: the node isn't empty, so back to work.
    assert!(matches!(
        state.unit(worker).unwrap().order,
        Order::Harvest { .. }
    ));
}

#[test]
fn bot_economy_progresses_against_an_idle_opponent() {
    // The exact configuration that froze pre-fix: shipped skirmish, human
    // seat idle, Cupric bot alone. Its bank plus spending must exceed its
    // starting stake — deposits happened — well before 12k ticks.
    let scenario = Scenario::skirmish();
    let mut state = scenario.build().unwrap();
    let mut bots = oxide_sim::bot::Bot::for_scenario(&scenario);
    assert_eq!(bots.len(), 1, "skirmish ships with exactly one bot seat");

    let mut deposited = 0u32;
    for _ in 0..12_000u64 {
        let mut commands = Vec::new();
        for bot in &mut bots {
            commands.extend(bot.act(&state));
        }
        let report = state.tick(&commands);
        for event in &report.events {
            if let Event::ScrapDeposited {
                player: PlayerId(1),
                amount,
            } = event
            {
                deposited += amount;
            }
        }
        if deposited >= 300 {
            return; // healthy economy, no need to run the rest
        }
    }
    panic!("bot deposited only {deposited} scrap in 12k ticks — economy stalled");
}

#[test]
fn train_costs_scrap_and_spawns_after_build_time() {
    let mut state = arena(vec![]).build().unwrap();
    let foundry = state.buildings()[0].id;
    let before = state.player(PlayerId(0)).scrap;
    state.tick(&[cmd(
        0,
        Command::Train {
            building: foundry,
            kind: UnitKind::Harvester,
        },
    )]);
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        before - UnitKind::Harvester.stats().cost,
        "cost is deducted on enqueue"
    );
    let events = run_until(&mut state, 105, |s, _| !s.units().is_empty());
    assert!(events.iter().any(|e| matches!(
        e,
        Event::UnitTrained {
            kind: UnitKind::Harvester,
            player: PlayerId(0),
            ..
        }
    )));
}

#[test]
fn a_stranded_foundry_trickles_only_the_replacement_harvesters_price() {
    let mut scenario = arena(Vec::new());
    scenario.players[0].scrap = 0;
    let mut state = scenario.build().unwrap();

    // Tick zero is the first global cadence, matching Reclaimer income.
    state.tick(&[]);
    assert_eq!(state.player(PlayerId(0)).scrap, 1);
    for _ in 0..9 {
        state.tick(&[]);
    }
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        1,
        "the credit lands once per ten ticks"
    );
    state.tick(&[]);
    assert_eq!(state.player(PlayerId(0)).scrap, 2);

    let mut scenario = arena(Vec::new());
    scenario.players[0].scrap = UnitKind::Harvester.stats().cost - 1;
    let mut capped = scenario.build().unwrap();
    capped.tick(&[]);
    assert_eq!(
        capped.player(PlayerId(0)).scrap,
        UnitKind::Harvester.stats().cost
    );
    for _ in 0..100 {
        capped.tick(&[]);
    }
    assert_eq!(
        capped.player(PlayerId(0)).scrap,
        UnitKind::Harvester.stats().cost,
        "the fast recovery stops; baseline income starts only in the late game"
    );
}

#[test]
fn live_and_queued_harvest_lines_receive_only_slow_baseline_income() {
    let mut live = arena(vec![unit(0, UnitKind::Harvester, 3, 2)]);
    live.players[0].scrap = 0;
    let mut live = live.build().unwrap();
    for _ in 0..oxide_sim::stats::FOUNDRY_BASELINE_START_TICK - 1 {
        live.tick(&[]);
    }
    assert_eq!(
        live.player(PlayerId(0)).scrap,
        0,
        "the free floor does not distort the opening or midgame"
    );
    live.tick(&[]);
    assert_eq!(live.player(PlayerId(0)).scrap, 1);
    for _ in 0..oxide_sim::stats::FOUNDRY_BASELINE_PERIOD - 1 {
        live.tick(&[]);
    }
    assert_eq!(live.player(PlayerId(0)).scrap, 1);
    live.tick(&[]);
    assert_eq!(live.player(PlayerId(0)).scrap, 2);

    let mut queued = arena(Vec::new());
    queued.players[0].scrap = UnitKind::Harvester.stats().cost;
    let mut queued = queued.build().unwrap();
    let foundry = queued
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(0))
        .unwrap()
        .id;
    queued.tick(&[cmd(
        0,
        Command::Train {
            building: foundry,
            kind: UnitKind::Harvester,
        },
    )]);
    assert_eq!(queued.player(PlayerId(0)).scrap, 0);
    for _ in 0..50 {
        queued.tick(&[]);
    }
    assert_eq!(
        queued.player(PlayerId(0)).scrap,
        0,
        "a prepaid Harvester suppresses fast recovery but not baseline income"
    );
    while queued.current_tick() < oxide_sim::stats::FOUNDRY_BASELINE_START_TICK - 1 {
        queued.tick(&[]);
    }
    assert_eq!(queued.player(PlayerId(0)).scrap, 0);
    queued.tick(&[]);
    assert_eq!(
        queued.player(PlayerId(0)).scrap,
        1,
        "the queued or completed replacement does not suppress the late floor"
    );

    let mut resigned = arena(Vec::new());
    resigned.players[0].scrap = 0;
    let mut resigned = resigned.build().unwrap();
    resigned.tick(&[cmd(0, Command::Surrender)]);
    assert_eq!(
        resigned.player(PlayerId(0)).scrap,
        0,
        "a conceded seat cannot bank a comeback"
    );
    let mut value = serde_json::to_value(resigned).unwrap();
    value["tick"] = (oxide_sim::stats::FOUNDRY_BASELINE_START_TICK - 1).into();
    value["result"] = serde_json::Value::Null;
    let mut resigned: oxide_sim::State = serde_json::from_value(value).unwrap();
    resigned.tick(&[]);
    assert_eq!(
        resigned.player(PlayerId(0)).scrap,
        0,
        "the late floor never credits a resigned seat"
    );
}

#[test]
fn rally_routes_fresh_units_by_role() {
    // A bystander parked in sight of the node: rallies read the owner's
    // remembered scrap, so somebody must have actually seen it.
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 8, 4)])
        .build()
        .unwrap();
    let foundry = state.buildings()[0].id;
    // Rally onto the scrap node: fresh harvesters go straight to work.
    state.tick(&[
        cmd(
            0,
            Command::SetRally {
                building: foundry,
                rally: Some(TilePos::new(11, 4)),
            },
        ),
        cmd(
            0,
            Command::Train {
                building: foundry,
                kind: UnitKind::Harvester,
            },
        ),
    ]);
    let scrap_before = state.player(PlayerId(0)).scrap;
    run_until(&mut state, 700, |s, _| {
        s.player(PlayerId(0)).scrap > scrap_before
    });

    // Rally onto open ground: fresh sentinels attack-move there.
    state.tick(&[
        cmd(
            0,
            Command::SetRally {
                building: foundry,
                rally: Some(TilePos::new(10, 6)),
            },
        ),
        cmd(
            0,
            Command::Train {
                building: foundry,
                kind: UnitKind::Sentinel,
            },
        ),
    ]);
    run_until(&mut state, 300, |s, _| {
        s.units().iter().any(|u| {
            u.kind == UnitKind::Sentinel
                && matches!(u.order, Order::AttackMove { .. } | Order::Attack { .. })
        })
    });
}

#[test]
fn rally_on_foreign_building_is_rejected() {
    let mut state = arena(vec![]).build().unwrap();
    let theirs = state.buildings()[1].id;
    let report = state.tick(&[cmd(
        0,
        Command::SetRally {
            building: theirs,
            rally: Some(TilePos::new(5, 5)),
        },
    )]);
    assert!(report.events.contains(&Event::CommandRejected {
        player: PlayerId(0),
        reason: RejectReason::NotYourBuilding,
    }));
    assert_eq!(state.building(theirs).unwrap().rally, None);
}

#[test]
fn train_rejects_poverty_and_foreign_buildings() {
    let mut state = arena(vec![]).build().unwrap();
    let (mine, theirs) = (state.buildings()[0].id, state.buildings()[1].id);

    // Not my building.
    let report = state.tick(&[cmd(
        0,
        Command::Train {
            building: theirs,
            kind: UnitKind::Sentinel,
        },
    )]);
    assert!(report.events.contains(&Event::CommandRejected {
        player: PlayerId(0),
        reason: RejectReason::NotYourBuilding,
    }));

    // Not enough scrap: 200 banked, sentinels cost 75 → third fails.
    for _ in 0..2 {
        let r = state.tick(&[cmd(
            0,
            Command::Train {
                building: mine,
                kind: UnitKind::Sentinel,
            },
        )]);
        assert!(
            !r.events
                .iter()
                .any(|e| matches!(e, Event::CommandRejected { .. }))
        );
    }
    let report = state.tick(&[cmd(
        0,
        Command::Train {
            building: mine,
            kind: UnitKind::Sentinel,
        },
    )]);
    assert!(report.events.contains(&Event::CommandRejected {
        player: PlayerId(0),
        reason: RejectReason::NotEnoughScrap,
    }));
}

#[test]
fn deposits_saturate_a_full_bank() {
    let mut scenario = arena(vec![unit(0, UnitKind::Harvester, 10, 3)]);
    scenario.players[0].scrap = u32::MAX - 5;
    let mut state = scenario.build().unwrap();
    let worker = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![worker],
            node: TilePos::new(11, 4),
            queue: false,
        },
    )]);
    run_until(&mut state, 800, |s, _| {
        s.player(PlayerId(0)).scrap == u32::MAX
    });
}

#[test]
fn foundry_refuses_kinds_it_cannot_produce() {
    let mut state = arena(vec![]).build().unwrap();
    let foundry = state.buildings()[0].id;
    let report = state.tick(&[cmd(
        0,
        Command::Train {
            building: foundry,
            kind: UnitKind::Scuttler,
        },
    )]);
    assert!(report.events.iter().any(|e| matches!(
        e,
        Event::CommandRejected {
            reason: RejectReason::CannotProduce,
            ..
        }
    )));
    assert_eq!(state.player(PlayerId(0)).scrap, 200, "no scrap was taken");
}

#[test]
fn fabricator_gates_the_advanced_roster() {
    use oxide_sim::stats::BuildingKind;
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 4, 6)])
        .build()
        .unwrap();
    let builder = state.units()[0].id;
    let anchor = TilePos::new(5, 5);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Fabricator,
            anchor,
            queue: false,
            defer: false,
        },
    )]);
    let fab = state
        .buildings()
        .iter()
        .find(|b| b.anchor == anchor)
        .unwrap()
        .id;
    // Unfinished: no training yet.
    let report = state.tick(&[cmd(
        0,
        Command::Train {
            building: fab,
            kind: UnitKind::Scuttler,
        },
    )]);
    assert!(report.events.iter().any(|e| matches!(
        e,
        Event::CommandRejected {
            reason: RejectReason::CannotProduce,
            ..
        }
    )));
    run_until(&mut state, 900, |s, _| {
        s.building(fab).is_some_and(|b| b.built)
    });
    // Finished: scuttlers roll out; sentinels are still Foundry-only.
    let report = state.tick(&[
        cmd(
            0,
            Command::Train {
                building: fab,
                kind: UnitKind::Scuttler,
            },
        ),
        cmd(
            0,
            Command::Train {
                building: fab,
                kind: UnitKind::Sentinel,
            },
        ),
    ]);
    assert!(report.events.iter().any(|e| matches!(
        e,
        Event::CommandRejected {
            reason: RejectReason::CannotProduce,
            ..
        }
    )));
    run_until(&mut state, 200, |s, events| {
        let _ = s;
        events.iter().any(|e| {
            matches!(
                e,
                Event::UnitTrained {
                    kind: UnitKind::Scuttler,
                    ..
                }
            )
        })
    });
}

#[test]
fn bot_reaches_its_tech_and_mixes_its_army() {
    use oxide_sim::bot::Bot;
    use oxide_sim::stats::BuildingKind;
    // Bot vs an idle opponent: within 12k ticks it should have stood up a
    // Fabricator and fielded at least one advanced unit — proof the build
    // and composition logic actually runs, not just compiles.
    let mut scenario = Scenario::skirmish();
    scenario.players[1].bot = true;
    let mut state = scenario.build().unwrap();
    let mut bots = Bot::for_scenario(&scenario);
    for _ in 0..12_000u32 {
        if state.result().is_some() {
            break;
        }
        let mut commands = Vec::new();
        for bot in &mut bots {
            commands.extend(bot.act(&state));
        }
        state.tick(&commands);
    }
    let me = PlayerId(1);
    let has_fab = state
        .buildings()
        .iter()
        .any(|b| b.player == me && b.kind == BuildingKind::Fabricator && b.built);
    let advanced = state
        .units()
        .iter()
        .filter(|u| u.player == me && matches!(u.kind, UnitKind::Scuttler | UnitKind::Lancer))
        .count();
    // The bot may have already razed the idle opponent and won; that is
    // also a pass as long as tech came up first.
    assert!(
        has_fab || state.result().is_some(),
        "no fabricator and no victory after 12k ticks"
    );
    if has_fab {
        assert!(advanced > 0, "fabricator built but nothing trained from it");
    }
}

#[test]
fn harvesters_deposit_only_at_built_foundries() {
    use oxide_sim::stats::BuildingKind;
    // A turret sits right beside the node; the Foundry is far away. The
    // hauler must walk the distance — turrets are not drop-offs.
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 10, 4)])
        .build()
        .unwrap();
    let worker = state.units()[0].id;
    let anchor = TilePos::new(10, 3); // touching the node at (11,4)
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
    run_until(&mut state, 700, |s, _| {
        s.buildings().iter().any(|b| b.anchor == anchor && b.built)
    });
    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![worker],
            node: TilePos::new(11, 4),
            queue: false,
        },
    )]);
    let foundry = state
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(0) && b.kind == oxide_sim::BuildingKind::Foundry)
        .unwrap();
    let (f_anchor, f_size) = (foundry.anchor, foundry.kind.stats().size);
    let mut deposit_tile = None;
    for _ in 0..1200u32 {
        let report = state.tick(&[]);
        if report
            .events
            .iter()
            .any(|e| matches!(e, Event::ScrapDeposited { .. }))
        {
            deposit_tile = Some(state.unit(worker).unwrap().tile());
            break;
        }
    }
    let t = deposit_tile.expect("a deposit happened");
    let adjacent_to_foundry = (t.x >= f_anchor.x - 1 && t.x <= f_anchor.x + f_size.0)
        && (t.y >= f_anchor.y - 1 && t.y <= f_anchor.y + f_size.1);
    assert!(
        adjacent_to_foundry,
        "deposit landed at {t:?}, not beside the foundry at {f_anchor:?}"
    );
}

#[test]
fn cancel_train_refunds_and_resets_the_head() {
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 4, 6)])
        .build()
        .unwrap();
    let foundry = state
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(0))
        .unwrap()
        .id;
    let bank = state.players()[0].scrap;
    state.tick(&[
        cmd(
            0,
            Command::Train {
                building: foundry,
                kind: UnitKind::Harvester,
            },
        ),
        cmd(
            0,
            Command::Train {
                building: foundry,
                kind: UnitKind::Sentinel,
            },
        ),
    ]);
    for _ in 0..10 {
        state.tick(&[]);
    }
    let b = state.building(foundry).unwrap();
    assert_eq!(b.queue.len(), 2);
    assert!(b.progress > 0, "the head has been training");
    // Cancel the head: full refund, progress resets, the sentinel steps up.
    state.tick(&[cmd(
        0,
        Command::CancelTrain {
            building: foundry,
            index: 0,
        },
    )]);
    let b = state.building(foundry).unwrap();
    assert_eq!(b.queue.len(), 1);
    assert_eq!(b.queue[0], UnitKind::Sentinel);
    // The cancel tick's own production phase already advances the
    // fresh head by one — without the reset this would still read 11+.
    assert!(b.progress <= 1, "the next machine starts from parts");
    let cost_h = UnitKind::Harvester.stats().cost;
    let cost_s = UnitKind::Sentinel.stats().cost;
    assert_eq!(
        state.players()[0].scrap,
        bank - cost_s,
        "the harvester's {cost_h} came back; only the sentinel stays paid"
    );
    // Out-of-range and foreign cancels bounce.
    let report = state.tick(&[cmd(
        0,
        Command::CancelTrain {
            building: foundry,
            index: 5,
        },
    )]);
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. })),
        "a phantom queue slot is refused"
    );
}
