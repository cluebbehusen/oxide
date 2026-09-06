//! Composed fog-history, exact ownership, and deterministic adaptation checks.

use chassis::grid::TilePos;
use oxide_sim::bot::{Brain, Observation, PublicMapBriefing};
use oxide_sim::scenario::{BotConfig, BotDifficulty, BotStance, PlayerSpec, UnitSpec};
use oxide_sim::{Command, Event, Faction, PlayerCommand, PlayerId, Scenario, UnitKind};
use std::collections::BTreeSet;
use std::sync::Arc;

fn arena() -> Scenario {
    let mut map = vec![vec![b'.'; 64]; 32];
    for row in &mut map {
        row[0] = b'#';
        row[63] = b'#';
    }
    map[0].fill(b'#');
    map[31].fill(b'#');
    map[3][3] = b'1';
    map[25][58] = b'2';
    map[12][4] = b'S';
    let mut units: Vec<_> = (0..12)
        .map(|index| UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 7 + index % 4,
            y: 5 + index / 4,
        })
        .collect();
    units.extend((0..3).map(|index| UnitSpec {
        player: 0,
        kind: UnitKind::Harvester,
        x: 4 + index,
        y: 10,
    }));
    units.push(UnitSpec {
        player: 1,
        kind: UnitKind::Harvester,
        x: 56,
        y: 25,
    });
    Scenario {
        name: "battlefield-adaptation".into(),
        seed: 981,
        map: map
            .into_iter()
            .map(|row| String::from_utf8(row).unwrap())
            .collect(),
        players: (0..2)
            .map(|seat| PlayerSpec {
                name: format!("Seat {seat}"),
                faction: Faction::Ferrous,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            })
            .collect(),
        units,
        buildings: Vec::new(),
        meta: None,
    }
}

fn brain(scenario: &Scenario, difficulty: BotDifficulty) -> Brain {
    Brain::scripted(
        PlayerId(0),
        BotConfig::scripted(difficulty, BotStance::Balanced, 9000),
        Arc::new(PublicMapBriefing::from_scenario(scenario).unwrap()),
    )
}

#[test]
fn complete_hidden_state_histories_and_tracing_produce_identical_commands() {
    for difficulty in [
        BotDifficulty::Scrapheap,
        BotDifficulty::Standard,
        BotDifficulty::Veteran,
        BotDifficulty::Prime,
    ] {
        let mut left = arena();
        for row in &mut left.map {
            let mut tiles = row.as_bytes().to_vec();
            tiles[32] = b'#';
            *row = String::from_utf8(tiles).unwrap();
        }
        let mut right = left.clone();
        right.units.last_mut().unwrap().x = 60;
        let mut left_state = left.build().unwrap();
        let mut right_state = right.build().unwrap();
        let mut traced = brain(&left, difficulty);
        let mut plain = brain(&right, difficulty);
        for tick in 0..600 {
            assert_eq!(
                Observation::fog_honest(&left_state, PlayerId(0)),
                Observation::fog_honest(&right_state, PlayerId(0)),
                "{difficulty:?} at {tick}"
            );
            let act = traced.act_traced(&left_state);
            let commands = plain.act(&right_state);
            assert_eq!(act.commands, commands, "{difficulty:?} at {tick}");
            left_state.tick(&commands);
            right_state.tick(&commands);
        }
    }
}

#[test]
fn observed_displacement_changes_history_without_projecting_a_hidden_trajectory() {
    let mut scenario = arena();
    scenario.units.push(UnitSpec {
        player: 0,
        kind: UnitKind::Kestrel,
        x: 19,
        y: 5,
    });
    scenario.units.push(UnitSpec {
        player: 1,
        kind: UnitKind::Sentinel,
        x: 21,
        y: 5,
    });
    let mut state = scenario.build().unwrap();
    let enemy = state
        .units()
        .iter()
        .find(|unit| unit.player == PlayerId(1) && unit.kind == UnitKind::Sentinel)
        .unwrap()
        .id;
    let mut brain = brain(&scenario, BotDifficulty::Prime);
    let mut saw_movement = false;
    let mut last_direct = None;
    let mut checked_loss = false;
    for tick in 0..1200 {
        let act = brain.act_traced(&state);
        if let Some(assessment) = act
            .trace
            .as_ref()
            .and_then(|trace| trace.battlefield.as_ref())
        {
            let observed = Observation::fog_honest(&state, PlayerId(0));
            if let Some(unit) = observed.enemy_units.iter().find(|unit| unit.id == enemy) {
                last_direct = Some((unit.tile, tick));
            }
            if let Some((_, from, _, to, seen)) =
                assessment.motion.iter().find(|(id, ..)| *id == enemy)
            {
                saw_movement |= from != to;
                if !observed.enemy_units.iter().any(|unit| unit.id == enemy) {
                    assert_eq!(Some((*to, *seen)), last_direct);
                    checked_loss = true;
                }
            }
        }
        let commands = if tick == 0 {
            vec![PlayerCommand {
                player: PlayerId(1),
                command: Command::Move {
                    units: vec![enemy],
                    goal: TilePos::new(50, 5),
                    queue: false,
                },
            }]
        } else {
            Vec::new()
        };
        state.tick(&commands);
    }
    assert!(saw_movement, "the live contact must actually move");
    assert!(checked_loss, "the contact must actually leave vision");
}

