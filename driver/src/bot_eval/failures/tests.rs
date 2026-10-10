use super::*;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, ScenarioMode, UnitSpec};
use oxide_sim::{Faction, Scenario};

/// The Scuttler, the cheapest unit a Foundry trains.
const CHEAPEST_FOUNDRY_UNIT: u32 = 40;

fn scenario(buildings: Vec<BuildingSpec>, scrap: u32) -> Scenario {
    let ground = ".".repeat(24);
    let mut anchored: Vec<char> = ground.chars().collect();
    anchored[2] = '1';
    anchored[20] = '2';
    let mut map = vec![ground.clone(); 12];
    map[2] = anchored.into_iter().collect();
    Scenario {
        mode: ScenarioMode::Match,
        name: "detectors".into(),
        map,
        players: [Faction::Ferrous, Faction::Cupric]
            .into_iter()
            .map(|faction| PlayerSpec {
                name: format!("{faction:?}"),
                faction,
                team: None,
                scrap,
                bot: false,
                bot_config: None,
            })
            .collect(),
        units: vec![UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: 6,
            y: 8,
        }],
        buildings,
        meta: None,
    }
}

fn staged(scenario: &Scenario, edit: impl FnOnce(&mut serde_json::Value)) -> State {
    let mut value = serde_json::to_value(scenario.build().unwrap()).unwrap();
    edit(&mut value);
    serde_json::from_value(value).unwrap()
}

fn building_index(value: &serde_json::Value, kind: &str) -> usize {
    value["buildings"]
        .as_array()
        .unwrap()
        .iter()
        .position(|building| building["kind"] == kind)
        .unwrap()
}

fn site(progress: u32) -> impl FnOnce(&mut serde_json::Value) {
    move |value| {
        let index = building_index(value, "fabricator");
        value["buildings"][index]["phase"] =
            serde_json::json!({"phase": "site", "progress": progress});
        value["buildings"][index]["hp"] = 100.into();
    }
}

fn fabricator() -> Vec<BuildingSpec> {
    vec![BuildingSpec {
        player: 0,
        kind: BuildingKind::Fabricator,
        x: 8,
        y: 6,
    }]
}

fn failures(detectors: FailureDetectors) -> Vec<SeatFailures> {
    detectors
        .finish()
        .into_iter()
        .map(|report| report.failures)
        .collect()
}

#[test]
fn repeated_stalls_fire_on_the_fifth_inside_the_window_only() {
    let mut detectors = FailureDetectors::new([true, false]);
    for tick in [100, 400, 700, 1_000] {
        detectors.record_stall(0, 7, "no_route", tick);
    }
    detectors.record_stall(0, 7, "ground_taken", 1_050);
    detectors.record_stall(0, 8, "no_route", 1_050);
    detectors.record_stall(1, 7, "no_route", 1_050);
    let mut spread = FailureDetectors::new([true]);
    for tick in [0, 300, 600, 900, 1_200] {
        spread.record_stall(0, 7, "no_route", tick);
    }
    assert_eq!(
        failures(spread)[0].repeated_orders.incidents,
        0,
        "five stalls spanning a full window are not repeated"
    );

    detectors.record_stall(0, 7, "no_route", 1_099);
    detectors.record_stall(0, 7, "no_route", 1_150);
    let failures = failures(detectors);
    assert_eq!(failures[0].repeated_orders.incidents, 1);
    assert_eq!(
        failures[0].repeated_orders.examples,
        [FailureIncident {
            tick: 1_099,
            subject: 7,
            detail: "no_route".into(),
        }]
    );
    assert_eq!(
        failures[1],
        SeatFailures::default(),
        "seat one is unwatched"
    );
}

#[test]
fn danger_holds_are_not_repeated_orders() {
    assert_eq!(
        super::super::wire_name(&oxide_sim::StallReason::DangerHold),
        EXEMPT_STALL_REASON
    );
    let mut detectors = FailureDetectors::new([true]);
    for tick in 0..10 {
        detectors.record_stall(0, 7, EXEMPT_STALL_REASON, tick * 10);
    }
    assert_eq!(failures(detectors)[0].repeated_orders.incidents, 0);
}

