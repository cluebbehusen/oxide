//! Targeted diagnostic for a neural policy's repair verbs.
//!
//! Unlike the composition gate, this fixture asks a narrow capability
//! question: when repair is useful and legal, does the policy issue the
//! commands, finish a Bay, and actually restore any value? Nothing here
//! enters training reward or promotion scoring.

use anyhow::{Context, Result, ensure};
use oxide_sim::bot::{NeuralBot, QuantNet};
use oxide_sim::scenario::{Scenario, UnitSpec};
use oxide_sim::{BuildingKind, Command, Event, Faction, PlayerId, State, UnitId, UnitKind};
use serde::Serialize;
use std::path::Path;

const CADENCE: u64 = 16;
const SKILL: u32 = 1000;
const AGGRESSION: u32 = 500;
const BLUNDER: u32 = 0;
const CASE_SEEDS: [u64; 2] = [13_101, 13_102];

#[derive(Debug, Clone, Copy)]
struct CaseSpec {
    seed: u64,
    seat: u8,
    faction: Faction,
}

#[derive(Debug)]
struct Fixture {
    state: State,
    wounded: Vec<(UnitId, u32)>,
}

#[derive(Debug, Serialize)]
struct ProbeProfile {
    cadence: u64,
    skill: u32,
    aggression: u32,
    blunder: u32,
}

#[derive(Debug, Serialize)]
struct RepairCase {
    seed: u64,
    controlled_seat: u8,
    faction: Faction,
    ticks: u64,
    repair_unit_commands: u64,
    repair_bay_build_attempts: u64,
    repair_bay_completions: u64,
    initial_damaged_purchase_value: u64,
    final_damaged_purchase_value: u64,
    actual_healing: bool,
}

#[derive(Debug, Serialize)]
struct RepairTotals {
    repair_unit_commands: u64,
    repair_bay_build_attempts: u64,
    repair_bay_completions: u64,
    initial_damaged_purchase_value: u64,
    final_damaged_purchase_value: u64,
    actual_healing: bool,
    cases_with_healing: u64,
}

#[derive(Debug, Serialize)]
struct RepairProbeReport {
    schema: u32,
    artifact: String,
    digest: String,
    max_ticks: u64,
    profile: ProbeProfile,
    cases: Vec<RepairCase>,
    totals: RepairTotals,
}

