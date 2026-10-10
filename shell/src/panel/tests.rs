use super::*;
use crate::game::Game;

#[test]
fn every_dock_chip_that_cancels_work_discards_it_and_nothing_else_does() {
    let site = oxide_sim::BuildingId(3);
    let tile = chassis::grid::TilePos::new(4, 5);
    for action in [
        CardAction::CancelOrder {
            unit: oxide_sim::UnitId(1),
            key: oxide_sim::OrderKey::Walk { tile },
            from_end: 0,
        },
        CardAction::CancelSite(site),
        CardAction::CancelFound(BuildingKind::Turret, tile),
        CardAction::CancelQueue(site, 0),
        CardAction::CancelProduction(UnitKind::Harvester),
    ] {
        assert!(action.discards_work(), "{action:?}");
    }
    for action in [
        CardAction::Dispatch(Action::Hunt),
        CardAction::ArmRally,
        CardAction::UnloadHere(oxide_sim::UnitId(1)),
        CardAction::FilterKind(UnitKind::Harvester),
        CardAction::None,
        CardAction::Refused,
    ] {
        assert!(!action.discards_work(), "{action:?}");
    }
}

#[test]
fn card_copy_names_only_what_the_hands_in_use_have() {
    use crate::platform::{ALL_HANDS, assert_copy_fits};
    for hands in ALL_HANDS {
        let roster = roster_filter_desc(hands);
        assert_eq!(roster.len(), 2);
        for line in &roster {
            assert_copy_fits(hands, line);
        }
        assert_copy_fits(hands, patrol_desc(hands));
        assert_copy_fits(hands, transport_load_desc(hands.touch()));
    }
    let patrols: std::collections::BTreeSet<_> = ALL_HANDS.map(patrol_desc).into();
    assert_eq!(
        patrols.len(),
        ALL_HANDS.len(),
        "each pairing has its own patrol copy"
    );
}

#[test]
fn stripping_hotkeys_clears_every_card_row() {
    let mut game = game();
    game.presentation.selection.buildings = vec![human_foundry(&game)];
    let mut panel =
        build_for_palette(&game.view(), &BindingMap::classic(), false).expect("hq panel");
    assert!(panel.cards.iter().any(|card| !card.hotkey.is_empty()));
    strip_hotkeys(&mut panel);
    for card in panel.roster.iter().chain(&panel.cards).chain(&panel.queue) {
        assert!(
            card.hotkey.is_empty(),
            "{} keeps {}",
            card.title,
            card.hotkey
        );
    }
}

fn stat<'a>(panel: &'a Panel, label: &str) -> &'a info::StatRow {
    panel
        .info
        .rows
        .iter()
        .find(|row| row.label == label)
        .unwrap()
}
use macroquad::prelude::vec2;
use oxide_sim::{Command, PlayerCommand, Scenario};

fn game() -> Game {
    Game::with_viewport(Scenario::skirmish(), vec2(1280.0, 800.0)).expect("skirmish builds")
}

fn human_foundry(game: &Game) -> oxide_sim::BuildingId {
    game.state
        .buildings()
        .iter()
        .find(|b| b.player == game.presentation.human)
        .expect("human foundry")
        .id
}

fn extractor_panel_game() -> Game {
    let mut scenario = Scenario::skirmish();
    let frames: Vec<_> = scenario
        .map
        .iter()
        .enumerate()
        .flat_map(|(y, row)| {
            row.char_indices()
                .filter(|(_, tile)| *tile == 'E')
                .map(move |(x, _)| (x.fit::<i32>(), y.fit::<i32>()))
        })
        .collect();
    assert!(frames.len() >= 3, "fixture needs home and remote frames");
    let last = frames.len() - 1;
    scenario
        .buildings
        .extend(frames.into_iter().enumerate().map(|(index, (x, y))| {
            oxide_sim::scenario::BuildingSpec {
                player: u8::from(index == last),
                kind: BuildingKind::Extractor,
                x,
                y,
            }
        }));
    Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("Extractor fixture builds")
}

#[test]
fn nothing_selected_builds_no_panel() {
    let game = game();
    assert!(build_for_palette(&game.view(), &BindingMap::classic(), false).is_none());
}

