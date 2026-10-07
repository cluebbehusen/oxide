//! Staged pressure scenarios. Each stages one situation a credible opponent
//! must answer: a scripted attacker seat issues tick-stamped commands for its
//! preset units while the defender seat's bot plays, and a check reads the
//! authoritative state to decide whether the bot answered in time.
//!
//! The checks read omniscient QA state and never reach a controller.

use anyhow::{Context, Result, bail, ensure};
use oxide_kit::GameReplay;
use oxide_kit::controller::{record_events, seat_controllers};

use oxide_sim::stats::{Domain, Role};
use oxide_sim::{BuildingKind, Command, PlayerCommand, PlayerId, Scenario, State, UnitId};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One staged situation.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PressureScenario {
    /// Short name used in reports.
    pub name: String,
    /// The staged match. The defender seat must be a configured bot; the
    /// attacker seat is driven only by `script`.
    pub scenario: Scenario,
    /// The bot seat under test.
    pub defender: u8,
    /// The scripted seat.
    pub attacker: u8,
    /// Ticks the check waits for.
    pub deadline: u64,
    /// Commands for the attacker's preset units. Unit ids follow scenario
    /// order.
    pub script: Vec<Scripted>,
    /// What counts as an answer.
    pub check: Check,
}

/// One attacker command, issued before the given tick executes.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scripted {
    /// Tick the command is stamped with.
    pub tick: u64,
    /// The command.
    pub command: Command,
}

/// What the defender must achieve by the deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Check {
    /// The defender still owns a Foundry.
    KeepsFoundry,
    /// Within `within` ticks of first seeing an attacker aircraft, the
    /// defender owns anti-air: a unit built for it or a built Flak Turret.
    AnswersAir {
        /// Ticks allowed after the first sighting.
        within: u64,
    },
    /// Every attacker indirect-fire unit is destroyed or out of its range of
    /// the defender's buildings. Killing only the spotter does not pass:
    /// artillery left in range fires again once anything spots for it.
    SilencesArtillery,
    /// Every attacker unit that was carried and set down is destroyed, or the
    /// carrier fell before setting anyone down.
    ClearsLanding,
}

/// How one scenario went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PressureOutcome {
    /// Scenario name.
    pub name: String,
    /// The check applied.
    pub check: Check,
    /// Whether the defender answered.
    pub passed: bool,
    /// The deciding fact, in words.
    pub detail: String,
    /// Tick the run stopped at.
    pub ticks: u64,
}

/// Loads every scenario in `dir`, in file-name order.
pub fn load_all(dir: &Path) -> Result<Vec<PressureScenario>> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<_>>()?;
    paths.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "json")
    });
    paths.sort();
    paths
        .iter()
        .map(|path| {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
        })
        .collect()
}

/// Runs one scenario, recording a replay when asked.
pub fn run(
    pressure: &PressureScenario,
    record: bool,
) -> Result<(PressureOutcome, Option<GameReplay>)> {
    let scenario = pressure.scenario.clone();
    let defender = PlayerId(pressure.defender);
    let attacker = PlayerId(pressure.attacker);
    let seat = scenario
        .players
        .get(usize::from(pressure.defender))
        .context("defender seat is not in the scenario")?;
    ensure!(
        seat.bot && seat.bot_config.is_some(),
        "defender seat is not a configured bot"
    );
    ensure!(
        scenario
            .players
            .get(usize::from(pressure.attacker))
            .is_some_and(|seat| !seat.bot),
        "attacker seat must be scripted, not a bot"
    );
    let mut state = scenario.build().context("building pressure scenario")?;
    let mut bots = seat_controllers(&scenario).context("building pressure controllers")?;
    let mut replay = record.then(|| GameReplay::new(oxide_sim::SIM_VERSION, scenario.clone()));
    let mut script = pressure.script.clone();
    script.sort_by_key(|scripted| scripted.tick);
    let mut script = script.into_iter().peekable();
    let mut watch = Watch::new(pressure.check);
    while state.current_tick() < pressure.deadline && state.result().is_none() {
        let tick = state.current_tick();
        let mut commands = Vec::new();
        while let Some(scripted) = script.next_if(|scripted| scripted.tick <= tick) {
            commands.push(PlayerCommand {
                player: attacker,
                command: scripted.command,
            });
        }
        commands.extend(oxide_kit::bot_execution::commands(&state, &mut bots));
        let report = oxide_kit::runner::record_and_tick(&mut state, commands, replay.as_mut());
        record_events(&mut bots, &report);
        watch.observe(&state, defender, attacker);
    }
    let (passed, detail) = watch.verdict(&state, defender, attacker)?;
    if let Some(replay) = replay.as_mut() {
        replay.meta.ticks = Some(state.current_tick());
    }
    let outcome = PressureOutcome {
        name: pressure.name.clone(),
        check: pressure.check,
        passed,
        detail,
        ticks: state.current_tick(),
    };
    Ok((outcome, replay))
}

/// Facts a check gathers while the scenario runs.
struct Watch {
    check: Check,
    first_air_sighting: Option<u64>,
    answered_air: Option<u64>,
    carriers: Vec<UnitId>,
    carried: Vec<UnitId>,
    landed: Vec<UnitId>,
}

impl Watch {
    fn new(check: Check) -> Self {
        Self {
            check,
            first_air_sighting: None,
            answered_air: None,
            carriers: Vec::new(),
            carried: Vec::new(),
            landed: Vec::new(),
        }
    }

