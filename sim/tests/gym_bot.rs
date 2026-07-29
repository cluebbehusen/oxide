//! Gym-interface contracts: a scripted action sequence reproduces
//! bit-identically (training rollouts must be replayable), and the
//! masked menu is honest enough to play a real game through.

use chassis::rng::Pcg32;
use oxide_sim::bot::{Action, Brain, Difficulty, GymBot, Level, NeuralBot};
use oxide_sim::state::GameResult;
use oxide_sim::{Command, PlayerId, Scenario, UnitKind};

/// Drives a full match: gym bot in seat 0 picks actions with a seeded
/// rng over the legal mask; a scripted tier drives seat 1. Returns the
/// final state hash and the result.
fn scripted_match(seed: u64) -> (u64, Option<GameResult>) {
    let mut scenario = Scenario::skirmish();
    scenario.seed = seed;
    let mut state = scenario.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let mut opponent = Brain::for_tier(PlayerId(1), seed, Difficulty::Standard);
    let mut rng = Pcg32::new(seed, 7777);
    for tick in 0..30_000u64 {
        let mut commands = Vec::new();
        if tick % gym.cadence() == 0 && state.result().is_none() {
            let decision = gym.decision(&state);
            let legal: Vec<usize> = decision
                .mask
                .iter()
                .enumerate()
                .filter(|(_, ok)| **ok)
                .map(|(i, _)| i)
                .collect();
            let pick = legal[rng.next_below(legal.len() as u32) as usize];
            commands.extend(gym.step(&state, Action::from_index(pick)));
        }
        commands.extend(opponent.act(&state));
        state.tick(&commands);
        if state.result().is_some() {
            break;
        }
    }
    (state.hash(), state.result())
}

fn stranded_scenario(scrap: u32) -> Scenario {
    use oxide_sim::BuildingKind;
    use oxide_sim::scenario::BuildingSpec;

    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = scrap;
    scenario
        .units
        .retain(|u| u.player != 0 || u.kind != UnitKind::Harvester);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Fabricator,
        x: 9,
        y: 3,
    });
    scenario
}

#[test]
fn recovery_reserves_partial_scrap_and_overrides_a_wrong_macro_action() {
    let state = stranded_scenario(UnitKind::Scuttler.stats().cost)
        .build()
        .unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let decision = gym.decision(&state);
    let legal: Vec<usize> = decision
        .mask
        .iter()
        .enumerate()
        .filter_map(|(index, legal)| legal.then_some(index))
        .collect();
    assert_eq!(
        legal,
        vec![Action::Idle as usize],
        "the Scuttler-priced partial reserve must not be spendable"
    );
    assert!(
        gym.step(&state, Action::TrainScuttler).is_empty(),
        "an out-of-contract action cannot bypass the reserve"
    );

    let state = stranded_scenario(UnitKind::Harvester.stats().cost)
        .build()
        .unwrap();
    let decision = gym.decision(&state);
    let legal: Vec<usize> = decision
        .mask
        .iter()
        .enumerate()
        .filter_map(|(index, legal)| legal.then_some(index))
        .collect();
    assert_eq!(
        legal,
        vec![Action::TrainHarvester as usize],
        "the completed reserve has exactly one legal use"
    );
    let commands = gym.step(&state, Action::TrainScuttler);
    assert!(commands.iter().any(|command| matches!(
        command.command,
        Command::Train {
            kind: UnitKind::Harvester,
            ..
        }
    )));
    assert!(!commands.iter().any(|command| matches!(
        command.command,
        Command::Train {
            kind: UnitKind::Scuttler,
            ..
        }
    )));
}

#[test]
fn scripted_and_neural_commanders_take_the_recovery_harvester() {
    let scenario = stranded_scenario(UnitKind::Harvester.stats().cost);
    let state = scenario.build().unwrap();
    let assert_recovery = |commands: &[oxide_sim::PlayerCommand]| {
        assert!(commands.iter().any(|command| matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Harvester,
                ..
            }
        )));
        assert!(!commands.iter().any(|command| matches!(
            command.command,
            Command::Train { kind, .. } if kind != UnitKind::Harvester
        )));
    };

    let mut scripted = Brain::for_tier(PlayerId(0), scenario.seed, Difficulty::Prime);
    assert_recovery(&scripted.act(&state));

    let mut neural = NeuralBot::ladder(
        PlayerId(0),
        scenario.seed,
        Level::Expert,
        Some(500),
        scenario.players[0].faction,
    );
    assert_recovery(&neural.act(&state));
}

