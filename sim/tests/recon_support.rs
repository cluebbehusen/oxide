//! Composed player-facing reconnaissance and support ownership contracts.

use chassis::grid::TilePos;
use oxide_sim::bot::trace::{ProposalDispositionTrace, ProposalKeyTrace, ReconPhaseTrace};
use oxide_sim::bot::{Brain, Observation, PublicMapBriefing};
use oxide_sim::scenario::{
    BotConfig, BotDifficulty, BotStance, BuildingSpec, PlayerSpec, UnitSpec,
};
use oxide_sim::{BuildingKind, Command, Event, Faction, PlayerId, Scenario, TickReport, UnitKind};
use std::collections::BTreeSet;
use std::sync::Arc;

fn three_fronts() -> Scenario {
    let mut map = vec![vec![b'.'; 64]; 32];
    for row in &mut map {
        row[0] = b'#';
        row[63] = b'#';
    }
    map[0].fill(b'#');
    map[31].fill(b'#');
    for (x, y, marker) in [(2, 14, b'1'), (57, 3, b'2'), (57, 26, b'3')] {
        map[y][x] = marker;
    }
    for y in [7, 10, 18, 21] {
        map[y][3] = b'S';
    }
    map[10][12] = b'E';
    let mut units = vec![
        UnitSpec {
            player: 0,
            kind: UnitKind::Kestrel,
            x: 9,
            y: 5,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Kestrel,
            x: 9,
            y: 25,
        },
        UnitSpec {
            player: 1,
            kind: UnitKind::Harvester,
            x: 60,
            y: 4,
        },
        UnitSpec {
            player: 2,
            kind: UnitKind::Harvester,
            x: 60,
            y: 27,
        },
    ];
    units.extend((0..4).map(|index| UnitSpec {
        player: 0,
        kind: UnitKind::Harvester,
        x: 5,
        y: 11 + index,
    }));
    units.extend((0..12).map(|index| UnitSpec {
        player: 0,
        kind: UnitKind::Sentinel,
        x: 7 + index % 4,
        y: 12 + index / 4,
    }));
    Scenario {
        name: "independent-information-fronts".into(),
        seed: 81,
        map: map
            .into_iter()
            .map(|row| String::from_utf8(row).unwrap())
            .collect(),
        players: (0..3)
            .map(|seat| PlayerSpec {
                name: format!("Seat {seat}"),
                faction: Faction::Ferrous,
                team: None,
                scrap: 700,
                bot: seat != 0,
                bot_config: None,
            })
            .collect(),
        units,
        buildings: vec![
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Fabricator,
                x: 5,
                y: 22,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Airworks,
                x: 8,
                y: 22,
            },
        ],
        meta: None,
    }
}

fn brain(scenario: &Scenario, difficulty: BotDifficulty) -> Brain {
    Brain::scripted(
        PlayerId(0),
        BotConfig::scripted(difficulty, BotStance::Balanced, 9_000),
        Arc::new(PublicMapBriefing::from_scenario(scenario).unwrap()),
    )
}

fn assert_legal(report: &TickReport) {
    assert!(
        !report
            .events
            .iter()
            .any(|event| matches!(event, Event::CommandRejected { .. })),
        "{report:?}"
    );
}

