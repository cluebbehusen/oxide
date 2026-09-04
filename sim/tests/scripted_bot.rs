//! Player-facing scripted opponent contracts.

use chassis::grid::TilePos;
use oxide_sim::bot::trace::{
    ClaimOwnerTrace, ObligationKeyTrace, ProducerJobAccessTrace, ProposalDispositionTrace,
    ProposalKeyTrace,
};
use oxide_sim::bot::{
    Brain, ConnectedForceStatus, ConnectedRecoveryReasonTrace, ConnectedRejectionReasonTrace,
    Dials, Observation, Orientation, PublicMapBriefing, TargetEvidenceTrace, seat_bots,
};
use oxide_sim::scenario::{
    BotConfig, BotDifficulty, BotStance, BuildingSpec, PlayerSpec, UnitSpec,
};
use oxide_sim::stats::{Domain, QUEUE_CAP, Role};
use oxide_sim::{
    BuildingKind, Command, Event, Faction, GameResult, Order, PlayerId, Scenario, TICKS_PER_SECOND,
    Target, UnitKind,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SCENARIO: AtomicU64 = AtomicU64::new(0);

fn public_map(scenario: &Scenario) -> Arc<PublicMapBriefing> {
    Arc::new(
        PublicMapBriefing::from_scenario(scenario)
            .expect("the focused scenario has a public map briefing"),
    )
}

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

    let bots = seat_bots(&scenario).expect("the skirmish has a briefing");
    assert_eq!(bots.len(), 2);
    assert_eq!(bots[0].player(), PlayerId(0));
    assert_eq!(bots[1].player(), PlayerId(1));

    scenario.players[0].bot_config = None;
    let bots = seat_bots(&scenario).expect("the skirmish has a briefing");
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

    let scenario = Scenario::skirmish();
    let scripted = Brain::balanced(PlayerId(1), public_map(&scenario));
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
    let scenario = Scenario::skirmish();
    let public_map = public_map(&scenario);
    let left = Brain::scripted(PlayerId(0), config, Arc::clone(&public_map));
    let right = Brain::scripted(PlayerId(1), config, public_map);

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
    let mut left_bots = seat_bots(&scenario).expect("the skirmish has a briefing");
    let mut right_bots = seat_bots(&scenario).expect("the skirmish has a briefing");
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

fn is_opening_core_unit(kind: UnitKind) -> bool {
    matches!(kind.role(), Role::Sentinel | Role::Warden | Role::Breaker)
}

fn ground_strength(kind: UnitKind, hp: u32) -> u64 {
    let damage_per_hundred_ticks = kind
        .stats()
        .weapons
        .iter()
        .filter(|weapon| weapon.targets.covers(Domain::Ground))
        .map(|weapon| u64::from(weapon.damage) * 100 / u64::from(weapon.cooldown_ticks))
        .sum::<u64>();
    u64::from(hp) * damage_per_hundred_ticks
}

fn full_ground_strength(kind: UnitKind) -> u64 {
    ground_strength(kind, kind.stats().max_hp)
}

fn opening_core_strength(observation: &Observation, commands: &[oxide_sim::PlayerCommand]) -> u64 {
    let live = observation
        .my_units
        .iter()
        .filter(|unit| is_opening_core_unit(unit.kind))
        .map(|unit| ground_strength(unit.kind, unit.hp))
        .sum::<u64>();
    let queued = observation
        .my_queues
        .iter()
        .flatten()
        .copied()
        .filter(|kind| is_opening_core_unit(*kind))
        .map(full_ground_strength)
        .sum::<u64>();
    let planned = commands
        .iter()
        .filter(|command| command.player == observation.me)
        .filter_map(|command| match command.command {
            Command::Train { kind, .. } if is_opening_core_unit(kind) => Some(kind),
            _ => None,
        })
        .map(full_ground_strength)
        .sum::<u64>();

    live + queued + planned
}

#[test]
fn scrapheap_and_prime_reach_their_opening_core_before_the_first_fabricator() {
    const OPENING_END: u64 = 5_000;

    for (difficulty, floor) in [
        (BotDifficulty::Scrapheap, 4_u64),
        (BotDifficulty::Prime, 8_u64),
    ] {
        let mut scenario = Scenario::skirmish();
        scenario.players[0].bot = true;
        scenario.players[0].bot_config = Some(BotConfig::scripted(
            difficulty,
            BotStance::Balanced,
            1_616_201,
        ));
        scenario.players[1].bot = false;
        scenario.players[1].bot_config = None;

        let mut state = scenario.build().expect("Skirmish builds");
        let mut brain = Brain::scripted(
            PlayerId(0),
            scenario.players[0]
                .bot_config
                .expect("the bot is configured"),
            public_map(&scenario),
        );
        let target_strength = full_ground_strength(UnitKind::Sentinel) * floor;
        let mut first_fabricator = None;

        while state.current_tick() < OPENING_END && first_fabricator.is_none() {
            let observation = Observation::fog_honest(&state, PlayerId(0));
            let commands = brain.act(&state);
            if commands.iter().any(|command| {
                matches!(
                    command.command,
                    Command::Build {
                        kind: BuildingKind::Fabricator,
                        ..
                    }
                )
            }) {
                let projected_strength = opening_core_strength(&observation, &commands);
                assert!(
                    projected_strength >= target_strength,
                    "{difficulty:?} spent on its first Fabricator with strength {projected_strength}, below its {floor}-equivalent target {target_strength}"
                );
                first_fabricator = Some(state.current_tick());
            }

            let report = state.tick(&commands);
            assert!(
                report.events.iter().all(|event| !matches!(
                    event,
                    Event::CommandRejected {
                        player: PlayerId(0),
                        ..
                    }
                )),
                "{difficulty:?} issued a rejected opening command: {:?}",
                report.events
            );
        }

        assert!(
            first_fabricator.is_some(),
            "{difficulty:?} did not reach a Fabricator within {OPENING_END} ticks"
        );
    }
}

#[test]
fn prime_skirmish_places_an_accepted_turret_on_the_hostile_approach() {
    let mut scenario = Scenario::skirmish();
    scenario.players[0].bot = true;
    scenario.players[0].bot_config = Some(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Balanced,
        9_000,
    ));
    scenario.players[1].bot = false;
    scenario.players[1].bot_config = None;

    let briefing = public_map(&scenario);
    let own_start = briefing
        .starting_foundries()
        .iter()
        .find(|start| start.player == PlayerId(0))
        .expect("Skirmish has the Prime seat's public starting Foundry")
        .anchor;
    let hostile_start = briefing
        .hostile_starting_foundries(PlayerId(0))
        .next()
        .expect("Skirmish has a public hostile starting Foundry")
        .anchor;
    let config = scenario.players[0].bot_config.expect("Prime is configured");
    let mut brain = Brain::scripted(PlayerId(0), config, briefing);
    let mut state = scenario.build().expect("Skirmish builds");
    let mut turret_anchor = None;
    let mut rejected = Vec::new();

    for _ in 0..TICKS_PER_SECOND * 4 * 60 {
        let commands = brain.act(&state);
        let proposed = commands.iter().find_map(|command| match command.command {
            Command::Build {
                kind: BuildingKind::Turret,
                anchor,
                ..
            } if command.player == PlayerId(0) => Some(anchor),
            _ => None,
        });
        if let Some(anchor) = proposed {
            if let Some(expected) = turret_anchor {
                assert_eq!(
                    anchor, expected,
                    "Prime should keep the accepted strategic site while its founder walks"
                );
            } else {
                let observation = Observation::fog_honest(&state, PlayerId(0));
                assert!(
                    observation.enemy_units.is_empty() && observation.enemy_buildings.is_empty(),
                    "the first Turret should use the public approach before live hostile contact"
                );
                turret_anchor = Some(anchor);
            }
        }
        let report = state.tick(&commands);
        rejected.extend(report.events.into_iter().filter_map(|event| match event {
            Event::CommandRejected {
                player: PlayerId(0),
                reason,
            } => Some((report.tick, reason)),
            _ => None,
        }));
        if turret_anchor.is_some_and(|anchor| {
            state.buildings().iter().any(|building| {
                building.player == PlayerId(0)
                    && building.kind == BuildingKind::Turret
                    && building.anchor == anchor
            })
        }) {
            break;
        }
    }

    let turret_anchor = turret_anchor.expect("Prime proposed its bounded pre-contact Turret");
    assert!(
        rejected.is_empty(),
        "Prime issued rejected commands before its Turret: {rejected:?}"
    );
    assert!(
        state.buildings().iter().any(|building| {
            building.player == PlayerId(0)
                && building.kind == BuildingKind::Turret
                && building.anchor == turret_anchor
        }),
        "the ordinary Build command should place the strategic Turret site"
    );
    assert_ne!(
        turret_anchor,
        TilePos::new(4, 1),
        "the public hostile approach should replace the old rear-corner placement"
    );

    let foundry_size = BuildingKind::Foundry.base_stats().size;
    let turret_size = BuildingKind::Turret.base_stats().size;
    let own_center = (
        own_start.x * 2 + foundry_size.0,
        own_start.y * 2 + foundry_size.1,
    );
    let hostile_center = (
        hostile_start.x * 2 + foundry_size.0,
        hostile_start.y * 2 + foundry_size.1,
    );
    let turret_center = (
        turret_anchor.x * 2 + turret_size.0,
        turret_anchor.y * 2 + turret_size.1,
    );
    let hostile_direction = (
        i64::from(hostile_center.0 - own_center.0),
        i64::from(hostile_center.1 - own_center.1),
    );
    let turret_direction = (
        i64::from(turret_center.0 - own_center.0),
        i64::from(turret_center.1 - own_center.1),
    );
    let forward_progress =
        hostile_direction.0 * turret_direction.0 + hostile_direction.1 * turret_direction.1;
    assert!(
        forward_progress > 0,
        "the Turret at {turret_anchor:?} must face the public hostile approach from {own_start:?} toward {hostile_start:?}"
    );
}

#[test]
fn prime_skirmish_recalls_one_public_probe_without_reprobing_during_a_chase() {
    let mut scenario = Scenario::skirmish();
    scenario.seed = 7_000;
    let briefing = public_map(&scenario);
    let own_start = briefing
        .starting_foundries()
        .iter()
        .find(|start| start.player == PlayerId(0))
        .expect("Skirmish has the Prime seat's public starting Foundry")
        .anchor;
    let hostile_start = briefing
        .hostile_starting_foundries(PlayerId(0))
        .next()
        .expect("Skirmish has a public hostile starting Foundry")
        .anchor;
    let (dx, dy) = (hostile_start.x - own_start.x, hostile_start.y - own_start.y);
    let distance = dx.abs().max(dy.abs());
    let public_probe = TilePos::new(
        hostile_start.x - dx * 5 / distance,
        hostile_start.y - dy * 5 / distance,
    );
    let mut state = scenario.build().expect("Skirmish builds");
    let starting_harvesters: Vec<_> = state
        .units()
        .iter()
        .filter(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Harvester)
        .map(|unit| unit.id)
        .collect();
    let mut prime = Brain::scripted(
        PlayerId(0),
        BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 9_000),
        Arc::clone(&briefing),
    );
    let mut overseer = Brain::overseer_with_policy_seed(PlayerId(1), 0);
    let mut probes = Vec::new();
    let mut recalls = Vec::new();
    let mut probed_workers = None;

    for tick in 0..1_200 {
        let mut commands = prime.act(&state);
        for command in &commands {
            let Command::Move { units, goal, .. } = &command.command else {
                continue;
            };
            if !units.iter().any(|unit| starting_harvesters.contains(unit)) {
                continue;
            }
            if *goal == public_probe {
                probes.push((tick, units.clone()));
                probed_workers = Some(units.clone());
            } else if probed_workers
                .as_ref()
                .is_some_and(|probed| units.iter().any(|unit| probed.contains(unit)))
            {
                recalls.push((tick, units.clone(), *goal));
            }
        }
        commands.extend(overseer.act(&state));
        state.tick(&commands);
    }

    assert_eq!(
        probes.len(),
        1,
        "Prime must not cycle replacement Harvesters through the same public probe: {probes:?}"
    );
    assert!(
        !recalls.is_empty(),
        "current danger should recall the assigned public probe"
    );
    assert!(
        recalls.iter().all(|(_, units, _)| *units == probes[0].1),
        "only the assigned scout should receive the chase evacuation: {recalls:?}"
    );
    assert!(
        recalls
            .windows(2)
            .all(|pair| { pair[1].2.chebyshev(own_start) < pair[0].2.chebyshev(own_start) }),
        "continued pursuit may require another evacuation, but every goal must make strict progress home: {recalls:?}"
    );
}

