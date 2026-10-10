use super::*;

fn manifest() -> LadderManifest {
    serde_json::from_value(serde_json::json!({
        "name": "ladder",
        "tick_limit": 600,
        "runs": 2,
        "personality_seed_base": 9000,
        "stances": ["balanced", "aggressive"],
        "comparisons": [
            {"higher": "prime", "lower": "standard", "gate": 650},
            {"higher": "prime", "lower": "scrapheap", "gate": 800},
        ],
        "min_decided_pairs": 40,
        "maps": [{"path": "skirmish", "family": "open"}],
    }))
    .unwrap()
}

fn rungs(leg: &LadderLeg) -> Vec<(BotDifficulty, BotStance, u64)> {
    leg.plan
        .controllers
        .iter()
        .map(|controller| {
            let config = controller.unwrap().config();
            (config.difficulty, config.stance, config.personality_seed)
        })
        .collect()
}

#[test]
fn manifests_expand_into_seat_swapped_pairs_sharing_a_personality_seed() {
    let manifest = manifest();
    manifest.validate().unwrap();
    let legs = expand(&manifest, &[Scenario::skirmish()]).unwrap();
    assert_eq!(
        legs.len(),
        2 * 2 * 2 * 2,
        "comparisons × stances × runs × legs"
    );
    let (forward, swapped) = (&legs[0], &legs[1]);
    assert_eq!(forward.plan.leg, EvaluationLeg::Forward);
    assert_eq!(swapped.plan.leg, EvaluationLeg::Swapped);
    assert_eq!(forward.label, swapped.label);
    assert_eq!(
        rungs(forward),
        [
            (BotDifficulty::Prime, BotStance::Balanced, 9000),
            (BotDifficulty::Standard, BotStance::Balanced, 9000),
        ]
    );
    assert_eq!(
        rungs(swapped),
        [
            (BotDifficulty::Standard, BotStance::Balanced, 9000),
            (BotDifficulty::Prime, BotStance::Balanced, 9000),
        ]
    );
    let last = legs.last().unwrap();
    assert_eq!(
        (last.label.lower, last.label.stance, last.label.run),
        (BotDifficulty::Scrapheap, BotStance::Aggressive, 1)
    );
    assert_eq!(last.label.gate, 800);
}

#[test]
fn invalid_manifests_are_refused() {
    type Edit = fn(&mut LadderManifest);
    let edits: [(Edit, &str); 7] = [
        (
            |m| m.comparisons[0].higher = BotDifficulty::Scrapheap,
            "does not rank above",
        ),
        (
            |m| m.comparisons[1] = m.comparisons[0],
            "repeat a rung pair",
        ),
        (|m| m.comparisons[0].gate = 1_001, "per mille"),
        (|m| m.stances.clear(), "stances are empty"),
        (|m| m.maps.clear(), "maps are empty"),
        (|m| m.runs = 0, "runs must be positive"),
        (|m| m.min_decided_pairs = 0, "minimum must be positive"),
    ];
    for (edit, message) in edits {
        let mut manifest = manifest();
        edit(&mut manifest);
        let error = manifest.validate().unwrap_err().to_string();
        assert!(error.contains(message), "{error}");
    }
    let mut value = serde_json::to_value(serde_json::json!({
        "name": "ladder", "tick_limit": 1, "runs": 1, "scenario_seed_base": 0,
        "personality_seed_base": 0, "stances": ["balanced"],
        "comparisons": [{"higher": "prime", "lower": "standard", "gate": 650}],
        "min_decided_pairs": 1, "maps": [{"path": "skirmish", "family": "open"}],
    }))
    .unwrap();
    value["difficulties"] = serde_json::json!(["prime"]);
    assert!(serde_json::from_value::<LadderManifest>(value).is_err());
}

#[test]
fn maps_that_are_not_duels_are_refused() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("evaluation");
    let mut manifest = manifest();
    manifest.maps = vec![ManifestMap {
        path: "../../../scenarios/salvage-triangle.json".into(),
        family: MapFamily::Open,
    }];
    let scenarios = manifest.scenarios(&base.join("ladder")).unwrap();
    let error = expand(&manifest, &scenarios).unwrap_err().to_string();
    assert!(error.contains("not a duel"), "{error}");
}

