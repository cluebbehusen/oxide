//! Consequential controller failures detected from authoritative state.
//!
//! Detectors read omniscient state and events after each tick. Their incidents
//! are QA evidence for evaluation rows and reports; nothing here reaches a
//! controller.

use oxide_sim::{Building, BuildingKind, PlayerId, State, UnitKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// Ticks a failing condition must persist, and the window repeated stalls
/// must fall inside, before an incident is recorded.
pub const FAILURE_WINDOW_TICKS: u64 = 1_200;

/// Stalls with one reason on one unit, inside [`FAILURE_WINDOW_TICKS`], that
/// make a repeated impossible order.
pub const REPEATED_ORDER_STALLS: usize = 5;

/// Incidents retained per detector and seat for replay review; the count
/// keeps growing past it.
pub const MAX_FAILURE_EXAMPLES: usize = 8;

/// One detected failure episode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureIncident {
    /// Simulation tick at which the episode crossed its threshold.
    pub tick: u64,
    /// The unit or building id the episode concerns.
    pub subject: u32,
    /// Stall reason or building kind.
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
    /// within [`FAILURE_WINDOW_TICKS`]. One episode lasts until that unit
    /// goes a full window without such a stall.
    pub repeated_orders: FailureTally,
    /// A paid, visible, unbuilt base-tier site made no construction progress
    /// for [`FAILURE_WINDOW_TICKS`]. Progress re-arms the site.
    pub abandoned_sites: FailureTally,
    /// A built producer stayed idle for [`FAILURE_WINDOW_TICKS`] while the
    /// unprotected bank covered the cheapest unit it could legally train.
    pub starved_producers: FailureTally,
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
struct SeatDetector {
    failures: SeatFailures,
    stalls: BTreeMap<(u32, String), StallHistory>,
    sites: BTreeMap<u32, Watch>,
    producers: BTreeMap<u32, Watch>,
}

/// Detector memory for one evaluation leg. Seats without a controller are not
/// watched.
pub(super) struct FailureDetectors {
    seats: Vec<Option<SeatDetector>>,
}

impl FailureDetectors {
    pub(super) fn new(watched: impl IntoIterator<Item = bool>) -> Self {
        Self {
            seats: watched
                .into_iter()
                .map(|watched| watched.then(SeatDetector::default))
                .collect(),
        }
    }

    /// Counts one `OrderStalled` event observed at `tick`.
    pub(super) fn record_stall(&mut self, seat: u8, unit: u32, reason: &str, tick: u64) {
        let Some(Some(seat)) = self.seats.get_mut(usize::from(seat)) else {
            return;
        };
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

    /// Checks sites and producers at `now`. `protected` is the scrap each
    /// seat's controller reports holding back for a saving target.
    pub(super) fn check(&mut self, state: &State, now: u64, protected: &[u32]) {
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
                detector.producers.clear();
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
                    let starving = building.queue.is_empty()
                        && cheapest_legal_unit(state, building, &completed)
                            .is_some_and(|cost| bank >= cost);
                    // Any change of the starving condition restarts the clock.
                    let watch =
                        observe(&mut detector.producers, building, now, u32::from(starving));
                    if starving && !watch.flagged && now - watch.since >= FAILURE_WINDOW_TICKS {
                        watch.flagged = true;
                        detector.failures.starved_producers.record(
                            now,
                            building.id.0,
                            building.kind.name(),
                        );
                    }
                }
            }
            detector.sites.retain(|_, watch| watch.seen == now);
            detector.producers.retain(|_, watch| watch.seen == now);
        }
    }

    /// Detected episodes by seat; unwatched seats report none.
    pub(super) fn finish(self) -> Vec<SeatFailures> {
        self.seats
            .into_iter()
            .map(|seat| seat.map(|seat| seat.failures).unwrap_or_default())
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

/// Price of the cheapest unit `producer` may train now under the shared
/// production rules: its roster, the seat's faction and completed
/// prerequisites.
fn cheapest_legal_unit(
    state: &State,
    producer: &Building,
    completed: &[BuildingKind],
) -> Option<u32> {
    legal_units(state, producer, completed)
        .map(|kind| kind.stats().cost)
        .min()
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
            spread.finish()[0].repeated_orders.incidents,
            0,
            "five stalls spanning a full window are not repeated"
        );

        detectors.record_stall(0, 7, "no_route", 1_099);
        detectors.record_stall(0, 7, "no_route", 1_150);
        let failures = detectors.finish();
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
        assert_eq!(detectors.finish()[0].repeated_orders.incidents, 1);

        let mut detectors = FailureDetectors::new([true, true]);
        for tick in 0..5 {
            detectors.record_stall(0, 7, "no_route", tick * 10);
        }
        detectors.check(&state, 1_240, &[0, 0]);
        for tick in 0..5 {
            detectors.record_stall(0, 7, "no_route", 1_250 + tick);
        }
        assert_eq!(detectors.finish()[0].repeated_orders.incidents, 2);
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
        let failures = detectors.finish();

        assert_eq!(before_rearm.abandoned_sites.incidents, 1);
        assert_eq!(before_rearm.abandoned_sites.examples[0].tick, 1_800);
        assert_eq!(
            before_rearm.abandoned_sites.examples[0].detail,
            "fabricator"
        );
        assert_eq!(failures[0].abandoned_sites.incidents, 2);
        assert_eq!(failures[0].starved_producers.incidents, 0);
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
            assert_eq!(detectors.finish()[0].abandoned_sites.incidents, 0);
        }
    }

    #[test]
    fn an_idle_affordable_producer_starves_at_the_window_and_protected_scrap_counts() {
        let state = scenario(Vec::new(), CHEAPEST_FOUNDRY_UNIT).build().unwrap();
        let mut detectors = FailureDetectors::new([true, true]);
        detectors.check(&state, 0, &[0, 0]);
        detectors.check(&state, FAILURE_WINDOW_TICKS - 12, &[0, 0]);
        assert_eq!(
            detectors.seats[0]
                .as_ref()
                .unwrap()
                .failures
                .starved_producers
                .incidents,
            0
        );
        detectors.check(&state, FAILURE_WINDOW_TICKS, &[0, 0]);
        detectors.check(&state, 5 * FAILURE_WINDOW_TICKS, &[0, 0]);
        let failures = detectors.finish();
        assert_eq!(failures[0].starved_producers.incidents, 1);
        assert_eq!(failures[0].starved_producers.examples[0].detail, "foundry");
        assert_eq!(failures[1].starved_producers.incidents, 1);

        let mut protected = FailureDetectors::new([true, true]);
        protected.check(&state, 0, &[1, 0]);
        protected.check(&state, 2 * FAILURE_WINDOW_TICKS, &[1, 0]);
        let failures = protected.finish();
        assert_eq!(failures[0].starved_producers.incidents, 0);
        assert_eq!(failures[1].starved_producers.incidents, 1);
    }

    #[test]
    fn a_busy_or_unaffordable_producer_is_not_starved() {
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
            assert_eq!(detectors.finish()[0].starved_producers.incidents, 0);
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
        assert_eq!(cheapest_legal_unit(&state, ferrous, &[]), Some(90));
        assert_eq!(cheapest_legal_unit(&state, cupric, &[]), Some(45));
        assert!(legal_units(&state, ferrous, &[]).all(|kind| kind != UnitKind::Stinger));
        let foundry = producer(0, BuildingKind::Foundry);
        assert!(!legal_units(&state, foundry, &[]).any(|kind| kind == UnitKind::Excavator));
        assert!(
            legal_units(&state, foundry, &[fabricator]).any(|kind| kind == UnitKind::Excavator)
        );
        assert_eq!(
            cheapest_legal_unit(&state, foundry, &[fabricator]),
            Some(CHEAPEST_FOUNDRY_UNIT)
        );
    }
}
