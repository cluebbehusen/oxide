//! Mirrored economic work must not seed a physical advantage before battle.

use chassis::fx::{Fx, Vec2Fx};
use oxide_sim::bot::seat_bots;
use oxide_sim::scenario::BotConfig;
use oxide_sim::{Event, PlayerId, Scenario};

#[test]
fn mirrored_workers_and_mustering_armies_preserve_positions_and_income() {
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
        player.bot_config = Some(BotConfig::default());
    }
    let mut state = scenario.build().unwrap();
    let mut bots = seat_bots(&scenario).unwrap();
    let extent = Vec2Fx::new(
        Fx::from_num(state.map().width()),
        Fx::from_num(state.map().height()),
    );
    for tick in 0..5000 {
        let commands: Vec<_> = bots.iter_mut().flat_map(|bot| bot.act(&state)).collect();
        let report = state.tick(&commands);
        assert!(
            !report
                .events
                .iter()
                .any(|event| matches!(event, Event::CommandRejected { .. }))
        );
        assert_eq!(
            state.players()[0].scrap,
            state.players()[1].scrap,
            "bank after tick {tick}"
        );
        let left: Vec<_> = state
            .units()
            .iter()
            .filter(|unit| unit.player == PlayerId(0))
            .collect();
        let right: Vec<_> = state
            .units()
            .iter()
            .filter(|unit| unit.player == PlayerId(1))
            .collect();
        assert_eq!(left.len(), right.len(), "roster after tick {tick}");
        for (left, right) in left.into_iter().zip(right) {
            assert_eq!(left.kind, right.kind);
            assert_eq!(
                extent - left.pos,
                right.pos,
                "units {}/{}, after tick {tick}",
                left.id,
                right.id
            );
            assert_eq!(left.hp, right.hp, "health after tick {tick}");
            assert_eq!(left.carrying, right.carrying, "cargo after tick {tick}");
        }
    }
}