/// Runs the deterministic repair fixture and prints one JSON report.
///
/// When `out` is present, the same report is also written there. The
/// artifact digest is included so copied experiment results retain their
/// provenance after a run directory disappears.
pub fn repair_probe(weights: &Path, max_ticks: u64, out: Option<&Path>) -> Result<()> {
    let report = run_probe(weights, max_ticks)?;
    let json = serde_json::to_string_pretty(&report)?;
    println!("{json}");
    if let Some(path) = out {
        std::fs::write(path, &json).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

fn run_probe(weights: &Path, max_ticks: u64) -> Result<RepairProbeReport> {
    ensure!(max_ticks > 0, "repair probe needs at least one tick");
    let json = std::fs::read_to_string(weights)
        .with_context(|| format!("reading weights {}", weights.display()))?;
    let net = QuantNet::from_json(&json).map_err(anyhow::Error::msg)?;
    let digest = format!("{:016x}", net.digest());

    let specs: Vec<CaseSpec> = CASE_SEEDS
        .into_iter()
        .flat_map(|seed| {
            [0u8, 1].into_iter().flat_map(move |seat| {
                [Faction::Ferrous, Faction::Cupric]
                    .into_iter()
                    .map(move |faction| CaseSpec {
                        seed,
                        seat,
                        faction,
                    })
            })
        })
        .collect();
    let mut cases = Vec::with_capacity(specs.len());
    for spec in specs {
        cases.push(run_case(spec, &net, max_ticks)?);
    }

    let totals = RepairTotals {
        repair_unit_commands: cases.iter().map(|case| case.repair_unit_commands).sum(),
        repair_bay_build_attempts: cases
            .iter()
            .map(|case| case.repair_bay_build_attempts)
            .sum(),
        repair_bay_completions: cases.iter().map(|case| case.repair_bay_completions).sum(),
        initial_damaged_purchase_value: cases
            .iter()
            .map(|case| case.initial_damaged_purchase_value)
            .sum(),
        final_damaged_purchase_value: cases
            .iter()
            .map(|case| case.final_damaged_purchase_value)
            .sum(),
        actual_healing: cases.iter().any(|case| case.actual_healing),
        cases_with_healing: cases.iter().filter(|case| case.actual_healing).count() as u64,
    };
    Ok(RepairProbeReport {
        schema: 1,
        artifact: weights.display().to_string(),
        digest,
        max_ticks,
        profile: ProbeProfile {
            cadence: CADENCE,
            skill: SKILL,
            aggression: AGGRESSION,
            blunder: BLUNDER,
        },
        cases,
        totals,
    })
}

fn run_case(spec: CaseSpec, net: &QuantNet, max_ticks: u64) -> Result<RepairCase> {
    let Fixture { mut state, wounded } = fixture(spec)?;
    let player = PlayerId(spec.seat);
    let initial_damaged_purchase_value = damaged_purchase_value(&state, player);
    let mut bot = NeuralBot::with_profile(
        player,
        CADENCE,
        net.clone(),
        SKILL,
        AGGRESSION,
        spec.faction,
        BLUNDER,
        spec.seed,
    );
    let mut repair_unit_commands = 0u64;
    let mut repair_bay_build_attempts = 0u64;
    let mut repair_bay_completions = 0u64;
    let mut actual_healing = false;

    for _ in 0..max_ticks {
        let commands = bot.act(&state);
        repair_unit_commands += commands
            .iter()
            .filter(|command| matches!(command.command, Command::RepairUnit { .. }))
            .count() as u64;
        repair_bay_build_attempts += commands
            .iter()
            .filter(|command| {
                matches!(
                    command.command,
                    Command::Build {
                        kind: BuildingKind::RepairBay,
                        ..
                    }
                )
            })
            .count() as u64;
        let report = state.tick(&commands);
        repair_bay_completions += report
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    Event::BuildingCompleted {
                        player: owner,
                        kind: BuildingKind::RepairBay,
                        ..
                    } if *owner == player
                )
            })
            .count() as u64;
        actual_healing |= wounded
            .iter()
            .any(|(id, initial_hp)| state.unit(*id).is_some_and(|unit| unit.hp > *initial_hp));
        if state.result().is_some() {
            break;
        }
    }

    Ok(RepairCase {
        seed: spec.seed,
        controlled_seat: spec.seat,
        faction: spec.faction,
        ticks: state.current_tick(),
        repair_unit_commands,
        repair_bay_build_attempts,
        repair_bay_completions,
        initial_damaged_purchase_value,
        final_damaged_purchase_value: damaged_purchase_value(&state, player),
        actual_healing,
    })
}

fn fixture(spec: CaseSpec) -> Result<Fixture> {
    let mut scenario = Scenario::skirmish();
    scenario.seed = spec.seed;
    for (seat, player) in scenario.players.iter_mut().enumerate() {
        player.scrap = if seat == usize::from(spec.seat) {
            1_000
        } else {
            0
        };
        player.bot = false;
        player.bot_config = None;
    }
    scenario.units.retain(|unit| unit.player == spec.seat);
    let width = scenario.map[0].len() as i32;
    let height = scenario.map.len() as i32;
    let orient = |x: i32, y: i32| {
        if spec.seat == 0 {
            (x, y)
        } else {
            (width - 1 - x, height - 1 - y)
        }
    };
    for (kind, x, y) in [
        (UnitKind::Lancer, 10, 7),
        (UnitKind::Scuttler, 10, 8),
        (UnitKind::Bombard, 9, 8),
    ] {
        let (x, y) = orient(x, y);
        scenario.units.push(UnitSpec {
            player: spec.seat,
            kind,
            x,
            y,
        });
    }
    scenario.retint_seat(usize::from(spec.seat), spec.faction);
    let state = scenario.build().context("building repair-probe fixture")?;
    let wounds: Vec<(usize, UnitId, u32)> = state
        .units()
        .iter()
        .enumerate()
        .filter(|(_, unit)| {
            unit.player == PlayerId(spec.seat)
                && unit.kind != UnitKind::Harvester
                && unit.kind.stats().domain == oxide_sim::stats::Domain::Ground
        })
        .map(|(index, unit)| (index, unit.id, (unit.kind.stats().max_hp / 3).max(1)))
        .collect();
    ensure!(
        wounds.len() >= 3,
        "repair fixture needs several wounded ground units"
    );
    let mut json = serde_json::to_value(state)?;
    let units = json["units"]
        .as_array_mut()
        .context("serialized State carries a unit array")?;
    for (index, _, hp) in &wounds {
        units[*index]["hp"] = serde_json::json!(hp);
    }
    let state: State = serde_json::from_value(json).context("validating wounded fixture")?;
    Ok(Fixture {
        state,
        wounded: wounds.into_iter().map(|(_, id, hp)| (id, hp)).collect(),
    })
}

