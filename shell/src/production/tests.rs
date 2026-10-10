use super::*;
use macroquad::prelude::vec2;
use oxide_sim::{BuildingKind, Faction, PlayerCommand, Scenario};

fn factories(scrap: u32) -> Game {
    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = scrap;
    scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
        player: 0,
        kind: BuildingKind::Foundry,
        x: 9,
        y: 3,
    });
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
    game.presentation.selection.buildings = game
        .state
        .buildings()
        .iter()
        .filter(|b| b.player == game.presentation.human)
        .map(|b| b.id)
        .collect();
    game
}

#[test]
fn provisional_foundry_does_not_keep_production_or_the_home_target_alive() {
    let mut game = factories(500);
    let home = game.home_foundry().unwrap().clone();
    let worker = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind.stats().harvest.is_some())
        .unwrap()
        .id;
    let mut snapshot = serde_json::to_value(&*game.state).unwrap();
    snapshot["buildings"].as_array_mut().unwrap().retain(|b| {
        b["player"] != serde_json::json!(game.presentation.human)
            || b["id"] == serde_json::json!(home.id)
    });
    let site = snapshot["buildings"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|b| b["id"] == serde_json::json!(home.id))
        .unwrap();
    site["phase"] = serde_json::json!({"phase": "provisional"});
    site["hp"] = (home.stats().max_hp / 5).into();
    let unit = snapshot["units"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|u| u["id"] == serde_json::json!(worker))
        .unwrap();
    unit["order"] = serde_json::to_value(oxide_sim::Order::Found {
        kind: home.kind,
        anchor: home.anchor,
    })
    .unwrap();
    *game.state = serde_json::from_value(snapshot).unwrap();
    game.state.validate_invariants().unwrap();
    assert!(game.state.result().is_none());
    assert!(!game.state.player(game.presentation.human).resigned);
    assert!(game.home_foundry().is_none());
    assert!(!Production::inspect(&game.view()).selected.accepts);
}

#[test]
fn grouped_production_spends_once_per_factory_in_id_order_and_projects_pending_commands() {
    let mut game = factories(150);
    let ids = game.presentation.selection.buildings.clone();
    game.presentation.selection.buildings = vec![ids[1], ids[0], ids[1]];
    let before = game.state.hash();
    train(&mut game, 0);
    train(&mut game, 0);
    train(&mut game, 0);
    assert_eq!(
        game.state.hash(),
        before,
        "preview never advances the match"
    );
    let targets: Vec<_> = game
        .pending
        .iter()
        .map(|pc| match pc.command {
            Command::Train { building, .. } => building,
            _ => panic!("ordinary train only"),
        })
        .collect();
    assert_eq!(targets, vec![ids[0], ids[1], ids[0]]);
    assert!(
        game.presentation
            .toasts
            .iter()
            .any(|t| t.text == "Queued 1 of 2: insufficient scrap")
    );
    let (cards, counts) = Production::inspect(&game.view()).collective_queue();
    assert_eq!(cards[0].title, "Harvester x 3");
    assert_eq!((counts[0].count, counts[0].active), (3, 2));
    let events = game.do_tick().events;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, oxide_sim::Event::CommandRejected { .. }))
    );
    assert_eq!(game.state.building(ids[0]).unwrap().queue.len(), 2);
    assert_eq!(game.state.building(ids[1]).unwrap().queue.len(), 1);
}

#[test]
fn grouped_production_skips_full_queues_including_staged_purchases() {
    let mut game = factories(5000);
    let ids = game.presentation.selection.buildings.clone();
    for _ in 0..oxide_sim::stats::QUEUE_CAP {
        game.issue(Command::Train {
            building: ids[0],
            kind: UnitKind::Harvester,
        });
    }
    let batch = Production::inspect(&game.view()).batch(0).unwrap();
    assert_eq!(batch.recipients, vec![ids[1]]);
    assert_eq!(batch.reason.as_deref(), Some("1 queue full"));
    for _ in 0..oxide_sim::stats::QUEUE_CAP + 2 {
        train(&mut game, 0);
    }
    assert_eq!(game.pending.len(), oxide_sim::stats::QUEUE_CAP * 2);
    let panel =
        crate::panel::build_for_palette(&game.view(), &BindingMap::classic(), false).unwrap();
    assert_eq!(panel.queue.len(), 1, "sixteen paid units occupy one tile");
    assert_eq!(panel.queue_groups[0].count, 16);
    assert!(
        panel
            .cards
            .iter()
            .filter(|c| c.cost.is_some())
            .all(|c| !c.enabled)
    );
    assert!(
        !game
            .do_tick()
            .events
            .iter()
            .any(|e| matches!(e, oxide_sim::Event::CommandRejected { .. }))
    );
}

