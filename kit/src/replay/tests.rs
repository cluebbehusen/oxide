use super::*;
use oxide_sim::SIM_VERSION;
use oxide_sim::scenario::ScenarioMode;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct TempReplay(PathBuf);

impl Drop for TempReplay {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn write_fixture(bytes: &[u8]) -> TempReplay {
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("oxide-kit-replay-{}-{id}.json", std::process::id()));
    std::fs::write(&path, bytes).expect("fixture is written");
    TempReplay(path)
}

fn current_document() -> Value {
    serde_json::to_value(GameReplay::new(SIM_VERSION, Scenario::skirmish()))
        .expect("current replay serializes")
}

#[test]
fn sandbox_replay_preserves_rules_through_the_strict_wire() {
    let mut setup = Scenario::skirmish();
    setup.mode = ScenarioMode::Sandbox;
    for row in &mut setup.map {
        *row = row.replace(['1', '2'], ".");
    }
    let mut replay = GameReplay::new(SIM_VERSION, setup.clone());
    replay.meta.ticks = Some(20);
    let fixture = write_fixture(&serde_json::to_vec(&replay).unwrap());
    let loaded = load_replay(&fixture.0).unwrap();
    assert_eq!(loaded.setup, setup);
    let world = crate::runner::run_replay(&loaded, None).unwrap();
    assert_eq!(world.current_tick(), 20);
    assert_eq!(world.mode(), ScenarioMode::Sandbox);
    assert!(world.result().is_none());
}

#[test]
fn current_replay_round_trips_through_the_strict_wire() {
    let document = current_document();
    let fixture = write_fixture(&serde_json::to_vec(&document).expect("fixture serializes"));

    let loaded = load_replay(&fixture.0).expect("current replay loads");
    assert_eq!(loaded.meta.sim_version, SIM_VERSION);
    assert_eq!(loaded.setup, Scenario::skirmish());
    assert!(loaded.commands.is_empty());
}

#[test]
fn current_replay_rejects_unknown_fields_at_strict_setup_boundaries() {
    let mut documents = Vec::new();

    let mut replay = current_document();
    replay["unexpected"] = json!(true);
    documents.push(("replay", replay));

    let mut setup = current_document();
    setup["setup"]["unexpected"] = json!(true);
    documents.push(("setup", setup));

    let mut player = current_document();
    player["setup"]["players"][1]["unexpected"] = json!(true);
    documents.push(("player", player));

    let mut config = current_document();
    config["setup"]["players"][1]["bot_config"]["unexpected"] = json!(true);
    documents.push(("bot config", config));

    for (boundary, document) in documents {
        let fixture = write_fixture(&serde_json::to_vec(&document).expect("fixture serializes"));
        assert!(
            load_replay(&fixture.0).is_err(),
            "unknown field was accepted at the {boundary} boundary"
        );
    }
}

#[test]
fn current_replay_rejects_duplicate_bot_config_fields() {
    let json = serde_json::to_string(&GameReplay::new(SIM_VERSION, Scenario::skirmish()))
        .expect("current replay serializes");
    let needle = r#""bot_config":{}"#;
    let replacement = concat!(r#""bot_config":{},"#, r#""bot_config":{}"#);
    let duplicate = json.replacen(needle, replacement, 1);
    assert_ne!(duplicate, json, "fixture must duplicate a real field");
    let fixture = write_fixture(duplicate.as_bytes());

    assert!(load_replay(&fixture.0).is_err());
}
