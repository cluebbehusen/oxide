//! Player-facing scripted opponent contracts.

use oxide_sim::bot::{Brain, Dials, seat_bots};
use oxide_sim::scenario::{BotConfig, BotDifficulty, BotStance, BuildingSpec, UnitSpec};
use oxide_sim::{
    BuildingKind, Command, Event, GameResult, PlayerId, Scenario, TICKS_PER_SECOND, Target,
    UnitKind,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SCENARIO: AtomicU64 = AtomicU64::new(0);

struct TempScenario(PathBuf);

impl Drop for TempScenario {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn write_scenario(json: &str) -> TempScenario {
    let id = NEXT_SCENARIO.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "oxide-scripted-scenario-{}-{id}.json",
        std::process::id()
    ));
    std::fs::write(&path, json).expect("scenario fixture is written");
    TempScenario(path)
}

#[test]
fn seating_keeps_scripted_and_empty_chairs_distinct() {
    let mut scenario = Scenario::skirmish();
    scenario.players[0].bot = true;
    scenario.players[0].bot_config = Some(BotConfig::default());
    scenario.players[1].bot = true;
    scenario.players[1].bot_config = Some(BotConfig::default());

    let bots = seat_bots(&scenario);
    assert_eq!(bots.len(), 2);
    assert_eq!(bots[0].player(), PlayerId(0));
    assert_eq!(bots[1].player(), PlayerId(1));

    scenario.players[0].bot_config = None;
    let bots = seat_bots(&scenario);
    assert_eq!(
        bots.len(),
        1,
        "a config-less bot flag remains an empty chair"
    );
    assert_eq!(bots[0].player(), PlayerId(1));
}

