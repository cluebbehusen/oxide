//! Consequential controller failures detected from authoritative state.
//!
//! Detectors read omniscient state and events after each tick. Their incidents
//! are QA evidence for evaluation rows and reports; nothing here reaches a
//! controller.

use oxide_opponent::MissionStatus;
use oxide_sim::{Building, BuildingKind, PlayerId, State, UnitKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Ticks a failing condition must persist, and the window repeated stalls
/// must fall inside, before an incident is recorded.
pub const FAILURE_WINDOW_TICKS: u64 = 1_200;

/// Stalls with one reason on one unit, inside [`FAILURE_WINDOW_TICKS`], that
/// make a repeated impossible order.
pub const REPEATED_ORDER_STALLS: usize = 5;

/// Incidents retained per detector and seat for replay review; the count
/// keeps growing past it.
pub const MAX_FAILURE_EXAMPLES: usize = 8;

/// Stall reason the repeated-order detector ignores: a harvest line holding
/// out of danger re-reports it every 100 ticks by design.
pub const EXEMPT_STALL_REASON: &str = "danger_hold";

/// One detected failure episode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureIncident {
    /// Simulation tick at which the episode crossed its threshold.
    pub tick: u64,
    /// The unit, building or seat the episode concerns.
    pub subject: u32,
    /// Stall reason, building kind or cheapest legal unit.
    pub detail: String,
}

/// Episodes one detector found for one seat.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureTally {
    /// Every episode detected.
    pub incidents: u64,
    /// The first [`MAX_FAILURE_EXAMPLES`] episodes.
    pub examples: Vec<FailureIncident>,
}

impl FailureTally {
    fn record(&mut self, tick: u64, subject: u32, detail: &str) {
        self.incidents = self.incidents.saturating_add(1);
        if self.examples.len() < MAX_FAILURE_EXAMPLES {
            self.examples.push(FailureIncident {
                tick,
                subject,
                detail: detail.to_owned(),
            });
        }
    }
}

/// Failure episodes for one seat.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatFailures {
    /// A unit stalled with the same reason [`REPEATED_ORDER_STALLS`] times
    /// within [`FAILURE_WINDOW_TICKS`], [`EXEMPT_STALL_REASON`] aside. One
    /// episode lasts until that unit goes a full window without such a stall.
    pub repeated_orders: FailureTally,
    /// A paid, visible, unbuilt base-tier site made no construction progress
    /// for [`FAILURE_WINDOW_TICKS`]. Progress re-arms the site.
    pub abandoned_sites: FailureTally,
    /// Every built producer of the seat stayed idle for
    /// [`FAILURE_WINDOW_TICKS`] while the unprotected bank covered the
    /// cheapest unit any of them could legally train. The subject is the
    /// seat and the detail that unit; queueing anything re-arms the seat.
    pub starved_production: FailureTally,
    /// A mission stayed in one phase for [`FAILURE_WINDOW_TICKS`] past the
    /// timeout its controller gives that phase. The subject is the mission
    /// and the detail its kind and phase; each phase is one episode.
    #[serde(default)]
    pub stuck_missions: FailureTally,
}

/// Diagnostic, not an incident: ticks one producer sat idle while the
/// unprotected bank covered the cheapest unit it could legally train,
/// sampled at every detector check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProducerIdle {
    /// Producer building id.
    pub building: u32,
    /// Producer kind.
    pub kind: String,
    /// Idle, affordable ticks over the leg.
    pub idle_ticks: u64,
}

#[derive(Default)]
struct StallHistory {
    ticks: VecDeque<u64>,
    flagged: bool,
}

struct Watch {
    progress: u32,
    since: u64,
    seen: u64,
    flagged: bool,
}

#[derive(Default)]
struct Starvation {
    since: Option<u64>,
    flagged: bool,
}

#[derive(Default)]
struct SeatDetector {
    failures: SeatFailures,
    stalls: BTreeMap<(u32, String), StallHistory>,
    sites: BTreeMap<u32, Watch>,
    starvation: Starvation,
    idle: BTreeMap<u32, ProducerIdle>,
    /// Mission phases already reported, by mission id and phase start.
    stuck: BTreeSet<(u64, u64)>,
}

/// Detector memory for one evaluation leg. Seats without a controller are not
/// watched.
pub(super) struct FailureDetectors {
    seats: Vec<Option<SeatDetector>>,
    last_check: Option<u64>,
}

