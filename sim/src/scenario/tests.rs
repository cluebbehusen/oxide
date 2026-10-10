use super::*;

#[test]
fn skirmish_builds_and_is_deterministic() {
    let scenario = Scenario::skirmish();
    let a = scenario.build().unwrap();
    let b = scenario.build().unwrap();
    assert_eq!(a.hash(), b.hash());
    assert_eq!(a.players.len(), 2);
    assert_eq!(a.buildings.len(), 2);
    assert_eq!(a.units.len(), 8);
    assert!(a.players.iter().all(|p| p.scrap > 0));
}

#[test]
fn retint_swaps_roster_name_and_faction_bound_kinds() {
    let mut scenario = Scenario::skirmish();
    // A faction-bound starter proves the role remap.
    scenario.units.push(UnitSpec {
        player: 1,
        kind: UnitKind::Stinger,
        x: 3,
        y: 3,
    });
    let old_name = scenario.players[1].name.clone();
    assert_eq!(scenario.players[1].faction, Faction::Cupric);
    scenario.retint_seat(1, Faction::Ferrous);
    assert_eq!(scenario.players[1].faction, Faction::Ferrous);
    assert_ne!(
        scenario.players[1].name, old_name,
        "a faction-derived name follows the roster"
    );
    assert!(
        scenario
            .units
            .iter()
            .filter(|u| u.player == 1)
            .all(|u| u.kind.faction() != Some(Faction::Cupric)),
        "no seat keeps the other roster's kinds"
    );
    assert!(
        scenario.units.iter().any(|u| u.kind == UnitKind::Flakhound),
        "the stinger crossed to its ferrous role twin"
    );
    // Same faction again: a no-op, not a name churn.
    let name = scenario.players[1].name.clone();
    scenario.retint_seat(1, Faction::Ferrous);
    assert_eq!(scenario.players[1].name, name);
    scenario.build().expect("a retinted scenario still builds");
}

#[test]
fn player_count_bounds_are_enforced_and_named() {
    let mut scenario = Scenario::skirmish();
    scenario.players.clear();
    let empty = scenario.build();
    assert!(matches!(empty, Err(ScenarioError::PlayerCount(0))));

    let mut crowded = Scenario::skirmish();
    while crowded.players.len() < 17 {
        crowded.players.push(crowded.players[0].clone());
    }
    let err = crowded.build().expect_err("seventeen seats must refuse");
    assert!(matches!(err, ScenarioError::PlayerCount(17)));
    assert!(
        err.to_string().contains("1 to 16"),
        "the message must name the real bound, got: {err}"
    );
}

#[test]
fn a_blocked_foundry_footprint_is_an_error() {
    // The anchor byte sits on open ground but its 2x2 footprint
    // reaches into border rock: the scenario must refuse rather
    // than stand a Foundry inside a wall.
    let mut scenario = Scenario::skirmish();
    let last = scenario.map.len() - 2;
    let row = scenario.map[last].clone();
    let inner = row.trim_matches('#').len() + row.len() - row.trim_start_matches('#').len() - 1;
    let _ = inner;
    // Move player 0's anchor to the last interior column, so the
    // footprint's second column lands on the border.
    for line in &mut scenario.map {
        *line = line.replace('1', ".");
    }
    let width = scenario.map[1].len();
    let mut edge_row: Vec<char> = scenario.map[1].chars().collect();
    edge_row[width - 2] = '1';
    scenario.map[1] = edge_row.into_iter().collect();
    assert!(matches!(
        scenario.build(),
        Err(ScenarioError::BadFootprint(PlayerId(0), _))
    ));
}

#[test]
fn missing_anchor_is_an_error() {
    let mut scenario = Scenario::skirmish();
    scenario.players.push(PlayerSpec {
        name: "third".into(),
        faction: Faction::Ferrous,
        team: None,
        scrap: 0,
        bot: false,
        bot_config: None,
    });
    assert!(matches!(
        scenario.build(),
        Err(ScenarioError::MissingAnchor(PlayerId(2)))
    ));
}

#[test]
fn authored_structures_stand_built_and_validate_their_ground() {
    let mut scenario = Scenario::skirmish();
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Turret,
        x: 9,
        y: 5,
    });
    let state = scenario.build().unwrap();
    let turret = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Turret)
        .expect("the authored turret stands");
    assert!(turret.built(), "at full strength from tick zero");
    assert_eq!(turret.hp, BuildingKind::Turret.base_stats().max_hp);

    // The same anchor twice: the second footprint reads occupied.
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Turret,
        x: 9,
        y: 5,
    });
    assert!(matches!(
        scenario.build(),
        Err(ScenarioError::BadBuilding(1))
    ));
    // Border rock refuses a footprint outright.
    scenario.buildings.clear();
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Turret,
        x: 0,
        y: 0,
    });
    assert!(matches!(
        scenario.build(),
        Err(ScenarioError::BadBuilding(0))
    ));
}

#[test]
fn bot_config_leaves_defaults_off_the_wire_and_refuses_unknown_fields() {
    let configured = BotConfig::new(BotDifficulty::Veteran, BotStance::Turtle, 42);
    let json = serde_json::to_string(&configured).unwrap();
    assert_eq!(
        json,
        r#"{"difficulty":"veteran","stance":"turtle","personality_seed":42}"#
    );
    assert_eq!(
        serde_json::from_str::<BotConfig>(&json).unwrap(),
        configured
    );
    assert_eq!(serde_json::to_string(&BotConfig::default()).unwrap(), "{}");
    assert_eq!(
        serde_json::from_str::<BotConfig>("{}").unwrap(),
        BotConfig::new(BotDifficulty::Standard, BotStance::Balanced, 0)
    );
    for rejected in [r#"{"controller":"opponent"}"#, r#"{"difficulty":"Prime"}"#] {
        assert!(
            serde_json::from_str::<BotConfig>(rejected).is_err(),
            "{rejected} must be refused"
        );
    }
}

#[test]
fn misplaced_unit_is_an_error() {
    let mut scenario = Scenario::skirmish();
    scenario.units.push(UnitSpec {
        player: 0,
        kind: UnitKind::Harvester,
        x: 0,
        y: 0, // border rock
    });
    assert!(matches!(scenario.build(), Err(ScenarioError::BadUnit(_))));
}