#[test]
fn bot_config_writes_and_reads_only_the_current_shape() {
    let json = serde_json::to_string(&BotConfig::default()).expect("scripted config serializes");
    assert_eq!(json, r#"{"controller":"scripted"}"#);
    assert_eq!(
        serde_json::from_str::<BotConfig>(&json).expect("scripted config round-trips"),
        BotConfig::default()
    );
    assert!(
        serde_json::from_str::<BotConfig>(r#"{"controller":"scripted","level":"hard"}"#).is_err(),
        "retired controller settings are not silently ignored"
    );
    assert!(
        serde_json::from_str::<BotConfig>(r#"{"level":"medium"}"#).is_err(),
        "retired settings belong only to the versioned replay loader"
    );

    let configured =
        BotConfig::scripted(BotDifficulty::Prime, BotStance::Aggressive, 9_876_543_210);
    let json = serde_json::to_string(&configured).expect("configured bot serializes");
    assert_eq!(
        json,
        r#"{"controller":"scripted","difficulty":"prime","stance":"aggressive","personality_seed":9876543210}"#
    );
    assert_eq!(
        serde_json::from_str::<BotConfig>(&json).expect("configured bot round-trips"),
        configured
    );
    assert!(
        serde_json::from_str::<BotConfig>(r#"{"controller":"scripted","difficulty":"impossible"}"#)
            .is_err(),
        "unknown difficulty values are rejected"
    );
    assert!(
        serde_json::from_str::<BotConfig>(
            r#"{"controller":"scripted","stance":"balanced","mystery":1}"#
        )
        .is_err(),
        "unknown current fields are rejected"
    );
}

#[test]
fn bot_setting_names_are_stable_and_cli_parseable() {
    for difficulty in BotDifficulty::ALL {
        assert_eq!(
            difficulty.as_str().parse::<BotDifficulty>().unwrap(),
            difficulty
        );
        assert_eq!(difficulty.to_string(), difficulty.as_str());
    }
    for stance in BotStance::ALL {
        assert_eq!(stance.as_str().parse::<BotStance>().unwrap(), stance);
        assert_eq!(stance.to_string(), stance.as_str());
    }
    assert_eq!("PRIME".parse(), Ok(BotDifficulty::Prime));
    assert_eq!("Turtle".parse(), Ok(BotStance::Turtle));
    assert!("hard".parse::<BotDifficulty>().is_err());
    assert!("rush".parse::<BotStance>().is_err());
}

#[test]
fn scenario_load_rejects_a_legacy_bot_config() {
    let mut document = serde_json::to_value(Scenario::skirmish()).expect("scenario serializes");
    document["players"][1]["bot_config"] = serde_json::json!({"level": "expert"});
    let json = serde_json::to_string(&document).expect("scenario document serializes");
    let fixture = write_scenario(&json);

    assert!(
        Scenario::load(&fixture.0).is_err(),
        "an authored scenario must not silently select a different controller"
    );
}

#[test]
fn standard_uses_the_full_fog_honest_tree_without_redefining_the_overseer() {
    let balanced = Dials::balanced();
    assert!(balanced.fog_honest);
    assert!(balanced.tech);
    assert!(balanced.turret_response);
    assert!(balanced.scouting);
    assert!(balanced.aa_response);
    assert!(balanced.radar);
    assert!(balanced.reclaimers);
    assert!(balanced.repair);
    assert!(balanced.air_harass);
    assert!(balanced.salvage);
    assert!(balanced.deep_tech);
    assert!(balanced.extractors);
    assert!(balanced.upgrades);
    assert!(balanced.expansion);
    assert!(balanced.ferry);
    assert!(balanced.mines);

    let scripted = Brain::balanced(PlayerId(1), 73);
    let overseer = Brain::overseer(PlayerId(1), 73);
    let profile = scripted
        .profile()
        .expect("player-facing brain has a profile");
    let scripted_expected = Dials::scripted(
        profile,
        oxide_sim::bot::DifficultyTuning::for_level(BotDifficulty::Standard),
    );
    assert_eq!(scripted.dials(), &scripted_expected);
    let mut overseer_surface = overseer.dials().clone();
    overseer_surface.army_size = Dials::overseer().army_size;
    assert_eq!(overseer_surface, Dials::overseer());
    assert!(
        scripted.dials().cadence > overseer.dials().cadence,
        "Standard loses attention cadence while retaining the full legal strategy surface"
    );
    assert_eq!(profile.difficulty, BotDifficulty::Standard);
    assert_eq!(profile.stance, BotStance::Balanced);
    assert!(
        overseer.profile().is_none(),
        "QA brain remains profile-free"
    );
}

#[test]
fn scripted_profile_is_seat_symmetric_while_overseer_keeps_legacy_jitter() {
    let config = BotConfig {
        difficulty: BotDifficulty::Prime,
        stance: BotStance::Aggressive,
        personality_seed: 8_675_309,
    };
    let left = Brain::scripted(PlayerId(0), 1, config);
    let right = Brain::scripted(PlayerId(1), 1, config);

    assert_eq!(left.profile(), right.profile());
    assert_eq!(left.dials(), right.dials());

    let left_overseer = Brain::overseer(PlayerId(0), 1);
    let right_overseer = Brain::overseer(PlayerId(1), 1);
    assert_ne!(
        left_overseer.dials().army_size,
        right_overseer.dials().army_size,
        "the profile-free QA controller retains its frozen seat jitter"
    );
}

#[test]
fn scripted_seat_is_deterministic_and_makes_progress_past_the_opening() {
    let mut scenario = Scenario::skirmish();
    scenario.players[1].bot = true;
    scenario.players[1].bot_config = Some(BotConfig::default());
    let mut left = scenario.build().expect("skirmish builds");
    let mut right = scenario.build().expect("skirmish builds again");
    let mut left_bots = seat_bots(&scenario);
    let mut right_bots = seat_bots(&scenario);
    let starting_units = left.units().len();
    let starting_buildings = left.buildings().len();
    let mut active_thinks = 0_u32;
    let mut rejected_commands = Vec::new();

    // Four simulated minutes: enough to exercise
    // harvesting, production, construction, and the first strategic
    // transition rather than merely accepting an opening command.
    for _ in 0..4_800 {
        let left_commands: Vec<_> = left_bots
            .iter_mut()
            .flat_map(|bot| bot.act(&left))
            .collect();
        let right_commands: Vec<_> = right_bots
            .iter_mut()
            .flat_map(|bot| bot.act(&right))
            .collect();
        assert_eq!(left_commands, right_commands);
        active_thinks += u32::from(!left_commands.is_empty());
        let left_report = left.tick(&left_commands);
        let right_report = right.tick(&right_commands);
        assert_eq!(
            left_report, right_report,
            "identical controllers produced different observable events"
        );
        rejected_commands.extend(left_report.events.iter().filter_map(|event| match event {
            Event::CommandRejected {
                player: PlayerId(1),
                reason,
            } => Some((left_report.tick, *reason)),
            _ => None,
        }));
        assert_eq!(left.hash(), right.hash());
    }

    assert!(
        rejected_commands.is_empty(),
        "the scripted seat issued rejected commands: {rejected_commands:?}"
    );
    assert!(
        active_thinks > 10,
        "the scripted seat stopped issuing commands"
    );
    assert!(
        left.units().len() > starting_units || left.buildings().len() > starting_buildings,
        "the scripted seat never turned its economy into a unit or structure"
    );
}

#[test]
fn balanced_mirror_plays_a_complete_decisive_match() {
    let mut scenario = Scenario::skirmish();
    for player in &mut scenario.players {
        player.bot = true;
        player.bot_config = Some(BotConfig::default());
    }
    let mut state = scenario.build().expect("skirmish builds");
    let mut bots = seat_bots(&scenario);
    let mut rejected_commands = Vec::new();

    for _ in 0..30_000 {
        if state.result().is_some() {
            break;
        }
        let commands: Vec<_> = bots.iter_mut().flat_map(|bot| bot.act(&state)).collect();
        let report = state.tick(&commands);
        rejected_commands.extend(report.events.into_iter().filter_map(|event| match event {
            Event::CommandRejected { player, reason } => Some((report.tick, player, reason)),
            _ => None,
        }));
    }

    assert!(
        rejected_commands.is_empty(),
        "the complete mirror produced rejected bot commands: {rejected_commands:?}"
    );
    assert!(
        matches!(state.result(), Some(GameResult::Victory { .. })),
        "the player-facing mirror should finish a real game: {:?}",
        state.result()
    );
}

#[test]
fn prime_air_operation_suppresses_visible_flak_before_committing_bombers() {
    let mut scenario = Scenario::skirmish();
    scenario.name = "Focused air operation".to_owned();
    scenario.seed = 0x0A16_0001;
    scenario.map = open_air_operation_map();
    scenario.meta = None;
    scenario.players[0].scrap = 0;
    scenario.players[0].bot = true;
    scenario.players[0].bot_config = Some(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Aggressive,
        u64::MAX,
    ));
    scenario.players[1].scrap = 0;
    scenario.players[1].bot = false;
    scenario.players[1].bot_config = None;
    scenario.buildings = vec![
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Fabricator,
            x: 3,
            y: 3,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Airworks,
            x: 7,
            y: 3,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Crucible,
            x: 11,
            y: 3,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Array,
            x: 24,
            y: 16,
        },
        BuildingSpec {
            player: 1,
            kind: BuildingKind::FlakTurret,
            x: 25,
            y: 10,
        },
        BuildingSpec {
            player: 1,
            kind: BuildingKind::Crucible,
            x: 30,
            y: 10,
        },
    ];
    scenario.units = vec![
        UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: 5,
            y: 18,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Kestrel,
            x: 18,
            y: 18,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Bombard,
            x: 17,
            y: 9,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Bombard,
            x: 17,
            y: 11,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Condor,
            x: 7,
            y: 12,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Condor,
            x: 7,
            y: 14,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 3,
            y: 20,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 4,
            y: 20,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 5,
            y: 20,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 6,
            y: 20,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 7,
            y: 20,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 8,
            y: 20,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 9,
            y: 20,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 10,
            y: 20,
        },
    ];

    let mut state = scenario.build().expect("focused scenario builds");
    assert_eq!(
        state
            .units()
            .iter()
            .filter(|unit| !unit.kind.stats().weapons.is_empty())
            .count(),
        12,
        "the integration fixture must remain exactly at the maintained operation-maturity boundary"
    );
    let flak = state
        .buildings()
        .iter()
        .find(|building| {
            building.player == PlayerId(1) && building.kind == BuildingKind::FlakTurret
        })
        .expect("enemy flak exists")
        .id;
    let objective = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(1) && building.kind == BuildingKind::Crucible)
        .expect("enemy objective exists")
        .id;
    let scout = state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Kestrel)
        .expect("scout exists")
        .id;
    let mut artillery: Vec<_> = state
        .units()
        .iter()
        .filter(|unit| unit.kind == UnitKind::Bombard)
        .map(|unit| unit.id)
        .collect();
    let mut bombers: Vec<_> = state
        .units()
        .iter()
        .filter(|unit| unit.kind == UnitKind::Condor)
        .map(|unit| unit.id)
        .collect();
    artillery.sort_unstable();
    artillery.truncate(1);
    bombers.sort_unstable();
    let mut brain = Brain::scripted(
        PlayerId(0),
        scenario.seed,
        scenario.players[0].bot_config.expect("bot is configured"),
    );
    let mut first_suppression = None;
    let mut flak_destroyed = None;
    let mut first_strike = None;
    let mut rejections = Vec::new();

    for _ in 0..2_500 {
        let tick = state.current_tick();
        let commands = brain.act(&state);
        for command in &commands {
            if let Command::Attack {
                units,
                target: Target::Building(target),
                ..
            } = &command.command
            {
                if *target == flak && *units == artillery {
                    first_suppression.get_or_insert(tick);
                }
                if *target == objective && *units == bombers {
                    first_strike.get_or_insert(tick);
                }
            }
        }
        let report = state.tick(&commands);
        rejections.extend(report.events.into_iter().filter_map(|event| match event {
            Event::CommandRejected {
                player: PlayerId(0),
                reason,
            } => Some(reason),
            _ => None,
        }));
        if flak_destroyed.is_none() && state.buildings().iter().all(|building| building.id != flak)
        {
            flak_destroyed = Some(tick);
        }
        if first_strike.is_some() {
            break;
        }
    }

    let suppression = first_suppression.expect("the artillery group focused the visible flak");
    let destroyed = flak_destroyed.expect("the ordinary artillery fire destroyed the flak");
    let strike =
        first_strike.expect("the exact bomber wing struck after re-verifying the corridor");
    assert!(suppression < destroyed);
    assert!(
        destroyed < strike,
        "bombers committed before flak destruction was observed"
    );
    assert!(
        state.units().iter().any(|unit| unit.id == scout),
        "the scout should spot from outside the known flak envelope"
    );
    assert!(
        rejections.is_empty(),
        "bot commands were rejected: {rejections:?}"
    );
}

