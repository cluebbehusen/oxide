use super::*;
use crate::decision::Producer;
use crate::frame::HomeFrame;
use crate::profile::ResolvedProfile;

/// The field with a scrap node beside West's Foundry and another at `far`.
fn fielded(far: (i32, i32)) -> Scenario {
    let mut scenario = field();
    for (x, y) in [(6, 11), far] {
        scenario.map[y as usize].replace_range(x as usize..x as usize + 1, "s");
    }
    scenario
}

/// West's worked nodes with their crews in `state`, at `stance` and `greed`.
fn crews(
    scenario: &Scenario,
    state: &State,
    stance: BotStance,
    greed: u8,
) -> Vec<(TilePos, usize)> {
    let observation = ObservationData::fog_honest(state, PlayerId(0));
    let model = map(scenario);
    let frame = HomeFrame::of(&observation, &model).unwrap();
    let mut profile =
        ResolvedProfile::resolve(BotConfig::opponent(BotDifficulty::Standard, stance, 11));
    profile.traits.greed = greed;
    let foundries: Vec<Producer<'_>> = observation
        .my_buildings
        .iter()
        .filter(|building| building.kind == BuildingKind::Foundry && building.built)
        .map(|building| Producer {
            building,
            ready: true,
        })
        .collect();
    crate::workers::staffing(&observation, &model, frame, &profile, &foundries)
        .crews()
        .to_vec()
}

fn crew(crews: &[(TilePos, usize)], node: (i32, i32)) -> Option<usize> {
    crews
        .iter()
        .find(|(worked, _)| *worked == TilePos::new(node.0, node.1))
        .map(|(_, crew)| *crew)
}

#[test]
fn a_node_beside_the_foundry_takes_more_than_two_and_a_far_one_waits_for_a_long_horizon() {
    let mut scenario = fielded((22, 3));
    // A Kestrel has seen the far node.
    scenario.units.push(unit(0, UnitKind::Kestrel, 21, 4));
    let state = scenario.build().unwrap();
    let balanced = crews(&scenario, &state, BotStance::Balanced, 50);
    assert!(
        crew(&balanced, (6, 11)).is_some_and(|crew| crew > 2),
        "{balanced:?}"
    );
    let hasty = crews(&scenario, &state, BotStance::Aggressive, 0);
    assert!(crew(&hasty, (6, 11)).is_some(), "{hasty:?}");
    assert_eq!(
        crew(&hasty, (22, 3)),
        None,
        "too far to repay a Harvester within an aggressive horizon"
    );
    assert!(
        crew(&balanced, (22, 3)).is_some(),
        "but within a balanced one: {balanced:?}"
    );
}

#[test]
fn an_economic_seat_fills_its_nodes_fuller_than_an_aggressive_one() {
    let scenario = fielded((10, 11));
    let state = scenario.build().unwrap();
    let wanted = |stance, greed| -> usize {
        crews(&scenario, &state, stance, greed)
            .iter()
            .map(|(_, crew)| crew)
            .sum()
    };
    assert!(wanted(BotStance::Turtle, 50) > wanted(BotStance::Aggressive, 50));
    assert!(wanted(BotStance::Balanced, 100) > wanted(BotStance::Balanced, 0));
}

#[test]
fn a_node_running_out_wants_no_more_workers_than_its_scrap_repays() {
    let scenario = fielded((10, 11));
    let mut state = scenario.build().unwrap();
    let node = TilePos::new(6, 11);
    let price = UnitKind::Harvester.stats().cost;
    let left = |state: &State| {
        ObservationData::fog_honest(state, PlayerId(0))
            .known_scrap
            .iter()
            .find(|(tile, _)| *tile == node)
            .map_or(0, |(_, amount)| *amount)
    };
    let full = crew(&crews(&scenario, &state, BotStance::Turtle, 100), (6, 11)).unwrap();
    // Mine the node down with a staged crew until only a few Harvesters'
    // worth of scrap is left.
    let mut spec = scenario.clone();
    spec.units.extend((5..=7).map(|y| harvester(0, 5, y + 5)));
    let mut state_mined = spec.build().unwrap();
    let miners: Vec<UnitId> = seat_units(&state_mined, PlayerId(0));
    state_mined.tick(&[PlayerCommand {
        player: PlayerId(0),
        command: Command::Harvest {
            units: miners,
            node,
            queue: false,
        },
    }]);
    while left(&state_mined) > 3 * price {
        assert!(
            state_mined.current_tick() < 20_000,
            "premise: the node is mined"
        );
        state_mined.tick(&[]);
    }
    state = state_mined;
    let repaid = (left(&state) / price) as usize;
    let crews = crews(&spec, &state, BotStance::Turtle, 100);
    let mined = crew(&crews, (6, 11)).unwrap_or(0);
    assert!(
        mined <= repaid.max(1) && mined < full,
        "{mined} of {full}, {repaid} repaid"
    );
}

#[test]
fn a_node_goes_to_the_nearest_foundry() {
    let mut scenario = fielded((22, 11));
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Foundry,
        x: 19,
        y: 11,
    });
    let state = scenario.build().unwrap();
    // Too far from the home Foundry for an aggressive horizon, but beside the
    // second one.
    let worked = crews(&scenario, &state, BotStance::Aggressive, 0);
    assert!(crew(&worked, (22, 11)).is_some(), "{worked:?}");
}

#[test]
fn memory_keeps_every_enemy_unit_in_sight_beyond_a_hundred_and_twenty_eight() {
    let mut scenario = field();
    for x in 8..=22 {
        for y in 4..=18 {
            scenario.units.push(unit(1, UnitKind::Sentinel, x, y));
        }
    }
    for (x, y) in [(10, 6), (10, 16), (20, 6), (20, 16)] {
        scenario.units.push(unit(0, UnitKind::Kestrel, x, y));
    }
    let state = scenario.build().unwrap();
    let seen = ObservationData::fog_honest(&state, PlayerId(0))
        .enemy_units
        .len();
    assert!(seen > 128, "premise: {seen} in sight");
    let mut opponent = seat(&scenario, 0);
    opponent.act(&state, &mut OwnEvents::default());
    let json = serde_json::to_value(opponent.checkpoint()).unwrap();
    assert_eq!(json["memory"]["units"].as_array().unwrap().len(), seen);
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
    assert!(Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).is_ok());
}

#[test]
fn an_allied_building_beside_a_node_takes_its_places_as_an_own_one_does() {
    let crews_beside = |owner: u8| {
        // A third seat far to the east, allied with West.
        let mut scenario = fielded((22, 3));
        scenario.map[3].replace_range(40..41, "3");
        let ally = scenario.players[1].clone();
        scenario.players.push(ally);
        for (player, team) in scenario.players.iter_mut().zip([0, 1, 0]) {
            player.team = Some(team);
        }
        for y in 10..=12 {
            scenario.buildings.push(BuildingSpec {
                player: owner,
                kind: BuildingKind::Barricade,
                x: 7,
                y,
            });
        }
        let state = scenario.build().unwrap();
        crew(&crews(&scenario, &state, BotStance::Turtle, 100), (6, 11))
    };
    let own = crews_beside(0);
    assert!(own.is_some(), "premise: the node is worked");
    assert_eq!(crews_beside(2), own);
}