#[test]
fn lost_island_observer_recovers_without_new_enemy_sight() {
    let mut scenario = three_fronts();
    for row in scenario.map.iter_mut().skip(1).take(30) {
        let mut tiles = row.as_bytes().to_vec();
        tiles[20] = b'#';
        *row = String::from_utf8(tiles).unwrap();
    }
    let mut state = scenario.build().unwrap();
    let mut brain = brain(&scenario, BotDifficulty::Prime);
    let initial = brain.act_traced(&state);
    let assignment = initial
        .trace
        .as_ref()
        .unwrap()
        .reconnaissance
        .assignments
        .entries
        .first()
        .expect("free island reconnaissance starts from public priors")
        .clone();
    let lost = assignment.unit.unwrap();
    assert_legal(&state.tick(&initial.commands));
    let mut document = serde_json::to_value(&state).unwrap();
    document["units"]
        .as_array_mut()
        .unwrap()
        .retain(|unit| unit["id"] != serde_json::json!(lost.0));
    state = serde_json::from_value(document).unwrap();
    let mut recovery = None;
    let mut retried = false;
    for _ in 0..4_800 {
        let decision = brain.act_traced(&state);
        if let Some(trace) = &decision.trace {
            if let Some(entry) = trace.reconnaissance.recovery.entries.iter().find(|entry| {
                matches!(&assignment.key, ProposalKeyTrace::Reconnaissance { consumer, anchor, .. }
                    if *consumer == entry.consumer && *anchor == entry.anchor)
            }) {
                recovery.get_or_insert(entry.retry_at);
            }
            for work in &trace.reconnaissance.assignments.entries {
                let same_question = match (&assignment.key, &work.key) {
                    (
                        ProposalKeyTrace::Reconnaissance {
                            consumer: a,
                            anchor: x,
                            ..
                        },
                        ProposalKeyTrace::Reconnaissance {
                            consumer: b,
                            anchor: y,
                            ..
                        },
                    ) => a == b && x == y,
                    _ => false,
                };
                if same_question && work.accepted_at > assignment.accepted_at {
                    assert!(
                        state.current_tick() >= recovery.expect("loss establishes local cooldown")
                    );
                    assert_ne!(work.unit, Some(lost));
                    retried = true;
                }
            }
        }
        assert_legal(&state.tick(&decision.commands));
        if retried {
            break;
        }
    }
    assert!(recovery.is_some(), "the exact lost owner enters recovery");
    assert!(
        retried,
        "an unanswered safe island question can recover without new contact"
    );
}

#[test]
fn disconnected_damage_does_not_hide_a_reachable_repair_patient() {
    let mut scenario = three_fronts();
    for row in scenario.map.iter_mut().skip(1).take(30) {
        let mut tiles = row.as_bytes().to_vec();
        tiles[20] = b'#';
        *row = String::from_utf8(tiles).unwrap();
    }
    scenario.units.extend([
        UnitSpec {
            player: 0,
            kind: UnitKind::Avalanche,
            x: 25,
            y: 20,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 12,
            y: 20,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Tender,
            x: 11,
            y: 20,
        },
    ]);
    let mut state = scenario.build().unwrap();
    let unreachable = state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Avalanche)
        .unwrap()
        .id;
    let reachable = state
        .units()
        .iter()
        .find(|unit| unit.tile() == TilePos::new(12, 20))
        .unwrap()
        .id;
    let mut document = serde_json::to_value(&state).unwrap();
    for unit in document["units"].as_array_mut().unwrap() {
        if unit["id"] == serde_json::json!(unreachable.0)
            || unit["id"] == serde_json::json!(reachable.0)
        {
            unit["hp"] = serde_json::json!(1);
        }
    }
    state = serde_json::from_value(document).unwrap();
    let mut brain = brain(&scenario, BotDifficulty::Prime);
    let mut served = false;
    for _ in 0..360 {
        let decision = brain.act_traced(&state);
        for command in &decision.commands {
            if let Command::RepairUnit { target, units, .. } = &command.command {
                assert_ne!(*target, unreachable);
                if *target == reachable {
                    assert_eq!(units.len(), 1);
                    served = true;
                    let trace = decision.trace.as_ref().unwrap();
                    assert!(trace.support.repairs.entries.iter().any(|repair| {
                        repair.worker == units[0] && repair.funded_until > state.current_tick()
                    }));
                }
            }
        }
        assert_legal(&state.tick(&decision.commands));
    }
    assert!(
        served,
        "the sealed higher-value casualty must not hide reachable work"
    );
    assert!(state.unit(reachable).unwrap().hp > 1);
    assert_eq!(state.unit(unreachable).unwrap().hp, 1);
}

