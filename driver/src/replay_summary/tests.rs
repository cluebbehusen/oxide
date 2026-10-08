use super::*;
use crate::support_tests::replay_fixture as fixture;

use oxide_sim::Scenario;

fn opts(until: Option<u64>, every: Option<u64>) -> SummaryOptions {
    SummaryOptions {
        until,
        every,
        minimaps: MinimapMode::None,
    }
}

#[test]
fn clock_formats_minutes_and_seconds() {
    assert_eq!(clock(0), "0:00");
    assert_eq!(clock(1_800), "1:30");
    assert_eq!(clock(160_000), "133:20");
}

#[test]
fn first_contact_requires_hostility_and_is_emitted_once_per_team_pair() {
    let state = Scenario::skirmish().build().expect("skirmish builds");
    let mut contacted = BTreeSet::new();
    let mut timeline = Vec::new();

    note_contact(
        &state,
        None,
        Some(1),
        TilePos::new(1, 1),
        10,
        &mut contacted,
        &mut timeline,
    );
    note_contact(
        &state,
        Some(0),
        Some(0),
        TilePos::new(2, 2),
        11,
        &mut contacted,
        &mut timeline,
    );
    note_contact(
        &state,
        Some(0),
        Some(1),
        TilePos::new(3, 4),
        12,
        &mut contacted,
        &mut timeline,
    );
    note_contact(
        &state,
        Some(1),
        Some(0),
        TilePos::new(9, 9),
        13,
        &mut contacted,
        &mut timeline,
    );

    assert_eq!(timeline.len(), 1);
    assert_eq!(timeline[0].tick, 12);
    match &timeline[0].kind {
        TimelineKind::FirstContact { teams, at } => {
            assert_eq!(*teams, (0, 1));
            assert_eq!(*at, [3, 4]);
        }
        other => panic!("expected first contact, got {other:?}"),
    }
}

#[test]
fn tech_firsts_deduplicate_and_only_loud_milestones_enter_the_timeline() {
    let mut reach = BTreeSet::new();
    let mut firsts = Vec::new();
    let mut timeline = Vec::new();
    note_tech_first(
        &mut reach,
        &mut firsts,
        1u8,
        "avalanche",
        true,
        0,
        40,
        &mut timeline,
    );
    note_tech_first(
        &mut reach,
        &mut firsts,
        1u8,
        "avalanche",
        true,
        0,
        60,
        &mut timeline,
    );
    note_tech_first(
        &mut reach,
        &mut firsts,
        2u8,
        "sentinel",
        false,
        0,
        80,
        &mut timeline,
    );

    assert_eq!(firsts.len(), 2);
    assert_eq!((firsts[0].name.as_str(), firsts[0].tick), ("avalanche", 40));
    assert_eq!((firsts[1].name.as_str(), firsts[1].tick), ("sentinel", 80));
    assert_eq!(timeline.len(), 1);
    assert!(matches!(
        &timeline[0].kind,
        TimelineKind::TechFirst { seat: 0, name } if name == "avalanche"
    ));
}

#[test]
fn lulls_begin_only_after_contact_and_need_two_quiet_windows() {
    let mut quiet = None;
    let mut timeline = Vec::new();
    track_lull(&mut quiet, &mut timeline, 0, false, 0, 100);
    assert!(quiet.is_none(), "opening buildup is not a lull");

    track_lull(&mut quiet, &mut timeline, 0, true, 100, 200);
    track_lull(&mut quiet, &mut timeline, 1, true, 200, 300);
    assert!(timeline.is_empty(), "one quiet window is not notable");

    track_lull(&mut quiet, &mut timeline, 0, true, 300, 400);
    track_lull(&mut quiet, &mut timeline, 0, true, 400, 500);
    track_lull(&mut quiet, &mut timeline, 2, true, 500, 600);
    assert_eq!(timeline.len(), 1);
    match timeline[0].kind {
        TimelineKind::Lull { from_tick, to_tick } => {
            assert_eq!((from_tick, to_tick), (300, 500));
        }
        ref other => panic!("expected lull, got {other:?}"),
    }
}