#[test]
fn gym_rollouts_reproduce_bit_identically() {
    let (a_hash, a_result) = scripted_match(11);
    let (b_hash, b_result) = scripted_match(11);
    assert_eq!(a_hash, b_hash, "same seed + same actions ⇒ same world");
    assert_eq!(a_result, b_result);
}

#[test]
fn the_mask_supports_playing_an_actual_game() {
    // A tiny hand-rolled policy over the gym menu: keep the economy at
    // four, drip sentinels, form an army, push when it stands. It must
    // function — units get built, an army forms, the match ends or at
    // minimum a real army exists by the cap.
    let scenario = Scenario::skirmish();
    let mut state = scenario.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let mut opponent = Brain::for_tier(PlayerId(1), scenario.seed, Difficulty::Scrapheap);
    let mut formed = false;
    for tick in 0..30_000u64 {
        let mut commands = Vec::new();
        if tick % gym.cadence() == 0 && state.result().is_none() {
            let d = gym.decision(&state);
            let harvesters = d.features[2];
            let staging_size = d.features[11];
            let want = if harvesters < 4 && d.mask[Action::TrainHarvester as usize] {
                Action::TrainHarvester
            } else if d.mask[Action::Push as usize] && staging_size >= 5 {
                Action::Push
            } else if d.mask[Action::FormArmy as usize] {
                Action::FormArmy
            } else if d.mask[Action::TrainSentinel as usize] {
                Action::TrainSentinel
            } else if d.mask[Action::Scout as usize] && tick % 1024 == 0 {
                Action::Scout
            } else {
                Action::Idle
            };
            formed |= staging_size > 0;
            commands.extend(gym.step(&state, want));
        }
        commands.extend(opponent.act(&state));
        state.tick(&commands);
        if let Some(GameResult::Victory { team }) = state.result() {
            assert_eq!(
                PlayerId(team),
                PlayerId(0),
                "the scripted gym line should beat Scrapheap"
            );
            assert!(formed, "it should have fought with a formed army");
            return;
        }
    }
    panic!("no decision against Scrapheap within the cap");
}

#[test]
fn salvage_masks_honestly_and_lowers_cheapest_first() {
    // v5's new verb: masked off with nothing eligible, on when an
    // eligible defense stands, lowering to the cheapest-and-least-
    // useful pick — and never the Fabricator or Foundry.
    use oxide_sim::scenario::BuildingSpec;
    use oxide_sim::stats::BuildingKind;
    let mut scenario = Scenario::skirmish();
    let mut gym = GymBot::new(PlayerId(0));
    let state = scenario.build().unwrap();
    let d = gym.decision(&state);
    assert!(
        !d.mask[Action::Salvage as usize],
        "nothing to strip at match start (a Foundry never counts)"
    );

    // Stand a turret and a bastion; the pick must be the turret.
    for (kind, x) in [(BuildingKind::Bastion, 9), (BuildingKind::Turret, 16)] {
        scenario.buildings.push(BuildingSpec {
            player: 0,
            kind,
            x,
            y: 3,
        });
    }
    let mut state = scenario.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let d = gym.decision(&state);
    assert!(
        d.mask[Action::Salvage as usize],
        "a standing defense arms it"
    );
    let my_building_value = d.features[63];
    let expected = BuildingKind::Turret.stats().construction.unwrap().cost
        + BuildingKind::Bastion.stats().construction.unwrap().cost;
    assert_eq!(
        my_building_value,
        i64::from(expected),
        "the v5 feature prices the standing stock"
    );
    let commands = gym.step(&state, Action::Salvage);
    let turret = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Turret)
        .unwrap()
        .id;
    assert!(
        commands.iter().any(|c| matches!(
            &c.command,
            oxide_sim::Command::Salvage { building, .. } if *building == turret
        )),
        "cheapest-first: the turret goes before the bastion: {commands:?}"
    );
    // And the sim accepts what the lowering emitted.
    let report = state.tick(&commands);
    assert!(
        !report
            .events
            .iter()
            .any(|e| matches!(e, oxide_sim::Event::CommandRejected { .. })),
        "the lowered command validates: {:?}",
        report.events
    );
}