#[test]
fn relief_skips_a_stronger_sealed_front_and_preserves_the_home_roster() {
    let mut scenario = three_fronts();
    for player in &mut scenario.players {
        player.team = Some(1);
    }
    scenario.players.push(PlayerSpec {
        name: "Pressure".into(),
        faction: Faction::Ferrous,
        team: Some(2),
        scrap: 0,
        bot: true,
        bot_config: None,
    });
    let mut map: Vec<_> = scenario
        .map
        .iter()
        .map(|row| row.as_bytes().to_vec())
        .collect();
    map[14][40] = b'4';
    for row in map.iter_mut().take(10).skip(1) {
        row[54] = b'#';
        row[62] = b'#';
    }
    map[1][54..=62].fill(b'#');
    map[9][54..=62].fill(b'#');
    scenario.map = map
        .into_iter()
        .map(|row| String::from_utf8(row).unwrap())
        .collect();
    scenario
        .units
        .extend([(56, 6), (59, 7), (55, 25)].map(|(x, y)| UnitSpec {
            player: 3,
            kind: UnitKind::Sentinel,
            x,
            y,
        }));
    let mut state = scenario.build().unwrap();
    let reachable = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(2))
        .unwrap()
        .id;
    let sealed = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(1))
        .unwrap()
        .id;
    let mut brain = brain(&scenario, BotDifficulty::Prime);
    let mut admitted = false;
    for _ in 0..120 {
        let decision = brain.act_traced(&state);
        if let Some(trace) = &decision.trace {
            for proposal in &trace.allocation.proposals.entries {
                if let ProposalKeyTrace::SupportRelief { foundry } = proposal.key {
                    assert_ne!(
                        foundry, sealed,
                        "pressure is not evidence of a usable ground approach"
                    );
                    if proposal.disposition == ProposalDispositionTrace::Accepted {
                        assert_eq!(foundry, reachable);
                        admitted = true;
                    }
                }
            }
        }
        assert_legal(&state.tick(&decision.commands));
    }
    assert!(
        admitted,
        "reachable relief must not be hidden by the stronger sealed front"
    );
    let home = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(0) && building.kind == BuildingKind::Foundry)
        .unwrap()
        .anchor;
    assert!(
        state
            .units()
            .iter()
            .filter(|unit| unit.player == PlayerId(0)
                && unit.kind == UnitKind::Sentinel
                && unit.tile().chebyshev(home) <= 12)
            .count()
            >= 8
    );
}

#[test]
fn independently_owned_questions_coexist_with_economic_allocation() {
    let scenario = three_fronts();
    let mut state = scenario.build().unwrap();
    let mut brain = brain(&scenario, BotDifficulty::Prime);
    let mut concurrent = false;
    let mut economic = false;
    for _ in 0..120 {
        let decision = brain.act_traced(&state);
        if let Some(trace) = &decision.trace {
            let assignments = &trace.reconnaissance.assignments.entries;
            let owners: BTreeSet<_> = assignments.iter().filter_map(|work| work.unit).collect();
            assert_eq!(
                owners.len(),
                assignments
                    .iter()
                    .filter(|work| work.unit.is_some())
                    .count()
            );
            let admitted = trace
                .allocation
                .proposals
                .entries
                .iter()
                .filter(|proposal| proposal.disposition == ProposalDispositionTrace::Accepted);
            let recon_admitted = admitted
                .clone()
                .filter(|proposal| matches!(proposal.key, ProposalKeyTrace::Reconnaissance { .. }))
                .count();
            assert!(
                recon_admitted <= 1,
                "one new alternative per pass, not one retained scout total"
            );
            for proposal in admitted {
                if !matches!(proposal.key, ProposalKeyTrace::Reconnaissance { .. }) {
                    assert!(
                        proposal
                            .claims
                            .units
                            .entries
                            .iter()
                            .chain(&proposal.claims.builders.entries)
                            .all(|unit| !owners.contains(unit)),
                        "allocation cannot reuse a retained observer"
                    );
                }
                economic |= matches!(proposal.key, ProposalKeyTrace::Economy { .. });
            }
            concurrent |= assignments
                .iter()
                .filter(|work| work.phase == ReconPhaseTrace::Outbound)
                .count()
                >= 2;
        }
        let report = state.tick(&decision.commands);
        assert_legal(&report);
    }
    assert!(
        concurrent,
        "two independent authored starts justify concurrent observers"
    );
    assert!(
        economic,
        "reconnaissance does not suppress independent economic investment"
    );
}

