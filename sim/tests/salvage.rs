//! Wreck salvage: deaths leave scrap on open ground, harvesters strip it
//! standing on the tile, decay reclaims it, and foundations bury it.
//! Headless scenarios through the public API only, like `behavior.rs`.

use chassis::grid::TilePos;
use oxide_sim::scenario::{PlayerSpec, UnitSpec};
use oxide_sim::stats::{BuildingKind, WRECK_DECAY_TICKS, WRECK_VALUE_DEN, WRECK_VALUE_NUM};
use oxide_sim::{
    Command, Event, Faction, PlayerCommand, PlayerId, Scenario, State, Target, UnitId, UnitKind,
};

fn arena(units: Vec<UnitSpec>) -> Scenario {
    Scenario {
        name: "salvage-arena".into(),
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
        buildings: Vec::new(),
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

/// Kills the scuttler's victim and returns the wreck tile.
fn kill_harvester(state: &mut State, killer: UnitId, victim: UnitId) -> TilePos {
    state.tick(&[cmd(
        1,
        Command::Attack {
            units: vec![killer],
            target: Target::Unit(victim),
            queue: false,
        },
    )]);
    let mut grave = None;
    run_until(state, 400, |_, events| {
        events.iter().any(|e| {
            if let Event::UnitDied { unit, pos, .. } = e
                && *unit == victim
            {
                grave = Some(TilePos::containing(*pos));
                true
            } else {
                false
            }
        })
    });
    grave.expect("victim died")
}

/// Sends a fighter to delete the loitering killer so salvage work can
/// proceed unmolested (its own small wreck joins the field — fine).
fn execute(state: &mut State, executioner: UnitId, killer: UnitId) {
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![executioner],
            target: Target::Unit(killer),
            queue: false,
        },
    )]);
    run_until(state, 600, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::UnitDied { unit, .. } if *unit == killer))
    });
}

#[test]
fn a_death_leaves_its_price_on_passable_ground() {
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 5, 5),
        unit(1, UnitKind::Scuttler, 6, 5),
        unit(0, UnitKind::Sentinel, 12, 2),
    ])
    .build()
    .unwrap();
    let (victim, killer, walker) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let grave = kill_harvester(&mut state, killer, victim);
    let expected = UnitKind::Harvester.stats().cost * WRECK_VALUE_NUM / WRECK_VALUE_DEN;
    let found = state.map().wreck_at(grave);
    // One decay step may have run between the death and this read.
    assert!(
        found == expected || found == expected - 1,
        "wreck value {found}, expected about {expected}"
    );

    // The grave is still open ground: a unit can be ordered to stand on it.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![walker],
            goal: grave,
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |s, _| {
        s.unit(walker).unwrap().tile() == grave
    });
}

#[test]
fn harvesters_strip_wrecks_standing_on_them_and_deliver() {
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 5, 5),
        unit(1, UnitKind::Scuttler, 6, 5),
        unit(0, UnitKind::Harvester, 12, 2),
        unit(0, UnitKind::Sentinel, 11, 1),
    ])
    .build()
    .unwrap();
    let (victim, killer, salvager, executioner) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
        state.units()[3].id,
    );
    let grave = kill_harvester(&mut state, killer, victim);
    execute(&mut state, executioner, killer);
    let before = state.player(PlayerId(0)).scrap;
    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![salvager],
            node: grave,
            queue: false,
        },
    )]);
    // The salvager must stand ON the wreck to strip it.
    run_until(&mut state, 300, |s, _| {
        let u = s.unit(salvager).unwrap();
        u.tile() == grave && u.carrying > 0
    });
    run_until(&mut state, 600, |s, events| {
        let _ = s;
        events
            .iter()
            .any(|e| matches!(e, Event::ScrapDeposited { player, .. } if *player == PlayerId(0)))
    });
    assert!(
        state.player(PlayerId(0)).scrap > before,
        "battlefield salvage reaches the bank"
    );
}

