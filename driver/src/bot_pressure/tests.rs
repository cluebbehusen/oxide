use super::*;
use oxide_sim::UnitKind;
use oxide_sim::scenario::UnitSpec;

fn shipped() -> Vec<PressureScenario> {
    load_all(&Path::new(env!("CARGO_MANIFEST_DIR")).join("evaluation/pressure")).unwrap()
}

fn named(name: &str) -> PressureScenario {
    shipped()
        .into_iter()
        .find(|pressure| pressure.name == name)
        .unwrap()
}

/// Unit ids a scripted command names, from its serialized form.
fn named_units(command: &Command) -> Vec<u64> {
    let value = serde_json::to_value(command).unwrap();
    let mut ids: Vec<u64> = value["units"]
        .as_array()
        .map(|units| units.iter().filter_map(serde_json::Value::as_u64).collect())
        .unwrap_or_default();
    ids.extend(value["transport"].as_u64());
    ids
}

#[test]
fn shipped_scenarios_script_only_the_attackers_units() {
    let scenarios = shipped();
    assert_eq!(scenarios.len(), 4);
    for pressure in &scenarios {
        pressure.scenario.build().unwrap();
        let seats = &pressure.scenario.players;
        assert!(
            seats[usize::from(pressure.defender)].bot,
            "{}",
            pressure.name
        );
        assert!(
            !seats[usize::from(pressure.attacker)].bot,
            "{}",
            pressure.name
        );
        assert!(!pressure.script.is_empty(), "{}", pressure.name);
        for scripted in &pressure.script {
            assert!(scripted.tick < pressure.deadline, "{}", pressure.name);
            for id in named_units(&scripted.command) {
                let unit = pressure.scenario.units[usize::try_from(id).unwrap()];
                assert_eq!(
                    unit.player, pressure.attacker,
                    "{} unit {id}",
                    pressure.name
                );
            }
        }
    }
}

#[test]
fn anti_air_means_a_dedicated_unit_or_a_built_flak_turret() {
    let pressure = named("air switch");
    let state = pressure.scenario.build().unwrap();
    let defender = PlayerId(pressure.defender);
    assert!(
        !owns_anti_air(&state, defender),
        "Sentinels alone do not count"
    );
    let mut armed = pressure.scenario.clone();
    armed.units.push(UnitSpec {
        player: pressure.defender,
        kind: UnitKind::Flakhound,
        x: 12,
        y: 10,
    });
    assert!(owns_anti_air(&armed.build().unwrap(), defender));
}

#[test]
fn a_standing_foundry_passes_and_an_unstaged_threat_is_an_error() {
    let pressure = named("early rush");
    let state = pressure.scenario.build().unwrap();
    let (defender, attacker) = (PlayerId(pressure.defender), PlayerId(pressure.attacker));
    let (passed, _) = Watch::new(Check::KeepsFoundry)
        .verdict(&state, defender, attacker)
        .unwrap();
    assert!(passed);
    assert!(
        Watch::new(Check::ClearsLanding)
            .verdict(&state, defender, attacker)
            .is_err()
    );
    assert!(
        Watch::new(Check::AnswersAir { within: 1 })
            .verdict(&state, defender, attacker)
            .is_err()
    );
}

#[test]
fn a_carrier_downed_before_landing_passes_and_one_still_flying_is_an_error() {
    let pressure = named("lift drop");
    let state = pressure.scenario.build().unwrap();
    let (defender, attacker) = (PlayerId(pressure.defender), PlayerId(pressure.attacker));
    let skyhook = state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Skyhook)
        .unwrap()
        .id;
    let mut watch = Watch::new(Check::ClearsLanding);
    watch.carriers.push(skyhook);
    assert!(watch.verdict(&state, defender, attacker).is_err());
    watch.carriers = vec![UnitId(u32::MAX)];
    let (passed, detail) = watch.verdict(&state, defender, attacker).unwrap();
    assert!(passed, "{detail}");
}

#[test]
fn a_recorded_replay_lasts_until_the_run_stopped() {
    let mut pressure = named("early rush");
    pressure.deadline = 36;
    let (outcome, replay) = run(&pressure, true).unwrap();
    assert_eq!(outcome.ticks, 36);
    assert_eq!(replay.unwrap().meta.ticks, Some(36));
}

#[test]
fn a_bot_attacker_or_a_defender_without_a_bot_is_refused() {
    let mut pressure = named("early rush");
    pressure.scenario.players[usize::from(pressure.attacker)].bot = true;
    assert!(run(&pressure, false).is_err());
    let mut pressure = named("early rush");
    pressure.scenario.players[usize::from(pressure.defender)].bot_config = None;
    assert!(run(&pressure, false).is_err());
}
