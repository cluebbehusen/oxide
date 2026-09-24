use super::*;

#[test]
fn inactive_seat_or_only_provisional_foundry_does_not_advance_planners() {
    let scenario = opening_core_team_relief_scenario();
    for difficulty in BotDifficulty::ALL {
        for (surrender, provisional) in [(false, false), (true, false), (false, true)] {
            let mut state = scenario.build().unwrap();
            let config = BotConfig::scripted(difficulty, BotStance::Balanced, 9000);
            let mut brain = scripted_brain(&scenario, PlayerId(0), config);
            let initial = brain.act_traced(&state);
            assert!(initial.trace.is_some());
            state.tick(&initial.commands);
            if surrender {
                state.tick(&[PlayerCommand {
                    player: PlayerId(0),
                    command: Command::Surrender,
                }]);
            }
            while !state.current_tick().is_multiple_of(brain.dials.cadence) {
                state.tick(&[]);
            }
            let before = brain.clone();
            let traced = if surrender {
                assert!(state.player(PlayerId(0)).resigned);
                assert!(brain.act(&state).is_empty());
                brain.act_traced(&state)
            } else {
                let mut obs = Observation::fog_honest(&state, PlayerId(0));
                if provisional {
                    for building in &mut obs.my_buildings {
                        if building.kind == BuildingKind::Foundry {
                            building.built = false;
                            building.provisional = true;
                        }
                    }
                } else {
                    obs.my_buildings
                        .retain(|building| building.kind != BuildingKind::Foundry);
                }
                assert!(crate::Brain::act(&mut brain, &obs).is_empty());
                crate::Brain::act_traced(&mut brain, &obs)
            };
            assert!(traced.commands.is_empty());
            assert!(traced.trace.is_none());
            assert_brain_unchanged(&before, &brain);
            let mut ally = scripted_brain(&scenario, PlayerId(1), config);
            assert!(ally.act_traced(&state).trace.is_some());
        }
    }
}

#[test]
fn home_orientation_uses_a_physical_foundry_instead_of_an_older_plan() {
    let scenario = opening_core_team_relief_scenario();
    let state = scenario.build().unwrap();
    let mut obs = Observation::fog_honest(&state, PlayerId(0));
    let planned = obs
        .my_buildings
        .iter_mut()
        .find(|b| b.kind == BuildingKind::Foundry)
        .unwrap();
    planned.anchor = TilePos::new(3, 1);
    planned.built = false;
    planned.provisional = true;
    let home = TilePos::new(30, 20);
    obs.my_buildings.push(crate::test_support::building(
        99,
        PlayerId(0),
        BuildingKind::Foundry,
        home,
    ));
    obs.my_queues.push(Vec::new());
    obs.my_queue_progress.push(0);
    let mut brain = scripted_brain(&scenario, PlayerId(0), BotConfig::default());
    assert!(crate::Brain::act_traced(&mut brain, &obs).trace.is_some());
    assert_eq!(brain.orientation, Some(Orientation::for_home(&obs, home)));
}

#[test]
fn the_brain_receives_the_public_map_briefing() {
    let scenario = Scenario::skirmish();
    let public_map = public_map(&scenario);
    let scripted = SeatBot::scripted(PlayerId(0), BotConfig::default(), Arc::clone(&public_map));

    assert!(Arc::ptr_eq(&scripted.mind().public_map, &public_map));
    assert!(scripted.mind().oriented_public_map.is_none());
}

#[test]
fn player_facing_brains_do_not_send_partial_musters_to_remote_expansions() {
    let scenario = remote_expansion_defense_scenario();
    let state = scenario.build().expect("the defense scenario builds");
    let observation = Observation::fog_honest(&state, PlayerId(0));
    assert_eq!(observation.enemy_units.len(), 1);
    let members: Vec<_> = observation.my_units.iter().map(|unit| unit.id).collect();
    assert_eq!(members.len(), 2);

    for difficulty in BotDifficulty::ALL {
        for personality_seed in 0..64 {
            let config = BotConfig::scripted(difficulty, BotStance::Balanced, personality_seed);
            let mut brain = scripted_brain(&scenario, PlayerId(0), config);
            let staging = TilePos::new(8, 7);
            let muster = brain.exec.apply_with_reservations(
                PlayerId(0),
                &observation,
                &[Intent::FormArmy { staging, size: 2 }],
                &[],
            );
            assert!(muster.iter().any(|command| matches!(
                &command.command,
                Command::AttackMove { units, goal, queue: false }
                    if units == &members && *goal == staging
            )));

            let commands = brain.act(&state);
            let army = &brain.exec.armies()[0];
            assert_eq!(
                (army.state, army.target),
                (ArmyState::Staging, None),
                "{difficulty:?} seed {personality_seed} dispatched the partial body: {commands:?}"
            );
            assert!(commands.iter().all(|command| !matches!(
                &command.command,
                Command::AttackMove { units, .. }
                    if units.iter().any(|unit| members.contains(unit))
            )));
        }
    }
}

