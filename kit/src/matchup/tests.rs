use super::*;

#[test]
fn mirrored_armies_are_exactly_neutral_across_both_orientations() {
    let army = parse_army("sentinel:6").unwrap();
    let out = duel(
        &army,
        &army,
        &Arena {
            max_ticks: 6_000,
            ..Arena::default()
        },
    )
    .unwrap();

    assert_eq!(
        out.a_as_player_0.a_value, out.a_as_player_1.b_value,
        "exchanging identical armies must exchange the physical result: {out:?}"
    );
    assert_eq!(
        out.a_as_player_0.b_value, out.a_as_player_1.a_value,
        "exchanging identical armies must exchange the physical result: {out:?}"
    );
    assert_eq!(
        out.a_total_value(),
        out.b_total_value(),
        "the paired mirror must be exactly neutral: {out:?}"
    );
    assert_eq!(out.verdict(), Some(DuelVerdict::Tie));
}

#[test]
fn paired_matchups_report_both_legs_when_one_roster_wins_in_either_seat() {
    let lancers = parse_army("lancer:3").unwrap();
    let bombards = parse_army("bombard:2").unwrap();
    let out = duel(&lancers, &bombards, &Arena::default()).unwrap();

    // This is a measured result under the current movement and combat
    // rules. Keep both physical legs visible even when the same logical
    // roster wins from either seat.
    assert_eq!(out.a_as_player_0.verdict(), Some(DuelVerdict::A), "{out:?}");
    assert_eq!(out.a_as_player_1.verdict(), Some(DuelVerdict::A), "{out:?}");
    assert_eq!(
        out.verdict_flips_on_swap(),
        Some(false),
        "the paired result must distinguish a roster sweep from a seat effect: {out:?}"
    );
}

#[test]
fn opposite_resolved_leg_verdicts_report_a_seat_dependent_winner() {
    let leg = |a_player, a_value, b_value| DuelLegOutcome {
        a_player,
        a_value,
        b_value,
        a_hp_value: u64::from(a_value),
        b_hp_value: u64::from(b_value),
        ticks: 1,
        termination: DuelTermination::Wipe,
    };
    let outcome = DuelOutcome {
        a_as_player_0: leg(0, 100, 0),
        a_as_player_1: leg(1, 0, 100),
    };

    assert_eq!(outcome.verdict_flips_on_swap(), Some(true));
}

#[test]
fn the_parser_speaks_kind_names_and_counts() {
    let army = parse_army("Sentinel:3, lancer:2").unwrap();
    assert_eq!(army.len(), 2);
    assert_eq!(
        army_cost(&army),
        3 * UnitKind::Sentinel.stats().cost + 2 * UnitKind::Lancer.stats().cost
    );
    assert!(parse_army("gremlin:4").is_err());
}

#[test]
fn a_slow_duel_resolves_instead_of_ending_as_a_phantom_draw() {
    // Two lone bombards spend hundreds of ticks marching before a
    // shell flies. Value-only quiescence once ended this at tick
    // 302 with both sides reported intact; combat-gated quiescence
    // must let the duel actually resolve.
    let army = parse_army("bombard:1").unwrap();
    let out = duel(
        &army,
        &army,
        &Arena {
            max_ticks: 20_000,
            ..Arena::default()
        },
    )
    .unwrap();
    for leg in out.legs() {
        assert!(
            leg.a_value == 0 || leg.b_value == 0 || leg.ticks == 20_000,
            "the duel neither resolved nor ran honestly to the cap: {out:?}"
        );
        assert_ne!(
            leg.termination,
            DuelTermination::NoProgress,
            "ended as a phantom draw: {out:?}"
        );
    }
}

#[test]
fn a_capped_pair_is_unresolved_regardless_of_survivor_value() {
    let a = parse_army("sentinel:2").unwrap();
    let b = parse_army("scuttler:2").unwrap();
    let out = duel(
        &a,
        &b,
        &Arena {
            max_ticks: 1,
            ..Arena::default()
        },
    )
    .unwrap();

    for leg in out.legs() {
        assert_eq!(leg.termination, DuelTermination::Cap, "{out:?}");
        assert_eq!(leg.verdict(), None, "{out:?}");
    }
    assert_eq!(out.verdict(), None);
    assert_eq!(out.verdict_flips_on_swap(), None);
}

#[test]
fn the_garrison_parser_speaks_building_names() {
    let garrison = parse_garrison("turret:2, FlakTurret:1").unwrap();
    assert_eq!(garrison.len(), 2);
    assert_eq!(
        garrison_cost(&garrison),
        2 * structure_cost(BuildingKind::Turret) + structure_cost(BuildingKind::FlakTurret)
    );
    assert!(
        parse_garrison("foundry:1").is_err(),
        "victory tokens refused"
    );
    assert!(parse_garrison("keep:1").is_err());
}

#[test]
fn a_lone_raider_breaks_on_a_fortified_line() {
    // Defense mode's floor: one scuttler cannot crack two turrets,
    // and a unit-less defending side must not read as pre-wiped.
    let raiders = parse_army("scuttler:1").unwrap();
    let garrison = parse_garrison("turret:2").unwrap();
    let out = siege(&raiders, &[], &garrison, &Arena::default()).unwrap();
    for leg in out.legs() {
        assert_eq!(leg.a_value, 0, "the raider dies on the wall: {out:?}");
        assert!(
            leg.b_value >= garrison_cost(&garrison),
            "the standing wall keeps its purchase value: {out:?}"
        );
    }
    assert_eq!(out.verdict(), Some(DuelVerdict::B));
}