#[test]
fn southeast_brain_maps_public_start_recon_through_an_ordinary_state_command() {
    const WIDTH: usize = 40;
    const HEIGHT: usize = 24;
    let northwest = TilePos::new(4, 4);
    let southeast = TilePos::new(34, 18);
    let mut rows = vec![vec![b'.'; WIDTH]; HEIGHT];
    rows[northwest.y as usize][northwest.x as usize] = b'1';
    rows[southeast.y as usize][southeast.x as usize] = b'2';
    let scenario = Scenario {
        name: "seat-one public recon orientation".into(),
        seed: 91,
        map: rows
            .into_iter()
            .map(|row| String::from_utf8(row).expect("ASCII map"))
            .collect(),
        players: vec![
            PlayerSpec {
                name: "northwest".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "southeast".into(),
                faction: Faction::Cupric,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
        ],
        units: vec![
            UnitSpec {
                player: 1,
                kind: UnitKind::Harvester,
                x: 28,
                y: 16,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Harvester,
                x: 29,
                y: 16,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Harvester,
                x: 30,
                y: 16,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Harvester,
                x: 31,
                y: 16,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Gnat,
                x: 30,
                y: 18,
            },
        ],
        buildings: Vec::new(),
        meta: None,
    };
    let mut state = scenario
        .build()
        .expect("the open orientation fixture builds");
    assert!(
        Observation::fog_honest(&state, PlayerId(1))
            .enemy_buildings
            .is_empty(),
        "the northwest Foundry begins outside current sight"
    );
    let scout = state
        .units()
        .iter()
        .find(|unit| unit.player == PlayerId(1) && unit.kind == UnitKind::Gnat)
        .expect("the southeast seat owns a dedicated scout")
        .id;
    let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 9_001);
    let mut brain = Brain::scripted(PlayerId(1), config, public_map(&scenario));

    let commands = brain.act(&state);
    let expected_goal = TilePos::new(0, 3);
    assert!(
        commands.iter().any(|command| {
            command.player == PlayerId(1)
                && matches!(
                    &command.command,
                    Command::Move {
                        units,
                        goal,
                        queue: false,
                    } if units == &vec![scout] && *goal == expected_goal
                )
        }),
        "the oriented public start should lower back to the southeast seat's world-space rear look: {commands:?}"
    );

    let report = state.tick(&commands);
    assert!(
        !report.events.iter().any(|event| matches!(
            event,
            Event::CommandRejected {
                player: PlayerId(1),
                ..
            }
        )),
        "the mapped reconnaissance command must be accepted: {:?}",
        report.events
    );
    assert_eq!(
        state.unit(scout).expect("the scout remains alive").order,
        Order::Move {
            goal: expected_goal,
        },
        "State must receive the same world-space goal emitted by the rotated Brain"
    );
}

#[test]
fn southeast_brain_maps_a_hidden_public_extractor_prior_without_learning_its_owner() {
    const WIDTH: usize = 40;
    const HEIGHT: usize = 24;
    let enemy_start = TilePos::new(26, 18);
    let own_start = TilePos::new(34, 18);
    let frame = TilePos::new(8, 5);
    let mut rows = vec![vec![b'.'; WIDTH]; HEIGHT];
    rows[enemy_start.y as usize][enemy_start.x as usize] = b'1';
    rows[own_start.y as usize][own_start.x as usize] = b'2';
    rows[frame.y as usize][frame.x as usize] = b'E';
    let scenario = Scenario {
        name: "seat-one public Extractor recon orientation".into(),
        seed: 92,
        map: rows
            .into_iter()
            .map(|row| String::from_utf8(row).expect("ASCII map"))
            .collect(),
        players: vec![
            PlayerSpec {
                name: "nearby opponent".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "southeast".into(),
                faction: Faction::Cupric,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
        ],
        units: vec![
            UnitSpec {
                player: 1,
                kind: UnitKind::Harvester,
                x: 31,
                y: 16,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Harvester,
                x: 32,
                y: 16,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Harvester,
                x: 33,
                y: 16,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Harvester,
                x: 34,
                y: 16,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Gnat,
                x: 32,
                y: 18,
            },
        ],
        buildings: Vec::new(),
        meta: None,
    };
    let mut occupied_scenario = scenario.clone();
    occupied_scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Extractor,
        x: frame.x,
        y: frame.y,
    });
    let mut bare = scenario.build().expect("the bare-frame fixture builds");
    let occupied = occupied_scenario
        .build()
        .expect("the hidden occupied-frame fixture builds");
    let bare_view = Observation::fog_honest(&bare, PlayerId(1));
    let occupied_view = Observation::fog_honest(&occupied, PlayerId(1));
    assert_eq!(
        bare_view, occupied_view,
        "an unseen restored Extractor must not leak through the fog-honest observation"
    );
    assert!(
        bare_view
            .enemy_buildings
            .iter()
            .any(|building| building.kind == BuildingKind::Foundry && building.seen),
        "the nearby current base keeps its normal refresh from being due at tick zero"
    );
    assert!(!bare_view.explored(frame));
    let scout = bare
        .units()
        .iter()
        .find(|unit| unit.player == PlayerId(1) && unit.kind == UnitKind::Gnat)
        .expect("the southeast seat owns a dedicated scout")
        .id;
    let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 9_002);
    let briefing = public_map(&scenario);
    assert_eq!(*briefing, *public_map(&occupied_scenario));
    let mut bare_brain = Brain::scripted(PlayerId(1), config, Arc::clone(&briefing));
    let mut occupied_brain = Brain::scripted(PlayerId(1), config, briefing);

    let commands = bare_brain.act(&bare);
    assert_eq!(
        commands,
        occupied_brain.act(&occupied),
        "hidden current ownership must not influence public-prior reconnaissance"
    );
    let expected_goal = frame.offset(1, 1);
    assert!(
        commands.iter().any(|command| {
            command.player == PlayerId(1)
                && matches!(
                    &command.command,
                    Command::Move {
                        units,
                        goal,
                        queue: false,
                    } if units == &vec![scout] && *goal == expected_goal
                )
        }),
        "the oriented public frame should lower back to a world-space tile inside its footprint: {commands:?}"
    );

    let report = bare.tick(&commands);
    assert!(
        !report.events.iter().any(|event| matches!(
            event,
            Event::CommandRejected {
                player: PlayerId(1),
                ..
            }
        )),
        "the mapped Extractor reconnaissance command must be accepted: {:?}",
        report.events
    );
    assert_eq!(
        bare.unit(scout).expect("the scout remains alive").order,
        Order::Move {
            goal: expected_goal,
        },
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
    let mut bots = seat_bots(&scenario).expect("the skirmish has a briefing");
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
fn connected_package_admission_fields_every_required_capability() {
    let scenario = connected_package_scenario(false);
    let mut state = scenario.build().expect("connected-package scenario builds");
    let mut brain = Brain::scripted(
        PlayerId(0),
        scenario.players[0].bot_config.expect("bot is configured"),
        public_map(&scenario),
    );

    let decision = brain.act_traced(&state);
    let trace = decision
        .trace
        .as_ref()
        .expect("the admission decision is traced");
    assert_eq!(
        trace.connected_force.status,
        ConnectedForceStatus::Active,
        "connected operation was not admitted: {trace:#?}"
    );
    assert_eq!(
        trace
            .connected_force
            .target
            .as_ref()
            .expect("the connected operation has an objective")
            .evidence,
        TargetEvidenceTrace::Current
    );
    let package = trace
        .connected_force
        .package
        .as_ref()
        .expect("the viable current target admits a connected package");
    assert!(package.chosen_capability.recon >= package.minimum_capability.recon);
    assert!(package.chosen_capability.suppression >= package.minimum_capability.suppression);
    assert!(package.chosen_capability.strike >= package.minimum_capability.strike);

    let trained: Vec<_> = decision
        .commands
        .iter()
        .filter_map(|command| match &command.command {
            Command::Train { kind, .. } => Some(*kind),
            _ => None,
        })
        .collect();
    for (family, demands) in [
        ("reconnaissance", &package.demands.recon),
        ("suppression", &package.demands.suppression),
        ("strike", &package.demands.strike),
    ] {
        assert!(!demands.is_empty(), "the package omitted {family}");
    }
    let assigned = &trace.connected_force.assigned;
    assert!(
        assigned.scout.is_some()
            || package
                .demands
                .recon
                .iter()
                .any(|demand| trained.contains(&demand.kind)),
        "reconnaissance was neither assigned nor queued"
    );
    assert!(
        !assigned.suppression.is_empty()
            || package
                .demands
                .suppression
                .iter()
                .any(|demand| trained.contains(&demand.kind)),
        "suppression was neither assigned nor queued"
    );
    assert!(
        !assigned.strike.is_empty()
            || package
                .demands
                .strike
                .iter()
                .any(|demand| trained.contains(&demand.kind)),
        "strike was neither assigned nor queued"
    );
    assert!(
        !trained.is_empty(),
        "the fixture intentionally lacks a complete package, so admission must lower its missing capability"
    );
    assert!(trained.iter().any(|kind| {
        package
            .demands
            .recon
            .iter()
            .chain(&package.demands.suppression)
            .chain(&package.demands.strike)
            .any(|demand| demand.kind == *kind)
    }));

    let report = state.tick(&decision.commands);
    assert!(report.events.iter().all(|event| {
        !matches!(
            event,
            Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )
    }));
}

#[test]
fn connected_package_waits_for_and_reuses_a_full_production_queue() {
    let mut scenario = connected_package_scenario(false);
    scenario.players[0].scrap = 50_000;
    scenario.units.retain(|unit| unit.kind != UnitKind::Bombard);
    let briefing = public_map(&scenario);
    let config = scenario.players[0].bot_config.expect("bot is configured");
    let mut state = scenario.build().expect("connected-package scenario builds");
    let fabricator = state
        .buildings()
        .iter()
        .find(|building| {
            building.player == PlayerId(0) && building.kind == BuildingKind::Fabricator
        })
        .expect("fixture has one Fabricator")
        .id;
    let fill_queue: Vec<_> = (0..QUEUE_CAP)
        .map(|_| oxide_sim::PlayerCommand {
            player: PlayerId(0),
            command: Command::Train {
                building: fabricator,
                kind: UnitKind::Lancer,
            },
        })
        .collect();
    let report = state.tick(&fill_queue);
    assert!(report.events.iter().all(|event| {
        !matches!(
            event,
            Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )
    }));
    assert_eq!(
        state
            .building(fabricator)
            .expect("Fabricator remains")
            .queue
            .len(),
        QUEUE_CAP
    );

    let mut brain = Brain::scripted(PlayerId(0), config, briefing);
    let mut admitted_deadline = None;
    let mut bombard_queued_at = None;
    for _ in 0..3_000 {
        let decision = brain.act_traced(&state);
        if let Some(trace) = &decision.trace
            && trace.connected_force.status == ConnectedForceStatus::Active
            && let Some(package) = &trace.connected_force.package
            && package
                .demands
                .suppression
                .iter()
                .any(|demand| demand.kind == UnitKind::Bombard && demand.count > 0)
        {
            admitted_deadline.get_or_insert(package.preparation_deadline);
        }
        if decision.commands.iter().any(|command| {
            matches!(
                command.command,
                Command::Train {
                    building,
                    kind: UnitKind::Bombard,
                } if building == fabricator
            )
        }) {
            bombard_queued_at = Some(state.current_tick());
        }

        let report = state.tick(&decision.commands);
        assert!(report.events.iter().all(|event| {
            !matches!(
                event,
                Event::CommandRejected {
                    player: PlayerId(0),
                    ..
                }
            )
        }));
        if bombard_queued_at.is_some() {
            break;
        }
    }

    let deadline = admitted_deadline.expect("the full lane remains feasible over the horizon");
    let queued_at = bombard_queued_at.unwrap_or_else(|| {
        panic!(
            "the operation did not refill the first released slot; tick={}, queue={:?}",
            state.current_tick(),
            state
                .building(fabricator)
                .expect("Fabricator remains")
                .queue
        )
    });
    assert!(
        queued_at < deadline,
        "the required provider must be purchased inside the original preparation window"
    );
}

#[test]
fn connected_package_uses_only_a_producer_that_can_reach_its_staging_route() {
    let mut scenario = connected_package_scenario(false);
    scenario.name = "Connected package with one isolated producer".to_owned();
    scenario.units.retain(|unit| unit.kind != UnitKind::Bombard);
    scenario.units.push(UnitSpec {
        player: 0,
        kind: UnitKind::Sentinel,
        x: 8,
        y: 21,
    });
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Fabricator,
        x: 11,
        y: 3,
    });
    let mut rows = scenario
        .map
        .iter()
        .map(|row| row.chars().collect::<Vec<_>>())
        .collect::<Vec<_>>();
    for row in [1, 6] {
        for cell in &mut rows[row][1..=6] {
            *cell = '^';
        }
    }
    for row in &mut rows[2..6] {
        row[1] = '^';
        row[6] = '^';
    }
    scenario.map = rows
        .into_iter()
        .map(|row| row.into_iter().collect())
        .collect();

    let state = scenario
        .build()
        .expect("isolated-producer connected-package scenario builds");
    let isolated = state
        .buildings()
        .iter()
        .find(|building| {
            building.player == PlayerId(0)
                && building.kind == BuildingKind::Fabricator
                && building.anchor == TilePos::new(3, 3)
        })
        .expect("fixture retains the enclosed Fabricator")
        .id;
    let reachable = state
        .buildings()
        .iter()
        .find(|building| {
            building.player == PlayerId(0)
                && building.kind == BuildingKind::Fabricator
                && building.anchor == TilePos::new(11, 3)
        })
        .expect("fixture adds the reachable Fabricator")
        .id;

    let mut legal_state = state.clone();
    let legal_report = legal_state.tick(&[oxide_sim::PlayerCommand {
        player: PlayerId(0),
        command: Command::Train {
            building: isolated,
            kind: UnitKind::Bombard,
        },
    }]);
    assert!(
        legal_report.events.iter().all(|event| !matches!(
            event,
            Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )),
        "the enclosed Fabricator must be locally legal so this tests strategic route access"
    );

    let mut brain = Brain::scripted(
        PlayerId(0),
        scenario.players[0].bot_config.expect("bot is configured"),
        public_map(&scenario),
    );
    let decision = brain.act_traced(&state);
    let trace = decision
        .trace
        .as_ref()
        .expect("the admission think is traced");
    assert_eq!(
        trace.connected_force.status,
        ConnectedForceStatus::Active,
        "the reachable lane should keep the connected package admissible: {trace:#?}"
    );
    let suppression_commands = decision
        .commands
        .iter()
        .filter_map(|command| match command.command {
            Command::Train {
                building,
                kind: UnitKind::Bombard,
            } => Some(building),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        suppression_commands,
        vec![reachable],
        "the package must not assign its suppression provider to the publicly isolated lane"
    );

    let mut accepted_state = state;
    let report = accepted_state.tick(&decision.commands);
    assert!(report.events.iter().all(|event| !matches!(
        event,
        Event::CommandRejected {
            player: PlayerId(0),
            ..
        }
    )));
}

