//! Teams: shared sight, unattackable allies, team-scoped victory, and
//! spectating seats. Headless scenarios through the public API only.

use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::scenario::{PlayerSpec, ScenarioError, UnitSpec};
use oxide_sim::{
    Command, Event, Faction, GameResult, Order, PlayerCommand, PlayerId, Scenario, State, Target,
    UnitKind,
};

/// A 2v2 arena: west team (seats 0, 1) against east team (seats 2, 3).
fn arena4(units: Vec<UnitSpec>) -> Scenario {
    let seat = |name: &str, faction, team| PlayerSpec {
        name: name.into(),
        faction,
        team: Some(team),
        scrap: 300,
        bot: false,
        bot_config: None,
    };
    Scenario {
        name: "team-arena".into(),
        seed: 42,
        map: vec![
            "########################".into(),
            "#1..................3..#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#2..................4..#".into(),
            "#......................#".into(),
            "########################".into(),
        ],
        players: vec![
            seat("West Ferrous", Faction::Ferrous, 0),
            seat("West Cupric", Faction::Cupric, 0),
            seat("East Ferrous", Faction::Ferrous, 1),
            seat("East Cupric", Faction::Cupric, 1),
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

#[test]
fn a_single_team_scenario_is_rejected() {
    let mut scenario = arena4(vec![]);
    for p in scenario.players.iter_mut() {
        p.team = Some(0);
    }
    assert!(matches!(scenario.build(), Err(ScenarioError::OneTeam)));
}

#[test]
fn allied_fighters_ignore_and_cannot_target_each_other() {
    let mut state = arena4(vec![
        unit(0, UnitKind::Sentinel, 8, 5),
        unit(1, UnitKind::Sentinel, 9, 5),
    ])
    .build()
    .unwrap();
    let (mine, ally) = (state.units()[0].id, state.units()[1].id);
    // Adjacent allied fighters ignore each other completely.
    for _ in 0..50 {
        state.tick(&[]);
    }
    assert_eq!(state.unit(mine).unwrap().order, Order::Idle);
    assert_eq!(
        state.unit(ally).unwrap().hp,
        UnitKind::Sentinel.stats().max_hp,
        "an untouched ally sits at full health"
    );

    // An attack command on a teammate bounces.
    let report = state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![mine],
            target: Target::Unit(ally),
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
fn splash_spares_the_teammate_in_the_blast() {
    // The enemy harvester stands one tile from the allied sentinel;
    // the shell's radius covers both, and only the enemy dies. (A
    // pacifist foe: a scuttler would brawl the sentinel through the
    // flight and muddy the hp ledger the assert reads.)
    let mut state = arena4(vec![
        unit(1, UnitKind::Sentinel, 9, 5),
        unit(2, UnitKind::Harvester, 10, 5),
        unit(0, UnitKind::Bombard, 8, 8),
    ])
    .build()
    .unwrap();
    let (ally, foe, bombard) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let ally_hp = state.unit(ally).unwrap().hp;
    // The shell launches on the command tick and lands after real
    // flight; the ally's sentinel chews the pacifist foe meanwhile,
    // which touches no ledger the asserts read.
    let report = state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![bombard],
            target: Target::Unit(foe),
            queue: false,
        },
    )]);
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e, Event::ShellLaunched { .. })),
        "the shell flies on the command tick"
    );
    for _ in 0..30 {
        state.tick(&[]);
        if state.unit(foe).is_none() {
            break;
        }
    }
    assert!(
        state.unit(foe).is_none(),
        "the shell deletes the raider it was aimed at"
    );
    assert_eq!(
        state.unit(ally).unwrap().hp,
        ally_hp,
        "splash never touches a teammate"
    );
}

#[test]
fn team_sight_is_shared() {
    let mut state = arena4(vec![
        unit(1, UnitKind::Sentinel, 15, 8),
        unit(2, UnitKind::Harvester, 17, 8),
    ])
    .build()
    .unwrap();
    state.tick(&[]);
    let spot = TilePos::new(17, 8);
    assert!(
        state.can_see(PlayerId(0), spot),
        "an ally's eyes are the team's eyes"
    );
    let obs = oxide_sim::bot::Observation::fog_honest(&state, PlayerId(0));
    assert!(
        obs.enemy_units
            .iter()
            .any(|u| u.player == PlayerId(2) && u.kind == UnitKind::Harvester),
        "the enemy the ally sees is known to the whole team"
    );
    assert!(
        obs.ally_units.iter().any(|u| u.player == PlayerId(1)),
        "and the ally itself is listed as an ally, not an enemy"
    );
    assert!(obs.enemy_units.iter().all(|u| u.player != PlayerId(1)));
}