#[test]
fn own_extractors_name_current_income_without_exposing_foreign_support() {
    let mut game = extractor_panel_game();
    let own_extractors: Vec<_> = game
        .state
        .buildings()
        .iter()
        .filter(|building| {
            building.player == game.presentation.human && building.kind == BuildingKind::Extractor
        })
        .map(|building| building.id)
        .collect();
    let supported = own_extractors
        .iter()
        .copied()
        .find(|id| game.state.extractor_income(*id) == Some(oxide_sim::ExtractorIncome::Supported))
        .expect("a home Extractor is supported");
    let remote = own_extractors
        .iter()
        .copied()
        .find(|id| game.state.extractor_income(*id) == Some(oxide_sim::ExtractorIncome::Remote))
        .expect("a distant Extractor is remote");

    game.presentation.selection.buildings = vec![supported];
    let panel =
        build_for_palette(&game.view(), &BindingMap::classic(), false).expect("supported panel");
    assert_eq!(stat(&panel, "Income").value, "180 scrap/min");
    assert_eq!(stat(&panel, "Support").value, "Foundry");

    game.presentation.selection.buildings = vec![remote];
    let panel =
        build_for_palette(&game.view(), &BindingMap::classic(), false).expect("remote panel");
    assert_eq!(stat(&panel, "Income").value, "120 scrap/min");
    assert_eq!(stat(&panel, "Support").value, "Remote");

    let foreign = game
        .state
        .buildings()
        .iter()
        .find(|building| {
            building.player != game.presentation.human && building.kind == BuildingKind::Extractor
        })
        .expect("foreign Extractor")
        .id;
    assert_eq!(
        game.state.extractor_income(foreign),
        Some(oxide_sim::ExtractorIncome::Supported),
        "the fixture needs private dynamic support to hide"
    );
    game.presentation.selection.buildings = vec![foreign];
    let panel =
        build_for_palette(&game.view(), &BindingMap::classic(), false).expect("foreign panel");
    assert!(
        panel
            .info
            .rows
            .iter()
            .all(|row| !matches!(row.label.as_str(), "Income" | "Support"))
    );
    assert_eq!(
        building_income(&game.view(), game.state.building(foreign).unwrap()),
        0
    );
    assert_eq!(stat(&panel, "Sight").value, "4 tiles");
}

#[test]
fn recurring_income_tracks_real_output_upgrade_downtime_and_foundry_warmup() {
    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = 10_000;
    scenario.buildings.extend([
        oxide_sim::scenario::BuildingSpec {
            player: 0,
            kind: BuildingKind::Reclaimer,
            x: 9,
            y: 3,
        },
        oxide_sim::scenario::BuildingSpec {
            player: 0,
            kind: BuildingKind::Fabricator,
            x: 12,
            y: 3,
        },
    ]);
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
    let reclaimer = game
        .state
        .buildings()
        .iter()
        .find(|building| building.kind == BuildingKind::Reclaimer)
        .unwrap()
        .id;
    let foundry = human_foundry(&game);
    let rate = |game: &Game, id| building_income(&game.view(), game.state.building(id).unwrap());
    assert_eq!(rate(&game, reclaimer), 50);
    assert_eq!(rate(&game, foundry), 0);
    let initial = game.state.player(game.presentation.human).scrap;
    for _ in 0..1_200 {
        game.state.tick(&[]);
    }
    assert_eq!(
        game.state.player(game.presentation.human).scrap - initial,
        rate(&game, reclaimer)
    );
    game.state.tick(&[PlayerCommand {
        player: game.presentation.human,
        command: Command::UpgradeBuilding {
            building: reclaimer,
        },
    }]);
    assert!(!game.state.building(reclaimer).unwrap().built());
    assert_eq!(rate(&game, reclaimer), 0);
    for _ in 0..300 {
        game.state.tick(&[]);
    }
    assert_eq!(game.state.building(reclaimer).unwrap().tier, 1);
    assert_eq!(rate(&game, reclaimer), 120);
    game.presentation.selection.buildings = vec![reclaimer];
    let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).unwrap();
    assert_eq!(panel.title, "Refinery");
    assert_eq!(
        panel.portrait,
        CardIcon::Building(BuildingKind::Reclaimer, 1)
    );
    while game.state.current_tick() < oxide_sim::stats::FOUNDRY_DRIP_START_TICK - 1 {
        game.state.tick(&[]);
    }
    assert_eq!(rate(&game, foundry), 0);
    game.state.tick(&[]);
    assert_eq!(rate(&game, foundry), 20);
    let initial = game.state.player(game.presentation.human).scrap;
    for _ in 0..1_200 {
        game.state.tick(&[]);
    }
    assert_eq!(
        game.state.player(game.presentation.human).scrap - initial,
        rate(&game, foundry) + rate(&game, reclaimer)
    );
}

#[test]
fn extractor_build_copy_names_both_rates_and_the_foundry_rule() {
    let extractor = building_economy_lines(BuildingKind::Extractor);
    assert!(
        extractor
            .iter()
            .any(|line| line.contains("120 scrap/min remote"))
    );
    assert!(
        extractor
            .iter()
            .any(|line| line.contains("180 scrap/min with non-stacking support"))
    );
    assert!(extractor.iter().any(|line| {
        line.contains("own completed Foundries") && line.contains("8 footprint tiles")
    }));

    let foundry = building_economy_lines(BuildingKind::Foundry);
    assert!(foundry.iter().any(|line| {
        line.contains("own completed Extractors")
            && line.contains("120 to 180 scrap/min")
            && line.contains("8 footprint tiles")
            && line.contains("do not stack")
    }));

    let mut game = game();
    let harvester = game
        .state
        .units()
        .iter()
        .find(|unit| unit.player == game.presentation.human && unit.kind == UnitKind::Harvester)
        .expect("starting Harvester")
        .id;
    game.presentation.selection.units = vec![harvester];
    let panel =
        build_for_palette(&game.view(), &BindingMap::classic(), true).expect("advanced builds");
    let extractor_card = panel
        .cards
        .iter()
        .find(|card| card.title == "Extractor")
        .expect("advanced palette contains Extractor");
    assert!(
        extractor_card
            .desc
            .iter()
            .any(|line| line.contains("120 scrap/min"))
    );
    assert!(
        extractor_card
            .desc
            .iter()
            .any(|line| line.contains("180 scrap/min"))
    );
}