#[test]
fn shipped_brain_allocates_compatible_foundry_connected_and_standing_work() {
    let foundry_cost = BuildingKind::Foundry
        .base_stats()
        .construction
        .expect("Foundries are constructible")
        .cost;
    let residual_investment = BuildingKind::Turret
        .base_stats()
        .construction
        .expect("Turrets are constructible")
        .cost
        .saturating_add(UnitKind::Harvester.stats().cost);
    let scenario = foundry_connected_allocation_scenario(
        foundry_cost
            .saturating_add(UnitKind::Buzzard.stats().cost)
            .saturating_add(UnitKind::Sentinel.stats().cost)
            .saturating_add(residual_investment),
    );
    let mut state = scenario
        .build()
        .expect("the cross-domain allocation scenario builds");
    let config = scenario.players[0].bot_config.expect("bot is configured");
    let briefing = public_map(&scenario);
    let home = briefing
        .starting_foundries()
        .iter()
        .find(|start| start.player == PlayerId(0))
        .expect("the configured seat has a public starting Foundry")
        .anchor;
    let orientation = Orientation::for_home(&Observation::fog_honest(&state, PlayerId(0)), home);
    let mut brain = Brain::scripted(PlayerId(0), config, Arc::clone(&briefing));

    let decision = brain.act_traced(&state);
    let trace = decision
        .trace
        .as_ref()
        .expect("the shared allocation decision is traced");
    assert!(trace.allocation.error.is_none());
    assert!(trace.allocation.coordinator_failure.is_none());

    let mut foundry_anchor = None;
    let mut connected_key = None;
    let mut standing_key = None;
    for proposal in &trace.allocation.proposals.entries {
        match proposal.key {
            ProposalKeyTrace::FoundryExpansion { anchor } => {
                assert_eq!(
                    proposal.disposition,
                    ProposalDispositionTrace::Accepted,
                    "the compatible Foundry proposal must be selected: {:?}",
                    trace.allocation.proposals
                );
                foundry_anchor = Some(anchor);
            }
            key @ ProposalKeyTrace::ConnectedOffenseMinimum { .. } => {
                assert_eq!(
                    proposal.disposition,
                    ProposalDispositionTrace::Accepted,
                    "the compatible connected proposal must be selected: {:?}",
                    trace.allocation.proposals
                );
                connected_key = Some(key);
            }
            key @ ProposalKeyTrace::StandingForce { .. } => {
                if proposal.disposition == ProposalDispositionTrace::Accepted {
                    assert!(
                        standing_key.replace(key).is_none(),
                        "only one alternative from the standing-force domain may win: {:?}",
                        trace.allocation.proposals
                    );
                }
            }
        }
    }
    let foundry_anchor = foundry_anchor.expect("the unsupported Extractor proposes a Foundry");
    let connected_key = connected_key.expect("the current target proposes connected offense");
    let standing_key = standing_key.expect("the residual current bank funds standing force");
    let world_foundry_anchor =
        orientation.anchor(foundry_anchor, BuildingKind::Foundry.base_stats().size);

    assert!(
        decision.commands.iter().any(|command| matches!(
            command.command,
            Command::Build {
                kind: BuildingKind::Foundry,
                anchor,
                ..
            } if anchor == world_foundry_anchor
        )),
        "the accepted Foundry must dispatch its exact build: commands={:?}, allocation={:?}",
        decision.commands,
        trace.allocation
    );
    let scheduled_connected = trace
        .allocation
        .producer_schedule
        .entries
        .iter()
        .filter(|job| {
            matches!(
                job.owner,
                ClaimOwnerTrace::Proposal { key } if key == connected_key
            )
        })
        .collect::<Vec<_>>();
    assert!(
        !scheduled_connected.is_empty(),
        "the accepted connected operation should retain its exact future producer work"
    );
    let standing_jobs = trace
        .allocation
        .producer_schedule
        .entries
        .iter()
        .filter(|job| {
            matches!(
                job.owner,
                ClaimOwnerTrace::Proposal { key } if key == standing_key
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        standing_jobs.len(),
        1,
        "one accepted standing-force alternative should own one immediate lane job"
    );
    let standing_job = standing_jobs[0];
    assert_eq!(standing_job.enqueued_at, state.current_tick());
    assert_eq!(standing_job.current_scrap, standing_job.kind.stats().cost);
    assert_eq!(standing_job.forecast_scrap, 0);
    assert!(matches!(
        standing_key,
        ProposalKeyTrace::StandingForce { kind, .. } if kind == standing_job.kind
    ));
    let standing_proposal = trace
        .allocation
        .proposals
        .entries
        .iter()
        .find(|proposal| proposal.key == standing_key)
        .expect("the standing proposal remains visible in the exact trace");
    assert_eq!(
        standing_proposal.claims.minimum_residual_scrap,
        residual_investment
    );
    assert_eq!(
        decision
            .commands
            .iter()
            .filter(|command| matches!(
                command.command,
                Command::Train { building, kind }
                    if building == standing_job.producer && kind == standing_job.kind
            ))
            .count(),
        1,
        "the exact accepted standing-force job must lower to one ordinary Train command"
    );

    let report = state.tick(&decision.commands);
    assert!(
        report.events.iter().all(|event| !matches!(
            event,
            Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )),
        "the authoritative State must accept both domains in one transaction: {:?}",
        report.events
    );
    assert!(state.buildings().iter().any(|building| {
        building.player == PlayerId(0)
            && building.kind == BuildingKind::Foundry
            && building.anchor == world_foundry_anchor
    }));
}

#[test]
fn completed_income_forecast_cannot_fund_an_immediate_standing_purchase() {
    let mut scenario = connected_package_scenario(false);
    scenario.name = "Forecast-only standing-force fixture".to_owned();
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Crucible,
        x: 11,
        y: 3,
    });
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Turret,
        x: 13,
        y: 9,
    });
    scenario.buildings.extend(
        [(15, 3), (18, 3), (21, 3), (15, 6)].map(|(x, y)| BuildingSpec {
            player: 0,
            kind: BuildingKind::Reclaimer,
            x,
            y,
        }),
    );
    scenario.units.extend((0..3).map(|index| UnitSpec {
        player: 0,
        kind: UnitKind::Harvester,
        x: 6 + index,
        y: 18,
    }));

    let reclaimer_credit = u32::try_from(
        scenario
            .buildings
            .iter()
            .filter(|building| building.player == 0 && building.kind == BuildingKind::Reclaimer)
            .count(),
    )
    .expect("the fixture has a small Reclaimer count");
    let queued_shallow_cost = UnitKind::Sentinel.stats().cost.saturating_mul(2);
    let prepare_state = |uncommitted_scrap: u32| {
        let mut prepared = scenario.clone();
        prepared.players[0].scrap = queued_shallow_cost.saturating_add(uncommitted_scrap);
        let mut state = prepared.build().expect("forecast control fixture builds");
        let foundry = state
            .buildings()
            .iter()
            .find(|building| {
                building.player == PlayerId(0) && building.kind == BuildingKind::Foundry
            })
            .expect("the fixture has a completed Foundry")
            .id;
        let report = state.tick(&[
            oxide_sim::PlayerCommand {
                player: PlayerId(0),
                command: Command::Train {
                    building: foundry,
                    kind: UnitKind::Sentinel,
                },
            },
            oxide_sim::PlayerCommand {
                player: PlayerId(0),
                command: Command::Train {
                    building: foundry,
                    kind: UnitKind::Sentinel,
                },
            },
        ]);
        assert!(
            report.events.iter().all(|event| !matches!(
                event,
                Event::CommandRejected {
                    player: PlayerId(0),
                    ..
                }
            )),
            "the exact prepaid shallow queue must be accepted: {:?}",
            report.events
        );
        while state.current_tick() < 24 {
            state.tick(&[]);
        }
        assert_eq!(
            Observation::fog_honest(&state, PlayerId(0)).scrap,
            uncommitted_scrap.saturating_add(reclaimer_credit)
        );
        state
    };

    let mut forecast_state = prepare_state(0);
    let mut forecast_brain = Brain::scripted(
        PlayerId(0),
        scenario.players[0].bot_config.expect("bot is configured"),
        public_map(&scenario),
    );
    let forecast_decision = forecast_brain.act_traced(&forecast_state);
    let forecast_trace = forecast_decision
        .trace
        .as_ref()
        .expect("the forecast-only decision is traced");
    assert_eq!(forecast_trace.resources.current_scrap, reclaimer_credit);
    assert!(
        forecast_trace.resources.forecast_scrap >= UnitKind::Sentinel.stats().cost,
        "completed recurring sources should make at least one ordinary unit affordable later"
    );
    let forecast_connected = forecast_trace
        .allocation
        .proposals
        .entries
        .iter()
        .find(|proposal| {
            matches!(
                proposal.key,
                ProposalKeyTrace::ConnectedOffenseMinimum { .. }
            ) && proposal.disposition == ProposalDispositionTrace::Accepted
        })
        .expect("the exact shallow guard lets forecast-funded connected work remain feasible");
    assert_eq!(
        forecast_connected.claims.minimum_residual_scrap, 0,
        "the paid second queue slot already satisfies the shallow guard"
    );
    let future_connected_jobs = forecast_trace
        .allocation
        .producer_schedule
        .entries
        .iter()
        .filter(|job| {
            matches!(
                job.owner,
                ClaimOwnerTrace::Proposal {
                    key: ProposalKeyTrace::ConnectedOffenseMinimum { .. }
                }
            ) && job.enqueued_at > forecast_state.current_tick()
        })
        .collect::<Vec<_>>();
    assert!(
        !future_connected_jobs.is_empty()
            && future_connected_jobs
                .iter()
                .all(|job| job.forecast_scrap > 0),
        "the fixture must prove forecast income remains useful for deadline-bound future work: {future_connected_jobs:?}"
    );
    assert_eq!(
        future_connected_jobs
            .iter()
            .map(|job| job.current_scrap)
            .sum::<u32>(),
        forecast_trace.resources.current_scrap,
        "the small live bank may part-fund future work but cannot make an immediate unit affordable"
    );
    assert!(
        forecast_trace
            .allocation
            .proposals
            .entries
            .iter()
            .all(|proposal| !matches!(proposal.key, ProposalKeyTrace::StandingForce { .. })),
        "forecast income must not make an immediate standing-force purchase legally affordable"
    );
    assert!(
        forecast_trace
            .allocation
            .producer_schedule
            .entries
            .iter()
            .all(|job| !matches!(
                job.owner,
                ClaimOwnerTrace::Proposal {
                    key: ProposalKeyTrace::StandingForce { .. }
                }
            ))
    );
    assert!(
        forecast_decision
            .commands
            .iter()
            .all(|command| !matches!(command.command, Command::Train { .. })),
        "forecast income must never lower into a command that spends absent current scrap"
    );
    let report = forecast_state.tick(&forecast_decision.commands);
    assert!(report.events.iter().all(|event| !matches!(
        event,
        Event::CommandRejected {
            player: PlayerId(0),
            ..
        }
    )));

    let mut current_state = prepare_state(UnitKind::Lancer.stats().cost);
    let mut current_brain = Brain::scripted(
        PlayerId(0),
        scenario.players[0].bot_config.expect("bot is configured"),
        public_map(&scenario),
    );
    let current_decision = current_brain.act_traced(&current_state);
    let current_trace = current_decision
        .trace
        .as_ref()
        .expect("the current-funded control decision is traced");
    let accepted_standing = current_trace
        .allocation
        .proposals
        .entries
        .iter()
        .find(|proposal| {
            proposal.disposition == ProposalDispositionTrace::Accepted
                && matches!(proposal.key, ProposalKeyTrace::StandingForce { .. })
        })
        .expect("current scrap should expose the standing-force control demand");
    assert_eq!(
        accepted_standing.claims.minimum_residual_scrap, 0,
        "completed tech, an existing Turret, and the shallow queue leave no unrelated investment floor"
    );
    let standing_job = current_trace
        .allocation
        .producer_schedule
        .entries
        .iter()
        .find(|job| {
            matches!(
                job.owner,
                ClaimOwnerTrace::Proposal { key } if key == accepted_standing.key
            )
        })
        .expect("the accepted standing-force control owns a producer job");
    assert_eq!(standing_job.enqueued_at, current_state.current_tick());
    assert_eq!(standing_job.current_scrap, standing_job.kind.stats().cost);
    assert_eq!(standing_job.forecast_scrap, 0);
    assert!(current_decision.commands.iter().any(|command| matches!(
        command.command,
        Command::Train { building, kind }
            if building == standing_job.producer && kind == standing_job.kind
    )));
    let report = current_state.tick(&current_decision.commands);
    assert!(report.events.iter().all(|event| !matches!(
        event,
        Event::CommandRejected {
            player: PlayerId(0),
            ..
        }
    )));
}