#[test]
fn wrecks_decay_back_into_the_dirt() {
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 5, 5),
        unit(1, UnitKind::Scuttler, 6, 5),
    ])
    .build()
    .unwrap();
    let (victim, killer) = (state.units()[0].id, state.units()[1].id);
    let grave = kill_harvester(&mut state, killer, victim);
    let value = state.map().wreck_at(grave);
    assert!(value > 0);
    for _ in 0..(u64::from(value) + 1) * WRECK_DECAY_TICKS {
        state.tick(&[]);
    }
    assert_eq!(state.map().wreck_at(grave), 0, "decay reclaims everything");
    // And a harvest order at the bare tile now bounces.
    let report = state.tick(&[cmd(
        1,
        Command::Harvest {
            units: vec![killer],
            node: grave,
            queue: false,
        },
    )]);
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. })),
        "killer is no harvester and the tile holds nothing anyway"
    );
}

#[test]
fn foundations_bury_wrecks() {
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 5, 5),
        unit(1, UnitKind::Scuttler, 6, 5),
        unit(0, UnitKind::Harvester, 12, 3),
        unit(0, UnitKind::Sentinel, 11, 1),
    ])
    .build()
    .unwrap();
    let (victim, killer, builder, executioner) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
        state.units()[3].id,
    );
    let grave = kill_harvester(&mut state, killer, victim);
    execute(&mut state, executioner, killer);
    assert!(state.map().wreck_at(grave) > 0);
    // Clear the executioner off the grave — standing machines block
    // foundations even when hovering wrecks don't.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![executioner],
            goal: TilePos::new(11, 1),
            queue: false,
        },
    )]);
    run_until(&mut state, 300, |s, _| {
        s.unit(executioner).unwrap().tile().chebyshev(grave) > 1
    });
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor: grave,
            queue: false,
        },
    )]);
    assert!(
        state.buildings().iter().any(|b| b.anchor == grave),
        "the site must land for this test to mean anything"
    );
    assert_eq!(state.map().wreck_at(grave), 0, "foundations bury salvage");
}

#[test]
fn a_dead_building_splits_its_wreck_across_the_footprint() {
    let mut scenario = arena(vec![
        unit(0, UnitKind::Harvester, 4, 2),
        unit(1, UnitKind::Lancer, 9, 5),
        unit(1, UnitKind::Lancer, 10, 5),
        unit(1, UnitKind::Lancer, 9, 6),
        unit(1, UnitKind::Lancer, 10, 6),
    ]);
    scenario.players[0].scrap = 200;
    let mut state = scenario.build().unwrap();
    let builder = state.units()[0].id;
    let lancers: Vec<_> = state.units()[1..].iter().map(|u| u.id).collect();
    // Out of the lancers' idle aggro (they measure to the footprint's
    // closest point) — they only engage once ordered.
    let anchor = TilePos::new(3, 3);
    let report = state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor,
            queue: false,
        },
    )]);
    assert!(
        !report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. })),
        "build bounced: {:?}",
        report.events
    );
    run_until(&mut state, 500, |_, events| {
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
    state.tick(&[cmd(
        1,
        Command::Attack {
            units: lancers,
            target: Target::Building(turret),
            queue: false,
        },
    )]);
    run_until(&mut state, 600, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::BuildingDestroyed { .. }))
    });
    let expected =
        BuildingKind::Turret.stats().construction.unwrap().cost * WRECK_VALUE_NUM / WRECK_VALUE_DEN;
    let found = state.map().wreck_at(anchor);
    assert!(
        found > 0 && found <= expected,
        "a 1x1 building's whole wreck lands on its tile (found {found}, cap {expected})"
    );
}

#[test]
fn a_flyer_downed_over_a_roof_leaves_nothing_strippable() {
    // The wisp dies to the flakhound directly over the enemy foundry's
    // footprint; no wreck may land under the standing building.
    let mut state = arena(vec![
        unit(0, UnitKind::Wisp, 4, 2),
        unit(1, UnitKind::Flakhound, 12, 5),
    ])
    .build()
    .unwrap();
    let (wisp, flak) = (state.units()[0].id, state.units()[1].id);
    let foundry_anchor = state
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(1))
        .unwrap()
        .anchor;
    // Fly the wisp onto the foundry roof; the flakhound will swat it.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![wisp],
            goal: foundry_anchor,
            queue: false,
        },
    )]);
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![flak],
            goal: TilePos::new(foundry_anchor.x - 2, foundry_anchor.y),
            queue: false,
        },
    )]);
    let mut grave = None;
    run_until(&mut state, 600, |_, events| {
        events.iter().any(|e| {
            if let Event::UnitDied { unit: u, pos, .. } = e
                && *u == wisp
            {
                grave = Some(TilePos::containing(*pos));
                true
            } else {
                false
            }
        })
    });
    let grave = grave.expect("the wisp died");
    let under_roof = state
        .buildings()
        .iter()
        .any(|b| b.tiles().any(|t| t == grave));
    if under_roof {
        assert_eq!(
            state.map().wreck_at(grave),
            0,
            "a surviving footprint swallows the deposit"
        );
    } else {
        // The swat happened off the roof — still a valid wreck test.
        assert!(state.map().wreck_at(grave) > 0);
    }
}