#[test]
fn a_repeated_order_rearms_after_a_quiet_window() {
    let state = scenario(Vec::new(), 0).build().unwrap();
    let mut detectors = FailureDetectors::new([true, true]);
    for tick in 0..5 {
        detectors.record_stall(0, 7, "no_route", tick * 10);
    }
    detectors.check(&state, 1_200, &[0, 0]);
    detectors.record_stall(0, 7, "no_route", 1_210);
    detectors.check(&state, 1_240, &[0, 0]);
    for tick in 0..4 {
        detectors.record_stall(0, 7, "no_route", 1_250 + tick);
    }
    assert_eq!(failures(detectors)[0].repeated_orders.incidents, 1);

    let mut detectors = FailureDetectors::new([true, true]);
    for tick in 0..5 {
        detectors.record_stall(0, 7, "no_route", tick * 10);
    }
    detectors.check(&state, 1_240, &[0, 0]);
    for tick in 0..5 {
        detectors.record_stall(0, 7, "no_route", 1_250 + tick);
    }
    assert_eq!(failures(detectors)[0].repeated_orders.incidents, 2);
}

#[test]
fn an_unprogressing_site_is_abandoned_at_the_window_and_progress_rearms_it() {
    let scenario = scenario(fabricator(), 0);
    let stalled = staged(&scenario, site(40));
    let mut detectors = FailureDetectors::new([true, true]);
    for now in (600..600 + FAILURE_WINDOW_TICKS).step_by(12) {
        detectors.check(&stalled, now, &[0, 0]);
    }
    detectors.check(&stalled, 600 + FAILURE_WINDOW_TICKS, &[0, 0]);
    detectors.check(&stalled, 612 + FAILURE_WINDOW_TICKS, &[0, 0]);
    let advanced = staged(&scenario, site(41));
    detectors.check(&advanced, 2_000, &[0, 0]);
    detectors.check(&advanced, 3_199, &[0, 0]);
    let before_rearm = detectors.seats[0].as_ref().unwrap().failures.clone();
    detectors.check(&advanced, 3_200, &[0, 0]);
    let failures = failures(detectors);

    assert_eq!(before_rearm.abandoned_sites.incidents, 1);
    assert_eq!(before_rearm.abandoned_sites.examples[0].tick, 1_800);
    assert_eq!(
        before_rearm.abandoned_sites.examples[0].detail,
        "fabricator"
    );
    assert_eq!(failures[0].abandoned_sites.incidents, 2);
    assert_eq!(failures[0].starved_production.incidents, 0);
}

#[test]
fn provisional_and_upgrading_works_are_not_abandoned_sites() {
    let provisional = staged(&scenario(fabricator(), 0), |value| {
        site(0)(value);
        let fabricator = building_index(value, "fabricator");
        value["buildings"][fabricator]["phase"] = serde_json::json!({"phase": "provisional"});
        value["units"][0]["order"] = serde_json::json!({
            "order": "found",
            "kind": "fabricator",
            "anchor": value["buildings"][fabricator]["anchor"].clone(),
        });
    });
    assert!(
        provisional
            .buildings()
            .iter()
            .any(oxide_sim::Building::provisional)
    );
    let upgrading = staged(
        &scenario(
            vec![BuildingSpec {
                player: 0,
                kind: BuildingKind::Turret,
                x: 12,
                y: 9,
            }],
            0,
        ),
        |value| {
            let turret = building_index(value, "turret");
            value["buildings"][turret]["phase"] = serde_json::json!({"phase": "upgrading"});
            value["buildings"][turret]["tier"] = 1.into();
        },
    );
    for state in [&provisional, &upgrading] {
        let mut detectors = FailureDetectors::new([true, true]);
        detectors.check(state, 0, &[0, 0]);
        detectors.check(state, 5_000, &[0, 0]);
        assert_eq!(failures(detectors)[0].abandoned_sites.incidents, 0);
    }
}