#[test]
fn connected_packages_share_capacity_without_stealing_recon_or_repair_owners() {
    let mut scenario = three_fronts();
    scenario.players[0].scrap = 3_000;
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Array,
        x: 50,
        y: 3,
    });
    scenario.units.extend([
        UnitSpec {
            player: 0,
            kind: UnitKind::Buzzard,
            x: 44,
            y: 4,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Buzzard,
            x: 44,
            y: 6,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Bombard,
            x: 45,
            y: 10,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Bombard,
            x: 46,
            y: 10,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Tender,
            x: 45,
            y: 11,
        },
    ]);
    let mut state = scenario.build().unwrap();
    let wounded = state
        .units()
        .iter()
        .find(|unit| unit.tile() == TilePos::new(45, 10))
        .unwrap()
        .id;
    let mut document = serde_json::to_value(&state).unwrap();
    for unit in document["units"].as_array_mut().unwrap() {
        if unit["id"] == serde_json::json!(wounded.0) {
            unit["hp"] = serde_json::json!(1);
        }
    }
    state = serde_json::from_value(document).unwrap();
    let mut brain = brain(&scenario, BotDifficulty::Prime);
    let mut connected = false;
    let mut reconnaissance = false;
    let mut support = false;
    for _ in 0..600 {
        let decision = brain.act_traced(&state);
        if let Some(trace) = &decision.trace {
            let assigned = &trace.connected_force.assigned;
            let operation: BTreeSet<_> = assigned
                .scout
                .into_iter()
                .chain(assigned.suppression.iter().copied())
                .chain(assigned.strike.iter().copied())
                .collect();
            connected |= !operation.is_empty();
            let observers: BTreeSet<_> = trace
                .reconnaissance
                .assignments
                .entries
                .iter()
                .filter_map(|work| work.unit)
                .collect();
            let repairers: BTreeSet<_> = trace
                .support
                .repairs
                .entries
                .iter()
                .map(|work| work.worker)
                .collect();
            assert!(operation.is_disjoint(&observers));
            assert!(operation.is_disjoint(&repairers));
            assert!(observers.is_disjoint(&repairers));
            reconnaissance |= !observers.is_empty();
            support |= !repairers.is_empty();
            assert!(trace.allocation.error.is_none());
            assert!(trace.allocation.coordinator_failure.is_none());
        }
        assert_legal(&state.tick(&decision.commands));
    }
    assert!(
        connected && reconnaissance && support,
        "all three consumers must exercise the common capacity: connected={connected}, reconnaissance={reconnaissance}, support={support}"
    );
}

#[test]
fn a_current_raid_objective_procures_only_its_missing_pair() {
    let mut scenario = three_fronts();
    scenario.players[0].scrap = 2_000;
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Array,
        x: 54,
        y: 3,
    });
    let mut state = scenario.build().unwrap();
    assert!(
        Observation::fog_honest(&state, PlayerId(0))
            .enemy_units
            .iter()
            .any(|unit| unit.kind.stats().harvest.is_some())
    );
    let mut brain = brain(&scenario, BotDifficulty::Prime);
    let mut bought = 0;
    for _ in 0..72 {
        let decision = brain.act_traced(&state);
        bought += decision
            .commands
            .iter()
            .filter(|command| {
                matches!(
                    command.command,
                    Command::Train {
                        kind: UnitKind::Scuttler,
                        ..
                    }
                )
            })
            .count();
        assert!(
            bought <= 2,
            "paid and same-think occurrences must be credited exactly once"
        );
        assert_legal(&state.tick(&decision.commands));
    }
    assert_eq!(
        bought, 2,
        "a current reachable worker justifies the exact missing tactical pair"
    );
}

#[test]
fn healthy_precontact_roster_has_no_speculative_tender_or_scuttler_purchase() {
    let scenario = three_fronts();
    let mut state = scenario.build().unwrap();
    let mut brain = brain(&scenario, BotDifficulty::Prime);
    for _ in 0..60 {
        let obs = Observation::fog_honest(&state, PlayerId(0));
        assert!(obs.enemy_units.is_empty() && obs.enemy_buildings.is_empty());
        let commands = brain.act(&state);
        assert!(
            !commands.iter().any(|command| matches!(
                command.command,
                Command::Train {
                    kind: UnitKind::Tender | UnitKind::Scuttler,
                    ..
                }
            )),
            "finite support and raid demand is required before buying a specialist"
        );
        assert_legal(&state.tick(&commands));
    }
}

#[test]
fn question_ownership_preserves_traced_and_untraced_commands_at_every_difficulty() {
    for difficulty in BotDifficulty::ALL {
        let scenario = three_fronts();
        let mut state = scenario.build().unwrap();
        let mut traced = brain(&scenario, difficulty);
        let mut ordinary = brain(&scenario, difficulty);
        let mut recon = false;
        for _ in 0..180 {
            let decision = traced.act_traced(&state);
            assert_eq!(decision.commands, ordinary.act(&state), "{difficulty:?}");
            recon |= decision
                .trace
                .as_ref()
                .is_some_and(|trace| !trace.reconnaissance.assignments.entries.is_empty());
            assert_legal(&state.tick(&decision.commands));
        }
        assert!(
            recon,
            "every difficulty retains reconnaissance capability: {difficulty:?}"
        );
    }
}