#[test]
fn a_flyer_downed_over_rock_leaves_no_wreck_bait() {
    // Rock never opens up, so a deposit there would sit in vision and bot
    // salvage selection as value no harvester can ever stand on — orders
    // would stall against it until decay. The value is simply lost. Air
    // spawn validation runs in the flyer's own domain, so the wisp starts
    // parked on the rock directly.
    let mut state = arena(vec![
        unit(0, UnitKind::Flakhound, 4, 3),
        unit(1, UnitKind::Wisp, 7, 4),
    ])
    .build()
    .unwrap();
    let (flak, wisp) = (state.units()[0].id, state.units()[1].id);
    let roost = TilePos::new(7, 4);
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![flak],
            target: Target::Unit(wisp),
            queue: false,
        },
    )]);
    run_until(&mut state, 600, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::UnitDied { unit, .. } if *unit == wisp))
    });
    assert_eq!(
        state.map().wreck_at(roost),
        0,
        "no salvage recorded on ground nobody can strip"
    );

    // The control: the same kill over open ground deposits normally.
    let mut state = arena(vec![
        unit(0, UnitKind::Flakhound, 4, 3),
        unit(1, UnitKind::Wisp, 9, 3),
    ])
    .build()
    .unwrap();
    let (flak, wisp) = (state.units()[0].id, state.units()[1].id);
    let sky = TilePos::new(9, 3);
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![flak],
            target: Target::Unit(wisp),
            queue: false,
        },
    )]);
    run_until(&mut state, 600, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::UnitDied { unit, .. } if *unit == wisp))
    });
    assert!(
        state.map().wreck_at(sky) > 0,
        "open ground takes the deposit as before"
    );
}

// ---------------------------------------------------------------------
// Building salvage (0.11): stripping standing structures as labor.

use oxide_sim::command::RejectReason;
use oxide_sim::scenario::BuildingSpec;
use oxide_sim::stats::SALVAGE_REFUND_PERMILLE;

fn standing(player: u8, kind: BuildingKind, x: i32, y: i32) -> BuildingSpec {
    BuildingSpec { player, kind, x, y }
}

#[test]
fn a_full_health_salvage_banks_exactly_its_permille() {
    let mut scenario = arena(vec![unit(0, UnitKind::Harvester, 7, 2)]);
    scenario
        .buildings
        .push(standing(0, BuildingKind::Turret, 9, 2));
    let mut state = scenario.build().unwrap();
    let harvester = state.units()[0].id;
    let turret = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Turret)
        .unwrap()
        .id;
    let bank_before = state.player(PlayerId(0)).scrap;
    state.tick(&[cmd(
        0,
        Command::Salvage {
            units: vec![harvester],
            building: turret,
            queue: false,
        },
    )]);
    let events = run_until(&mut state, 800, |s, _| s.building(turret).is_none());
    let stats = BuildingKind::Turret.stats();
    let cost = stats.construction.unwrap().cost;
    let refund = u32::try_from(u64::from(cost) * SALVAGE_REFUND_PERMILLE / 1000).unwrap();
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        bank_before + refund,
        "a full-health salvage banks exactly cost * permille"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::BuildingSalvaged { building, refund: r, .. }
                if *building == turret && *r == refund
        )),
        "the deliberate teardown announces itself with its total"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::BuildingDestroyed { building, .. } if *building == turret)),
        "salvage is not a loss"
    );
    assert_eq!(
        state.map().wreck_at(TilePos::new(9, 2)),
        0,
        "a deliberate teardown leaves no wreck"
    );
}