#[test]
fn public_hostile_start_routes_standing_force_through_a_reachable_producer() {
    let mut scenario = Scenario::skirmish();
    scenario.name = "Public-start standing routing fixture".to_owned();
    let builder_commitment = BuildingKind::Barricade
        .base_stats()
        .construction
        .expect("Barricades are constructible")
        .cost;
    scenario.players[0].scrap = UnitKind::Sentinel
        .stats()
        .cost
        .saturating_add(builder_commitment);
    scenario.players[0].bot = true;
    scenario.players[0].bot_config = Some(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Balanced,
        17,
    ));
    scenario.players[1].scrap = 0;
    scenario.players[1].bot = false;
    scenario.players[1].bot_config = None;
    scenario.units.retain(|unit| unit.player == 0);
    scenario.units.extend((0..7).map(|index| UnitSpec {
        player: 0,
        kind: UnitKind::Sentinel,
        x: 5 + index % 4,
        y: 8 + index / 4,
    }));
    scenario.units.push(UnitSpec {
        player: 0,
        kind: UnitKind::Harvester,
        x: 8,
        y: 5,
    });
    let mut rows = scenario
        .map
        .iter()
        .map(|row| row.chars().collect::<Vec<_>>())
        .collect::<Vec<_>>();
    rows[4][8] = '.';
    rows[9][15] = '.';
    rows[6][10..=16].fill('^');
    rows[12][10..=16].fill('^');
    for row in &mut rows[6..=12] {
        row[10] = '^';
        row[16] = '^';
    }
    scenario.map = rows
        .into_iter()
        .map(|row| row.into_iter().collect())
        .collect();
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Foundry,
        x: 12,
        y: 8,
    });
    let briefing = public_map(&scenario);
    let mut state = scenario.build().expect("routing fixture builds");
    let home_anchor = briefing
        .starting_foundries()
        .iter()
        .find(|start| start.player == PlayerId(0))
        .expect("the fixture has the bot's public starting Foundry")
        .anchor;
    let home = state
        .buildings()
        .iter()
        .find(|building| {
            building.player == PlayerId(0)
                && building.kind == BuildingKind::Foundry
                && building.anchor == home_anchor
        })
        .expect("the public starting Foundry exists")
        .id;
    let isolated = state
        .buildings()
        .iter()
        .find(|building| {
            building.player == PlayerId(0)
                && building.kind == BuildingKind::Foundry
                && building.anchor == TilePos::new(12, 8)
        })
        .expect("the terrain-enclosed Foundry exists")
        .id;
    let mut local_legality = state.clone();
    let report = local_legality.tick(&[oxide_sim::PlayerCommand {
        player: PlayerId(0),
        command: Command::Train {
            building: isolated,
            kind: UnitKind::Sentinel,
        },
    }]);
    assert!(
        report.events.iter().all(|event| !matches!(
            event,
            Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )),
        "the enclosed producer is locally legal; only strategic reachability should exclude it"
    );
    let builders = state
        .units()
        .iter()
        .filter(|unit| unit.kind == UnitKind::Harvester)
        .map(|unit| unit.id)
        .collect::<Vec<_>>();
    assert_eq!(builders.len(), 4, "the fixture retains its worker floor");
    let report = state.tick(&[oxide_sim::PlayerCommand {
        player: PlayerId(0),
        command: Command::Build {
            units: builders,
            kind: BuildingKind::Barricade,
            anchor: TilePos::new(8, 4),
            queue: false,
            defer: false,
        },
    }]);
    assert!(
        report.events.iter().all(|event| !matches!(
            event,
            Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )),
        "the authoritative builder commitment must be accepted: {:?}",
        report.events
    );
    assert_eq!(
        Observation::fog_honest(&state, PlayerId(0)).scrap,
        UnitKind::Sentinel.stats().cost
    );

    let mut brain = Brain::scripted(
        PlayerId(0),
        scenario.players[0].bot_config.expect("bot is configured"),
        briefing,
    );
    while !state.current_tick().is_multiple_of(brain.dials().cadence) {
        state.tick(&[]);
    }
    let decision = brain.act_traced(&state);
    let trace = decision
        .trace
        .as_ref()
        .expect("the public-prior decision is traced");
    assert_eq!(trace.evidence.current_enemy_units, 0);
    assert_eq!(trace.evidence.current_enemy_buildings, 0);
    assert_eq!(trace.evidence.remembered_enemy_buildings, 0);
    assert_eq!(trace.evidence.radar_blips, 0);
    let standing = trace
        .allocation
        .proposals
        .entries
        .iter()
        .find(|proposal| {
            matches!(proposal.key, ProposalKeyTrace::StandingForce { .. })
                && proposal.disposition == ProposalDispositionTrace::Accepted
        })
        .expect("the uncleared public hostile start should justify force projection");
    assert_eq!(standing.claims.producer_jobs.total, 1);
    let job_claim = standing
        .claims
        .producer_jobs
        .entries
        .first()
        .expect("standing force claims one immediate producer job");
    match &job_claim.access {
        ProducerJobAccessTrace::Flexible { eligible_producers } => {
            assert_eq!(eligible_producers.entries, vec![home]);
            assert_eq!(eligible_producers.total, 1);
        }
        ProducerJobAccessTrace::Fixed { .. } => {
            panic!("fresh standing force must retain flexible routed producer access")
        }
    }
    let scheduled = trace
        .allocation
        .producer_schedule
        .entries
        .iter()
        .find(|job| {
            matches!(
                job.owner,
                ClaimOwnerTrace::Proposal { key } if key == standing.key
            )
        })
        .expect("the accepted standing force owns one exact producer job");
    assert_eq!(scheduled.producer, home);
    assert_ne!(scheduled.producer, isolated);
    assert!(decision.commands.iter().any(|command| matches!(
        command.command,
        Command::Train { building, kind }
            if building == home && kind == scheduled.kind
    )));
    let report = state.tick(&decision.commands);
    assert!(
        report.events.iter().all(|event| !matches!(
            event,
            Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )),
        "the authoritative State must accept the routed Train command: {:?}",
        report.events
    );
}