#[test]
fn victory_takes_every_enemy_foundry_and_spectators_stay_muted() {
    let mut state = arena4(vec![
        unit(0, UnitKind::Bombard, 16, 3),
        unit(0, UnitKind::Bombard, 17, 4),
        unit(0, UnitKind::Bombard, 16, 5),
        unit(0, UnitKind::Bombard, 17, 5),
        unit(0, UnitKind::Bombard, 18, 5),
        unit(0, UnitKind::Bombard, 19, 5),
        unit(2, UnitKind::Harvester, 14, 2),
        unit(0, UnitKind::Scuttler, 12, 8),
    ])
    .build()
    .unwrap();
    let guns: Vec<_> = state.units()[..6].iter().map(|u| u.id).collect();
    let survivor = state.units()[6].id;
    let scout = state.units()[7].id;
    let east_foundry = state
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(2))
        .unwrap()
        .id;
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: guns.clone(),
            target: Target::Building(east_foundry),
            queue: false,
        },
    )]);
    run_until(&mut state, 1200, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::BuildingDestroyed { .. }))
    });
    assert!(
        state.result().is_none(),
        "half a team down is not a victory"
    );
    // The foundry-less seat is a spectator; its teammate plays on.
    let report = state.tick(&[cmd(
        2,
        Command::Move {
            units: vec![survivor],
            goal: TilePos::new(12, 6),
            queue: false,
        },
    )]);
    assert!(report.events.iter().any(|e| matches!(
        e,
        Event::CommandRejected {
            reason: RejectReason::Eliminated,
            ..
        }
    )));
    // Its remnants keep their brains: the harvester still exists.
    assert!(state.unit(survivor).is_some());

    // A scout walks south to put the second foundry in team sight (the
    // guns outrange their own eyes), then the guns finish the match.
    let east_foundry_2 = state
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(3))
        .unwrap()
        .id;
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: TilePos::new(18, 9),
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |s, _| {
        s.can_see(PlayerId(0), TilePos::new(20, 11))
    });
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: guns,
            target: Target::Building(east_foundry_2),
            queue: false,
        },
    )]);
    run_until(&mut state, 3000, |s, _| s.result().is_some());
    assert_eq!(state.result(), Some(GameResult::Victory { team: 0 }));
    assert_eq!(state.winners(), vec![PlayerId(0), PlayerId(1)]);
}

#[test]
fn a_2v2_scenario_reproduces_bit_identically() {
    let scenario = Scenario::load("../scenarios/twin-forges.json").unwrap();
    let run = || {
        let mut state = scenario.build().unwrap();
        let mut bots = oxide_sim::bot::seat_bots(&scenario);
        for _ in 0..600 {
            let mut commands = Vec::new();
            for bot in bots.iter_mut() {
                commands.extend(bot.act(&state));
            }
            state.tick(&commands);
        }
        state.hash()
    };
    assert_eq!(run(), run(), "same seed, same commands, same world");
}

#[test]
fn an_omitted_team_can_never_alias_an_explicit_one() {
    // Seat 0 authors team 1 while seat 1 omits its team entirely. Raw
    // values would alias them onto one side (and reject the map as one
    // team); normalization keeps the omitted seat a genuine team of one.
    let mut scenario = arena4(vec![
        unit(0, UnitKind::Sentinel, 5, 5),
        unit(1, UnitKind::Sentinel, 18, 5),
    ]);
    scenario.players.truncate(2);
    scenario.map = vec![
        "########################".into(),
        "#1..................2..#".into(),
        "#......................#".into(),
        "#......................#".into(),
        "#......................#".into(),
        "#......................#".into(),
        "#......................#".into(),
        "########################".into(),
    ];
    scenario.players[0].team = Some(1);
    scenario.players[1].team = None;
    let state = scenario.build().expect("two distinct teams, not OneTeam");
    let (a, b) = (state.players()[0].team, state.players()[1].team);
    assert_ne!(a, b, "the omitted seat fights alone");
}

#[test]
fn explicit_team_ids_group_by_value_whatever_numbers_authors_pick() {
    // Sparse, out-of-order ids (7 and 3) group exactly like dense ones:
    // the numbers are opaque labels, only the grouping is real.
    let mut scenario = arena4(vec![
        unit(0, UnitKind::Sentinel, 5, 5),
        unit(1, UnitKind::Sentinel, 18, 5),
        unit(2, UnitKind::Sentinel, 5, 8),
        unit(3, UnitKind::Sentinel, 18, 8),
    ]);
    scenario.players[0].team = Some(7);
    scenario.players[1].team = Some(3);
    scenario.players[2].team = Some(7);
    scenario.players[3].team = Some(3);
    let state = scenario.build().unwrap();
    let teams: Vec<u8> = state.players().iter().map(|p| p.team).collect();
    assert_eq!(teams[0], teams[2], "the sevens stand together");
    assert_eq!(teams[1], teams[3], "the threes stand together");
    assert_ne!(teams[0], teams[1], "and the two sides stay enemies");
}

#[test]
fn a_teamed_seat_cannot_field_the_classic_bot() {
    // The config-less fallback is the frozen 0.6 bot, which is team-blind
    // and would spend the match targeting its own allies. A teamed bot
    // seat without a bot_config is a build error, not a crippled match.
    let mut scenario = arena4(vec![unit(0, UnitKind::Sentinel, 5, 5)]);
    scenario.players[1].bot = true;
    scenario.players[1].bot_config = None;
    let err = scenario.build().unwrap_err();
    assert!(
        err.to_string().contains("p1"),
        "the error names the offending seat: {err}"
    );

    // A team of one keeps the classic fallback: everyone really is its
    // enemy there, and pre-0.7 content depends on it.
    let mut duel = arena4(vec![unit(0, UnitKind::Sentinel, 5, 5)]);
    duel.players.truncate(2);
    duel.map = vec![
        "########################".into(),
        "#1..................2..#".into(),
        "#......................#".into(),
        "#......................#".into(),
        "#......................#".into(),
        "#......................#".into(),
        "#......................#".into(),
        "########################".into(),
    ];
    duel.players[0].team = None;
    duel.players[1].team = None;
    duel.players[1].bot = true;
    duel.players[1].bot_config = None;
    duel.build()
        .expect("a lone-team classic bot is still legal");
}