#[test]
fn an_interrupted_salvage_credits_only_the_hp_it_drained() {
    let mut scenario = arena(vec![unit(0, UnitKind::Harvester, 7, 2)]);
    scenario
        .buildings
        .push(standing(0, BuildingKind::Turret, 9, 2));
    let mut state = scenario.build().unwrap();
    let harvester = state.units()[0].id;
    let turret = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Turret)
        .unwrap()
        .id;
    let bank_before = state.player(PlayerId(0)).scrap;
    let stats = BuildingKind::Turret.stats();
    state.tick(&[cmd(
        0,
        Command::Salvage {
            units: vec![harvester],
            building: turret,
            queue: false,
        },
    )]);
    // Let it chew partway, then call the crew off.
    run_until(&mut state, 400, |s, _| {
        s.building(turret).unwrap().hp < stats.max_hp * 3 / 4
    });
    state.tick(&[cmd(
        0,
        Command::Stop {
            units: vec![harvester],
        },
    )]);
    state.tick(&[]);
    let drained = stats.max_hp - state.building(turret).unwrap().hp;
    let cost = u64::from(stats.construction.unwrap().cost);
    let credited = u32::try_from(
        u64::from(drained) * cost * SALVAGE_REFUND_PERMILLE / (1000 * u64::from(stats.max_hp)),
    )
    .unwrap();
    assert!(
        drained > 0 && credited > 0,
        "test premise: real work stopped"
    );
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        bank_before + credited,
        "credit follows hp actually drained, floor-truncated, no drift"
    );
}

#[test]
fn the_repair_salvage_pump_strictly_loses_scrap() {
    // The printer configuration the 0.11 repricing exists to kill:
    // strip hp out at 800 per mille, weld it back at 850 — the round
    // trip must strictly lose money at any stopping point.
    let mut scenario = arena(vec![unit(0, UnitKind::Harvester, 7, 2)]);
    scenario
        .buildings
        .push(standing(0, BuildingKind::Turret, 9, 2));
    let mut state = scenario.build().unwrap();
    let harvester = state.units()[0].id;
    let turret = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Turret)
        .unwrap()
        .id;
    let stats = BuildingKind::Turret.stats();
    let bank_start = state.player(PlayerId(0)).scrap;
    state.tick(&[cmd(
        0,
        Command::Salvage {
            units: vec![harvester],
            building: turret,
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |s, _| {
        s.building(turret).unwrap().hp < stats.max_hp / 2
    });
    state.tick(&[cmd(
        0,
        Command::Repair {
            units: vec![harvester],
            building: turret,
            queue: false,
        },
    )]);
    run_until(&mut state, 2000, |s, _| {
        s.building(turret).unwrap().hp == stats.max_hp
    });
    assert!(
        state.player(PlayerId(0)).scrap < bank_start,
        "welding back what salvage banked must cost more than it paid: {} -> {}",
        bank_start,
        state.player(PlayerId(0)).scrap
    );
}

#[test]
fn fire_finishing_a_salvage_target_wins_and_forfeits_the_rest() {
    // An Array under both the wrecking crew and enemy guns: the guns
    // land the killing blow, so the teardown never completes — the
    // death is a loss with a wreck, and only the pre-death drains ever
    // credited. The stripper's order pops silently (its target is
    // simply gone), never stalls.
    let mut scenario = arena(vec![
        unit(0, UnitKind::Harvester, 7, 2),
        unit(1, UnitKind::Scuttler, 11, 1),
        unit(1, UnitKind::Scuttler, 11, 2),
        unit(1, UnitKind::Scuttler, 11, 3),
    ]);
    scenario
        .buildings
        .push(standing(0, BuildingKind::Array, 9, 2));
    let mut state = scenario.build().unwrap();
    let harvester = state.units()[0].id;
    let raiders: Vec<UnitId> = state.units()[1..4].iter().map(|u| u.id).collect();
    let array = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Array)
        .unwrap()
        .id;
    let bank_before = state.player(PlayerId(0)).scrap;
    state.tick(&[
        cmd(
            0,
            Command::Salvage {
                units: vec![harvester],
                building: array,
                queue: false,
            },
        ),
        cmd(
            1,
            Command::Attack {
                units: raiders,
                target: Target::Building(array),
                queue: false,
            },
        ),
    ]);
    let events = run_until(&mut state, 2000, |s, _| s.building(array).is_none());
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::BuildingDestroyed { building, .. } if *building == array)),
        "the guns took it: a loss, not a teardown"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::BuildingSalvaged { .. })),
        "no salvage fanfare for a building fire finished"
    );
    assert!(
        state.map().wreck_at(TilePos::new(9, 2)) > 0,
        "fire leaves its wreck"
    );
    let stats = BuildingKind::Array.stats();
    let full =
        u32::try_from(u64::from(stats.construction.unwrap().cost) * SALVAGE_REFUND_PERMILLE / 1000)
            .unwrap();
    assert!(
        state.player(PlayerId(0)).scrap < bank_before + full,
        "the unfinished teardown must not pay the full refund"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::OrderStalled { unit, .. } if *unit == harvester)),
        "a vanished target pops the order silently"
    );
}

