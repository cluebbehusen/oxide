use super::*;

pub(super) fn public_map(scenario: &Scenario) -> Arc<PublicMapBriefing> {
    Arc::new(
        PublicMapBriefing::from_scenario(scenario)
            .expect("the focused scenario has a public map briefing"),
    )
}

pub(super) fn scripted_brain(scenario: &Scenario, player: PlayerId, config: BotConfig) -> SeatBot {
    SeatBot::scripted(player, config, public_map(scenario))
}

pub(super) fn operation_identity_brain(player: PlayerId, scenario: &Scenario) -> SeatBot {
    let difficulty = BotDifficulty::Prime;
    let config = BotConfig::scripted(difficulty, BotStance::Balanced, 20_024);
    let brain = scripted_brain(scenario, player, config);
    let profile = brain.profile();
    assert_eq!(
        (profile.primary, profile.secondary),
        (Specialty::Guile, Specialty::Fortification)
    );
    assert_eq!((profile.traits.air, profile.traits.guile), (40, 74));
    assert_eq!(
        brain.dials(),
        &Dials::scripted(profile, DifficultyTuning::for_level(difficulty)),
        "the public config must resolve the profile and matching dials together"
    );
    brain
}

pub(super) fn enlist_opening_core(brain: &mut SeatBot, state: &State) {
    let obs = Observation::fog_honest(state, PlayerId(0));
    let core: Vec<_> = obs
        .my_units
        .iter()
        .filter(|unit| unit.kind == UnitKind::Sentinel)
        .collect();
    assert_eq!(core.len(), 8);
    let staging = TilePos::new(
        core.iter().map(|unit| unit.tile.x).sum::<i32>() / 8,
        core.iter().map(|unit| unit.tile.y).sum::<i32>() / 8,
    );
    let _ = brain.exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy { staging, size: 8 }],
        &[],
    );
    let enlisted: Vec<_> = brain.exec.enlisted().collect();
    assert_eq!(enlisted.len(), 8);
    assert!(enlisted.iter().all(|id| {
        obs.my_units
            .iter()
            .any(|unit| unit.id == *id && unit.kind == UnitKind::Sentinel)
    }));
}

pub(super) fn assert_brain_unchanged(before: &SeatBot, after: &SeatBot) {
    assert_eq!(after.player, before.player);
    assert_eq!(after.dials, before.dials);
    assert_eq!(after.mind, before.mind);
    assert_eq!(after.policy, before.policy);
    assert_eq!(after.exec, before.exec);
    assert_eq!(after.orientation, before.orientation);
}

pub(super) fn opening_core_team_relief_scenario() -> Scenario {
    let mut rows = vec![vec!['.'; 40]; 24];
    rows.first_mut().expect("map has a north edge").fill('#');
    rows.last_mut().expect("map has a south edge").fill('#');
    for row in &mut rows {
        row[0] = '#';
        row[39] = '#';
    }
    rows[10][3] = '1';
    rows[10][24] = '2';
    rows[10][36] = '3';

    Scenario {
        mode: Default::default(),
        name: "brain opening-core team-relief admission".into(),
        seed: 0x0A16_7EA1,
        map: rows
            .into_iter()
            .map(|row| row.into_iter().collect())
            .collect(),
        players: vec![
            player_spec("Ferrous", Faction::Ferrous, 0, Some(0)),
            player_spec("Cupric ally", Faction::Cupric, 0, Some(0)),
            player_spec("Cupric enemy", Faction::Cupric, 0, Some(1)),
        ],
        units: core::iter::once(unit_spec(0, UnitKind::Harvester, 8, 12))
            .chain(
                (0..8).map(|index| unit_spec(0, UnitKind::Sentinel, 3 + index % 4, 15 + index / 4)),
            )
            .chain(core::iter::once(unit_spec(2, UnitKind::Sentinel, 28, 10)))
            .collect(),
        buildings: Vec::new(),
        meta: None,
    }
}

pub(super) fn prospective_lift_reservation_scenario() -> Scenario {
    let mut scenario = bulk_lift_capacity_scenario();
    scenario.units.retain(|unit| unit.kind != UnitKind::Kestrel);
    scenario
        .units
        .extend([2, 11, 21].map(|y| unit_spec(0, UnitKind::Kestrel, 18, y)));
    scenario
        .units
        .push(unit_spec(1, UnitKind::Harvester, 18, 20));
    scenario.buildings.extend([
        building_spec(0, BuildingKind::Foundry, 14, 3),
        building_spec(0, BuildingKind::Array, 14, 7),
    ]);
    scenario
}

pub(super) fn combined_operation_scenario() -> Scenario {
    let mut scenario = bulk_lift_capacity_scenario();
    scenario.players[0].scrap = 50_000;
    for unit in scenario
        .units
        .iter_mut()
        .filter(|unit| unit.kind == UnitKind::Sentinel)
    {
        unit.kind = UnitKind::Scuttler;
    }
    scenario.units.push(unit_spec(0, UnitKind::Kestrel, 14, 9));
    scenario.units.extend(
        (0..8).map(|index| unit_spec(0, UnitKind::Sentinel, 13 + index % 4, 21 + index / 4)),
    );
    scenario
        .buildings
        .push(building_spec(1, BuildingKind::Extractor, 14, 6));
    scenario
}

