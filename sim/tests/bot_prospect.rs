//! The harvest starvation ladder (0.12): when nothing harvestable is
//! known, the neural bot's economy chore escalates — contested nodes
//! first, then one prospector walking the sweep legs — instead of
//! letting the economy flatline on a dry home patch. The scripted
//! tiers never climb the ladder; these tests drive the `GymBot`
//! directly, the way the neural wrapper does.

use oxide_sim::bot::{Action, FEATURE_NAMES, GymBot};
use oxide_sim::{PlayerId, Scenario};

/// A starvation box: a one-tile home patch (400 scrap), and the only
/// other scrap 14 Chebyshev away — far outside the sim's radius-2
/// retarget hop — and deeper into the enemy's half than the bot's own, so the
/// normal economy filter refuses it even once seen. Reaching it takes
/// the whole ladder: home dries, the line starves, the prospector's
/// centre leg walks into vision of the patch, and the contested-node
/// rung assigns it.
fn starvation_box() -> Scenario {
    let map = [
        "################################",
        "#..............................#",
        "#..............................#",
        "#..1..s........................#",
        "#..............................#",
        "#..............................#",
        "#..............................#",
        "#..............................#",
        "#..............................#",
        "#..............................#",
        "#..............................#",
        "#..............................#",
        "#...................ss.........#",
        "#...................ss.........#",
        "#..............................#",
        "#..........................2...#",
        "#..............................#",
        "#..............................#",
        "#..............................#",
        "################################",
    ];
    let json = serde_json::json!({
        "name": "Starvation Box",
        "seed": 7,
        "players": [
            {"name": "Prospector", "faction": "ferrous", "scrap": 0, "bot": false},
            {"name": "Idle", "faction": "cupric", "scrap": 0, "bot": false}
        ],
        "map": map,
        "units": [
            {"player": 0, "kind": "harvester", "x": 6, "y": 4},
            {"player": 0, "kind": "harvester", "x": 7, "y": 4}
        ]
    });
    Scenario::from_json(&json.to_string()).expect("the starvation box parses")
}

#[test]
fn the_starvation_ladder_reaches_far_contested_scrap() {
    let scenario = starvation_box();
    let mut state = scenario.build().expect("the starvation box builds");
    let mut gym = GymBot::new(PlayerId(0));
    for _ in 0..30_000u32 {
        let commands = if state.current_tick().is_multiple_of(16) {
            gym.step(&state, Action::Idle)
        } else {
            Vec::new()
        };
        state.tick(&commands);
    }
    let scrap_at = |x: i32, y: i32| {
        state
            .map()
            .tile(chassis::grid::TilePos::new(x, y))
            .map_or(0, |t| t.scrap)
    };
    assert_eq!(
        scrap_at(6, 3),
        0,
        "the home patch must actually run dry — otherwise this test \
         proves nothing about starvation"
    );
    let far_left = scrap_at(20, 12) + scrap_at(21, 12) + scrap_at(20, 13) + scrap_at(21, 13);
    assert!(
        far_left < 4 * 400,
        "the far contested patch was never touched ({far_left} of 1600 \
         left): the starvation ladder failed to reach it"
    );
}

#[test]
fn prospecting_never_stamps_the_intel_age_feature() {
    let idx = FEATURE_NAMES
        .iter()
        .position(|n| *n == "intel_age")
        .expect("intel_age is a gym feature");
    let scenario = starvation_box();
    let mut state = scenario.build().expect("the starvation box builds");
    let mut gym = GymBot::new(PlayerId(0));
    for _ in 0..20_000u32 {
        let commands = if state.current_tick().is_multiple_of(16) {
            let decision = gym.decision(&state);
            assert_eq!(
                decision.features[idx],
                10_000,
                "intel_age moved at tick {} — something scouted; the \
                 prospector must never stamp scouted_at (the feature is \
                 trained against Action::Scout, not against chores)",
                state.current_tick()
            );
            gym.step(&state, Action::Idle)
        } else {
            Vec::new()
        };
        state.tick(&commands);
    }
}

/// A scrap-free yard: nothing harvestable anywhere, so the ladder's
/// rung 2 wants a prospector every think — while the chosen action
/// spends a harvester the executive picks only at lowering time. The
/// two must never pick the same machine.
fn barren_yard(harvesters: usize) -> Scenario {
    let units: Vec<_> = (0..harvesters)
        .map(|i| serde_json::json!({"player": 0, "kind": "harvester", "x": 5 + i as i32, "y": 4}))
        .collect();
    Scenario::from_json(
        &serde_json::json!({
            "name": "Barren Yard",
            "seed": 5,
            "players": [
                {"name": "Digger", "faction": "ferrous", "scrap": 500, "bot": false},
                {"name": "Idle", "faction": "cupric", "scrap": 0, "bot": false}
            ],
            "map": [
                "################",
                "#..............#",
                "#..............#",
                "#..1...........#",
                "#..............#",
                "#............2.#",
                "#..............#",
                "################"
            ],
            "units": units,
            "buildings": [
                {"player": 0, "kind": "turret", "x": 10, "y": 2}
            ]
        })
        .to_string(),
    )
    .expect("the barren yard parses")
}

#[test]
fn the_prospector_never_strips_the_actions_builder() {
    use oxide_sim::Command;
    for harvesters in 1..=3 {
        let state = barren_yard(harvesters)
            .build()
            .expect("the barren yard builds");
        let mut gym = GymBot::new(PlayerId(0));
        let commands = gym.step(&state, Action::BuildFabricator);
        let builder = commands
            .iter()
            .find_map(|pc| match &pc.command {
                Command::Build { units, .. } => units.first().copied(),
                _ => None,
            })
            .expect("the action staged its site");
        let stripped = commands.iter().any(|pc| match &pc.command {
            Command::Move { units, .. } => units.contains(&builder),
            _ => false,
        });
        assert!(
            !stripped,
            "{harvesters} harvesters: the prospecting sweep replaced the \
             builder's order — the site is paid for and orphaned: {commands:?}"
        );
    }
}

#[test]
fn the_paid_site_is_actually_under_construction_next_tick() {
    use oxide_sim::Order;
    let mut state = barren_yard(1).build().expect("builds");
    let mut gym = GymBot::new(PlayerId(0));
    let commands = gym.step(&state, Action::BuildFabricator);
    state.tick(&commands);
    let site = state
        .buildings()
        .iter()
        .find(|b| !b.built)
        .expect("the paid site stands")
        .id;
    assert!(
        state
            .units()
            .iter()
            .any(|u| matches!(u.order, Order::Build { site: s } if s == site)),
        "the only harvester walked off to prospect instead of building \
         the site its own action paid for"
    );
}

#[test]
fn the_prospector_never_strips_the_salvage_crew() {
    use oxide_sim::Command;
    let state = barren_yard(1).build().expect("builds");
    let mut gym = GymBot::new(PlayerId(0));
    let commands = gym.step(&state, Action::Salvage);
    let crew = commands
        .iter()
        .find_map(|pc| match &pc.command {
            Command::Salvage { units, .. } => units.first().copied(),
            _ => None,
        })
        .expect("the action staged its strip");
    let stripped = commands.iter().any(|pc| match &pc.command {
        Command::Move { units, .. } => units.contains(&crew),
        _ => false,
    });
    assert!(
        !stripped,
        "the prospecting sweep replaced the salvage order: {commands:?}"
    );
}