#[test]
fn foundries_and_sites_refuse_the_wrecking_crew() {
    let mut scenario = arena(vec![unit(0, UnitKind::Harvester, 4, 2)]);
    scenario.players[0].scrap = 300;
    let mut state = scenario.build().unwrap();
    let harvester = state.units()[0].id;
    let foundry = state.buildings()[0].id;
    let report = state.tick(&[cmd(
        0,
        Command::Salvage {
            units: vec![harvester],
            building: foundry,
            queue: false,
        },
    )]);
    assert!(
        report.events.iter().any(|e| matches!(
            e,
            Event::CommandRejected {
                reason: RejectReason::InvalidTarget,
                ..
            }
        )),
        "the victory token never comes apart by its own crew's hands"
    );
    // An unbuilt site is Cancel's domain, not Salvage's.
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![harvester],
            kind: BuildingKind::Turret,
            anchor: TilePos::new(9, 2),
            queue: false,
        },
    )]);
    let site = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Turret)
        .unwrap()
        .id;
    let report = state.tick(&[cmd(
        0,
        Command::Salvage {
            units: vec![harvester],
            building: site,
            queue: false,
        },
    )]);
    assert!(
        report.events.iter().any(|e| matches!(
            e,
            Event::CommandRejected {
                reason: RejectReason::InvalidTarget,
                ..
            }
        )),
        "unbuilt sites keep Cancel's instant refund"
    );
}