#[test]
fn scripted_opponents_show_their_difficulty_and_stance() {
    let mut game = game();
    game.scenario.players[1].bot_config = Some(oxide_sim::scenario::BotConfig::new(
        oxide_sim::scenario::BotDifficulty::Prime,
        oxide_sim::scenario::BotStance::Aggressive,
        91,
    ));
    assert_eq!(
        bot_controller_label(&game.view(), oxide_sim::PlayerId(1)),
        Some("Prime / Aggressive AI".to_string())
    );
}

#[test]
fn verb_cards_wear_their_atlas_icons() {
    use chassis::grid::TilePos;
    use oxide_sim::UnitKind;
    let mut game = game();
    let sentinel = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Sentinel)
        .expect("skirmish authors a sentinel")
        .id;
    game.state.tick(&[PlayerCommand {
        player: game.presentation.human,
        command: Command::Hunt {
            units: vec![sentinel],
            goal: TilePos::new(20, 12),
            queue: false,
        },
    }]);
    game.presentation.selection.units = vec![sentinel];
    let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
    let patrol = panel
        .cards
        .iter()
        .find(|c| c.title == "Patrol")
        .expect("patrol card");
    assert_eq!(patrol.icon, CardIcon::Verb(VerbIcon::Patrol));
    assert_eq!(patrol.hotkey, "R", "the tooltip chord stays live");
    let hunt = panel
        .cards
        .iter()
        .find(|c| c.title == "Hunt")
        .expect("hunt card");
    assert_eq!(hunt.action, CardAction::Dispatch(Action::Hunt));
    assert_eq!(hunt.hotkey, "F");
    let chip = &panel.queue[0];
    assert!(chip.title.starts_with("Hunt"), "{}", chip.title);
    assert_eq!(
        chip.icon,
        CardIcon::Verb(VerbIcon::Hunt),
        "chips wear pictograms, not letters that shadow chords"
    );
}

#[test]
fn the_foundry_panel_speaks_its_roster() {
    let mut game = game();
    let foundry = human_foundry(&game);
    game.presentation.selection.buildings = vec![foundry];
    let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
    assert_eq!(panel.title, "Foundry");
    assert_eq!(panel.cards.len(), 6, "four units plus two rally controls");
    assert_eq!(panel.cards[0].title, "Set rally");
    assert_eq!(panel.cards[0].action, CardAction::ArmRally);
    assert_eq!(panel.cards[2].hotkey, "Q");
    assert_eq!(panel.cards[2].cost, Some(50));
    assert_eq!(unit_train_time_label(UnitKind::Harvester), "5s");
    assert_eq!(unit_train_time_label(UnitKind::Sentinel), "7.5s");
    assert!(panel.cards[2].enabled, "150 scrap affords a harvester");
    assert_eq!(
        panel.cards[2].action,
        CardAction::Dispatch(Action::TrainSlot(0)),
        "the card IS its hotkey"
    );
    assert!(panel.queue.is_empty(), "nothing queued yet");
    // The harvester's card carries no weapon line; the sentinel's
    // carries both of its guns.
    assert!(!panel.cards[2].desc.iter().any(|l| l.contains("dmg")));
    assert!(panel.cards[3].desc.iter().any(|l| l.contains("dmg")));

    game.state.tick(&[PlayerCommand {
        player: game.presentation.human,
        command: Command::Train {
            building: foundry,
            kind: UnitKind::Harvester,
        },
    }]);
    let panel =
        build_for_palette(&game.view(), &BindingMap::classic(), false).expect("queued panel");
    assert_eq!(panel.queue_label, "5s");
    game.state.tick(&[]);
    let panel =
        build_for_palette(&game.view(), &BindingMap::classic(), false).expect("progressing panel");
    assert_eq!(panel.queue_label, "4.9s");
}

#[test]
fn a_producer_always_exposes_set_reset_and_clear_rally_actions() {
    let mut game = game();
    let foundry = human_foundry(&game);
    game.presentation.selection.buildings = vec![foundry];
    let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
    assert!(
        panel
            .cards
            .iter()
            .any(|card| { card.title == "Set rally" && card.action == CardAction::ArmRally })
    );
    assert!(
        panel
            .cards
            .iter()
            .any(|card| card.title == "Clear rally" && !card.enabled)
    );

    game.state.tick(&[PlayerCommand {
        player: game.presentation.human,
        command: Command::SetRally {
            building: foundry,
            rally: Some(chassis::grid::TilePos::new(12, 8)),
        },
    }]);
    let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
    assert!(
        panel
            .cards
            .iter()
            .any(|card| { card.title == "Reset rally" && card.action == CardAction::ArmRally })
    );
    assert!(panel.cards.iter().any(|card| {
        card.title == "Clear rally" && card.action == CardAction::ClearRally && card.enabled
    }));
}