#[test]
fn active_connected_paid_queue_work_leaves_only_independent_standing_capacity() {
    let mut scenario = connected_package_scenario(true);
    scenario.name = "Active connected paid-queue ownership".to_owned();
    scenario.players[0].bot_config = Some(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Aggressive,
        5,
    ));
    let mut state = scenario.build().expect("paid-queue probe builds");
    let mut brain = Brain::scripted(
        PlayerId(0),
        scenario.players[0].bot_config.expect("bot is configured"),
        public_map(&scenario),
    );
    let admission = brain.act_traced(&state);
    let admission_trace = admission.trace.as_ref().expect("admission is traced");
    assert_eq!(
        admission_trace.connected_force.status,
        ConnectedForceStatus::Active
    );
    let connected_key = admission_trace
        .allocation
        .proposals
        .entries
        .iter()
        .find_map(|proposal| {
            (proposal.disposition == ProposalDispositionTrace::Accepted)
                .then_some(proposal.key)
                .filter(|key| matches!(key, ProposalKeyTrace::ConnectedOffenseMinimum { .. }))
        })
        .expect("the visible rich cluster admits one connected operation");
    let connected_jobs = admission_trace
        .allocation
        .producer_schedule
        .entries
        .iter()
        .filter(|job| {
            matches!(
                job.owner,
                ClaimOwnerTrace::Proposal { key } if key == connected_key
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        admission_trace.allocation.producer_schedule.omitted, 0,
        "the fixture needs the complete exact producer schedule"
    );
    assert!(
        connected_jobs
            .iter()
            .any(|job| job.kind == UnitKind::Bombard),
        "the connected package must own paid ground-suppression production"
    );
    assert!(
        connected_jobs
            .iter()
            .any(|job| job.kind == UnitKind::Buzzard),
        "the connected package must own paid strike production"
    );
    assert!(connected_jobs.iter().all(|job| {
        job.enqueued_at == state.current_tick()
            && job.current_scrap == job.kind.stats().cost
            && job.forecast_scrap == 0
            && job.enqueued_at <= job.starts_at
            && job.starts_at <= job.ready_at
            && job.ready_at < job.ready_before
    }));
    let mut connected_lane_counts = BTreeMap::new();
    for job in &connected_jobs {
        *connected_lane_counts
            .entry((job.producer, job.kind))
            .or_insert(0_usize) += 1;
    }
    for (&(producer, kind), &scheduled_count) in &connected_lane_counts {
        assert_eq!(
            admission
                .commands
                .iter()
                .filter(|command| matches!(
                    command.command,
                    Command::Train { building, kind: trained }
                        if building == producer && trained == kind
                ))
                .count(),
            scheduled_count,
            "every operation-owned lane job must lower exactly once"
        );
    }
    let report = state.tick(&admission.commands);
    assert!(report.events.iter().all(|event| !matches!(
        event,
        Event::CommandRejected {
            player: PlayerId(0),
            ..
        }
    )));

    let earliest_ready = connected_jobs
        .iter()
        .map(|job| job.ready_at)
        .min()
        .expect("the connected package owns paid queue work");
    let observation_tick = earliest_ready / brain.dials().cadence * brain.dials().cadence;
    assert!(
        observation_tick > 0 && observation_tick < earliest_ready,
        "the fixture observes every paid job inside its inclusive enqueue-to-ready lifetime"
    );
    while state.current_tick() < observation_tick {
        state.tick(&[]);
    }
    assert!(connected_jobs.iter().all(|job| {
        job.enqueued_at <= state.current_tick() && state.current_tick() <= job.ready_at
    }));
    for (&(producer, kind), &scheduled_count) in &connected_lane_counts {
        assert_eq!(
            state
                .building(producer)
                .expect("every scheduled producer remains alive")
                .queue
                .iter()
                .filter(|queued| **queued == kind)
                .count(),
            scheduled_count,
            "operation-owned queue inventory must survive through its inclusive ready tick"
        );
    }

    let continued = brain.act_traced(&state);
    let continued_trace = continued
        .trace
        .as_ref()
        .expect("the in-flight queue decision is traced");
    assert_eq!(
        continued_trace.connected_force.status,
        ConnectedForceStatus::Active
    );
    let (objective, anchor) = match connected_key {
        ProposalKeyTrace::ConnectedOffenseMinimum { objective, anchor } => (objective, anchor),
        _ => unreachable!("the filtered proposal key is connected offense"),
    };
    assert!(
        continued_trace
            .allocation
            .obligations
            .entries
            .iter()
            .any(|obligation| matches!(
                obligation.key,
                ObligationKeyTrace::ConnectedOffense {
                    objective: retained_objective,
                    anchor: retained_anchor,
                } if retained_objective == objective && retained_anchor == anchor
            ))
    );
    let accepted_standing = continued_trace
        .allocation
        .proposals
        .entries
        .iter()
        .find_map(|proposal| {
            (proposal.disposition == ProposalDispositionTrace::Accepted)
                .then_some(proposal.key)
                .filter(|key| matches!(key, ProposalKeyTrace::StandingForce { .. }))
        })
        .expect("wealth left after the paid operation funds independent standing force");
    assert!(matches!(
        accepted_standing,
        ProposalKeyTrace::StandingForce {
            kind: UnitKind::Avalanche,
            ..
        }
    ));
    let standing_job = continued_trace
        .allocation
        .producer_schedule
        .entries
        .iter()
        .find(|job| {
            matches!(
                job.owner,
                ClaimOwnerTrace::Proposal { key } if key == accepted_standing
            )
        })
        .expect("the accepted standing alternative owns one exact lane");
    let operation_producers = connected_jobs
        .iter()
        .map(|job| job.producer)
        .collect::<BTreeSet<_>>();
    assert!(
        !operation_producers.contains(&standing_job.producer),
        "standing production may use only producer capacity independent of the paid operation"
    );
    assert!(
        state
            .building(standing_job.producer)
            .is_some_and(|producer| producer.queue.is_empty()),
        "the independent standing lane starts unoccupied"
    );
    assert_eq!(standing_job.enqueued_at, state.current_tick());
    assert_eq!(standing_job.current_scrap, standing_job.kind.stats().cost);
    assert_eq!(standing_job.forecast_scrap, 0);
    assert_eq!(
        continued
            .commands
            .iter()
            .filter(|command| matches!(
                command.command,
                Command::Train { building, kind }
                    if building == standing_job.producer && kind == standing_job.kind
            ))
            .count(),
        1
    );
    assert!(continued.commands.iter().all(|command| {
        !connected_jobs.iter().any(|job| {
            matches!(
                command.command,
                Command::Train { building, kind }
                    if building == job.producer && kind == job.kind
            )
        })
    }));

    let report = state.tick(&continued.commands);
    assert!(report.events.iter().all(|event| !matches!(
        event,
        Event::CommandRejected {
            player: PlayerId(0),
            ..
        }
    )));
}

#[test]
fn full_tech_standing_production_replaces_the_tier_one_fallback_across_cadences() {
    let scenario = full_tech_standing_scenario();
    let mut state = scenario.build().expect("full-tech standing fixture builds");
    let mut brain = Brain::scripted(
        PlayerId(0),
        scenario.players[0].bot_config.expect("bot is configured"),
        public_map(&scenario),
    );
    let mut prior_ready_at = None;

    for cadence in 0..2 {
        let decision = brain.act_traced(&state);
        let trace = decision.trace.as_ref().expect("the cadence is traced");
        assert_eq!(trace.connected_force.status, ConnectedForceStatus::Idle);
        assert_eq!(trace.evidence.current_enemy_units, 0);
        assert_eq!(trace.evidence.current_enemy_buildings, 0);
        let accepted = trace
            .allocation
            .proposals
            .entries
            .iter()
            .filter(|proposal| {
                proposal.disposition == ProposalDispositionTrace::Accepted
                    && matches!(proposal.key, ProposalKeyTrace::StandingForce { .. })
            })
            .collect::<Vec<_>>();
        assert_eq!(
            accepted.len(),
            1,
            "cadence {cadence} should choose exactly one standing alternative"
        );
        assert!(matches!(
            accepted[0].key,
            ProposalKeyTrace::StandingForce {
                kind: UnitKind::Avalanche,
                ..
            }
        ));
        assert!(trace.allocation.proposals.entries.iter().any(|proposal| {
            matches!(
                proposal.key,
                ProposalKeyTrace::StandingForce {
                    kind: UnitKind::Sentinel | UnitKind::Lancer,
                    ..
                }
            ) && proposal.disposition != ProposalDispositionTrace::Accepted
        }));
        let scheduled = trace
            .allocation
            .producer_schedule
            .entries
            .iter()
            .find(|job| {
                matches!(
                    job.owner,
                    ClaimOwnerTrace::Proposal { key } if key == accepted[0].key
                )
            })
            .expect("the selected higher-tier alternative owns one lane");
        assert_eq!(scheduled.enqueued_at, state.current_tick());
        assert_eq!(scheduled.current_scrap, UnitKind::Avalanche.stats().cost);
        assert_eq!(scheduled.forecast_scrap, 0);
        if let Some(previous_ready_at) = prior_ready_at {
            assert_eq!(
                scheduled.starts_at,
                previous_ready_at + 1,
                "the second cadence must account for the first paid queue entry"
            );
        }
        prior_ready_at = Some(scheduled.ready_at);
        assert_eq!(
            decision
                .commands
                .iter()
                .filter(|command| matches!(
                    command.command,
                    Command::Train {
                        building,
                        kind: UnitKind::Avalanche,
                    } if building == scheduled.producer
                ))
                .count(),
            1
        );
        assert!(decision.commands.iter().all(|command| !matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Sentinel | UnitKind::Lancer,
                ..
            }
        )));
        let report = state.tick(&decision.commands);
        assert!(report.events.iter().all(|event| !matches!(
            event,
            Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )));
        if cadence == 0 {
            while !state.current_tick().is_multiple_of(brain.dials().cadence) {
                state.tick(&[]);
            }
        }
    }
}

#[test]
fn standing_force_preserves_and_dispatches_an_exact_legal_turret_threshold() {
    let turret_cost = BuildingKind::Turret
        .base_stats()
        .construction
        .expect("Turrets are constructible")
        .cost;
    let residual_threshold = turret_cost.saturating_add(UnitKind::Harvester.stats().cost);
    let standing_cost = UnitKind::Sentinel.stats().cost;
    let scenario = mature_standing_force_scenario(
        residual_threshold.saturating_add(standing_cost),
        false,
        false,
    );
    let mut state = scenario.build().expect("the exact Turret fixture builds");
    let mut brain = Brain::scripted(
        PlayerId(0),
        scenario.players[0].bot_config.expect("bot is configured"),
        public_map(&scenario),
    );

    let decision = brain.act_traced(&state);
    let trace = decision.trace.as_ref().expect("the decision is traced");
    let standing = trace
        .allocation
        .proposals
        .entries
        .iter()
        .find(|proposal| {
            proposal.disposition == ProposalDispositionTrace::Accepted
                && matches!(
                    proposal.key,
                    ProposalKeyTrace::StandingForce {
                        kind: UnitKind::Sentinel,
                        ..
                    }
                )
        })
        .expect("the exact surplus admits one independently useful screen");
    assert_eq!(standing.claims.minimum_residual_scrap, residual_threshold);
    assert_eq!(
        decision
            .commands
            .iter()
            .filter(|command| matches!(
                command.command,
                Command::Train {
                    kind: UnitKind::Sentinel,
                    ..
                }
            ))
            .count(),
        1,
    );
    assert_eq!(
        decision
            .commands
            .iter()
            .filter(|command| matches!(
                command.command,
                Command::Build {
                    kind: BuildingKind::Turret,
                    ..
                }
            ))
            .count(),
        1,
        "the current-only floor must remain available to the exact scored Turret rung: {:?}",
        decision.commands
    );

    let report = state.tick(&decision.commands);
    assert!(report.events.iter().all(|event| !matches!(
        event,
        Event::CommandRejected {
            player: PlayerId(0),
            ..
        }
    )));
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        UnitKind::Harvester.stats().cost
    );
    assert!(state.buildings().iter().any(|building| {
        building.player == PlayerId(0) && building.kind == BuildingKind::Turret && !building.built
    }));
}

