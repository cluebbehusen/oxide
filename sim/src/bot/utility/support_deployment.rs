use super::*;
use crate::bot::allocation::{
    ClaimBundle, Confidence, ExecutionSafety, ProposalCase, StrategicValue, TimeToImpact, Urgency,
};
use crate::ids::Target;
use crate::stats::Role;
use chassis::Tick;
use std::collections::BTreeMap;

const HORIZON: Tick = 1_800;
const QUIET: Tick = 300;
const SERVICE_RADIUS: i32 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ProtectionKey {
    pub(crate) asset: Target,
    pub(crate) air: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProtectionRequest {
    pub(crate) key: ProtectionKey,
    pub(crate) tile: TilePos,
    pub(crate) pressure: u64,
    pub(crate) missing: u64,
    pub(crate) value: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SupportDeployment {
    pub(crate) key: ProtectionKey,
    pub(crate) unit: UnitId,
    pub(crate) accepted_at: Tick,
    pub(crate) deadline: Tick,
    pub(crate) goal: TilePos,
    pub(crate) arrival_at: Tick,
    pub(crate) quiet_since: Option<Tick>,
}

impl SupportDeployment {
    pub(crate) fn covers(&self, request: &ProtectionRequest) -> bool {
        self.key.air == request.key.air && self.goal.chebyshev(request.tile) <= SERVICE_RADIUS
    }
    pub(crate) fn claims(&self) -> ClaimBundle {
        ClaimBundle::new(0, vec![], vec![], vec![self.unit], vec![], vec![])
            .expect("one exact protective deployment")
    }
    pub(crate) fn case(&self) -> ProposalCase {
        ProposalCase {
            urgency: Urgency::Pressing,
            confidence: Confidence::Current,
            value: StrategicValue::Material,
            time_to_impact: TimeToImpact::Near,
            safety: ExecutionSafety::Managed,
        }
    }
    pub(crate) fn intent(&self) -> Intent {
        Intent::AttackMoveUnits {
            units: vec![self.unit],
            goal: self.goal,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SupportDeployments {
    pub(crate) active: Vec<SupportDeployment>,
    pub(crate) requests: Vec<ProtectionRequest>,
    pub(crate) released: Vec<(
        SupportDeployment,
        crate::bot::trace::DeploymentReleaseReason,
    )>,
    evidence: BTreeMap<ProtectionKey, Tick>,
    observed_at: Option<Tick>,
}

fn provider(kind: UnitKind, air: bool) -> bool {
    if air {
        kind.role() == Role::AntiAir
    } else {
        matches!(kind.role(), Role::Sentinel | Role::Warden | Role::Breaker)
    }
}

fn strength(kind: UnitKind, hp: u32, air: bool) -> u64 {
    u64::from(hp)
        * kind
            .stats()
            .weapons
            .iter()
            .filter(|weapon| {
                weapon
                    .targets
                    .covers(if air { Domain::Air } else { Domain::Ground })
            })
            .map(super::super::executive::weapon_burst_dps100)
            .sum::<u64>()
}

fn asset(obs: &Observation, target: Target) -> Option<(TilePos, Domain, u32, bool)> {
    match target {
        Target::Unit(id) => obs
            .my_units
            .iter()
            .find(|unit| unit.id == id && unit.hp > 0)
            .map(|unit| {
                (
                    unit.tile,
                    unit.body_domain(),
                    unit.kind.stats().cost,
                    matches!(
                        unit.kind.role(),
                        Role::Harvester
                            | Role::Excavator
                            | Role::Tender
                            | Role::Lancer
                            | Role::Bombard
                            | Role::Avalanche
                            | Role::Skyhook
                    ),
                )
            }),
        Target::Building(id) => obs
            .my_buildings
            .iter()
            .find(|building| building.id == id && building.hp > 0)
            .map(|building| {
                (
                    building.anchor,
                    Domain::Ground,
                    building
                        .kind
                        .tier_stats(building.tier)
                        .construction
                        .map_or(crate::stats::FOUNDRY_REPAIR_PRICE, |stats| stats.cost),
                    true,
                )
            }),
    }
}

pub(super) fn protection_requests(
    context: EconomicInvestmentContext<'_>,
) -> Vec<ProtectionRequest> {
    let obs = context.obs;
    if obs.enemy_units.is_empty() {
        return vec![];
    }
    let mut requests = Vec::new();
    for target in obs.my_units.iter().map(|unit| Target::Unit(unit.id)).chain(
        obs.my_buildings
            .iter()
            .map(|building| Target::Building(building.id)),
    ) {
        let Some((tile, body, value, needs_screen)) = asset(obs, target) else {
            continue;
        };
        for air in [false, true] {
            if !air && !needs_screen {
                continue;
            }
            let pressure = obs
                .enemy_units
                .iter()
                .filter(|enemy| {
                    enemy.hp > 0
                        && (enemy.kind.stats().domain == Domain::Air) == air
                        && enemy.kind.stats().weapons.iter().any(|weapon| {
                            weapon.targets.covers(body)
                                && enemy.tile.chebyshev(tile)
                                    <= weapon.range.to_num::<i32>() + SERVICE_RADIUS
                        })
                })
                .map(|enemy| strength(enemy.kind, enemy.hp, body == Domain::Air))
                .sum::<u64>();
            if pressure == 0 {
                continue;
            }
            requests.push(ProtectionRequest {
                key: ProtectionKey { asset: target, air },
                tile,
                pressure,
                missing: pressure,
                value,
            });
        }
    }
    requests.sort_by_key(|request| {
        (
            std::cmp::Reverse(request.value),
            std::cmp::Reverse(request.pressure),
            request.key,
        )
    });
    let mut regions: Vec<ProtectionRequest> = Vec::new();
    let mut credited = std::collections::BTreeSet::new();
    let mut routes = RouteProjection::with_public_terrain_and_orientation(
        obs,
        Domain::Ground,
        context.briefing,
        context.orientation,
    );
    for mut request in requests {
        if !regions.iter().any(|prior| {
            prior.key.air == request.key.air && prior.tile.chebyshev(request.tile) <= SERVICE_RADIUS
        }) {
            let mut providers: Vec<_> = obs
                .my_units
                .iter()
                .filter(|unit| {
                    unit.hp > 0
                        && provider(unit.kind, request.key.air)
                        && unit.tile.chebyshev(request.tile) <= SERVICE_RADIUS
                        && !credited.contains(&unit.id)
                        && routes.group_reaches_command_goal(&[unit.id], request.tile)
                        && obs.enemy_units.iter().any(|enemy| {
                            enemy.hp > 0
                                && (enemy.kind.stats().domain == Domain::Air) == request.key.air
                                && enemy.tile.chebyshev(request.tile) <= SERVICE_RADIUS
                                && unit.kind.stats().weapons.iter().any(|weapon| {
                                    weapon.targets.covers(enemy.kind.stats().domain)
                                        && unit.tile.chebyshev(enemy.tile)
                                            <= weapon.range.to_num::<i32>()
                                })
                        })
                })
                .collect();
            providers.sort_by_key(|unit| unit.id);
            for unit in providers {
                credited.insert(unit.id);
                request.missing =
                    request
                        .missing
                        .saturating_sub(strength(unit.kind, unit.hp, request.key.air));
                if request.missing == 0 {
                    break;
                }
            }
            regions.push(request);
        }
    }
    regions
}

impl UtilityPolicy {
    pub(in crate::bot) fn discretionary_protection_work(
        &self,
        now: Tick,
        tuning: DifficultyTuning,
    ) -> Vec<ProtectionRequest> {
        self.support_deployments
            .requests
            .iter()
            .filter(|request| {
                self.support_deployments
                    .evidence
                    .get(&request.key)
                    .is_some_and(|first| now.saturating_sub(*first) >= tuning.reaction_delay)
            })
            .take(tuning.attention_slots)
            .cloned()
            .collect()
    }
    pub(in crate::bot) fn support_reservations(&self) -> Vec<UnitId> {
        let mut units: Vec<_> = self
            .support_work
            .repairs
            .iter()
            .map(|repair| repair.key.worker)
            .chain(
                self.support_deployments
                    .active
                    .iter()
                    .map(|deployment| deployment.unit),
            )
            .collect();
        units.sort_unstable();
        units.dedup();
        units
    }

    pub(in crate::bot) fn observe_support_deployments(
        &mut self,
        context: EconomicInvestmentContext<'_>,
        requests: &[ProtectionRequest],
    ) -> Vec<Intent> {
        let obs = context.obs;
        if self.support_deployments.observed_at == Some(obs.tick) {
            return vec![];
        }
        self.support_deployments.observed_at = Some(obs.tick);
        self.support_deployments.released.clear();
        self.support_deployments.requests = requests.to_vec();
        self.support_deployments
            .evidence
            .retain(|key, _| requests.iter().any(|request| request.key == *key));
        for request in requests {
            self.support_deployments
                .evidence
                .entry(request.key)
                .or_insert(obs.tick);
        }
        let mut intents = Vec::new();
        let mut routes = RouteProjection::with_public_terrain_and_orientation(
            obs,
            Domain::Ground,
            context.briefing,
            context.orientation,
        );
        let old = core::mem::take(&mut self.support_deployments.active);
        for mut deployment in old {
            let Some(unit) = obs
                .my_units
                .iter()
                .find(|unit| unit.id == deployment.unit && unit.hp > 0)
            else {
                self.support_deployments.released.push((
                    deployment,
                    crate::bot::trace::DeploymentReleaseReason::UnitUnavailable,
                ));
                continue;
            };
            let target = asset(obs, deployment.key.asset);
            if requests.iter().any(|request| deployment.covers(request)) {
                deployment.quiet_since = None;
            } else {
                deployment.quiet_since.get_or_insert(obs.tick);
            }
            if target.is_none()
                || obs.tick >= deployment.deadline
                || deployment
                    .quiet_since
                    .is_some_and(|quiet| obs.tick.saturating_sub(quiet) >= QUIET)
                || obs.my_queued_units.contains(&unit.id)
            {
                let reason = if target.is_none() {
                    crate::bot::trace::DeploymentReleaseReason::AssetUnavailable
                } else if obs.my_queued_units.contains(&unit.id) {
                    crate::bot::trace::DeploymentReleaseReason::Preempted
                } else if obs.tick >= deployment.deadline {
                    crate::bot::trace::DeploymentReleaseReason::DeadlineExpired
                } else {
                    crate::bot::trace::DeploymentReleaseReason::PressureCleared
                };
                if !obs.my_queued_units.contains(&unit.id) {
                    intents.push(Intent::StopUnits {
                        units: vec![unit.id],
                    });
                }
                self.support_deployments.released.push((deployment, reason));
                continue;
            }
            let goal = target.expect("live asset was checked").0;
            if goal.chebyshev(deployment.goal) >= 3
                || unit.idle && unit.tile.chebyshev(goal) > SERVICE_RADIUS
            {
                if !routes.group_reaches_command_goal(&[unit.id], goal) {
                    intents.push(Intent::StopUnits {
                        units: vec![unit.id],
                    });
                    self.support_deployments.released.push((
                        deployment,
                        crate::bot::trace::DeploymentReleaseReason::RouteUnavailable,
                    ));
                    continue;
                }
                deployment.goal = goal;
                intents.push(deployment.intent());
            }
            self.support_deployments.active.push(deployment);
        }
        intents
    }

    pub(in crate::bot) fn prepare_support_deployments(
        &self,
        context: EconomicInvestmentContext<'_>,
        tuning: DifficultyTuning,
        minimum_core: u32,
    ) -> Vec<SupportDeployment> {
        let obs = context.obs;
        if !strategic_admission_tick(obs.tick) {
            return vec![];
        }
        let requests = self
            .support_deployments
            .requests
            .iter()
            .filter(|request| {
                request.missing > 0
                    && !self
                        .support_deployments
                        .active
                        .iter()
                        .any(|work| work.covers(request))
                    && self
                        .support_deployments
                        .evidence
                        .get(&request.key)
                        .is_some_and(|first| {
                            obs.tick.saturating_sub(*first) >= tuning.reaction_delay
                        })
            })
            .take(tuning.attention_slots);
        let owned = self.support_reservations();
        let home = obs
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Foundry && building.built)
            .min_by_key(|building| building.id)
            .map(|building| building.anchor);
        let home_strength = obs
            .my_units
            .iter()
            .filter(|unit| {
                !context.unavailable.contains(&unit.id)
                    && home.is_some_and(|home| home.chebyshev(unit.tile) <= 12)
            })
            .map(|unit| super::super::executive::ground_strength(unit.kind, unit.hp))
            .sum::<u64>();
        let home_floor = home_strength.min(
            super::super::executive::full_ground_strength(UnitKind::Sentinel)
                .saturating_mul(u64::from(minimum_core)),
        );
        let mut routes = RouteProjection::with_public_terrain_and_orientation(
            obs,
            Domain::Ground,
            context.briefing,
            context.orientation,
        );
        let mut proposals = Vec::new();
        for request in requests {
            let member = obs
                .my_units
                .iter()
                .filter(|unit| {
                    unit.hp > 0
                        && unit.idle
                        && provider(unit.kind, request.key.air)
                        && !context.unavailable.contains(&unit.id)
                        && !obs.my_queued_units.contains(&unit.id)
                        && !owned.contains(&unit.id)
                        && routes.group_reaches_command_goal(&[unit.id], request.tile)
                })
                .filter(|unit| {
                    let mut excluded = context.unavailable.to_vec();
                    excluded.push(unit.id);
                    let removed = if home.is_some_and(|home| home.chebyshev(unit.tile) <= 12) {
                        super::super::executive::ground_strength(unit.kind, unit.hp)
                    } else {
                        0
                    };
                    home_strength.saturating_sub(removed) >= home_floor
                        && combat_core_status(obs, &excluded, &[], u64::from(minimum_core)).ready
                })
                .min_by_key(|unit| (unit.tile.manhattan(request.tile), unit.id));
            let Some(member) = member else {
                continue;
            };
            let Some(cost) = routes.prospective_group_route_cost(member.tile, request.tile, 1)
            else {
                continue;
            };
            let arrival_at = obs
                .tick
                .saturating_add(super::economic_value::travel_ticks(member.kind, cost));
            if arrival_at >= obs.tick + HORIZON {
                continue;
            }
            proposals.push(SupportDeployment {
                key: request.key,
                unit: member.id,
                accepted_at: obs.tick,
                deadline: obs.tick + HORIZON,
                goal: request.tile,
                arrival_at,
                quiet_since: None,
            });
        }
        proposals
    }

    pub(in crate::bot) fn commit_support_deployment(
        &mut self,
        deployment: SupportDeployment,
        obs: &Observation,
        intents: &mut Vec<Intent>,
    ) -> bool {
        if deployment.accepted_at != obs.tick
            || deployment.deadline <= obs.tick
            || self.support_reservations().contains(&deployment.unit)
            || self
                .support_deployments
                .active
                .iter()
                .any(|work| work.key == deployment.key)
            || asset(obs, deployment.key.asset).is_none()
            || !obs.my_units.iter().any(|unit| {
                unit.id == deployment.unit
                    && unit.hp > 0
                    && unit.idle
                    && provider(unit.kind, deployment.key.air)
                    && !obs.my_queued_units.contains(&unit.id)
            })
        {
            return false;
        }
        intents.push(deployment.intent());
        self.support_deployments.active.push(deployment);
        self.support_deployments.active.sort_by_key(|work| work.key);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::{BotConfig, BotDifficulty, BotStance};

    fn fixture() -> (Observation, PublicMapBriefing, ResolvedProfile) {
        let scenario = crate::Scenario::skirmish();
        let mut obs = Observation::omniscient(&scenario.build().unwrap(), PlayerId(0));
        obs.tick = 120;
        obs.map_width = 48;
        obs.map_height = 32;
        obs.visible = vec![true; 48 * 32];
        obs.explored = obs.visible.clone();
        obs.known_rock.clear();
        obs.known_peaks.clear();
        obs.known_scrap.clear();
        obs.enemy_buildings.clear();
        obs.ally_buildings.clear();
        obs.enemy_units.clear();
        obs.my_buildings.truncate(1);
        obs.my_buildings[0].anchor = TilePos::new(2, 12);
        obs.my_queues = vec![vec![]];
        obs.my_queue_progress = vec![0];
        let template = obs.my_units[0].clone();
        let unit = |id, player, kind: UnitKind, tile| UnitObs {
            id: UnitId(id),
            player: PlayerId(player),
            kind,
            tile,
            hp: kind.stats().max_hp,
            idle: true,
            ..template.clone()
        };
        obs.my_units = (0..8)
            .map(|index| {
                unit(
                    index + 10,
                    0,
                    UnitKind::Sentinel,
                    TilePos::new(7, 9 + index as i32),
                )
            })
            .collect();
        obs.my_units.extend([
            unit(50, 0, UnitKind::Bombard, TilePos::new(30, 10)),
            unit(51, 0, UnitKind::Harvester, TilePos::new(30, 25)),
            unit(100, 0, UnitKind::Flakhound, TilePos::new(10, 10)),
            unit(101, 0, UnitKind::Sentinel, TilePos::new(16, 24)),
        ]);
        obs.enemy_units = vec![
            unit(200, 1, UnitKind::Buzzard, TilePos::new(33, 10)),
            unit(201, 1, UnitKind::Sentinel, TilePos::new(33, 25)),
        ];
        let map = PublicMapBriefing {
            map_width: 48,
            map_height: 32,
            starting_foundries: vec![],
            teams: vec![None, None],
            non_ground_terrain: vec![],
            extractor_frames: vec![],
            initial_scrap: vec![],
        };
        (
            obs,
            map,
            BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 7).resolve_profile(),
        )
    }

    fn context<'a>(
        obs: &'a Observation,
        map: &'a PublicMapBriefing,
        profile: &'a ResolvedProfile,
        resources: &'a ResourceSnapshot,
    ) -> EconomicInvestmentContext<'a> {
        EconomicInvestmentContext {
            obs,
            resources,
            profile,
            briefing: map,
            orientation: super::super::super::orient::Orientation::for_home(
                obs,
                TilePos::new(2, 12),
            ),
            unavailable: &[],
            demands: &[],
            unit_contacts: &[],
            building_contacts: &[],
            cadence: 12,
            protected_scrap: 0,
            air_work: &[],
        }
    }

    #[test]
    fn disjoint_protection_requests_keep_exact_members_and_home_strength() {
        let (obs, map, mut profile) = fixture();
        profile.traits.support = 0;
        let resources = ResourceSnapshot::from_observation(&obs);
        let context = context(&obs, &map, &profile, &resources);
        let mut policy = UtilityPolicy::new();
        let snapshot = policy.support_work_snapshot(context);
        policy.observe_support_deployments(context, &snapshot.protection);
        let quoted = policy.prepare_support_deployments(
            context,
            DifficultyTuning::for_level(profile.difficulty),
            8,
        );
        assert_eq!(quoted.len(), 2);
        assert!(
            quoted
                .iter()
                .any(|work| work.key.air && work.unit == UnitId(100))
        );
        assert!(
            quoted
                .iter()
                .any(|work| !work.key.air && work.unit == UnitId(101))
        );
        for work in quoted {
            assert!(policy.commit_support_deployment(work.clone(), &obs, &mut vec![]));
            assert!(!policy.commit_support_deployment(work, &obs, &mut vec![]));
        }
        assert_eq!(
            policy.support_reservations(),
            vec![UnitId(100), UnitId(101)]
        );
        assert!(
            policy
                .prepare_support_deployments(
                    context,
                    DifficultyTuning::for_level(profile.difficulty),
                    8
                )
                .is_empty()
        );
    }

    #[test]
    fn zero_attention_services_retained_deployments_once_and_death_releases_only_one() {
        let (mut obs, map, profile) = fixture();
        let mut policy = UtilityPolicy::new();
        let resources = ResourceSnapshot::from_observation(&obs);
        let initial = context(&obs, &map, &profile, &resources);
        let snapshot = policy.support_work_snapshot(initial);
        policy.observe_support_deployments(initial, &snapshot.protection);
        let quoted = policy.prepare_support_deployments(
            initial,
            DifficultyTuning::for_level(profile.difficulty),
            8,
        );
        for work in quoted {
            assert!(policy.commit_support_deployment(work, &obs, &mut vec![]));
        }
        let deadline = policy.support_deployments.active[0].deadline;
        obs.tick += 24;
        obs.my_units.retain(|unit| unit.id != UnitId(101));
        obs.my_units
            .iter_mut()
            .find(|unit| unit.id == UnitId(50))
            .unwrap()
            .tile
            .x += 4;
        let resources = ResourceSnapshot::from_observation(&obs);
        let current = context(&obs, &map, &profile, &resources);
        let snapshot = policy.support_work_snapshot(current);
        assert_eq!(
            policy
                .observe_support_deployments(current, &snapshot.protection)
                .len(),
            1
        );
        assert!(
            policy
                .observe_support_deployments(current, &snapshot.protection)
                .is_empty()
        );
        assert_eq!(policy.support_reservations(), vec![UnitId(100)]);
        assert_eq!(policy.support_deployments.active[0].deadline, deadline);
        let mut tuning = DifficultyTuning::for_level(profile.difficulty);
        tuning.attention_slots = 0;
        assert!(
            policy
                .prepare_support_deployments(current, tuning, 8)
                .is_empty()
        );
    }
}