#[test]
fn a_multi_producer_panel_puts_shared_rally_before_production() {
    let mut scenario = Scenario::skirmish();
    scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
        player: 0,
        kind: BuildingKind::Fabricator,
        x: 9,
        y: 3,
    });
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("fixture builds");
    game.presentation.selection.buildings = game
        .state
        .buildings()
        .iter()
        .filter(|building| building.player == game.presentation.human)
        .map(|building| building.id)
        .collect();

    let panel = build_for_palette(&game.view(), &BindingMap::classic(), false)
        .expect("multi-building panel");
    assert_eq!(panel.title, "2 BUILDINGS");
    assert_eq!(panel.cards.len(), 2);
    assert_eq!(panel.cards[0].title, "Set rallies");
    assert_eq!(panel.cards[0].action, CardAction::ArmRally);
}

#[test]
fn a_non_producer_never_offers_a_rally_action() {
    let mut scenario = Scenario::skirmish();
    scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
        player: 0,
        kind: BuildingKind::Turret,
        x: 9,
        y: 3,
    });
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("fixture builds");
    let turret = game
        .state
        .buildings()
        .iter()
        .find(|building| building.kind == BuildingKind::Turret)
        .unwrap()
        .id;
    game.presentation.selection.buildings = vec![turret];

    let panel =
        build_for_palette(&game.view(), &BindingMap::classic(), false).expect("Turret panel");
    assert_eq!(panel.cards.len(), 1, "the turret offers its tier upgrade");
    assert!(
        panel.stop.is_none(),
        "a turret without a target has nothing to stop"
    );
    assert!(
        matches!(panel.cards[0].action, CardAction::Upgrade),
        "the turret's card lifts its tier"
    );
    assert!(
        panel
            .cards
            .iter()
            .all(|card| !matches!(card.action, CardAction::ArmRally | CardAction::ClearRally)),
        "a defense cannot rally units it never produces"
    );
}

#[test]
fn an_upgrade_needs_no_harvester_and_explains_its_downtime() {
    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = 500;
    scenario
        .units
        .retain(|unit| unit.player != 0 || unit.kind != oxide_sim::UnitKind::Harvester);
    scenario.buildings.extend([
        oxide_sim::scenario::BuildingSpec {
            player: 0,
            kind: BuildingKind::Fabricator,
            x: 9,
            y: 3,
        },
        oxide_sim::scenario::BuildingSpec {
            player: 0,
            kind: BuildingKind::Turret,
            x: 12,
            y: 3,
        },
    ]);
    let mut game =
        Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("upgrade fixture builds");
    let turret = game
        .state
        .buildings()
        .iter()
        .find(|building| building.kind == BuildingKind::Turret)
        .expect("fixture has a turret")
        .id;
    game.presentation.selection.buildings = vec![turret];

    let panel =
        build_for_palette(&game.view(), &BindingMap::classic(), false).expect("turret panel");
    let upgrade = panel
        .cards
        .iter()
        .find(|card| matches!(card.action, CardAction::Upgrade))
        .expect("turret offers its upgrade");
    assert!(upgrade.enabled, "automatic upgrades need no crew");
    assert!(
        upgrade.desc.iter().any(|line| line.contains("Offline")),
        "the card explains the downtime: {:?}",
        upgrade.desc
    );

    game.state.tick(&[PlayerCommand {
        player: game.presentation.human,
        command: Command::UpgradeBuilding { building: turret },
    }]);
    let panel =
        build_for_palette(&game.view(), &BindingMap::classic(), false).expect("upgrade panel");
    assert_eq!(panel.cards[0].title, "Upgrading");
    assert!(panel.cards[0].progress.is_some());
    assert!(
        panel.cards[0]
            .desc
            .iter()
            .all(|line| !line.contains("crew"))
    );
}

