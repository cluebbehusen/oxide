//! Fixed-facing building contact preserves deterministic bot continuation.

use oxide_bot::seat_bots;
use oxide_sim::scenario::BotConfig;
use oxide_sim::{Event, Scenario, State};

#[test]
fn fixed_facing_economies_and_attack_followups_replay_identically() {
    let mut scenario = Scenario::skirmish();
    let mut rows: Vec<Vec<char>> = scenario
        .map
        .iter()
        .map(|row| row.chars().collect())
        .collect();
    for row in &mut rows {
        for tile in row {
            if *tile == 'S' {
                *tile = 's';
            }
        }
    }
    for (x, y, tile) in [
        (5, 14, 'E'),
        (33, 8, 'E'),
        (5, 12, 's'),
        (6, 12, 's'),
        (34, 11, 's'),
        (33, 11, 's'),
    ] {
        rows[y][x] = tile;
    }
    scenario.map = rows
        .into_iter()
        .map(|row| row.into_iter().collect())
        .collect();
    for player in &mut scenario.players {
        player.bot = true;
        player.bot_config = Some(BotConfig::scripted(
            Default::default(),
            Default::default(),
            0,
        ));
    }
    // Rosters differ by faction in price and kind, so only same-faction
    // seats can keep mirrored banks and rosters.
    let faction = scenario.players[0].faction;
    scenario.retint_seat(1, faction);
    let mut state = scenario.build().unwrap();
    let mut bots = seat_bots(&scenario).unwrap();
    let mut replay = state.clone();
    let mut replay_bots = seat_bots(&scenario).unwrap();
    for tick in 0..12_300 {
        let commands: Vec<_> = bots.iter_mut().flat_map(|bot| bot.act(&state)).collect();
        let report = state.tick(&commands);
        assert!(
            !report
                .events
                .iter()
                .any(|event| matches!(event, Event::CommandRejected { .. }))
        );
        let replay_commands: Vec<_> = replay_bots
            .iter_mut()
            .flat_map(|bot| bot.act(&replay))
            .collect();
        assert_eq!(commands, replay_commands, "commands after tick {tick}");
        assert_eq!(
            report,
            replay.tick(&replay_commands),
            "events after tick {tick}"
        );
        assert_eq!(state.hash(), replay.hash(), "world after tick {tick}");
        if tick % 500 == 0 {
            replay =
                serde_json::from_slice::<State>(&serde_json::to_vec(&replay).unwrap()).unwrap();
            replay.validate_invariants().unwrap();
        }
    }
}