#[test]
fn nearby_losses_merge_and_distant_losses_split() {
    let mut clusterer = BattleClusterer::default();
    clusterer.note_loss(100, 0, TilePos::new(10, 10), 90, LossKind::Combat);
    clusterer.note_loss(200, 1, TilePos::new(18, 10), 110, LossKind::Combat);
    clusterer.note_loss(250, 0, TilePos::new(40, 40), 500, LossKind::Combat);
    clusterer.finish();
    assert_eq!(clusterer.battles.len(), 2);
    let merged = &clusterer.battles[0];
    assert_eq!(merged.from_tick, 100);
    assert_eq!(merged.to_tick, 200);
    assert_eq!(merged.losses.len(), 2);
    assert_eq!(clusterer.battles[1].losses[0].value, 500);
}

#[test]
fn a_quiet_gap_closes_the_battle() {
    let mut clusterer = BattleClusterer::default();
    clusterer.note_loss(100, 0, TilePos::new(10, 10), 300, LossKind::Combat);
    clusterer.note_loss(
        100 + BATTLE_QUIET_TICKS + 1,
        1,
        TilePos::new(10, 10),
        300,
        LossKind::Combat,
    );
    clusterer.finish();
    assert_eq!(clusterer.battles.len(), 2);
}

#[test]
fn sub_floor_battles_fold_into_skirmishes() {
    let mut clusterer = BattleClusterer::default();
    clusterer.note_loss(
        100,
        0,
        TilePos::new(10, 10),
        BATTLE_VALUE_FLOOR - 20,
        LossKind::Combat,
    );
    clusterer.note_loss(
        2_000,
        0,
        TilePos::new(10, 10),
        BATTLE_VALUE_FLOOR + 10,
        LossKind::Combat,
    );
    clusterer.finish();
    assert_eq!(clusterer.battles.len(), 1);
    assert_eq!(clusterer.skirmishes, vec![100]);
}

#[test]
fn verdicts_read_the_loss_gap_by_team() {
    let loss = |seat, value| SeatLoss {
        seat,
        value,
        units: 1,
        buildings: 0,
        sites: 0,
        combat_value: value,
        worker_value: 0,
        transport_value: 0,
        building_value: 0,
    };
    let duel = &[0u8, 1];
    assert_eq!(battle_verdict(&[loss(0, 400), loss(1, 300)], duel), "even");
    assert_eq!(
        battle_verdict(&[loss(0, 400), loss(1, 100)], duel),
        "favors team 1"
    );
    assert_eq!(
        battle_verdict(&[loss(0, 400)], duel),
        "one-sided against team 0"
    );
    // Two allies each losing "less than" their lone opponent still lost
    // the exchange as a team.
    let two_on_one = &[0u8, 1, 0];
    assert_eq!(
        battle_verdict(&[loss(1, 400), loss(0, 300), loss(2, 300)], two_on_one),
        "favors team 1"
    );
}