#[test]
fn support_identity_reserves_an_exact_relief_group_for_a_visible_allied_emergency() {
    let mut scenario = Scenario::skirmish();
    scenario.name = "Focused team relief".to_owned();
    scenario.seed = 0x0A16_0002;
    scenario.map = open_team_relief_map();
    scenario.meta = None;
    let mut bot = scenario.players[0].clone();
    bot.name = "Support bot".to_owned();
    bot.team = Some(0);
    bot.scrap = 0;
    bot.bot = true;
    bot.bot_config = Some(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Turtle,
        0,
    ));
    let mut ally = scenario.players[0].clone();
    ally.name = "Ally".to_owned();
    ally.team = Some(0);
    ally.scrap = 0;
    ally.bot = false;
    ally.bot_config = None;
    let mut enemy = scenario.players[1].clone();
    enemy.name = "Enemy".to_owned();
    enemy.team = Some(1);
    enemy.scrap = 0;
    enemy.bot = false;
    enemy.bot_config = None;
    scenario.players = vec![bot, ally, enemy];
    scenario.buildings.clear();
    scenario.units = vec![
        UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: 6,
            y: 18,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 7,
            y: 12,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 7,
            y: 13,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 7,
            y: 14,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 7,
            y: 15,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 7,
            y: 16,
        },
        UnitSpec {
            player: 2,
            kind: UnitKind::Sentinel,
            x: 29,
            y: 14,
        },
    ];

    let mut state = scenario.build().expect("team-relief scenario builds");
    let allied_foundry = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(1) && building.kind == BuildingKind::Foundry)
        .expect("allied foundry exists")
        .anchor;
    let allied_foundry_size = BuildingKind::Foundry.base_stats().size;
    let owned_fighters: Vec<_> = state
        .units()
        .iter()
        .filter(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Sentinel)
        .map(|unit| unit.id)
        .collect();
    let mut brain = Brain::scripted(
        PlayerId(0),
        scenario.seed,
        scenario.players[0].bot_config.expect("bot is configured"),
    );

    let mut relief = None;
    let mut command_trace = Vec::new();
    for _ in 0..=TICKS_PER_SECOND * 2 {
        let commands = brain.act(&state);
        command_trace.push((state.current_tick(), commands.clone()));
        relief = commands.iter().find_map(|command| match &command.command {
            Command::AttackMove { units, goal, .. }
                if (allied_foundry.x..allied_foundry.x + allied_foundry_size.0)
                    .contains(&goal.x)
                    && (allied_foundry.y..allied_foundry.y + allied_foundry_size.1)
                        .contains(&goal.y) =>
            {
                Some(units.clone())
            }
            _ => None,
        });
        let report = state.tick(&commands);
        assert!(report.events.iter().all(
            |event| !matches!(event, Event::CommandRejected { player, .. } if *player == PlayerId(0))
        ));
        if relief.is_some() {
            break;
        }
    }
    let relief = relief.unwrap_or_else(|| {
        panic!("sustained visible allied pressure triggers a relief march: {command_trace:?}")
    });
    assert_eq!(relief.len(), 2);
    assert!(relief.iter().all(|id| owned_fighters.contains(id)));
    assert_eq!(
        owned_fighters.len() - relief.len(),
        3,
        "the emergency response must preserve a real home-defense floor"
    );
}