impl FailureDetectors {
    pub(super) fn new(watched: impl IntoIterator<Item = bool>) -> Self {
        Self {
            seats: watched
                .into_iter()
                .map(|watched| watched.then(SeatDetector::default))
                .collect(),
            last_check: None,
        }
    }

    /// Counts one `OrderStalled` event observed at `tick`.
    pub(super) fn record_stall(&mut self, seat: u8, unit: u32, reason: &str, tick: u64) {
        let Some(Some(seat)) = self.seats.get_mut(usize::from(seat)) else {
            return;
        };
        if reason == EXEMPT_STALL_REASON {
            return;
        }
        let history = seat.stalls.entry((unit, reason.to_owned())).or_default();
        history.ticks.push_back(tick);
        if history.ticks.len() > REPEATED_ORDER_STALLS {
            history.ticks.pop_front();
        }
        if !history.flagged
            && history.ticks.len() == REPEATED_ORDER_STALLS
            && tick - history.ticks[0] < FAILURE_WINDOW_TICKS
        {
            history.flagged = true;
            seat.failures.repeated_orders.record(tick, unit, reason);
        }
    }

    /// Checks sites and production at `now`. `protected` is the scrap each
    /// seat's controller reports holding back for a saving target.
    pub(super) fn check(&mut self, state: &State, now: u64, protected: &[u32]) {
        let elapsed = now - self.last_check.unwrap_or(now);
        self.last_check = Some(now);
        for (index, detector) in self.seats.iter_mut().enumerate() {
            let Some(detector) = detector else {
                continue;
            };
            detector.stalls.retain(|_, history| {
                history
                    .ticks
                    .back()
                    .is_some_and(|last| last + FAILURE_WINDOW_TICKS > now)
            });
            let player = PlayerId(index as u8);
            if !state.accepts_commands(player) {
                detector.sites.clear();
                detector.starvation = Starvation::default();
                continue;
            }
            let completed: Vec<BuildingKind> = state
                .buildings()
                .iter()
                .filter(|building| building.player == player && building.built)
                .map(|building| building.kind)
                .collect();
            let bank = state
                .player(player)
                .scrap
                .saturating_sub(protected.get(index).copied().unwrap_or(0));
            let mut producers = 0_usize;
            let mut all_idle = true;
            let mut cheapest: Option<UnitKind> = None;
            for building in state
                .buildings()
                .iter()
                .filter(|building| building.player == player && building.hp > 0)
            {
                if !building.built && !building.provisional && building.tier == 0 {
                    let watch = observe(&mut detector.sites, building, now, building.progress);
                    if !watch.flagged && now - watch.since >= FAILURE_WINDOW_TICKS {
                        watch.flagged = true;
                        detector.failures.abandoned_sites.record(
                            now,
                            building.id.0,
                            building.kind.name(),
                        );
                    }
                } else if building.built && !building.stats().produces.is_empty() {
                    producers += 1;
                    let idle = building.queue.is_empty();
                    all_idle &= idle;
                    let unit = cheapest_legal_unit(state, building, &completed);
                    if let Some(unit) = unit
                        && cheapest.is_none_or(|kind| unit.stats().cost < kind.stats().cost)
                    {
                        cheapest = Some(unit);
                    }
                    if idle && elapsed > 0 && unit.is_some_and(|unit| bank >= unit.stats().cost) {
                        let record =
                            detector
                                .idle
                                .entry(building.id.0)
                                .or_insert_with(|| ProducerIdle {
                                    building: building.id.0,
                                    kind: building.kind.name().to_owned(),
                                    idle_ticks: 0,
                                });
                        record.idle_ticks += elapsed;
                    }
                }
            }
            detector.sites.retain(|_, watch| watch.seen == now);
            let starving =
                producers > 0 && all_idle && cheapest.is_some_and(|unit| bank >= unit.stats().cost);
            let starvation = &mut detector.starvation;
            if !starving {
                *starvation = Starvation::default();
                continue;
            }
            let since = *starvation.since.get_or_insert(now);
            if !starvation.flagged && now - since >= FAILURE_WINDOW_TICKS {
                starvation.flagged = true;
                detector.failures.starved_production.record(
                    now,
                    index as u32,
                    cheapest.expect("starving implies a legal unit").name(),
                );
            }
        }
    }