#[test]
fn poverty_and_capacity_disable_cards_with_reasons() {
    let mut scenario = Scenario::skirmish();
    // The bank must outlast the queue cap or poverty masks it.
    scenario.players[0].scrap = 500;
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("skirmish builds");
    let foundry = human_foundry(&game);
    game.presentation.selection.buildings = vec![foundry];
    // Queue harvesters until 50 scrap remains: the sentinel card
    // (75) must dim with the price named.
    for _ in 0..3 {
        game.state.tick(&[PlayerCommand {
            player: game.presentation.human,
            command: Command::Train {
                building: foundry,
                kind: oxide_sim::UnitKind::Harvester,
            },
        }]);
    }
    // 500 - 3x50 = 350: still rich, cards enabled, ghosts armed.
    let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
    assert!(panel.cards[2].enabled);
    assert_eq!(panel.queue.len(), 3);
    assert_eq!(panel.queue[1].action, CardAction::CancelQueue(foundry, 1));
    // Fill to the sim's cap: every production card refuses.
    for _ in 0..oxide_sim::stats::QUEUE_CAP {
        game.state.tick(&[PlayerCommand {
            player: game.presentation.human,
            command: Command::Train {
                building: foundry,
                kind: oxide_sim::UnitKind::Harvester,
            },
        }]);
    }
    let queued = game.state.building(foundry).unwrap().queue.len();
    assert_eq!(queued, oxide_sim::stats::QUEUE_CAP, "the sim capped it");
    let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
    assert_eq!(
        panel.queue.len(),
        oxide_sim::stats::QUEUE_CAP,
        "every paid queue slot remains inspectable and cancelable"
    );
    assert!(
        panel
            .cards
            .iter()
            .filter(|card| card.cost.is_some())
            .all(|card| !card.enabled)
    );
    assert!(
        panel
            .cards
            .iter()
            .filter(|card| card.cost.is_some())
            .all(|card| card.why.as_deref() == Some("queue is full")),
        "the reason names the cap, not the bank"
    );
}

#[test]
fn the_harvester_panel_is_the_same_grammar() {
    let mut game = game();
    let harvester = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind == oxide_sim::UnitKind::Harvester)
        .expect("starting harvester")
        .id;
    game.presentation.selection.units = vec![harvester];
    let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
    assert_eq!(panel.title, "Harvester");
    // Unarmed, it has no Run or Hunt, and an empty hopper has
    // no cargo to return.
    let titles: Vec<&str> = panel.cards.iter().map(|card| card.title.as_str()).collect();
    assert_eq!(titles, ["Patrol", "Salvage", "Weld", "Build"]);
    assert!(
        !panel
            .info
            .rows
            .iter()
            .any(|row| matches!(row.label.as_str(), "Ground" | "Air"))
    );
    assert_eq!(panel.info.health, Some((60, 60)));
    assert_eq!(stat(&panel, "Speed").value, "2.5 tiles/s");
    assert!(panel.cards.iter().any(|card| card.title == "Build"));
    assert!(
        !panel
            .cards
            .iter()
            .any(|card| matches!(card.action, CardAction::ArmBuild(_)))
    );
    let construction = build_for_palette(&game.view(), &BindingMap::classic(), true).unwrap();
    assert_eq!(construction.cards.len(), 14);
    let (back, buildings) = construction.cards.split_last().expect("cards");
    assert!(
        buildings
            .iter()
            .all(|card| matches!(card.action, CardAction::ArmBuild(_)))
    );
    assert_eq!(back.action, CardAction::ClosePalette, "Back comes last");
    // An idle unit with nothing queued shows no order chips at all —
    // the dock only exists when there is a program to show.
    assert!(panel.queue.is_empty(), "idle shows no dock");
    assert!(panel.stop.is_none(), "an idle unit has nothing to stop");
    // Give it a program: the strip appears, and its chip removes the
    // order it shows.
    let goal = chassis::grid::TilePos::new(8, 8);
    game.state.tick(&[oxide_sim::PlayerCommand {
        player: game.presentation.human,
        command: oxide_sim::Command::Hunt {
            units: vec![harvester],
            goal,
            queue: false,
        },
    }]);
    let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
    assert_eq!(panel.queue.len(), 1);
    assert_eq!(
        panel.queue[0].action,
        CardAction::CancelOrder {
            unit: harvester,
            key: oxide_sim::OrderKey::Walk { tile: goal },
            from_end: 0,
        }
    );
    let stop = panel.stop.as_ref().expect("a busy unit can stop");
    assert_eq!(stop.action, CardAction::Dispatch(Action::StopOrScrap));
    assert_eq!(stop.hotkey, "X");
}

/// Places `kind` on the first tile the sim accepts near the
/// harvester, returns the site.
fn place(
    game: &mut Game,
    builder: oxide_sim::UnitId,
    kind: BuildingKind,
    queue: bool,
) -> BuildingId {
    use chassis::grid::TilePos;
    let here = game.state.unit(builder).expect("builder").tile();
    let before: Vec<BuildingId> = game.state.buildings().iter().map(|b| b.id).collect();
    for dy in -6i32..=6 {
        for dx in -6i32..=6 {
            let (x, y) = (here.x + dx, here.y + dy);
            if x < 0 || y < 0 {
                continue;
            }
            let anchor = TilePos::new(x, y);
            if !game.state.can_place(game.presentation.human, kind, anchor) {
                continue;
            }
            game.state.tick(&[PlayerCommand {
                player: game.presentation.human,
                command: Command::Build {
                    units: vec![builder],
                    kind,
                    anchor,
                    queue,
                    defer: false,
                },
            }]);
            if let Some(b) = game
                .state
                .buildings()
                .iter()
                .find(|b| !before.contains(&b.id) && b.kind == kind)
            {
                return b.id;
            }
        }
    }
    panic!("no ground accepted a {}", kind.name());
}