#[test]
fn player_facing_rear_tiles_mirror_inside_odd_and_even_footprints() {
    let mut obs = test_island_observation();
    obs.map_width = 48;
    obs.map_height = 30;
    let left_anchor = TilePos::new(5, 4);

    for size in [(1, 1), (2, 2), (3, 1), (4, 2)] {
        let right_anchor = TilePos::new(
            obs.map_width - size.0 - left_anchor.x,
            obs.map_height - size.1 - left_anchor.y,
        );
        let left_orientation = Orientation::for_home(&obs, left_anchor);
        let right_orientation = Orientation::for_home(&obs, right_anchor);
        let left = player_facing_rear_tile(left_orientation, left_anchor, size);
        let right = player_facing_rear_tile(right_orientation, right_anchor, size);
        let inside = |tile: TilePos, anchor: TilePos| {
            tile.x >= anchor.x
                && tile.x < anchor.x + size.0
                && tile.y >= anchor.y
                && tile.y < anchor.y + size.1
        };

        assert!(inside(left, left_anchor), "left {size:?}");
        assert!(inside(right, right_anchor), "right {size:?}");
        assert_eq!(
            TilePos::new(obs.map_width - 1 - left.x, obs.map_height - 1 - left.y,),
            right,
            "{size:?} footprint goals must be exact half-turns"
        );
    }
}

#[test]
fn mirrored_wounded_armies_withdraw_to_mirrored_foundry_tiles() {
    let (width, height) = (30, 20);
    let mut rows = vec![vec!['.'; width]; height];
    rows.first_mut().expect("map has a north edge").fill('#');
    rows.last_mut().expect("map has a south edge").fill('#');
    for row in &mut rows {
        row[0] = '#';
        row[width - 1] = '#';
    }
    let left_foundry = TilePos::new(4, 4);
    let right_foundry = TilePos::new(24, 14);
    rows[left_foundry.y as usize][left_foundry.x as usize] = '1';
    rows[right_foundry.y as usize][right_foundry.x as usize] = '2';
    let left_unit_tile = TilePos::new(8, 6);
    let right_unit_tile = TilePos::new(21, 13);
    let scenario = Scenario {
        mode: Default::default(),
        name: "mirrored wounded withdrawal".into(),
        seed: 1_616_101,
        map: rows
            .into_iter()
            .map(|row| row.into_iter().collect())
            .collect(),
        players: vec![
            player_spec("West Ferrous", Faction::Ferrous, 0, None),
            player_spec("East Ferrous", Faction::Ferrous, 0, None),
        ],
        units: vec![
            unit_spec(0, UnitKind::Sentinel, left_unit_tile.x, left_unit_tile.y),
            unit_spec(1, UnitKind::Sentinel, right_unit_tile.x, right_unit_tile.y),
        ],
        buildings: Vec::new(),
        meta: None,
    };
    let mut state = scenario.build().expect("the mirrored withdrawal builds");
    let wounded_hp = UnitKind::Sentinel.stats().max_hp / 4;
    crate::test_support::edit_units(&mut state, |units| {
        for unit in units {
            unit.hp = wounded_hp;
        }
    });
    let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 1_616_201);
    let public_map = public_map(&scenario);
    let mut brains = [
        SeatBot::scripted(PlayerId(0), config, Arc::clone(&public_map)),
        SeatBot::scripted(PlayerId(1), config, public_map),
    ];
    for (index, brain) in brains.iter_mut().enumerate() {
        let (player, staging) = if index == 0 {
            (PlayerId(0), left_unit_tile)
        } else {
            (PlayerId(1), right_unit_tile)
        };
        let obs = Observation::fog_honest(&state, player);
        let formed = brain.exec.apply_with_reservations(
            player,
            &obs,
            &[Intent::FormArmy { staging, size: 1 }],
            &[],
        );
        assert!(
            formed.iter().any(|command| matches!(
                &command.command,
                Command::AttackMove { units, goal, queue: false }
                    if units.len() == 1 && *goal == staging
            )),
            "the wounded machine must begin inside a real army"
        );
    }

    let withdrawal_goal = |brain: &mut SeatBot, player: PlayerId| {
        let commands = brain.act(&state);
        commands
            .iter()
            .find_map(|command| match &command.command {
                Command::Move {
                    units,
                    goal,
                    queue: false,
                } if units.iter().any(|unit| {
                    state
                        .unit(*unit)
                        .is_some_and(|member| member.player == player)
                }) =>
                {
                    Some(*goal)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("{player} did not withdraw its wounded army: {commands:?}"))
    };
    let left_goal = withdrawal_goal(&mut brains[0], PlayerId(0));
    let right_goal = withdrawal_goal(&mut brains[1], PlayerId(1));
    let foundry_size = BuildingKind::Foundry.base_stats().size;
    let inside = |goal: TilePos, anchor: TilePos| {
        goal.x >= anchor.x
            && goal.x < anchor.x + foundry_size.0
            && goal.y >= anchor.y
            && goal.y < anchor.y + foundry_size.1
    };
    assert!(inside(left_goal, left_foundry));
    assert!(inside(right_goal, right_foundry));
    assert_eq!(left_goal, left_foundry);
    assert_eq!(
        TilePos::new(
            width as i32 - 1 - left_goal.x,
            height as i32 - 1 - left_goal.y
        ),
        right_goal
    );
}

#[test]
fn a_finished_brain_emits_nothing_and_preserves_all_controller_memory() {
    let scenario = Scenario::skirmish();
    let mut state = scenario.build().expect("the skirmish builds");
    let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 20_042);
    let mut brain = scripted_brain(&scenario, PlayerId(0), config);

    let _ = brain.act(&state);
    assert_eq!(
        brain.mind().intelligence.observed_at(),
        Some(0),
        "the fixture must populate real strategic memory before the match ends"
    );
    state.tick(&[PlayerCommand {
        player: PlayerId(1),
        command: Command::Surrender,
    }]);
    assert!(state.result().is_some());
    let before = brain.clone();

    assert!(brain.act(&state).is_empty());
    assert_brain_unchanged(&before, &brain);
}

