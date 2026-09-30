//! End-to-end contract coverage for `oxide-driver bot-matrix` and
//! `bot-matrix-report`.

use serde_json::Value;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "oxide-bot-matrix-cli-{name}-{}",
        std::process::id()
    ));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn driver(args: &[&OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args(args)
        .output()
        .expect("run oxide-driver")
}

fn succeeded(output: &Output) -> String {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

/// Writes a one-cell manifest over `maps` at 24 ticks.
fn manifest(dir: &Path, maps: Value) -> PathBuf {
    let path = dir.join("manifest.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "name": "cli",
            "tick_limit": 24,
            "runs": 1,
            "scenario_seed_base": 7000,
            "personality_seed_base": 9000,
            "difficulties": ["prime"],
            "stances": ["balanced"],
            "maps": maps,
        }))
        .unwrap(),
    )
    .unwrap();
    path
}

fn matrix(manifest: &Path, out: &Path, cache: &Path) -> Output {
    driver(&[
        "bot-matrix".as_ref(),
        manifest.as_os_str(),
        "--jobs".as_ref(),
        "2".as_ref(),
        "--out".as_ref(),
        out.as_os_str(),
        "--baseline-cache".as_ref(),
        cache.as_os_str(),
    ])
}

fn scenario(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../scenarios/{name}.json"))
}