fn builder_game() -> (Game, oxide_sim::UnitId) {
    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = 5000;
    let game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("skirmish builds");
    let harvester = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Harvester)
        .expect("starting harvester")
        .id;
    (game, harvester)
}

#[test]
fn build_chips_wear_the_works_they_are_raising() {
    let (mut game, harvester) = builder_game();
    let turret = place(&mut game, harvester, BuildingKind::Turret, false);
    let array = place(&mut game, harvester, BuildingKind::Array, true);
    game.presentation.selection.units = vec![harvester];
    let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
    assert_eq!(panel.queue.len(), 2, "two legs of one program");
    // Two Build chips look different: each carries its own works,
    // ghosted while the site is still rising.
    assert_eq!(
        panel.queue[0].icon,
        CardIcon::Order {
            subject: OrderSubject::Building(BuildingKind::Turret),
            verb: VerbIcon::Build,
            ghost: true,
        }
    );
    assert_eq!(
        panel.queue[1].icon,
        CardIcon::Order {
            subject: OrderSubject::Building(BuildingKind::Array),
            verb: VerbIcon::Build,
            ghost: true,
        }
    );
    assert!(
        panel.queue[0].title.starts_with("Build - Turret"),
        "{}",
        panel.queue[0].title
    );
    assert!(
        panel.queue[1].title.starts_with("Build - Array"),
        "{}",
        panel.queue[1].title
    );
    assert!(panel.queue[0].desc.iter().any(|l| l.contains("% raised")));
    assert!(
        panel.queue[0].progress.is_some(),
        "a site chip meters its rise"
    );
    assert_eq!(
        panel.queue[0].action,
        CardAction::CancelSite(turret),
        "the active site can be abandoned from its order chip"
    );
    assert_eq!(
        panel.queue[1].action,
        CardAction::CancelSite(array),
        "a queued paid site targets its own works"
    );
}

#[test]
fn deferred_build_chips_cancel_their_logical_sites() {
    use chassis::grid::TilePos;

    let (game, harvester) = builder_game();
    let program = Program {
        orders: vec![Order::Found {
            kind: BuildingKind::Bastion,
            anchor: TilePos::new(11, 7),
        }],
        looping: false,
    };
    let card = own_order_card(&game.view(), harvester, &program, 0, &Projection::default());
    assert_eq!(
        card.action,
        CardAction::CancelFound(BuildingKind::Bastion, TilePos::new(11, 7))
    );
    assert!(card.desc.iter().any(|line| line.contains("planned site")));
}

/// Contiguous index per [`Order`] variant: a new order stops this
/// compiling until its chip is covered below.
fn order_kind(order: &Order) -> usize {
    match order {
        Order::Idle => 0,
        Order::Run { .. } => 1,
        Order::Harvest { .. } => 2,
        Order::Attack { .. } => 3,
        Order::Build { .. } => 4,
        Order::Repair { .. } => 5,
        Order::Hunt { .. } => 6,
        Order::Salvage { .. } => 7,
        Order::Found { .. } => 8,
        Order::RepairUnit { .. } => 9,
        Order::Advance { .. } => 10,
        Order::Board { .. } => 11,
        Order::Unload { .. } => 12,
        Order::Land { .. } => 13,
        Order::ReturnCargo { .. } => 14,
    }
}