#[test]
fn completed_income_accumulates_into_the_better_shipped_standing_provider() {
    let control =
        mature_standing_accumulation_scenario(UnitKind::Sentinel.stats().cost, false, false);
    let (mut control_state, mut control_brain) = prepared_mature_standing_force(&control);
    let cheap = control_brain.act_traced(&control_state);
    assert_eq!(
        cheap
            .commands
            .iter()
            .filter(|command| matches!(
                command.command,
                Command::Train {
                    kind: UnitKind::Sentinel,
                    ..
                }
            ))
            .count(),
        1,
        "without bounded completed income, the shipped policy buys the useful fallback: {:?}",
        cheap.commands,
    );
    let report = control_state.tick(&cheap.commands);
    assert!(report.events.iter().all(|event| !matches!(
        event,
        Event::CommandRejected {
            player: PlayerId(0),
            ..
        }
    )));

    let scenario =
        mature_standing_accumulation_scenario(UnitKind::Sentinel.stats().cost, true, false);
    let (mut state, mut brain) = prepared_mature_standing_force(&scenario);
    let held = brain.act_traced(&state);
    assert!(
        held.commands.iter().all(|command| !matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Sentinel,
                ..
            }
        )),
        "commands={:?}",
        held.commands,
    );
    let held_trace = held.trace.as_ref().expect("the held decision is traced");
    let wait = held_trace
        .allocation
        .proposals
        .entries
        .iter()
        .find(|proposal| {
            matches!(
                proposal.key,
                ProposalKeyTrace::StandingForce {
                    kind: UnitKind::Warden,
                    ..
                }
            )
        })
        .expect("the bounded Warden wait must participate in shared allocation");
    assert_eq!(wait.disposition, ProposalDispositionTrace::Accepted);
    assert_eq!(wait.claims.current_scrap, UnitKind::Sentinel.stats().cost);
    assert_eq!(
        wait.claims.forecast_scrap_total,
        u128::from(
            UnitKind::Warden
                .stats()
                .cost
                .saturating_sub(UnitKind::Sentinel.stats().cost)
        )
    );
    assert_eq!(wait.claims.deferrable_capital, None);
    assert!(wait.claims.producer_jobs.entries.is_empty());

    let started_at = state.current_tick();
    let mut warden_orders = 0_usize;
    while state.current_tick() < started_at.saturating_add(1_440) {
        let decision = if state.current_tick() == started_at {
            held.clone()
        } else {
            brain.act_traced(&state)
        };
        assert!(decision.commands.iter().all(|command| !matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Sentinel,
                ..
            }
        )));
        warden_orders = warden_orders.saturating_add(
            decision
                .commands
                .iter()
                .filter(|command| {
                    matches!(
                        command.command,
                        Command::Train {
                            kind: UnitKind::Warden,
                            ..
                        }
                    )
                })
                .count(),
        );
        let report = state.tick(&decision.commands);
        assert!(report.events.iter().all(|event| !matches!(
            event,
            Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )));
        if warden_orders > 0 {
            break;
        }
    }

    assert_eq!(
        warden_orders,
        1,
        "completed income never funded the selected Warden: tick={}, bank={}, result={:?}, profile={:?}, queues={:?}",
        state.current_tick(),
        state.player(PlayerId(0)).scrap,
        state.result(),
        brain.profile(),
        state
            .buildings()
            .iter()
            .filter(|building| building.player == PlayerId(0) && !building.queue.is_empty())
            .map(|building| (building.kind, building.queue.clone()))
            .collect::<Vec<_>>()
    );
    assert!(
        state.current_tick() > started_at,
        "the higher-tier purchase must follow real authoritative income"
    );
    assert_eq!(
        state
            .buildings()
            .iter()
            .filter(|building| building.player == PlayerId(0))
            .flat_map(|building| building.queue.iter())
            .filter(|kind| **kind == UnitKind::Warden)
            .count(),
        1,
        "the accumulated current bank must enqueue the better provider exactly once"
    );
}

#[test]
fn current_air_pressure_spends_while_developmental_ground_force_is_accumulating() {
    let control =
        mature_standing_accumulation_scenario(UnitKind::Flakhound.stats().cost, true, false);
    let (control_state, mut control_brain) = prepared_mature_standing_force(&control);
    let held = control_brain.act_traced(&control_state);
    assert!(
        held.commands
            .iter()
            .all(|command| !matches!(command.command, Command::Train { .. }))
    );

    let scenario =
        mature_standing_accumulation_scenario(UnitKind::Flakhound.stats().cost, true, true);
    let (mut state, mut brain) = prepared_mature_standing_force(&scenario);
    let observation = Observation::fog_honest(&state, PlayerId(0));
    assert!(
        observation
            .enemy_units
            .iter()
            .any(|unit| unit.kind == UnitKind::Moth)
    );

    let decision = brain.act_traced(&state);
    let trace = decision
        .trace
        .as_ref()
        .expect("the counter decision is traced");
    let counter = trace
        .allocation
        .proposals
        .entries
        .iter()
        .find(|proposal| {
            proposal.disposition == ProposalDispositionTrace::Accepted
                && matches!(
                    proposal.key,
                    ProposalKeyTrace::StandingForce {
                        kind: UnitKind::Flakhound,
                        ..
                    }
                )
        })
        .unwrap_or_else(|| {
            panic!(
                "current air pressure must remain eligible beside the held ground need: {:?}",
                trace.allocation.proposals.entries
            )
        });
    assert_eq!(counter.claims.minimum_residual_scrap, 0);
    assert_eq!(
        decision
            .commands
            .iter()
            .filter(|command| matches!(
                command.command,
                Command::Train {
                    kind: UnitKind::Flakhound,
                    ..
                }
            ))
            .count(),
        1,
    );
    assert!(decision.commands.iter().all(|command| !matches!(
        command.command,
        Command::Train {
            kind: UnitKind::Sentinel,
            ..
        }
    )));

    let report = state.tick(&decision.commands);
    assert!(report.events.iter().all(|event| !matches!(
        event,
        Event::CommandRejected {
            player: PlayerId(0),
            ..
        }
    )));
}

#[test]
fn connected_package_refills_one_lane_until_an_oversized_roster_freezes() {
    let scenario = oversized_connected_package_scenario();
    let strike_kind = Role::AirGround.unit_for(scenario.players[0].faction);
    let briefing = public_map(&scenario);
    let config = scenario.players[0].bot_config.expect("bot is configured");
    let mut state = scenario
        .build()
        .expect("oversized connected-package scenario builds");
    let airworks = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(0) && building.kind == BuildingKind::Airworks)
        .expect("fixture has exactly one Airworks")
        .id;
    let foundry = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(0) && building.kind == BuildingKind::Foundry)
        .expect("fixture has exactly one own Foundry")
        .id;
    let fabricator = state
        .buildings()
        .iter()
        .find(|building| {
            building.player == PlayerId(0) && building.kind == BuildingKind::Fabricator
        })
        .expect("fixture has exactly one Fabricator")
        .id;
    let prefill: Vec<_> = (0..QUEUE_CAP)
        .flat_map(|_| {
            [
                oxide_sim::PlayerCommand {
                    player: PlayerId(0),
                    command: Command::Train {
                        building: foundry,
                        kind: UnitKind::Excavator,
                    },
                },
                oxide_sim::PlayerCommand {
                    player: PlayerId(0),
                    command: Command::Train {
                        building: fabricator,
                        kind: UnitKind::Tender,
                    },
                },
            ]
        })
        .collect();
    let prefill_report = state.tick(&prefill);
    assert!(prefill_report.events.iter().all(|event| {
        !matches!(
            event,
            Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )
    }));
    let mut brain = Brain::scripted(PlayerId(0), config, briefing);

    let mut fixed_deadline = None;
    let mut fixed_target_value = None;
    let mut largest_requested_strike = 0usize;
    let mut strike_orders = 0usize;
    let mut strike_completions = 0usize;
    let mut refills_after_full = 0usize;
    let mut saw_full_queue = false;
    let mut frozen_strike = None;
    let mut premature_hits = Vec::new();
    let mut precommit_hold_batches = 0usize;
    let mut precommit_held = BTreeSet::new();
    let mut freeze_observation_tick = None;
    let mut last_strike_completion_tick = None;
    let home = TilePos::new(3, 17);

    for _ in 0..3_000 {
        let queue_len = state
            .building(airworks)
            .expect("Airworks remains present")
            .queue
            .len();
        assert!(
            queue_len <= QUEUE_CAP,
            "the live queue exceeded its rule cap"
        );
        saw_full_queue |= queue_len == QUEUE_CAP;

        let decision = brain.act_traced(&state);
        let preparing_connected = decision.trace.as_ref().is_some_and(|trace| {
            trace.connected_force.status == ConnectedForceStatus::Active
                && trace.connected_force.package.is_some()
                && !trace.connected_force.assigned.membership_frozen
        });
        if let Some(trace) = &decision.trace
            && trace.connected_force.status == ConnectedForceStatus::Active
            && let Some(package) = &trace.connected_force.package
        {
            let strike = package
                .demands
                .strike
                .iter()
                .map(|demand| usize::try_from(demand.count).expect("provider count fits usize"))
                .sum::<usize>();
            let deadline = *fixed_deadline.get_or_insert(package.preparation_deadline);
            assert_eq!(
                package.preparation_deadline, deadline,
                "queue refills must not extend the fixed preparation horizon"
            );
            let target_value = *fixed_target_value.get_or_insert(package.target_value);
            assert_eq!(
                package.target_value, target_value,
                "the held preparation force must leave the stable target cluster untouched"
            );
            largest_requested_strike = largest_requested_strike.max(strike);
            if trace.connected_force.assigned.membership_frozen {
                freeze_observation_tick = Some(state.current_tick());
                frozen_strike = Some((strike, trace.connected_force.assigned.strike.len()));
            }
        }

        let ordered_now = decision
            .commands
            .iter()
            .filter(|command| {
                matches!(
                    command.command,
                    Command::Train { building, kind }
                        if building == airworks && kind == strike_kind
                )
            })
            .count();
        if saw_full_queue && queue_len < QUEUE_CAP {
            refills_after_full = refills_after_full.saturating_add(ordered_now);
        }
        strike_orders = strike_orders.saturating_add(ordered_now);
        for command in &decision.commands {
            if preparing_connected
                && let Command::Move {
                    units,
                    goal,
                    queue: false,
                } = &command.command
                && goal.chebyshev(home) <= 6
            {
                let held: Vec<_> = units
                    .iter()
                    .copied()
                    .filter(|id| state.unit(*id).is_some_and(|unit| unit.kind == strike_kind))
                    .collect();
                if !held.is_empty() {
                    precommit_hold_batches = precommit_hold_batches.saturating_add(1);
                    precommit_held.extend(held);
                }
            }
        }

        let report = state.tick(&decision.commands);
        for event in &report.events {
            if let Event::AttackHit {
                attacker_kind,
                target: Target::Building(_),
                ..
            } = event
            {
                premature_hits.push((report.tick, *attacker_kind));
            }
        }
        assert!(report.events.iter().all(|event| {
            !matches!(
                event,
                Event::CommandRejected {
                    player: PlayerId(0),
                    ..
                }
            )
        }));
        let completed_now = report
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    Event::UnitTrained {
                        building,
                        kind,
                        player: PlayerId(0),
                        ..
                    } if *building == airworks && *kind == strike_kind
                )
            })
            .count();
        if completed_now > 0 {
            last_strike_completion_tick = Some(report.tick);
        }
        strike_completions = strike_completions.saturating_add(completed_now);

        if frozen_strike.is_some() {
            break;
        }
    }

    let (requested, assigned) =
        frozen_strike.expect("the connected package naturally reached its exact-id freeze");
    assert!(
        requested >= QUEUE_CAP + 2,
        "the fixture must require repeated refills beyond one live queue: requested={requested}"
    );
    assert!(
        saw_full_queue,
        "the operation never filled its only Airworks"
    );
    assert_eq!(
        strike_orders, largest_requested_strike,
        "the operation purchased outside the largest roster justified before commitment"
    );
    assert!(
        strike_completions >= requested && strike_completions <= strike_orders,
        "exact membership cannot freeze before every queued provider becomes live"
    );
    assert!(
        refills_after_full >= requested - QUEUE_CAP,
        "the operation did not refill each slot needed beyond the initial live queue"
    );
    assert!(
        precommit_hold_batches >= 2 && precommit_held.len() >= QUEUE_CAP,
        "incrementally trained strike aircraft were not repeatedly held near home before commitment: batches={precommit_hold_batches}, held={precommit_held:?}"
    );
    assert_eq!(
        assigned, requested,
        "the naturally assembled operation froze a partial or oversized roster"
    );
    assert!(
        largest_requested_strike >= requested,
        "pre-commit revisions may narrow but must not invent providers at freeze"
    );
    assert!(
        premature_hits.is_empty(),
        "preparation units attacked the target before exact-roster freeze: {premature_hits:?}"
    );
    let deadline = fixed_deadline.expect("an admitted package fixes a deadline");
    assert!(
        last_strike_completion_tick.is_some_and(|tick| tick < deadline),
        "the last required provider did not complete before the deadline observation: completed at {last_strike_completion_tick:?}, deadline {deadline}"
    );
    assert!(
        freeze_observation_tick.is_some_and(|tick| tick <= deadline),
        "the complete exact roster did not freeze by its deadline observation: froze at {freeze_observation_tick:?}, deadline {deadline}"
    );
}