/// A long corridor for the v6 weld verb: p0's foundry west, p1's east,
/// far enough apart that a welder can stand outside the patient leash.
fn weld_arena(units: Vec<oxide_sim::scenario::UnitSpec>) -> Scenario {
    let mut map = vec![format!("#{}#", ".".repeat(38)); 9];
    map[0] = "#".repeat(40);
    map[8] = "#".repeat(40);
    map[1] = format!("#1{}#", ".".repeat(37));
    map[6] = format!("#{}2{}#", ".".repeat(35), ".".repeat(2));
    Scenario {
        name: "weld-corridor".into(),
        seed: 42,
        map,
        players: (0..2)
            .map(|i| oxide_sim::scenario::PlayerSpec {
                name: ["Ferrous", "Cupric"][i].into(),
                faction: [oxide_sim::Faction::Ferrous, oxide_sim::Faction::Cupric][i],
                team: None,
                scrap: 200,
                bot: false,
                bot_config: None,
            })
            .collect(),
        units,
        buildings: Vec::new(),
        meta: None,
    }
}

fn spec(player: u8, kind: oxide_sim::UnitKind, x: i32, y: i32) -> oxide_sim::scenario::UnitSpec {
    oxide_sim::scenario::UnitSpec { player, kind, x, y }
}

/// Chews the patient down below `floor` hp with the raider, then walks
/// the raider home so the wound sits still for the decision under test.
fn wound(
    state: &mut oxide_sim::State,
    patient: oxide_sim::UnitId,
    raider: oxide_sim::UnitId,
    floor: u32,
) {
    use chassis::grid::TilePos;
    let orders = |units: Vec<oxide_sim::UnitId>, goal: TilePos| {
        vec![oxide_sim::PlayerCommand {
            player: PlayerId(1),
            command: oxide_sim::Command::Move {
                units,
                goal,
                queue: false,
            },
        }]
    };
    state.tick(&orders(vec![raider], TilePos::new(6, 2)));
    for _ in 0..2_000 {
        state.tick(&[]);
        if state.unit(patient).unwrap().hp <= floor {
            break;
        }
    }
    state.tick(&orders(vec![raider], TilePos::new(34, 4)));
    for _ in 0..2_000 {
        state.tick(&[]);
        if state.unit(raider).unwrap().tile() == TilePos::new(34, 4) {
            break;
        }
    }
    let hp = state.unit(patient).unwrap().hp;
    let max = state.unit(patient).unwrap().kind.stats().max_hp;
    assert!(hp > 0 && hp < max, "test premise: a live, weldable wound");
}

#[test]
fn repair_unit_masks_on_the_leash_and_welds_the_wound() {
    use chassis::grid::TilePos;
    use oxide_sim::UnitKind;
    // Patient near home, welder parked far beyond the leash.
    let scenario = weld_arena(vec![
        spec(0, UnitKind::Harvester, 4, 2),  // patient
        spec(0, UnitKind::Harvester, 28, 2), // welder, out on the corridor
        spec(1, UnitKind::Scuttler, 34, 4),  // raider
    ]);
    let mut state = scenario.build().unwrap();
    let (patient, welder, raider) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let mut gym = GymBot::new(PlayerId(0));
    let d = gym.decision(&state);
    assert!(
        !d.mask[Action::RepairUnit as usize],
        "no wound, no weld verb"
    );
    assert_eq!(d.features[64], 0, "an unwounded roster locks no scrap");

    wound(&mut state, patient, raider, 40);
    let hurt = state.unit(patient).unwrap().hp;
    let stats = UnitKind::Harvester.stats();
    let d = gym.decision(&state);
    assert_eq!(
        d.features[64],
        i64::from(stats.cost) * i64::from(stats.max_hp - hurt) / i64::from(stats.max_hp),
        "the v6 feature prices the wound whether or not the verb is legal"
    );
    assert!(
        !d.mask[Action::RepairUnit as usize],
        "a welder 24 tiles away is not on the leash — masking it on would lie"
    );

    // Walk the welder home; the mask must flip on with the geometry.
    state.tick(&[oxide_sim::PlayerCommand {
        player: PlayerId(0),
        command: oxide_sim::Command::Move {
            units: vec![welder],
            goal: TilePos::new(10, 2),
            queue: false,
        },
    }]);
    for _ in 0..2_000 {
        state.tick(&[]);
        if state.unit(welder).unwrap().tile() == TilePos::new(10, 2) {
            break;
        }
    }
    let d = gym.decision(&state);
    assert!(
        d.mask[Action::RepairUnit as usize],
        "wound plus a welder inside the leash arms the verb"
    );
    let commands = gym.step(&state, Action::RepairUnit);
    assert!(
        commands.iter().any(|c| matches!(
            &c.command,
            oxide_sim::Command::RepairUnit { units, target, .. }
                if *target == patient && units.contains(&welder)
        )),
        "the lowering sends the welder at the patient: {commands:?}"
    );
    // And the sim honors it end to end: the wound closes. Only the
    // weld ticks — the chore channel may prospect the idle patient,
    // and a walking patient outpaces the proof, not the verb.
    let weld: Vec<_> = commands
        .into_iter()
        .filter(|c| matches!(&c.command, oxide_sim::Command::RepairUnit { .. }))
        .collect();
    state.tick(&weld);
    for _ in 0..1_000 {
        state.tick(&[]);
        if state.unit(patient).unwrap().hp > hurt {
            return;
        }
    }
    panic!("the weld never landed an hp");
}