#[test]
fn every_own_chip_removes_its_order_but_sites_cancel_outright() {
    use chassis::grid::TilePos;
    use oxide_sim::{AttackTarget, ContactId, Goal, OrderKey};

    let (mut game, harvester) = builder_game();
    let site = place(&mut game, harvester, BuildingKind::Turret, false);
    let foundry = human_foundry(&game);
    let tile = TilePos::new(9, 9);
    let goal = Goal::at(tile);
    let contact = AttackTarget::Contact(ContactId(7));
    let planned = TilePos::new(11, 7);
    let program = Program {
        orders: vec![
            Order::Idle,
            Order::Run { goal },
            Order::Hunt { goal },
            Order::Advance { goal },
            Order::Attack {
                target: contact,
                pursue: false,
                resume: Some(goal),
            },
            Order::Land {
                goal: TilePos::new(3, 3),
                from: Some(tile),
            },
            Order::Land {
                goal: tile,
                from: None,
            },
            Order::Attack {
                target: contact,
                pursue: true,
                resume: None,
            },
            Order::Unload {
                at: goal,
                reverse: false,
            },
            Order::Harvest {
                node: TilePos::new(4, 4),
                anchor: tile,
                retiring: false,
            },
            Order::ReturnCargo {
                foundry,
                repair: false,
            },
            Order::Build { site },
            Order::Build { site: foundry },
            Order::Found {
                kind: BuildingKind::Bastion,
                anchor: planned,
            },
            Order::Repair { building: foundry },
            Order::Salvage { building: foundry },
            Order::RepairUnit { unit: harvester },
            Order::Board {
                transport: harvester,
            },
        ],
        looping: false,
    };
    let mut kinds: Vec<usize> = program.orders.iter().map(order_kind).collect();
    kinds.sort_unstable();
    kinds.dedup();
    assert_eq!(kinds, (0..15).collect::<Vec<_>>(), "every order has a chip");

    let view = game.view();
    let actions: Vec<CardAction> = (0..program.orders.len())
        .map(|index| {
            own_order_card(&view, harvester, &program, index, &Projection::default()).action
        })
        .collect();
    let remove = |key, from_end| CardAction::CancelOrder {
        unit: harvester,
        key,
        from_end,
    };
    let walk = OrderKey::Walk { tile };
    assert_eq!(
        actions,
        [
            CardAction::None,
            remove(walk, 4),
            remove(walk, 3),
            remove(walk, 2),
            remove(walk, 1),
            remove(walk, 0),
            remove(OrderKey::Land { pad: tile }, 0),
            remove(OrderKey::Attack { objective: contact }, 0),
            remove(OrderKey::Unload { tile }, 0),
            remove(OrderKey::Harvest { anchor: tile }, 0),
            remove(OrderKey::ReturnCargo, 0),
            CardAction::CancelSite(site),
            remove(OrderKey::Build { site: foundry }, 0),
            CardAction::CancelFound(BuildingKind::Bastion, planned),
            remove(OrderKey::Repair { building: foundry }, 0),
            remove(OrderKey::Salvage { building: foundry }, 0),
            remove(OrderKey::RepairUnit { unit: harvester }, 0),
            remove(
                OrderKey::Board {
                    transport: harvester
                },
                0
            ),
        ]
    );
}

#[test]
fn a_chip_whose_subject_is_gone_falls_back_to_the_bare_verb() {
    // Orders outlive their subjects by a tick — the panel names
    // what it can find and never invents a silhouette.
    let (game, _) = builder_game();
    let dangling = Order::Repair {
        building: BuildingId(9999),
    };
    let card = order_card(&game.view(), &dangling, true, true, None);
    assert_eq!(card.icon, CardIcon::Verb(VerbIcon::Repair));
    assert_eq!(card.title, "Repair (now)");
    assert!(card.progress.is_none());
    assert_eq!(card.desc.len(), 1, "no detail line it cannot back up");
}

#[test]
fn a_foreign_program_is_never_enriched() {
    // A teammate's dock shows the verb and nothing about what it acts
    // on, rather than relying on what team sight shares.
    let (mut game, harvester) = builder_game();
    place(&mut game, harvester, BuildingKind::Turret, false);
    game.presentation.selection.units = vec![harvester];
    let own = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
    assert!(matches!(own.queue[0].icon, CardIcon::Order { .. }));
    let order = game.state.unit(harvester).expect("builder").order;
    let bare = order_card(&game.view(), &order, true, false, None);
    assert_eq!(bare.icon, CardIcon::Verb(VerbIcon::Build));
    assert_eq!(bare.title, "Build (now)");
    assert!(bare.progress.is_none());
}

#[test]
fn weapon_lines_read_from_the_stats_table() {
    let sentinel = weapon_lines(oxide_sim::UnitKind::Sentinel);
    assert_eq!(sentinel.len(), 2, "main gun and the anti-air poke");
    assert!(sentinel[0].contains("dmg"));
    assert!(sentinel[0].contains("tiles"));
    assert!(sentinel[0].contains("ground"));
    assert!(sentinel[1].contains("air"));
    let bombard = weapon_lines(oxide_sim::UnitKind::Bombard);
    assert!(bombard[0].contains("projectile"));
    assert!(bombard[0].contains("splash"));
}