#[test]
fn connected_package_scales_with_economy_throughput_and_target_value() {
    let admitted = |scenario: &Scenario| {
        let state = scenario.build().expect("connected-package scenario builds");
        let mut brain = Brain::scripted(
            PlayerId(0),
            scenario.players[0].bot_config.expect("bot is configured"),
            public_map(scenario),
        );
        let decision = brain.act_traced(&state);
        let trace = decision.trace.expect("the admission decision is traced");
        assert_eq!(
            trace.connected_force.status,
            ConnectedForceStatus::Active,
            "connected operation was not admitted: {trace:#?}"
        );
        (
            trace.resources,
            trace
                .connected_force
                .package
                .expect("the current target admits a connected package"),
        )
    };

    let (ordinary_resources, ordinary) = admitted(&connected_package_scenario(false));
    let (rich_resources, rich) = admitted(&connected_package_scenario(true));

    assert!(rich.current_scrap > ordinary.current_scrap);
    assert!(rich.forecast_scrap > ordinary.forecast_scrap);
    assert!(rich_resources.completed_producers > ordinary_resources.completed_producers);
    assert!(rich.target_value > ordinary.target_value);
    assert!(rich.observed_aa_firepower > ordinary.observed_aa_firepower);
    assert!(rich.chosen_capability.suppression >= ordinary.chosen_capability.suppression);
    assert!(rich.chosen_capability.strike >= ordinary.chosen_capability.strike);
    assert!(
        rich.chosen_capability.suppression > ordinary.chosen_capability.suppression
            || rich.chosen_capability.strike > ordinary.chosen_capability.strike,
        "the richer production base and more valuable defended target did not scale the useful force: ordinary={ordinary:?}, rich={rich:?}"
    );
}

#[test]
fn dense_connected_target_trains_and_freezes_the_selected_bomber_roster() {
    let mut scenario = connected_package_scenario(true);
    scenario.name = "Dense connected-package bombing fixture".to_owned();
    scenario.units.extend(
        [
            (27, 9),
            (28, 9),
            (29, 9),
            (30, 9),
            (31, 9),
            (32, 9),
            (33, 9),
            (34, 9),
            (34, 10),
            (34, 11),
            (34, 12),
            (34, 13),
            (33, 14),
            (32, 14),
            (31, 14),
            (30, 14),
            (29, 14),
            (28, 14),
        ]
        .into_iter()
        .map(|(x, y)| UnitSpec {
            player: 1,
            kind: UnitKind::Lancer,
            x,
            y,
        }),
    );
    let bomber = Role::Bomber.unit_for(scenario.players[0].faction);
    let mut state = scenario.build().expect("dense bombing fixture builds");
    let mut brain = Brain::scripted(
        PlayerId(0),
        scenario.players[0].bot_config.expect("bot is configured"),
        public_map(&scenario),
    );
    let mut ordered_bomber = false;
    let mut frozen_roster = None;

    for _ in 0..5_000 {
        let decision = brain.act_traced(&state);
        ordered_bomber |= decision.commands.iter().any(|command| {
            matches!(
                command.command,
                Command::Train { kind, .. } if kind == bomber
            )
        });

        if let Some(trace) = &decision.trace
            && let Some(package) = &trace.connected_force.package
            && trace.connected_force.assigned.membership_frozen
        {
            let expected = package
                .demands
                .strike
                .iter()
                .map(|demand| {
                    (
                        demand.kind,
                        usize::try_from(demand.count).expect("small demand"),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            let mut assigned = BTreeMap::new();
            for id in &trace.connected_force.assigned.strike {
                let kind = state.unit(*id).expect("frozen member is live").kind;
                *assigned.entry(kind).or_insert(0_usize) += 1;
            }
            frozen_roster = Some((expected, assigned, package.chosen_bombing));
            break;
        }

        let report = state.tick(&decision.commands);
        assert!(report.events.iter().all(|event| {
            !matches!(
                event,
                Event::CommandRejected {
                    player: PlayerId(0),
                    ..
                }
            )
        }));
    }

    assert!(
        ordered_bomber,
        "the package scheduler never emitted the selected bomber purchase"
    );
    let (expected, assigned, chosen_bombing) =
        frozen_roster.expect("the selected connected package reached exact-id commitment");
    assert!(
        chosen_bombing > 0,
        "the frozen roster must own bombing work"
    );
    assert!(
        expected.get(&bomber).is_some_and(|count| *count > 0),
        "the dense current target must select a newly produced bomber: {expected:?}"
    );
    assert_eq!(
        assigned, expected,
        "the committed exact ids must match the selected mixed strike package"
    );
}

#[test]
fn connected_package_assembly_keeps_one_deadline_and_aborts_when_no_longer_feasible() {
    let mut scenario = connected_package_scenario(false);
    scenario.map = connected_package_deadline_map();
    scenario.units.retain(|unit| unit.kind != UnitKind::Bombard);
    scenario.units.push(UnitSpec {
        player: 0,
        kind: UnitKind::Sentinel,
        x: 8,
        y: 21,
    });
    let mut state = scenario.build().expect("connected-package scenario builds");
    let mut brain = Brain::scripted(
        PlayerId(0),
        scenario.players[0].bot_config.expect("bot is configured"),
        public_map(&scenario),
    );
    let mut fixed_deadline = None;
    let mut infeasible_at = None;
    let mut aborted_at = None;
    let mut observed_statuses = Vec::new();
    let mut cancelled_provider = false;
    let mut last_trace = None;
    let mut last_provider_cancellation = None;
    let mut recovery_predecessor = None;

    for _ in 0..3_000 {
        let mut decision = brain.act_traced(&state);
        if let Some(trace) = &decision.trace {
            if observed_statuses
                .last()
                .is_none_or(|(_, status)| *status != trace.connected_force.status)
            {
                observed_statuses.push((state.current_tick(), trace.connected_force.status));
            }
            if let Some(package) = &trace.connected_force.package {
                if let Some(deadline) = fixed_deadline {
                    assert_eq!(
                        package.preparation_deadline, deadline,
                        "repeated package derivation extended the preparation horizon"
                    );
                } else {
                    fixed_deadline = Some(package.preparation_deadline);
                }
            }
            if trace.connected_force.status
                == ConnectedForceStatus::Recovering(
                    ConnectedRecoveryReasonTrace::PreparationInfeasible,
                )
                && infeasible_at.is_none()
            {
                infeasible_at = Some(state.current_tick());
                recovery_predecessor = Some((last_trace, last_provider_cancellation));
            }
            if trace.connected_force.status == ConnectedForceStatus::Aborted {
                aborted_at = Some(state.current_tick());
                break;
            }
            last_trace = Some((state.current_tick(), trace.connected_force.status));
        }

        let cancellations: Vec<_> = decision
            .commands
            .iter()
            .filter_map(|command| match command.command {
                Command::Train { building, .. } => Some(oxide_sim::PlayerCommand {
                    player: command.player,
                    command: Command::CancelTrain { building, index: 0 },
                }),
                _ => None,
            })
            .collect();
        cancelled_provider |= !cancellations.is_empty();
        if !cancellations.is_empty() {
            last_provider_cancellation = Some(state.current_tick());
        }
        decision.commands.extend(cancellations);
        let report = state.tick(&decision.commands);
        assert!(report.events.iter().all(|event| {
            !matches!(
                event,
                Event::CommandRejected {
                    player: PlayerId(0),
                    ..
                }
            )
        }));
    }

    let deadline = fixed_deadline.expect("the connected package was admitted");
    assert!(
        cancelled_provider,
        "the fixture must cancel at least one accepted provider purchase"
    );
    let infeasible_at = infeasible_at.expect(
        "cancelling an accepted provider makes the exact package infeasible inside its original window",
    );
    assert!(
        infeasible_at <= deadline,
        "the fixed window must close no later than its deadline: deadline={deadline}, observed={observed_statuses:?}"
    );
    assert!(
        recovery_predecessor.is_some(),
        "the first infeasible observation must retain its preceding causal evidence"
    );
    let (preceding_trace, cancellation_at) = recovery_predecessor.expect("checked above");
    let (preceding_tick, preceding_status) =
        preceding_trace.expect("recovery follows a traced active operation");
    let cancellation_at =
        cancellation_at.expect("recovery follows an externally cancelled provider purchase");
    assert_eq!(
        preceding_status,
        ConnectedForceStatus::Active,
        "the operation must be healthy immediately before the cancellation is observed"
    );
    assert_eq!(
        preceding_tick, cancellation_at,
        "the immediately preceding decision must be the one whose provider purchase was cancelled"
    );
    assert!(
        cancellation_at < infeasible_at,
        "recovery must be a response to authoritative cancellation, not a same-think prediction"
    );
    let aborted_at = aborted_at.expect("the infeasible operation reaches its terminal abort");
    assert!(
        aborted_at > infeasible_at && aborted_at <= infeasible_at.saturating_add(500),
        "recovery must release the infeasible cohort inside its bounded return window: {observed_statuses:?}"
    );
}

#[test]
fn connected_package_rejection_reports_its_fixed_preparation_deadline() {
    let mut scenario = connected_package_scenario(true);
    scenario.name = "Blocked connected-package production window".to_owned();
    scenario.players[0].scrap = 10_000;
    scenario.units.retain(|unit| unit.kind != UnitKind::Bombard);
    scenario.units.push(UnitSpec {
        player: 0,
        kind: UnitKind::Sentinel,
        x: 8,
        y: 21,
    });
    let mut retained_crucible = false;
    scenario.buildings.retain(|building| {
        if building.player != 0 {
            return true;
        }
        match building.kind {
            BuildingKind::Fabricator => false,
            BuildingKind::Crucible if retained_crucible => false,
            BuildingKind::Crucible => {
                retained_crucible = true;
                true
            }
            _ => true,
        }
    });

    let mut reference_state = scenario
        .build()
        .expect("reference connected-package scenario builds");
    while reference_state.current_tick() < 24 {
        reference_state.tick(&[]);
    }
    let mut reference_brain = Brain::scripted(
        PlayerId(0),
        scenario.players[0].bot_config.expect("bot is configured"),
        public_map(&scenario),
    );
    let reference = reference_brain.act_traced(&reference_state);
    let fixed_deadline = reference
        .trace
        .as_ref()
        .and_then(|trace| trace.connected_force.package.as_ref())
        .expect("an open production lane admits the reference package")
        .preparation_deadline;

    let mut blocked_state = scenario
        .build()
        .expect("blocked connected-package scenario builds");
    let crucible = blocked_state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(0) && building.kind == BuildingKind::Crucible)
        .expect("fixture retains one Crucible")
        .id;
    let fill_queue: Vec<_> = (0..QUEUE_CAP)
        .map(|_| oxide_sim::PlayerCommand {
            player: PlayerId(0),
            command: Command::Train {
                building: crucible,
                kind: UnitKind::Breaker,
            },
        })
        .collect();
    let starting_scrap = blocked_state.player(PlayerId(0)).scrap;
    let report = blocked_state.tick(&fill_queue);
    assert!(
        report.events.iter().all(|event| {
            !matches!(
                event,
                Event::CommandRejected {
                    player: PlayerId(0),
                    ..
                }
            )
        }),
        "the blocking queue must be legal with {starting_scrap} scrap: {:?}",
        report.events,
    );
    while blocked_state.current_tick() < 24 {
        blocked_state.tick(&[]);
    }

    let mut blocked_brain = Brain::scripted(
        PlayerId(0),
        scenario.players[0].bot_config.expect("bot is configured"),
        public_map(&scenario),
    );
    let blocked = blocked_brain.act_traced(&blocked_state);
    let trace = blocked
        .trace
        .expect("the blocked admission think is traced");
    let rejected = trace
        .connected_force
        .rejected_candidate
        .expect("the fixed production window rejects the connected package");
    assert!(
        matches!(
            rejected.reason,
            ConnectedRejectionReasonTrace::PreparationWindowTooShort {
                observed_at: 24,
                deadline,
                ..
            } if deadline == fixed_deadline
        ),
        "unexpected rejection for the fixed production window: {rejected:#?}"
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
    bombers.sort_unstable();
    let mut brain = Brain::scripted(
        PlayerId(0),
        scenario.players[0].bot_config.expect("bot is configured"),
        public_map(&scenario),
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
                if *target == flak && units.iter().any(|unit| artillery.contains(unit)) {
                    first_suppression.get_or_insert(tick);
                }
                if *target == objective && units.iter().any(|unit| bombers.contains(unit)) {
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
            player: 0,
            kind: UnitKind::Sentinel,
            x: 8,
            y: 12,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 8,
            y: 13,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 8,
            y: 14,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 8,
            y: 15,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 8,
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
        scenario.players[0].bot_config.expect("bot is configured"),
        public_map(&scenario),
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
        8,
        "the exact relief group must leave the rest of Prime's opening core at home"
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
    scenario.units.extend((0..20).map(|index| UnitSpec {
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
        scenario.players[0].bot_config.unwrap(),
        public_map(&scenario),
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
        "the wealthy lift fills three carriers and leaves Prime's eight-unit home floor"
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

fn connected_package_deadline_map() -> Vec<String> {
    let mut rows: Vec<Vec<_>> = open_air_operation_map()
        .into_iter()
        .map(|row| row.chars().collect())
        .collect();
    rows[19][2..=9].fill('#');
    rows[22][2..=9].fill('#');
    for row in &mut rows[20..=21] {
        row[2] = '#';
        row[9] = '#';
    }
    rows.into_iter()
        .map(|row| row.into_iter().collect())
        .collect()
}

fn connected_package_scenario(rich: bool) -> Scenario {
    let mut scenario = Scenario::skirmish();
    scenario.name = if rich {
        "Rich connected-package fixture"
    } else {
        "Ordinary connected-package fixture"
    }
    .to_owned();
    scenario.seed = 0x0A17_0001 + u64::from(rich);
    scenario.map = open_air_operation_map();
    scenario.meta = None;
    scenario.players[0].scrap = if rich { 6_000 } else { 600 };
    scenario.players[0].bot = true;
    scenario.players[0].bot_config = Some(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Balanced,
        71,
    ));
    scenario.players[1].scrap = 0;
    scenario.players[1].bot = false;
    scenario.players[1].bot_config = None;
    scenario.buildings = vec![
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Foundry,
            x: 3,
            y: 14,
        },
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
            kind: BuildingKind::Array,
            x: 24,
            y: 14,
        },
    ];
    if rich {
        scenario.buildings.extend([
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Fabricator,
                x: 3,
                y: 6,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Airworks,
                x: 7,
                y: 6,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Crucible,
                x: 11,
                y: 3,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Crucible,
                x: 11,
                y: 6,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Reclaimer,
                x: 15,
                y: 3,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Reclaimer,
                x: 17,
                y: 3,
            },
        ]);
    }
    scenario.buildings.extend(if rich {
        vec![
            BuildingSpec {
                player: 1,
                kind: BuildingKind::FlakTurret,
                x: 29,
                y: 10,
            },
            BuildingSpec {
                player: 1,
                kind: BuildingKind::FlakTurret,
                x: 29,
                y: 12,
            },
            BuildingSpec {
                player: 1,
                kind: BuildingKind::Crucible,
                x: 30,
                y: 10,
            },
            BuildingSpec {
                player: 1,
                kind: BuildingKind::Airworks,
                x: 32,
                y: 10,
            },
            BuildingSpec {
                player: 1,
                kind: BuildingKind::Fabricator,
                x: 30,
                y: 12,
            },
            BuildingSpec {
                player: 1,
                kind: BuildingKind::Reclaimer,
                x: 32,
                y: 12,
            },
        ]
    } else {
        vec![BuildingSpec {
            player: 1,
            kind: BuildingKind::Foundry,
            x: 30,
            y: 10,
        }]
    });
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
    ];
    scenario.units.extend((0..11).map(|index| UnitSpec {
        player: 0,
        kind: UnitKind::Sentinel,
        x: 3 + index % 6,
        y: 20 + index / 6,
    }));
    scenario
}

fn foundry_connected_allocation_scenario(scrap: u32) -> Scenario {
    let mut scenario = connected_package_scenario(false);
    scenario.name = "Shipped cross-domain allocation".to_owned();
    scenario.seed = 1_616_305;
    scenario.players[0].scrap = scrap;
    scenario.players[0].bot = true;
    scenario.players[0].bot_config = Some(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Aggressive,
        u64::MAX,
    ));
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Crucible,
        x: 11,
        y: 3,
    });
    let frame = TilePos::new(18, 16);
    let row = scenario
        .map
        .get_mut(frame.y as usize)
        .expect("the fixture contains the expansion row");
    let mut bytes = row.as_bytes().to_vec();
    bytes[frame.x as usize] = b'E';
    *row = String::from_utf8(bytes).expect("the fixture map remains ASCII");
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Extractor,
        x: frame.x,
        y: frame.y,
    });
    scenario.units.extend([
        UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: 6,
            y: 18,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: 7,
            y: 18,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: 8,
            y: 18,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Condor,
            x: 10,
            y: 18,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Condor,
            x: 11,
            y: 18,
        },
    ]);
    scenario
}