#[test]
fn every_real_difficulty_thinks_only_on_its_authored_cadence() {
    for difficulty in BotDifficulty::ALL {
        let scenario = Scenario::skirmish();
        let mut state = scenario.build().expect("the skirmish builds");
        let config = BotConfig::scripted(difficulty, BotStance::Balanced, 20_042);
        let mut brain = scripted_brain(&scenario, PlayerId(0), config);
        let cadence = DifficultyTuning::for_level(difficulty).cadence;
        assert_eq!(brain.dials.cadence, cadence);

        for tick in 0..=crate::difficulty::STRATEGIC_ADMISSION_CADENCE * 2 {
            assert_eq!(state.current_tick(), tick);
            let before = brain.clone();
            let commands = brain.act(&state);
            if tick.is_multiple_of(cadence) {
                assert_eq!(
                    brain.mind().intelligence.observed_at(),
                    Some(tick),
                    "{difficulty:?} did not observe on its cadence"
                );
            } else {
                assert!(commands.is_empty(), "{difficulty:?} acted at tick {tick}");
                assert_brain_unchanged(&before, &brain);
            }
            state.tick(&[]);
        }
    }
}

#[test]
fn prime_alone_directs_overlapping_defenses_through_an_accepted_player_command() {
    let scenario = prime_defense_focus_scenario();
    let state = scenario.build().expect("the focus scenario builds");
    let prime_config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 20_042);
    let veteran_config = BotConfig::scripted(BotDifficulty::Veteran, BotStance::Balanced, 20_042);
    let mut prime = scripted_brain(&scenario, PlayerId(0), prime_config);
    let mut veteran = scripted_brain(&scenario, PlayerId(0), veteran_config);

    let prime_commands = prime.act(&state);
    let focus = prime_commands
        .iter()
        .find(|command| matches!(command.command, Command::FocusFire { .. }))
        .cloned()
        .expect("Prime should direct the overlapping turret line");
    assert!(
        veteran
            .act(&state)
            .iter()
            .all(|command| !matches!(command.command, Command::FocusFire { .. })),
        "Veteran should retain ordinary static-defense acquisition"
    );

    let (defenses, target) = match &focus.command {
        Command::FocusFire { buildings, target } => (buildings.clone(), *target),
        _ => unreachable!(),
    };
    assert_eq!(defenses.len(), 2);
    assert!(defenses.windows(2).all(|pair| pair[0] < pair[1]));
    let mut applied = state.clone();
    let report = applied.tick(&[focus]);
    assert!(
        report
            .events
            .iter()
            .all(|event| !matches!(event, oxide_sim::Event::CommandRejected { .. }))
    );
    for defense in defenses {
        assert_eq!(
            applied.building(defense).expect("defense stands").focus,
            applied.attack_objective(PlayerId(0), target)
        );
    }
}

