use super::*;

fn evidence(idle: u64, income: [(u64, u32, u32); 2]) -> serde_json::Value {
    serde_json::json!({
        "seat": 0,
        "failures": {
            "repeated_orders": {"incidents": 1, "examples": []},
            "abandoned_sites": {"incidents": 0, "examples": []},
            "starved_production": {"incidents": 0, "examples": []},
            "stuck_missions": {"incidents": 0, "examples": []},
            "idle_army": {"incidents": idle, "examples": []},
        },
        "income": income
            .iter()
            .map(|(tick, actual, saturation)| serde_json::json!({
                "tick": tick,
                "actual_per_minute": actual,
                "saturation_per_minute": saturation,
            }))
            .collect::<Vec<_>>(),
    })
}

fn row(teams: &[u8], difficulties: &[&str], idle: u64) -> String {
    serde_json::json!({
        "duration_ticks": 9_000,
        "seats": teams
            .iter()
            .zip(difficulties)
            .enumerate()
            .map(|(seat, (team, difficulty))| serde_json::json!({
                "seat": seat,
                "team": team,
                "config": {"difficulty": difficulty},
            }))
            .collect::<Vec<_>>(),
        "evidence": teams
            .iter()
            .map(|_| evidence(idle, [(6_000, 900, 1_000), (12_000, 600, 1_200)]))
            .collect::<Vec<_>>(),
    })
    .to_string()
}

#[test]
fn rows_pool_by_layout_and_difficulty_from_the_lowest_rung_up() {
    let dir = std::env::temp_dir().join(format!("oxide-seat-summary-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rows.jsonl");
    let rows = [
        row(&[0, 1], &["prime", "standard"], 2),
        row(&[0, 1], &["prime", "standard"], 0),
        row(&[0, 0, 1, 1], &["standard"; 4], 1),
        row(&[0, 0, 1], &["standard"; 3], 0),
        row(&[0, 0, 1, 1, 2, 2], &["standard"; 6], 0),
    ];
    std::fs::write(&path, rows.join("\n")).unwrap();
    let summary = summarize(&[path]).unwrap();
    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(summary.legs, 5);
    let groups: Vec<(&str, u32, u64, u64)> = summary
        .groups
        .iter()
        .map(|(label, seats)| {
            (
                label.as_str(),
                seats.seat_legs,
                seats.repeated_orders,
                seats.idle_army,
            )
        })
        .collect();
    assert_eq!(
        groups,
        [
            ("2v1 standard", 3, 3, 0),
            ("2v2v2 standard", 6, 6, 0),
            ("duel standard", 2, 2, 2),
            ("duel prime", 2, 2, 2),
            ("teams standard", 4, 4, 4),
        ]
    );
    let income = &summary.groups[2].1.income;
    assert_eq!(income.len(), 2);
    assert_eq!(
        (income[0].samples, income[0].percent_of_saturation),
        (2, Some(90))
    );
    let text = summary.render();
    assert!(text.contains("failure incidents"), "{text}");
    assert!(text.contains("teams standard"), "{text}");
}