#[test]
fn every_timeline_moment_renders() {
    let loss = SeatLoss {
        seat: 0,
        value: 300,
        units: 3,
        buildings: 1,
        sites: 0,
        combat_value: 0,
        worker_value: 0,
        transport_value: 0,
        building_value: 0,
    };
    let cases: Vec<(TimelineKind, &str)> = vec![
        (
            TimelineKind::FirstContact {
                teams: (0, 1),
                at: [3, 4],
            },
            "first contact: team 0 <-> team 1 at (3,4)",
        ),
        (
            TimelineKind::Battle {
                from_tick: 0,
                to_tick: 600,
                at: [5, 6],
                losses: vec![loss],
                verdict: "even".into(),
                shootdown: false,
            },
            "battle 0:00-0:30 at (5,6): seat 0 lost 300 (3u+1b) — even",
        ),
        (
            TimelineKind::Expansion {
                seat: 2,
                at: Some([7, 8]),
            },
            "expansion: seat 2 foundry at (7,8)",
        ),
        (
            TimelineKind::ExtractorOnline { seat: 1, at: None },
            "extractor online: seat 1",
        ),
        (
            TimelineKind::TechFirst {
                seat: 3,
                name: "avalanche".into(),
            },
            "tech first: seat 3 avalanche",
        ),
        (
            TimelineKind::FoundryLost {
                seat: 4,
                at: [9, 1],
            },
            "foundry lost: seat 4 at (9,1)",
        ),
        (TimelineKind::Elimination { seat: 4 }, "eliminated: seat 4"),
        (TimelineKind::Resignation { seat: 5 }, "resigned: seat 5"),
        (
            TimelineKind::Lull {
                from_tick: 1_200,
                to_tick: 4_800,
            },
            "lull: no combat 1:00-4:00",
        ),
        (
            TimelineKind::GameOver {
                result: GameResult::Victory { team: 1 },
                winner_seats: vec![1, 3],
            },
            "game over: victory team 1 (seats: 1, 3)",
        ),
    ];
    for (kind, expected) in cases {
        assert_eq!(render_moment(&kind), expected);
    }
    assert_eq!(render_result(GameResult::Draw, &[]), "draw");
}

#[test]
fn minimap_fits_bounds_and_marks_foundries_and_scrap() {
    let state = Scenario::skirmish().build().expect("skirmish builds");
    let map = minimap(&state);
    let repeat = minimap(&state);
    assert_eq!(map, repeat, "minimap must be deterministic");
    assert!(map.len() <= 16);
    assert!(map.iter().all(|row| row.chars().count() <= 46));
    let all: String = map.concat();
    assert!(all.contains('0'), "seat 0 foundry digit missing:\n{all}");
    assert!(all.contains('1'), "seat 1 foundry digit missing:\n{all}");
    assert!(all.contains('$'), "scrap marker missing:\n{all}");
}

#[test]
fn digests_land_on_the_stride_plus_the_closing_tick() {
    let report = summarize(&fixture(), &opts(None, Some(5))).expect("fixture summarizes");
    assert_eq!(
        report
            .digests
            .iter()
            .map(|digest| digest.tick)
            .collect::<Vec<_>>(),
        vec![5, 10, 12]
    );
    assert_eq!(report.scenario.effective_ticks, 12);
    assert_eq!(report.digests.len(), 3);
}

#[test]
fn zero_tick_and_legacy_duration_records_keep_an_exact_closing_digest() {
    let mut empty = fixture();
    empty.commands.clear();
    empty.meta.ticks = Some(0);
    let report = summarize(&empty, &opts(None, Some(5))).expect("empty record summarizes");
    assert_eq!(report.scenario.effective_ticks, 0);
    assert_eq!(
        report.digests.iter().map(|d| d.tick).collect::<Vec<_>>(),
        vec![0]
    );

    let mut legacy = fixture();
    legacy.meta.ticks = None;
    let expected = legacy.commands.last().unwrap().tick + 1;
    let report = summarize(&legacy, &opts(None, Some(5))).expect("legacy record summarizes");
    assert_eq!(report.scenario.effective_ticks, expected);
    assert_eq!(
        report.digests.last().map(|digest| digest.tick),
        Some(expected)
    );
}

#[test]
fn until_truncates_without_an_unconsumed_commands_error() {
    let report = summarize(&fixture(), &opts(Some(5), Some(4))).expect("truncation is fine");
    assert_eq!(
        report
            .digests
            .iter()
            .map(|digest| digest.tick)
            .collect::<Vec<_>>(),
        vec![4, 5]
    );
    assert_eq!(report.scenario.effective_ticks, 5);
    assert_eq!(report.scenario.total_ticks, 12);
}

#[test]
fn render_is_deterministic_and_carries_the_header() {
    let report = summarize(&fixture(), &opts(None, None)).expect("fixture summarizes");
    let text = report.render();
    let again = summarize(&fixture(), &opts(None, None))
        .expect("fixture summarizes")
        .render();
    assert_eq!(text, again);
    assert!(text.contains("Skirmish Basin"));
    assert!(text.contains("digest t=12"));
}