    fn observe(&mut self, state: &State, defender: PlayerId, attacker: PlayerId) {
        let now = state.current_tick();
        match self.check {
            Check::KeepsFoundry | Check::SilencesArtillery => {}
            Check::AnswersAir { .. } => {
                if self.first_air_sighting.is_none() {
                    let seen = state.units().iter().any(|unit| {
                        unit.player == attacker
                            && unit.hp > 0
                            && unit.kind.stats().domain == Domain::Air
                            && state.can_see(defender, unit.tile())
                    });
                    if seen {
                        self.first_air_sighting = Some(now);
                    }
                }
                if self.answered_air.is_none() && owns_anti_air(state, defender) {
                    self.answered_air = Some(now);
                }
            }
            Check::ClearsLanding => {
                for carrier in state.units().iter().filter(|unit| unit.player == attacker) {
                    if !carrier.cargo.is_empty() && !self.carriers.contains(&carrier.id) {
                        self.carriers.push(carrier.id);
                    }
                    for passenger in &carrier.cargo {
                        if !self.carried.contains(&passenger.id) {
                            self.carried.push(passenger.id);
                        }
                    }
                }
                for id in &self.carried {
                    if !self.landed.contains(id) && state.unit(*id).is_some_and(|unit| unit.hp > 0)
                    {
                        self.landed.push(*id);
                    }
                }
            }
        }
    }

    fn verdict(
        &self,
        state: &State,
        defender: PlayerId,
        attacker: PlayerId,
    ) -> Result<(bool, String)> {
        let has_foundry = state.buildings().iter().any(|building| {
            building.player == defender && building.kind == BuildingKind::Foundry && building.hp > 0
        });
        Ok(match self.check {
            Check::KeepsFoundry => (
                has_foundry,
                if has_foundry {
                    "the Foundry stands".into()
                } else {
                    "the Foundry fell".into()
                },
            ),
            Check::AnswersAir { within } => {
                let Some(seen) = self.first_air_sighting else {
                    bail!("the attacker's aircraft were never seen; the scenario is broken");
                };
                match self.answered_air {
                    Some(answered) if answered <= seen + within => (
                        true,
                        format!(
                            "anti-air by tick {answered}, {} after first sighting",
                            answered.saturating_sub(seen)
                        ),
                    ),
                    Some(answered) => (
                        false,
                        format!(
                            "anti-air only at tick {answered}, {} after first sighting",
                            answered - seen
                        ),
                    ),
                    None => (
                        false,
                        format!("no anti-air since first sighting at tick {seen}"),
                    ),
                }
            }
            Check::SilencesArtillery => {
                let shelling = state
                    .units()
                    .iter()
                    .filter(|unit| unit.player == attacker && unit.hp > 0)
                    .filter(|unit| {
                        unit.kind.stats().weapons.iter().any(|weapon| {
                            weapon.indirect
                                && state.buildings().iter().any(|building| {
                                    building.player == defender
                                        && building.hp > 0
                                        && unit.pos.dist(building.closest_point_to(unit.pos))
                                            <= weapon.range
                                })
                        })
                    })
                    .count();
                (
                    has_foundry && shelling == 0,
                    if !has_foundry {
                        "the Foundry fell".into()
                    } else if shelling == 0 {
                        "no artillery in range of the base".into()
                    } else {
                        format!("{shelling} artillery still in range of the base")
                    },
                )
            }
            Check::ClearsLanding if self.landed.is_empty() => {
                let shot_down = !self.carriers.is_empty()
                    && self
                        .carriers
                        .iter()
                        .all(|id| state.unit(*id).is_none_or(|unit| unit.hp == 0));
                ensure!(
                    shot_down,
                    "no carried unit was ever set down; the scenario is broken"
                );
                (
                    has_foundry,
                    if has_foundry {
                        "the carrier fell before setting anyone down".into()
                    } else {
                        "the Foundry fell".into()
                    },
                )
            }
            Check::ClearsLanding => {
                let alive = self
                    .landed
                    .iter()
                    .filter(|id| state.unit(**id).is_some_and(|unit| unit.hp > 0))
                    .count();
                (
                    has_foundry && alive == 0,
                    if !has_foundry {
                        "the Foundry fell".into()
                    } else if alive == 0 {
                        format!("all {} landed units destroyed", self.landed.len())
                    } else {
                        format!("{alive} of {} landed units alive", self.landed.len())
                    },
                )
            }
        })
    }
}

/// Whether `player` owns anti-air: a unit whose role is to shoot aircraft,
/// or a completed Flak Turret.
pub(crate) fn owns_anti_air(state: &State, player: PlayerId) -> bool {
    let unit = state.units().iter().any(|unit| {
        unit.player == player
            && unit.hp > 0
            && matches!(
                unit.kind.role(),
                Role::AntiAir | Role::AirAir | Role::Interceptor
            )
    });
    unit || state.buildings().iter().any(|building| {
        building.player == player && building.kind == BuildingKind::FlakTurret && building.built
    })
}

/// A plain-text table of outcomes.
pub fn report(outcomes: &[PressureOutcome]) -> String {
    let mut text = String::from("scenario           check              result  ticks  detail\n");
    for outcome in outcomes {
        let check = match outcome.check {
            Check::KeepsFoundry => "keeps_foundry",
            Check::AnswersAir { .. } => "answers_air",
            Check::SilencesArtillery => "silences_artillery",
            Check::ClearsLanding => "clears_landing",
        };
        text.push_str(&format!(
            "{:<18} {:<18} {:<7} {:>5}  {}\n",
            outcome.name,
            check,
            if outcome.passed { "pass" } else { "FAIL" },
            outcome.ticks,
            outcome.detail
        ));
    }
    let passed = outcomes.iter().filter(|outcome| outcome.passed).count();
    text.push_str(&format!("{passed}/{} passed\n", outcomes.len()));
    text
}

#[cfg(test)]
mod tests {
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
}
