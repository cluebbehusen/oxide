//! Surrender: concession is a first-class fact, seat-scoped like the
//! command gate and counted by the team-scoped victory check. Headless
//! scenarios through the public API only.

use crate::common;
use common::cmd;
use oxide_sim::scenario::ScenarioMode;

use chassis::replay::Replay;
use oxide_sim::command::RejectReason;
use oxide_sim::scenario::PlayerSpec;
use oxide_sim::{Command, Event, GameResult, PlayerCommand, PlayerId, SIM_VERSION, Scenario};

/// A 2v2 of bare Foundries: west team (seats 0, 1) against east
/// (seats 2, 3). No armies — concession is the only way anyone here
/// ever loses.
fn arena4() -> Scenario {
    let seat = |name: &str, team| PlayerSpec {
        name: name.into(),
        team: Some(team),
        scrap: 100,
        bot: false,
        bot_config: None,
    };
    Scenario {
        mode: ScenarioMode::Match,
        name: "surrender-arena".into(),
        map: vec![
            "####################".into(),
            "#1..............3..#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#2..............4..#".into(),
            "#..................#".into(),
            "####################".into(),
        ],
        players: vec![
            seat("North West", 0),
            seat("South West", 0),
            seat("South East", 1),
            seat("North East", 1),
        ],
        units: Vec::new(),
        buildings: Vec::new(),
        meta: None,
    }
}

#[test]
fn a_1v1_surrender_decides_the_match_on_its_own_tick() {
    // Commands apply before the tick's victory check, so the concession
    // and the result share a tick.
    let mut state = Scenario::skirmish().build().unwrap();
    let winner_team = state.player(PlayerId(0)).team;
    let report = state.tick(&[cmd(1, Command::Surrender)]);
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e, Event::PlayerResigned { player } if *player == PlayerId(1))),
        "the concession is reported"
    );
    assert!(state.player(PlayerId(1)).resigned);
    assert_eq!(
        state.result(),
        Some(GameResult::Victory { team: winner_team }),
        "a 1v1 surrender ends the match on the spot"
    );
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e, Event::GameOver { .. })),
        "the same tick reports the end"
    );
}

#[test]
fn surrender_inside_a_decided_match_is_a_frozen_no_op() {
    let mut state = Scenario::skirmish().build().unwrap();
    state.tick(&[cmd(1, Command::Surrender)]);
    assert!(state.result().is_some());
    let report = state.tick(&[cmd(0, Command::Surrender)]);
    assert!(report.events.is_empty(), "a frozen world reports nothing");
    assert!(
        !state.player(PlayerId(0)).resigned,
        "the winner never conceded — the frozen tick ignored the command"
    );
}

#[test]
fn a_team_concession_is_seat_scoped_until_the_whole_team_resigns() {
    let mut state = arena4().build().unwrap();
    let east_team = state.player(PlayerId(2)).team;

    // One seat down: its team still holds Foundries, so the match runs.
    let report = state.tick(&[cmd(1, Command::Surrender)]);
    assert!(state.player(PlayerId(1)).resigned);
    assert!(
        state.result().is_none(),
        "a teammate's Foundry keeps the team in the match"
    );
    assert!(
        !report
            .events
            .iter()
            .any(|e| matches!(e, Event::GameOver { .. }))
    );

    // The conceded seat has no voice left: any later command — a second
    // Surrender included — rejects at the gate.
    for command in [Command::Stop { units: Vec::new() }, Command::Surrender] {
        let report = state.tick(&[cmd(1, command)]);
        assert!(
            report.events.iter().any(|e| matches!(
                e,
                Event::CommandRejected {
                    player: PlayerId(1),
                    reason: RejectReason::Eliminated,
                }
            )),
            "a resigned seat's commands reject as Eliminated"
        );
        assert!(
            !report
                .events
                .iter()
                .any(|e| matches!(e, Event::PlayerResigned { .. })),
            "conceding twice states nothing new"
        );
    }

    // The teammate concedes too: the west team is fully resigned, and
    // its standing Foundries stop counting — east wins on that tick.
    state.tick(&[cmd(0, Command::Surrender)]);
    assert_eq!(
        state.result(),
        Some(GameResult::Victory { team: east_team }),
        "a fully-resigned team is eliminated on the spot"
    );
}

#[test]
fn a_record_with_a_surrender_reproduces_headlessly() {
    let mut replay: Replay<Scenario, PlayerCommand> =
        Replay::new(SIM_VERSION, "test", Scenario::skirmish());
    replay.record(4, cmd(1, Command::Surrender));
    replay.meta.ticks = Some(10);
    let json = serde_json::to_string(&replay).unwrap();
    let loaded: Replay<Scenario, PlayerCommand> = serde_json::from_str(&json).unwrap();

    let run = |replay: &Replay<Scenario, PlayerCommand>| {
        let mut state = replay.setup.clone().build().unwrap();
        for tick in 0..replay.meta.ticks.unwrap() {
            let commands: Vec<PlayerCommand> = replay
                .commands
                .iter()
                .filter(|c| c.tick == tick)
                .map(|c| c.command.clone())
                .collect();
            state.tick(&commands);
        }
        (state.hash(), state.result())
    };
    let (hash, result) = run(&replay);
    assert_eq!(run(&loaded), (hash, result), "the wire changed the run");
    assert!(
        matches!(result, Some(GameResult::Victory { .. })),
        "the recorded concession decided the re-run too"
    );
}