fn damaged_purchase_value(state: &State, player: PlayerId) -> u64 {
    state
        .units()
        .iter()
        .filter(|unit| {
            unit.player == player
                && unit.kind.stats().domain == oxide_sim::stats::Domain::Ground
                && unit.hp < unit.kind.stats().max_hp
        })
        .map(|unit| {
            let stats = unit.kind.stats();
            u64::from(stats.cost) * u64::from(stats.max_hp - unit.hp) / u64::from(stats.max_hp)
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn incumbent_path(label: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "oxide-repair-probe-{label}-{}.json",
            std::process::id()
        ));
        std::fs::write(&path, include_str!("../../sim/src/bot/ladder_weights.json")).unwrap();
        path
    }

    #[test]
    fn fixture_is_wounded_fog_honest_and_pressure_free_in_every_orientation() {
        for seat in [0u8, 1] {
            for faction in [Faction::Ferrous, Faction::Cupric] {
                let fixture = fixture(CaseSpec {
                    seed: CASE_SEEDS[0],
                    seat,
                    faction,
                })
                .unwrap();
                let player = PlayerId(seat);
                assert_eq!(fixture.wounded.len(), 4);
                assert_eq!(fixture.state.player(player).faction, faction);
                assert!(
                    fixture.state.buildings().iter().any(|building| {
                        building.player != player
                            && building.kind == BuildingKind::Foundry
                            && building.built
                    }),
                    "the idle opponent still keeps the match alive"
                );
                assert!(
                    fixture
                        .state
                        .units()
                        .iter()
                        .all(|unit| unit.player == player),
                    "no opposing unit pressures the repair fixture"
                );
                assert!(damaged_purchase_value(&fixture.state, player) > 0);
            }
        }
    }

    #[test]
    fn report_carries_provenance_shape_and_the_incumbents_zero_use_baseline() {
        let path = incumbent_path("shape");
        let report = run_probe(&path, 512).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(
            report.digest,
            format!("{:016x}", QuantNet::ladder().digest())
        );
        assert_eq!(report.cases.len(), 8);
        assert_eq!(report.totals.repair_unit_commands, 0);
        assert_eq!(report.totals.repair_bay_build_attempts, 0);
        assert_eq!(report.totals.repair_bay_completions, 0);
        assert!(!report.totals.actual_healing);
        assert_eq!(report.totals.cases_with_healing, 0);
        assert_eq!(
            report.totals.initial_damaged_purchase_value,
            report.totals.final_damaged_purchase_value
        );

        let json = serde_json::to_value(report).unwrap();
        assert_eq!(json["schema"], 1);
        assert!(json["artifact"].is_string());
        assert!(json["digest"].is_string());
        assert_eq!(json["profile"]["cadence"], CADENCE);
        assert!(json["cases"].is_array());
        for key in [
            "repair_unit_commands",
            "repair_bay_build_attempts",
            "repair_bay_completions",
            "initial_damaged_purchase_value",
            "final_damaged_purchase_value",
            "actual_healing",
        ] {
            assert!(
                !json["totals"][key].is_null(),
                "totals carries the {key} diagnostic"
            );
        }
    }
}