#[test]
fn a_patient_is_never_its_own_welder() {
    use oxide_sim::UnitKind;
    // One harvester total: wounded, site-free, and by the builder_free
    // rule a "free harvester" — but it cannot crew its own weld, so the
    // mask must stay off.
    let scenario = weld_arena(vec![
        spec(0, UnitKind::Harvester, 4, 2), // patient, the only harvester
        spec(1, UnitKind::Scuttler, 34, 4), // raider
    ]);
    let mut state = scenario.build().unwrap();
    let (patient, raider) = (state.units()[0].id, state.units()[1].id);
    wound(&mut state, patient, raider, 40);
    let mut gym = GymBot::new(PlayerId(0));
    let d = gym.decision(&state);
    assert!(
        !d.mask[Action::RepairUnit as usize],
        "the wounded harvester is a patient, not a crew"
    );
}

#[test]
fn repair_unit_prefers_recoverable_value_over_raw_hp_deficit() {
    use oxide_sim::UnitKind;

    let scenario = weld_arena(vec![
        spec(0, UnitKind::Harvester, 4, 2), // cheap, deeply wounded patient
        spec(0, UnitKind::Harvester, 5, 2), // full-health welder
        spec(0, UnitKind::Lancer, 6, 2),    // pricier, shallower wound
    ]);
    let state = scenario.build().unwrap();
    let (cheap, valuable) = (state.units()[0].id, state.units()[2].id);
    let cheap_hp = 10;
    let valuable_hp = 30;
    let mut json = serde_json::to_value(state).unwrap();
    json["units"][0]["hp"] = serde_json::json!(cheap_hp);
    json["units"][2]["hp"] = serde_json::json!(valuable_hp);
    let state: oxide_sim::State = serde_json::from_value(json).unwrap();

    let cheap_stats = UnitKind::Harvester.stats();
    let valuable_stats = UnitKind::Lancer.stats();
    let cheap_deficit = cheap_stats.max_hp - cheap_hp;
    let valuable_deficit = valuable_stats.max_hp - valuable_hp;
    assert!(
        cheap_deficit > valuable_deficit,
        "test premise: raw hp points favor the cheap patient"
    );
    assert!(
        cheap_stats.cost * cheap_deficit / cheap_stats.max_hp
            < valuable_stats.cost * valuable_deficit / valuable_stats.max_hp,
        "test premise: recoverable purchase value favors the Lancer"
    );

    let mut gym = GymBot::new(PlayerId(0));
    let commands = gym.step(&state, Action::RepairUnit);
    assert!(
        commands.iter().any(|command| matches!(
            command.command,
            Command::RepairUnit { target, .. } if target == valuable
        )),
        "the smaller, more valuable wound should win: {commands:?}"
    );
    assert!(
        !commands.iter().any(|command| matches!(
            command.command,
            Command::RepairUnit { target, .. } if target == cheap
        )),
        "raw hp deficit must not choose the cheaper wound"
    );
}

