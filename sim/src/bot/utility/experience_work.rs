use super::*;
use crate::bot::experience::{
    Doctrine, EpisodeId, EpisodeOwner, EpisodeReport, ExperienceKey, Outcome, OutcomeJournal,
    OutcomeReason,
};
use crate::ids::Target;
use chassis::Tick;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
struct FailedWork {
    tile: TilePos,
    failed_at: Tick,
    visible: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation() -> Observation {
        Observation {
            tick: 100,
            map_width: 30,
            map_height: 20,
            visible: vec![true; 600],
            explored: vec![true; 600],
            scrap: 1000,
            known_scrap: vec![(TilePos::new(10, 10), 1000)],
            my_units: vec![UnitObs {
                id: UnitId(1),
                player: crate::ids::PlayerId(0),
                kind: UnitKind::Harvester,
                tile: TilePos::new(3, 3),
                hp: UnitKind::Harvester.stats().max_hp,
                idle: true,
                carrying: 0,
                harvesting: None,
                cargo: 0,
                site: None,
                salvaging: None,
                founding: None,
                repairing: false,
                grounded: false,
            }],
            ..Observation::default()
        }
    }

    #[test]
    fn harvest_bounce_has_one_exact_episode_and_bounded_retry() {
        let mut obs = observation();
        let node = TilePos::new(10, 10);
        let mut policy = UtilityPolicy::default();
        policy.observe_work_experience(&obs);
        policy.record_dispatched_harvest(&obs, UnitId(1), node);
        obs.tick = 124;
        policy.observe_work_experience(&obs);
        assert!(policy.dead_nodes.contains(&node));
        assert_eq!(policy.work_experience.pending.len(), 1);
        assert_eq!(policy.work_experience.pending[0].started_at, 100);
        assert_eq!(
            policy.work_experience.pending[0].reason,
            OutcomeReason::BlockedRoute
        );
        policy.observe_work_experience(&obs);
        assert_eq!(policy.work_experience.pending.len(), 1);
        obs.tick = 424;
        policy.observe_work_experience(&obs);
        assert!(!policy.dead_nodes.contains(&node));
        assert!(
            policy.last_sent.is_empty(),
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
            policy.record_dispatched_harvest(&obs, UnitId(1), node);
            policy.record_exact_build_attempt(&obs, &[UnitId(1)], BuildingKind::Fabricator, node);
            match cause {
                0 => {
                    policy.record_dispatched_retask(&[UnitId(1)]);
                    policy.record_work_retask(&obs, &[UnitId(1)], None);
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
            assert!(policy.dead_nodes.is_empty(), "cause {cause}");
            assert!(policy.dead_anchors.is_empty(), "cause {cause}");
            assert!(
                policy
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
                policy.record_exact_build_attempt(
                    &obs,
                    &[UnitId(1)],
                    BuildingKind::Fabricator,
                    site,
                );
                let occupant = BuildingObs {
                    id: BuildingId(8),
                    player: PlayerId(owner),
                    kind: BuildingKind::RepairBay,
                    anchor: site.offset(1, 0),
                    hp: 10,
                    built,
                    seen: true,
                    tier: 0,
                };
                match owner {
                    0 => obs.my_buildings.push(occupant),
                    1 => obs.ally_buildings.push(occupant),
                    _ => obs.enemy_buildings.push(occupant),
                }
                obs.tick = 124;
                policy.observe_work_experience(&obs);
                assert!(
                    policy.dead_anchors.is_empty(),
                    "owner {owner}, built {built}"
                );
                assert!(policy.work_experience.builds.is_empty());
                assert_eq!(policy.work_experience.pending.len(), 1);
                let report = &policy.work_experience.pending[0];
                assert_eq!(report.outcome, Outcome::Invalidated);
                assert_eq!(report.reason, OutcomeReason::SiteOccupied);
                assert_eq!(report.own_lost_value, 0);
                assert!(!report.doctrine_eligible);
                policy.observe_work_experience(&obs);
                assert_eq!(policy.work_experience.pending.len(), 1);
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
            policy.record_exact_build_attempt(&obs, &[UnitId(1)], BuildingKind::Fabricator, site);
            obs.enemy_buildings.push(BuildingObs {
                id: BuildingId(8),
                player: PlayerId(2),
                kind: BuildingKind::RepairBay,
                anchor: site.offset(offset, 0),
                hp,
                built: true,
                seen,
                tier: 0,
            });
            obs.tick = 124;
            policy.observe_work_experience(&obs);
            assert_eq!(policy.work_experience.pending.len(), 1);
            assert_eq!(
                policy.work_experience.pending[0].reason,
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
        policy.record_exact_build_attempt(&obs, &[UnitId(1)], BuildingKind::Fabricator, site);
        obs.tick = 124;
        obs.my_buildings.push(BuildingObs {
            id: BuildingId(8),
            player: obs.me,
            kind: BuildingKind::Fabricator,
            anchor: site,
            hp: 10,
            built: false,
            seen: true,
            tier: 0,
        });
        policy.observe_work_experience(&obs);
        assert!(policy.work_experience.builds.is_empty());
        assert_eq!(policy.work_experience.foundations.len(), 1);
        assert!(policy.work_experience.pending.is_empty());
        let mut cancelled = policy.clone();
        cancelled.record_foundation_cancellation(&obs, BuildingId(8));
        assert!(cancelled.work_experience.foundations.is_empty());
        let cancellation = &cancelled.work_experience.pending[0];
        assert_eq!(cancellation.outcome, Outcome::Invalidated);
        assert_eq!(cancellation.reason, OutcomeReason::Preempted);
        assert_eq!(cancellation.own_lost_value, 0);
        cancelled.record_foundation_cancellation(&obs, BuildingId(8));
        assert_eq!(cancelled.work_experience.pending.len(), 1);
        policy.record_work_retask(&obs, &[UnitId(1)], None);
        obs.my_units.clear();
        obs.tick = 148;
        obs.my_buildings[0].built = true;
        policy.observe_work_experience(&obs);
        assert!(policy.dead_anchors.is_empty());
        assert!(policy.work_experience.foundations.is_empty());
        assert_eq!(policy.work_experience.pending.len(), 1);
        assert_eq!(policy.work_experience.pending[0].outcome, Outcome::Complete);
        assert_eq!(policy.work_experience.pending[0].own_lost_value, 0);
    }

    #[test]
    fn failed_build_retries_after_delay_but_a_walking_founder_keeps_its_claim() {
        let mut obs = observation();
        let site = TilePos::new(10, 10);
        let mut policy = UtilityPolicy::default();
        policy.observe_work_experience(&obs);
        policy.record_exact_build_attempt(&obs, &[UnitId(1)], BuildingKind::Fabricator, site);
        obs.my_units[0].founding = Some((BuildingKind::Fabricator, site));
        obs.my_units[0].idle = false;
        obs.tick = 400;
        policy.observe_work_experience(&obs);
        assert!(policy.dead_anchors.is_empty());
        assert_eq!(policy.work_experience.builds.len(), 1);
        obs.my_units[0].founding = None;
        obs.my_units[0].idle = true;
        obs.tick = 424;
        policy.observe_work_experience(&obs);
        assert!(policy.dead_anchors.contains(&site));
        obs.tick = 724;
        policy.observe_work_experience(&obs);
        assert!(policy.dead_anchors.is_empty());
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct FoundationWatch {
    building: crate::ids::BuildingId,
    cost: u32,
    journal: OutcomeJournal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HarvestAttempt {
    node: TilePos,
    since: Tick,
    carrying: u32,
    collected: u32,
    journal: OutcomeJournal,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::bot) struct WorkExperience {
    pub(super) enabled: bool,
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
    pub(in crate::bot) pending: Vec<EpisodeReport>,
}

impl WorkExperience {
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
    pub(in crate::bot) fn observe_work_experience(&mut self, obs: &Observation) {
        self.work_experience.enabled = true;
        if self
            .work_experience
            .observed_at
            .is_some_and(|tick| tick >= obs.tick)
        {
            return;
        }
        self.work_experience.observed_at = Some(obs.tick);
        self.work_experience.construction_work_tiles = obs
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
        self.work_experience.nodes.retain(retain);
        self.work_experience.sites.retain(retain);
        self.dead_nodes = self
            .work_experience
            .nodes
            .iter()
            .map(|failure| failure.tile)
            .collect();
        self.dead_anchors = self
            .work_experience
            .sites
            .iter()
            .map(|failure| failure.tile)
            .collect();
        self.audit_harvests(obs);
        for (id, mut attempt) in std::mem::take(&mut self.work_experience.harvests) {
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
                let carried = crate::bot::experience::own_unit_health(obs, id).is_some();
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
                self.work_experience.harvests.insert(id, attempt);
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
            self.work_experience.pending.extend(attempt.journal.pending);
        }
        for mut attempt in std::mem::take(&mut self.work_experience.builds) {
            let appeared = obs.my_buildings.iter().find(|building| {
                building.kind == attempt.kind && building.anchor == attempt.anchor
            });
            let worker = obs
                .my_units
                .iter()
                .find(|unit| unit.id == attempt.worker && unit.hp > 0);
            if let Some(building) = appeared {
                attempt.journal.progress(1);
                self.work_experience.foundations.push(FoundationWatch {
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
                self.work_experience.builds.push(attempt);
                continue;
            } else if worker.is_none() {
                let carried =
                    crate::bot::experience::own_unit_health(obs, attempt.worker).is_some();
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
                self.work_experience.sites.push(FailedWork {
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
            self.work_experience.pending.extend(attempt.journal.pending);
        }
        for mut foundation in std::mem::take(&mut self.work_experience.foundations) {
            let building = obs
                .my_buildings
                .iter()
                .find(|building| building.id == foundation.building);
            if building.is_some_and(|building| !building.built && building.hp > 0) {
                self.work_experience.foundations.push(foundation);
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
            self.work_experience
                .pending
                .extend(foundation.journal.pending);
        }
        for repair in &self.support_work.repairs {
            let key = repair.key;
            if !self.work_experience.repairs.contains_key(&key) {
                let id = self.work_experience.id(EpisodeOwner::Support);
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
                self.work_experience.repairs.insert(key, journal);
            }
        }
        for (key, mut journal) in std::mem::take(&mut self.work_experience.repairs) {
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
                self.work_experience.repairs.insert(key, journal);
                continue;
            }
            self.work_experience.pending.append(&mut journal.pending);
            if self
                .support_work
                .repairs
                .iter()
                .any(|repair| repair.key == key)
            {
                self.work_experience.repairs.insert(key, journal);
            }
        }
        for (key, work) in &self.reconnaissance.assignments {
            let Some(unit) = work.unit else { continue };
            if !self.work_experience.recon.contains_key(key) {
                let id = self.work_experience.id(EpisodeOwner::Reconnaissance);
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
                self.work_experience.recon.insert(*key, journal);
            }
        }
        for (key, mut journal) in std::mem::take(&mut self.work_experience.recon) {
            if let Some(work) = self.reconnaissance.assignments.get(&key) {
                if work
                    .unit
                    .is_some_and(|id| crate::bot::experience::own_unit_health(obs, id).is_none())
                {
                    journal.finish(
                        obs,
                        Outcome::Ineffective,
                        OutcomeReason::RequiredUnitLost,
                        1000,
                        false,
                    );
                } else if self.recon_answered(obs, &work.proposal.question) {
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
                    self.work_experience.recon.insert(key, journal);
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
            self.work_experience.pending.append(&mut journal.pending);
            if self.reconnaissance.assignments.contains_key(&key) {
                self.work_experience.recon.insert(key, journal);
            }
        }
        self.dead_nodes
            .sort_unstable_by_key(|tile| (tile.y, tile.x));
        self.dead_nodes.dedup();
        self.dead_anchors
            .sort_unstable_by_key(|tile| (tile.y, tile.x));
        self.dead_anchors.dedup();
    }

    pub(super) fn record_failed_work(
        &mut self,
        obs: &Observation,
        worker: UnitId,
        tile: TilePos,
        kind: Option<BuildingKind>,
    ) {
        self.work_experience.nodes.push(FailedWork {
            tile,
            failed_at: obs.tick,
            visible: obs.visible(tile),
        });
        let id = self.work_experience.id(EpisodeOwner::Harvest);
        let mut journal = self
            .work_experience
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
                subject: kind.map_or(0, |kind| kind as u64),
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
        self.work_experience.pending.extend(journal.pending);
    }

    pub(super) fn record_harvest_episode(
        &mut self,
        obs: &Observation,
        worker: UnitId,
        node: TilePos,
    ) {
        if !self.work_experience.enabled
            || self
                .work_experience
                .harvests
                .get(&worker)
                .is_some_and(|attempt| attempt.node == node)
        {
            return;
        }
        let Some(unit) = obs.my_units.iter().find(|unit| unit.id == worker) else {
            return;
        };
        let id = self.work_experience.id(EpisodeOwner::Harvest);
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
        self.work_experience.harvests.insert(
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

    pub(in crate::bot) fn record_foundation_cancellation(
        &mut self,
        obs: &Observation,
        building: BuildingId,
    ) {
        let Some(index) = self
            .work_experience
            .foundations
            .iter()
            .position(|foundation| foundation.building == building)
        else {
            return;
        };
        let mut foundation = self.work_experience.foundations.remove(index);
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
        self.work_experience
            .pending
            .extend(foundation.journal.pending);
    }

    pub(in crate::bot) fn record_work_retask(
        &mut self,
        obs: &Observation,
        units: &[UnitId],
        build: Option<(BuildingKind, TilePos)>,
    ) {
        for mut attempt in std::mem::take(&mut self.work_experience.builds) {
            if units.contains(&attempt.worker) && build != Some((attempt.kind, attempt.anchor)) {
                attempt.journal.finish(
                    obs,
                    Outcome::Invalidated,
                    OutcomeReason::Preempted,
                    1000,
                    false,
                );
                self.work_experience.pending.extend(attempt.journal.pending);
            } else {
                self.work_experience.builds.push(attempt);
            }
        }
        for id in units {
            if let Some(mut attempt) = self.work_experience.harvests.remove(id) {
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
                self.work_experience.pending.extend(attempt.journal.pending);
            }
        }
    }

    pub(in crate::bot) fn record_exact_build_attempt(
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
        if self.work_experience.builds.iter().any(|attempt| {
            attempt.worker == worker.id && attempt.kind == kind && attempt.anchor == anchor
        }) {
            return;
        }
        let id = self.work_experience.id(EpisodeOwner::Construction);
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
        self.work_experience.builds.push(BuildAttempt {
            worker: worker.id,
            kind,
            anchor,
            from: worker.tile,
            journal,
        });
    }
}