#[test]
fn an_idle_affordable_seat_starves_at_the_window_and_protected_scrap_counts() {
    let state = scenario(Vec::new(), CHEAPEST_FOUNDRY_UNIT).build().unwrap();
    let mut detectors = FailureDetectors::new([true, true]);
    detectors.check(&state, 0, &[0, 0]);
    detectors.check(&state, FAILURE_WINDOW_TICKS - 12, &[0, 0]);
    assert_eq!(
        detectors.seats[0]
            .as_ref()
            .unwrap()
            .failures
            .starved_production
            .incidents,
        0
    );
    detectors.check(&state, FAILURE_WINDOW_TICKS, &[0, 0]);
    detectors.check(&state, 5 * FAILURE_WINDOW_TICKS, &[0, 0]);
    let finished = detectors.finish();
    let (failures, idle) = (&finished[0].failures, &finished[0].idle_producers);
    assert_eq!(failures.starved_production.incidents, 1);
    assert_eq!(
        failures.starved_production.examples,
        [FailureIncident {
            tick: FAILURE_WINDOW_TICKS,
            subject: 0,
            detail: "scuttler".into(),
        }]
    );
    assert_eq!(finished[1].failures.starved_production.incidents, 1);
    let [foundry] = idle.as_slice() else {
        panic!("one idle producer: {idle:?}");
    };
    assert_eq!(foundry.kind, "foundry");
    assert_eq!(foundry.idle_ticks, 5 * FAILURE_WINDOW_TICKS);

    let mut protected = FailureDetectors::new([true, true]);
    protected.check(&state, 0, &[1, 0]);
    protected.check(&state, 2 * FAILURE_WINDOW_TICKS, &[1, 0]);
    let finished = protected.finish();
    assert_eq!(finished[0].failures.starved_production.incidents, 0);
    assert!(
        finished[0].idle_producers.is_empty(),
        "protected scrap is not idle money"
    );
    assert_eq!(finished[1].failures.starved_production.incidents, 1);
}

#[test]
fn one_busy_producer_keeps_the_seat_from_starving() {
    let state = staged(&scenario(fabricator(), 1_000), |value| {
        let index = building_index(value, "fabricator");
        value["buildings"][index]["queue"] = serde_json::json!(["lancer"]);
    });
    let mut detectors = FailureDetectors::new([true, false]);
    detectors.check(&state, 0, &[0, 0]);
    detectors.check(&state, 3 * FAILURE_WINDOW_TICKS, &[0, 0]);
    let finished = detectors.finish();
    let (failures, idle) = (&finished[0].failures, &finished[0].idle_producers);
    assert_eq!(failures.starved_production.incidents, 0);
    let [foundry] = idle.as_slice() else {
        panic!("only the idle Foundry is recorded: {idle:?}");
    };
    assert_eq!(foundry.kind, "foundry");
    assert_eq!(foundry.idle_ticks, 3 * FAILURE_WINDOW_TICKS);
}

#[test]
fn a_busy_or_unaffordable_seat_is_not_starved() {
    let poor = scenario(Vec::new(), CHEAPEST_FOUNDRY_UNIT - 1)
        .build()
        .unwrap();
    let busy = staged(&scenario(Vec::new(), 1_000), |value| {
        let index = building_index(value, "foundry");
        value["buildings"][index]["queue"] = serde_json::json!(["harvester"]);
    });
    for state in [&poor, &busy] {
        let mut detectors = FailureDetectors::new([true, false]);
        detectors.check(state, 0, &[0, 0]);
        detectors.check(state, 3 * FAILURE_WINDOW_TICKS, &[0, 0]);
        let finished = detectors.finish();
        assert_eq!(finished[0].failures.starved_production.incidents, 0);
        assert!(finished[0].idle_producers.is_empty());
    }
}