pub(super) fn foundry_saving_air_competition_scenario(scrap: u32) -> Scenario {
    let width = 48usize;
    let height = 24usize;
    let mut rows = vec![vec!['.'; width]; height];
    rows.first_mut().expect("map has a north edge").fill('#');
    rows.last_mut().expect("map has a south edge").fill('#');
    for row in &mut rows {
        row[0] = '#';
        row[width - 1] = '#';
    }
    rows[1][1] = '1';
    rows[20][44] = '2';
    rows[16][30] = 'E';

    let mut units: Vec<_> = (0..4)
        .map(|index| unit_spec(0, UnitKind::Harvester, 5 + index, 11))
        .collect();
    units.extend(
        [
            (4, 4),
            (7, 9),
            (10, 10),
            (13, 11),
            (16, 12),
            (19, 13),
            (22, 14),
            (25, 15),
            (27, 17),
            (28, 14),
            (24, 12),
            (20, 10),
        ]
        .into_iter()
        .map(|(x, y)| unit_spec(0, UnitKind::Sentinel, x, y)),
    );
    units.push(unit_spec(0, UnitKind::Kestrel, 9, 18));

    Scenario {
        mode: Default::default(),
        name: "accepted Foundry saving competes with connected air".into(),
        seed: 1_616_305,
        map: rows
            .into_iter()
            .map(|row| row.into_iter().collect())
            .collect(),
        players: vec![
            player_spec("West Ferrous", Faction::Ferrous, scrap, None),
            player_spec("East Cupric", Faction::Cupric, 0, None),
        ],
        units,
        buildings: vec![
            building_spec(0, BuildingKind::Fabricator, 5, 2),
            building_spec(0, BuildingKind::Airworks, 1, 6),
            building_spec(0, BuildingKind::Crucible, 5, 6),
            building_spec(0, BuildingKind::Foundry, 11, 1),
            building_spec(0, BuildingKind::Extractor, 30, 16),
        ],
        meta: None,
    }
}

pub(super) fn foundry_competition_brain(scenario: &Scenario) -> SeatBot {
    let profile = foundry_competition_profile();
    let mut brain = scripted_brain(
        scenario,
        PlayerId(0),
        BotConfig::scripted(profile.difficulty, profile.stance, profile.personality_seed),
    );
    brain.dials = Dials::scripted(&profile, DifficultyTuning::for_level(profile.difficulty));
    brain.dials.harvester_target = 4;
    brain.dials.army_size = 100;

    let mind = brain.mind_mut();
    mind.profile = profile;
    brain
}

pub(super) fn seeded_bulk_lift(state: &State, orientation: Orientation) -> LiftPlanner {
    let raw = Observation::fog_honest(state, PlayerId(0));
    let oriented = orientation.observe(&raw);
    let home = oriented
        .my_buildings
        .iter()
        .filter(|building| building.kind == BuildingKind::Foundry)
        .min_by_key(|building| building.id)
        .expect("the lift fixture retains its home Foundry")
        .anchor;
    let mut planner = LiftPlanner::new();
    let decision = planner.think_unrestricted(&oriented, home, &[], LiftAirSupport::Independent);
    assert!(
        planner.operation().is_some(),
        "the bulk lift must be admissible"
    );
    assert!(
        decision.intents.iter().any(|intent| matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Skyhook,
                ..
            }
        )),
        "the seeded lift must still need one carrier: operation={:?}, intents={:?}",
        planner.operation(),
        decision.intents
    );
    planner
}

pub(super) fn independent_bomber_operation_scenario() -> Scenario {
    let mut scenario = bulk_lift_capacity_scenario();
    scenario.players[0].scrap = 0;
    scenario.units.clear();
    scenario
        .units
        .extend((0..6).map(|index| unit_spec(0, UnitKind::Condor, 3 + index, 15)));
    scenario
        .units
        .extend((0..6).map(|index| unit_spec(0, UnitKind::Buzzard, 9 + index, 15)));
    scenario.units.push(unit_spec(0, UnitKind::Kestrel, 31, 11));
    scenario.buildings.extend([
        building_spec(0, BuildingKind::Reclaimer, 14, 3),
        building_spec(0, BuildingKind::Reclaimer, 17, 3),
    ]);
    scenario
}

pub(super) fn prior_foundry_sighting(
    raw: &Observation,
    foundry: &oxide_sim::Building,
    last_seen: u64,
) -> Observation {
    let mut prior = raw.clone();
    prior.tick = last_seen;
    prior.enemy_buildings.push(BuildingObs {
        hp: foundry.hp,
        built: foundry.built,
        tier: foundry.tier,
        ..crate::test_support::building(foundry.id.0, foundry.player, foundry.kind, foundry.anchor)
    });
    prior
}
