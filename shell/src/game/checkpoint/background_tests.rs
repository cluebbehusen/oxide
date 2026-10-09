use super::*;

#[test]
fn pause_save_bulk_advance_and_replacement_keep_the_settled_boundary() {
    if std::thread::available_parallelism().map_or(1, std::num::NonZero::get) < 2 {
        return; // A single-core host intentionally has no background executor.
    }
    let mut game = Game::with_viewport(Scenario::skirmish(), vec2(1280.0, 800.0)).unwrap();
    game.advance_ticks(120);
    game.issue(Command::Train {
        building: BuildingId(0),
        kind: UnitKind::Harvester,
    });
    let settled = serde_json::to_vec(&game).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while game.bot_decision.is_none() {
        game.prepare_bot_decision();
        assert!(
            std::time::Instant::now() < deadline,
            "executor admission never became available"
        );
        std::thread::yield_now();
    }
    game.prepare_bot_decision();
    game.clock.paused = true;
    assert!(!game.advance_wall_clock(1.0, None));
    assert!(game.bot_decision.is_some());
    assert_eq!(serde_json::to_vec(&game).unwrap(), settled);
    let mut loaded: Game = serde_json::from_slice(&settled).unwrap();
    assert!(loaded.bot_decision.is_none());
    game.advance_ticks(120);
    loaded.advance_ticks(120);
    assert!(game.bot_decision.is_none());
    assert_eq!(game.hash_hex(), loaded.hash_hex());
    assert_eq!(
        serde_json::to_vec(&game).unwrap(),
        serde_json::to_vec(&loaded).unwrap()
    );
    game.prepare_bot_decision();
    let replacement = (*loaded.state).clone();
    game.replace_state_after_jump(&replacement);
    assert!(game.bot_decision.is_none());
    game.advance_ticks(120);
    loaded.advance_ticks(120);
    assert_eq!(game.hash_hex(), loaded.hash_hex());
}