#[test]
fn panel_copy_uses_only_supported_font_glyphs() {
    let units = [
        UnitKind::Harvester,
        UnitKind::Sentinel,
        UnitKind::Scuttler,
        UnitKind::Lancer,
        UnitKind::Bombard,
        UnitKind::Flakhound,
        UnitKind::Buzzard,
        UnitKind::Talon,
    ];
    let buildings = [
        BuildingKind::Foundry,
        BuildingKind::Fabricator,
        BuildingKind::Turret,
        BuildingKind::FlakTurret,
        BuildingKind::Bastion,
        BuildingKind::Array,
        BuildingKind::Reclaimer,
        BuildingKind::RepairBay,
        BuildingKind::Extractor,
    ];
    let supported = |text: &str| text.is_ascii();
    let assert_card = |card: &Card| {
        assert!(supported(&card.title), "card title: {}", card.title);
        assert!(supported(&card.hotkey), "card hotkey: {}", card.hotkey);
        if let Some(why) = &card.why {
            assert!(supported(why), "card refusal: {why}");
        }
        for line in &card.desc {
            assert!(supported(line), "card description: {line}");
        }
    };
    let assert_panel = |panel: &Panel| {
        assert!(supported(&panel.title), "panel title: {}", panel.title);
        assert!(
            supported(&panel.summary),
            "panel subtitle: {}",
            panel.summary
        );
        assert!(
            supported(&panel.queue_label),
            "panel queue label: {}",
            panel.queue_label
        );
        for row in &panel.info.rows {
            assert!(
                supported(&row.label) && supported(&row.value),
                "panel stat: {row:?}"
            );
        }
        for status in &panel.info.status {
            assert!(supported(status), "panel status: {status}");
        }
        for card in panel.roster.iter().chain(&panel.cards).chain(&panel.queue) {
            assert_card(card);
        }
    };
    for kind in units {
        assert!(supported(unit_flavor(kind)), "{} flavor", kind.name());
        for line in weapon_lines(kind) {
            assert!(supported(&line), "{} weapon: {line}", kind.name());
        }
    }
    for kind in buildings {
        assert!(supported(building_flavor(kind)), "{} flavor", kind.name());
    }

    let mut foundry_game = game();
    foundry_game.presentation.selection.buildings = vec![human_foundry(&foundry_game)];
    assert_panel(
        &build_for_palette(&foundry_game.view(), &BindingMap::classic(), false)
            .expect("Foundry panel"),
    );

    let (mut builder_game, harvester) = builder_game();
    let site = place(&mut builder_game, harvester, BuildingKind::Array, false);
    builder_game.presentation.selection.units = vec![harvester];
    assert_panel(
        &build_for_palette(&builder_game.view(), &BindingMap::classic(), false)
            .expect("Harvester panel"),
    );
    builder_game.presentation.selection.units.clear();
    builder_game.presentation.selection.buildings = vec![site];
    assert_panel(
        &build_for_palette(&builder_game.view(), &BindingMap::classic(), false)
            .expect("site panel"),
    );
}

#[test]
fn a_selected_bastion_shows_its_minimum_and_maximum_range() {
    let (mut game, harvester) = builder_game();
    let bastion = place(&mut game, harvester, BuildingKind::Bastion, false);
    game.presentation.selection.units.clear();
    game.presentation.selection.buildings = vec![bastion];

    let panel =
        build_for_palette(&game.view(), &BindingMap::classic(), false).expect("Bastion panel");
    assert_eq!(panel.title, "Bastion");
    assert_eq!(stat(&panel, "Range").value, "2.5-11.0 tiles");
    assert_eq!(stat(&panel, "Sight").value, "6 tiles");
    assert_eq!(
        stat(&panel, "Ground").icon,
        Some(info::StatIcon::Capability(CapabilityIcon::Weapon))
    );
}

#[test]
fn a_single_unit_exposes_static_combat_facts_but_a_group_does_not() {
    let mut game = game();
    let sentinel = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Sentinel)
        .expect("starting sentinel")
        .id;
    game.presentation.selection.units = vec![sentinel];
    let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
    assert_eq!(stat(&panel, "Ground").value, "10 dmg/hit");
    assert_eq!(
        stat(&panel, "Ground").icon,
        Some(info::StatIcon::Capability(CapabilityIcon::Weapon))
    );
    assert_eq!(stat(&panel, "Air").value, "4 dmg/hit");
    assert_eq!(
        stat(&panel, "Air").icon,
        Some(info::StatIcon::Capability(CapabilityIcon::AirWeapon))
    );
    assert_eq!(stat(&panel, "Speed").value, "2.2 tiles/s");

    let harvester = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Harvester)
        .expect("starting harvester")
        .id;
    game.presentation.selection.units = vec![sentinel, harvester];
    let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
    assert!(
        panel.info.rows.is_empty(),
        "mixed selections keep combat detail out of the command band"
    );
    assert_eq!(
        panel.roster.len(),
        2,
        "each selected kind gets one roster chip"
    );
    assert!(
        panel.summary.is_empty(),
        "the counted roster tiles replace the redundant kind list"
    );
    assert!(
        panel
            .cards
            .iter()
            .all(|card| !matches!(card.action, CardAction::FilterKind(_))),
        "roster filters cannot consume command-card capacity"
    );
}

#[test]
fn production_queue_time_counts_partial_head_and_marks_a_blocked_spawn_ready() {
    let queue = std::collections::VecDeque::from([UnitKind::Harvester, UnitKind::Sentinel]);
    assert_eq!(
        production_queue_label(&queue, 25).as_deref(),
        Some("11.3s"),
        "75 head ticks plus 150 queued ticks"
    );
    assert_eq!(
        production_queue_label(&queue, UnitKind::Harvester.stats().train_ticks).as_deref(),
        Some("Ready"),
        "nothing behind a blocked head is counting down"
    );
    assert_eq!(
        production_queue_label(
            &std::collections::VecDeque::from([UnitKind::Harvester]),
            UnitKind::Harvester.stats().train_ticks,
        )
        .as_deref(),
        Some("Ready")
    );
    assert!(production_queue_label(&std::collections::VecDeque::new(), 0).is_none());
}
