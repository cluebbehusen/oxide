use super::*;
use crate::experience::{
    Doctrine, EpisodeId, EpisodeOwner, EpisodeReport, ExperienceKey, Outcome, OutcomeJournal,
    OutcomeReason,
};
#[cfg(test)]
use crate::observation::ObservationData;
use chassis::Tick;
use oxide_sim::Command;
use oxide_sim::ids::Target;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct FailedWork {
    tile: TilePos,
    failed_at: Tick,
    visible: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emitted_command_order_controls_bounce_attribution_in_both_orientations() {
        for home in [TilePos::new(0, 0), TilePos::new(29, 19)] {
            for (harvest_first, queued_move) in [(true, false), (false, false), (true, true)] {
                let mut obs = observation();
                let node = obs.known_scrap[0].0;
                let orientation = crate::Orientation::for_home(&obs, home);
                let harvest = Command::Harvest {
                    units: vec![UnitId(1)],
                    node: orientation.tile(node),
                    queue: false,
                };
                let movement = Command::Move {
                    units: vec![UnitId(1)],
                    goal: orientation.tile(TilePos::new(7, 3)),
                    queue: queued_move,
                };
                let commands = if harvest_first {
                    [harvest, movement]
                } else {
                    [movement, harvest]
                }
                .map(|command| oxide_sim::PlayerCommand {
                    player: obs.me,
                    command,
                });
                let mut policy = UtilityPolicy::new();
                policy.observe_work_experience(&obs);
                policy.record_dispatched_work(&obs, orientation, &commands);
                obs.tick += 24;
                obs.my_units[0].tile.x += 1;
                policy.observe_work_experience(&obs);
                let work = &policy.state.work_experience;
                let expected_bounce = !harvest_first || queued_move;
                assert_eq!(work.dead_nodes.contains(&node), expected_bounce);
                assert_eq!(work.pending.len(), 1);
                assert_eq!(
                    work.pending[0].reason,
                    if expected_bounce {
                        OutcomeReason::BlockedRoute
                    } else {
                        OutcomeReason::Preempted
                    }
                );
                let observed = work.clone();
                policy.observe_work_experience(&obs);
                assert_eq!(policy.state.work_experience, observed);
            }
        }
    }

    #[test]
    fn emitted_build_tracks_its_oriented_footprint_and_retask_releases_it() {
        let mut obs = observation();
        let anchor = TilePos::new(9, 4);
        let kind = BuildingKind::Fabricator;
        let orientation = crate::Orientation::for_home(&obs, TilePos::new(29, 19));
        let build = oxide_sim::PlayerCommand {
            player: obs.me,
            command: Command::Build {
                units: vec![UnitId(1)],
                kind,
                anchor: orientation.anchor(anchor, kind.base_stats().size),
                queue: false,
                defer: true,
            },
        };
        let mut policy = UtilityPolicy::new();
        policy.observe_work_experience(&obs);
        policy.record_dispatched_work(&obs, orientation, &[build]);
        assert_eq!(policy.state.work_experience.builds[0].anchor, anchor);
        obs.my_units[0].founding = Some((kind, anchor));
        obs.my_units[0].idle = false;
        obs.tick += 24;
        policy.observe_work_experience(&obs);
        assert_eq!(policy.state.work_experience.builds.len(), 1);
        policy.record_dispatched_work(
            &obs,
            orientation,
            &[oxide_sim::PlayerCommand {
                player: obs.me,
                command: Command::Stop {
                    units: vec![UnitId(1)],
                },
            }],
        );
        obs.my_units[0].founding = None;
        obs.my_units[0].idle = true;
        obs.tick += 24;
        policy.observe_work_experience(&obs);
        assert!(policy.state.work_experience.builds.is_empty());
        assert!(policy.state.work_experience.dead_anchors.is_empty());
        assert_eq!(policy.state.work_experience.pending.len(), 1);
        assert_eq!(
            policy.state.work_experience.pending[0].reason,
            OutcomeReason::Preempted
        );
    }

    fn observation() -> Observation {
        Observation::from_data(ObservationData {
            tick: 100,
            map_width: 30,
            map_height: 20,
            visible: vec![true; 600],
            explored: vec![true; 600],
            scrap: 1000,
            known_scrap: vec![(TilePos::new(10, 10), 1000)],
            my_units: vec![crate::test_support::unit(
                1,
                oxide_sim::ids::PlayerId(0),
                UnitKind::Harvester,
                TilePos::new(3, 3),
            )],
            ..crate::test_support::observation_data()
        })
    }

    #[test]
    fn a_walking_founder_defers_the_site_audits_verdict() {
        let site = TilePos::new(10, 10);
        for walking in [false, true] {
            let mut obs = observation();
            let mut policy = UtilityPolicy::new();
            policy.observe_work_experience(&obs);
            policy.state.work_experience.record_exact_build_attempt(
                &obs,
                &[UnitId(1)],
                BuildingKind::Fabricator,
                site,
            );
            if walking {
                obs.my_units[0].founding = Some((BuildingKind::Fabricator, site));
                obs.my_units[0].idle = false;
            }
            obs.tick += 24;
            policy.observe_work_experience(&obs);
            assert_eq!(
                policy.state.work_experience.builds.len(),
                usize::from(walking)
            );
            assert_eq!(
                policy.state.work_experience.dead_anchors.contains(&site),
                !walking
            );
            assert_eq!(
                policy.state.work_experience.pending.len(),
                usize::from(!walking)
            );
        }
    }

    #[test]
    fn the_founder_shields_only_its_own_anchor() {
        let mut obs = observation();
        let claimed = TilePos::new(9, 4);
        let refused = TilePos::new(15, 8);
        obs.my_units.push(crate::test_support::unit(
            2,
            PlayerId(0),
            UnitKind::Harvester,
            TilePos::new(4, 3),
        ));
        let mut policy = UtilityPolicy::new();
        policy.observe_work_experience(&obs);
        policy.state.work_experience.record_exact_build_attempt(
            &obs,
            &[UnitId(1)],
            BuildingKind::Fabricator,
            claimed,
        );
        policy.state.work_experience.record_exact_build_attempt(
            &obs,
            &[UnitId(2)],
            BuildingKind::Fabricator,
            refused,
        );
        obs.my_units[0].founding = Some((BuildingKind::Fabricator, claimed));
        obs.my_units[0].idle = false;
        obs.tick += 24;
        policy.observe_work_experience(&obs);
        assert_eq!(policy.state.work_experience.builds.len(), 1);
        assert_eq!(policy.state.work_experience.builds[0].anchor, claimed);
        assert_eq!(policy.state.work_experience.dead_anchors, [refused]);
        assert_eq!(policy.state.work_experience.pending.len(), 1);
        assert_eq!(
            policy.state.work_experience.pending[0].reason,
            OutcomeReason::BlockedRoute
        );
    }

    #[test]
    fn harvest_bounce_has_one_exact_episode_and_bounded_retry() {
        let mut obs = observation();
        let node = TilePos::new(10, 10);
        let mut policy = UtilityPolicy::default();
        policy.observe_work_experience(&obs);
        policy
            .state
            .work_experience
            .record_dispatched_harvest(&obs, UnitId(1), node);
        obs.tick = 124;
        policy.observe_work_experience(&obs);
        assert!(policy.state.work_experience.dead_nodes.contains(&node));
        assert_eq!(policy.state.work_experience.pending.len(), 1);
        assert_eq!(policy.state.work_experience.pending[0].started_at, 100);
        assert_eq!(
            policy.state.work_experience.pending[0].reason,
            OutcomeReason::BlockedRoute
        );
        policy.observe_work_experience(&obs);
        assert_eq!(policy.state.work_experience.pending.len(), 1);
        obs.tick = 424;
        policy.observe_work_experience(&obs);
        assert!(!policy.state.work_experience.dead_nodes.contains(&node));
        assert!(
            policy.state.work_experience.last_sent.is_empty(),
            "retry eligibility must not dispatch work"
        );
    }

    #[test]
    fn preemption_death_exhaustion_and_unfunded_builds_do_not_blacklist_routes() {
        let node = TilePos::new(10, 10);
        for cause in 0..4 {
            let mut obs = observation();
            let mut policy = UtilityPolicy::default();
            policy.observe_work_experience(&obs);
            policy
                .state
                .work_experience
                .record_dispatched_harvest(&obs, UnitId(1), node);
            policy.state.work_experience.record_exact_build_attempt(
                &obs,
                &[UnitId(1)],
                BuildingKind::Fabricator,
                node,
            );
            match cause {
                0 => {
                    policy
                        .state
                        .work_experience
                        .record_work_retask(&obs, &[UnitId(1)], None);
                }
                1 => obs.my_units.clear(),
                2 => {
                    obs.known_scrap.clear();
                    obs.my_units[0].idle = false;
                }
                _ => {
                    obs.scrap = 0;
                    obs.my_units[0].idle = false;
                }
            }
            obs.tick = 124;
            policy.observe_work_experience(&obs);
            assert!(
                policy.state.work_experience.dead_nodes.is_empty(),
                "cause {cause}"
            );
            assert!(
                policy.state.work_experience.dead_anchors.is_empty(),
                "cause {cause}"
            );
            assert!(
                policy
                    .state
                    .work_experience
                    .pending
                    .iter()
                    .all(|report| report.reason != OutcomeReason::BlockedRoute)
            );
        }
    }

    #[test]
    fn occupied_build_sites_are_invalidation_not_route_failure() {
        for owner in 0..3 {
            for built in [false, true] {
                let mut obs = observation();
                let mut policy = UtilityPolicy::default();
                let site = TilePos::new(10, 10);
                policy.observe_work_experience(&obs);
                policy.state.work_experience.record_exact_build_attempt(
                    &obs,
                    &[UnitId(1)],
                    BuildingKind::Fabricator,
                    site,
                );
                let occupant = BuildingObs {
                    hp: 10,
                    built,
                    ..crate::test_support::building(
                        8,
                        PlayerId(owner),
                        BuildingKind::RepairBay,
                        site.offset(1, 0),
                    )
                };
                match owner {
                    0 => obs.my_buildings.push(occupant),
                    1 => obs.ally_buildings.push(occupant),
                    _ => obs.enemy_buildings.push(occupant),
                }
                obs.tick = 124;
                policy.observe_work_experience(&obs);
                assert!(
                    policy.state.work_experience.dead_anchors.is_empty(),
                    "owner {owner}, built {built}"
                );
                assert!(policy.state.work_experience.builds.is_empty());
                assert_eq!(policy.state.work_experience.pending.len(), 1);
                let report = &policy.state.work_experience.pending[0];
                assert_eq!(report.outcome, Outcome::Invalidated);
                assert_eq!(report.reason, OutcomeReason::SiteOccupied);
                assert_eq!(report.own_lost_value, 0);
                assert!(!report.doctrine_eligible);
                policy.observe_work_experience(&obs);
                assert_eq!(policy.state.work_experience.pending.len(), 1);
            }
        }
    }

    #[test]
    fn remembered_or_nonoverlapping_buildings_do_not_prove_site_occupation() {
        for (seen, offset, hp) in [(false, 1, 10), (true, 8, 10), (true, 1, 0)] {
            let mut obs = observation();
            let mut policy = UtilityPolicy::default();
            let site = TilePos::new(10, 10);
            policy.observe_work_experience(&obs);
            policy.state.work_experience.record_exact_build_attempt(
                &obs,
                &[UnitId(1)],
                BuildingKind::Fabricator,
                site,
            );
            obs.enemy_buildings.push(BuildingObs {
                hp,
                seen,
                ..crate::test_support::building(
                    8,
                    PlayerId(2),
                    BuildingKind::RepairBay,
                    site.offset(offset, 0),
                )
            });
            obs.tick = 124;
            policy.observe_work_experience(&obs);
            assert_eq!(policy.state.work_experience.pending.len(), 1);
            assert_eq!(
                policy.state.work_experience.pending[0].reason,
                OutcomeReason::BlockedRoute
            );
        }
    }

    #[test]
    fn a_dispatched_foundation_is_watched_after_its_worker_is_released() {
        let mut obs = observation();
        let mut policy = UtilityPolicy::default();
        let site = TilePos::new(10, 10);
        policy.observe_work_experience(&obs);
        policy.state.work_experience.record_exact_build_attempt(
            &obs,
            &[UnitId(1)],
            BuildingKind::Fabricator,
            site,
        );
        obs.tick = 124;
        {
            let obs = &mut *obs;
            obs.my_buildings.push(BuildingObs {
                hp: 10,
                built: false,
                ..crate::test_support::building(8, obs.me, BuildingKind::Fabricator, site)
            });
        }
        policy.observe_work_experience(&obs);
        assert!(policy.state.work_experience.builds.is_empty());
        assert_eq!(policy.state.work_experience.foundations.len(), 1);
        assert!(policy.state.work_experience.pending.is_empty());
        let mut cancelled = policy.clone();
        cancelled
            .state
            .work_experience
            .record_foundation_cancellation(&obs, BuildingId(8));
        assert!(cancelled.state.work_experience.foundations.is_empty());
        let cancellation = &cancelled.state.work_experience.pending[0];
        assert_eq!(cancellation.outcome, Outcome::Invalidated);
        assert_eq!(cancellation.reason, OutcomeReason::Preempted);
        assert_eq!(cancellation.own_lost_value, 0);
        cancelled
            .state
            .work_experience
            .record_foundation_cancellation(&obs, BuildingId(8));
        assert_eq!(cancelled.state.work_experience.pending.len(), 1);
        policy
            .state
            .work_experience
            .record_work_retask(&obs, &[UnitId(1)], None);
        obs.my_units.clear();
        obs.tick = 148;
        obs.my_buildings[0].built = true;
        policy.observe_work_experience(&obs);
        assert!(policy.state.work_experience.dead_anchors.is_empty());
        assert!(policy.state.work_experience.foundations.is_empty());
        assert_eq!(policy.state.work_experience.pending.len(), 1);
        assert_eq!(
            policy.state.work_experience.pending[0].outcome,
            Outcome::Complete
        );
        assert_eq!(policy.state.work_experience.pending[0].own_lost_value, 0);
    }

    #[test]
    fn failed_build_retries_after_delay_but_a_walking_founder_keeps_its_claim() {
        let mut obs = observation();
        let site = TilePos::new(10, 10);
        let mut policy = UtilityPolicy::default();
        policy.observe_work_experience(&obs);
        policy.state.work_experience.record_exact_build_attempt(
            &obs,
            &[UnitId(1)],
            BuildingKind::Fabricator,
            site,
        );
        obs.my_units[0].founding = Some((BuildingKind::Fabricator, site));
        obs.my_units[0].idle = false;
        obs.tick = 400;
        policy.observe_work_experience(&obs);
        assert!(policy.state.work_experience.dead_anchors.is_empty());
        assert_eq!(policy.state.work_experience.builds.len(), 1);
        obs.my_units[0].founding = None;
        obs.my_units[0].idle = true;
        obs.tick = 424;
        policy.observe_work_experience(&obs);
        assert!(policy.state.work_experience.dead_anchors.contains(&site));
        obs.tick = 724;
        policy.observe_work_experience(&obs);
        assert!(policy.state.work_experience.dead_anchors.is_empty());
    }

    #[test]
    fn dispatched_move_replaces_harvest_memory_but_queued_move_does_not() {
        let unit = UnitId(3);
        let immediate = Command::Move {
            units: vec![unit],
            goal: TilePos::new(8, 5),
            queue: false,
        };
        let queued = Command::Move {
            units: vec![unit],
            goal: TilePos::new(8, 5),
            queue: true,
        };
        let harvest = Command::Harvest {
            units: vec![unit],
            node: TilePos::new(6, 5),
            queue: false,
        };

        assert_eq!(
            queue_replacing_non_harvest_units(&immediate),
            Some(&[unit][..])
        );
        assert_eq!(queue_replacing_non_harvest_units(&queued), None);
        assert_eq!(queue_replacing_non_harvest_units(&harvest), None);
    }
    #[test]
    fn every_nonqueued_worker_retask_clears_its_harvest_assignment() {
        let unit = UnitId(3);
        let commands = [
            Command::Repair {
                units: vec![unit],
                building: BuildingId(9),
                queue: false,
            },
            Command::RepairUnit {
                units: vec![unit],
                target: UnitId(4),
                queue: false,
            },
            Command::Advance {
                units: vec![unit],
                goal: TilePos::new(8, 5),
                queue: false,
            },
            Command::Patrol {
                units: vec![unit],
                waypoints: vec![TilePos::new(8, 5), TilePos::new(9, 5)],
            },
        ];

        for command in &commands {
            assert_eq!(
                queue_replacing_non_harvest_units(command),
                Some(&[unit][..]),
                "{command:?} replaces the worker's current Harvest program"
            );
        }

        for command in [
            Command::Repair {
                units: vec![unit],
                building: BuildingId(9),
                queue: true,
            },
            Command::RepairUnit {
                units: vec![unit],
                target: UnitId(4),
                queue: true,
            },
            Command::Advance {
                units: vec![unit],
                goal: TilePos::new(8, 5),
                queue: true,
            },
        ] {
            assert_eq!(
                queue_replacing_non_harvest_units(&command),
                None,
                "{command:?} preserves the active Harvest until the queue advances"
            );
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct BuildAttempt {
    worker: UnitId,
    kind: BuildingKind,
    anchor: TilePos,
    from: TilePos,
    journal: OutcomeJournal,
}

impl BuildAttempt {
    fn site_currently_occupied(&self, obs: &Observation) -> bool {
        let Some(site) = SiteFootprint::new(self.anchor, self.kind.base_stats().size) else {
            return false;
        };
        obs.my_buildings
            .iter()
            .chain(&obs.ally_buildings)
            .chain(&obs.enemy_buildings)
            .filter(|building| building.seen && building.hp > 0)
            .any(|building| {
                SiteFootprint::new(
                    building.anchor,
                    building.kind.tier_stats(building.tier).size,
                )
                .is_some_and(|occupant| site.overlaps(occupant))
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct FoundationWatch {
    building: oxide_sim::ids::BuildingId,
    cost: u32,
    journal: OutcomeJournal,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct HarvestAttempt {
    node: TilePos,
    since: Tick,
    carrying: u32,
    collected: u32,
    journal: OutcomeJournal,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct WorkExperience {
    pub(super) dead_nodes: Vec<TilePos>,
    pub(super) dead_anchors: Vec<TilePos>,
    pub(super) last_sent: Vec<(UnitId, TilePos, TilePos)>,
    pub(super) construction_work_tiles: BTreeSet<TilePos>,
    observed_at: Option<Tick>,
    serial: u64,
    nodes: Vec<FailedWork>,
    sites: Vec<FailedWork>,
    builds: Vec<BuildAttempt>,
    foundations: Vec<FoundationWatch>,
    harvests: BTreeMap<UnitId, HarvestAttempt>,
    repairs: BTreeMap<SupportKey, OutcomeJournal>,
    recon: BTreeMap<ReconQuestionKey, OutcomeJournal>,
    pub(crate) pending: Vec<EpisodeReport>,
}

impl WorkExperience {
    fn observe(
        &mut self,
        obs: &Observation,
        support: &super::support_allocation::SupportWork,
        reconnaissance: &super::reconnaissance::Reconnaissance,
        contested_regions: &[ContestedHarvestRegion],
    ) {
        if self.observed_at.is_some_and(|tick| tick >= obs.tick) {
            return;
        }
        self.observed_at = Some(obs.tick);
        self.construction_work_tiles = obs
            .my_units
            .iter()
            .filter(|unit| unit.hp > 0 && unit.site.is_some())
            .map(|unit| unit.tile)
            .collect();
        let retain = |failure: &FailedWork| {
            obs.tick.saturating_sub(failure.failed_at) < 1800
                && obs.tick < failure.failed_at.saturating_add(300)
                && (failure.visible || !obs.visible(failure.tile))
        };
        self.nodes.retain(retain);
        self.sites.retain(retain);
        self.dead_nodes = self.nodes.iter().map(|failure| failure.tile).collect();
        self.dead_anchors = self.sites.iter().map(|failure| failure.tile).collect();
        self.audit_harvests(obs);
        for (id, mut attempt) in std::mem::take(&mut self.harvests) {
            let worker = obs
                .my_units
                .iter()
                .find(|unit| unit.id == id && unit.hp > 0);
            if let Some(worker) = worker {
                attempt.collected = attempt
                    .collected
                    .saturating_add(worker.carrying.saturating_sub(attempt.carrying));
                attempt.carrying = worker.carrying;
                attempt.journal.progress(attempt.collected);
            }
            let exhausted = obs.visible(attempt.node)
                && !obs
                    .known_scrap
                    .iter()
                    .chain(&obs.known_wrecks)
                    .any(|(tile, amount)| *tile == attempt.node && *amount > 0);
            if worker.is_none() {
                let carried = crate::experience::own_unit_health(obs, id).is_some();
                attempt.journal.finish(
                    obs,
                    if carried {
                        Outcome::Invalidated
                    } else {
                        Outcome::Ineffective
                    },
                    if carried {
                        OutcomeReason::Preempted
                    } else {
                        OutcomeReason::RequiredUnitLost
                    },
                    1000,
                    false,
                );
            } else if exhausted {
                attempt.journal.finish(
                    obs,
                    if attempt.collected > 0 {
                        Outcome::Complete
                    } else {
                        Outcome::Invalidated
                    },
                    OutcomeReason::ResourceExhausted,
                    1000,
                    false,
                );
            } else if worker.is_some_and(|unit| unit.harvesting == Some(attempt.node))
                && obs.tick.saturating_sub(attempt.since) < 1800
            {
                self.harvests.insert(id, attempt);
                continue;
            } else {
                attempt.journal.finish(
                    obs,
                    if attempt.collected > 0 {
                        Outcome::Partial
                    } else {
                        Outcome::Invalidated
                    },
                    OutcomeReason::Preempted,
                    1000,
                    false,
                );
            }
            self.pending.extend(attempt.journal.pending);
        }
        for mut attempt in std::mem::take(&mut self.builds) {
            let appeared = obs.my_buildings.iter().find(|building| {
                building.kind == attempt.kind && building.anchor == attempt.anchor
            });
            let worker = obs
                .my_units
                .iter()
                .find(|unit| unit.id == attempt.worker && unit.hp > 0);
            if let Some(building) = appeared {
                attempt.journal.progress(1);
                self.foundations.push(FoundationWatch {
                    building: building.id,
                    cost: attempt
                        .kind
                        .base_stats()
                        .construction
                        .map_or(0, |cost| cost.cost),
                    journal: attempt.journal,
                });
                continue;
            } else if worker
                .is_some_and(|unit| unit.founding == Some((attempt.kind, attempt.anchor)))
            {
                self.builds.push(attempt);
                continue;
            } else if worker.is_none() {
                let carried = crate::experience::own_unit_health(obs, attempt.worker).is_some();
                attempt.journal.finish(
                    obs,
                    if carried {
                        Outcome::Invalidated
                    } else {
                        Outcome::Ineffective
                    },
                    if carried {
                        OutcomeReason::Preempted
                    } else {
                        OutcomeReason::RequiredUnitLost
                    },
                    1000,
                    false,
                );
            } else if attempt.site_currently_occupied(obs) {
                attempt.journal.finish(
                    obs,
                    Outcome::Invalidated,
                    OutcomeReason::SiteOccupied,
                    1000,
                    false,
                );
            } else if attempt
                .kind
                .base_stats()
                .construction
                .is_some_and(|cost| obs.scrap < cost.cost)
            {
                attempt.journal.finish(
                    obs,
                    Outcome::Invalidated,
                    OutcomeReason::FundingUnavailable,
                    1000,
                    false,
                );
            } else if worker.is_some_and(|unit| {
                unit.idle
                    && (unit.tile.chebyshev(attempt.from) <= 1
                        || unit.tile.chebyshev(attempt.anchor) <= 3)
            }) {
                self.sites.push(FailedWork {
                    tile: attempt.anchor,
                    failed_at: obs.tick,
                    visible: obs.visible(attempt.anchor),
                });
                self.dead_anchors.push(attempt.anchor);
                attempt.journal.finish(
                    obs,
                    Outcome::Aborted,
                    OutcomeReason::BlockedRoute,
                    750,
                    false,
                );
            } else {
                attempt.journal.finish(
                    obs,
                    Outcome::Invalidated,
                    OutcomeReason::Preempted,
                    1000,
                    false,
                );
            }
            self.pending.extend(attempt.journal.pending);
        }
        for mut foundation in std::mem::take(&mut self.foundations) {
            let building = obs
                .my_buildings
                .iter()
                .find(|building| building.id == foundation.building);
            if building.is_some_and(|building| !building.built && building.hp > 0) {
                self.foundations.push(foundation);
                continue;
            }
            let completed = building.is_some_and(|building| building.built && building.hp > 0);
            foundation.journal.finish(
                obs,
                if completed {
                    Outcome::Complete
                } else {
                    Outcome::Ineffective
                },
                if completed {
                    OutcomeReason::ServiceCompleted
                } else {
                    OutcomeReason::RequiredUnitLost
                },
                1000,
                false,
            );
            for report in &mut foundation.journal.pending {
                // After laying the foundation the worker is free; its later fate
                // is not a construction loss.
                report.own_lost_value = if completed { 0 } else { foundation.cost };
            }
            self.pending.extend(foundation.journal.pending);
        }
        for repair in &support.repairs {
            let key = repair.key;
            if !self.repairs.contains_key(&key) {
                let id = self.id(EpisodeOwner::Support);
                let mut journal = OutcomeJournal::default();
                let (tile, subject) = match key.patient {
                    Target::Unit(id) => (
                        obs.my_units
                            .iter()
                            .find(|unit| unit.id == id)
                            .map_or(TilePos::new(0, 0), |unit| unit.tile),
                        u64::from(id.0),
                    ),
                    Target::Building(id) => (
                        obs.my_buildings
                            .iter()
                            .find(|building| building.id == id)
                            .map_or(TilePos::new(0, 0), |building| building.anchor),
                        u64::from(id.0),
                    ),
                };
                journal.watch(
                    obs,
                    id,
                    ExperienceKey {
                        doctrine: Doctrine::Sustain,
                        x: tile.x,
                        y: tile.y,
                        subject,
                    },
                    &[key.worker],
                    0,
                );
                self.repairs.insert(key, journal);
            }
        }
        for (key, mut journal) in std::mem::take(&mut self.repairs) {
            let healthy = match key.patient {
                Target::Unit(id) => obs
                    .my_units
                    .iter()
                    .find(|unit| unit.id == id)
                    .map(|unit| unit.hp >= unit.kind.stats().max_hp),
                Target::Building(id) => obs
                    .my_buildings
                    .iter()
                    .find(|building| building.id == id)
                    .map(|building| building.hp >= building.kind.tier_stats(building.tier).max_hp),
            };
            if healthy == Some(true) {
                journal.finish(
                    obs,
                    Outcome::Complete,
                    OutcomeReason::ServiceCompleted,
                    750,
                    true,
                );
            } else if healthy.is_none() || obs.repair_target(key.worker) != Some(key.patient) {
                journal.finish(
                    obs,
                    Outcome::Invalidated,
                    OutcomeReason::Preempted,
                    1000,
                    false,
                );
            } else {
                self.repairs.insert(key, journal);
                continue;
            }
            self.pending.append(&mut journal.pending);
            if support.repairs.iter().any(|repair| repair.key == key) {
                self.repairs.insert(key, journal);
            }
        }
        for (key, work) in &reconnaissance.assignments {
            let Some(unit) = work.unit else { continue };
            if !self.recon.contains_key(key) {
                let id = self.id(EpisodeOwner::Reconnaissance);
                let mut journal = OutcomeJournal::default();
                journal.watch(
                    obs,
                    id,
                    ExperienceKey {
                        doctrine: Doctrine::Pressure,
                        y: key.y,
                        x: key.x,
                        subject: 0,
                    },
                    &[unit],
                    0,
                );
                self.recon.insert(*key, journal);
            }
        }
        for (key, mut journal) in std::mem::take(&mut self.recon) {
            if let Some(work) = reconnaissance.assignments.get(&key) {
                if work
                    .unit
                    .is_some_and(|id| crate::experience::own_unit_health(obs, id).is_none())
                {
                    journal.finish(
                        obs,
                        Outcome::Ineffective,
                        OutcomeReason::RequiredUnitLost,
                        1000,
                        false,
                    );
                } else if work.proposal.question.answered(obs, contested_regions) {
                    journal.finish(
                        obs,
                        Outcome::Complete,
                        OutcomeReason::ServiceCompleted,
                        1000,
                        false,
                    );
                } else if matches!(
                    key.consumer,
                    super::reconnaissance::ReconConsumer::Approach(_)
                ) && work.phase == super::reconnaissance::ReconPhase::Recall
                    && !work.inspected.is_empty()
                {
                    journal.progress(work.inspected.len() as u32);
                    journal.finish(
                        obs,
                        Outcome::Partial,
                        OutcomeReason::ObservedProgress,
                        500,
                        false,
                    );
                } else if obs.tick >= work.proposal.deadline {
                    journal.finish(
                        obs,
                        Outcome::Inconclusive,
                        OutcomeReason::Deadline,
                        0,
                        false,
                    );
                } else {
                    self.recon.insert(key, journal);
                    continue;
                }
            } else {
                journal.finish(
                    obs,
                    Outcome::Invalidated,
                    OutcomeReason::Preempted,
                    1000,
                    false,
                );
            }
            self.pending.append(&mut journal.pending);
            if reconnaissance.assignments.contains_key(&key) {
                self.recon.insert(key, journal);
            }
        }
        self.dead_nodes
            .sort_unstable_by_key(|tile| (tile.y, tile.x));
        self.dead_nodes.dedup();
        self.dead_anchors
            .sort_unstable_by_key(|tile| (tile.y, tile.x));
        self.dead_anchors.dedup();
    }
    fn id(&mut self, owner: EpisodeOwner) -> EpisodeId {
        let id = EpisodeId {
            owner,
            serial: self.serial,
        };
        self.serial = self.serial.saturating_add(1);
        id
    }
}

impl UtilityPolicy {
    pub(crate) fn record_dispatched_work(
        &mut self,
        obs: &Observation,
        orientation: crate::Orientation,
        commands: &[oxide_sim::PlayerCommand],
    ) {
        for command in commands {
            if let Some(units) = queue_replacing_non_harvest_units(&command.command) {
                let build = if let Command::Build { kind, anchor, .. } = command.command {
                    Some((kind, orientation.anchor(anchor, kind.base_stats().size)))
                } else {
                    None
                };
                self.state
                    .work_experience
                    .record_work_retask(obs, units, build);
            }
            match &command.command {
                Command::Build {
                    units,
                    kind,
                    anchor,
                    ..
                } => {
                    let oriented_anchor = orientation.anchor(*anchor, kind.base_stats().size);
                    self.record_dispatched_foundry_build(units, *kind, oriented_anchor);
                    self.state.work_experience.record_exact_build_attempt(
                        obs,
                        units,
                        *kind,
                        oriented_anchor,
                    );
                }
                Command::Harvest { units, node, .. } => {
                    let oriented_node = orientation.tile(*node);
                    for &unit in units {
                        self.state.work_experience.record_dispatched_harvest(
                            obs,
                            unit,
                            oriented_node,
                        );
                    }
                }
                Command::Cancel { building } => {
                    self.state
                        .work_experience
                        .record_foundation_cancellation(obs, *building);
                }
                _ => {}
            }
        }
    }

    pub(crate) fn observe_work_experience(&mut self, obs: &Observation) {
        self.state.work_experience.observe(
            obs,
            &self.state.support_work,
            &self.state.reconnaissance,
            &self.state.contested_harvest_regions,
        );
    }
}

/// Unit orders that replace a worker's current Harvest program. Keeping this
/// match exhaustive makes a future command variant choose its bookkeeping
/// semantics explicitly.
fn queue_replacing_non_harvest_units(command: &Command) -> Option<&[UnitId]> {
    match command {
        Command::Move {
            units,
            queue: false,
            ..
        }
        | Command::Attack {
            units,
            queue: false,
            ..
        }
        | Command::AttackMove {
            units,
            queue: false,
            ..
        }
        | Command::Build {
            units,
            queue: false,
            ..
        }
        | Command::Repair {
            units,
            queue: false,
            ..
        }
        | Command::Salvage {
            units,
            queue: false,
            ..
        }
        | Command::RepairUnit {
            units,
            queue: false,
            ..
        }
        | Command::Advance {
            units,
            queue: false,
            ..
        }
        | Command::Load {
            units,
            queue: false,
            ..
        }
        | Command::ReturnCargo { units, .. }
        | Command::Patrol { units, .. }
        | Command::Stop { units } => Some(units),
        Command::Move { queue: true, .. }
        | Command::Attack { queue: true, .. }
        | Command::AttackMove { queue: true, .. }
        | Command::Harvest { .. }
        | Command::Build { queue: true, .. }
        | Command::Repair { queue: true, .. }
        | Command::Salvage { queue: true, .. }
        | Command::RepairUnit { queue: true, .. }
        | Command::Advance { queue: true, .. }
        | Command::Load { queue: true, .. }
        | Command::Train { .. }
        | Command::Cancel { .. }
        | Command::CancelTrain { .. }
        | Command::SetRally { .. }
        | Command::Surrender
        | Command::FocusFire { .. }
        | Command::CancelFound { .. }
        | Command::UpgradeBuilding { .. }
        | Command::Unload { .. }
        | Command::ClearFocus { .. } => None,
    }
}

impl WorkExperience {
    /// Only a still-valued source can prove a route bounce. Exhaustion must
    /// not exclude future deposits at the same tile.
    fn audit_harvests(&mut self, obs: &Observation) {
        for (id, node, sent_from) in std::mem::take(&mut self.last_sent) {
            // Collision separation can nudge a routeless worker one tile
            // from its send point, so exact equality misses a bounce.
            let bounced = obs
                .my_units
                .iter()
                .any(|u| u.id == id && u.idle && u.hp > 0 && u.tile.chebyshev(sent_from) <= 1);
            let still_reports = obs
                .known_scrap
                .iter()
                .chain(obs.known_wrecks.iter())
                .any(|(pos, amount)| *pos == node && *amount > 0);
            if bounced && still_reports && !self.dead_nodes.contains(&node) {
                self.dead_nodes.push(node);
                self.record_failed_harvest(obs, id, node);
            }
        }
    }

    /// Remember a Harvest command that survived intent lowering long enough
    /// to audit an immediate no-route bounce on the next think.
    pub(super) fn record_dispatched_harvest(
        &mut self,
        obs: &Observation,
        unit: UnitId,
        node: TilePos,
    ) {
        let Some(worker) = obs.my_units.iter().find(|worker| worker.id == unit) else {
            return;
        };
        self.last_sent.retain(|(sent, _, _)| *sent != unit);
        self.last_sent.push((unit, node, worker.tile));
        self.record_harvest_episode(obs, unit, node);
    }

    fn record_failed_harvest(&mut self, obs: &Observation, worker: UnitId, tile: TilePos) {
        self.nodes.push(FailedWork {
            tile,
            failed_at: obs.tick,
            visible: obs.visible(tile),
        });
        let id = self.id(EpisodeOwner::Harvest);
        let mut journal = self
            .harvests
            .remove(&worker)
            .map_or_else(OutcomeJournal::default, |attempt| attempt.journal);
        let id = journal.episode_id().unwrap_or(id);
        journal.watch(
            obs,
            id,
            ExperienceKey {
                doctrine: Doctrine::Expansion,
                x: tile.x,
                y: tile.y,
                subject: 0,
            },
            &[worker],
            0,
        );
        journal.finish(
            obs,
            Outcome::Aborted,
            OutcomeReason::BlockedRoute,
            750,
            false,
        );
        self.pending.extend(journal.pending);
    }

    fn record_harvest_episode(&mut self, obs: &Observation, worker: UnitId, node: TilePos) {
        if self
            .harvests
            .get(&worker)
            .is_some_and(|attempt| attempt.node == node)
        {
            return;
        }
        let Some(unit) = obs.my_units.iter().find(|unit| unit.id == worker) else {
            return;
        };
        let id = self.id(EpisodeOwner::Harvest);
        let mut journal = OutcomeJournal::default();
        journal.watch(
            obs,
            id,
            ExperienceKey {
                doctrine: Doctrine::Expansion,
                x: node.x,
                y: node.y,
                subject: 0,
            },
            &[worker],
            0,
        );
        self.harvests.insert(
            worker,
            HarvestAttempt {
                node,
                since: obs.tick,
                carrying: unit.carrying,
                collected: 0,
                journal,
            },
        );
    }

    pub(crate) fn record_foundation_cancellation(
        &mut self,
        obs: &Observation,
        building: BuildingId,
    ) {
        let Some(index) = self
            .foundations
            .iter()
            .position(|foundation| foundation.building == building)
        else {
            return;
        };
        let mut foundation = self.foundations.remove(index);
        foundation.journal.finish(
            obs,
            Outcome::Invalidated,
            OutcomeReason::Preempted,
            1000,
            false,
        );
        for report in &mut foundation.journal.pending {
            report.own_lost_value = 0;
        }
        self.pending.extend(foundation.journal.pending);
    }

    pub(crate) fn record_work_retask(
        &mut self,
        obs: &Observation,
        units: &[UnitId],
        build: Option<(BuildingKind, TilePos)>,
    ) {
        self.last_sent.retain(|(unit, _, _)| !units.contains(unit));
        for mut attempt in std::mem::take(&mut self.builds) {
            if units.contains(&attempt.worker) && build != Some((attempt.kind, attempt.anchor)) {
                attempt.journal.finish(
                    obs,
                    Outcome::Invalidated,
                    OutcomeReason::Preempted,
                    1000,
                    false,
                );
                self.pending.extend(attempt.journal.pending);
            } else {
                self.builds.push(attempt);
            }
        }
        for id in units {
            if let Some(mut attempt) = self.harvests.remove(id) {
                attempt.journal.finish(
                    obs,
                    if attempt.collected > 0 {
                        Outcome::Partial
                    } else {
                        Outcome::Invalidated
                    },
                    OutcomeReason::Preempted,
                    1000,
                    false,
                );
                self.pending.extend(attempt.journal.pending);
            }
        }
    }

    pub(crate) fn record_exact_build_attempt(
        &mut self,
        obs: &Observation,
        workers: &[UnitId],
        kind: BuildingKind,
        anchor: TilePos,
    ) {
        if obs
            .my_buildings
            .iter()
            .any(|building| building.anchor == anchor)
        {
            return;
        }
        let Some(worker) = workers
            .first()
            .and_then(|id| obs.my_units.iter().find(|unit| unit.id == *id))
        else {
            return;
        };
        if self.builds.iter().any(|attempt| {
            attempt.worker == worker.id && attempt.kind == kind && attempt.anchor == anchor
        }) {
            return;
        }
        let id = self.id(EpisodeOwner::Construction);
        let mut journal = OutcomeJournal::default();
        journal.watch(
            obs,
            id,
            ExperienceKey {
                doctrine: match kind {
                    BuildingKind::RepairBay => Doctrine::Sustain,
                    BuildingKind::Turret
                    | BuildingKind::FlakTurret
                    | BuildingKind::Bastion
                    | BuildingKind::ScuttleCharge
                    | BuildingKind::Barricade
                    | BuildingKind::Array => Doctrine::Fortification,
                    _ => Doctrine::Expansion,
                },
                x: anchor.x,
                y: anchor.y,
                subject: kind as u64,
            },
            &[worker.id],
            0,
        );
        self.builds.push(BuildAttempt {
            worker: worker.id,
            kind,
            anchor,
            from: worker.tile,
            journal,
        });
    }
}