#[test]
fn harvesters_carry_their_purchase_value() {
    // A harvester screen is a legitimate experiment; valuing the
    // workers at zero once declared a live side wiped on the spot.
    let workers = parse_army("harvester:4").unwrap();
    let fighters = parse_army("sentinel:1").unwrap();
    let out = duel(
        &fighters,
        &workers,
        &Arena {
            max_ticks: 4_000,
            ..Arena::default()
        },
    )
    .unwrap();
    let worker_cost = army_cost(&workers);
    for leg in out.legs() {
        assert!(
            leg.b_value > 0 || leg.ticks > 50,
            "a live harvester side must not read as instantly wiped: {out:?} (cost \
                 {worker_cost})"
        );
    }
}

#[test]
fn the_arena_seats_one_roster_unless_told_otherwise() {
    assert_eq!(Arena::default().factions, SeatFactions::default());
    let default = SeatFactions::default();
    assert_eq!(
        default.west, default.east,
        "a leg swap must not exchange rosters by accident"
    );
    assert_eq!(
        parse_factions("fc").unwrap(),
        SeatFactions {
            west: Faction::Ferrous,
            east: Faction::Cupric,
        }
    );
    assert_eq!(
        parse_factions(" CF ").unwrap(),
        SeatFactions {
            west: Faction::Cupric,
            east: Faction::Ferrous,
        }
    );
    assert!(parse_factions("fx").is_err());
    assert!(parse_factions("f").is_err());
    assert!(parse_factions("ffc").is_err());
}

#[test]
fn a_seat_roster_moves_no_number_in_the_arena() {
    // The arena runs no economy and trains nothing, so a seat's
    // faction selects no unit stat. That is what makes same-faction
    // seating free: it takes a label out of the leg swap's bundle
    // without restating a single measured outcome.
    let a = parse_army("lancer:4").unwrap();
    let b = parse_army("scuttler:8").unwrap();
    let one_roster = duel(&a, &b, &Arena::default()).unwrap();
    let split_rosters = duel(
        &a,
        &b,
        &Arena {
            factions: parse_factions("fc").unwrap(),
            ..Arena::default()
        },
    )
    .unwrap();
    for (same, split) in one_roster.legs().iter().zip(split_rosters.legs()) {
        assert_eq!(
            (same.a_value, same.b_value, same.ticks),
            (split.a_value, split.b_value, split.ticks),
            "seat rosters steered an arena outcome: {same:?} vs {split:?}"
        );
    }
}

#[test]
fn wound_discounted_value_rides_beside_the_verdict_without_entering_it() {
    let a = parse_army("sentinel:4").unwrap();
    let b = parse_army("scuttler:6").unwrap();
    let intact = duel(
        &a,
        &b,
        &Arena {
            max_ticks: 1,
            ..Arena::default()
        },
    )
    .unwrap();
    for leg in intact.legs() {
        assert_eq!(u64::from(leg.a_value), leg.a_hp_value, "{intact:?}");
        assert_eq!(u64::from(leg.b_value), leg.b_hp_value, "{intact:?}");
    }

    let fought = duel(&a, &b, &Arena::default()).unwrap();
    let mut discounted = false;
    for leg in fought.legs() {
        assert!(
            leg.a_hp_value <= u64::from(leg.a_value) && leg.b_hp_value <= u64::from(leg.b_value),
            "wounds may only discount a survivor's price: {fought:?}"
        );
        discounted |=
            leg.a_hp_value < u64::from(leg.a_value) || leg.b_hp_value < u64::from(leg.b_value);
        // The verdict stays a purchase-value comparison. Reading the
        // discounted number instead would restate every arena
        // conclusion already recorded.
        if let Some(verdict) = leg.verdict() {
            assert_eq!(
                verdict,
                match leg.a_value.cmp(&leg.b_value) {
                    std::cmp::Ordering::Greater => DuelVerdict::A,
                    std::cmp::Ordering::Less => DuelVerdict::B,
                    std::cmp::Ordering::Equal => DuelVerdict::Tie,
                },
                "{fought:?}"
            );
        }
    }
    assert!(
        discounted,
        "a fought-out duel must leave someone wounded, or the number is dead: {fought:?}"
    );
}

#[test]
fn the_garrison_pitch_keeps_its_shipped_grid_and_refuses_overlap() {
    let raiders = parse_army("scuttler:1").unwrap();
    let wall = |kind: &str, n: u32, pitch: i32| {
        siege(
            &raiders,
            &[],
            &parse_garrison(&format!("{kind}:{n}")).unwrap(),
            &Arena {
                garrison_pitch: pitch,
                max_ticks: 1,
                ..Arena::default()
            },
        )
    };
    // Pitch 3 is the shipped grid: three columns of six.
    assert!(wall("turret", 18, 3).is_ok());
    assert!(wall("turret", 19, 3).is_err());
    // A tighter pitch stands more wall, but never tighter than the
    // widest structure in it.
    assert!(wall("turret", 19, 2).is_ok());
    assert!(wall("bastion", 1, 1).is_err());
    assert!(wall("bastion", 1, 2).is_ok());
}

#[test]
fn an_empty_side_is_refused_unless_side_b_has_a_garrison() {
    let units = parse_army("sentinel:1").unwrap();
    let no_units: Army = Vec::new();

    let missing_a = duel(&no_units, &units, &Arena::default()).unwrap_err();
    assert!(missing_a.to_string().contains("side A needs units"));
    let missing_b = duel(&units, &no_units, &Arena::default()).unwrap_err();
    assert!(
        missing_b
            .to_string()
            .contains("side B needs units or a garrison")
    );

    let wall = parse_garrison("turret:1").unwrap();
    assert!(
        siege(
            &units,
            &no_units,
            &wall,
            &Arena {
                max_ticks: 1,
                ..Arena::default()
            },
        )
        .is_ok()
    );
}