#[test]
fn replaying_the_command_prefix_reconstructs_the_adaptive_command_suffix() {
    let mut scenario = arena();
    for y in 4..8 {
        scenario.units.push(UnitSpec {
            player: 1,
            kind: UnitKind::Sentinel,
            x: 14,
            y,
        });
    }
    scenario.units.push(UnitSpec {
        player: 0,
        kind: UnitKind::Kestrel,
        x: 18,
        y: 6,
    });
    let mut state = scenario.build().unwrap();
    let mut uninterrupted = brain(&scenario, BotDifficulty::Prime);
    let mut prefix = Vec::new();
    let mut experienced = false;
    for _ in 0..1800 {
        let act = uninterrupted.act_traced(&state);
        if let Some(trace) = act.trace {
            let evidence = serde_json::to_value(trace.experience).unwrap();
            experienced |= evidence["contexts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry[1].as_i64().unwrap() != 0);
        }
        let commands = act.commands;
        state.tick(&commands);
        prefix.push(commands);
    }
    assert!(
        experienced,
        "the replay prefix must exercise nonneutral grounded experience"
    );
    let mut replayed = scenario.build().unwrap();
    let mut rebuilt = brain(&scenario, BotDifficulty::Prime);
    for recorded in prefix {
        let _ = rebuilt.act_traced(&replayed);
        replayed.tick(&recorded);
    }
    for tick in 1800..2400 {
        let expected = uninterrupted.act(&state);
        let actual = rebuilt.act_traced(&replayed).commands;
        assert_eq!(actual, expected, "reconstructed suffix at {tick}");
        state.tick(&expected);
        replayed.tick(&actual);
        assert_eq!(
            Observation::fog_honest(&state, PlayerId(0)),
            Observation::fog_honest(&replayed, PlayerId(0))
        );
    }
}

#[test]
fn allocated_ground_members_stay_disjoint_and_commands_remain_ordinary() {
    let mut scenario = arena();
    scenario.units.push(UnitSpec {
        player: 1,
        kind: UnitKind::Sentinel,
        x: 15,
        y: 5,
    });
    let mut state = scenario.build().unwrap();
    let mut brain = brain(&scenario, BotDifficulty::Prime);
    let mut saw_army = false;
    for _ in 0..900 {
        let act = brain.act_traced(&state);
        let mut members = BTreeSet::new();
        for army in brain.executive().armies() {
            saw_army = true;
            for id in &army.members {
                assert!(members.insert(*id), "two Executive owners for {id:?}");
            }
        }
        let report = state.tick(&act.commands);
        assert!(
            !report
                .events
                .iter()
                .any(|event| matches!(event, Event::CommandRejected { .. })),
            "{report:?}"
        );
    }
    assert!(
        saw_army,
        "the scenario must actually exercise army ownership"
    );
}

#[test]
fn two_live_fronts_get_exact_disjoint_responses_without_emptying_home() {
    use oxide_sim::BuildingKind;
    use oxide_sim::bot::executive::{ArmyPurpose, MissionDisposition};
    use oxide_sim::scenario::BuildingSpec;
    let mut scenario = arena();
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Foundry,
        x: 31,
        y: 3,
    });
    scenario.units.extend((0..12).map(|index| UnitSpec {
        player: 0,
        kind: UnitKind::Sentinel,
        x: 35 + index % 3,
        y: 6 + index / 3,
    }));
    for x in [14, 42] {
        scenario.units.push(UnitSpec {
            player: 1,
            kind: UnitKind::Bombard,
            x,
            y: 4,
        });
        scenario.units.push(UnitSpec {
            player: 0,
            kind: UnitKind::Kestrel,
            x: x - 2,
            y: 4,
        });
    }
    let mut state = scenario.build().unwrap();
    let mut brain = brain(&scenario, BotDifficulty::Prime);
    let mut responses = std::collections::BTreeMap::new();
    for _ in 0..300 {
        let act = brain.act_traced(&state);
        if let Some(trace) = &act.trace {
            for decision in &trace.mission_decisions {
                if decision.disposition == MissionDisposition::Accepted
                    && let ArmyPurpose::Defend(asset) = decision.mission.purpose
                {
                    responses
                        .entry(asset)
                        .or_insert_with(|| decision.members.clone());
                }
            }
        }
        let report = state.tick(&act.commands);
        assert!(
            !report
                .events
                .iter()
                .any(|event| matches!(event, Event::CommandRejected { .. }))
        );
    }
    assert_eq!(responses.len(), 2, "{responses:?}");
    let mut all = BTreeSet::new();
    for response in responses.values() {
        assert!(response.len() >= 2);
        for id in response {
            assert!(all.insert(*id));
        }
    }
    let home = TilePos::new(3, 3);
    assert!(
        Observation::fog_honest(&state, PlayerId(0))
            .my_units
            .iter()
            .filter(|unit| {
                unit.kind == UnitKind::Sentinel && unit.hp > 0 && unit.tile.chebyshev(home) <= 12
            })
            .count()
            >= 4
    );
}
