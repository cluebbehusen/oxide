use super::*;
use oxide_sim::BuildingKind;
use oxide_sim::scenario::ScenarioMode;

#[test]
fn summary_reports_nearest_rank_p99_and_integer_average() {
    assert_eq!(Summary::of(&[]), Summary::default());
    assert_eq!(
        Summary::of(&[7]),
        Summary {
            count: 1,
            avg_ns: 7,
            p99_ns: 7,
            max_ns: 7,
            total_ns: 7
        }
    );
    let mut hundred: Vec<u64> = (1..=100).collect();
    hundred.reverse();
    hundred.swap(3, 71);
    assert_eq!(
        Summary::of(&hundred),
        Summary {
            count: 100,
            avg_ns: 50,
            p99_ns: 99,
            max_ns: 100,
            total_ns: 5050
        }
    );
    let hundred_fifty: Vec<u64> = (1..=150).map(|value| value * 10).collect();
    let summary = Summary::of(&hundred_fifty);
    assert_eq!((summary.p99_ns, summary.max_ns), (1490, 1500));
    assert_eq!(summary.avg_ns, 755);
}

#[test]
fn workloads_name_their_maps_seats_and_windows() {
    let seats: [&[u8]; 3] = [&[0, 1], &[1, 2, 3, 4, 5, 6, 7], &[0, 1]];
    for (workload, expected) in Workload::ALL.into_iter().zip(seats) {
        assert_eq!(workload.name().parse(), Ok(workload));
        assert!(workload.ticks() > 0);
        let scenario = workload.scenario();
        scenario.build().expect("workload scenarios build");
        let seated = seat_controllers(&scenario).unwrap();
        assert_eq!(
            seated
                .iter()
                .map(|seat| seat.player().0)
                .collect::<Vec<_>>(),
            expected,
            "{workload} seats"
        );
        for seat in &seated {
            assert_eq!(
                scenario.players[usize::from(seat.player().0)].bot_config,
                Some(PROFILE)
            );
        }
    }
    assert_eq!(Workload::Skyhook.ticks(), 20_000);
    assert!("oracle".parse::<Workload>().is_err());
}

#[test]
fn mature_armies_mirror_an_army_and_structures_for_each_seat() {
    let scenario = Workload::MatureArmies.scenario();
    assert_eq!(scenario.mode, ScenarioMode::Match);
    let state = scenario.build().unwrap();
    let holdings = |player: PlayerId| {
        let mut units: Vec<_> = state
            .units()
            .iter()
            .filter(|unit| unit.player == player)
            .map(|unit| unit.kind.role() as u8)
            .collect();
        units.sort_unstable();
        let mut structures: Vec<_> = state
            .buildings()
            .iter()
            .filter(|building| building.player == player)
            .map(|building| building.kind)
            .collect();
        structures.sort();
        (units, structures)
    };
    let (units, structures) = holdings(PlayerId(0));
    assert!(units.len() >= 40, "a mature army, not an opening");
    assert!(structures.contains(&BuildingKind::Fabricator));
    assert!(structures.contains(&BuildingKind::Airworks));
    assert_eq!(holdings(PlayerId(1)), (units, structures));
}