#[test]
fn scripted_lift_launches_three_full_manifests_together_and_returns_every_carrier() {
    let mut scenario = Scenario::skirmish();
    scenario.name = "Focused ferry cycle".to_owned();
    scenario.seed = 0x0A16_0003;
    scenario.map = ferry_cycle_map();
    scenario.meta = None;
    scenario.players[0].scrap = 0;
    scenario.players[0].bot = true;
    scenario.players[0].bot_config = Some(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Balanced,
        17,
    ));
    scenario.players[1].scrap = 0;
    scenario.players[1].bot = false;
    scenario.players[1].bot_config = None;
    scenario.buildings = vec![BuildingSpec {
        player: 0,
        kind: BuildingKind::Array,
        x: 7,
        y: 6,
    }];
    scenario.units = vec![UnitSpec {
        player: 0,
        kind: UnitKind::Kestrel,
        x: 20,
        y: 7,
    }];
    scenario.units.extend((0..3).map(|index| UnitSpec {
        player: 0,
        kind: UnitKind::Skyhook,
        x: 3 + index,
        y: 7,
    }));
    scenario.units.extend((0..16).map(|index| UnitSpec {
        player: 0,
        kind: UnitKind::Sentinel,
        x: 3 + index % 6,
        y: 9 + index / 6,
    }));

    let mut state = scenario.build().expect("severed ferry scenario builds");
    let mut carriers: Vec<_> = state
        .units()
        .iter()
        .filter(|unit| unit.kind == UnitKind::Skyhook)
        .map(|unit| unit.id)
        .collect();
    carriers.sort_unstable();
    let mut brain = Brain::scripted(
        PlayerId(0),
        scenario.seed,
        scenario.players[0].bot_config.unwrap(),
    );
    let mut loads = Vec::new();
    let mut boarded = Vec::new();
    let mut unloaded = Vec::new();
    let mut no_routes = Vec::new();
    let mut stalls = Vec::new();
    let mut deaths = Vec::new();
    let mut rejected = Vec::new();
    let mut first_unload = None;
    let mut unload_sites = Vec::new();
    let mut assaulted = Vec::new();

    for _ in 0..6_000 {
        let commands = brain.act(&state);
        for command in &commands {
            match &command.command {
                Command::Load {
                    units, transport, ..
                } if carriers.contains(transport) => loads.push((*transport, units.clone())),
                Command::Unload { transport, at, .. } if carriers.contains(transport) => {
                    first_unload.get_or_insert((state.current_tick(), boarded.len()));
                    unload_sites.push(*at);
                }
                Command::AttackMove { units, goal, .. } if goal.x > 14 => {
                    assaulted.extend(units.iter().copied());
                }
                _ => {}
            }
        }
        let report = state.tick(&commands);
        for event in report.events {
            match event {
                Event::UnitBoarded { unit, .. } => boarded.push(unit),
                Event::UnitUnloaded { unit, .. } => unloaded.push(unit),
                Event::OrderStalled {
                    unit,
                    player: PlayerId(0),
                    reason,
                    ..
                } => {
                    if reason == oxide_sim::event::StallReason::NoRoute {
                        no_routes.push(unit);
                    }
                    stalls.push((unit, reason));
                }
                Event::UnitDied {
                    unit,
                    player: PlayerId(0),
                    ..
                } => deaths.push(unit),
                Event::CommandRejected {
                    player: PlayerId(0),
                    reason,
                } => rejected.push(reason),
                _ => {}
            }
        }
        if !unloaded.is_empty()
            && carriers.iter().all(|carrier| {
                state
                    .unit(*carrier)
                    .is_some_and(|unit| unit.tile().x < 14 && unit.cargo.is_empty())
            })
        {
            break;
        }
    }

    boarded.sort_unstable();
    boarded.dedup();
    unloaded.sort_unstable();
    unloaded.dedup();
    loads.sort_unstable_by_key(|(transport, _)| *transport);
    assert_eq!(loads.len(), 3, "each carrier receives one manifest command");
    assert_eq!(
        loads.iter().map(|(_, riders)| riders.len()).sum::<usize>(),
        12,
        "the wealthy lift fills three carriers and leaves a four-unit home floor"
    );
    let mut manifest_riders: Vec<_> = loads
        .iter()
        .flat_map(|(_, riders)| riders.iter().copied())
        .collect();
    manifest_riders.sort_unstable();
    manifest_riders.dedup();
    assert_eq!(manifest_riders.len(), 12, "manifests must be disjoint");
    assert_eq!(
        boarded.len(),
        12,
        "every assigned rider boards; manifests={loads:?}, boarded={boarded:?}, unloaded={unloaded:?}, stalls={stalls:?}, deaths={deaths:?}"
    );
    assert_eq!(
        unloaded, boarded,
        "the same exact wave reaches the far shore"
    );
    assert_eq!(
        first_unload.map(|(_, boarded_before_launch)| boarded_before_launch),
        Some(12),
        "no carrier may launch before the shared boarding barrier is full"
    );
    unload_sites.sort_unstable();
    unload_sites.dedup();
    assert_eq!(unload_sites.len(), 3, "carriers use distinct landing slots");
    assaulted.sort_unstable();
    assaulted.dedup();
    assert_eq!(
        assaulted, unloaded,
        "every landed survivor is handed off to the island assault"
    );
    assert!(carriers.iter().all(|carrier| {
        state
            .unit(*carrier)
            .is_some_and(|unit| unit.tile().x < 14 && unit.cargo.is_empty())
    }));
    assert!(
        no_routes.is_empty(),
        "the complete fair controller cycle issued impossible routes: {no_routes:?}"
    );
    assert!(
        rejected.is_empty(),
        "the complete wave must use only legal player commands: {rejected:?}"
    );
}