#[test]
fn shipped_ladder_manifests_expand() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("evaluation/ladder");
    let mut names: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    names.sort();
    assert!(!names.is_empty());
    for path in names {
        let manifest = LadderManifest::load(&path).unwrap();
        let legs = expand(&manifest, &manifest.scenarios(&dir).unwrap()).unwrap();
        let per_comparison = manifest.maps.len()
            * manifest.stances.len()
            * usize::try_from(manifest.runs).unwrap()
            * 2;
        assert_eq!(
            legs.len(),
            manifest.comparisons.len() * per_comparison,
            "{}",
            path.display()
        );
    }
}

/// A scored leg of Prime against Standard on Skirmish, Balanced; `won`
/// names the winning rung, `None` an undecided leg.
fn row(run: u64, leg: EvaluationLeg, won: Option<BotDifficulty>) -> ScoredLadderRow {
    let (higher, lower) = (BotDifficulty::Prime, BotDifficulty::Standard);
    let seats = match leg {
        EvaluationLeg::Swapped => [lower, higher],
        _ => [higher, lower],
    };
    let winner_seats = won
        .map(|rung| {
            vec![u8::try_from(seats.iter().position(|seat| *seat == rung).unwrap()).unwrap()]
        })
        .unwrap_or_default();
    ScoredLadderRow {
        duration_ticks: 1_000,
        ladder: LadderLabel {
            manifest: "ladder".into(),
            map: "skirmish".into(),
            family: MapFamily::Open,
            higher,
            lower,
            gate: 650,
            min_decided_pairs: 3,
            stance: BotStance::Balanced,
            run,
        },
        leg,
        termination: if won.is_some() {
            Termination::Decided
        } else {
            Termination::TickLimit
        },
        winner_seats,
        seats: seats
            .iter()
            .enumerate()
            .map(|(seat, &difficulty)| LadderSeat {
                seat: u8::try_from(seat).unwrap(),
                config: Some(BotConfig::new(difficulty, BotStance::Balanced, 9000)),
            })
            .collect(),
        evidence: Vec::new(),
    }
}

fn pair(
    run: u64,
    forward: Option<BotDifficulty>,
    swapped: Option<BotDifficulty>,
) -> [ScoredLadderRow; 2] {
    [
        row(run, EvaluationLeg::Forward, forward),
        row(run, EvaluationLeg::Swapped, swapped),
    ]
}

#[test]
fn pairs_classify_by_both_legs_and_the_gate_needs_enough_decided_pairs() {
    use BotDifficulty::{Prime, Standard};
    let mut rows: Vec<ScoredLadderRow> = [
        pair(0, Some(Prime), Some(Prime)),
        pair(1, Some(Prime), Some(Standard)),
        pair(2, Some(Standard), Some(Standard)),
        pair(3, Some(Prime), None),
    ]
    .into_iter()
    .flatten()
    .collect();
    let report = build_report(&rows).unwrap();
    let [comparison] = &report.comparisons[..] else {
        panic!("{report:?}");
    };
    let tally = &comparison.overall;
    assert_eq!((tally.legs, tally.decided, tally.higher_wins), (8, 7, 4));
    assert_eq!(
        tally.pairs,
        PairTally {
            higher_both: 1,
            split: 1,
            lower_both: 1,
            undecided: 1,
        }
    );
    assert_eq!(tally.wilson, Some(wilson(4, 7)));
    assert_eq!(comparison.verdict, Verdict::Fail, "4/7 is under 65%");
    assert_eq!(comparison.stances[0].0, BotStance::Balanced);

    rows.extend(pair(4, Some(Prime), Some(Prime)));
    rows.extend(pair(5, Some(Prime), Some(Prime)));
    assert_eq!(
        build_report(&rows).unwrap().comparisons[0].verdict,
        Verdict::Pass,
        "8/11 reaches 65%"
    );

    let few: Vec<ScoredLadderRow> = pair(0, Some(Prime), Some(Prime)).into();
    assert_eq!(
        build_report(&few).unwrap().comparisons[0].verdict,
        Verdict::TooFewPairs
    );

    let mut unminimized: Vec<ScoredLadderRow> = pair(0, None, None).into();
    for row in &mut unminimized {
        row.ladder.min_decided_pairs = 0;
    }
    assert_eq!(
        build_report(&unminimized).unwrap().comparisons[0].verdict,
        Verdict::TooFewPairs,
        "no decided pair never passes"
    );
}