#[test]
fn build_repair_bay_lowers_to_an_accepted_foundation() {
    use oxide_sim::stats::BuildingKind;
    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = 400;
    let mut state = scenario.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let d = gym.decision(&state);
    assert!(
        d.mask[Action::BuildRepairBay as usize],
        "scrap and a free builder arm the bay slot"
    );
    let commands = gym.step(&state, Action::BuildRepairBay);
    assert!(
        commands.iter().any(|c| matches!(
            &c.command,
            oxide_sim::Command::Build { kind, .. } if *kind == BuildingKind::RepairBay
        )),
        "the training slot founds a bay: {commands:?}"
    );
    let report = state.tick(&commands);
    assert!(
        !report
            .events
            .iter()
            .any(|e| matches!(e, oxide_sim::Event::CommandRejected { .. })),
        "the lowered foundation validates: {:?}",
        report.events
    );
    assert!(
        state
            .buildings()
            .iter()
            .any(|b| b.kind == BuildingKind::RepairBay && b.player == PlayerId(0)),
        "the site stands"
    );
    let bay = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::RepairBay && b.player == PlayerId(0))
        .unwrap()
        .id;

    let d = gym.decision(&state);
    assert!(
        !d.mask[Action::BuildRepairBay as usize],
        "an in-progress Bay blocks a duplicate"
    );
    let mut duplicate_probe = state.clone();
    duplicate_probe.tick(&gym.step(&state, Action::BuildRepairBay));
    assert_eq!(
        duplicate_probe
            .buildings()
            .iter()
            .filter(|b| b.kind == BuildingKind::RepairBay && b.player == PlayerId(0))
            .count(),
        1,
        "direct lowering cannot bypass the in-progress guard"
    );

    for _ in 0..500 {
        state.tick(&[]);
        if state.building(bay).is_some_and(|b| b.built) {
            break;
        }
    }
    assert!(
        state.building(bay).is_some_and(|b| b.built),
        "test premise: the first Bay completes"
    );
    let mut completed = GymBot::new(PlayerId(0));
    assert!(
        !completed.decision(&state).mask[Action::BuildRepairBay as usize],
        "a completed Bay blocks a duplicate"
    );
    let mut duplicate_probe = state.clone();
    duplicate_probe.tick(&completed.step(&state, Action::BuildRepairBay));
    assert_eq!(
        duplicate_probe
            .buildings()
            .iter()
            .filter(|b| b.kind == BuildingKind::RepairBay && b.player == PlayerId(0))
            .count(),
        1,
        "direct lowering cannot bypass the completed guard"
    );

    let salvager = state
        .units()
        .iter()
        .find(|u| u.player == PlayerId(0) && u.kind == UnitKind::Harvester)
        .unwrap()
        .id;
    state.tick(&[oxide_sim::PlayerCommand {
        player: PlayerId(0),
        command: Command::Salvage {
            units: vec![salvager],
            building: bay,
            queue: false,
        },
    }]);
    for _ in 0..1_000 {
        state.tick(&[]);
        if state.building(bay).is_none() {
            break;
        }
    }
    assert!(
        state.building(bay).is_none(),
        "test premise: the original Bay is gone"
    );
    let mut rebuild = GymBot::new(PlayerId(0));
    assert!(
        rebuild.decision(&state).mask[Action::BuildRepairBay as usize],
        "once the old Bay is gone, rebuilding becomes legal"
    );
}

#[test]
fn the_repair_channel_leaves_salvage_targets_alone() {
    // The two verbs must never share a target: with the only wounded
    // building under an own crew's salvage, Repair masks off.
    use oxide_sim::scenario::BuildingSpec;
    use oxide_sim::stats::BuildingKind;
    let mut scenario = Scenario::skirmish();
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Turret,
        x: 9,
        y: 6,
    });
    let mut state = scenario.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let commands = gym.step(&state, Action::Salvage);
    state.tick(&commands);
    // Let the crew walk over and leave first scars.
    for _ in 0..300 {
        state.tick(&[]);
        let turret = state
            .buildings()
            .iter()
            .find(|b| b.kind == BuildingKind::Turret);
        if turret.is_some_and(|b| b.hp < b.kind.stats().max_hp) {
            break;
        }
    }
    let turret = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Turret)
        .expect("still standing");
    assert!(
        turret.hp < turret.kind.stats().max_hp,
        "test premise: the strip left a wound repair would otherwise take"
    );
    let d = gym.decision(&state);
    assert!(
        !d.mask[Action::Repair as usize],
        "a building under own salvage is not a patient"
    );
}
