use super::*;
use oxide_kit::controller::{record_events, seat_controllers};
use oxide_sim::State;

fn synthetic(factions: [&str; 2], outcome: SweepOutcome, ticks: u64) -> FactorialMatch {
    FactorialMatch {
        seed: 7,
        cell: vec![0],
        levels: vec!["roster:base".into()],
        factions: [factions[0].into(), factions[1].into()],
        ticks,
        outcome,
        hash: 0,
    }
}

/// The audit measured every `.then(...)` in the record builders at
/// zero execution: no test ever produced a victory, so win rates,
/// intervals, and quartiles were unverifiable. Hand-built matches
/// exercise the full response surface without a simulation.
#[test]
fn level_records_compute_rates_intervals_and_quartiles() {
    let matches = [
        synthetic(
            ["ferrous", "cupric"],
            SweepOutcome::Victory { seat: 0 },
            100,
        ),
        synthetic(
            ["ferrous", "cupric"],
            SweepOutcome::Victory { seat: 0 },
            200,
        ),
        synthetic(
            ["ferrous", "cupric"],
            SweepOutcome::Victory { seat: 0 },
            300,
        ),
        synthetic(
            ["ferrous", "cupric"],
            SweepOutcome::Victory { seat: 1 },
            400,
        ),
        synthetic(["ferrous", "cupric"], SweepOutcome::Draw, 250),
        synthetic(["ferrous", "cupric"], SweepOutcome::Undecided, 999),
    ];
    let refs: Vec<&FactorialMatch> = matches.iter().collect();
    let record = level_record("roster:base", &refs);
    assert_eq!(record.matches, 6);
    assert_eq!(record.victories, 4);
    assert_eq!(record.draws, 1);
    assert_eq!(record.undecided, 1);
    assert_eq!(record.seat_wins, [3, 1]);
    assert_eq!(record.seat0_win_rate, Some(0.75));
    let [lo, hi] = record.wilson.expect("victories imply an interval");
    assert!(lo < 0.75 && 0.75 < hi);
    assert!((0.0..=1.0).contains(&lo) && (0.0..=1.0).contains(&hi));
    let quartiles = record
        .decision_ticks
        .expect("decided matches imply quartiles");
    assert!(quartiles.p25 <= quartiles.median && quartiles.median <= quartiles.p75);

    let empty = level_record("roster:base", &[]);
    assert_eq!(empty.seat0_win_rate, None);
    assert_eq!(empty.wilson, None);
    assert!(empty.decision_ticks.is_none());
}

/// Roster attribution is by the WINNING seat's faction, not the
/// seat index — a CF cell's seat-0 win is a Cupric win.
#[test]
fn roster_records_attribute_wins_to_factions_not_seats() {
    let matches = vec![
        synthetic(
            ["ferrous", "cupric"],
            SweepOutcome::Victory { seat: 0 },
            100,
        ),
        synthetic(
            ["ferrous", "cupric"],
            SweepOutcome::Victory { seat: 1 },
            100,
        ),
        synthetic(
            ["cupric", "ferrous"],
            SweepOutcome::Victory { seat: 0 },
            100,
        ),
        synthetic(
            ["ferrous", "ferrous"],
            SweepOutcome::Victory { seat: 0 },
            100,
        ),
        synthetic(["ferrous", "cupric"], SweepOutcome::Draw, 100),
    ];
    let record = roster_record(&matches).expect("cross-faction victories exist");
    assert_eq!(record.ferrous_wins, 1, "only the FC seat-0 win is Ferrous");
    assert_eq!(
        record.cupric_wins, 2,
        "the FC seat-1 and CF seat-0 wins are Cupric"
    );
    assert_eq!(record.ferrous_win_rate, 1.0 / 3.0);

    assert!(
        roster_record(&[synthetic(
            ["ferrous", "ferrous"],
            SweepOutcome::Victory { seat: 0 },
            100
        )])
        .is_none(),
        "mirror matches carry no roster signal"
    );
}

/// Rotating twice is the identity — the transform that would
/// silently hand the seats different worlds is exactly the one a
/// controlled comparison cannot survive.
#[test]
fn rotation_is_an_involution_and_preserves_the_world() {
    let base = crate::runner::load_scenario("skirmish").unwrap();
    let once = rotate_180(&base).unwrap();
    assert_ne!(once.map, base.map, "the anchors actually moved");
    assert_eq!(rotate_180(&once).unwrap(), base);

    let (before_map, _) = oxide_sim::map::Map::parse(&base.map).unwrap();
    let (after_map, _) = oxide_sim::map::Map::parse(&once.map).unwrap();
    let (ew, eh) = BuildingKind::Extractor.base_stats().size;
    let mut expected_frames: Vec<_> = before_map
        .extractor_frames()
        .iter()
        .map(|frame| chassis::grid::TilePos {
            x: before_map.width() - ew - frame.x,
            y: before_map.height() - eh - frame.y,
        })
        .collect();
    expected_frames.sort_by_key(|frame| (frame.y, frame.x));
    assert_eq!(after_map.extractor_frames(), expected_frames);

    let before = base.build().unwrap();
    let after = once.build().unwrap();
    let open = |state: &State| {
        let map = state.map();
        (0..map.height())
            .flat_map(|y| (0..map.width()).map(move |x| chassis::grid::TilePos::new(x, y)))
            .filter(|t| state.passable(*t))
            .count()
    };
    let scrap = |state: &State| {
        let map = state.map();
        (0..map.height())
            .flat_map(|y| (0..map.width()).map(move |x| chassis::grid::TilePos::new(x, y)))
            .filter_map(|t| map.tile(t))
            .map(|tile| u64::from(tile.scrap))
            .sum::<u64>()
    };
    assert_eq!(open(&before), open(&after));
    assert_eq!(scrap(&before), scrap(&after));
    assert_eq!(before.units().len(), after.units().len());
    assert_eq!(before.buildings().len(), after.buildings().len());
}