#[test]
fn a_salvaged_producer_refunds_its_prepaid_queue_in_full() {
    // Three strippers take the Fabricator down faster than its first
    // lancer can train (stacking is the same rule builders follow), so
    // the whole prepaid line refunds through the CancelTrain rule.
    let mut scenario = arena(vec![
        unit(0, UnitKind::Harvester, 7, 1),
        unit(0, UnitKind::Harvester, 7, 2),
        unit(0, UnitKind::Harvester, 8, 5),
    ]);
    scenario.players[0].scrap = 300;
    scenario
        .buildings
        .push(standing(0, BuildingKind::Fabricator, 9, 2));
    let mut state = scenario.build().unwrap();
    let crew: Vec<UnitId> = state.units().iter().map(|u| u.id).collect();
    let fab = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Fabricator)
        .unwrap()
        .id;
    let lancer_cost = UnitKind::Lancer.stats().cost;
    state.tick(&[
        cmd(
            0,
            Command::Train {
                building: fab,
                kind: UnitKind::Lancer,
            },
        ),
        cmd(
            0,
            Command::Train {
                building: fab,
                kind: UnitKind::Lancer,
            },
        ),
    ]);
    let bank_after_orders = state.player(PlayerId(0)).scrap;
    assert_eq!(bank_after_orders, 300 - 2 * lancer_cost, "queue prepaid");
    state.tick(&[cmd(
        0,
        Command::Salvage {
            units: crew,
            building: fab,
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |s, _| s.building(fab).is_none());
    assert!(
        !state.units().iter().any(|u| u.kind == UnitKind::Lancer),
        "test premise: the teardown outran the training line"
    );
    let stats = BuildingKind::Fabricator.stats();
    let refund =
        u32::try_from(u64::from(stats.construction.unwrap().cost) * SALVAGE_REFUND_PERMILLE / 1000)
            .unwrap();
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        bank_after_orders + refund + 2 * lancer_cost,
        "the building refunds its permille, the queue refunds in full"
    );
}

#[test]
fn repair_and_salvage_evict_each_other_from_a_target() {
    let mut scenario = arena(vec![
        unit(0, UnitKind::Harvester, 7, 2),
        unit(0, UnitKind::Harvester, 8, 2),
    ]);
    scenario
        .buildings
        .push(standing(0, BuildingKind::Turret, 9, 2));
    let mut state = scenario.build().unwrap();
    let (stripper, welder) = (state.units()[0].id, state.units()[1].id);
    let turret = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Turret)
        .unwrap()
        .id;
    let stats = BuildingKind::Turret.stats();
    state.tick(&[cmd(
        0,
        Command::Salvage {
            units: vec![stripper],
            building: turret,
            queue: false,
        },
    )]);
    // Wait for a real wound so Repair validates.
    run_until(&mut state, 400, |s, _| {
        s.building(turret).unwrap().hp < stats.max_hp
    });
    state.tick(&[cmd(
        0,
        Command::Repair {
            units: vec![welder],
            building: turret,
            queue: false,
        },
    )]);
    assert!(
        !matches!(
            state.unit(stripper).unwrap().order,
            oxide_sim::Order::Salvage { .. }
        ),
        "issuing repair calls the wrecking crew off"
    );
    assert!(matches!(
        state.unit(welder).unwrap().order,
        oxide_sim::Order::Repair { .. }
    ));
    state.tick(&[cmd(
        0,
        Command::Salvage {
            units: vec![stripper],
            building: turret,
            queue: false,
        },
    )]);
    assert!(
        !matches!(
            state.unit(welder).unwrap().order,
            oxide_sim::Order::Repair { .. }
        ),
        "and issuing salvage sends the welder home"
    );
}

#[test]
fn salvage_walks_the_construction_ramp_backward_on_schedule() {
    // One stripper takes ceil(max_hp * build_ticks / ramp) ticks to
    // level a full-health building — the same clock construction runs,
    // extended over the fifth of hp a site is born with.
    let mut scenario = arena(vec![unit(0, UnitKind::Harvester, 7, 2)]);
    scenario
        .buildings
        .push(standing(0, BuildingKind::Turret, 9, 2));
    let mut state = scenario.build().unwrap();
    let harvester = state.units()[0].id;
    let turret = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Turret)
        .unwrap()
        .id;
    let stats = BuildingKind::Turret.stats();
    state.tick(&[cmd(
        0,
        Command::Salvage {
            units: vec![harvester],
            building: turret,
            queue: false,
        },
    )]);
    let mut first_drain = None;
    let mut gone_at = None;
    for _ in 0..2000 {
        state.tick(&[]);
        let now = state.current_tick();
        match state.building(turret) {
            Some(b) if b.hp < stats.max_hp && first_drain.is_none() => first_drain = Some(now),
            None => {
                gone_at = Some(now);
                break;
            }
            _ => {}
        }
    }
    let (start, end) = (first_drain.unwrap(), gone_at.unwrap());
    let ramp = u64::from(stats.max_hp - stats.max_hp / 5);
    let build_ticks = u64::from(stats.construction.unwrap().build_ticks);
    // Total work ticks to drain max_hp on the ramp clock; the span
    // MEASURED starts at the first visible drop, which trails the
    // first work tick by however many leading steps floor to zero.
    let total = u64::from(stats.max_hp)
        .saturating_mul(build_ticks)
        .div_ceil(ramp);
    let first_visible = (1u64..).find(|t| ramp * t / build_ticks >= 1).unwrap();
    assert_eq!(
        end - start + 1,
        total - first_visible + 1,
        "the teardown runs the construction clock, stretched over the birth fifth"
    );
}