fn open_air_operation_map() -> Vec<String> {
    let mut rows = vec![vec!['.'; 40]; 24];
    rows.first_mut().expect("map has a north edge").fill('#');
    rows.last_mut().expect("map has a south edge").fill('#');
    for row in &mut rows {
        row[0] = '#';
        row[39] = '#';
    }
    rows[17][3] = '1';
    rows[17][34] = '2';
    rows.into_iter()
        .map(|row| row.into_iter().collect())
        .collect()
}

fn open_team_relief_map() -> Vec<String> {
    let mut rows = vec![vec!['.'; 40]; 24];
    rows.first_mut().expect("map has a north edge").fill('#');
    rows.last_mut().expect("map has a south edge").fill('#');
    for row in &mut rows {
        row[0] = '#';
        row[39] = '#';
    }
    rows[17][3] = '1';
    rows[17][28] = '2';
    rows[3][34] = '3';
    rows.into_iter()
        .map(|row| row.into_iter().collect())
        .collect()
}

fn ferry_cycle_map() -> Vec<String> {
    let mut rows = vec![vec!['.'; 28]; 16];
    rows.first_mut().expect("map has a north edge").fill('#');
    rows.last_mut().expect("map has a south edge").fill('#');
    for row in &mut rows {
        row[0] = '#';
        row[27] = '#';
        row[14] = '~';
    }
    rows[7][2] = '1';
    rows[7][24] = '2';
    rows.into_iter()
        .map(|row| row.into_iter().collect())
        .collect()
}