    /// Checks the missions `seat`'s controller reports at `now`.
    pub(super) fn check_missions(&mut self, seat: u8, now: u64, missions: &[MissionStatus]) {
        let Some(Some(detector)) = self.seats.get_mut(usize::from(seat)) else {
            return;
        };
        detector.stuck.retain(|(id, since)| {
            missions
                .iter()
                .any(|mission| (mission.id, mission.since) == (*id, *since))
        });
        for mission in missions {
            let overdue = now >= mission.since + mission.timeout + FAILURE_WINDOW_TICKS;
            if overdue && detector.stuck.insert((mission.id, mission.since)) {
                detector.failures.stuck_missions.record(
                    now,
                    u32::try_from(mission.id).unwrap_or(u32::MAX),
                    &format!("{} {}", mission.kind.name(), mission.phase.name()),
                );
            }
        }
    }

    /// Detected episodes and idle-producer diagnostics by seat; unwatched
    /// seats report none.
    pub(super) fn finish(self) -> Vec<(SeatFailures, Vec<ProducerIdle>)> {
        self.seats
            .into_iter()
            .map(|seat| {
                seat.map(|seat| (seat.failures, seat.idle.into_values().collect()))
                    .unwrap_or_default()
            })
            .collect()
    }
}

/// Updates one watched building's state, restarting its clock when `value`
/// changes.
fn observe<'a>(
    watches: &'a mut BTreeMap<u32, Watch>,
    building: &Building,
    now: u64,
    value: u32,
) -> &'a mut Watch {
    let watch = watches.entry(building.id.0).or_insert(Watch {
        progress: value,
        since: now,
        seen: now,
        flagged: false,
    });
    if watch.progress != value {
        watch.progress = value;
        watch.since = now;
        watch.flagged = false;
    }
    watch.seen = now;
    watch
}

/// The cheapest unit `producer` may train now under the shared production
/// rules: its roster, the seat's faction and completed prerequisites. Equal
/// prices keep roster order.
fn cheapest_legal_unit(
    state: &State,
    producer: &Building,
    completed: &[BuildingKind],
) -> Option<UnitKind> {
    legal_units(state, producer, completed).min_by_key(|kind| kind.stats().cost)
}

