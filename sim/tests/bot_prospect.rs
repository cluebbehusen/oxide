//! The harvest starvation ladder (0.12): when nothing harvestable is
//! known, the neural bot's economy chore escalates — contested nodes
//! first, then one prospector walking the sweep legs — instead of
//! letting the economy flatline on a dry home patch. The scripted
//! tiers never climb the ladder; these tests drive the `GymBot`
//! directly, the way the neural wrapper does.

use oxide_sim::bot::{Action, FEATURE_NAMES, GymBot};
use oxide_sim::{PlayerId, Scenario};

/// A starvation box: a one-tile home patch (400 scrap), and the only
/// other scrap 14 Chebyshev away — outside the sim's 10-tile retarget
/// hop — and deeper into the enemy's half than the bot's own, so the
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