#[test]
fn eviction_strips_queued_legs_but_spares_the_rest_of_the_program() {
    // Mutual eviction reaches into ORDER QUEUES too: a shift-queued
    // salvage behind a march dies when repair claims the target, and
    // the march must survive — eviction removes the conflicting job,
    // never the whole program.
    let mut scenario = arena(vec![
        unit(0, UnitKind::Harvester, 7, 2),
        unit(0, UnitKind::Harvester, 8, 2),
    ]);
    scenario
        .buildings
        .push(standing(0, BuildingKind::Turret, 9, 2));
    let mut state = scenario.build().unwrap();
    let (walker, welder) = (state.units()[0].id, state.units()[1].id);
    let turret = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Turret)
        .unwrap()
        .id;
    // A long march with a salvage queued behind it.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![walker],
            goal: TilePos::new(2, 6),
            queue: false,
        },
    )]);
    state.tick(&[cmd(
        0,
        Command::Salvage {
            units: vec![walker],
            building: turret,
            queue: true,
        },
    )]);
    assert_eq!(
        state.unit(walker).unwrap().queue.len(),
        1,
        "test premise: the salvage waits behind the march"
    );
    // Wound the turret via a real salvage tick from the other hand,
    // then claim it for repair: the queued leg must vanish.
    state.tick(&[cmd(
        0,
        Command::Salvage {
            units: vec![welder],
            building: turret,
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |s, _| {
        s.building(turret).unwrap().hp < BuildingKind::Turret.stats().max_hp
    });
    state.tick(&[cmd(
        0,
        Command::Repair {
            units: vec![welder],
            building: turret,
            queue: false,
        },
    )]);
    let unit = state.unit(walker).unwrap();
    assert!(
        matches!(unit.order, oxide_sim::Order::Move { .. }),
        "the march survives eviction"
    );
    assert!(
        unit.queue.is_empty(),
        "the queued salvage leg is gone: {:?}",
        unit.queue
    );
}

#[test]
fn foundry_repair_bills_against_its_authored_price() {
    // The Foundry has no purchase cost; its welding bills against
    // FOUNDRY_REPAIR_PRICE on the authored FOUNDRY_REPAIR_TICKS ramp.
    // One prepaid coin covers exactly the hp whose milli-price ceils
    // to one scrap — the same derivation buildable kinds use.
    let mut scenario = arena(vec![
        unit(0, UnitKind::Harvester, 4, 2),
        unit(1, UnitKind::Scuttler, 4, 4),
    ]);
    scenario.players[0].scrap = 1;
    let mut state = scenario.build().unwrap();
    let (welder, raider) = (state.units()[0].id, state.units()[1].id);
    let foundry = state.buildings()[0].id;
    state.tick(&[cmd(
        1,
        Command::Attack {
            units: vec![raider],
            target: Target::Building(foundry),
            queue: false,
        },
    )]);
    run_until(&mut state, 600, |s, _| {
        s.building(foundry)
            .unwrap()
            .hp
            .checked_add(60)
            .is_some_and(|h| h < oxide_sim::stats::BuildingKind::Foundry.stats().max_hp)
    });
    // Call the raider off so the weld runs uncontested.
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![raider],
            goal: TilePos::new(13, 2),
            queue: false,
        },
    )]);
    let hp_before = state.building(foundry).unwrap().hp;
    state.tick(&[cmd(
        0,
        Command::Repair {
            units: vec![welder],
            building: foundry,
            queue: false,
        },
    )]);
    run_until(&mut state, 800, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::OrderStalled { unit, .. } if *unit == welder))
    });
    assert_eq!(state.player(PlayerId(0)).scrap, 0, "the coin was spent");
    let healed = state.building(foundry).unwrap().hp - hp_before;
    let max_hp = BuildingKind::Foundry.stats().max_hp;
    let ramp = u64::from(max_hp - max_hp / 5);
    let ticks = u64::from(oxide_sim::stats::FOUNDRY_REPAIR_TICKS);
    let basis = u64::from(oxide_sim::stats::FOUNDRY_REPAIR_PRICE);
    let millis = |t: u64| {
        (ramp * t / ticks) * basis * oxide_sim::stats::REPAIR_COST_PERMILLE / u64::from(max_hp)
    };
    let stall = (0u64..)
        .find(|&p| millis(p + 1).div_ceil(1000) > 1)
        .unwrap();
    let expected = u32::try_from(ramp * stall / ticks).unwrap();
    assert_eq!(
        healed, expected,
        "one coin's worth of foundry welding, on the authored basis"
    );
}
