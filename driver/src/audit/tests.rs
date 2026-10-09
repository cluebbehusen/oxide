use super::*;
use oxide_sim::scenario::ScenarioMode;

#[test]
fn an_obstruction_never_shrinks_the_reported_air_route() {
    // The same map with and without the peak blob: adding an
    // obstruction must never make air_tiles smaller.
    let build_map = |peaks: bool| {
        let mut rows = vec!["####################".to_string()];
        for y in 1..13 {
            let mut row = String::from("#");
            for x in 1..19 {
                row.push(match (x, y) {
                    (2, 2) => '1',
                    (16, 10) => '2',
                    (9..=10, 6..=7) if peaks => '^',
                    _ => '.',
                });
            }
            row.push('#');
            rows.push(row);
        }
        rows.push("####################".to_string());
        Scenario {
            mode: ScenarioMode::Match,
            name: "detour".into(),
            seed: 5,
            map: rows,
            players: Scenario::skirmish().players,
            units: Vec::new(),
            buildings: Vec::new(),
            meta: None,
        }
    };
    let open = audit(&build_map(false)).unwrap().routes[0]
        .air_tiles
        .unwrap();
    let blocked = audit(&build_map(true)).unwrap().routes[0]
        .air_tiles
        .unwrap();
    assert!(
        blocked >= open - 1e-9,
        "a peak in the way reads shorter: open {open:.2}, blocked {blocked:.2}"
    );
}

#[test]
fn a_peak_detour_never_reads_shorter_than_the_straight_line() {
    // Foundries on a diagonal with a peak blob astride the line: the
    // detour must report the same unit as open sky, so it never reads
    // closer than an unobstructed flight.
    let mut rows = vec!["####################".to_string()];
    for y in 1..13 {
        let mut row = String::from("#");
        for x in 1..19 {
            row.push(match (x, y) {
                (2, 2) => '1',
                (16, 10) => '2',
                (9..=10, 6..=7) => '^',
                _ => '.',
            });
        }
        row.push('#');
        rows.push(row);
    }
    rows.push("####################".to_string());
    let scenario = Scenario {
        mode: ScenarioMode::Match,
        name: "detour".into(),
        seed: 5,
        map: rows,
        players: Scenario::skirmish().players,
        units: Vec::new(),
        buildings: Vec::new(),
        meta: None,
    };
    let report = audit(&scenario).unwrap();
    let air = report.routes[0].air_tiles.expect("sky routes around");
    // Centers: (3,3) and (17,11) -> straight line ~16.1 tiles.
    let euclid = (14.0f64 * 14.0 + 8.0 * 8.0).sqrt();
    assert!(
        air >= euclid - 1.0,
        "detour {air:.1} reads shorter than the straight line {euclid:.1}"
    );
}

#[test]
fn mirrored_seats_measure_identically() {
    // Skirmish is 180-degree symmetric, so its mirror-identical seats
    // must measure the same room and spacing.
    let audit = audit(&Scenario::skirmish()).unwrap();
    assert_eq!(audit.seats.len(), 2);
    let (a, b) = (&audit.seats[0], &audit.seats[1]);
    assert_eq!(a.reachable_tiles, b.reachable_tiles);
    assert!(
        (a.nearest_scrap - b.nearest_scrap).abs() < 1e-9,
        "mirror seats see mirror scrap ({} vs {})",
        a.nearest_scrap,
        b.nearest_scrap
    );
    assert_eq!(a.nearest_enemy_route, b.nearest_enemy_route);
}

#[test]
fn pressure_and_routes_are_sane_on_the_shipped_duel() {
    let audit = audit(&Scenario::skirmish()).unwrap();
    assert_eq!(audit.routes.len(), 1, "one hostile pair in a duel");
    let route = &audit.routes[0];
    let steps = route.ground_steps.expect("shipped maps are connected");
    assert!(steps > 0);
    assert!(route.air_tiles.expect("open sky on the shipped duel") > 0.0);
    let pressure = route.artillery_pressure.expect("routed pair has pressure");
    assert!(
        (0.0..1.0).contains(&pressure),
        "artillery should pressure, not blanket, a shipped map ({pressure})"
    );
    assert!(audit.free_tiles > 0);
    assert!(audit.scrap_total > 0);
}
