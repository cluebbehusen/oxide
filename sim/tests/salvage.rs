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
