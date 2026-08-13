//! Buildable Foundries: the 0.15 expansion base. Construction sits
//! behind the Fabricator tech gate, sites count for survival exactly
//! like standing works, abandoned scaffolds rust away, and every
//! completed Foundry smelts the transparent drip.

use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::stats::{BuildingKind, FOUNDRY_DRIP_PERIOD, FOUNDRY_DRIP_START_TICK};
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

fn arena(scrap: u32, fabricator: bool, units: Vec<UnitSpec>) -> Scenario {
    let mut buildings = Vec::new();
    if fabricator {
        buildings.push(BuildingSpec {
            player: 0,
            kind: BuildingKind::Fabricator,
            x: 3,
            y: 6,
        });
    }
    Scenario {
        name: "foundry-arena".into(),
        seed: 5,
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
        units,
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

fn build_foundry(builder: oxide_sim::UnitId, anchor: TilePos) -> PlayerCommand {
    cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Foundry,
            anchor,
            queue: false,
            defer: false,
        },
    )
}

fn harvester(x: i32, y: i32) -> UnitSpec {
    UnitSpec {
        player: 0,
        kind: UnitKind::Harvester,
        x,
        y,
    }
}

#[test]
fn a_foundry_expansion_needs_a_standing_fabricator() {
    let mut bare = arena(1_000, false, vec![harvester(8, 4)]).build().unwrap();
    let builder = bare.units()[0].id;
    let report = bare.tick(&[build_foundry(builder, TilePos::new(10, 4))]);
    assert!(
        report.events.iter().any(|e| matches!(
            e,
            Event::CommandRejected {
                reason: RejectReason::MissingPrerequisite,
                ..
            }
        )),
        "the tech gate names the missing prerequisite"
    );
    assert_eq!(bare.player(PlayerId(0)).scrap, 1_000, "nothing was charged");

    let mut teched = arena(1_000, true, vec![harvester(8, 4)]).build().unwrap();
    let builder = teched.units()[0].id;
    teched.tick(&[build_foundry(builder, TilePos::new(10, 4))]);
    assert_eq!(
        teched.player(PlayerId(0)).scrap,
        1_000
            - BuildingKind::Foundry
                .base_stats()
                .construction
                .unwrap()
                .cost,
        "the site claims its full price on placement"
    );
    assert!(
        teched
            .buildings()
            .iter()
            .any(|b| b.kind == BuildingKind::Foundry
                && !b.built
                && b.anchor == TilePos::new(10, 4)),
        "the expansion site stands"
    );
}

#[test]
fn a_completed_expansion_produces_and_smelts_its_own_drip() {
    let mut state = arena(1_000, true, vec![harvester(8, 4)]).build().unwrap();
    let builder = state.units()[0].id;
    state.tick(&[build_foundry(builder, TilePos::new(10, 4))]);
    let expansion = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Foundry && !b.built)
        .unwrap()
        .id;
    for _ in 0..BuildingKind::Foundry
        .base_stats()
        .construction
        .unwrap()
        .build_ticks
        + 40
    {
        state.tick(&[]);
        if state.building(expansion).is_some_and(|b| b.built) {
            break;
        }
    }
    assert!(
        state.building(expansion).unwrap().built,
        "the attended site completes"
    );
    let bank = state.player(PlayerId(0)).scrap;
    let report = state.tick(&[cmd(
        0,
        Command::Train {
            building: expansion,
            kind: UnitKind::Harvester,
        },
    )]);
    assert!(
        !report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. })),
        "an expansion Foundry trains the basics"
    );
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        bank - UnitKind::Harvester.stats().cost
    );

    // Two standing Foundries smelt two per period once the warm-up ends.
    let mut value = serde_json::to_value(&state).unwrap();
    value["tick"] = serde_json::json!(FOUNDRY_DRIP_START_TICK - 1);
    let mut state: State = serde_json::from_value(value).unwrap();
    let bank = state.player(PlayerId(0)).scrap;
    state.tick(&[]);
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        bank + 2,
        "each completed Foundry smelts its own drip credit"
    );
    for _ in 0..FOUNDRY_DRIP_PERIOD - 1 {
        state.tick(&[]);
    }
    assert_eq!(state.player(PlayerId(0)).scrap, bank + 2);
    state.tick(&[]);
    assert_eq!(state.player(PlayerId(0)).scrap, bank + 4);
}