fn legal_units<'a>(
    state: &State,
    producer: &'a Building,
    completed: &'a [BuildingKind],
) -> impl Iterator<Item = UnitKind> + 'a {
    let faction = state.player(producer.player).faction;
    producer
        .stats()
        .produces
        .iter()
        .copied()
        .filter(move |kind| kind.faction().is_none_or(|owner| owner == faction))
        .filter(|kind| {
            kind.stats()
                .requires
                .iter()
                .all(|required| completed.contains(required))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
    use oxide_sim::{Faction, Scenario};

    /// The Scuttler, the cheapest unit a Foundry trains.
    const CHEAPEST_FOUNDRY_UNIT: u32 = 40;

    fn scenario(buildings: Vec<BuildingSpec>, scrap: u32) -> Scenario {
        let ground = ".".repeat(24);
        let mut anchored: Vec<char> = ground.chars().collect();
        anchored[2] = '1';
        anchored[20] = '2';
        let mut map = vec![ground.clone(); 12];
        map[2] = anchored.into_iter().collect();
        Scenario {
            mode: Default::default(),
            name: "detectors".into(),
            seed: 3,
            map,
            players: [Faction::Ferrous, Faction::Cupric]
                .into_iter()
                .map(|faction| PlayerSpec {
                    name: format!("{faction:?}"),
                    faction,
                    team: None,
                    scrap,
                    bot: false,
                    bot_config: None,
                })
                .collect(),
            units: vec![UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 6,
                y: 8,
            }],
            buildings,
            meta: None,
        }
    }

    fn staged(scenario: &Scenario, edit: impl FnOnce(&mut serde_json::Value)) -> State {
        let mut value = serde_json::to_value(scenario.build().unwrap()).unwrap();
        edit(&mut value);
        serde_json::from_value(value).unwrap()
    }

    fn building_index(value: &serde_json::Value, kind: &str) -> usize {
        value["buildings"]
            .as_array()
            .unwrap()
            .iter()
            .position(|building| building["kind"] == kind)
            .unwrap()
    }

    fn site(progress: u32) -> impl FnOnce(&mut serde_json::Value) {
        move |value| {
            let index = building_index(value, "fabricator");
            value["buildings"][index]["built"] = false.into();
            value["buildings"][index]["hp"] = 100.into();
            value["buildings"][index]["progress"] = progress.into();
        }
    }

    fn fabricator() -> Vec<BuildingSpec> {
        vec![BuildingSpec {
            player: 0,
            kind: BuildingKind::Fabricator,
            x: 8,
            y: 6,
        }]
    }

    fn failures(detectors: FailureDetectors) -> Vec<SeatFailures> {
        detectors
            .finish()
            .into_iter()
            .map(|(failures, _)| failures)
            .collect()
    }

    #[test]
    fn repeated_stalls_fire_on_the_fifth_inside_the_window_only() {
        let mut detectors = FailureDetectors::new([true, false]);
        for tick in [100, 400, 700, 1_000] {
            detectors.record_stall(0, 7, "no_route", tick);
        }
        detectors.record_stall(0, 7, "ground_taken", 1_050);
        detectors.record_stall(0, 8, "no_route", 1_050);
        detectors.record_stall(1, 7, "no_route", 1_050);
        let mut spread = FailureDetectors::new([true]);
        for tick in [0, 300, 600, 900, 1_200] {
            spread.record_stall(0, 7, "no_route", tick);
        }
        assert_eq!(
            failures(spread)[0].repeated_orders.incidents,
            0,
            "five stalls spanning a full window are not repeated"
        );

        detectors.record_stall(0, 7, "no_route", 1_099);
        detectors.record_stall(0, 7, "no_route", 1_150);
        let failures = failures(detectors);
        assert_eq!(failures[0].repeated_orders.incidents, 1);
        assert_eq!(
            failures[0].repeated_orders.examples,
            [FailureIncident {
                tick: 1_099,
                subject: 7,
                detail: "no_route".into(),
            }]
        );
        assert_eq!(
            failures[1],
            SeatFailures::default(),
            "seat one is unwatched"
        );
    }

    #[test]
    fn danger_holds_are_not_repeated_orders() {
        assert_eq!(
            super::super::wire_name(&oxide_sim::StallReason::DangerHold),
            EXEMPT_STALL_REASON
        );
        let mut detectors = FailureDetectors::new([true]);
        for tick in 0..10 {
            detectors.record_stall(0, 7, EXEMPT_STALL_REASON, tick * 10);
        }
        assert_eq!(failures(detectors)[0].repeated_orders.incidents, 0);
    }

    #[test]
    fn a_repeated_order_rearms_after_a_quiet_window() {
        let state = scenario(Vec::new(), 0).build().unwrap();
        let mut detectors = FailureDetectors::new([true, true]);
        for tick in 0..5 {
            detectors.record_stall(0, 7, "no_route", tick * 10);
        }
        detectors.check(&state, 1_200, &[0, 0]);
        detectors.record_stall(0, 7, "no_route", 1_210);
        detectors.check(&state, 1_240, &[0, 0]);
        for tick in 0..4 {
            detectors.record_stall(0, 7, "no_route", 1_250 + tick);
        }
        assert_eq!(failures(detectors)[0].repeated_orders.incidents, 1);

        let mut detectors = FailureDetectors::new([true, true]);
        for tick in 0..5 {
            detectors.record_stall(0, 7, "no_route", tick * 10);
        }
        detectors.check(&state, 1_240, &[0, 0]);
        for tick in 0..5 {
            detectors.record_stall(0, 7, "no_route", 1_250 + tick);
        }
        assert_eq!(failures(detectors)[0].repeated_orders.incidents, 2);
    }

    #[test]
    fn an_unprogressing_site_is_abandoned_at_the_window_and_progress_rearms_it() {
        let scenario = scenario(fabricator(), 0);
        let stalled = staged(&scenario, site(40));
        let mut detectors = FailureDetectors::new([true, true]);
        for now in (600..600 + FAILURE_WINDOW_TICKS).step_by(12) {
            detectors.check(&stalled, now, &[0, 0]);
        }
        detectors.check(&stalled, 600 + FAILURE_WINDOW_TICKS, &[0, 0]);
        detectors.check(&stalled, 612 + FAILURE_WINDOW_TICKS, &[0, 0]);
        let advanced = staged(&scenario, site(41));
        detectors.check(&advanced, 2_000, &[0, 0]);
        detectors.check(&advanced, 3_199, &[0, 0]);
        let before_rearm = detectors.seats[0].as_ref().unwrap().failures.clone();
        detectors.check(&advanced, 3_200, &[0, 0]);
        let failures = failures(detectors);

        assert_eq!(before_rearm.abandoned_sites.incidents, 1);
        assert_eq!(before_rearm.abandoned_sites.examples[0].tick, 1_800);
        assert_eq!(
            before_rearm.abandoned_sites.examples[0].detail,
            "fabricator"
        );
        assert_eq!(failures[0].abandoned_sites.incidents, 2);
        assert_eq!(failures[0].starved_production.incidents, 0);
    }

    #[test]
    fn provisional_and_upgrading_works_are_not_abandoned_sites() {
        let provisional = staged(&scenario(fabricator(), 0), |value| {
            site(0)(value);
            let fabricator = building_index(value, "fabricator");
            value["buildings"][fabricator]["provisional"] = true.into();
            value["units"][0]["order"] = serde_json::json!({
                "order": "found",
                "kind": "fabricator",
                "anchor": value["buildings"][fabricator]["anchor"].clone(),
            });
        });
        assert!(provisional.buildings().iter().any(|site| site.provisional));
        let upgrading = staged(
            &scenario(
                vec![BuildingSpec {
                    player: 0,
                    kind: BuildingKind::Turret,
                    x: 12,
                    y: 9,
                }],
                0,
            ),
            |value| {
                let turret = building_index(value, "turret");
                value["buildings"][turret]["built"] = false.into();
                value["buildings"][turret]["tier"] = 1.into();
            },
        );
        for state in [&provisional, &upgrading] {
            let mut detectors = FailureDetectors::new([true, true]);
            detectors.check(state, 0, &[0, 0]);
            detectors.check(state, 5_000, &[0, 0]);
            assert_eq!(failures(detectors)[0].abandoned_sites.incidents, 0);
        }
    }

    #[test]
    fn an_idle_affordable_seat_starves_at_the_window_and_protected_scrap_counts() {
        let state = scenario(Vec::new(), CHEAPEST_FOUNDRY_UNIT).build().unwrap();
        let mut detectors = FailureDetectors::new([true, true]);
        detectors.check(&state, 0, &[0, 0]);
        detectors.check(&state, FAILURE_WINDOW_TICKS - 12, &[0, 0]);
        assert_eq!(
            detectors.seats[0]
                .as_ref()
                .unwrap()
                .failures
                .starved_production
                .incidents,
            0
        );
        detectors.check(&state, FAILURE_WINDOW_TICKS, &[0, 0]);
        detectors.check(&state, 5 * FAILURE_WINDOW_TICKS, &[0, 0]);
        let finished = detectors.finish();
        let (failures, idle) = &finished[0];
        assert_eq!(failures.starved_production.incidents, 1);
        assert_eq!(
            failures.starved_production.examples,
            [FailureIncident {
                tick: FAILURE_WINDOW_TICKS,
                subject: 0,
                detail: "scuttler".into(),
            }]
        );
        assert_eq!(finished[1].0.starved_production.incidents, 1);
        let [foundry] = idle.as_slice() else {
            panic!("one idle producer: {idle:?}");
        };
        assert_eq!(foundry.kind, "foundry");
        assert_eq!(foundry.idle_ticks, 5 * FAILURE_WINDOW_TICKS);

        let mut protected = FailureDetectors::new([true, true]);
        protected.check(&state, 0, &[1, 0]);
        protected.check(&state, 2 * FAILURE_WINDOW_TICKS, &[1, 0]);
        let finished = protected.finish();
        assert_eq!(finished[0].0.starved_production.incidents, 0);
        assert!(
            finished[0].1.is_empty(),
            "protected scrap is not idle money"
        );
        assert_eq!(finished[1].0.starved_production.incidents, 1);
    }

    #[test]
    fn one_busy_producer_keeps_the_seat_from_starving() {
        let state = staged(&scenario(fabricator(), 1_000), |value| {
            let index = building_index(value, "fabricator");
            value["buildings"][index]["queue"] = serde_json::json!(["lancer"]);
        });
        let mut detectors = FailureDetectors::new([true, false]);
        detectors.check(&state, 0, &[0, 0]);
        detectors.check(&state, 3 * FAILURE_WINDOW_TICKS, &[0, 0]);
        let finished = detectors.finish();
        let (failures, idle) = &finished[0];
        assert_eq!(failures.starved_production.incidents, 0);
        let [foundry] = idle.as_slice() else {
            panic!("only the idle Foundry is recorded: {idle:?}");
        };
        assert_eq!(foundry.kind, "foundry");
        assert_eq!(foundry.idle_ticks, 3 * FAILURE_WINDOW_TICKS);
    }

    #[test]
    fn a_busy_or_unaffordable_seat_is_not_starved() {
        let poor = scenario(Vec::new(), CHEAPEST_FOUNDRY_UNIT - 1)
            .build()
            .unwrap();
        let busy = staged(&scenario(Vec::new(), 1_000), |value| {
            let index = building_index(value, "foundry");
            value["buildings"][index]["queue"] = serde_json::json!(["harvester"]);
        });
        for state in [&poor, &busy] {
            let mut detectors = FailureDetectors::new([true, false]);
            detectors.check(state, 0, &[0, 0]);
            detectors.check(state, 3 * FAILURE_WINDOW_TICKS, &[0, 0]);
            let finished = detectors.finish();
            assert_eq!(finished[0].0.starved_production.incidents, 0);
            assert!(finished[0].1.is_empty());
        }
    }

    #[test]
    fn legal_units_follow_faction_and_prerequisites() {
        let mut buildings = fabricator();
        buildings.push(BuildingSpec {
            player: 1,
            kind: BuildingKind::Fabricator,
            x: 14,
            y: 6,
        });
        let state = scenario(buildings, 0).build().unwrap();
        let producer = |player: u8, kind: BuildingKind| {
            state
                .buildings()
                .iter()
                .find(|building| building.player == PlayerId(player) && building.kind == kind)
                .unwrap()
        };
        let fabricator = BuildingKind::Fabricator;
        let ferrous = producer(0, fabricator);
        let cupric = producer(1, fabricator);
        assert_eq!(
            cheapest_legal_unit(&state, ferrous, &[]),
            Some(UnitKind::Flakhound)
        );
        assert_eq!(
            cheapest_legal_unit(&state, cupric, &[]),
            Some(UnitKind::Stinger)
        );
        assert!(legal_units(&state, ferrous, &[]).all(|kind| kind != UnitKind::Stinger));
        let foundry = producer(0, BuildingKind::Foundry);
        assert!(!legal_units(&state, foundry, &[]).any(|kind| kind == UnitKind::Excavator));
        assert!(
            legal_units(&state, foundry, &[fabricator]).any(|kind| kind == UnitKind::Excavator)
        );
        assert_eq!(
            cheapest_legal_unit(&state, foundry, &[fabricator]).map(|kind| kind.stats().cost),
            Some(CHEAPEST_FOUNDRY_UNIT)
        );
    }

    #[test]
    fn a_mission_stuck_past_its_timeout_counts_once_per_phase() {
        use oxide_opponent::{MissionKind, Phase};
        let status = |since: u64| MissionStatus {
            id: 3,
            kind: MissionKind::Defend {
                asset: oxide_sim::BuildingId(1),
            },
            phase: Phase::Engage,
            since,
            timeout: 600,
            units: 2,
            goal: chassis::grid::TilePos::new(4, 4),
        };
        let mut detectors = FailureDetectors::new([true, false]);
        let overdue = 600 + FAILURE_WINDOW_TICKS;
        detectors.check_missions(0, overdue - 1, &[status(0)]);
        detectors.check_missions(1, overdue, &[status(0)]);
        detectors.check_missions(0, overdue, &[status(0)]);
        detectors.check_missions(0, overdue + 12, &[status(0)]);
        detectors.check_missions(0, overdue + 12, &[status(overdue)]);
        detectors.check_missions(0, 2 * overdue - 1, &[status(overdue)]);
        detectors.check_missions(0, 2 * overdue, &[status(overdue)]);
        let seats = detectors.finish();
        let stuck = &seats[0].0.stuck_missions;
        assert_eq!(stuck.incidents, 2);
        assert_eq!(
            stuck.examples[0],
            FailureIncident {
                tick: overdue,
                subject: 3,
                detail: "defend engage".into(),
            }
        );
        assert_eq!(seats[1].0, SeatFailures::default(), "an unwatched seat");
    }

    #[test]
    fn failures_recorded_before_the_mission_detector_still_load() {
        let mut value = serde_json::to_value(SeatFailures::default()).unwrap();
        value.as_object_mut().unwrap().remove("stuck_missions");
        let loaded: SeatFailures = serde_json::from_value(value).unwrap();
        assert_eq!(loaded, SeatFailures::default());
    }
}