#[test]
fn legal_units_follow_faction_and_prerequisites() {
    let mut buildings = fabricator();
    buildings.push(BuildingSpec {
        player: 1,
        kind: BuildingKind::Fabricator,
        x: 14,
        y: 6,
    });
    let state = scenario(buildings, 0).build().unwrap();
    let producer = |player: u8, kind: BuildingKind| {
        state
            .buildings()
            .iter()
            .find(|building| building.player == PlayerId(player) && building.kind == kind)
            .unwrap()
    };
    let fabricator = BuildingKind::Fabricator;
    let ferrous = producer(0, fabricator);
    let cupric = producer(1, fabricator);
    assert_eq!(
        cheapest_legal_unit(&state, ferrous, &[]),
        Some(UnitKind::Flakhound)
    );
    assert_eq!(
        cheapest_legal_unit(&state, cupric, &[]),
        Some(UnitKind::Stinger)
    );
    assert!(legal_units(&state, ferrous, &[]).all(|kind| kind != UnitKind::Stinger));
    let foundry = producer(0, BuildingKind::Foundry);
    assert!(!legal_units(&state, foundry, &[]).any(|kind| kind == UnitKind::Excavator));
    assert!(legal_units(&state, foundry, &[fabricator]).any(|kind| kind == UnitKind::Excavator));
    assert_eq!(
        cheapest_legal_unit(&state, foundry, &[fabricator]).map(|kind| kind.stats().cost),
        Some(CHEAPEST_FOUNDRY_UNIT)
    );
}

#[test]
fn a_mission_stuck_past_its_timeout_counts_once_per_phase() {
    use oxide_opponent::{MissionKind, Phase};
    let status = |since: u64| MissionStatus {
        id: 3,
        kind: MissionKind::Defend {
            asset: oxide_sim::BuildingId(1),
        },
        phase: Phase::Engage,
        since,
        timeout: 600,
        units: 2,
        goal: chassis::grid::TilePos::new(4, 4),
    };
    let mut detectors = FailureDetectors::new([true, false]);
    let overdue = 600 + FAILURE_WINDOW_TICKS;
    detectors.check_missions(0, overdue - 1, &[status(0)]);
    detectors.check_missions(1, overdue, &[status(0)]);
    detectors.check_missions(0, overdue, &[status(0)]);
    detectors.check_missions(0, overdue + 12, &[status(0)]);
    detectors.check_missions(0, overdue + 12, &[status(overdue)]);
    detectors.check_missions(0, 2 * overdue - 1, &[status(overdue)]);
    detectors.check_missions(0, 2 * overdue, &[status(overdue)]);
    let seats = detectors.finish();
    let stuck = &seats[0].failures.stuck_missions;
    assert_eq!(stuck.incidents, 2);
    assert_eq!(
        stuck.examples[0],
        FailureIncident {
            tick: overdue,
            subject: 3,
            detail: "defend engage".into(),
        }
    );
    assert_eq!(
        seats[1].failures,
        SeatFailures::default(),
        "an unwatched seat"
    );
}

#[test]
fn failures_recorded_before_the_mission_detector_still_load() {
    let mut value = serde_json::to_value(SeatFailures::default()).unwrap();
    value.as_object_mut().unwrap().remove("stuck_missions");
    let loaded: SeatFailures = serde_json::from_value(value).unwrap();
    assert_eq!(loaded, SeatFailures::default());
}

#[test]
fn failures_recorded_before_the_idle_army_detector_read_as_unmeasured() {
    let mut value = serde_json::to_value(SeatFailures::default()).unwrap();
    value.as_object_mut().unwrap().remove("idle_army");
    let loaded: SeatFailures = serde_json::from_value(value).unwrap();
    assert_eq!(loaded.idle_army, None);
}

/// The detectors' scenario with seat zero's Sentinels on row `y`, from
/// column `x` rightward, and any extra units after them.
fn army(count: u32, x: i32, y: i32, extra: &[UnitSpec]) -> Scenario {
    let mut scenario = scenario(Vec::new(), 0);
    for index in 0..i32::try_from(count).unwrap() {
        scenario.units.push(UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: x + index % 8,
            y: y + index / 8,
        });
    }
    scenario.units.extend_from_slice(extra);
    scenario
}

/// Enough Sentinels to clear [`IDLE_ARMY_FLOOR`].
fn large() -> u32 {
    u32::try_from(IDLE_ARMY_FLOOR / u64::from(UnitKind::Sentinel.stats().cost)).unwrap() + 1
}