#[test]
fn manifests_that_play_the_same_legs_keep_separate_tallies() {
    use BotDifficulty::Prime;
    let full = pair(0, Some(Prime), Some(Prime));
    let mut smoke = full.clone();
    for row in &mut smoke {
        row.ladder.manifest = "ladder-smoke".into();
    }
    let rows: Vec<ScoredLadderRow> = full.into_iter().chain(smoke).collect();
    let report = build_report(&rows).unwrap();
    let tallies: Vec<(&str, u32)> = report
        .comparisons
        .iter()
        .map(|comparison| (comparison.manifest.as_str(), comparison.overall.legs))
        .collect();
    assert_eq!(tallies, [("ladder", 2), ("ladder-smoke", 2)]);
}

#[test]
fn incomplete_repeated_and_mismatched_rows_are_refused() {
    use BotDifficulty::Prime;
    let [forward, swapped] = pair(0, Some(Prime), Some(Prime));
    let error = build_report(std::slice::from_ref(&forward))
        .unwrap_err()
        .to_string();
    assert!(error.contains("lacks one of its two legs"), "{error}");
    let error = build_report(&[forward.clone(), forward.clone(), swapped.clone()])
        .unwrap_err()
        .to_string();
    assert!(error.contains("repeats"), "{error}");
    let mut wrong = swapped.clone();
    wrong.seats.swap(0, 1);
    wrong.seats[0].seat = 0;
    wrong.seats[1].seat = 1;
    let error = build_report(&[forward.clone(), wrong])
        .unwrap_err()
        .to_string();
    assert!(error.contains("needs oxide-opponent"), "{error}");
    let mut absent = swapped.clone();
    absent.winner_seats = vec![5];
    let error = build_report(&[forward.clone(), absent])
        .unwrap_err()
        .to_string();
    assert!(error.contains("winner seat 5"), "{error}");
    let mut regated = swapped;
    regated.ladder.run = 1;
    regated.ladder.gate = 700;
    let mut other = forward.clone();
    other.ladder.run = 1;
    other.ladder.gate = 700;
    let error = build_report(&[
        forward,
        pair(0, Some(Prime), Some(Prime))[1].clone(),
        other,
        regated,
    ])
    .unwrap_err()
    .to_string();
    assert!(error.contains("disagree on the gate"), "{error}");
}

#[test]
fn worth_shares_follow_the_higher_rung_and_ledgers_pool_by_rung() {
    use BotDifficulty::{Prime, Standard};
    let ledger = |worth: u64| SeatLedger {
        worth: vec![worth; 6],
        ..SeatLedger::default()
    };
    let mut rows: Vec<ScoredLadderRow> = pair(0, Some(Prime), Some(Prime)).into();
    for row in &mut rows {
        let higher = usize::from(row.leg == EvaluationLeg::Swapped);
        let mut evidence: Vec<SeatEvidence> = (0..2)
            .map(|_| SeatEvidence {
                ledger: Some(ledger(300)),
                ..SeatEvidence::default()
            })
            .collect();
        evidence[higher].ledger = Some(ledger(700));
        evidence[higher].attacks = Some(crate::bot_eval::AttackCalibration::default());
        row.evidence = evidence;
    }
    rows.extend(pair(1, Some(Standard), None));
    let report = build_report(&rows).unwrap();
    let shares = &report.comparisons[0].worth;
    assert_eq!(shares.len(), crate::ledger::WORTH_TICKS.len());
    assert!(
        shares
            .iter()
            .all(|share| share.pairs == 1 && (share.mean - 0.7).abs() < 1e-9),
        "rows without a ledger add no pair: {shares:?}"
    );
    let rungs: Vec<(BotDifficulty, u64, bool)> = report
        .rungs
        .iter()
        .map(|rung| {
            (
                rung.rung,
                rung.summary.ledger.seats,
                rung.summary.attacks.is_some(),
            )
        })
        .collect();
    assert_eq!(
        rungs,
        [(Standard, 2, false), (Prime, 2, true)],
        "lowest rung first"
    );
    assert!(report.render().contains("prime share of net worth"));
}

/// The interval brackets the point estimate and never leaves 0..1.
#[test]
fn the_wilson_interval_stays_inside_the_unit_range() {
    for (wins, n) in [(0u32, 1u32), (1, 1), (0, 8), (8, 8), (3, 7), (40, 95)] {
        let [lo, hi] = wilson(wins, n);
        let p = f64::from(wins) / f64::from(n);
        assert!((0.0..=1.0).contains(&lo) && (0.0..=1.0).contains(&hi));
        assert!(lo <= p && p <= hi, "{wins}/{n} -> [{lo}, {hi}]");
    }
}