#[test]
fn prime_defense_focus_is_unchanged_by_hidden_authoritative_unit_state() {
    let scenario = prime_defense_focus_scenario();
    let state = scenario.build().expect("the focus scenario builds");
    let mut counterfactual = state.clone();
    let hidden = counterfactual
        .units()
        .iter()
        .find(|unit| unit.tile() == TilePos::new(24, 4))
        .expect("the hidden counterfactual unit exists")
        .id;
    crate::test_support::edit_units(&mut counterfactual, |units| {
        units.iter_mut().find(|unit| unit.id == hidden).unwrap().hp = 1
    });
    assert_eq!(
        Observation::fog_honest(&state, PlayerId(0)),
        Observation::fog_honest(&counterfactual, PlayerId(0)),
        "the fixture mutation must remain outside the bot's knowledge"
    );

    let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 20_042);
    let mut baseline = scripted_brain(&scenario, PlayerId(0), config);
    let mut changed = scripted_brain(&scenario, PlayerId(0), config);
    let baseline_act = baseline.act_traced(&state);
    let changed_act = changed.act_traced(&counterfactual);
    assert_eq!(baseline_act.commands, changed_act.commands);
    assert_eq!(baseline_act.trace, changed_act.trace);
    assert_eq!(
        serde_json::to_string(&baseline_act.trace).expect("the baseline trace serializes"),
        serde_json::to_string(&changed_act.trace).expect("the counterfactual trace serializes"),
        "a decision trace must not expose authoritative facts absent from the fog-honest observation"
    );
    assert_brain_unchanged(&baseline, &changed);
}

pub(super) fn remote_expansion_defense_scenario() -> Scenario {
    let mut rows = vec![vec!['.'; 40]; 24];
    rows.first_mut().expect("map has a north edge").fill('#');
    rows.last_mut().expect("map has a south edge").fill('#');
    for row in &mut rows {
        row[0] = '#';
        row[39] = '#';
    }
    rows[5][5] = '1';
    rows[18][33] = '2';

    Scenario {
        mode: Default::default(),
        name: "remote expansion defense".into(),
        seed: 0x0A16_0DEF,
        map: rows
            .into_iter()
            .map(|row| row.into_iter().collect())
            .collect(),
        players: vec![
            player_spec("West Ferrous", Faction::Ferrous, 300, None),
            player_spec("East Ferrous", Faction::Ferrous, 300, None),
        ],
        units: vec![
            unit_spec(0, UnitKind::Sentinel, 8, 7),
            unit_spec(0, UnitKind::Sentinel, 9, 7),
            unit_spec(1, UnitKind::Sentinel, 27, 13),
        ],
        buildings: vec![building_spec(0, BuildingKind::Foundry, 24, 12)],
        meta: None,
    }
}

pub(super) fn prime_defense_focus_scenario() -> Scenario {
    let mut rows = vec![vec!['.'; 30]; 18];
    rows.first_mut().expect("map has a north edge").fill('#');
    rows.last_mut().expect("map has a south edge").fill('#');
    for row in &mut rows {
        row[0] = '#';
        row[29] = '#';
    }
    rows[8][4] = '1';
    rows[8][24] = '2';

    Scenario {
        mode: Default::default(),
        name: "prime defense focus".into(),
        seed: 0x0A16_DEF0,
        map: rows
            .into_iter()
            .map(|row| row.into_iter().collect())
            .collect(),
        players: vec![
            player_spec("West Ferrous", Faction::Ferrous, 0, None),
            player_spec("East Cupric", Faction::Cupric, 0, None),
        ],
        units: vec![
            unit_spec(1, UnitKind::Sentinel, 12, 8),
            unit_spec(1, UnitKind::Sentinel, 12, 10),
            unit_spec(1, UnitKind::Sentinel, 24, 4),
        ],
        buildings: vec![
            building_spec(0, BuildingKind::Turret, 8, 7),
            building_spec(0, BuildingKind::Turret, 8, 11),
        ],
        meta: None,
    }
}