fn idle_army(detectors: FailureDetectors) -> u64 {
    failures(detectors)[0]
        .idle_army
        .as_ref()
        .map_or(0, |tally| tally.incidents)
}

fn run(detectors: &mut FailureDetectors, state: &State, ticks: std::ops::Range<u64>) {
    for now in ticks.step_by(12) {
        detectors.check(state, now, &[0, 0]);
    }
}

#[test]
fn an_army_resting_at_home_is_idle_after_its_rest_and_the_window() {
    let state = army(large(), 4, 6, &[]).build().unwrap();
    let mut detectors = FailureDetectors::new([true, true]);
    run(&mut detectors, &state, 0..IDLE_TICKS + FAILURE_WINDOW_TICKS);
    assert_eq!(
        detectors.seats[0]
            .as_ref()
            .unwrap()
            .failures
            .idle_army
            .as_ref()
            .unwrap()
            .incidents,
        0
    );
    run(
        &mut detectors,
        &state,
        IDLE_TICKS + FAILURE_WINDOW_TICKS..3 * IDLE_TICKS,
    );
    let failures = failures(detectors);
    let tally = failures[0].idle_army.as_ref().unwrap();
    assert_eq!(tally.incidents, 1, "one episode while it holds");
    assert_eq!(tally.examples[0].tick, IDLE_TICKS + FAILURE_WINDOW_TICKS);
    assert_eq!(
        failures[1].idle_army.as_ref().unwrap().incidents,
        0,
        "a seat without an army is never idle"
    );
}

#[test]
fn moving_or_meeting_an_enemy_restarts_a_rest() {
    let settled = army(large(), 4, 6, &[]).build().unwrap();
    let moved = army(large(), 8, 6, &[]).build().unwrap();
    let mut detectors = FailureDetectors::new([true, true]);
    run(&mut detectors, &settled, 0..3_000);
    run(&mut detectors, &moved, 3_000..3_000 + IDLE_TICKS);
    assert_eq!(idle_army(detectors), 0, "the army moved before its episode");

    let enemy = UnitSpec {
        player: 1,
        kind: UnitKind::Sentinel,
        x: 9,
        y: 9,
    };
    let watched = army(large(), 4, 6, &[enemy]).build().unwrap();
    let mut detectors = FailureDetectors::new([true, true]);
    run(&mut detectors, &watched, 0..3 * IDLE_TICKS);
    assert_eq!(idle_army(detectors), 0, "an enemy in reach keeps it busy");
}

#[test]
fn an_enemy_the_army_cannot_hit_does_not_keep_it_busy() {
    let flak =
        i32::try_from(IDLE_ARMY_FLOOR / u64::from(UnitKind::Flakhound.stats().cost)).unwrap() + 1;
    let mut scenario = scenario(Vec::new(), 0);
    for index in 0..flak {
        scenario.units.push(UnitSpec {
            player: 0,
            kind: UnitKind::Flakhound,
            x: 4 + index % 8,
            y: 6 + index / 8,
        });
    }
    scenario.units.push(UnitSpec {
        player: 1,
        kind: UnitKind::Sentinel,
        x: 9,
        y: 9,
    });
    let state = scenario.build().unwrap();
    let mut detectors = FailureDetectors::new([true, true]);
    run(&mut detectors, &state, 0..3 * IDLE_TICKS);
    assert_eq!(idle_army(detectors), 1, "anti-air beside a ground enemy");
}

#[test]
fn a_small_guard_or_a_won_match_is_not_an_idle_army() {
    let guard = army(2, 4, 6, &[]).build().unwrap();
    let won = staged(&army(large(), 4, 6, &[]), |value| {
        value["players"][1]["eliminated_at"] = 0.into();
    });
    for state in [&guard, &won] {
        let mut detectors = FailureDetectors::new([true, true]);
        run(&mut detectors, state, 0..3 * IDLE_TICKS);
        assert_eq!(idle_army(detectors), 0);
    }
}

/// The detectors' map with a pit column between the two starts.
fn severed(units: &[UnitSpec]) -> Scenario {
    let mut scenario = scenario(Vec::new(), 0);
    for row in &mut scenario.map {
        row.replace_range(12..13, "~");
    }
    scenario.units.extend_from_slice(units);
    scenario
}