#[test]
fn a_foundry_site_keeps_its_team_alive() {
    let mut state = arena(1_000, true, vec![harvester(8, 4)]).build().unwrap();
    let builder = state.units()[0].id;
    state.tick(&[build_foundry(builder, TilePos::new(10, 4))]);
    let standing = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Foundry && b.built && b.player == PlayerId(0))
        .unwrap()
        .id;

    // The standing Foundry falls; the site under construction remains.
    // Removing a live id through the trust boundary is legal (ids are
    // never reused), so the victory rule is probed directly.
    let mut value = serde_json::to_value(&state).unwrap();
    value["buildings"]
        .as_array_mut()
        .unwrap()
        .retain(|b| b["id"] != serde_json::json!(standing.0));
    let mut state: State = serde_json::from_value(value).unwrap();
    let report = state.tick(&[]);
    assert!(
        state.result().is_none(),
        "a Foundry site under construction keeps the seat in the game"
    );
    assert!(
        !report
            .events
            .iter()
            .any(|e| matches!(e, Event::GameOver { .. }))
    );

    // The site falls too: now the seat is out and the match decides.
    let site = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Foundry && b.player == PlayerId(0))
        .unwrap()
        .id;
    let mut value = serde_json::to_value(&state).unwrap();
    value["buildings"]
        .as_array_mut()
        .unwrap()
        .retain(|b| b["id"] != serde_json::json!(site.0));
    let mut state: State = serde_json::from_value(value).unwrap();
    let fell_at = state.current_tick();
    state.tick(&[]);
    assert!(
        matches!(
            state.result(),
            Some(oxide_sim::state::GameResult::Victory { team }) if team == state.player(PlayerId(1)).team
        ),
        "no Foundry and no site ends it"
    );
    assert_eq!(
        state.player(PlayerId(0)).eliminated_at,
        Some(fell_at),
        "the loser's elimination tick is stamped — the FFA placement key"
    );
    assert_eq!(
        state.player(PlayerId(1)).eliminated_at,
        None,
        "survivors carry no stamp"
    );
    // The stamp is written once and never moves.
    for _ in 0..5 {
        state.tick(&[]);
    }
    assert_eq!(state.player(PlayerId(0)).eliminated_at, Some(fell_at));
}

#[test]
fn a_surrender_stamps_the_same_elimination_clock() {
    let mut state = arena(1_000, false, vec![]).build().unwrap();
    for _ in 0..7 {
        state.tick(&[]);
    }
    let conceded_at = state.current_tick();
    state.tick(&[cmd(1, Command::Surrender)]);
    assert_eq!(
        state.player(PlayerId(1)).eliminated_at,
        Some(conceded_at),
        "a concession stamps the seat out on the spot"
    );
}

#[test]
fn abandoned_sites_rust_away_and_attended_sites_do_not() {
    let mut state = arena(1_000, true, vec![harvester(8, 4), harvester(12, 6)])
        .build()
        .unwrap();
    let (builder, bystander) = (state.units()[0].id, state.units()[1].id);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor: TilePos::new(9, 4),
            queue: false,
            defer: false,
        },
    )]);
    let site = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Turret)
        .unwrap()
        .id;
    // Attended (the builder works it): the site only ever gains hp.
    let hp_start = state.building(site).unwrap().hp;
    for _ in 0..40 {
        state.tick(&[]);
    }
    assert!(
        state.building(site).unwrap().hp > hp_start,
        "an attended site grows"
    );

    // Called away: the scaffold decays one hp per period.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![builder],
            goal: TilePos::new(16, 2),
            queue: false,
        },
    )]);
    for _ in 0..60 {
        state.tick(&[]);
    }
    let hp_abandoned = state.building(site).unwrap().hp;
    for _ in 0..80 {
        state.tick(&[]);
    }
    let decayed = state.building(site).unwrap().hp;
    assert!(
        decayed < hp_abandoned,
        "an abandoned scaffold rusts ({hp_abandoned} -> {decayed})"
    );

    // A parked harvester beside the footprint counts as attendance.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![bystander],
            goal: TilePos::new(10, 4),
            queue: false,
        },
    )]);
    for _ in 0..40 {
        state.tick(&[]);
    }
    let held = state.building(site).unwrap().hp;
    for _ in 0..80 {
        state.tick(&[]);
    }
    assert!(
        state.building(site).unwrap().hp >= held,
        "an attended scaffold stops rusting"
    );

    // Left alone for good, the scaffold dies through the ordinary
    // destroyed-building path.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![bystander],
            goal: TilePos::new(16, 2),
            queue: false,
        },
    )]);
    let mut destroyed = false;
    for _ in 0..2_000 {
        let report = state.tick(&[]);
        if report
            .events
            .iter()
            .any(|e| matches!(e, Event::BuildingDestroyed { building, .. } if *building == site))
        {
            destroyed = true;
            break;
        }
    }
    assert!(destroyed, "a scaffold nobody returns to eventually falls");
}