/// An asymmetric map is refused rather than rotated into a rigged
/// verdict.
#[test]
fn an_asymmetric_map_refuses_rotation() {
    let mut base = crate::runner::load_scenario("skirmish").unwrap();
    let mut row: Vec<char> = base.map[10].chars().collect();
    let x = row
        .iter()
        .position(|c| *c == '.')
        .expect("the basin has open ground");
    row[x] = '#';
    base.map[10] = row.into_iter().collect();
    let err = rotate_180(&base).unwrap_err().to_string();
    assert!(err.contains("not 180-symmetric"), "{err}");
}

/// The spawn lever moves the low id range and nothing else.
#[test]
fn the_spawn_lever_hands_the_low_ids_to_the_named_seat() {
    let base = crate::runner::load_scenario("skirmish").unwrap();
    for first in [0u8, 1] {
        let mut sc = base.clone();
        permute_spawn_order(&mut sc, first);
        assert_eq!(sc.units.len(), base.units.len());
        let state = sc.build().unwrap();
        let low = state
            .units()
            .iter()
            .min_by_key(|u| u.id.0)
            .expect("the basin starts units");
        assert_eq!(low.player, PlayerId(first));
    }
}

/// The all-baseline cell must reproduce, bit for bit, a reference
/// run that seats [`seat_controllers`] per seat and steps the sim
/// directly — proof that the harness transforms are neutral when
/// every lever sits at its baseline.
#[test]
fn the_baseline_cell_reproduces_a_direct_controller_run() {
    let base = crate::runner::load_scenario("skirmish").unwrap();
    for seed in [0u64, 17, 4_242] {
        let mut sc = base.clone();
        sc.seed = seed;
        oxide_kit::bench::all_bots(&mut sc);
        let mut state = sc.build().unwrap();
        let mut bots = seat_controllers(&sc).unwrap();
        for _ in 0..200 {
            let mut commands = Vec::new();
            for bot in &mut bots {
                commands.extend(bot.act(&state));
            }
            let report = state.tick(&commands);
            record_events(&mut bots, &report);
            if state.result().is_some() {
                break;
            }
        }
        let probed = play(
            &base,
            seed,
            [0; FACTOR_COUNT],
            200,
            oxide_sim::scenario::BotConfig::default(),
        )
        .unwrap();
        assert_eq!(probed.hash, state.hash(), "seed {seed}");
        assert_eq!(probed.ticks, state.current_tick(), "seed {seed}");
    }
}

/// Every cell is played on every seed, every match lands in exactly
/// one cell, and the marginals account for all of them.
#[test]
fn the_design_accounts_for_every_cell() {
    let report = run_factorial(
        "skirmish",
        &Factor::ALL,
        1,
        30,
        7_000,
        oxide_sim::scenario::BotConfig::default(),
    )
    .unwrap();
    assert_eq!(report.cells, 4 * 2 * 2 * 2);
    assert_eq!(report.matches_played as usize, report.cells);
    assert_eq!(report.per_cell.len(), report.cells);
    assert!(report.per_cell.iter().all(|c| c.matches == 1));
    for factor in &report.per_factor {
        let counted: u32 = factor.per_level.iter().map(|l| l.matches).sum();
        assert_eq!(counted, report.matches_played, "{}", factor.factor);
    }
    // The faction levels are absolute rosters, not relative flips.
    let ff = report
        .matches
        .iter()
        .find(|m| m.cell[Factor::Faction.index()] == 2)
        .unwrap();
    assert_eq!(ff.factions, ["ferrous".to_string(), "ferrous".to_string()]);
}

/// A subset design pins the factors it left out at their baseline.
#[test]
fn a_subset_design_pins_the_disabled_factors() {
    let report = run_factorial(
        "skirmish",
        &[Factor::Command, Factor::Geometry],
        1,
        20,
        7_000,
        oxide_sim::scenario::BotConfig::default(),
    )
    .unwrap();
    assert_eq!(report.cells, 4);
    assert_eq!(report.factors, ["command", "geometry"]);
    for m in &report.matches {
        assert_eq!(m.cell[Factor::Faction.index()], 0);
        assert_eq!(m.cell[Factor::Spawn.index()], 0);
        assert_eq!(m.levels.len(), 2);
    }
}

/// Columns and rows are both read off `Factor::ALL`, so a
/// caller-ordered list is normalized rather than labelled apart —
/// and a repeated factor is refused instead of silently doubling
/// the design.
#[test]
fn the_design_normalizes_its_factor_order_and_refuses_repeats() {
    let report = run_factorial(
        "skirmish",
        &[Factor::Geometry, Factor::Faction],
        1,
        20,
        7_000,
        oxide_sim::scenario::BotConfig::default(),
    )
    .unwrap();
    assert_eq!(report.factors, ["faction", "geometry"]);
    assert_eq!(
        report.per_factor[0].factor, "faction",
        "the marginals follow the same order as the columns"
    );
    let err = run_factorial(
        "skirmish",
        &[Factor::Geometry, Factor::Geometry],
        1,
        20,
        7_000,
        oxide_sim::scenario::BotConfig::default(),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("only once"), "{err}");
}

/// An unknown factor names the ones that exist.
#[test]
fn an_unknown_factor_lists_the_known_ones() {
    let err = Factor::parse("weather").unwrap_err().to_string();
    assert!(err.contains("faction") && err.contains("geometry"), "{err}");
}