fn sentinel(x: i32) -> UnitSpec {
    UnitSpec {
        player: 0,
        kind: UnitKind::Sentinel,
        x,
        y: 9,
    }
}

fn trained(state: &State) -> Event {
    let unit = state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Sentinel)
        .unwrap();
    Event::UnitTrained {
        building: oxide_sim::BuildingId(0),
        unit: unit.id,
        kind: unit.kind,
        player: unit.player,
    }
}

fn deliveries(detectors: FailureDetectors) -> Deliveries {
    detectors.finish()[0].deliveries.unwrap()
}

#[test]
fn units_trained_on_severed_ground_are_delivered_lost_or_left_home() {
    let cost = u64::from(UnitKind::Sentinel.stats().cost);
    let home = severed(&[sentinel(6)]).build().unwrap();
    let across = severed(&[sentinel(16)]).build().unwrap();
    let carried = severed(&[]).build().unwrap();

    let mut detectors = FailureDetectors::new([true, true]);
    detectors.observe_events(&home, &[trained(&home)], 100);
    detectors.check(&carried, 3_000, &[0, 0]);
    detectors.check(&across, 4_000, &[0, 0]);
    assert_eq!(
        deliveries(detectors),
        Deliveries {
            delivered: cost,
            ..Deliveries::default()
        },
        "carried, then set down across the pit"
    );

    let mut detectors = FailureDetectors::new([true, true]);
    let event = trained(&home);
    detectors.observe_events(&home, std::slice::from_ref(&event), 100);
    let Event::UnitTrained {
        unit, kind, player, ..
    } = event
    else {
        unreachable!()
    };
    let died = Event::UnitDied {
        unit,
        kind,
        player,
        pos: home.unit(unit).unwrap().pos,
        grounded: true,
    };
    detectors.observe_events(&home, std::slice::from_ref(&died), 200);
    assert_eq!(deliveries(detectors).lost, cost);

    let mut detectors = FailureDetectors::new([true, true]);
    detectors.observe_events(&carried, &[event.clone(), died.clone()], 100);
    assert_eq!(
        deliveries(detectors).lost,
        cost,
        "killed on the tick it was trained"
    );

    let unloaded = Event::UnitUnloaded {
        transport: UnitId(u32::MAX),
        unit,
        player,
        at: TilePos::new(16, 9),
    };
    let mut detectors = FailureDetectors::new([true, true]);
    detectors.observe_events(&home, std::slice::from_ref(&event), 100);
    detectors.observe_events(&across, &[unloaded, died], 110);
    assert_eq!(
        deliveries(detectors),
        Deliveries {
            delivered: cost,
            ..Deliveries::default()
        },
        "set down across the pit, then killed before the next check"
    );

    let mut detectors = FailureDetectors::new([true, true]);
    detectors.observe_events(&home, &[trained(&home)], 100);
    detectors.check(&home, 100 + DELIVERY_TICKS - 12, &[0, 0]);
    detectors.check(&home, 100 + DELIVERY_TICKS, &[0, 0]);
    assert_eq!(deliveries(detectors).undelivered, cost);

    let mut detectors = FailureDetectors::new([true, true]);
    detectors.observe_events(&home, &[trained(&home)], 100);
    detectors.check(&carried, 100 + 2 * DELIVERY_TICKS, &[0, 0]);
    assert_eq!(
        deliveries(detectors),
        Deliveries::default(),
        "a unit still aboard is not counted"
    );
}

#[test]
fn units_trained_on_connected_ground_are_not_followed() {
    let mut connected = scenario(Vec::new(), 0);
    connected.units.push(sentinel(6));
    let state = connected.build().unwrap();
    let mut detectors = FailureDetectors::new([true, true]);
    detectors.observe_events(&state, &[trained(&state)], 100);
    detectors.check(&state, 100 + DELIVERY_TICKS, &[0, 0]);
    let finished = detectors.finish();
    assert_eq!(finished[0].deliveries, Some(Deliveries::default()));
    assert_eq!(finished[1].deliveries, Some(Deliveries::default()));
}