#[test]
fn collective_cancellation_preserves_heads_and_repeated_clicks_use_updated_slots() {
    let mut game = factories(150);
    let ids = game.presentation.selection.buildings.clone();
    train(&mut game, 0);
    train(&mut game, 0);
    cancel_one(&mut game, UnitKind::Harvester);
    assert!(
        matches!(game.pending.last().unwrap().command, Command::CancelTrain { building, index: 1 } if building == ids[0])
    );
    assert_eq!(Production::inspect(&game.view()).selected.scrap, 50);
    train(&mut game, 1); // 75 scrap must still be refused.
    assert!(matches!(
        game.pending.last().unwrap().command,
        Command::CancelTrain { .. }
    ));
    cancel_one(&mut game, UnitKind::Harvester);
    train(&mut game, 1); // The second refund can now buy one Sentinel.
    assert!(matches!(
        game.pending.last().unwrap().command,
        Command::Train {
            kind: UnitKind::Sentinel,
            ..
        }
    ));
    cancel_one(&mut game, UnitKind::Harvester);
    let len = game.pending.len();
    cancel_one(&mut game, UnitKind::Harvester);
    assert_eq!(
        game.pending.len(),
        len,
        "an exhausted stale tile cancels nothing else"
    );
    assert!(
        !game
            .do_tick()
            .events
            .iter()
            .any(|e| matches!(e, oxide_sim::Event::CommandRejected { .. }))
    );
    assert_eq!(
        game.state.building(ids[0]).unwrap().queue.front(),
        Some(&UnitKind::Sentinel)
    );
    assert!(game.state.building(ids[1]).unwrap().queue.is_empty());
}

#[test]
fn the_stop_square_empties_every_selected_queue_with_full_refunds() {
    let mut game = factories(5000);
    let ids = game.presentation.selection.buildings.clone();
    let stop = |game: &Game| {
        crate::panel::build_for_palette(&game.view(), &BindingMap::classic(), false)
            .and_then(|panel| panel.stop)
            .map(|card| card.action)
    };
    assert_eq!(stop(&game), None, "idle factories have nothing to stop");
    for slot in [0, 0, 1] {
        train(&mut game, slot);
    }
    game.do_tick();
    game.do_tick();
    assert!(game.state.player(game.presentation.human).scrap < 5000);
    assert!(
        ids.iter()
            .all(|id| game.state.building(*id).unwrap().queue.len() == 3)
    );
    assert!(
        game.state.building(ids[0]).unwrap().training_progress() > 0,
        "the head has started"
    );
    assert_eq!(stop(&game), Some(CardAction::ClearQueues));

    cancel_all(&mut game);
    assert!(
        !game
            .do_tick()
            .events
            .iter()
            .any(|e| matches!(e, oxide_sim::Event::CommandRejected { .. }))
    );
    assert!(
        ids.iter()
            .all(|id| game.state.building(*id).unwrap().queue.is_empty())
    );
    assert_eq!(
        game.state.player(game.presentation.human).scrap,
        5000,
        "every job, the started heads included, is refunded in full"
    );
    assert_eq!(stop(&game), None);
}

#[test]
fn grouped_production_honors_foreign_ownership_tech_and_pending_elimination() {
    let mut game = factories(5000);
    let ids = game.presentation.selection.buildings.clone();
    let foreign = game
        .state
        .buildings()
        .iter()
        .find(|b| b.player != game.presentation.human)
        .unwrap()
        .id;
    game.presentation.selection.buildings = vec![foreign];
    train(&mut game, 0);
    assert!(game.pending.is_empty());
    game.presentation.selection.buildings = ids;
    // Excavators require an Array.
    let slot = BuildingKind::Foundry
        .base_stats()
        .produces
        .iter()
        .position(|k| *k == UnitKind::Excavator)
        .unwrap();
    let batch = Production::inspect(&game.view()).batch(slot).unwrap();
    assert!(batch.recipients.is_empty());
    assert!(batch.reason.unwrap().contains("standing"));
    game.issue(Command::Surrender);
    train(&mut game, 0);
    assert_eq!(game.pending.len(), 1);
}

#[test]
fn grouped_production_observes_pending_invalid_purchases_and_refunds_without_charging_twice() {
    let mut game = factories(50);
    let id = game.presentation.selection.buildings[0];
    game.stage(PlayerCommand {
        player: game.presentation.human,
        command: Command::Train {
            building: id,
            kind: UnitKind::Condor,
        },
    });
    train(&mut game, 0);
    assert_eq!(Production::inspect(&game.view()).selected.scrap, 0);
    assert_eq!(
        Production::inspect(&game.view()).selected.buildings[0]
            .queue
            .len(),
        1
    );
    cancel_one(&mut game, UnitKind::Harvester);
    assert_eq!(Production::inspect(&game.view()).selected.scrap, 50);
}

#[test]
fn collective_rosters_fit_the_dock_for_both_factions() {
    for faction in [Faction::Ferrous, Faction::Cupric] {
        for kind in [
            BuildingKind::Foundry,
            BuildingKind::Fabricator,
            BuildingKind::Airworks,
            BuildingKind::Crucible,
        ] {
            let roster: Vec<_> = kind
                .base_stats()
                .produces
                .iter()
                .filter(|k| k.faction().is_none_or(|f| f == faction))
                .collect();
            assert!(
                roster.len() <= 8,
                "{kind:?} must retain access to every aggregate"
            );
        }
    }
}