fn full_tech_standing_scenario() -> Scenario {
    let mut scenario = Scenario::skirmish();
    scenario.name = "Full-tech standing-force fixture".to_owned();
    scenario.seed = 0x0A17_0004;
    scenario.map = open_air_operation_map();
    scenario.meta = None;
    scenario.players[0].scrap = 4_000;
    scenario.players[0].bot = true;
    scenario.players[0].bot_config = Some(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Aggressive,
        5,
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
    ];
    scenario.units = (0..4)
        .map(|index| UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: 8 + index,
            y: 18,
        })
        .chain((0..11).map(|index| UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 3 + index % 6,
            y: 20 + index / 6,
        }))
        .collect();
    scenario
}

fn mature_standing_force_scenario(
    scrap: u32,
    recurring_income: bool,
    current_air_threat: bool,
) -> Scenario {
    let mut rows = vec![vec!['.'; 80]; 24];
    rows.first_mut().expect("map has a north edge").fill('#');
    rows.last_mut().expect("map has a south edge").fill('#');
    for row in &mut rows {
        row[0] = '#';
        row[79] = '#';
    }
    rows[17][3] = '1';
    rows[17][74] = '2';
    let extractor_frames = [
        TilePos::new(3, 8),
        TilePos::new(6, 8),
        TilePos::new(9, 8),
        TilePos::new(3, 11),
        TilePos::new(6, 11),
        TilePos::new(9, 11),
        TilePos::new(3, 14),
        TilePos::new(6, 14),
        TilePos::new(9, 14),
    ];
    if recurring_income {
        for frame in extractor_frames {
            rows[frame.y as usize][frame.x as usize] = 'E';
        }
    }

    let mut scenario = Scenario::skirmish();
    scenario.name = "Mature standing-force integration fixture".to_owned();
    scenario.seed = 0x0A17_0005 + u64::from(recurring_income) + u64::from(current_air_threat) * 2;
    scenario.map = rows
        .into_iter()
        .map(|row| row.into_iter().collect())
        .collect();
    scenario.meta = None;
    scenario.players[0].scrap = scrap;
    scenario.players[0].bot = true;
    scenario.players[0].bot_config = Some(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Turtle,
        0,
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
            kind: BuildingKind::Crucible,
            x: 15,
            y: 3,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Array,
            x: 20,
            y: 3,
        },
    ];
    if recurring_income {
        scenario
            .buildings
            .extend(extractor_frames.map(|frame| BuildingSpec {
                player: 0,
                kind: BuildingKind::Extractor,
                x: frame.x,
                y: frame.y,
            }));
    }

    scenario.units = (0..6)
        .map(|index| UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: 3 + index,
            y: 20,
        })
        .chain((0..11).map(|index| UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 16 + index,
            y: 20,
        }))
        .chain((0..2).map(|index| UnitSpec {
            player: 0,
            kind: UnitKind::Scuttler,
            x: 10 + index,
            y: 21,
        }))
        .chain((0..2).map(|index| UnitSpec {
            player: 0,
            kind: UnitKind::Excavator,
            x: 12 + index,
            y: 21,
        }))
        .collect();
    if current_air_threat {
        scenario.units.push(UnitSpec {
            player: 1,
            kind: UnitKind::Moth,
            x: 11,
            y: 19,
        });
    }
    scenario
}

fn mature_standing_accumulation_scenario(
    scrap: u32,
    recurring_income: bool,
    current_air_threat: bool,
) -> Scenario {
    let mut scenario = mature_standing_force_scenario(scrap, recurring_income, current_air_threat);
    scenario.units.extend((0..4).map(|index| UnitSpec {
        player: 0,
        kind: UnitKind::Lancer,
        x: 28 + index,
        y: 20,
    }));
    scenario.units.push(UnitSpec {
        player: 0,
        kind: UnitKind::Bombard,
        x: 34,
        y: 20,
    });
    scenario.units.extend((0..5).map(|index| UnitSpec {
        player: 0,
        kind: UnitKind::Buzzard,
        x: 35 + index,
        y: 20,
    }));
    scenario
}

fn prepared_mature_standing_force(scenario: &Scenario) -> (oxide_sim::State, Brain) {
    let mut state = scenario
        .build()
        .expect("the mature standing-force fixture builds");
    let builders = state
        .units()
        .iter()
        .filter(|unit| {
            unit.player == PlayerId(0)
                && matches!(unit.kind, UnitKind::Harvester | UnitKind::Excavator)
        })
        .map(|unit| unit.id)
        .collect::<Vec<_>>();
    let report = state.tick(&[oxide_sim::PlayerCommand {
        player: PlayerId(0),
        command: Command::Patrol {
            units: builders.clone(),
            waypoints: vec![TilePos::new(4, 19), TilePos::new(9, 19)],
        },
    }]);
    assert!(report.events.iter().all(|event| !matches!(
        event,
        Event::CommandRejected {
            player: PlayerId(0),
            ..
        }
    )));
    let brain = Brain::scripted(
        PlayerId(0),
        scenario.players[0].bot_config.expect("bot is configured"),
        public_map(scenario),
    );
    while !state.current_tick().is_multiple_of(brain.dials().cadence) {
        state.tick(&[]);
    }
    let observation = Observation::fog_honest(&state, PlayerId(0));
    assert_eq!(observation.scrap, scenario.players[0].scrap);
    assert!(
        builders
            .iter()
            .all(|unit| observation.my_queued_units.contains(unit))
    );
    (state, brain)
}

fn oversized_connected_package_scenario() -> Scenario {
    let mut scenario = connected_package_scenario(false);
    scenario.name = "Oversized connected-package fixture".to_owned();
    scenario.seed = 0x0A17_0003;
    scenario.players[0].scrap = 100_000;
    scenario.retint_seat(0, Faction::Cupric);
    let mut map: Vec<Vec<_>> = scenario
        .map
        .iter()
        .map(|row| row.chars().collect())
        .collect();
    map[19][2..=9].fill('#');
    map[22][2..=9].fill('#');
    for row in &mut map[20..=21] {
        row[2] = '#';
        row[9] = '#';
    }
    scenario.map = map
        .into_iter()
        .map(|row| row.into_iter().collect())
        .collect();
    scenario
        .buildings
        .retain(|building| building.player == 0 && building.kind != BuildingKind::Foundry);
    let forward_array = scenario
        .buildings
        .iter_mut()
        .find(|building| building.kind == BuildingKind::Array)
        .expect("connected-package fixture includes an Array");
    forward_array.x = 30;
    forward_array.y = 12;
    let bombard = scenario
        .units
        .iter_mut()
        .find(|unit| unit.kind == UnitKind::Bombard)
        .expect("connected-package fixture includes a Bombard");
    bombard.x = 10;
    bombard.y = 9;
    scenario.buildings.push(BuildingSpec {
        player: 1,
        kind: BuildingKind::Crucible,
        x: 30,
        y: 10,
    });
    scenario.buildings.extend(
        [(26, 10), (34, 10), (30, 6), (30, 14)].map(|(x, y)| BuildingSpec {
            player: 1,
            kind: BuildingKind::Foundry,
            x,
            y,
        }),
    );
    scenario
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