fn rows(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn a_matrix_writes_rows_reuses_its_baseline_and_reports_its_own_rows() {
    let dir = scratch("two-maps");
    let manifest = manifest(
        &dir,
        serde_json::json!([
            {"path": "skirmish", "family": "open"},
            {"path": scenario("severance"), "family": "severed"},
        ]),
    );
    let cache = dir.join("cache");
    let run = |out: &Path| matrix(&manifest, out, &cache);

    let first = run(&dir.join("first"));
    let text = succeeded(&first);
    let stderr = String::from_utf8_lossy(&first.stderr);
    assert!(stderr.contains("evaluated 6 legs, reused 0"), "{stderr}");
    assert!(
        text.starts_with("bot-matrix: 4 head-to-head legs, 0 mixed legs, 2 baseline legs"),
        "{text}"
    );
    assert!(text.contains("family severed"), "{text}");
    assert!(text.contains("failure incidents"), "{text}");
    let first_rows = rows(&dir.join("first/rows.jsonl"));
    assert_eq!(first_rows.len(), 6);
    for row in &first_rows {
        assert_eq!(row["matrix"]["manifest"], "cli");
        assert_eq!(row["tick_limit"], 24);
        assert!(
            row["reference_digest"]
                .as_str()
                .unwrap()
                .starts_with("fnv1a64:")
        );
        assert!(row["evidence"][0]["failures"].is_object());
    }
    assert_eq!(first_rows[0]["matrix"]["pairing"], "head_to_head");
    assert_eq!(first_rows[0]["seats"][0]["controller"], "opponent");
    assert_eq!(first_rows[1]["seats"][1]["controller"], "opponent");
    assert_eq!(first_rows[2]["matrix"]["pairing"], "baseline");
    assert_eq!(first_rows[5]["matrix"]["map"], "severance");

    let second = run(&dir.join("second"));
    succeeded(&second);
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(stderr.contains("evaluated 4 legs, reused 2"), "{stderr}");
    assert_eq!(
        rows(&dir.join("second/rows.jsonl"))[2],
        first_rows[2],
        "the cached baseline row is republished"
    );

    let again = run(&dir.join("first"));
    assert!(!again.status.success(), "existing rows are never replaced");

    let report = driver(&[
        "bot-matrix-report".as_ref(),
        "--json".as_ref(),
        dir.join("first/rows.jsonl").as_os_str(),
    ]);
    let json: Value = serde_json::from_str(&succeeded(&report)).unwrap();
    assert_eq!(json["head_to_head_legs"], 4);
    assert_eq!(json["baseline_legs"], 2);
    assert_eq!(json["groups"][0]["group"], "overall");
    assert_eq!(
        json["groups"][0]["controllers"][0]["controller"],
        "opponent"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn team_and_free_for_all_maps_seat_every_chair_and_report_placement() {
    let dir = scratch("teams");
    let manifest = manifest(
        &dir,
        serde_json::json!([
            {"path": scenario("open-quarry"), "family": "open"},
            {"path": scenario("salvage-triangle"), "family": "open"},
        ]),
    );
    let cache = dir.join("cache");
    let first = matrix(&manifest, &dir.join("first"), &cache);
    let text = succeeded(&first);
    let stderr = String::from_utf8_lossy(&first.stderr);
    assert!(stderr.contains("evaluated 8 legs, reused 0"), "{stderr}");
    assert!(
        text.starts_with("bot-matrix: 2 head-to-head legs, 4 mixed legs, 2 baseline legs"),
        "{text}"
    );
    assert!(text.contains("placement in mixed legs"), "{text}");

    let rows = rows(&dir.join("first/rows.jsonl"));
    let pairings: Vec<&str> = rows
        .iter()
        .map(|row| row["matrix"]["pairing"].as_str().unwrap())
        .collect();
    assert_eq!(
        pairings,
        [
            "head_to_head",
            "head_to_head",
            "mixed",
            "mixed",
            "baseline",
            "mixed",
            "mixed",
            "baseline"
        ]
    );
    let controllers = |row: &Value| -> Vec<String> {
        row["seats"]
            .as_array()
            .unwrap()
            .iter()
            .map(|seat| seat["controller"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(
        controllers(&rows[0]),
        ["opponent", "opponent", "scripted", "scripted"],
        "a team of each bot"
    );
    let mixed = controllers(&rows[2]);
    assert_eq!(
        mixed.iter().filter(|seat| *seat == "opponent").count(),
        2,
        "{mixed:?}"
    );
    assert_eq!(
        controllers(&rows[5]),
        ["opponent", "scripted", "opponent"],
        "the authored human chair is seated"
    );

    let second = matrix(&manifest, &dir.join("second"), &cache);
    succeeded(&second);
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(stderr.contains("evaluated 6 legs, reused 2"), "{stderr}");

    let report = driver(&[
        "bot-matrix-report".as_ref(),
        "--json".as_ref(),
        dir.join("first/rows.jsonl").as_os_str(),
    ]);
    let json: Value = serde_json::from_str(&succeeded(&report)).unwrap();
    assert_eq!(json["mixed_legs"], 4);
    let overall = |mode: &str| {
        json["groups"]
            .as_array()
            .unwrap()
            .iter()
            .find(|group| group["mode"] == mode && group["group"] == "overall")
            .unwrap()
            .clone()
    };
    let teams = overall("teams");
    assert_eq!(teams["head_to_head"]["legs"]["legs"], 2);
    assert_eq!(teams["mixed"]["legs"]["legs"], 2);
    let free_for_all = overall("free_for_all");
    assert_eq!(free_for_all["mixed"]["pairs"]["undecided"], 1);
    assert_eq!(
        free_for_all["controllers"][0]["placement"]["mean_place"], 2.0,
        "every seat survives, so all tie"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_report_refuses_rows_from_bot_eval() {
    let dir = scratch("not-matrix");
    let rows = dir.join("rows.jsonl");
    let eval = driver(&[
        "bot-eval".as_ref(),
        "skirmish".as_ref(),
        "--ticks".as_ref(),
        "1".as_ref(),
        "--candidate".as_ref(),
        "cli".as_ref(),
        "--out".as_ref(),
        rows.as_os_str(),
    ]);
    succeeded(&eval);
    let report = driver(&["bot-matrix-report".as_ref(), rows.as_os_str()]);
    assert!(!report.status.success());
    assert!(
        String::from_utf8_lossy(&report.stderr).contains("is not a bot-matrix row"),
        "{}",
        String::from_utf8_lossy(&report.stderr)
    );
    std::fs::remove_dir_all(dir).unwrap();
}
