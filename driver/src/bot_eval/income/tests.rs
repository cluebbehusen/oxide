use super::*;
use oxide_sim::Scenario;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, ScenarioMode};
use oxide_sim::{Command, PlayerCommand, stats::HarvestStats};
use std::path::Path;

fn scenario(map: Vec<String>, buildings: Vec<BuildingSpec>) -> Scenario {
    Scenario {
        mode: ScenarioMode::Match,
        name: "income".into(),
        map,
        players: ["West", "East"]
            .into_iter()
            .map(|name| PlayerSpec {
                name: name.into(),
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            })
            .collect(),
        units: Vec::new(),
        buildings,
        meta: None,
    }
}

fn row(width: usize, marks: &[(usize, char)]) -> String {
    let mut row: Vec<char> = ".".repeat(width).chars().collect();
    for (x, mark) in marks {
        row[*x] = *mark;
    }
    row.into_iter().collect()
}

fn node_rate(distance: Fx) -> Fx {
    let HarvestStats {
        capacity,
        ticks_per_scrap,
    } = UnitKind::Harvester.stats().harvest.unwrap();
    Fx::from_num(HARVESTERS_PER_NODE * capacity * TICKS_PER_MINUTE)
        / (Fx::from_num(capacity * ticks_per_scrap)
            + distance * 2 / UnitKind::Harvester.stats().speed)
}

/// Summed node rates at distances given in tenths of a tile.
fn rates(tenths: [i64; 4]) -> u32 {
    tenths
        .into_iter()
        .map(|tenths| node_rate(Fx::from_num(tenths) / 10))
        .fold(Fx::ZERO, |sum, rate| sum + rate)
        .to_num::<u32>()
}

#[test]
fn saturation_counts_each_foundrys_nearest_remaining_nodes() {
    // Foundry 1's footprint spans x and y from 2 to 4. Its nodes sit 1.5,
    // 6.5, 7.5, 8.5 and 16.5 tiles beyond the east face; only the nearest
    // four count, and a mined-out node is skipped.
    let mut map = vec![".".repeat(30); 14];
    map[2] = row(
        30,
        &[
            (2, '1'),
            (10, 's'),
            (11, 's'),
            (12, 's'),
            (20, 's'),
            (26, '2'),
        ],
    );
    map[3] = row(30, &[(5, 's')]);
    let state = scenario(map.clone(), Vec::new()).build().unwrap();
    let expected = rates([15, 65, 75, 85]);
    assert!(expected > 0);
    assert_eq!(saturation_per_minute(&state, PlayerId(0)), expected);
    assert_eq!(
        passive_per_minute(&state, PlayerId(0)),
        0,
        "the drip has not started at tick zero"
    );

    map[3] = ".".repeat(30);
    let depleted = scenario(map, Vec::new()).build().unwrap();
    assert_eq!(
        saturation_per_minute(&depleted, PlayerId(0)),
        rates([65, 75, 85, 165])
    );
}

#[test]
fn two_foundries_never_count_one_node_twice() {
    let mut map = vec![".".repeat(30); 14];
    map[2] = row(30, &[(2, '1'), (10, 's'), (11, 's'), (26, '2')]);
    map[3] = row(30, &[(5, 's')]);
    map[9] = row(30, &[(14, 's')]);
    let expansion = BuildingSpec {
        player: 0,
        kind: BuildingKind::Foundry,
        x: 12,
        y: 6,
    };
    let state = scenario(map, vec![expansion]).build().unwrap();
    let foundries: Vec<&Building> = state
        .buildings()
        .iter()
        .filter(|building| building.player == PlayerId(0))
        .collect();
    assert_eq!(foundries.len(), 2);
    let nearest = |node: TilePos| {
        foundries
            .iter()
            .map(|foundry| node.center().dist(foundry.closest_point_to(node.center())))
            .min()
            .unwrap()
    };
    let expected = [(5, 3), (10, 2), (11, 2), (14, 9)]
        .into_iter()
        .map(|(x, y)| node_rate(nearest(TilePos::new(x, y))))
        .fold(Fx::ZERO, |sum, rate| sum + rate)
        .to_num::<u32>();
    assert_eq!(saturation_per_minute(&state, PlayerId(0)), expected);
}

#[test]
fn passive_rates_follow_reclaimer_tiers_extractors_and_the_drip() {
    let mut map = vec![".".repeat(30); 14];
    map[2] = row(30, &[(2, '1'), (26, '2')]);
    let buildings = vec![
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Reclaimer,
            x: 8,
            y: 8,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Reclaimer,
            x: 12,
            y: 8,
        },
    ];
    let mut value = serde_json::to_value(scenario(map, buildings).build().unwrap()).unwrap();
    let reclaimer = value["buildings"]
        .as_array()
        .unwrap()
        .iter()
        .rposition(|building| building["kind"] == "reclaimer")
        .unwrap();
    value["buildings"][reclaimer]["tier"] = 1.into();
    value["tick"] = FOUNDRY_DRIP_START_TICK.into();
    let state: State = serde_json::from_value(value).unwrap();
    assert_eq!(
        passive_per_minute(&state, PlayerId(0)),
        TICKS_PER_MINUTE / u32::try_from(RECLAIMER_PERIOD).unwrap()
            + TICKS_PER_MINUTE / u32::try_from(REFINERY_PERIOD).unwrap()
            + TICKS_PER_MINUTE / u32::try_from(FOUNDRY_DRIP_PERIOD).unwrap()
    );
    assert_eq!(
        passive_per_minute(&state, PlayerId(1)),
        TICKS_PER_MINUTE / u32::try_from(FOUNDRY_DRIP_PERIOD).unwrap()
    );
}

#[test]
fn checkpoints_compare_the_last_minute_with_the_estimate() {
    let mut state = Scenario::skirmish().build().unwrap();
    let mut tracker = IncomeTracker::new(&state, vec![true, false]);
    while state.current_tick() < INCOME_CHECKPOINTS[0] {
        let report = state.tick(&[]);
        tracker.observe(&state, &report.events, 12);
    }
    let samples = tracker.finish();
    assert!(samples[1].is_empty(), "seat one is unwatched");
    let [sample] = samples[0].as_slice() else {
        panic!("one checkpoint was reached: {samples:?}");
    };
    assert_eq!(sample.tick, INCOME_CHECKPOINTS[0]);
    assert_eq!(
        sample.saturation_per_minute,
        saturation_per_minute(&state, PlayerId(0))
    );
    let drip = TICKS_PER_MINUTE / u32::try_from(FOUNDRY_DRIP_PERIOD).unwrap();
    assert!(
        sample.actual_per_minute >= drip,
        "an idle seat still earns its drip: {sample:?}"
    );
}

#[test]
fn a_seat_out_of_a_team_match_stops_sampling() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scenarios/open-quarry.json");
    let mut state = Scenario::load(&path).unwrap().build().unwrap();
    let mut tracker = IncomeTracker::new(&state, vec![true; 4]);
    let surrender = PlayerCommand {
        player: PlayerId(1),
        command: Command::Surrender,
    };
    let report = state.tick(&[surrender]);
    tracker.observe(&state, &report.events, 12);
    while state.current_tick() < INCOME_CHECKPOINTS[0] {
        let report = state.tick(&[]);
        tracker.observe(&state, &report.events, 12);
    }
    assert!(state.result().is_none(), "seat one's ally plays on");
    let samples: Vec<usize> = tracker.finish().iter().map(Vec::len).collect();
    assert_eq!(samples, [1, 0, 1, 1]);
}
