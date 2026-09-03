//! Economy and production decision channels.

#[cfg(test)]
use super::construction::FOUNDRY_RECOVERY_TICKS;
use super::construction::{FoundryCommitmentOutcome, FoundrySavingCommitment, commit_foundry_plan};
use super::*;

#[derive(Clone, Copy)]
struct ProductionGuards {
    voluntary: u32,
    capital: u32,
}

/// Production capacity required to finish one active strategic air plan in
/// two minutes. Capacity is ordinary infrastructure: it still costs scrap,
/// takes builder time, and obeys the shared placement and prerequisite rules.
const AIRWORKS_ASSEMBLY_HORIZON_TICKS: u64 = 2_400;

impl UtilityPolicy {
    /// Economy channel: idle harvesters back to work on the nearest
    /// known node that hasn't bounced anyone. A node only qualifies if
    /// it sits no deeper in their half than ours — a returning scout
    /// must not be "efficiently" assigned to mine at the enemy's
    /// doorstep.
    pub(super) fn economy(
        &mut self,
        obs: &Observation,
        home: TilePos,
        player_facing: bool,
        unit_contacts: Option<&[UnitContact]>,
        building_contacts: Option<&[BuildingContact]>,
        intents: &mut Vec<Intent>,
    ) {
        let enemy_base = obs
            .enemy_buildings
            .iter()
            .filter(|building| !player_facing || building.kind == BuildingKind::Foundry)
            .map(|b| (b.anchor.manhattan(home), b.anchor.y, b.anchor.x))
            .min()
            .map(|(_, y, x)| TilePos::new(x, y));
        let has_eligible_worker = obs.my_units.iter().any(|unit| {
            unit.kind.stats().harvest.is_some()
                && unit.idle
                && Some(unit.id) != self.scout
                && !self.evacuating_workers.contains(&unit.id)
        });
        let danger = (player_facing && has_eligible_worker)
            .then(|| self.harvest_danger_projection(obs, unit_contacts, building_contacts));
        let mut routes = danger.as_ref().map(|danger| {
            crate::bot::routing::RouteProjection::ground_avoiding(obs, |tile| {
                self.harvest_location_contested(tile) || danger.contains(tile)
            })
        });
        for u in obs.my_units.iter().filter(|u| {
            u.kind.stats().harvest.is_some()
                && u.idle
                && Some(u.id) != self.scout
                && !self.evacuating_workers.contains(&u.id)
        }) {
            if routes
                .as_mut()
                .is_some_and(|routes| !Self::harvester_reaches_drop_off(obs, u, routes))
            {
                continue;
            }
            let mut candidates: Vec<_> = obs
                .known_scrap
                .iter()
                .chain(obs.known_wrecks.iter())
                .filter(|(pos, amount)| {
                    *amount > 0
                        && !self.dead_nodes.contains(pos)
                        && (!player_facing
                            || (!Self::source_in_salvage_incident(obs, *pos)
                                && !self.harvest_location_contested(*pos)
                                && !danger
                                    .as_ref()
                                    .expect("player-facing economy prepared worker danger")
                                    .contains(*pos)))
                        && enemy_base.is_none_or(|eb| pos.manhattan(home) <= pos.manhattan(eb))
                })
                .map(|(pos, _)| (pos.manhattan(u.tile), pos.y, pos.x))
                .collect();
            candidates.sort_unstable();
            let node = candidates.into_iter().find_map(|(_, y, x)| {
                let source = TilePos::new(x, y);
                routes
                    .as_mut()
                    .is_none_or(|routes| Self::harvester_reaches_source(obs, u, source, routes))
                    .then_some(source)
            });
            if let Some(node) = node {
                intents.push(Intent::AssignHarvest { unit: u.id, node });
                // The profile-free Overseer audits its historical intent-time
                // record. Player-facing play records only a Harvest command
                // that survives lowering, after higher-priority channels have
                // had the chance to claim this worker.
                if !player_facing {
                    self.last_sent.push((u.id, node, u.tile));
                }
            }
        }
    }

    pub(super) fn source_in_salvage_incident(obs: &Observation, source: TilePos) -> bool {
        obs.salvage_incidents.iter().any(|incident| {
            incident.chebyshev(source) <= crate::stats::HARVEST_INCIDENT_DANGER_RADIUS
        })
    }

    fn harvester_reaches_drop_off(
        obs: &Observation,
        harvester: &UnitObs,
        routes: &mut crate::bot::routing::RouteProjection<'_>,
    ) -> bool {
        obs.my_buildings
            .iter()
            .filter(|building| building.built && building.kind.is_drop_off())
            .any(|building| {
                let (width, height) = building.kind.tier_stats(building.tier).size;
                (-1..=height).any(|dy| {
                    (-1..=width).any(|dx| {
                        let inside = dx >= 0 && dx < width && dy >= 0 && dy < height;
                        !inside
                            && routes.direct_line_avoids_blocked(
                                harvester.tile,
                                building.anchor.offset(dx, dy),
                            )
                            && routes.reaches(harvester.tile, building.anchor.offset(dx, dy))
                            && routes.command_path_avoids_blocked(
                                harvester.tile,
                                building.anchor.offset(dx, dy),
                            )
                    })
                })
            })
    }

    fn harvester_reaches_source(
        obs: &Observation,
        harvester: &UnitObs,
        source: TilePos,
        routes: &mut crate::bot::routing::RouteProjection<'_>,
    ) -> bool {
        if !obs.known_scrap_at(source) {
            return routes.direct_line_avoids_blocked(harvester.tile, source)
                && routes.reaches(harvester.tile, source)
                && Self::harvest_work_tile_reaches_drop_off(obs, source, routes)
                && routes.command_path_avoids_blocked(harvester.tile, source);
        }
        (-1..=1).any(|dy| {
            (-1..=1).any(|dx| {
                if dx == 0 && dy == 0 {
                    return false;
                }
                let work_tile = source.offset(dx, dy);
                routes.direct_line_avoids_blocked(harvester.tile, work_tile)
                    && routes.reaches(harvester.tile, work_tile)
                    && Self::harvest_work_tile_reaches_drop_off(obs, work_tile, routes)
                    && routes.command_path_avoids_blocked(harvester.tile, work_tile)
            })
        })
    }

    fn harvest_work_tile_reaches_drop_off(
        obs: &Observation,
        work_tile: TilePos,
        routes: &mut crate::bot::routing::RouteProjection<'_>,
    ) -> bool {
        obs.my_buildings
            .iter()
            .filter(|building| building.built && building.kind.is_drop_off())
            .any(|building| {
                let (width, height) = building.kind.tier_stats(building.tier).size;
                (-1..=height).any(|dy| {
                    (-1..=width).any(|dx| {
                        let inside = dx >= 0 && dx < width && dy >= 0 && dy < height;
                        !inside && routes.reaches(work_tile, building.anchor.offset(dx, dy))
                    })
                })
            })
    }

    /// Production channel: harvesters to target, then a sentinel drip
    /// from the Foundry; counters and lancers from the Fabricator. The
    /// unbounded drip arms respect [`Self::capital_reserve`] so the tech
    /// fund can accumulate instead of being consumed by each think.
    #[cfg(test)]
    pub(super) fn production(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        home: TilePos,
        claims: ConstructionClaims<'_>,
        budget: &mut u32,
        intents: &mut Vec<Intent>,
    ) -> bool {
        self.production_with_air_demand(
            dials,
            obs,
            ProductionContext::new(home, claims, None),
            budget,
            intents,
        )
    }

    pub(super) fn opening_core_production(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        context: ProductionContext<'_>,
        budget: &mut u32,
        intents: &mut Vec<Intent>,
    ) -> CombatCoreStatus {
        let ProductionContext {
            home,
            claims,
            combat_core_exclusions,
            unit_contacts,
            building_contacts,
            public_map,
            producer_lane_reservations,
            ..
        } = context;
        let capital_context = ConstructionContext::new(home, claims)
            .with_combat_core_exclusions(combat_core_exclusions)
            .with_intelligence(unit_contacts, building_contacts)
            .with_public_map(public_map);
        let home_extractor_reserve =
            self.opening_home_extractor_reserve(dials, obs, capital_context, intents);

        let queued_harvesters = obs
            .my_queues
            .iter()
            .flatten()
            .filter(|kind| **kind == UnitKind::Harvester)
            .count();
        let planned_harvesters = intents
            .iter()
            .filter(|intent| {
                matches!(
                    intent,
                    Intent::TrainAt {
                        kind: UnitKind::Harvester,
                        ..
                    }
                )
            })
            .count();
        let harvesters = obs
            .my_units
            .iter()
            .filter(|unit| unit.kind == UnitKind::Harvester)
            .count()
            .saturating_add(queued_harvesters)
            .saturating_add(planned_harvesters);
        if harvesters < immediate_harvester_target(dials) as usize {
            let harvester_cost = UnitKind::Harvester.stats().cost;
            let mut foundries: Vec<_> = obs
                .my_buildings
                .iter()
                .enumerate()
                .filter(|(_, building)| {
                    building.built
                        && building.kind == BuildingKind::Foundry
                        && producer_lane_reservations.allows_immediate_append(
                            building.id,
                            &super::production::planned_kinds_at(intents, building.id),
                            UnitKind::Harvester,
                        )
                })
                .map(|(queue_index, building)| {
                    (
                        building.id,
                        obs.my_queues
                            .get(queue_index)
                            .map_or(2, Vec::len)
                            .saturating_add(super::production::planned_at(intents, building.id)),
                    )
                })
                .collect();
            foundries.sort_unstable_by_key(|(building, _)| *building);
            if let Some((building, _)) = foundries.into_iter().find(|(_, depth)| *depth < 2)
                && *budget >= harvester_cost.saturating_add(home_extractor_reserve)
            {
                *budget -= harvester_cost;
                intents.push(Intent::TrainAt {
                    building,
                    kind: UnitKind::Harvester,
                });
            }
        }

        super::production::fill_combat_core(
            obs,
            combat_core_exclusions,
            u64::from(dials.minimum_core_equivalents),
            home_extractor_reserve,
            producer_lane_reservations,
            budget,
            intents,
        )
    }

    pub(super) fn opening_bootstrap_reserve(
        &self,
        dials: &Dials,
        obs: &Observation,
        context: ConstructionContext<'_>,
        intents: &[Intent],
    ) -> u32 {
        let harvesters = obs
            .my_units
            .iter()
            .filter(|unit| unit.kind == UnitKind::Harvester)
            .count()
            + obs
                .my_queues
                .iter()
                .flatten()
                .filter(|kind| **kind == UnitKind::Harvester)
                .count()
            + intents
                .iter()
                .filter(|intent| {
                    matches!(
                        intent,
                        Intent::TrainAt {
                            kind: UnitKind::Harvester,
                            ..
                        }
                    )
                })
                .count();
        let harvester_reserve = if harvesters < immediate_harvester_target(dials) as usize {
            UnitKind::Harvester.stats().cost
        } else {
            0
        };
        harvester_reserve
            .saturating_add(self.opening_home_extractor_reserve(dials, obs, context, intents))
    }

    fn opening_home_extractor_reserve(
        &self,
        dials: &Dials,
        obs: &Observation,
        context: ConstructionContext<'_>,
        intents: &[Intent],
    ) -> u32 {
        let extractor_planned = intents.iter().any(|intent| {
            matches!(
                intent,
                Intent::Build {
                    kind: BuildingKind::Extractor,
                    ..
                } | Intent::BuildWith {
                    kind: BuildingKind::Extractor,
                    ..
                }
            )
        });
        if !extractor_planned
            && dials.extractors
            && self
                .starting_home_frame_restoration_claim(obs, context)
                .is_some()
        {
            BuildingKind::Extractor
                .base_stats()
                .construction
                .map_or(0, |construction| construction.cost)
        } else {
            0
        }
    }

    /// The lowest-id built producer of `kind` with its queue index — the
    /// canonical choice every purchase stage shares, so no stage can
    /// restate the id tie-break slightly differently.
    fn open_producer<'a>(
        obs: &'a Observation,
        producer_kind: BuildingKind,
        unit_kind: UnitKind,
        depth_limit: usize,
        producer_lane_reservations: &ProducerLaneReservations,
        intents: &[Intent],
    ) -> Option<(usize, &'a BuildingObs)> {
        obs.my_buildings
            .iter()
            .enumerate()
            .filter(|(index, building)| {
                building.kind == producer_kind
                    && building.built
                    && obs.my_queues.get(*index).is_some_and(|queue| {
                        queue
                            .len()
                            .saturating_add(super::production::planned_at(intents, building.id))
                            < depth_limit
                    })
                    && producer_lane_reservations.allows_immediate_append(
                        building.id,
                        &super::production::planned_kinds_at(intents, building.id),
                        unit_kind,
                    )
            })
            .min_by_key(|(_, building)| building.id)
    }

    /// The Airworks capacity purchase: buys the held extra Airworks when
    /// the site is actionable, and reports either the capital the still
    /// unbought site keeps reserved, or that this think's voluntary
    /// spending ends with the purchase (the exact-guard early return).
    fn airworks_capacity_stage(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        context: ProductionContext<'_>,
        budget: &mut u32,
        intents: &mut Vec<Intent>,
    ) -> std::ops::ControlFlow<(), Option<u32>> {
        let ProductionContext {
            home,
            claims,
            outstanding_air_production_ticks,
            unit_contacts,
            building_contacts,
            voluntary_scrap_guard,
            ..
        } = context;
        let ConstructionClaims {
            enlisted, reserved, ..
        } = claims;
        let airworks_cost = BuildingKind::Airworks
            .base_stats()
            .construction
            .map_or(0, |construction| construction.cost);
        let capacity_guard = voluntary_scrap_guard.amount(TECH_RESERVE);
        let mut capacity_site =
            self.airworks_capacity_site(dials, obs, home, claims, outstanding_air_production_ticks);
        let capacity_active = capacity_site.is_some();
        if let Some(anchor) = capacity_site
            && *budget >= airworks_cost.saturating_add(capacity_guard)
        {
            let mut unavailable = Vec::new();
            for intent in intents.iter() {
                Self::claim_non_preemptible_intent_units(intent, &mut unavailable);
            }
            let mut builders: Vec<_> = self
                .construction_builders(obs, enlisted, reserved)
                .into_iter()
                .filter(|builder| !unavailable.contains(&builder.id))
                .collect();
            let accepted_builds: Vec<_> = intents
                .iter()
                .filter_map(|intent| match intent {
                    Intent::Build { kind, anchor } | Intent::BuildWith { kind, anchor, .. } => {
                        Some((*kind, *anchor))
                    }
                    _ => None,
                })
                .collect();
            self.prepare_ground_producer_egress(obs);
            let preserves_egress = self.preserves_ground_producer_egress_prepared(
                &accepted_builds,
                (BuildingKind::Airworks, anchor),
            );
            let danger = preserves_egress
                .then(|| self.harvest_danger_projection(obs, unit_contacts, building_contacts));
            if preserves_egress
                && let Some(builder) = self.safe_implicit_builder(
                    obs,
                    BuildingKind::Airworks,
                    anchor,
                    &mut builders,
                    danger
                        .as_deref()
                        .expect("an actionable capacity site prepared worker danger"),
                    None,
                )
            {
                *budget -= airworks_cost;
                Self::insert_build_before_harvest(
                    intents,
                    BuildingKind::Airworks,
                    anchor,
                    Intent::BuildWith {
                        builder,
                        kind: BuildingKind::Airworks,
                        anchor,
                    },
                );
                capacity_site = None;
                if capacity_guard > 0 && voluntary_scrap_guard.is_exact() {
                    return std::ops::ControlFlow::Break(());
                }
            }
        }
        std::ops::ControlFlow::Continue(capacity_active.then(|| {
            if capacity_site.is_some() {
                airworks_cost.saturating_add(capacity_guard)
            } else {
                0
            }
        }))
    }

    fn alive_count(obs: &Observation, kind: UnitKind) -> usize {
        obs.my_units.iter().filter(|u| u.kind == kind).count()
    }

    fn queued_count(obs: &Observation, kind: UnitKind) -> usize {
        obs.my_queues
            .iter()
            .flat_map(|q| q.iter())
            .filter(|k| **k == kind)
            .count()
    }

    /// One Warden per think from a standing Fabricator, and one Breaker
    /// whenever the Crucible is idle and the bank can take it. Deep-tech
    /// production runs before the basic military drip.
    fn deep_tech_drip(
        dials: &Dials,
        obs: &Observation,
        producer_lane_reservations: &ProducerLaneReservations,
        voluntary_guard: u32,
        budget: &mut u32,
        intents: &mut Vec<Intent>,
    ) {
        if !dials.deep_tech || dials.adaptive_composition {
            return;
        }
        let alive = |kind| Self::alive_count(obs, kind);
        let queued = |kind| Self::queued_count(obs, kind);
        let crucible = Self::open_producer(
            obs,
            BuildingKind::Crucible,
            UnitKind::Breaker,
            1,
            producer_lane_reservations,
            intents,
        );
        if let Some((_, crucible)) = crucible
            && alive(UnitKind::Breaker) + queued(UnitKind::Breaker) < 2
            && *budget
                >= UnitKind::Breaker
                    .stats()
                    .cost
                    .saturating_add(TECH_RESERVE)
                    .saturating_add(voluntary_guard)
        {
            *budget -= UnitKind::Breaker.stats().cost;
            intents.push(Intent::TrainAt {
                building: crucible.id,
                kind: UnitKind::Breaker,
            });
        }
        let fabricator = Self::open_producer(
            obs,
            BuildingKind::Fabricator,
            UnitKind::Warden,
            SHALLOW_QUEUE_DEPTH,
            producer_lane_reservations,
            intents,
        );
        if let Some((_, fabricator)) = fabricator
            && alive(UnitKind::Warden) + queued(UnitKind::Warden) < 4
            && *budget
                >= UnitKind::Warden
                    .stats()
                    .cost
                    .saturating_add(UnitKind::Harvester.stats().cost)
                    .saturating_add(voluntary_guard)
        {
            *budget -= UnitKind::Warden.stats().cost;
            intents.push(Intent::TrainAt {
                building: fabricator.id,
                kind: UnitKind::Warden,
            });
        }
        // Once the whole tree stands, a small bomber wing: the payload
        // that decides sieges — and island wars, where no crawler ever
        // crosses.
        use crate::stats::Role;
        let bomber_kind = Role::Bomber.unit_for(obs.faction);
        let airworks = Self::open_producer(
            obs,
            BuildingKind::Airworks,
            bomber_kind,
            SHALLOW_QUEUE_DEPTH,
            producer_lane_reservations,
            intents,
        );
        let crucible_stands = obs
            .my_buildings
            .iter()
            .any(|b| b.kind == BuildingKind::Crucible && b.built);
        if let Some((_, airworks)) = airworks
            && crucible_stands
            && alive(bomber_kind) + queued(bomber_kind) < 2
            && *budget
                >= bomber_kind
                    .stats()
                    .cost
                    .saturating_add(TECH_RESERVE)
                    .saturating_add(voluntary_guard)
        {
            *budget -= bomber_kind.stats().cost;
            intents.push(Intent::TrainAt {
                building: airworks.id,
                kind: bomber_kind,
            });
        }
    }

    /// The Fabricator-era priority ladder: anti-air first, turret
    /// breakers, then the closed tree's raiders, harass wing, and
    /// repeatable Lancer drip.
    fn fabricator_drip(
        &self,
        dials: &Dials,
        obs: &Observation,
        producer_lane_reservations: &ProducerLaneReservations,
        guards: ProductionGuards,
        budget: &mut u32,
        intents: &mut Vec<Intent>,
    ) {
        let ProductionGuards {
            voluntary: voluntary_guard,
            capital,
        } = guards;
        let alive = |kind| Self::alive_count(obs, kind);
        let queued = |kind| Self::queued_count(obs, kind);
        let enemy_turrets = obs
            .enemy_buildings
            .iter()
            .filter(|b| b.kind == BuildingKind::Turret && b.built)
            .count();
        let enemy_harvesters = obs
            .enemy_units
            .iter()
            .filter(|u| u.kind.stats().harvest.is_some())
            .count();
        use crate::stats::Role;
        let aa_kind = Role::AntiAir.unit_for(obs.faction);
        let wing_kind = Role::AirGround.unit_for(obs.faction);
        let open = |producer_kind, unit_kind| {
            Self::open_producer(
                obs,
                producer_kind,
                unit_kind,
                SHALLOW_QUEUE_DEPTH,
                producer_lane_reservations,
                intents,
            )
        };
        let lancer = UnitKind::Lancer.stats().cost;
        let scuttler = UnitKind::Scuttler.stats().cost;
        let reserve = UnitKind::Sentinel.stats().cost;
        // The sky answers first: enemy air on the field (or ever
        // sighted) wants a dedicated gun per two known wings, before
        // any ground purchase.
        let enemy_air = obs
            .enemy_units
            .iter()
            .filter(|unit| super::is_air_threat(unit))
            .count();
        let want_aa = if enemy_air > 0 {
            enemy_air.div_ceil(2) + 1
        } else {
            usize::from(self.seen_air)
        };
        if dials.aa_response
            && alive(aa_kind) + queued(aa_kind) < want_aa
            && *budget >= aa_kind.stats().cost.saturating_add(voluntary_guard)
            && let Some((_, fabricator)) = open(BuildingKind::Fabricator, aa_kind)
        {
            *budget -= aa_kind.stats().cost;
            intents.push(Intent::TrainAt {
                building: fabricator.id,
                kind: aa_kind,
            });
        } else if enemy_turrets > alive(UnitKind::Lancer) + queued(UnitKind::Lancer)
            && *budget >= lancer.saturating_add(voluntary_guard)
            && let Some((_, fabricator)) = open(BuildingKind::Fabricator, UnitKind::Lancer)
        {
            *budget -= lancer;
            intents.push(Intent::TrainAt {
                building: fabricator.id,
                kind: UnitKind::Lancer,
            });
        } else if !dials.adaptive_composition
            && alive(UnitKind::Scuttler) < 4
            && enemy_harvesters >= 2
            && *budget >= scuttler + reserve
            && let Some((_, raid_bay)) = open(BuildingKind::Foundry, UnitKind::Scuttler)
        {
            // The Scuttler homes at the Foundry on the closed tree.
            *budget -= scuttler;
            intents.push(Intent::TrainAt {
                building: raid_bay.id,
                kind: UnitKind::Scuttler,
            });
        } else if !dials.adaptive_composition
            && dials.air_harass
            && alive(wing_kind) + queued(wing_kind) < AIR_WING
            && (enemy_harvesters >= 2 || !obs.enemy_buildings.is_empty())
            && *budget >= wing_kind.stats().cost + reserve
            && let Some((_, airworks)) = open(BuildingKind::Airworks, wing_kind)
        {
            // A wing for the harvest line — bought once raiding has
            // something to eat OR the enemy base is known at all
            // (on an island map the wing IS the reach), and only
            // from a standing Airworks.
            *budget -= wing_kind.stats().cost;
            intents.push(Intent::TrainAt {
                building: airworks.id,
                kind: wing_kind,
            });
        } else if !dials.adaptive_composition
            && *budget >= lancer + reserve + capital
            && let Some((_, fabricator)) = open(BuildingKind::Fabricator, UnitKind::Lancer)
        {
            *budget -= lancer;
            intents.push(Intent::TrainAt {
                building: fabricator.id,
                kind: UnitKind::Lancer,
            });
        }
    }

    #[cfg(test)]
    pub(super) fn production_with_air_demand(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        context: ProductionContext<'_>,
        budget: &mut u32,
        intents: &mut Vec<Intent>,
    ) -> bool {
        self.production_with_commitments(dials, obs, context, budget, None, intents)
    }

    pub(super) fn production_with_commitments(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        context: ProductionContext<'_>,
        budget: &mut u32,
        mut commitments: Option<&mut PolicyCommitments>,
        intents: &mut Vec<Intent>,
    ) -> bool {
        let ProductionContext {
            home,
            claims,
            combat_core_exclusions,
            outstanding_air_production_ticks,
            unit_contacts,
            building_contacts,
            public_map,
            voluntary_scrap_guard,
            fresh_foundry_admission,
            producer_lane_reservations,
        } = context;
        let ConstructionClaims { player_facing, .. } = claims;
        let queued = |kind| Self::queued_count(obs, kind);
        let alive = |kind| Self::alive_count(obs, kind);
        let harvesters = alive(UnitKind::Harvester) + queued(UnitKind::Harvester);
        // Survival outranks saving: with the home screen thin, the drip
        // spends freely — a banked Fabricator is worthless underneath a
        // Sentinel rush.
        let screen = obs
            .my_units
            .iter()
            .filter(|u| {
                let stats = u.kind.stats();
                stats.domain == Domain::Ground && stats.can_fight()
            })
            .count();
        // A desperate economy with a road to march releases the capital
        // fund: saving for the next tech rung is saving for a purchase
        // no income will ever complete, while the freed bank buys the
        // bodies that end the game now. Island desperation keeps the
        // fund — with no ground road, the tech chain to the sky is the
        // only road left, and spending its savings on infantry is how
        // forty-seven fighters end up staring at a gulf forever.
        let mut unavailable_builders = Vec::new();
        for intent in intents.iter() {
            Self::claim_non_preemptible_intent_units(intent, &mut unavailable_builders);
        }
        let capital_context = ConstructionContext::new(home, claims)
            .with_combat_core_exclusions(combat_core_exclusions)
            .with_intelligence(unit_contacts, building_contacts)
            .with_public_map(public_map)
            .excluding_builders(&unavailable_builders);

        // Finish capital projects that were already admitted before letting a
        // new expansion claim the bank. A completed capacity purchase still
        // owns this think's construction channel, while a partial one retains
        // its fund. Actionable Extractor restoration keeps the same precedence
        // it has in construction.
        let capacity_stage =
            match self.airworks_capacity_stage(dials, obs, context, budget, intents) {
                std::ops::ControlFlow::Break(()) => return false,
                std::ops::ControlFlow::Continue(stage) => stage,
            };
        let capacity_capital = capacity_stage.unwrap_or(0);
        if commitments
            .as_ref()
            .is_some_and(|commitments| commitments.foundry_saving_blocked())
        {
            self.retain_blocked_foundry_saving(obs.tick);
            if self.foundry_saving.is_none() {
                self.release_foundry_saving(commitments.as_deref_mut(), budget);
            }
            return true;
        }
        if fresh_foundry_admission == FreshFoundryAdmission::Adjudicated
            && self
                .foundry_saving
                .as_ref()
                .is_some_and(|saving| saving.accepted_at == obs.tick)
        {
            // Cross-domain allocation already froze this exact plan using its
            // bounded forecast. Current-only continuation cannot immediately
            // reprice and release it in the same observation.
            return true;
        }
        if self.foundry_saving.as_ref().is_some_and(|saving| {
            intents.iter().any(|intent| {
                matches!(
                    intent,
                    Intent::BuildWith {
                        builder,
                        kind: BuildingKind::Foundry,
                        anchor,
                    } if *builder == saving.plan.builder && *anchor == saving.plan.anchor
                )
            })
        }) {
            return true;
        }
        let extractor_restoration_precedes_expansion = dials.extractors
            && self
                .supported_frame_restoration_claim(obs, capital_context)
                .is_some();
        let saved_foundry = self.foundry_saving.clone();
        let prior_capital_project = saved_foundry.is_none()
            && (capacity_stage.is_some() || extractor_restoration_precedes_expansion);

        let expansion_inputs = if player_facing && dials.expansion && !prior_capital_project {
            let (foundries, pending_foundries) = Self::projected_foundries(obs);
            let builders: Vec<_> = if let Some(saving) = &saved_foundry {
                obs.my_units
                    .iter()
                    .filter(|builder| builder.id == saving.plan.builder)
                    .filter(|builder| builder_is_free(obs, builder))
                    .filter(|builder| !claims.enlisted.contains(&builder.id))
                    .filter(|builder| !claims.reserved.contains(&builder.id))
                    .filter(|builder| !unavailable_builders.contains(&builder.id))
                    .filter(|builder| self.scout != Some(builder.id))
                    .collect()
            } else {
                self.construction_builders(obs, claims.enlisted, claims.reserved)
                    .into_iter()
                    .filter(|builder| builder_is_free(obs, builder))
                    .filter(|builder| !unavailable_builders.contains(&builder.id))
                    .collect()
            };
            (pending_foundries == 0).then_some((foundries, builders))
        } else {
            None
        };
        let current_expansion_scrap = if saved_foundry.is_some() {
            commitments.as_ref().map_or(*budget, |commitments| {
                commitments.available_for_foundry_saving()
            })
        } else {
            *budget
        };
        let expansion_resources = ResourceSnapshot::from_observation(obs);
        let expansion_quote_basis = saved_foundry.as_ref().map_or_else(
            || Some((current_expansion_scrap, voluntary_scrap_guard)),
            |saving| {
                if saving.forecast_basis.is_none() {
                    return Some((current_expansion_scrap, voluntary_scrap_guard));
                }
                let funding = Self::foundry_funding_revalidation(
                    &expansion_resources,
                    saving,
                    current_expansion_scrap,
                );
                funding.viable.then_some((
                    funding.planning_scrap,
                    Reserve::Exact(funding.protected_reserve),
                ))
            },
        );
        let expansion_context = expansion_inputs
            .as_ref()
            .zip(expansion_quote_basis)
            .and_then(|((foundries, builders), (spendable_scrap, quote_guard))| {
                public_map.map(|public_map| FoundryAssessmentContext {
                    claim: FoundryClaimContext {
                        home,
                        projected_foundries: foundries,
                        builders,
                        support_extractors: obs.my_buildings.iter().any(|building| {
                            building.kind == BuildingKind::Fabricator && building.built
                        }),
                        ordinary_frontiers: !dials.deep_tech
                            || Self::projected_count(obs, BuildingKind::Airworks, player_facing)
                                > 0,
                        unit_contacts,
                        building_contacts,
                    },
                    public_map,
                    combat_core_exclusions,
                    spendable_scrap,
                    voluntary_scrap_guard: quote_guard,
                    required_anchor: saved_foundry.as_ref().map(|saving| saving.plan.anchor),
                })
            });
        let admit_assessment = |_: &expansion::FoundryExpansionAssessment| {
            saved_foundry.is_some() || fresh_foundry_admission == FreshFoundryAdmission::Legacy
        };
        let expansion_assessment = expansion_context
            .and_then(|context| self.player_facing_foundry_assessment(dials, obs, context, intents))
            .filter(admit_assessment);

        let expansion_assessment = match expansion_assessment {
            Some(assessment)
                if matches!(
                    assessment.disposition,
                    expansion::ExpansionDisposition::Prepare { .. }
                ) =>
            {
                if saved_foundry.is_some() {
                    self.release_foundry_saving(commitments.as_deref_mut(), budget);
                }
                let selected_anchor = assessment.plan.anchor;
                let guard = voluntary_scrap_guard.amount(0);
                let status = super::production::fill_combat_core_to_strength(
                    obs,
                    combat_core_exclusions,
                    assessment.preparation_target_strength,
                    guard,
                    producer_lane_reservations,
                    budget,
                    intents,
                );
                if !status.ready {
                    return true;
                }

                let reassessed = expansion_context
                    .and_then(|context| {
                        self.player_facing_foundry_assessment(
                            dials,
                            obs,
                            FoundryAssessmentContext {
                                spendable_scrap: *budget,
                                required_anchor: Some(selected_anchor),
                                ..context
                            },
                            intents,
                        )
                    })
                    .filter(admit_assessment);
                let Some(reassessed) = reassessed else {
                    return true;
                };
                Some(reassessed)
            }
            assessment => assessment,
        };
        if expansion_assessment.is_none() && saved_foundry.is_some() {
            if self.retain_blocked_foundry_saving(obs.tick) {
                return true;
            }
            self.release_foundry_saving(commitments.as_deref_mut(), budget);
        }
        if let Some(assessment) = expansion_assessment
            && assessment.disposition == expansion::ExpansionDisposition::Build
        {
            let foundry_cost = BuildingKind::Foundry
                .base_stats()
                .construction
                .expect("Foundries are constructible")
                .cost;
            let guard = saved_foundry
                .as_ref()
                .and_then(|saving| saving.forecast_basis)
                .map_or_else(
                    || voluntary_scrap_guard.amount(TECH_RESERVE),
                    |basis| basis.protected_reserve,
                );
            if let Some(commitments) = commitments {
                let current_required_scrap = foundry_cost.saturating_add(guard);
                let required_scrap = saved_foundry
                    .as_ref()
                    .map_or(current_required_scrap, |saving| {
                        saving.required_scrap.max(current_required_scrap)
                    });
                let guard = required_scrap.saturating_sub(foundry_cost);
                let outcome =
                    commit_foundry_plan(commitments, budget, assessment.plan, guard, true);
                match outcome {
                    Some(FoundryCommitmentOutcome::Build(commitment)) => {
                        let accepted_at = self
                            .foundry_saving
                            .as_ref()
                            .map_or(obs.tick, |saving| saving.accepted_at);
                        self.foundry_saving = Some(FoundrySavingCommitment {
                            plan: commitment.plan.clone(),
                            accepted_at,
                            required_scrap: commitment.required_scrap,
                            forecast_basis: saved_foundry
                                .as_ref()
                                .and_then(|saving| saving.forecast_basis),
                            blocked_since: None,
                        });
                        Self::insert_build_before_harvest(
                            intents,
                            BuildingKind::Foundry,
                            commitment.plan.anchor,
                            Intent::BuildWith {
                                builder: commitment.plan.builder,
                                kind: BuildingKind::Foundry,
                                anchor: commitment.plan.anchor,
                            },
                        );
                    }
                    Some(FoundryCommitmentOutcome::Save(commitment)) => {
                        if let Some(saving) = &mut self.foundry_saving {
                            debug_assert_eq!(saving.plan.anchor, commitment.plan.anchor);
                            debug_assert_eq!(saving.plan.builder, commitment.plan.builder);
                            saving.required_scrap = commitment.required_scrap;
                            saving.blocked_since = None;
                        } else {
                            self.foundry_saving = Some(FoundrySavingCommitment {
                                plan: commitment.plan,
                                accepted_at: obs.tick,
                                required_scrap: commitment.required_scrap,
                                forecast_basis: None,
                                blocked_since: None,
                            });
                        }
                    }
                    None => {
                        if !self.retain_blocked_foundry_saving(obs.tick) {
                            self.release_foundry_saving(Some(commitments), budget);
                        }
                    }
                }
            } else if *budget >= foundry_cost.saturating_add(guard) {
                *budget -= foundry_cost;
                Self::insert_build_before_harvest(
                    intents,
                    BuildingKind::Foundry,
                    assessment.plan.anchor,
                    Intent::BuildWith {
                        builder: assessment.plan.builder,
                        kind: BuildingKind::Foundry,
                        anchor: assessment.plan.anchor,
                    },
                );
            } else {
                // A safe, worthwhile project owns the partial fund. Mask the
                // planning budget so no later channel can turn accumulation
                // into a permanently receding target.
                *budget = 0;
            }
            return true;
        }
        let ordinary_capital = if screen < 3 || (self.desperate && self.desperate_road) {
            0
        } else {
            self.capital_reserve(dials, obs, capital_context)
        };
        let ordinary_capital = if extractor_restoration_precedes_expansion {
            ordinary_capital.max(
                BuildingKind::Extractor
                    .base_stats()
                    .construction
                    .map_or(0, |construction| construction.cost),
            )
        } else {
            ordinary_capital
        };

        let voluntary_guard = voluntary_scrap_guard.amount(0);
        let capital = ordinary_capital.max(capacity_capital).max(voluntary_guard);
        let allow_repeatable_ground =
            !player_facing || self.has_honest_ground_objective(dials, obs, home, public_map);

        // Current public-map or contested work may require air, while a failed
        // ground look preserves the same demand durably. Keep exactly one
        // faction scout alive or queued once an Airworks can build it.
        if dials.scouting && self.air_scout_needed() && !self.solo_air_scout_suspended {
            let scout_kind = crate::stats::Role::Scout.unit_for(obs.faction);
            let planned_scouts = intents
                .iter()
                .filter(|intent| {
                    matches!(
                        intent,
                        Intent::TrainAt { kind, .. } if *kind == scout_kind
                    )
                })
                .count();
            let scout_count = alive(scout_kind) + queued(scout_kind) + planned_scouts;
            let airworks = Self::open_producer(
                obs,
                BuildingKind::Airworks,
                scout_kind,
                2,
                producer_lane_reservations,
                intents,
            );
            if scout_count == 0
                && let Some((_, airworks)) = airworks
            {
                let price = scout_kind.stats().cost;
                if *budget >= price.saturating_add(voluntary_guard) {
                    *budget -= price;
                    intents.push(Intent::TrainAt {
                        building: airworks.id,
                        kind: scout_kind,
                    });
                } else {
                    // Reconnaissance is the prerequisite for every
                    // target-driven island purchase, so cheaper drips
                    // must not spend its partial fund.
                    *budget = (*budget).min(voluntary_guard);
                }
            }
        }

        // The ferry fund: with a built Airworks, a known island target,
        // a squad worth lifting, and no lifter, the Skyhook's price is
        // banked ahead of every other military purchase — the wing and
        // AA arms otherwise skim the bank at their own smaller reserves
        // forever and the lifter never arrives (the Severance probe's
        // exact stall). Bought the moment the Airworks has room; the
        // hold ends with the purchase. The squad gate keeps the order
        // right and the seat alive: a lifter without riders is dead
        // capital, so while the last squad lies dead on the far shore
        // the fund stands down and the drip rebuilds fighters first.
        if !player_facing
            && dials.ferry
            && screen >= FERRY_SQUAD
            && alive(UnitKind::Skyhook) + queued(UnitKind::Skyhook) < 1
        {
            let airworks = Self::open_producer(
                obs,
                BuildingKind::Airworks,
                UnitKind::Skyhook,
                SHALLOW_QUEUE_DEPTH,
                producer_lane_reservations,
                intents,
            );
            if let Some((_, airworks)) = airworks
                && (Self::island_target(obs, home).is_some()
                    || (self.desperate && !self.desperate_road))
            {
                let price = UnitKind::Skyhook.stats().cost + TECH_RESERVE;
                if *budget >= price {
                    *budget -= UnitKind::Skyhook.stats().cost;
                    intents.push(Intent::TrainAt {
                        building: airworks.id,
                        kind: UnitKind::Skyhook,
                    });
                } else {
                    *budget = budget.saturating_sub(price);
                }
            }
        }

        Self::deep_tech_drip(
            dials,
            obs,
            producer_lane_reservations,
            voluntary_guard,
            budget,
            intents,
        );

        if harvesters < immediate_harvester_target(dials) as usize
            && *budget >= UnitKind::Harvester.stats().cost
        {
            if let Some((_, foundry)) = Self::open_producer(
                obs,
                BuildingKind::Foundry,
                UnitKind::Harvester,
                SHALLOW_QUEUE_DEPTH,
                producer_lane_reservations,
                intents,
            ) {
                *budget -= UnitKind::Harvester.stats().cost;
                intents.push(Intent::TrainAt {
                    building: foundry.id,
                    kind: UnitKind::Harvester,
                });
            }
        } else if !dials.adaptive_composition
            && allow_repeatable_ground
            && *budget >= UnitKind::Sentinel.stats().cost + capital
            && let Some((_, foundry)) = Self::open_producer(
                obs,
                BuildingKind::Foundry,
                UnitKind::Sentinel,
                SHALLOW_QUEUE_DEPTH,
                producer_lane_reservations,
                intents,
            )
        {
            *budget -= UnitKind::Sentinel.stats().cost;
            intents.push(Intent::TrainAt {
                building: foundry.id,
                kind: UnitKind::Sentinel,
            });
        }

        if !dials.tech {
            if dials.adaptive_composition {
                self.adaptive_production(
                    dials,
                    obs,
                    super::production::AdaptiveProductionContext::new(
                        combat_core_exclusions,
                        outstanding_air_production_ticks.is_none(),
                        capital,
                    )
                    .with_repeatable_ground(allow_repeatable_ground)
                    .with_producer_lane_reservations(producer_lane_reservations),
                    budget,
                    intents,
                );
            }
            return false;
        }
        self.fabricator_drip(
            dials,
            obs,
            producer_lane_reservations,
            ProductionGuards {
                voluntary: voluntary_guard,
                capital,
            },
            budget,
            intents,
        );
        if dials.adaptive_composition {
            self.adaptive_production(
                dials,
                obs,
                super::production::AdaptiveProductionContext::new(
                    combat_core_exclusions,
                    outstanding_air_production_ticks.is_none(),
                    capital,
                )
                .with_repeatable_ground(allow_repeatable_ground)
                .with_producer_lane_reservations(producer_lane_reservations),
                budget,
                intents,
            );
        }
        false
    }

    /// Whether another ordinary ground reinforcement has an honestly known
    /// job it can reach. The minimum fighting core and bounded specialists are
    /// still produced without one; this gate only stops the perpetual
    /// Sentinel/Lancer stream from turning a completed island operation into
    /// hundreds of idle bodies. A current island objective also qualifies
    /// while ordinary transport capacity exists, but a dark ghost alone does
    /// not authorize an endless next wave.
    pub(super) fn ordinary_ground_has_work(
        &self,
        dials: &Dials,
        obs: &Observation,
        home: TilePos,
    ) -> bool {
        if self.desperate && self.desperate_road {
            return true;
        }

        let mut routes = crate::bot::routing::RouteProjection::known_ground(obs);
        if obs
            .enemy_buildings
            .iter()
            .any(|building| Self::ground_reaches_building(&mut routes, home, building))
            || obs.enemy_units.iter().any(|unit| {
                unit.kind.stats().domain == Domain::Ground && routes.reaches(home, unit.tile)
            })
        {
            return true;
        }

        let transport_capable = obs
            .my_buildings
            .iter()
            .any(|building| building.built && building.kind == BuildingKind::Airworks)
            || obs
                .my_units
                .iter()
                .any(|unit| unit.kind == UnitKind::Skyhook)
            || obs
                .my_queues
                .iter()
                .flatten()
                .any(|kind| *kind == UnitKind::Skyhook);
        dials.ferry
            && transport_capable
            && obs.enemy_buildings.iter().any(|building| {
                building.seen
                    && building.built
                    && !Self::ground_reaches_building(&mut routes, home, building)
            })
    }

    fn ground_reaches_building(
        routes: &mut crate::bot::routing::RouteProjection<'_>,
        home: TilePos,
        building: &BuildingObs,
    ) -> bool {
        let (width, height) = building.kind.base_stats().size;
        (-1..=height).any(|dy| {
            (-1..=width).any(|dx| {
                let inside = dx >= 0 && dx < width && dy >= 0 && dy < height;
                !inside && routes.reaches(home, building.anchor.offset(dx, dy))
            })
        })
    }

    fn airworks_capacity_site(
        &self,
        dials: &Dials,
        obs: &Observation,
        home: TilePos,
        claims: ConstructionClaims<'_>,
        outstanding_air_production_ticks: Option<u64>,
    ) -> Option<TilePos> {
        let target = Self::airworks_capacity_target(outstanding_air_production_ticks)?;
        if !claims.player_facing || !dials.deep_tech {
            return None;
        }
        let completed = |kind: BuildingKind| {
            obs.my_buildings
                .iter()
                .filter(|building| building.kind == kind && building.built)
                .count()
        };
        if completed(BuildingKind::Fabricator) == 0
            || completed(BuildingKind::Airworks) == 0
            || completed(BuildingKind::Crucible) == 0
        {
            return None;
        }

        let completed_airworks = completed(BuildingKind::Airworks);
        let projected_airworks = Self::projected_count(obs, BuildingKind::Airworks, true);
        if projected_airworks >= target || projected_airworks > completed_airworks {
            return None;
        }
        if self
            .construction_builders(obs, claims.enlisted, claims.reserved)
            .is_empty()
        {
            return None;
        }
        self.placement_near(obs, BuildingKind::Airworks, home)
    }

    /// Scrap the strategic planners must leave untouched while one active air
    /// plan needs another ordinary Airworks. The caller may subtract this from
    /// the planners' private observation before they schedule units; Utility
    /// still sees the authoritative bank and spends the held fund through the
    /// exact same eligibility boundary as [`Self::airworks_capacity_site`].
    pub(in crate::bot) fn airworks_capacity_commitment(
        &self,
        dials: &Dials,
        obs: &Observation,
        home: TilePos,
        outstanding_air_production_ticks: Option<u64>,
        unavailable_builders: &[UnitId],
    ) -> u32 {
        let claims = ConstructionClaims {
            player_facing: true,
            enlisted: unavailable_builders,
            reserved: &[],
        };
        self.airworks_capacity_site(dials, obs, home, claims, outstanding_air_production_ticks)
            .and_then(|_| BuildingKind::Airworks.base_stats().construction)
            .map_or(0, |construction| {
                construction.cost.saturating_add(TECH_RESERVE)
            })
    }

    fn airworks_capacity_target(outstanding_air_production_ticks: Option<u64>) -> Option<usize> {
        let ticks = outstanding_air_production_ticks?;
        let target = ticks.div_ceil(AIRWORKS_ASSEMBLY_HORIZON_TICKS).max(1);
        Some(usize::try_from(target).unwrap_or(usize::MAX))
    }

    /// The next owed tech rung's price plus the fighting reserve — the
    /// fund the unbounded military drip must leave untouched so the
    /// construction channel can ever afford to climb. Zero once the
    /// dials' tree is fully raised (a standing site counts: its cost is
    /// already spent).
    fn capital_reserve(
        &self,
        dials: &Dials,
        obs: &Observation,
        context: ConstructionContext<'_>,
    ) -> u32 {
        let ConstructionContext { claims, .. } = context;
        let player_facing = claims.player_facing;
        let have = |kind: BuildingKind| Self::projected_count(obs, kind, player_facing) > 0;
        let price =
            |kind: BuildingKind| kind.base_stats().construction.map(|c| c.cost).unwrap_or(0);
        if !dials.tech {
            return 0;
        }
        let mut rungs = vec![BuildingKind::Fabricator];
        if dials.deep_tech {
            rungs.push(BuildingKind::Airworks);
        }
        // The profile-free Overseer remains the frozen QA yardstick. The
        // player-facing controller prices expansion separately through one
        // shared opportunity and security assessment.
        if dials.expansion && !player_facing {
            let foundries: Vec<_> = obs
                .my_buildings
                .iter()
                .filter(|building| building.kind == BuildingKind::Foundry)
                .map(|building| building.anchor)
                .collect();
            let ordinary_frontier_unlocked = !dials.deep_tech || have(BuildingKind::Airworks);
            let expansion_claim = ordinary_frontier_unlocked
                && obs
                    .known_scrap
                    .iter()
                    .filter(|(_, amount)| *amount > 0)
                    .map(|(tile, _)| *tile)
                    .chain(obs.known_frames.iter().copied().filter(|frame| {
                        !obs.my_buildings
                            .iter()
                            .chain(obs.enemy_buildings.iter())
                            .any(|building| building.anchor == *frame)
                    }))
                    .any(|tile| {
                        foundries
                            .iter()
                            .all(|f| f.chebyshev(tile) > EXPANSION_RADIUS)
                    });
            if expansion_claim
                && foundries.len() < LEGACY_FOUNDRY_CAP
                && have(BuildingKind::Foundry)
            {
                return price(BuildingKind::Foundry) + TECH_RESERVE;
            }
        }
        if dials.deep_tech {
            rungs.push(BuildingKind::Crucible);
        }
        rungs
            .into_iter()
            .find(|kind| !have(*kind))
            .map(|kind| price(kind) + TECH_RESERVE)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::super::AdjudicatedFoundryCommit;
    use super::*;
    use crate::bot::executive::Executive;
    use crate::bot::intelligence::{ContactEvidence, StrategicIntelligence};
    use crate::bot::observation::BuildingObs;
    use crate::command::Command;
    use crate::ids::{BuildingId, PlayerId};
    use crate::scenario::{BotConfig, BotDifficulty, BotStance, PlayerSpec, Scenario};
    use crate::state::Faction;

    fn observation() -> Observation {
        let harvester = UnitObs {
            id: UnitId(3),
            player: PlayerId(0),
            kind: UnitKind::Harvester,
            tile: TilePos::new(3, 4),
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
        };
        Observation {
            tick: 0,
            map_width: 16,
            map_height: 10,
            my_units: vec![harvester],
            my_buildings: vec![BuildingObs {
                id: BuildingId(0),
                player: PlayerId(0),
                kind: BuildingKind::Foundry,
                anchor: TilePos::new(1, 1),
                hp: BuildingKind::Foundry.base_stats().max_hp,
                built: true,
                seen: true,
                tier: 0,
            }],
            my_queues: vec![Vec::new()],
            visible: vec![true; 16 * 10],
            explored: vec![true; 16 * 10],
            known_rock: (0..10).map(|y| TilePos::new(7, y)).collect(),
            ..Observation::default()
        }
    }

    fn direct_player_facing_economy(
        policy: &UtilityPolicy,
        obs: &Observation,
        home: TilePos,
        unit_contacts: &[UnitContact],
        building_contacts: &[BuildingContact],
    ) -> Vec<Intent> {
        let enemy_base = obs
            .enemy_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Foundry)
            .map(|building| {
                (
                    building.anchor.manhattan(home),
                    building.anchor.y,
                    building.anchor.x,
                )
            })
            .min()
            .map(|(_, y, x)| TilePos::new(x, y));
        let mut routes = crate::bot::routing::RouteProjection::ground_avoiding(obs, |tile| {
            policy.harvest_location_contested(tile)
                || UtilityPolicy::source_has_known_danger(
                    obs,
                    tile,
                    Some(unit_contacts),
                    Some(building_contacts),
                )
        });
        let mut intents = Vec::new();
        for unit in obs.my_units.iter().filter(|unit| {
            unit.kind.stats().harvest.is_some()
                && unit.idle
                && Some(unit.id) != policy.scout
                && !policy.evacuating_workers.contains(&unit.id)
        }) {
            if !UtilityPolicy::harvester_reaches_drop_off(obs, unit, &mut routes) {
                continue;
            }
            let mut candidates: Vec<_> = obs
                .known_scrap
                .iter()
                .chain(obs.known_wrecks.iter())
                .filter(|(position, amount)| {
                    *amount > 0
                        && !policy.dead_nodes.contains(position)
                        && !UtilityPolicy::source_in_salvage_incident(obs, *position)
                        && !policy.harvest_location_contested(*position)
                        && !UtilityPolicy::source_has_known_danger(
                            obs,
                            *position,
                            Some(unit_contacts),
                            Some(building_contacts),
                        )
                        && enemy_base.is_none_or(|enemy| {
                            position.manhattan(home) <= position.manhattan(enemy)
                        })
                })
                .map(|(position, _)| (position.manhattan(unit.tile), position.y, position.x))
                .collect();
            candidates.sort_unstable();
            let node = candidates.into_iter().find_map(|(_, y, x)| {
                let source = TilePos::new(x, y);
                UtilityPolicy::harvester_reaches_source(obs, unit, source, &mut routes)
                    .then_some(source)
            });
            if let Some(node) = node {
                intents.push(Intent::AssignHarvest {
                    unit: unit.id,
                    node,
                });
            }
        }
        intents
    }

    #[test]
    fn player_facing_danger_projection_is_lazy_and_shared_between_channels() {
        let mut obs = observation();
        let mut policy = UtilityPolicy::new();
        let mut intents = Vec::new();

        policy.economy(&obs, TilePos::new(1, 1), false, None, None, &mut intents);
        assert_eq!(policy.harvest_danger_build_count(), 0);

        obs.my_units[0].idle = false;
        policy.economy(&obs, TilePos::new(1, 1), true, None, None, &mut intents);
        assert_eq!(policy.harvest_danger_build_count(), 0);

        obs.my_units[0].idle = true;
        policy.economy(&obs, TilePos::new(1, 1), true, None, None, &mut intents);
        assert_eq!(policy.harvest_danger_build_count(), 1);

        obs.salvage_incidents = vec![TilePos::new(12, 8)];
        policy.refresh_contested_harvest_regions(&obs, None, None);
        let mut build_intents = vec![Intent::Build {
            kind: BuildingKind::Turret,
            anchor: TilePos::new(4, 1),
        }];
        policy.bind_player_facing_builders(&obs, &[], &[], &[], &[], &mut build_intents);
        assert_eq!(
            policy.harvest_danger_build_count(),
            1,
            "economy, region clearance, and builder routing must share one immutable projection",
        );

        obs.blips.push(TilePos::new(3, 4));
        policy.economy(&obs, TilePos::new(1, 1), true, None, None, &mut intents);
        assert_eq!(policy.harvest_danger_build_count(), 2);
    }

    #[test]
    fn projected_economy_matches_direct_candidate_and_route_decisions() {
        let mut obs = observation();
        obs.map_width = 32;
        obs.map_height = 20;
        obs.visible = vec![true; 32 * 20];
        obs.explored = vec![true; 32 * 20];
        obs.known_rock = (0..20)
            .filter(|y| !matches!(*y, 9..=11))
            .map(|y| TilePos::new(15, y))
            .collect();
        obs.my_units[0].tile = TilePos::new(3, 4);
        obs.my_units.push(UnitObs {
            id: UnitId(4),
            tile: TilePos::new(3, 15),
            ..obs.my_units[0].clone()
        });
        obs.known_scrap = vec![
            (TilePos::new(6, 4), 100),
            (TilePos::new(6, 17), 100),
            (TilePos::new(22, 10), 100),
        ];
        obs.known_wrecks = vec![(TilePos::new(4, 13), 80), (TilePos::new(28, 17), 80)];
        obs.blips = vec![TilePos::new(6, 4)];
        let mobile = UnitContact {
            id: UnitId(90),
            player: PlayerId(1),
            kind: UnitKind::Sentinel,
            tile: TilePos::new(22, 10),
            hp: UnitKind::Sentinel.stats().max_hp,
            grounded: false,
            last_seen: obs.tick,
            evidence: ContactEvidence::Remembered,
        };
        let ghost_anchor = TilePos::new(27, 16);
        obs.enemy_buildings.push(BuildingObs {
            id: BuildingId(u32::MAX),
            player: PlayerId(1),
            kind: BuildingKind::Turret,
            anchor: ghost_anchor,
            hp: BuildingKind::Turret.base_stats().max_hp,
            built: true,
            seen: false,
            tier: 0,
        });
        let ghost = BuildingContact {
            id: Some(BuildingId(91)),
            player: PlayerId(1),
            kind: BuildingKind::Turret,
            anchor: ghost_anchor,
            hp: BuildingKind::Turret.tier_stats(1).max_hp,
            built: true,
            tier: 1,
            last_seen: Some(obs.tick),
            evidence: ContactEvidence::Remembered,
        };
        let policy = UtilityPolicy::new();
        let expected = direct_player_facing_economy(
            &policy,
            &obs,
            TilePos::new(1, 1),
            std::slice::from_ref(&mobile),
            std::slice::from_ref(&ghost),
        );
        assert!(!expected.is_empty(), "the parity fixture must assign work");

        let mut optimized = policy.clone();
        let mut actual = Vec::new();
        optimized.economy(
            &obs,
            TilePos::new(1, 1),
            true,
            Some(std::slice::from_ref(&mobile)),
            Some(std::slice::from_ref(&ghost)),
            &mut actual,
        );

        assert_eq!(actual, expected);
    }

    fn add_building(
        obs: &mut Observation,
        id: u32,
        kind: BuildingKind,
        anchor: TilePos,
        built: bool,
    ) {
        obs.my_buildings.push(BuildingObs {
            id: BuildingId(id),
            player: PlayerId(0),
            kind,
            anchor,
            hp: kind.base_stats().max_hp,
            built,
            seen: true,
            tier: 0,
        });
        obs.my_queues.push(Vec::new());
    }

    fn add_unit(obs: &mut Observation, id: u32, kind: UnitKind, tile: TilePos) {
        obs.my_units.push(UnitObs {
            id: UnitId(id),
            player: PlayerId(0),
            kind,
            tile,
            hp: kind.stats().max_hp,
            idle: true,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        });
    }

    fn add_enemy_building(
        obs: &mut Observation,
        id: u32,
        kind: BuildingKind,
        anchor: TilePos,
        seen: bool,
    ) {
        obs.enemy_buildings.push(BuildingObs {
            id: BuildingId(id),
            player: PlayerId(1),
            kind,
            anchor,
            hp: kind.base_stats().max_hp,
            built: true,
            seen,
            tier: 0,
        });
    }

    fn completed_tree() -> Observation {
        let mut obs = observation();
        add_building(
            &mut obs,
            1,
            BuildingKind::Fabricator,
            TilePos::new(4, 1),
            true,
        );
        add_building(
            &mut obs,
            2,
            BuildingKind::Airworks,
            TilePos::new(1, 5),
            true,
        );
        add_building(
            &mut obs,
            3,
            BuildingKind::Crucible,
            TilePos::new(4, 5),
            true,
        );
        for id in 4..8 {
            add_unit(
                &mut obs,
                id,
                UnitKind::Harvester,
                TilePos::new(2 + i32::try_from(id - 4).unwrap(), 3),
            );
        }
        for id in 20..23 {
            add_unit(
                &mut obs,
                id,
                UnitKind::Sentinel,
                TilePos::new(2 + i32::try_from(id - 20).unwrap(), 8),
            );
        }
        obs
    }

    fn expansion_briefing(obs: &Observation, home: TilePos, hostile: TilePos) -> PublicMapBriefing {
        let width = usize::try_from(obs.map_width).expect("fixture width is positive");
        let height = usize::try_from(obs.map_height).expect("fixture height is positive");
        let mut map = vec![vec!['.'; width]; height];
        map[usize::try_from(home.y).expect("home y is in bounds")]
            [usize::try_from(home.x).expect("home x is in bounds")] = '1';
        map[usize::try_from(hostile.y).expect("hostile y is in bounds")]
            [usize::try_from(hostile.x).expect("hostile x is in bounds")] = '2';
        PublicMapBriefing::from_scenario(&Scenario {
            name: "expansion economy fixture".into(),
            seed: 0,
            map: map
                .into_iter()
                .map(|row| row.into_iter().collect())
                .collect(),
            players: vec![
                PlayerSpec {
                    name: "home".into(),
                    faction: Faction::Ferrous,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "hostile".into(),
                    faction: Faction::Cupric,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
            ],
            units: Vec::new(),
            buildings: Vec::new(),
            meta: None,
        })
        .expect("expansion economy briefing is valid")
    }

    #[derive(Clone, Copy)]
    struct ExpansionDecisionState<'a> {
        spendable_scrap: u32,
        same_think_intents: &'a [Intent],
    }

    fn player_expansion_assessment(
        policy: &UtilityPolicy,
        dials: &Dials,
        obs: &Observation,
        home: TilePos,
        public_map: &PublicMapBriefing,
        unit_contacts: Option<&[UnitContact]>,
        decision: ExpansionDecisionState<'_>,
    ) -> Option<expansion::FoundryExpansionAssessment> {
        let (foundries, pending_foundries) = UtilityPolicy::projected_foundries(obs);
        assert_eq!(
            pending_foundries, 0,
            "fixture must not include a pending Foundry"
        );
        let builders = policy.construction_builders(obs, &[], &[]);
        policy.player_facing_foundry_assessment(
            dials,
            obs,
            FoundryAssessmentContext {
                claim: FoundryClaimContext {
                    home,
                    projected_foundries: &foundries,
                    builders: &builders,
                    support_extractors: obs.my_buildings.iter().any(|building| {
                        building.kind == BuildingKind::Fabricator && building.built
                    }),
                    ordinary_frontiers: !dials.deep_tech
                        || UtilityPolicy::projected_count(obs, BuildingKind::Airworks, true) > 0,
                    unit_contacts,
                    building_contacts: Some(&[]),
                },
                public_map,
                combat_core_exclusions: &[],
                spendable_scrap: decision.spendable_scrap,
                voluntary_scrap_guard: Reserve::Ordinary,
                required_anchor: None,
            },
            decision.same_think_intents,
        )
    }

    #[test]
    fn repeatable_ground_requires_a_reachable_or_current_transport_objective() {
        let home = TilePos::new(1, 1);
        let mut dials = Dials::balanced();
        let mut policy = UtilityPolicy::new();

        let mut connected = observation();
        add_enemy_building(
            &mut connected,
            80,
            BuildingKind::Foundry,
            TilePos::new(4, 5),
            false,
        );
        assert!(
            policy.ordinary_ground_has_work(&dials, &connected, home),
            "a remembered building on the explored home component remains a deployable job"
        );
        for y in 0..connected.map_height {
            connected.explored[(y * connected.map_width + 3) as usize] = false;
        }
        assert!(
            !policy.ordinary_ground_has_work(&dials, &connected, home),
            "an unexplored corridor is not an honestly known ground deployment route"
        );
        for y in 0..connected.map_height {
            connected.explored[(y * connected.map_width + 3) as usize] = true;
        }
        assert!(policy.ordinary_ground_has_work(&dials, &connected, home));

        let mut island = observation();
        add_enemy_building(
            &mut island,
            81,
            BuildingKind::Foundry,
            TilePos::new(11, 5),
            true,
        );
        assert!(!policy.ordinary_ground_has_work(&dials, &island, home));
        add_building(
            &mut island,
            2,
            BuildingKind::Airworks,
            TilePos::new(1, 5),
            true,
        );
        assert!(
            policy.ordinary_ground_has_work(&dials, &island, home),
            "a current disconnected objective plus real transport capacity can consume another wave"
        );

        island.enemy_buildings[0].seen = false;
        assert!(
            !policy.ordinary_ground_has_work(&dials, &island, home),
            "a dark island ghost cannot fund an unbounded next wave"
        );
        island.enemy_buildings[0].seen = true;
        dials.ferry = false;
        assert!(
            !policy.ordinary_ground_has_work(&dials, &island, home),
            "transport capacity is not a deployment plan when ferry play is disabled"
        );

        island.enemy_buildings.clear();
        island.enemy_units.push(UnitObs {
            id: UnitId(90),
            player: PlayerId(1),
            kind: UnitKind::Sentinel,
            tile: TilePos::new(4, 5),
            hp: UnitKind::Sentinel.stats().max_hp,
            idle: false,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        });
        assert!(policy.ordinary_ground_has_work(&dials, &island, home));
        island.enemy_units[0].kind = crate::stats::Role::Scout.unit_for(island.faction);
        assert!(
            !policy.ordinary_ground_has_work(&dials, &island, home),
            "a ground army cannot prosecute an aircraft contact"
        );

        policy.desperate = true;
        policy.desperate_road = true;
        assert!(
            policy.ordinary_ground_has_work(&dials, &island, home),
            "a fully explored mirror road keeps a dark connected endgame live"
        );
    }

    #[test]
    fn foundry_drip_obeys_the_deployment_contract() {
        let home = TilePos::new(1, 1);
        let mut obs = observation();
        for id in 4..8 {
            add_unit(
                &mut obs,
                id,
                UnitKind::Harvester,
                TilePos::new(2 + i32::try_from(id - 4).unwrap(), 3),
            );
        }
        for id in 20..25 {
            add_unit(
                &mut obs,
                id,
                UnitKind::Sentinel,
                TilePos::new(2 + i32::try_from(id - 20).unwrap(), 7),
            );
        }
        for id in 30..34 {
            add_unit(
                &mut obs,
                id,
                UnitKind::Scuttler,
                TilePos::new(2 + i32::try_from(id - 30).unwrap(), 8),
            );
        }
        add_unit(&mut obs, 40, UnitKind::Excavator, TilePos::new(6, 8));
        obs.scrap = 10_000;
        let mut dials = Dials::balanced();
        dials.adaptive_composition = true;
        let train = |world: &Observation| {
            let mut budget = world.scrap;
            let mut intents = Vec::new();
            UtilityPolicy::new().production(
                &dials,
                world,
                home,
                ConstructionClaims {
                    player_facing: true,
                    enlisted: &[],
                    reserved: &[],
                },
                &mut budget,
                &mut intents,
            );
            intents
                .into_iter()
                .filter(|intent| {
                    matches!(
                        intent,
                        Intent::TrainAt {
                            building: BuildingId(0),
                            kind: UnitKind::Sentinel,
                        }
                    )
                })
                .count()
        };

        assert_eq!(
            train(&obs),
            0,
            "a complete worker roster and no deployable enemy must leave the recurring Foundry drip idle"
        );
        add_enemy_building(
            &mut obs,
            80,
            BuildingKind::Foundry,
            TilePos::new(4, 5),
            false,
        );
        assert_eq!(train(&obs), 1);
        obs.enemy_buildings.clear();
        assert_eq!(
            train(&obs),
            0,
            "removing the last actionable objective closes the recurring stream on the next admitted think"
        );
    }

    #[test]
    fn adaptive_core_production_remains_available_without_the_tech_channel() {
        let mut obs = observation();
        for id in 4..=6 {
            obs.my_units.push(UnitObs {
                id: UnitId(id),
                tile: TilePos::new(id as i32, 4),
                ..obs.my_units[0].clone()
            });
        }
        let sentinel_cost = UnitKind::Sentinel.stats().cost;
        obs.scrap = sentinel_cost * 2;

        let mut dials = Dials::full();
        dials.tech = false;
        dials.adaptive_composition = true;
        dials.harvester_target = 4;
        dials.army_size = 2;
        dials.discretionary_slots = 0;
        let mut budget = obs.scrap;
        let mut intents = Vec::new();

        UtilityPolicy::new().production(
            &dials,
            &obs,
            TilePos::new(1, 1),
            ConstructionClaims {
                player_facing: true,
                enlisted: &[],
                reserved: &[],
            },
            &mut budget,
            &mut intents,
        );

        assert_eq!(budget, 0);
        assert_eq!(
            intents,
            vec![
                Intent::TrainAt {
                    building: BuildingId(0),
                    kind: UnitKind::Sentinel,
                },
                Intent::TrainAt {
                    building: BuildingId(0),
                    kind: UnitKind::Sentinel,
                },
            ],
            "disabling the tech channel must not disable the player-facing defensive core"
        );
    }

    fn capacity_decision(obs: &Observation, demand: Option<u64>) -> (u32, Vec<Intent>) {
        let mut budget = obs.scrap;
        let mut intents = Vec::new();
        UtilityPolicy::new().production_with_air_demand(
            &Dials::balanced(),
            obs,
            ProductionContext::new(
                TilePos::new(1, 1),
                ConstructionClaims {
                    player_facing: true,
                    enlisted: &[],
                    reserved: &[],
                },
                demand,
            ),
            &mut budget,
            &mut intents,
        );
        (budget, intents)
    }

    fn guarded_capacity_decision(
        obs: &Observation,
        demand: Option<u64>,
        guard: u32,
        mut intents: Vec<Intent>,
    ) -> (u32, Vec<Intent>) {
        let mut budget = obs.scrap;
        UtilityPolicy::new().production_with_air_demand(
            &Dials::balanced(),
            obs,
            ProductionContext::new(
                TilePos::new(1, 1),
                ConstructionClaims {
                    player_facing: true,
                    enlisted: &[],
                    reserved: &[],
                },
                demand,
            )
            .with_voluntary_scrap_guard(Reserve::Exact(guard)),
            &mut budget,
            &mut intents,
        );
        (budget, intents)
    }

    fn capital_reserve_for(
        policy: &UtilityPolicy,
        dials: &Dials,
        obs: &Observation,
        home: TilePos,
        player_facing: bool,
    ) -> u32 {
        policy.capital_reserve(
            dials,
            obs,
            ConstructionContext::new(
                home,
                ConstructionClaims {
                    player_facing,
                    enlisted: &[],
                    reserved: &[],
                },
            ),
        )
    }

    fn airworks_builds(intents: &[Intent]) -> Vec<TilePos> {
        intents
            .iter()
            .filter_map(|intent| match intent {
                Intent::Build {
                    kind: BuildingKind::Airworks,
                    anchor,
                }
                | Intent::BuildWith {
                    kind: BuildingKind::Airworks,
                    anchor,
                    ..
                } => Some(*anchor),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn strategic_airworks_prelude_prevents_a_duplicate_or_delaying_scout_order() {
        let mut obs = completed_tree();
        add_building(
            &mut obs,
            4,
            BuildingKind::Airworks,
            TilePos::new(8, 5),
            true,
        );
        obs.scrap = 10_000;
        let scout = crate::stats::Role::Scout.unit_for(obs.faction);
        let airworks = BuildingId(2);
        let second_airworks = BuildingId(4);
        let mut intents = vec![
            Intent::TrainAt {
                building: airworks,
                kind: scout,
            },
            Intent::TrainAt {
                building: airworks,
                kind: UnitKind::Buzzard,
            },
        ];
        let mut policy = UtilityPolicy::new();
        policy.persistent_air_scout_needed = true;
        let mut dials = Dials::balanced();
        dials.adaptive_composition = true;
        let mut budget = obs.scrap;

        policy.production_with_air_demand(
            &dials,
            &obs,
            ProductionContext::new(
                TilePos::new(1, 1),
                ConstructionClaims {
                    player_facing: true,
                    enlisted: &[],
                    reserved: &[],
                },
                Some(4_000),
            ),
            &mut budget,
            &mut intents,
        );

        assert_eq!(
            intents
                .iter()
                .filter(|intent| matches!(
                    intent,
                    Intent::TrainAt { building, kind }
                        if *building == airworks && *kind == scout
                ))
                .count(),
            1,
            "the strategic replacement scout satisfies utility reconnaissance"
        );
        assert_eq!(
            super::production::planned_at(&intents, airworks),
            2,
            "utility must honor the full strategic Airworks prelude: {intents:?}"
        );
        assert_eq!(
            super::production::planned_at(&intents, second_airworks),
            0,
            "an active operation owns every Airworks queue until its cohort is complete: {intents:?}"
        );
    }

    #[test]
    fn partial_air_scout_fund_blocks_a_cheaper_worker_until_recon_can_launch() {
        let mut obs = completed_tree();
        obs.my_units
            .retain(|unit| !matches!(unit.id, UnitId(6 | 7)));
        let scout = crate::stats::Role::Scout.unit_for(obs.faction);
        let scout_cost = scout.stats().cost;
        assert!(UnitKind::Harvester.stats().cost < scout_cost);

        let mut dials = Dials::balanced();
        dials.adaptive_composition = true;
        let decide = |public_start_demand: bool, persistent_demand: bool, scrap: u32| {
            let mut policy = UtilityPolicy::new();
            policy.public_prior_air_scout_needed = public_start_demand;
            policy.persistent_air_scout_needed = persistent_demand;
            let mut current = obs.clone();
            current.scrap = scrap;
            let mut budget = scrap;
            let mut intents = Vec::new();
            policy.production_with_air_demand(
                &dials,
                &current,
                ProductionContext::new(
                    TilePos::new(1, 1),
                    ConstructionClaims {
                        player_facing: true,
                        enlisted: &[],
                        reserved: &[],
                    },
                    None,
                ),
                &mut budget,
                &mut intents,
            );
            (budget, intents)
        };

        assert_eq!(
            decide(false, false, scout_cost - 1),
            (
                scout_cost - 1 - UnitKind::Harvester.stats().cost,
                vec![Intent::TrainAt {
                    building: BuildingId(0),
                    kind: UnitKind::Harvester,
                }]
            ),
            "without an owed flyer, the same finite bank can replace the missing worker"
        );
        assert_eq!(
            decide(false, true, scout_cost - 1),
            (0, Vec::new()),
            "an incomplete reconnaissance fund must not leak into a cheaper worker order"
        );
        assert_eq!(
            decide(false, true, scout_cost),
            (
                0,
                vec![Intent::TrainAt {
                    building: BuildingId(2),
                    kind: scout,
                }]
            ),
            "the completed fund must become exactly one faction scout at the Airworks"
        );
        assert_eq!(
            decide(true, false, scout_cost),
            decide(false, true, scout_cost),
            "a current public-map requirement and proven ground failure share the production mechanism without sharing persistence"
        );
    }

    #[test]
    fn voluntary_guard_survives_scout_and_current_air_response_priorities() {
        let guard = UnitKind::Sentinel.stats().cost;
        let scout = crate::stats::Role::Scout.unit_for(Faction::Ferrous);
        let scout_cost = scout.stats().cost;
        let aa = crate::stats::Role::AntiAir.unit_for(Faction::Ferrous);
        let aa_cost = aa.stats().cost;
        let mut dials = Dials::balanced();
        dials.adaptive_composition = true;

        let scout_decision = |scrap| {
            let mut obs = completed_tree();
            obs.scrap = scrap;
            let mut policy = UtilityPolicy::new();
            policy.persistent_air_scout_needed = true;
            let mut budget = scrap;
            let mut intents = Vec::new();
            policy.production_with_air_demand(
                &dials,
                &obs,
                ProductionContext::new(
                    TilePos::new(1, 1),
                    ConstructionClaims {
                        player_facing: true,
                        enlisted: &[],
                        reserved: &[],
                    },
                    None,
                )
                .with_voluntary_scrap_guard(Reserve::Exact(guard)),
                &mut budget,
                &mut intents,
            );
            (budget, intents)
        };
        let (guarded_budget, guarded_scout) = scout_decision(scout_cost + guard - 1);
        assert_eq!(guarded_budget, guard);
        assert!(guarded_scout.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt { kind, .. } if *kind == scout
        )));
        let (funded_budget, funded_scout) = scout_decision(scout_cost + guard);
        assert_eq!(funded_budget, guard);
        assert!(funded_scout.iter().any(|intent| matches!(
            intent,
            Intent::TrainAt { kind, .. } if *kind == scout
        )));

        let aa_decision = |scrap| {
            let mut obs = completed_tree();
            obs.scrap = scrap;
            add_unit(&mut obs, 90, UnitKind::Condor, TilePos::new(7, 7));
            let enemy = obs.my_units.pop().expect("the air contact was appended");
            obs.enemy_units.push(UnitObs {
                player: PlayerId(1),
                ..enemy
            });
            let mut budget = scrap;
            let mut intents = Vec::new();
            UtilityPolicy::new().production_with_air_demand(
                &dials,
                &obs,
                ProductionContext::new(
                    TilePos::new(1, 1),
                    ConstructionClaims {
                        player_facing: true,
                        enlisted: &[],
                        reserved: &[],
                    },
                    None,
                )
                .with_voluntary_scrap_guard(Reserve::Exact(guard)),
                &mut budget,
                &mut intents,
            );
            (budget, intents)
        };
        let (guarded_budget, guarded_aa) = aa_decision(aa_cost + guard - 1);
        assert_eq!(guarded_budget, aa_cost + guard - 1);
        assert!(guarded_aa.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt { kind, .. } if *kind == aa
        )));
        let (funded_budget, funded_aa) = aa_decision(aa_cost + guard);
        assert_eq!(funded_budget, guard);
        assert!(funded_aa.iter().any(|intent| matches!(
            intent,
            Intent::TrainAt { kind, .. } if *kind == aa
        )));
    }

    #[test]
    fn a_lost_solo_air_scout_releases_production_until_fresh_enemy_sight() {
        let home = TilePos::new(1, 1);
        let enemy_base = TilePos::new(12, 4);
        let scout_kind = crate::stats::Role::Scout.unit_for(Faction::Ferrous);
        let scout_cost = scout_kind.stats().cost;
        let mut obs = completed_tree();
        obs.tick = 2_000;
        obs.scrap = scout_cost;
        obs.enemy_buildings.push(BuildingObs {
            id: BuildingId(u32::MAX),
            player: PlayerId(1),
            kind: BuildingKind::Foundry,
            anchor: enemy_base,
            hp: BuildingKind::Foundry.base_stats().max_hp,
            built: true,
            seen: false,
            tier: 0,
        });
        let mut dials = Dials::balanced();
        dials.adaptive_composition = true;
        let produce = |policy: &mut UtilityPolicy, observation: &Observation| {
            let mut budget = scout_cost;
            let mut intents = Vec::new();
            policy.production_with_air_demand(
                &dials,
                observation,
                ProductionContext::new(
                    home,
                    ConstructionClaims {
                        player_facing: true,
                        enlisted: &[],
                        reserved: &[],
                    },
                    None,
                ),
                &mut budget,
                &mut intents,
            );
            (budget, intents)
        };

        let mut policy = UtilityPolicy::new();
        let ground_scout = UnitId(3);
        let ground_start = obs
            .my_units
            .iter()
            .find(|unit| unit.id == ground_scout)
            .expect("the fixture owns a ground scout")
            .tile;
        policy.scout = Some(ground_scout);
        policy.scout_dispatch = Some(ScoutDispatch::ordinary(
            ground_scout,
            ground_start,
            enemy_base,
        ));
        policy.scouting(&obs, home, None, &[], &mut Vec::new());
        assert!(policy.persistent_air_scout_needed);
        assert!(!policy.solo_air_scout_suspended);
        assert_eq!(
            produce(&mut policy, &obs),
            (
                0,
                vec![Intent::TrainAt {
                    building: BuildingId(2),
                    kind: scout_kind,
                }]
            ),
            "the first proven island reconnaissance receives one dedicated flyer"
        );

        add_unit(&mut obs, 99, scout_kind, TilePos::new(2, 5));
        obs.tick += 1;
        let mut dispatch = Vec::new();
        policy.scouting(&obs, home, None, &[], &mut dispatch);
        assert!(matches!(
            dispatch.as_slice(),
            [Intent::Scout {
                unit: UnitId(99),
                ..
            }]
        ));
        assert!(policy.scout_dispatch.is_some());

        obs.enemy_units.push(UnitObs {
            id: UnitId(100),
            player: PlayerId(1),
            kind: UnitKind::Sentinel,
            tile: TilePos::new(8, 4),
            hp: UnitKind::Sentinel.stats().max_hp,
            idle: false,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        });
        obs.my_units.retain(|unit| unit.id != UnitId(99));
        obs.tick += 1;
        let mut after_loss = Vec::new();
        policy.scouting(&obs, home, None, &[], &mut after_loss);
        assert!(after_loss.is_empty());
        assert!(policy.solo_air_scout_suspended);
        assert_eq!(
            produce(&mut policy, &obs),
            (scout_cost, Vec::new()),
            "the dead scout must release its factory and bank instead of buying a replacement"
        );

        let mut fresh_sight_policy = policy.clone();
        let mut fresh_sight = obs.clone();
        fresh_sight.enemy_units.clear();
        fresh_sight.tick += 1;
        fresh_sight_policy.scouting(&fresh_sight, home, None, &[], &mut Vec::new());
        assert!(fresh_sight_policy.solo_air_scout_suspended);
        fresh_sight.tick += 1;
        fresh_sight.enemy_buildings[0].seen = true;
        fresh_sight_policy.scouting(&fresh_sight, home, None, &[], &mut Vec::new());
        assert!(
            !fresh_sight_policy.solo_air_scout_suspended,
            "a dark-to-current enemy-base sighting may rearm recon before the timed retry"
        );

        obs.tick += 10_000;
        policy.scouting(&obs, home, None, &[], &mut Vec::new());
        assert!(policy.solo_air_scout_suspended);
        assert_eq!(
            produce(&mut policy, &obs),
            (scout_cost, Vec::new()),
            "enemy sight that persisted through the loss is not fresh evidence"
        );

        obs.enemy_units.clear();
        policy.scouting(&obs, home, None, &[], &mut Vec::new());
        assert!(policy.solo_air_scout_suspended);
        let enemy_scout = crate::stats::Role::Scout.unit_for(Faction::Cupric);
        obs.enemy_units.push(UnitObs {
            id: UnitId(101),
            player: PlayerId(1),
            kind: enemy_scout,
            tile: TilePos::new(8, 4),
            hp: enemy_scout.stats().max_hp,
            idle: false,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        });
        obs.tick += SOLO_SCOUT_QUIET_TICKS - 1;
        policy.scouting(&obs, home, None, &[], &mut Vec::new());
        assert!(policy.solo_air_scout_suspended);
        assert_eq!(
            produce(&mut policy, &obs),
            (scout_cost, Vec::new()),
            "a reciprocal scout cannot bypass the complete quiet window"
        );

        obs.tick += 1;
        policy.scouting(&obs, home, None, &[], &mut Vec::new());
        assert!(!policy.solo_air_scout_suspended);
        assert_eq!(
            produce(&mut policy, &obs),
            (
                0,
                vec![Intent::TrainAt {
                    building: BuildingId(2),
                    kind: scout_kind,
                }]
            ),
            "a complete quiet window permits exactly one later solo reconnaissance cycle"
        );
    }

    #[test]
    fn airworks_capacity_rounds_active_work_up_to_two_minute_factory_loads() {
        assert_eq!(UtilityPolicy::airworks_capacity_target(None), None);
        assert_eq!(UtilityPolicy::airworks_capacity_target(Some(0)), Some(1));
        assert_eq!(
            UtilityPolicy::airworks_capacity_target(Some(AIRWORKS_ASSEMBLY_HORIZON_TICKS)),
            Some(1)
        );
        assert_eq!(
            UtilityPolicy::airworks_capacity_target(Some(AIRWORKS_ASSEMBLY_HORIZON_TICKS + 1)),
            Some(2)
        );
        assert_eq!(
            UtilityPolicy::airworks_capacity_target(Some(AIRWORKS_ASSEMBLY_HORIZON_TICKS * 2)),
            Some(2)
        );
        assert_eq!(
            UtilityPolicy::airworks_capacity_target(Some(AIRWORKS_ASSEMBLY_HORIZON_TICKS * 2 + 1)),
            Some(3)
        );
    }

    fn competing_expansion_fixture() -> (TilePos, TilePos, Observation, Dials, PublicMapBriefing) {
        let home = TilePos::new(1, 1);
        let extractor = TilePos::new(30, 16);
        let mut obs = completed_tree();
        obs.map_width = 48;
        obs.map_height = 24;
        obs.visible = vec![true; 48 * 24];
        obs.explored = obs.visible.clone();
        obs.known_rock.clear();
        obs.known_scrap.clear();
        add_building(
            &mut obs,
            10,
            BuildingKind::Foundry,
            TilePos::new(11, 1),
            true,
        );
        add_building(&mut obs, 11, BuildingKind::Extractor, extractor, true);

        let mut dials = Dials::full();
        dials.deep_tech = true;
        dials.expansion = true;
        dials.harvester_target = 5;
        dials.army_size = 3;
        dials.scouting = false;
        dials.radar = false;
        dials.reclaimers = false;
        dials.repair = false;
        dials.air_harass = false;
        dials.ferry = false;
        dials.mines = false;
        dials.discretionary_slots = 0;

        let public_map = expansion_briefing(&obs, home, TilePos::new(44, 20));
        (home, extractor, obs, dials, public_map)
    }

    #[test]
    fn active_airworks_capacity_precedes_an_otherwise_ready_expansion() {
        let (home, extractor, mut obs, mut dials, public_map) = competing_expansion_fixture();
        dials.extractors = false;
        let claims = ConstructionClaims {
            player_facing: true,
            enlisted: &[],
            reserved: &[],
        };
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundry has construction stats")
            .cost;
        let airworks_cost = BuildingKind::Airworks
            .base_stats()
            .construction
            .expect("Airworks has construction stats")
            .cost;
        obs.scrap = foundry_cost + airworks_cost + TECH_RESERVE;

        let ready = player_expansion_assessment(
            &UtilityPolicy::new(),
            &dials,
            &obs,
            home,
            &public_map,
            None,
            ExpansionDecisionState {
                spendable_scrap: obs.scrap,
                same_think_intents: &[],
            },
        )
        .expect("the supported Extractor makes an expansion worthwhile");
        assert_eq!(ready.disposition, expansion::ExpansionDisposition::Build);
        assert!(UtilityPolicy::foundry_supports_extractor(
            ready.plan.anchor,
            extractor
        ));

        let mut expansion_budget = obs.scrap;
        let mut expansion_intents = Vec::new();
        UtilityPolicy::new().production_with_air_demand(
            &dials,
            &obs,
            ProductionContext::new(home, claims, None).with_public_map(Some(&public_map)),
            &mut expansion_budget,
            &mut expansion_intents,
        );
        assert!(expansion_intents.iter().any(|intent| matches!(
            intent,
            Intent::BuildWith {
                kind: BuildingKind::Foundry,
                anchor,
                ..
            } if *anchor == ready.plan.anchor
        )));

        let demand = Some(AIRWORKS_ASSEMBLY_HORIZON_TICKS + 1);
        let capacity_anchor = UtilityPolicy::new()
            .airworks_capacity_site(&dials, &obs, home, claims, demand)
            .expect("the outstanding air work needs a second actionable Airworks");
        let mut competing_budget = obs.scrap;
        let mut competing_intents = Vec::new();
        UtilityPolicy::new().production_with_air_demand(
            &dials,
            &obs,
            ProductionContext::new(home, claims, demand).with_public_map(Some(&public_map)),
            &mut competing_budget,
            &mut competing_intents,
        );

        assert!(competing_intents.iter().any(|intent| matches!(
            intent,
            Intent::BuildWith {
                kind: BuildingKind::Airworks,
                anchor,
                ..
            } if *anchor == capacity_anchor
        )));
        assert!(competing_intents.iter().all(|intent| !matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Foundry,
                ..
            } | Intent::BuildWith {
                kind: BuildingKind::Foundry,
                ..
            }
        )));
        assert_eq!(
            competing_intents
                .iter()
                .filter(|intent| matches!(intent, Intent::Build { .. } | Intent::BuildWith { .. }))
                .count(),
            1,
            "capacity must exclusively own the construction channel"
        );
    }

    #[test]
    fn actionable_supported_extractor_precedes_an_otherwise_ready_expansion() {
        let (home, extractor, mut obs, mut dials, public_map) = competing_expansion_fixture();
        let frame = TilePos::new(9, 1);
        obs.known_frames = vec![frame];
        dials.extractors = true;
        let claims = ConstructionClaims {
            player_facing: true,
            enlisted: &[],
            reserved: &[],
        };
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundry has construction stats")
            .cost;
        let extractor_cost = BuildingKind::Extractor
            .base_stats()
            .construction
            .expect("Extractor has construction stats")
            .cost;
        obs.scrap = foundry_cost + extractor_cost + TECH_RESERVE;

        let ready = player_expansion_assessment(
            &UtilityPolicy::new(),
            &dials,
            &obs,
            home,
            &public_map,
            None,
            ExpansionDecisionState {
                spendable_scrap: obs.scrap,
                same_think_intents: &[],
            },
        )
        .expect("the distant supported Extractor makes an expansion worthwhile");
        assert_eq!(ready.disposition, expansion::ExpansionDisposition::Build);
        assert!(UtilityPolicy::foundry_supports_extractor(
            ready.plan.anchor,
            extractor
        ));
        assert!(
            UtilityPolicy::new()
                .supported_frame_restoration_claim(
                    &obs,
                    ConstructionContext::new(home, claims).with_public_map(Some(&public_map)),
                )
                .is_some(),
            "the nearby supported frame must have a legal builder"
        );

        let mut without_frame = obs.clone();
        without_frame.known_frames.clear();
        let mut expansion_budget = without_frame.scrap;
        let mut expansion_intents = Vec::new();
        UtilityPolicy::new().production_with_air_demand(
            &dials,
            &without_frame,
            ProductionContext::new(home, claims, None).with_public_map(Some(&public_map)),
            &mut expansion_budget,
            &mut expansion_intents,
        );
        assert!(expansion_intents.iter().any(|intent| matches!(
            intent,
            Intent::BuildWith {
                kind: BuildingKind::Foundry,
                anchor,
                ..
            } if *anchor == ready.plan.anchor
        )));

        let mut policy = UtilityPolicy::new();
        let mut competing_budget = obs.scrap;
        let mut competing_intents = Vec::new();
        policy.production_with_air_demand(
            &dials,
            &obs,
            ProductionContext::new(home, claims, None).with_public_map(Some(&public_map)),
            &mut competing_budget,
            &mut competing_intents,
        );
        assert!(competing_intents.iter().all(|intent| !matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Foundry,
                ..
            } | Intent::BuildWith {
                kind: BuildingKind::Foundry,
                ..
            }
        )));
        assert_eq!(
            competing_budget, extractor_cost,
            "production must preserve the actionable restoration fund"
        );
        policy.construction(
            &dials,
            &obs,
            ConstructionContext::new(home, claims).with_public_map(Some(&public_map)),
            &mut competing_budget,
            &mut competing_intents,
        );

        assert!(competing_intents.iter().any(|intent| matches!(
            intent,
            Intent::BuildWith {
                kind: BuildingKind::Extractor,
                anchor,
                ..
            } if *anchor == frame
        )));
        assert!(competing_intents.iter().all(|intent| !matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Foundry,
                ..
            } | Intent::BuildWith {
                kind: BuildingKind::Foundry,
                ..
            }
        )));
        assert_eq!(
            competing_intents
                .iter()
                .filter(|intent| matches!(intent, Intent::Build { .. } | Intent::BuildWith { .. }))
                .count(),
            1,
            "restoration must exclusively own the construction channel"
        );
    }

    #[test]
    fn expansion_preparation_queues_only_the_missing_screen_then_commits_the_foundry() {
        let home = TilePos::new(1, 1);
        let mut obs = completed_tree();
        obs.map_width = 40;
        obs.map_height = 24;
        obs.visible = vec![true; 40 * 24];
        obs.explored = vec![true; 40 * 24];
        obs.known_rock.clear();
        add_building(
            &mut obs,
            10,
            BuildingKind::Foundry,
            TilePos::new(10, 1),
            true,
        );
        obs.known_scrap = vec![(TilePos::new(30, 18), 800)];
        for id in 23..26 {
            add_unit(
                &mut obs,
                id,
                UnitKind::Sentinel,
                TilePos::new(5 + i32::try_from(id - 23).unwrap(), 8),
            );
        }

        let profile = BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, 1_616_304)
            .resolve_profile();
        let mut dials = Dials::scripted(
            &profile,
            DifficultyTuning::for_level(BotDifficulty::Standard),
        );
        dials.harvester_target = 4;
        dials.scouting = false;
        assert_eq!((profile.traits.greed, dials.expansion_greed), (64, 64));

        let foundry_fund = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundry has construction stats")
            .cost
            + TECH_RESERVE;
        let sentinel_cost = UnitKind::Sentinel.stats().cost;
        obs.scrap = foundry_fund + sentinel_cost;
        let public_map = expansion_briefing(&obs, home, TilePos::new(36, 20));
        let claims = ConstructionClaims {
            player_facing: true,
            enlisted: &[],
            reserved: &[],
        };
        let mut policy = UtilityPolicy::new();
        let before = player_expansion_assessment(
            &policy,
            &dials,
            &obs,
            home,
            &public_map,
            None,
            ExpansionDecisionState {
                spendable_scrap: obs.scrap,
                same_think_intents: &[],
            },
        )
        .expect("the rich forward district is worth one more screen unit");
        assert_eq!(before.missing_security_scrap, sentinel_cost);
        assert!(matches!(
            before.disposition,
            expansion::ExpansionDisposition::Prepare { .. }
        ));

        let mut budget = obs.scrap;
        let mut intents = Vec::new();
        policy.production_with_air_demand(
            &dials,
            &obs,
            ProductionContext::new(home, claims, None).with_public_map(Some(&public_map)),
            &mut budget,
            &mut intents,
        );
        assert_eq!(
            intents,
            vec![
                Intent::TrainAt {
                    building: BuildingId(0),
                    kind: UnitKind::Sentinel,
                },
                Intent::BuildWith {
                    builder: before.plan.builder,
                    kind: BuildingKind::Foundry,
                    anchor: before.plan.anchor,
                },
            ],
            "preparation should buy only the missing screen before committing the same site"
        );
        assert_eq!(budget, TECH_RESERVE);

        let mut adjudicated_policy = UtilityPolicy::new();
        let mut adjudicated_budget = obs.scrap;
        let mut adjudicated_intents = Vec::new();
        let promised = adjudicated_policy.production_with_air_demand(
            &dials,
            &obs,
            ProductionContext::new(home, claims, None)
                .with_public_map(Some(&public_map))
                .with_fresh_foundry_admission(FreshFoundryAdmission::Adjudicated),
            &mut adjudicated_budget,
            &mut adjudicated_intents,
        );
        assert!(!promised);
        assert!(
            adjudicated_intents.iter().all(|intent| !matches!(
                intent,
                Intent::BuildWith {
                    kind: BuildingKind::Foundry,
                    ..
                }
            )),
            "residual policy cannot dispatch a fresh adjudicated Foundry"
        );
        assert!(adjudicated_policy.foundry_saving.is_none());
        let mut no_expansion_dials = dials.clone();
        no_expansion_dials.expansion = false;
        let mut no_expansion_policy = UtilityPolicy::new();
        let mut no_expansion_budget = obs.scrap;
        let mut no_expansion_intents = Vec::new();
        no_expansion_policy.production_with_air_demand(
            &no_expansion_dials,
            &obs,
            ProductionContext::new(home, claims, None).with_public_map(Some(&public_map)),
            &mut no_expansion_budget,
            &mut no_expansion_intents,
        );
        assert_eq!(adjudicated_intents, no_expansion_intents);
        assert_eq!(adjudicated_budget, no_expansion_budget);

        let after = player_expansion_assessment(
            &policy,
            &dials,
            &obs,
            home,
            &public_map,
            None,
            ExpansionDecisionState {
                spendable_scrap: foundry_fund,
                same_think_intents: &intents,
            },
        )
        .expect("the same-think screen order completes expansion security");
        assert_eq!(after.disposition, expansion::ExpansionDisposition::Build);
    }

    #[test]
    fn unfinished_expansion_preparation_blocks_later_capital_projects() {
        let home = TilePos::new(1, 1);
        let frontier = TilePos::new(30, 18);
        let mut obs = completed_tree();
        obs.map_width = 40;
        obs.map_height = 24;
        obs.visible = vec![true; 40 * 24];
        obs.explored = obs.visible.clone();
        obs.known_rock.clear();
        obs.known_scrap = vec![(frontier, 10_000)];
        add_building(
            &mut obs,
            10,
            BuildingKind::Foundry,
            TilePos::new(10, 1),
            true,
        );
        for id in 23..26 {
            add_unit(
                &mut obs,
                id,
                UnitKind::Sentinel,
                TilePos::new(5 + i32::try_from(id - 23).unwrap(), 8),
            );
        }
        let crucible = obs
            .my_buildings
            .iter()
            .position(|building| building.kind == BuildingKind::Crucible)
            .expect("the completed-tree fixture has a Crucible");
        obs.my_buildings.remove(crucible);
        obs.my_queues.remove(crucible);

        let profile = BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, 1_616_304)
            .resolve_profile();
        let mut dials = Dials::scripted(
            &profile,
            DifficultyTuning::for_level(BotDifficulty::Standard),
        );
        dials.extractors = false;
        dials.upgrades = false;
        dials.harvester_target = 4;
        dials.scouting = false;
        dials.turret_response = false;
        dials.aa_response = false;
        dials.own_strength_scale = 2_500;

        let sentinel_cost = UnitKind::Sentinel.stats().cost;
        obs.scrap = 5_000;
        let public_map = expansion_briefing(&obs, home, TilePos::new(36, 20));
        let assessment = player_expansion_assessment(
            &UtilityPolicy::new(),
            &dials,
            &obs,
            home,
            &public_map,
            None,
            ExpansionDecisionState {
                spendable_scrap: obs.scrap,
                same_think_intents: &[],
            },
        )
        .expect("the rich frontier can finance its larger screen");
        assert!(assessment.missing_security_scrap > sentinel_cost * 4);
        assert!(matches!(
            assessment.disposition,
            expansion::ExpansionDisposition::Prepare { .. }
        ));

        let context = StrategicUtilityContext::new(&[], &[], &[], &public_map, Vec::new());
        let mut policy = UtilityPolicy::new();
        let intents = policy.think_with_intelligence(&dials, &obs, &[], &[], context);
        let sentinels = intents
            .iter()
            .filter(|intent| {
                matches!(
                    intent,
                    Intent::TrainAt {
                        kind: UnitKind::Sentinel,
                        ..
                    }
                )
            })
            .count();
        assert!(
            sentinels > 0,
            "preparation should make progress: {intents:?}"
        );
        assert!(
            u32::try_from(sentinels)
                .unwrap()
                .saturating_mul(sentinel_cost)
                < assessment.missing_security_scrap,
            "the per-think producer depth must leave security unfinished: {intents:?}"
        );
        assert!(
            intents.iter().all(|intent| !matches!(
                intent,
                Intent::Build { .. } | Intent::BuildWith { .. } | Intent::Upgrade { .. }
            )),
            "unfinished security preparation must retain the remaining Foundry fund: {intents:?}"
        );
    }

    #[test]
    fn remembered_danger_releases_an_unsafe_foundry_reserve_to_core_production() {
        let home = TilePos::new(1, 1);
        let frontier = TilePos::new(30, 18);
        let mut obs = completed_tree();
        obs.map_width = 40;
        obs.map_height = 24;
        obs.visible = vec![true; 40 * 24];
        obs.explored = obs.visible.clone();
        obs.known_rock.clear();
        obs.known_scrap = vec![(frontier, 800)];
        add_building(
            &mut obs,
            10,
            BuildingKind::Foundry,
            TilePos::new(10, 1),
            true,
        );
        for id in 23..27 {
            add_unit(
                &mut obs,
                id,
                UnitKind::Sentinel,
                TilePos::new(5 + i32::try_from(id - 23).unwrap(), 8),
            );
        }

        let profile = BotConfig::scripted(BotDifficulty::Standard, BotStance::Turtle, 1_616_304)
            .resolve_profile();
        let mut dials = Dials::scripted(
            &profile,
            DifficultyTuning::for_level(BotDifficulty::Standard),
        );
        dials.extractors = false;
        dials.upgrades = false;
        dials.harvester_target = 4;
        dials.scouting = false;
        assert_eq!(dials.army_size, 7);
        assert_eq!(dials.expansion_greed, profile.traits.greed);

        let claims = ConstructionClaims {
            player_facing: true,
            enlisted: &[],
            reserved: &[],
        };
        let foundry_fund = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundry has construction stats")
            .cost
            + TECH_RESERVE;
        obs.scrap = foundry_fund;
        let public_map = expansion_briefing(&obs, home, TilePos::new(36, 20));
        let safe_policy = UtilityPolicy::new();
        let safe = player_expansion_assessment(
            &safe_policy,
            &dials,
            &obs,
            home,
            &public_map,
            None,
            ExpansionDecisionState {
                spendable_scrap: obs.scrap,
                same_think_intents: &[],
            },
        )
        .expect("the visible rich frontier has a safe economic plan");
        assert_eq!(safe.disposition, expansion::ExpansionDisposition::Build);
        let mut safe_budget = obs.scrap;
        let mut safe_intents = Vec::new();
        UtilityPolicy::new().production_with_air_demand(
            &dials,
            &obs,
            ProductionContext::new(home, claims, None).with_public_map(Some(&public_map)),
            &mut safe_budget,
            &mut safe_intents,
        );
        assert_eq!(
            safe_intents,
            vec![Intent::BuildWith {
                builder: safe.plan.builder,
                kind: BuildingKind::Foundry,
                anchor: safe.plan.anchor,
            }]
        );
        assert_eq!(safe_budget, TECH_RESERVE);

        let mut threatened = obs.clone();
        threatened
            .my_units
            .retain(|unit| unit.kind != UnitKind::Sentinel || unit.id <= UnitId(24));
        threatened.scrap = UnitKind::Sentinel.stats().cost;
        let threat_tile = frontier.offset(-1, 0);
        let threat_index = usize::try_from(threat_tile.y * threatened.map_width + threat_tile.x)
            .expect("the threat index is nonnegative");
        threatened.visible[threat_index] = false;
        let contacts = [UnitContact {
            id: UnitId(90),
            player: PlayerId(1),
            kind: UnitKind::Avalanche,
            tile: threat_tile,
            hp: UnitKind::Avalanche.stats().max_hp,
            grounded: false,
            last_seen: threatened.tick,
            evidence: ContactEvidence::Remembered,
        }];
        assert!(
            player_expansion_assessment(
                &UtilityPolicy::new(),
                &dials,
                &threatened,
                home,
                &public_map,
                Some(&contacts),
                ExpansionDecisionState {
                    spendable_scrap: foundry_fund,
                    same_think_intents: &[],
                },
            )
            .is_none(),
            "remembered danger covering every site and builder route must release the project"
        );

        let mut budget = threatened.scrap;
        let mut intents = Vec::new();
        UtilityPolicy::new().production_with_air_demand(
            &dials,
            &threatened,
            ProductionContext::new(home, claims, None)
                .with_intelligence(Some(&contacts), Some(&[]))
                .with_public_map(Some(&public_map)),
            &mut budget,
            &mut intents,
        );
        assert_eq!(
            intents,
            vec![Intent::TrainAt {
                building: BuildingId(0),
                kind: UnitKind::Sentinel,
            }],
            "released capital should buy the one fighter still missing from the ordinary core"
        );
        assert_eq!(budget, 0);
    }

    #[test]
    fn unactionable_supported_extractor_releases_its_fund_to_core_production() {
        let home = TilePos::new(1, 1);
        let frame = TilePos::new(9, 1);
        let mut obs = completed_tree();
        obs.known_rock.clear();
        obs.known_frames = vec![frame];
        add_enemy_building(
            &mut obs,
            90,
            BuildingKind::Foundry,
            TilePos::new(12, 7),
            true,
        );
        obs.scrap = UnitKind::Sentinel.stats().cost;

        let mut dials = Dials::balanced();
        dials.adaptive_composition = false;
        dials.expansion = false;
        dials.upgrades = false;
        let open_claims = ConstructionClaims {
            player_facing: true,
            enlisted: &[],
            reserved: &[],
        };
        let policy = UtilityPolicy::new();
        assert!(UtilityPolicy::foundry_supports_extractor(home, frame));
        assert!(
            policy
                .supported_frame_restoration_claim(
                    &obs,
                    ConstructionContext::new(home, open_claims),
                )
                .is_some(),
            "the clear supported frame must have an exact safe builder before testing reserve release"
        );

        let mut safe_budget = obs.scrap;
        let mut safe_intents = Vec::new();
        UtilityPolicy::new().production(
            &dials,
            &obs,
            home,
            open_claims,
            &mut safe_budget,
            &mut safe_intents,
        );
        assert!(safe_intents.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Sentinel,
                ..
            }
        )));
        assert_eq!(
            safe_budget, obs.scrap,
            "an actionable restoration keeps its exact fund out of the ordinary fighter drip"
        );

        let harvesters: Vec<_> = obs
            .my_units
            .iter()
            .filter(|unit| unit.kind.stats().harvest.is_some())
            .map(|unit| unit.id)
            .collect();
        let claimed_builders = ConstructionClaims {
            player_facing: true,
            enlisted: &harvesters,
            reserved: &[],
        };
        assert!(
            policy
                .supported_frame_restoration_claim(
                    &obs,
                    ConstructionContext::new(home, claimed_builders),
                )
                .is_none(),
            "a frame without one available exact builder is not an actionable capital claim"
        );
        let mut claimed_budget = obs.scrap;
        let mut claimed_intents = Vec::new();
        UtilityPolicy::new().production(
            &dials,
            &obs,
            home,
            claimed_builders,
            &mut claimed_budget,
            &mut claimed_intents,
        );
        assert_eq!(
            claimed_intents,
            vec![Intent::TrainAt {
                building: BuildingId(0),
                kind: UnitKind::Sentinel,
            }],
            "claimed builders must release the unusable restoration fund into the missing core fighter"
        );
        assert_eq!(claimed_budget, 0);

        let contact_tile = TilePos::new(7, 2);
        let contact_index = usize::try_from(contact_tile.y * obs.map_width + contact_tile.x)
            .expect("the remembered contact index is nonnegative");
        obs.visible[contact_index] = false;
        let contacts = [UnitContact {
            id: UnitId(91),
            player: PlayerId(1),
            kind: UnitKind::Avalanche,
            tile: contact_tile,
            hp: UnitKind::Avalanche.stats().max_hp,
            grounded: false,
            last_seen: obs.tick,
            evidence: ContactEvidence::Remembered,
        }];
        let remembered_context = ConstructionContext::new(home, open_claims)
            .with_intelligence(Some(&contacts), Some(&[]));
        assert!(
            policy
                .supported_frame_restoration_claim(&obs, remembered_context)
                .is_none(),
            "remembered long-range fire must invalidate every unsafe exact route to the frame"
        );
        let mut remembered_budget = obs.scrap;
        let mut remembered_intents = Vec::new();
        UtilityPolicy::new().production_with_air_demand(
            &dials,
            &obs,
            ProductionContext::new(home, open_claims, None)
                .with_intelligence(Some(&contacts), Some(&[])),
            &mut remembered_budget,
            &mut remembered_intents,
        );
        assert_eq!(
            remembered_intents,
            vec![Intent::TrainAt {
                building: BuildingId(0),
                kind: UnitKind::Sentinel,
            }],
            "remembered route danger must release the unusable restoration fund into the missing core fighter"
        );
        assert_eq!(remembered_budget, 0);
    }

    #[test]
    fn ready_expansion_commits_the_assessed_builder_and_site_before_other_spending() {
        let home = TilePos::new(1, 1);
        let extractor = TilePos::new(30, 16);
        let mut obs = completed_tree();
        obs.map_width = 48;
        obs.map_height = 24;
        obs.visible = vec![true; 48 * 24];
        obs.explored = vec![true; 48 * 24];
        obs.known_rock.clear();
        add_building(
            &mut obs,
            10,
            BuildingKind::Foundry,
            TilePos::new(11, 1),
            true,
        );
        add_building(&mut obs, 11, BuildingKind::Extractor, extractor, true);
        for id in 23..27 {
            add_unit(
                &mut obs,
                id,
                UnitKind::Sentinel,
                TilePos::new(5 + i32::try_from(id - 23).unwrap(), 8),
            );
        }

        let profile = BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, 1_616_304)
            .resolve_profile();
        let mut dials = Dials::scripted(
            &profile,
            DifficultyTuning::for_level(BotDifficulty::Standard),
        );
        dials.extractors = false;
        dials.upgrades = false;
        dials.harvester_target = 4;
        dials.scouting = false;
        let fund = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundry has construction stats")
            .cost
            + TECH_RESERVE;
        obs.scrap = fund;
        let public_map = expansion_briefing(&obs, home, TilePos::new(44, 20));
        let claims = ConstructionClaims {
            player_facing: true,
            enlisted: &[],
            reserved: &[],
        };

        let mut policy = UtilityPolicy::new();
        let assessed = player_expansion_assessment(
            &policy,
            &dials,
            &obs,
            home,
            &public_map,
            None,
            ExpansionDecisionState {
                spendable_scrap: fund,
                same_think_intents: &[],
            },
        )
        .expect("the supported Extractor makes one safe expansion worthwhile");
        assert_eq!(assessed.disposition, expansion::ExpansionDisposition::Build);

        let mut production_budget = fund;
        let mut production_intents = Vec::new();
        let promised = policy.production_with_air_demand(
            &dials,
            &obs,
            ProductionContext::new(home, claims, None).with_public_map(Some(&public_map)),
            &mut production_budget,
            &mut production_intents,
        );
        assert!(promised);
        assert!(matches!(
            production_intents.as_slice(),
            [Intent::BuildWith {
                builder,
                kind: BuildingKind::Foundry,
                anchor,
            }] if *builder == assessed.plan.builder
                && *anchor == assessed.plan.anchor
                && UtilityPolicy::foundry_supports_extractor(*anchor, extractor)
        ));
        assert_eq!(production_budget, TECH_RESERVE);

        let mut adjudicated_budget = fund;
        let mut adjudicated_intents = Vec::new();
        UtilityPolicy::new().production_with_air_demand(
            &dials,
            &obs,
            ProductionContext::new(home, claims, None)
                .with_public_map(Some(&public_map))
                .with_fresh_foundry_admission(FreshFoundryAdmission::Adjudicated),
            &mut adjudicated_budget,
            &mut adjudicated_intents,
        );
        assert!(adjudicated_intents.iter().all(|intent| !matches!(
            intent,
            Intent::BuildWith {
                kind: BuildingKind::Foundry,
                ..
            }
        )));
    }

    struct SavedFoundryFixture {
        home: TilePos,
        obs: Observation,
        dials: Dials,
        public_map: PublicMapBriefing,
    }

    fn saved_foundry_fixture() -> SavedFoundryFixture {
        let home = TilePos::new(1, 1);
        let extractor = TilePos::new(30, 16);
        let mut obs = completed_tree();
        obs.map_width = 48;
        obs.map_height = 24;
        obs.visible = vec![true; 48 * 24];
        obs.explored = obs.visible.clone();
        obs.known_rock.clear();
        obs.known_scrap.clear();
        add_building(
            &mut obs,
            10,
            BuildingKind::Foundry,
            TilePos::new(11, 1),
            true,
        );
        add_building(&mut obs, 11, BuildingKind::Extractor, extractor, true);
        for id in 23..27 {
            add_unit(
                &mut obs,
                id,
                UnitKind::Sentinel,
                TilePos::new(5 + i32::try_from(id - 23).unwrap(), 8),
            );
        }
        let repairer = obs
            .my_units
            .iter_mut()
            .find(|unit| unit.id == UnitId(4))
            .expect("fixture has the repair Harvester");
        repairer.tile = TilePos::new(19, 8);
        repairer.idle = false;
        repairer.repairing = true;
        let salvager = obs
            .my_units
            .iter_mut()
            .find(|unit| unit.id == UnitId(5))
            .expect("fixture has the salvage Harvester");
        salvager.tile = TilePos::new(18, 8);
        salvager.idle = false;
        salvager.salvaging = Some(BuildingId(1));
        obs.my_buildings[1].hp /= 2;

        let profile = BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, 1_616_304)
            .resolve_profile();
        let mut dials = Dials::scripted(
            &profile,
            DifficultyTuning::for_level(BotDifficulty::Standard),
        );
        dials.extractors = false;
        dials.upgrades = false;
        dials.harvester_target = 4;
        dials.scouting = false;
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundry has construction stats")
            .cost;
        obs.scrap = foundry_cost - 1;
        let public_map = expansion_briefing(&obs, home, TilePos::new(44, 20));
        SavedFoundryFixture {
            home,
            obs,
            dials,
            public_map,
        }
    }

    fn begin_foundry_saving(
        fixture: &SavedFoundryFixture,
    ) -> (UtilityPolicy, FoundrySavingCommitment, Vec<Intent>) {
        let assessment = player_expansion_assessment(
            &UtilityPolicy::new(),
            &fixture.dials,
            &fixture.obs,
            fixture.home,
            &fixture.public_map,
            None,
            ExpansionDecisionState {
                spendable_scrap: fixture.obs.scrap,
                same_think_intents: &[],
            },
        )
        .expect("the project is safe and worthwhile before its last scrap arrives");
        assert_eq!(
            assessment.disposition,
            expansion::ExpansionDisposition::Build
        );

        let context = StrategicUtilityContext::new(&[], &[], &[], &fixture.public_map, Vec::new());
        let mut policy = UtilityPolicy::new();
        let intents =
            policy.think_with_intelligence(&fixture.dials, &fixture.obs, &[], &[], context);
        let saving = policy
            .foundry_saving
            .clone()
            .expect("the accepted partial fund freezes one exact expansion");
        (policy, saving, intents)
    }

    #[test]
    fn operation_priority_follows_the_foundry_acceptance_order() {
        let fixture = saved_foundry_fixture();
        let (policy, saving, _) = begin_foundry_saving(&fixture);

        assert!(
            policy.operation_precedes_foundry_saving(saving.accepted_at),
            "an operation admitted earlier in the same normal pass keeps its priority"
        );
        assert!(!policy.operation_precedes_foundry_saving(saving.accepted_at + 1));
    }

    #[test]
    fn partial_foundry_fund_stops_paid_repairs_and_all_voluntary_spending() {
        let mut fixture = saved_foundry_fixture();
        add_unit(
            &mut fixture.obs,
            101,
            UnitKind::Tender,
            fixture.home.offset(2, 2),
        );
        add_unit(
            &mut fixture.obs,
            102,
            UnitKind::Bombard,
            fixture.home.offset(3, 2),
        );
        fixture
            .obs
            .my_units
            .iter_mut()
            .find(|unit| unit.id == UnitId(102))
            .expect("the support patient was added")
            .hp /= 2;
        assert!(fixture.dials.adaptive_composition && fixture.dials.repair);
        let mut gross_bank_control = Vec::new();
        UtilityPolicy::new().mobile_support(
            &fixture.dials,
            &fixture.obs,
            true,
            fixture.obs.scrap,
            &mut gross_bank_control,
        );
        assert!(gross_bank_control.iter().any(|intent| matches!(
            intent,
            Intent::RepairUnits { welders, target }
                if welders == &[UnitId(101)] && *target == UnitId(102)
        )));
        let (_, saving, intents) = begin_foundry_saving(&fixture);

        assert!(
            intents.iter().all(|intent| !matches!(
                intent,
                Intent::TrainAt { .. }
                    | Intent::Build { .. }
                    | Intent::BuildWith { .. }
                    | Intent::Upgrade { .. }
            )),
            "a partial Foundry fund cannot be spent: {intents:?}"
        );
        assert!(intents.iter().any(|intent| matches!(
            intent,
            Intent::StopUnits { units } if units.contains(&UnitId(4))
        )));
        assert!(
            intents
                .iter()
                .all(|intent| !matches!(intent, Intent::RepairUnits { .. })),
            "mobile support cannot reuse the partial Foundry fund"
        );
        assert!(![UnitId(4), UnitId(5)].contains(&saving.plan.builder));
        for unavailable in [UnitId(4), UnitId(5)] {
            let unavailable = fixture
                .obs
                .my_units
                .iter()
                .find(|unit| unit.id == unavailable)
                .expect("the unavailable builder remains observed");
            let selected = fixture
                .obs
                .my_units
                .iter()
                .find(|unit| unit.id == saving.plan.builder)
                .expect("the selected builder remains observed");
            assert!(
                unavailable.tile.manhattan(saving.plan.anchor)
                    < selected.tile.manhattan(saving.plan.anchor),
                "a nearer repairing or salvaging worker must yield to the farther free builder"
            );
        }
    }

    #[test]
    fn saved_foundry_plan_releases_on_transition_or_invalidation() {
        let fixture = saved_foundry_fixture();
        let (policy, saving, _) = begin_foundry_saving(&fixture);

        let mut unrelated = fixture.obs.clone();
        add_building(
            &mut unrelated,
            90,
            BuildingKind::Fabricator,
            TilePos::new(40, 16),
            false,
        );
        let mut control = policy.clone();
        assert_eq!(
            control.validated_foundry_saving(&unrelated, true),
            saving.required_scrap,
            "unrelated construction must not release the exact expansion"
        );
        assert_eq!(control.foundry_saving.as_ref(), Some(&saving));
        assert!(control.foundry_builder_lease(&unrelated).is_some());

        let mut foundry_appeared = fixture.obs.clone();
        add_building(
            &mut foundry_appeared,
            91,
            BuildingKind::Foundry,
            saving.plan.anchor,
            false,
        );

        let mut builder_started = fixture.obs.clone();
        let builder = builder_started
            .my_units
            .iter_mut()
            .find(|unit| unit.id == saving.plan.builder)
            .expect("the saved builder remains observed");
        builder.idle = false;
        builder.founding = Some((BuildingKind::Foundry, saving.plan.anchor));

        let mut builder_disappeared = fixture.obs.clone();
        builder_disappeared
            .my_units
            .retain(|unit| unit.id != saving.plan.builder);

        let mut builder_lost_capability = fixture.obs.clone();
        builder_lost_capability
            .my_units
            .iter_mut()
            .find(|unit| unit.id == saving.plan.builder)
            .expect("the saved builder remains observed")
            .kind = UnitKind::Sentinel;

        let mut builder_started_salvaging = fixture.obs.clone();
        let builder = builder_started_salvaging
            .my_units
            .iter_mut()
            .find(|unit| unit.id == saving.plan.builder)
            .expect("the saved builder remains observed");
        builder.idle = false;
        builder.salvaging = Some(BuildingId(1));

        let mut builder_started_repairing = fixture.obs.clone();
        let builder = builder_started_repairing
            .my_units
            .iter_mut()
            .find(|unit| unit.id == saving.plan.builder)
            .expect("the saved builder remains observed");
        builder.idle = false;
        builder.repairing = true;

        let mut builder_acquired_queue = fixture.obs.clone();
        builder_acquired_queue.my_queued_units = vec![saving.plan.builder];

        let mut blacklisted = policy.clone();
        blacklisted.dead_anchors.push(saving.plan.anchor);

        for (reason, mut candidate, observation) in [
            (
                "the exact Foundry appeared",
                policy.clone(),
                foundry_appeared,
            ),
            (
                "the builder began founding",
                policy.clone(),
                builder_started,
            ),
            (
                "the builder disappeared",
                policy.clone(),
                builder_disappeared,
            ),
            (
                "the builder lost construction capability",
                policy.clone(),
                builder_lost_capability,
            ),
            (
                "the builder began salvaging",
                policy.clone(),
                builder_started_salvaging,
            ),
            (
                "the builder began repairing",
                policy.clone(),
                builder_started_repairing,
            ),
            (
                "the builder acquired a queued program",
                policy.clone(),
                builder_acquired_queue,
            ),
            (
                "the anchor was blacklisted",
                blacklisted,
                fixture.obs.clone(),
            ),
        ] {
            assert_eq!(
                candidate.validated_foundry_saving(&observation, true),
                0,
                "{reason}"
            );
            assert!(candidate.foundry_saving.is_none(), "{reason}");
            assert!(
                candidate.foundry_builder_lease(&observation).is_none(),
                "{reason}"
            );
        }
    }

    #[test]
    fn conflicting_foundry_import_reports_exact_blockage_without_a_scalar_fallback() {
        let fixture = saved_foundry_fixture();
        let (mut policy, saving, _) = begin_foundry_saving(&fixture);
        let mut commitments = PolicyCommitments::new(&fixture.obs, 0, &[], &[]);
        let prior_owner = commitments.next_legacy_owner();
        commitments
            .ledger
            .claim_builder(prior_owner, saving.plan.builder)
            .expect("the prior channel can claim the otherwise-free builder");

        let outcome = commitments.import_foundry_saving(&saving);

        assert!(matches!(
            outcome,
            FoundrySavingImport::Blocked(ClaimConflict::Unit {
                unit,
                existing,
            }) if unit == saving.plan.builder && existing.owner == prior_owner
        ));
        assert!(commitments.foundry_saving_owner.is_none());
        assert!(commitments.foundry_saving_blocked());
        assert_eq!(commitments.ledger.unit_claims().len(), 1);
        assert_eq!(commitments.ledger.unit_claims()[0].owner, prior_owner);
        assert_eq!(
            commitments.ledger.unit_claims()[0].unit,
            saving.plan.builder
        );
        assert!(commitments.ledger.site_claims().is_empty());
        assert!(commitments.ledger.holds().is_empty());
        assert_eq!(commitments.available_scrap(), fixture.obs.scrap);

        let mut budget = commitments.available_scrap();
        policy.release_foundry_saving(Some(&mut commitments), &mut budget);

        assert!(policy.foundry_saving.is_none());
        assert!(commitments.foundry_saving_owner.is_none());
        assert!(!commitments.foundry_saving_blocked());
        assert!(commitments.ledger.holds().is_empty());
        assert!(commitments.ledger.site_claims().is_empty());
        assert_eq!(commitments.ledger.unit_claims().len(), 1);
        assert_eq!(commitments.ledger.unit_claims()[0].owner, prior_owner);
        assert_eq!(commitments.available_scrap(), fixture.obs.scrap);
        assert_eq!(budget, fixture.obs.scrap);
    }

    #[test]
    fn older_deferred_construction_keeps_priority_over_foundry_saving() {
        let fixture = saved_foundry_fixture();
        let (_, saving, _) = begin_foundry_saving(&fixture);
        let mut observation = fixture.obs.clone();
        observation.scrap = saving.required_scrap;
        let deferred_anchor = TilePos::new(36, 4);
        add_unit(
            &mut observation,
            100,
            UnitKind::Harvester,
            deferred_anchor.offset(-2, 0),
        );
        let deferred_builder = observation
            .my_units
            .iter_mut()
            .find(|unit| unit.id == UnitId(100))
            .expect("the deferred builder was added");
        deferred_builder.idle = false;
        deferred_builder.founding = Some((BuildingKind::Fabricator, deferred_anchor));

        let fabricator_cost = BuildingKind::Fabricator
            .base_stats()
            .construction
            .expect("Fabricators are constructible")
            .cost;
        let mut commitments = PolicyCommitments::new(&observation, 0, &[], &[]);
        commitments
            .import_deferred_claims(&observation, &[(BuildingKind::Fabricator, deferred_anchor)]);
        let deferred_owner = commitments.ledger.holds()[0].owner;
        assert_eq!(commitments.ledger.holds()[0].amount, fabricator_cost);

        commitments.import_foundry_saving(&saving);

        let saving_owner = commitments
            .foundry_saving_owner
            .expect("the later saving retains a claim on only the remaining bank");
        let deferred_hold = commitments
            .ledger
            .holds()
            .iter()
            .find(|hold| hold.owner == deferred_owner)
            .expect("the older deferred build keeps its hold");
        let saving_hold = commitments
            .ledger
            .holds()
            .iter()
            .find(|hold| hold.owner == saving_owner)
            .expect("the Foundry saving holds the remaining bank");
        assert_eq!(deferred_hold.amount, fabricator_cost);
        assert_eq!(
            saving_hold.amount,
            observation.scrap.saturating_sub(fabricator_cost)
        );
        assert_eq!(commitments.ledger.held_scrap(), observation.scrap);

        let mut budget = commitments.available_scrap();
        let outcome =
            commit_foundry_plan(&mut commitments, &mut budget, saving.plan.clone(), 0, true);

        let Some(FoundryCommitmentOutcome::Save(commitment)) = outcome else {
            panic!("a later Foundry cannot spend capital already owned by deferred construction");
        };
        assert_eq!(commitment.plan, saving.plan);
        assert_eq!(budget, 0);
        assert_eq!(commitments.ledger.holds().len(), 2);
        assert!(
            commitments
                .ledger
                .holds()
                .iter()
                .any(|hold| hold.owner == deferred_owner && hold.amount == fabricator_cost)
        );
        assert!(commitments.ledger.spending().is_empty());
    }

    #[test]
    fn scouting_cannot_steal_a_saved_foundry_builder() {
        let mut fixture = saved_foundry_fixture();
        let (mut policy, saving, _) = begin_foundry_saving(&fixture);
        let mut scout_due = fixture.obs.clone();
        scout_due.tick = crate::bot::difficulty::next_strategic_admission_tick(fixture.obs.tick);
        let original_dispatch = policy.scout_dispatch;
        let original_anchor = saving.plan.anchor;
        let original_builder = saving.plan.builder;
        let dials = &mut fixture.dials;
        dials.scouting = true;
        let unavailable_scouts: Vec<_> = scout_due
            .my_units
            .iter()
            .filter(|unit| unit.id != saving.plan.builder)
            .map(|unit| unit.id)
            .collect();
        let context = StrategicUtilityContext::new(&[], &[], &[], &fixture.public_map, Vec::new());
        let scout_intents =
            policy.think_with_intelligence(dials, &scout_due, &[], &unavailable_scouts, context);
        assert_ne!(policy.scout, Some(saving.plan.builder));
        assert!(
            scout_intents.iter().all(|intent| !matches!(
                intent,
                Intent::Scout { unit, .. } if *unit == saving.plan.builder
            )),
            "the scouting channel cannot dispatch the saved founder: {scout_intents:?}"
        );
        assert_eq!(policy.scout_dispatch, original_dispatch);
        assert_eq!(
            policy
                .foundry_saving
                .as_ref()
                .map(|current| (current.plan.anchor, current.plan.builder)),
            Some((original_anchor, original_builder)),
            "scouting admission must leave the frozen site and builder unchanged"
        );
    }

    #[test]
    fn funded_foundry_commitment_emits_exact_build_then_releases_on_dispatch() {
        let fixture = saved_foundry_fixture();
        let (mut policy, saving, _) = begin_foundry_saving(&fixture);
        let mut funded = fixture.obs.clone();
        funded.tick = crate::bot::difficulty::next_strategic_admission_tick(fixture.obs.tick);
        funded.scrap = saving.required_scrap;
        let context = StrategicUtilityContext::new(&[], &[], &[], &fixture.public_map, Vec::new())
            .with_adjudicated_fresh_foundry();
        let mut funded_intents =
            policy.think_with_intelligence(&fixture.dials, &funded, &[], &[], context);
        assert!(
            policy.foundry_saving.is_some(),
            "a funded commitment may clear only after its exact build lowers"
        );
        policy.bind_player_facing_builders(&funded, &[], &[], &[], &[], &mut funded_intents);
        assert!(
            funded_intents.iter().any(|intent| matches!(
                intent,
                Intent::BuildWith {
                    builder,
                    kind: BuildingKind::Foundry,
                    anchor,
                } if *builder == saving.plan.builder && *anchor == saving.plan.anchor
            )),
            "the funded frozen plan must dispatch unchanged: {funded_intents:?}"
        );
        let lease = policy
            .foundry_builder_lease(&funded)
            .expect("the exact lease survives until lowering emits the command");
        let commands = Executive::new().apply_with_builder_lease(
            PlayerId(0),
            &funded,
            &funded_intents,
            &[],
            Some(lease),
        );
        let emitted = commands.iter().find_map(|command| match &command.command {
            Command::Build {
                units,
                kind: BuildingKind::Foundry,
                anchor,
                ..
            } if units == &[saving.plan.builder] && *anchor == saving.plan.anchor => {
                Some((units.clone(), *anchor))
            }
            _ => None,
        });
        let (builders, anchor) = emitted.expect("the funded frozen plan lowers exactly once");
        policy.record_dispatched_foundry_build(&builders, BuildingKind::Foundry, anchor);
        assert!(policy.foundry_saving.is_none());
    }

    #[test]
    fn same_think_adjudicated_foundry_build_is_not_rederived_as_a_blocked_saving() {
        let fixture = saved_foundry_fixture();
        let (mut policy, saving, _) = begin_foundry_saving(&fixture);
        let mut funded = fixture.obs.clone();
        funded.scrap = saving.required_scrap;
        let mut commitments = PolicyCommitments::new(&funded, 0, &[], &[]);
        assert_eq!(
            commitments.import_foundry_saving(&saving),
            FoundrySavingImport::Imported
        );
        let mut budget = commitments.available_scrap();
        let exact = Intent::BuildWith {
            builder: saving.plan.builder,
            kind: BuildingKind::Foundry,
            anchor: saving.plan.anchor,
        };
        let mut intents = vec![exact.clone()];

        let promised = policy.production_with_commitments(
            &fixture.dials,
            &funded,
            ProductionContext::new(
                fixture.home,
                ConstructionClaims {
                    player_facing: true,
                    enlisted: &[],
                    reserved: &[],
                },
                None,
            )
            .with_public_map(Some(&fixture.public_map))
            .with_fresh_foundry_admission(FreshFoundryAdmission::Adjudicated),
            &mut budget,
            Some(&mut commitments),
            &mut intents,
        );

        assert!(promised);
        assert_eq!(intents, vec![exact]);
        assert_eq!(
            policy
                .foundry_saving
                .as_ref()
                .and_then(|current| current.blocked_since),
            None
        );
    }

    #[test]
    fn same_think_forecast_funded_foundry_save_survives_current_only_utility() {
        let fixture = saved_foundry_fixture();
        let mut observation = fixture.obs.clone();
        observation.scrap = 0;
        let mut policy = UtilityPolicy::new();
        let available_builders: Vec<_> = policy
            .construction_builders(&observation, &[], &[])
            .into_iter()
            .map(|builder| builder.id)
            .collect();
        let resources = ResourceSnapshot::from_observation(&observation);
        let proposal = policy
            .fresh_foundry_proposal(
                &fixture.dials,
                &observation,
                &resources,
                FreshFoundryProposalContext {
                    home: fixture.home,
                    available_builders: &available_builders,
                    combat_core_exclusions: &[],
                    unit_contacts: &[],
                    building_contacts: &[],
                    public_map: &fixture.public_map,
                    same_think_intents: &[],
                    current_scrap: observation.scrap,
                    protected_reserve: 0,
                },
            )
            .expect("bounded future income makes the exact safe proposal quotable");
        let mut intents = Vec::new();
        policy
            .commit_adjudicated_foundry(proposal, observation.tick, &mut intents)
            .expect("there is no prior expansion obligation");
        let accepted = policy
            .foundry_saving
            .clone()
            .expect("forecast acceptance freezes the exact proposal");
        let mut commitments = PolicyCommitments::new(&observation, 0, &[], &[]);
        assert_eq!(
            commitments.import_foundry_saving(&accepted),
            FoundrySavingImport::Imported
        );
        let mut budget = commitments.available_scrap();

        let promised = policy.production_with_commitments(
            &fixture.dials,
            &observation,
            ProductionContext::new(
                fixture.home,
                ConstructionClaims {
                    player_facing: true,
                    enlisted: &[],
                    reserved: &[],
                },
                None,
            )
            .with_public_map(Some(&fixture.public_map))
            .with_fresh_foundry_admission(FreshFoundryAdmission::Adjudicated),
            &mut budget,
            Some(&mut commitments),
            &mut intents,
        );

        assert!(promised);
        assert!(intents.is_empty());
        assert_eq!(policy.foundry_saving.as_ref(), Some(&accepted));
        assert_eq!(budget, 0);
    }

    #[test]
    fn forecast_funded_foundry_keeps_its_fixed_horizon_until_income_disappears() {
        let fixture = saved_foundry_fixture();
        let mut accepted_observation = fixture.obs.clone();
        accepted_observation.scrap = 0;
        for (id, anchor) in [
            (100, TilePos::new(2, 18)),
            (101, TilePos::new(6, 18)),
            (102, TilePos::new(10, 18)),
            (103, TilePos::new(14, 18)),
        ] {
            add_building(
                &mut accepted_observation,
                id,
                BuildingKind::Reclaimer,
                anchor,
                true,
            );
        }
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let deadline = accepted_observation.tick.saturating_add(
            expansion_economy(
                &fixture.dials,
                &accepted_observation,
                accepted_observation.scrap,
                Reserve::Exact(0),
            )
            .horizon_ticks(),
        );
        assert!(
            ResourceSnapshot::from_observation(&accepted_observation)
                .forecast()
                .income_through(deadline)
                .amount()
                >= foundry_cost,
            "the completed sources must honestly cover the accepted forecast"
        );
        let available_builders: Vec<_> = UtilityPolicy::new()
            .construction_builders(&accepted_observation, &[], &[])
            .into_iter()
            .map(|builder| builder.id)
            .collect();
        let resources = ResourceSnapshot::from_observation(&accepted_observation);
        let proposal = UtilityPolicy::new()
            .fresh_foundry_proposal(
                &fixture.dials,
                &accepted_observation,
                &resources,
                FreshFoundryProposalContext {
                    home: fixture.home,
                    available_builders: &available_builders,
                    combat_core_exclusions: &[],
                    unit_contacts: &[],
                    building_contacts: &[],
                    public_map: &fixture.public_map,
                    same_think_intents: &[],
                    current_scrap: 0,
                    protected_reserve: 0,
                },
            )
            .expect("the completed-source forecast supports one exact expansion");
        assert_eq!(proposal.forecast_deadline(), deadline);
        assert_eq!(
            proposal.adjudicated_commit(),
            AdjudicatedFoundryCommit::Save
        );
        let mut policy = UtilityPolicy::new();
        policy
            .commit_adjudicated_foundry(proposal, accepted_observation.tick, &mut Vec::new())
            .expect("there is no prior expansion obligation");

        let claims = ConstructionClaims {
            player_facing: true,
            enlisted: &[],
            reserved: &[],
        };
        let advance = |policy: &mut UtilityPolicy, observation: &Observation| {
            let mut commitments = PolicyCommitments::new(observation, 0, &[], &[]);
            assert_eq!(
                commitments.import_foundry_saving(
                    policy
                        .foundry_saving
                        .as_ref()
                        .expect("the exact forecast-backed plan remains present")
                ),
                FoundrySavingImport::Imported
            );
            let mut budget = commitments.available_scrap();
            let mut intents = Vec::new();
            let promised = policy.production_with_commitments(
                &fixture.dials,
                observation,
                ProductionContext::new(fixture.home, claims, None)
                    .with_public_map(Some(&fixture.public_map))
                    .with_fresh_foundry_admission(FreshFoundryAdmission::Adjudicated),
                &mut budget,
                Some(&mut commitments),
                &mut intents,
            );
            assert!(promised);
            assert!(intents.iter().all(|intent| !matches!(
                intent,
                Intent::BuildWith {
                    kind: BuildingKind::Foundry,
                    ..
                }
            )));
        };

        let mut shortly_after_acceptance = accepted_observation.clone();
        shortly_after_acceptance.tick = 8;
        advance(&mut policy, &shortly_after_acceptance);
        assert_eq!(
            policy
                .foundry_saving
                .as_ref()
                .and_then(|saving| saving.blocked_since),
            None,
            "current-only repricing must not reopen an honestly forecast-backed save"
        );

        let mut intact = accepted_observation.clone();
        intact.tick = FOUNDRY_RECOVERY_TICKS.saturating_add(16);
        let remaining = ResourceSnapshot::from_observation(&intact)
            .forecast()
            .income_through(deadline)
            .amount()
            .min(foundry_cost);
        intact.scrap = foundry_cost.saturating_sub(remaining);
        let mut intact_policy = policy.clone();
        advance(&mut intact_policy, &intact);
        assert_eq!(
            intact_policy
                .foundry_saving
                .as_ref()
                .and_then(|saving| saving.blocked_since),
            None,
            "accrued bank plus the surviving fixed-horizon forecast keeps the exact save viable"
        );

        let mut lost_sources = intact.clone();
        lost_sources
            .my_buildings
            .retain(|building| building.kind != BuildingKind::Reclaimer);
        lost_sources
            .my_queues
            .truncate(lost_sources.my_buildings.len());
        let surviving = ResourceSnapshot::from_observation(&lost_sources)
            .forecast()
            .income_through(deadline)
            .amount();
        assert!(
            lost_sources.scrap.saturating_add(surviving) < foundry_cost,
            "removing the accepted income premise must make the fixed quote unaffordable"
        );
        let mut lost_policy = policy;
        advance(&mut lost_policy, &lost_sources);
        assert_eq!(
            lost_policy
                .foundry_saving
                .as_ref()
                .and_then(|saving| saving.blocked_since),
            Some(lost_sources.tick),
            "a disappeared completed-income premise starts bounded recovery"
        );
    }

    #[test]
    fn current_funded_foundry_build_can_coexist_with_free_mobile_support() {
        let mut fixture = saved_foundry_fixture();
        fixture.dials.minimum_core_equivalents = 0;
        add_unit(
            &mut fixture.obs,
            101,
            UnitKind::Tender,
            fixture.home.offset(2, 2),
        );
        add_unit(
            &mut fixture.obs,
            102,
            UnitKind::Bombard,
            fixture.home.offset(3, 2),
        );
        fixture
            .obs
            .my_units
            .iter_mut()
            .find(|unit| unit.id == UnitId(102))
            .expect("the support patient was added")
            .hp /= 2;
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        fixture.obs.scrap = foundry_cost.saturating_add(UnitKind::Sentinel.stats().cost);
        let mut policy = UtilityPolicy::new();
        let available_builders: Vec<_> = policy
            .construction_builders(&fixture.obs, &[], &[])
            .into_iter()
            .map(|builder| builder.id)
            .collect();
        let resources = ResourceSnapshot::from_observation(&fixture.obs);
        let proposal = policy
            .fresh_foundry_proposal(
                &fixture.dials,
                &fixture.obs,
                &resources,
                FreshFoundryProposalContext {
                    home: fixture.home,
                    available_builders: &available_builders,
                    combat_core_exclusions: &[],
                    unit_contacts: &[],
                    building_contacts: &[],
                    public_map: &fixture.public_map,
                    same_think_intents: &[],
                    current_scrap: fixture.obs.scrap,
                    protected_reserve: 0,
                },
            )
            .expect("the current bank supports the exact expansion");
        assert_eq!(
            proposal.adjudicated_commit(),
            AdjudicatedFoundryCommit::Build
        );
        let expected_anchor = proposal.anchor();
        let expected_builder = proposal.builder();
        let mut prelude = Vec::new();
        policy
            .commit_adjudicated_foundry(proposal, fixture.obs.tick, &mut prelude)
            .expect("there is no prior expansion obligation");

        let intents = policy.think_with_intelligence(
            &fixture.dials,
            &fixture.obs,
            &[],
            &[],
            StrategicUtilityContext::new(&[], &[], &[], &fixture.public_map, prelude)
                .with_adjudicated_fresh_foundry(),
        );

        assert!(intents.iter().any(|intent| matches!(
            intent,
            Intent::BuildWith {
                builder,
                kind: BuildingKind::Foundry,
                anchor,
            } if *builder == expected_builder && *anchor == expected_anchor
        )));
        assert!(intents.iter().any(|intent| matches!(
            intent,
            Intent::RepairUnits { welders, target }
                if welders == &[UnitId(101)] && *target == UnitId(102)
        )));
    }

    #[test]
    fn mirrored_foundry_builder_lease_lowers_the_exact_world_anchor() {
        use crate::bot::orient::Orientation;

        let fixture = saved_foundry_fixture();
        let (mut policy, saving, _) = begin_foundry_saving(&fixture);
        let mut funded = fixture.obs.clone();
        funded.tick = crate::bot::difficulty::next_strategic_admission_tick(funded.tick);
        funded.scrap = saving.required_scrap;
        let context = StrategicUtilityContext::new(&[], &[], &[], &fixture.public_map, Vec::new());
        let mut intents =
            policy.think_with_intelligence(&fixture.dials, &funded, &[], &[], context);
        policy.bind_player_facing_builders(&funded, &[], &[], &[], &[], &mut intents);
        let canonical_lease = policy
            .foundry_builder_lease(&funded)
            .expect("the funded plan retains its canonical lease until lowering");
        let orientation = Orientation::for_home(&funded, TilePos::new(44, 20));
        assert!(!orientation.is_identity());
        let world = orientation.observe(&funded);
        let world_anchor =
            orientation.anchor(saving.plan.anchor, BuildingKind::Foundry.base_stats().size);
        let world_lease = BuilderLease::new(
            canonical_lease.builder(),
            canonical_lease.kind(),
            world_anchor,
        );

        let commands = Executive::new().apply_with_builder_lease(
            PlayerId(0),
            &world,
            &orientation.emit(intents),
            &[],
            Some(world_lease),
        );

        assert!(commands.iter().any(|command| matches!(
            &command.command,
            Command::Build {
                units,
                kind: BuildingKind::Foundry,
                anchor,
                ..
            } if units == &[saving.plan.builder] && *anchor == world_anchor
        )));
    }

    #[test]
    fn repeated_underfunded_foundry_thinks_keep_the_exact_accepted_plan() {
        let fixture = saved_foundry_fixture();
        let (mut policy, saving, _) = begin_foundry_saving(&fixture);
        let mut underfunded = fixture.obs.clone();
        let cadence = crate::bot::difficulty::next_strategic_admission_tick(underfunded.tick);

        for tick in [cadence, cadence.saturating_mul(2)] {
            underfunded.tick = tick;
            let context =
                StrategicUtilityContext::new(&[], &[], &[], &fixture.public_map, Vec::new());
            let intents =
                policy.think_with_intelligence(&fixture.dials, &underfunded, &[], &[], context);

            assert_eq!(policy.foundry_saving.as_ref(), Some(&saving));
            assert!(intents.iter().all(|intent| !matches!(
                intent,
                Intent::TrainAt { .. }
                    | Intent::Build { .. }
                    | Intent::BuildWith { .. }
                    | Intent::Upgrade { .. }
            )));
        }
    }

    #[test]
    fn saved_foundry_requirement_ratchets_up_with_a_new_safety_guard() {
        let fixture = saved_foundry_fixture();
        let (mut policy, saving, _) = begin_foundry_saving(&fixture);
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let shallow_guard = UnitKind::Sentinel.stats().cost;
        assert_eq!(saving.required_scrap, foundry_cost);

        let mut guarded = fixture.obs.clone();
        guarded.tick = crate::bot::difficulty::next_strategic_admission_tick(guarded.tick);
        guarded.scrap = saving.required_scrap;
        let mut commitments = PolicyCommitments::new(&guarded, 0, &[], &[]);
        commitments.import_foundry_saving(&saving);
        let mut budget = commitments.available_scrap();
        let mut intents = Vec::new();
        let claims = ConstructionClaims {
            player_facing: true,
            enlisted: &[],
            reserved: &[],
        };

        let promised = policy.production_with_commitments(
            &fixture.dials,
            &guarded,
            ProductionContext::new(fixture.home, claims, None)
                .with_public_map(Some(&fixture.public_map))
                .with_voluntary_scrap_guard(Reserve::Exact(shallow_guard)),
            &mut budget,
            Some(&mut commitments),
            &mut intents,
        );

        assert!(promised);
        assert!(intents.iter().all(|intent| !matches!(
            intent,
            Intent::BuildWith {
                kind: BuildingKind::Foundry,
                ..
            }
        )));
        let retained = policy
            .foundry_saving
            .as_ref()
            .expect("the plan remains saved until it covers the new guard");
        assert_eq!(retained.plan, saving.plan);
        assert_eq!(
            retained.required_scrap,
            foundry_cost.saturating_add(shallow_guard)
        );
        assert_eq!(budget, 0);
    }

    #[test]
    fn saved_foundry_releases_its_hold_for_newly_required_preparation() {
        let fixture = saved_foundry_fixture();
        let (mut policy, saving, _) = begin_foundry_saving(&fixture);
        let mut underprotected = fixture.obs.clone();
        underprotected.tick =
            crate::bot::difficulty::next_strategic_admission_tick(underprotected.tick);
        underprotected
            .my_units
            .retain(|unit| ![UnitId(25), UnitId(26)].contains(&unit.id));
        underprotected.known_scrap = vec![(saving.plan.anchor.offset(5, 0), 10_000)];
        underprotected.scrap = saving.required_scrap - 1;
        let (foundries, pending_foundries) = UtilityPolicy::projected_foundries(&underprotected);
        assert_eq!(pending_foundries, 0);
        let builder = underprotected
            .my_units
            .iter()
            .find(|unit| unit.id == saving.plan.builder)
            .expect("the saved builder remains available");
        let builders = [builder];
        let assessment = policy.player_facing_foundry_assessment(
            &fixture.dials,
            &underprotected,
            FoundryAssessmentContext {
                claim: FoundryClaimContext {
                    home: fixture.home,
                    projected_foundries: &foundries,
                    builders: &builders,
                    support_extractors: true,
                    ordinary_frontiers: true,
                    unit_contacts: None,
                    building_contacts: None,
                },
                public_map: &fixture.public_map,
                combat_core_exclusions: &[],
                spendable_scrap: underprotected.scrap,
                voluntary_scrap_guard: Reserve::Exact(0),
                required_anchor: Some(saving.plan.anchor),
            },
            &[],
        );
        assert!(
            assessment.as_ref().is_some_and(|assessment| matches!(
                assessment.disposition,
                expansion::ExpansionDisposition::Prepare { .. }
            )),
            "removing the screen must make the accepted site require preparation: {assessment:?}"
        );
        let mut commitments = PolicyCommitments::new(&underprotected, 0, &[], &[]);
        commitments.import_foundry_saving(&saving);
        let mut budget = commitments.available_scrap();
        let mut intents = Vec::new();
        let claims = ConstructionClaims {
            player_facing: true,
            enlisted: &[],
            reserved: &[],
        };

        let promised = policy.production_with_commitments(
            &fixture.dials,
            &underprotected,
            ProductionContext::new(fixture.home, claims, None)
                .with_public_map(Some(&fixture.public_map))
                .with_voluntary_scrap_guard(Reserve::Exact(0)),
            &mut budget,
            Some(&mut commitments),
            &mut intents,
        );

        assert!(promised);
        assert!(intents.iter().any(|intent| matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Sentinel,
                ..
            }
        )));
        assert!(intents.iter().all(|intent| !matches!(
            intent,
            Intent::BuildWith {
                kind: BuildingKind::Foundry,
                ..
            }
        )));
        let reaccepted = policy
            .foundry_saving
            .as_ref()
            .expect("projected protection may reaccept the same exact plan");
        assert_eq!(
            (reaccepted.plan.anchor, reaccepted.plan.builder),
            (saving.plan.anchor, saving.plan.builder),
            "projected protection may reaccept the same exact plan after spending the safety fund"
        );
        assert_eq!(
            reaccepted.accepted_at, underprotected.tick,
            "yielding the fund for required preparation creates a new commitment order"
        );
        assert_eq!(budget, 0);
    }

    #[test]
    fn blocked_foundry_saving_releases_after_the_bounded_recovery_window() {
        let fixture = saved_foundry_fixture();
        let (mut policy, saving, _) = begin_foundry_saving(&fixture);
        let mut blocked = fixture.obs.clone();
        blocked.enemy_units.push(UnitObs {
            id: UnitId(900),
            player: PlayerId(1),
            kind: UnitKind::Sentinel,
            tile: saving.plan.anchor,
            hp: UnitKind::Sentinel.stats().max_hp,
            idle: true,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        });
        let first_blocked = crate::bot::difficulty::next_strategic_admission_tick(fixture.obs.tick);

        blocked.tick = first_blocked;
        let context = StrategicUtilityContext::new(&[], &[], &[], &fixture.public_map, Vec::new());
        let _ = policy.think_with_intelligence(&fixture.dials, &blocked, &[], &[], context);
        assert_eq!(
            policy
                .foundry_saving
                .as_ref()
                .and_then(|current| current.blocked_since),
            Some(first_blocked)
        );

        blocked.tick = first_blocked.saturating_add(FOUNDRY_RECOVERY_TICKS);
        let context = StrategicUtilityContext::new(&[], &[], &[], &fixture.public_map, Vec::new());
        let _ = policy.think_with_intelligence(&fixture.dials, &blocked, &[], &[], context);
        assert!(
            policy.foundry_saving.is_none(),
            "a continuously blocked exact plan must release its bank for replanning"
        );
    }

    #[test]
    fn recovered_foundry_saving_gets_a_fresh_bounded_recovery_window() {
        let fixture = saved_foundry_fixture();
        let (mut policy, saving, _) = begin_foundry_saving(&fixture);
        let mut blocked = fixture.obs.clone();
        blocked.enemy_units.push(UnitObs {
            id: UnitId(900),
            player: PlayerId(1),
            kind: UnitKind::Sentinel,
            tile: saving.plan.anchor,
            hp: UnitKind::Sentinel.stats().max_hp,
            idle: true,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        });
        let first_blocked = crate::bot::difficulty::next_strategic_admission_tick(fixture.obs.tick);

        blocked.tick = first_blocked;
        let context = StrategicUtilityContext::new(&[], &[], &[], &fixture.public_map, Vec::new());
        let _ = policy.think_with_intelligence(&fixture.dials, &blocked, &[], &[], context);
        assert_eq!(
            policy
                .foundry_saving
                .as_ref()
                .and_then(|current| current.blocked_since),
            Some(first_blocked)
        );

        let mut recovered = fixture.obs.clone();
        recovered.tick = crate::bot::difficulty::next_strategic_admission_tick(first_blocked);
        let context = StrategicUtilityContext::new(&[], &[], &[], &fixture.public_map, Vec::new());
        let _ = policy.think_with_intelligence(&fixture.dials, &recovered, &[], &[], context);
        let refreshed = policy
            .foundry_saving
            .as_ref()
            .expect("a valid underfunded reassessment retains the exact plan");
        assert_eq!(refreshed.plan, saving.plan);
        assert_eq!(refreshed.accepted_at, saving.accepted_at);
        assert_eq!(refreshed.required_scrap, saving.required_scrap);
        assert_eq!(refreshed.blocked_since, None);

        let second_blocked = crate::bot::difficulty::next_strategic_admission_tick(recovered.tick);
        blocked.tick = second_blocked;
        let context = StrategicUtilityContext::new(&[], &[], &[], &fixture.public_map, Vec::new());
        let _ = policy.think_with_intelligence(&fixture.dials, &blocked, &[], &[], context);
        assert_eq!(
            policy
                .foundry_saving
                .as_ref()
                .and_then(|current| current.blocked_since),
            Some(second_blocked)
        );

        blocked.tick = first_blocked.saturating_add(FOUNDRY_RECOVERY_TICKS);
        let context = StrategicUtilityContext::new(&[], &[], &[], &fixture.public_map, Vec::new());
        let _ = policy.think_with_intelligence(&fixture.dials, &blocked, &[], &[], context);
        let retained = policy
            .foundry_saving
            .as_ref()
            .expect("the recovered plan must outlive its original recovery deadline");
        assert_eq!(retained.plan, saving.plan);
        assert_eq!(retained.blocked_since, Some(second_blocked));

        blocked.tick = second_blocked.saturating_add(FOUNDRY_RECOVERY_TICKS);
        let context = StrategicUtilityContext::new(&[], &[], &[], &fixture.public_map, Vec::new());
        let _ = policy.think_with_intelligence(&fixture.dials, &blocked, &[], &[], context);
        assert!(
            policy.foundry_saving.is_none(),
            "the second continuous blockage must expire at its own fresh deadline"
        );
    }

    #[test]
    fn repeated_exact_claim_conflict_reaches_the_foundry_recovery_deadline() {
        let fixture = saved_foundry_fixture();
        let (mut policy, saving, _) = begin_foundry_saving(&fixture);
        let fabricator_cost = BuildingKind::Fabricator
            .base_stats()
            .construction
            .expect("Fabricators are constructible")
            .cost;
        let conflict = (BuildingKind::Fabricator, saving.plan.anchor);
        let first_blocked = crate::bot::difficulty::next_strategic_admission_tick(fixture.obs.tick);
        let mut blocked = fixture.obs.clone();
        blocked.scrap = saving.required_scrap.saturating_add(fabricator_cost);
        blocked.tick = first_blocked;

        let mut commitments = PolicyCommitments::new(&blocked, 0, &[], &[]);
        commitments.import_deferred_claims(&blocked, &[conflict]);
        assert!(matches!(
            commitments.import_foundry_saving(&saving),
            FoundrySavingImport::Blocked(ClaimConflict::Site { .. })
        ));
        let mut budget = commitments.available_scrap();
        let mut intents = Vec::new();
        let claims = ConstructionClaims {
            player_facing: true,
            enlisted: &[],
            reserved: &[],
        };
        policy.production_with_commitments(
            &fixture.dials,
            &blocked,
            ProductionContext::new(fixture.home, claims, None)
                .with_public_map(Some(&fixture.public_map))
                .with_voluntary_scrap_guard(Reserve::Exact(0)),
            &mut budget,
            Some(&mut commitments),
            &mut intents,
        );

        assert_eq!(
            policy
                .foundry_saving
                .as_ref()
                .and_then(|current| current.blocked_since),
            Some(first_blocked)
        );
        assert_eq!(
            policy.foundry_saving.as_ref().map(|saved| &saved.plan),
            Some(&saving.plan)
        );
        assert!(commitments.foundry_saving_owner.is_none());
        assert!(commitments.foundry_saving_blocked());
        assert_eq!(commitments.ledger.holds().len(), 1);
        assert_eq!(budget, saving.required_scrap);
        assert!(intents.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt { .. }
                | Intent::Build { .. }
                | Intent::BuildWith { .. }
                | Intent::Upgrade { .. }
        )));

        blocked.tick = first_blocked.saturating_add(FOUNDRY_RECOVERY_TICKS);
        let retained = policy
            .foundry_saving
            .clone()
            .expect("the exact plan remains saved before its recovery deadline");
        let mut commitments = PolicyCommitments::new(&blocked, 0, &[], &[]);
        commitments.import_deferred_claims(&blocked, &[conflict]);
        assert!(matches!(
            commitments.import_foundry_saving(&retained),
            FoundrySavingImport::Blocked(ClaimConflict::Site { .. })
        ));
        let mut budget = commitments.available_scrap();
        let mut intents = Vec::new();
        policy.production_with_commitments(
            &fixture.dials,
            &blocked,
            ProductionContext::new(fixture.home, claims, None)
                .with_public_map(Some(&fixture.public_map))
                .with_voluntary_scrap_guard(Reserve::Exact(0)),
            &mut budget,
            Some(&mut commitments),
            &mut intents,
        );

        assert!(policy.foundry_saving.is_none());
        assert!(commitments.foundry_saving_owner.is_none());
        assert!(!commitments.foundry_saving_blocked());
        assert_eq!(
            budget, saving.required_scrap,
            "the expired Foundry hold releases while the older conflicting build keeps its fund"
        );
    }

    #[test]
    fn exact_foundry_claim_recovers_without_changing_its_frozen_plan() {
        let fixture = saved_foundry_fixture();
        let (mut policy, saving, _) = begin_foundry_saving(&fixture);
        let conflict = (BuildingKind::Fabricator, saving.plan.anchor);
        let first_blocked = crate::bot::difficulty::next_strategic_admission_tick(fixture.obs.tick);
        let mut blocked = fixture.obs.clone();
        blocked.tick = first_blocked;
        blocked.scrap = saving.required_scrap.saturating_add(
            BuildingKind::Fabricator
                .base_stats()
                .construction
                .expect("Fabricators are constructible")
                .cost,
        );
        let mut commitments = PolicyCommitments::new(&blocked, 0, &[], &[]);
        commitments.import_deferred_claims(&blocked, &[conflict]);
        assert!(matches!(
            commitments.import_foundry_saving(&saving),
            FoundrySavingImport::Blocked(ClaimConflict::Site { .. })
        ));
        let mut budget = commitments.available_scrap();
        let claims = ConstructionClaims {
            player_facing: true,
            enlisted: &[],
            reserved: &[],
        };
        policy.production_with_commitments(
            &fixture.dials,
            &blocked,
            ProductionContext::new(fixture.home, claims, None)
                .with_public_map(Some(&fixture.public_map))
                .with_voluntary_scrap_guard(Reserve::Exact(0)),
            &mut budget,
            Some(&mut commitments),
            &mut Vec::new(),
        );
        assert_eq!(
            policy
                .foundry_saving
                .as_ref()
                .and_then(|current| current.blocked_since),
            Some(first_blocked)
        );

        let mut recovered = fixture.obs.clone();
        recovered.tick = crate::bot::difficulty::next_strategic_admission_tick(first_blocked);
        recovered.scrap = saving.required_scrap.saturating_sub(1);
        let mut commitments = PolicyCommitments::new(&recovered, 0, &[], &[]);
        assert_eq!(
            commitments.import_foundry_saving(
                policy
                    .foundry_saving
                    .as_ref()
                    .expect("the blocked plan remains frozen")
            ),
            FoundrySavingImport::Imported
        );
        let mut budget = commitments.available_scrap();
        let mut intents = Vec::new();
        policy.production_with_commitments(
            &fixture.dials,
            &recovered,
            ProductionContext::new(fixture.home, claims, None)
                .with_public_map(Some(&fixture.public_map))
                .with_voluntary_scrap_guard(Reserve::Exact(0)),
            &mut budget,
            Some(&mut commitments),
            &mut intents,
        );

        let retained = policy
            .foundry_saving
            .as_ref()
            .expect("the underfunded recovered plan remains saved");
        assert_eq!(retained.plan, saving.plan);
        assert_eq!(retained.accepted_at, saving.accepted_at);
        assert_eq!(retained.blocked_since, None);
        assert!(commitments.foundry_saving_owner.is_some());
        assert!(!commitments.foundry_saving_blocked());
        assert!(intents.iter().all(|intent| !matches!(
            intent,
            Intent::BuildWith {
                kind: BuildingKind::Foundry,
                ..
            }
        )));
    }

    #[test]
    fn insufficient_non_saving_foundry_probe_restores_the_imported_commitment() {
        let fixture = saved_foundry_fixture();
        let (_, saving, _) = begin_foundry_saving(&fixture);
        let mut commitments = PolicyCommitments::new(&fixture.obs, 0, &[], &[]);
        commitments.import_foundry_saving(&saving);
        let before_ledger = commitments.ledger.clone();
        let before_owner = commitments.foundry_saving_owner;
        let mut budget = commitments.available_scrap();

        let outcome = commit_foundry_plan(
            &mut commitments,
            &mut budget,
            saving.plan.clone(),
            saving.required_scrap.saturating_sub(
                BuildingKind::Foundry
                    .base_stats()
                    .construction
                    .expect("Foundries are constructible")
                    .cost,
            ),
            false,
        );

        assert!(outcome.is_none());
        assert_eq!(commitments.ledger, before_ledger);
        assert_eq!(commitments.foundry_saving_owner, before_owner);
        assert_eq!(budget, 0);
    }

    #[test]
    fn ratcheted_foundry_reclaim_retains_a_releasable_replacement_owner() {
        let fixture = saved_foundry_fixture();
        let (mut policy, saving, _) = begin_foundry_saving(&fixture);
        let mut commitments = PolicyCommitments::new(&fixture.obs, 0, &[], &[]);
        commitments.import_foundry_saving(&saving);
        let imported_owner = commitments
            .foundry_saving_owner
            .expect("the imported saving owns exact revisable claims");
        let guard = UnitKind::Sentinel.stats().cost;
        let expected_requirement = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost
            .saturating_add(guard);
        let mut budget = commitments.available_scrap();

        let outcome = commit_foundry_plan(
            &mut commitments,
            &mut budget,
            saving.plan.clone(),
            guard,
            true,
        );

        let Some(FoundryCommitmentOutcome::Save(commitment)) = outcome else {
            panic!("the underfunded ratchet must replace the imported saving");
        };
        assert_eq!(commitment.plan, saving.plan);
        assert_eq!(commitment.required_scrap, expected_requirement);
        let replacement_owner = commitments
            .foundry_saving_owner
            .expect("the replacement saving retains its release handle");
        assert_eq!(replacement_owner, imported_owner);
        assert_eq!(commitments.ledger.holds().len(), 1);
        assert_eq!(commitments.ledger.unit_claims().len(), 1);
        assert_eq!(commitments.ledger.site_claims().len(), 1);
        assert!(
            commitments
                .ledger
                .holds()
                .iter()
                .all(|claim| claim.owner == replacement_owner)
        );
        assert!(
            commitments
                .ledger
                .unit_claims()
                .iter()
                .all(|claim| claim.owner == replacement_owner)
        );
        assert!(
            commitments
                .ledger
                .site_claims()
                .iter()
                .all(|claim| claim.owner == replacement_owner)
        );

        policy.release_foundry_saving(Some(&mut commitments), &mut budget);

        assert!(policy.foundry_saving.is_none());
        assert!(commitments.foundry_saving_owner.is_none());
        assert_eq!(commitments.ledger.committed_scrap(), 0);
        assert!(commitments.ledger.unit_claims().is_empty());
        assert!(commitments.ledger.site_claims().is_empty());
        assert!(commitments.ledger.producer_claims().is_empty());
        assert_eq!(commitments.available_scrap(), fixture.obs.scrap);
        assert_eq!(budget, fixture.obs.scrap);
    }

    #[test]
    fn funded_foundry_reclaim_releases_only_its_revisable_claims() {
        let fixture = saved_foundry_fixture();
        let (mut policy, saving, _) = begin_foundry_saving(&fixture);
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let guard = UnitKind::Sentinel.stats().cost;
        let required_scrap = foundry_cost.saturating_add(guard);
        let mut funded = fixture.obs.clone();
        funded.scrap = required_scrap;
        let mut commitments = PolicyCommitments::new(&funded, 0, &[], &[]);
        commitments.import_foundry_saving(&saving);
        let imported_owner = commitments
            .foundry_saving_owner
            .expect("the partial saving owns its revisable claims");
        assert_eq!(commitments.ledger.held_scrap(), saving.required_scrap);
        assert_eq!(commitments.ledger.holds().len(), 1);
        assert_eq!(commitments.ledger.holds()[0].owner, imported_owner);
        assert_eq!(commitments.ledger.unit_claims().len(), 1);
        assert_eq!(commitments.ledger.unit_claims()[0].owner, imported_owner);
        assert_eq!(commitments.ledger.site_claims().len(), 1);
        assert_eq!(commitments.ledger.site_claims()[0].owner, imported_owner);
        let mut budget = commitments.available_scrap();

        let outcome = commit_foundry_plan(
            &mut commitments,
            &mut budget,
            saving.plan.clone(),
            guard,
            true,
        );

        let Some(FoundryCommitmentOutcome::Build(commitment)) = outcome else {
            panic!("the fully funded ratchet must build the imported plan");
        };
        assert_eq!(commitment.plan, saving.plan);
        assert_eq!(commitment.required_scrap, required_scrap);
        let replacement_owner = commitments
            .foundry_saving_owner
            .expect("the funded replacement retains its release handle");
        assert_eq!(replacement_owner, imported_owner);
        assert!(commitments.ledger.holds().is_empty());
        assert_eq!(commitments.ledger.spending().len(), 1);
        assert_eq!(commitments.ledger.spending()[0].owner, replacement_owner);
        assert_eq!(commitments.ledger.spending()[0].amount, foundry_cost);
        assert_eq!(commitments.ledger.unit_claims().len(), 1);
        assert_eq!(commitments.ledger.unit_claims()[0].owner, replacement_owner);
        assert_eq!(commitments.ledger.site_claims().len(), 1);
        assert_eq!(commitments.ledger.site_claims()[0].owner, replacement_owner);
        assert_eq!(commitments.available_scrap(), guard);

        policy.release_foundry_saving(Some(&mut commitments), &mut budget);

        assert!(policy.foundry_saving.is_none());
        assert!(commitments.foundry_saving_owner.is_none());
        assert!(commitments.ledger.holds().is_empty());
        assert!(commitments.ledger.unit_claims().is_empty());
        assert!(commitments.ledger.site_claims().is_empty());
        assert_eq!(commitments.ledger.spent_scrap(), foundry_cost);
        assert_eq!(commitments.available_scrap(), guard);
        assert_eq!(budget, guard);
    }

    #[test]
    fn refused_or_mismatched_foundry_build_keeps_the_persistent_saving() {
        let fixture = saved_foundry_fixture();
        let (mut policy, saving, _) = begin_foundry_saving(&fixture);
        let lease = policy
            .foundry_builder_lease(&fixture.obs)
            .expect("the accepted plan owns one exact builder lease");
        let commands = Executive::new().apply_with_builder_lease(
            PlayerId(0),
            &fixture.obs,
            &[Intent::BuildWith {
                builder: saving.plan.builder,
                kind: BuildingKind::Foundry,
                anchor: saving.plan.anchor.offset(1, 0),
            }],
            &[],
            Some(lease),
        );
        assert!(
            commands.is_empty(),
            "the lease must refuse the wrong anchor"
        );
        assert_eq!(policy.foundry_saving.as_ref(), Some(&saving));

        policy.record_dispatched_foundry_build(
            &[saving.plan.builder],
            BuildingKind::Fabricator,
            saving.plan.anchor,
        );
        policy.record_dispatched_foundry_build(
            &[saving.plan.builder],
            BuildingKind::Foundry,
            saving.plan.anchor.offset(1, 0),
        );
        policy.record_dispatched_foundry_build(
            &[UnitId(999)],
            BuildingKind::Foundry,
            saving.plan.anchor,
        );
        assert_eq!(policy.foundry_saving.as_ref(), Some(&saving));
    }

    #[test]
    fn opening_core_loss_releases_saved_foundry_capital_for_reinforcement() {
        let fixture = saved_foundry_fixture();
        let (mut policy, _, _) = begin_foundry_saving(&fixture);
        let mut deficient = fixture.obs.clone();
        deficient.tick = crate::bot::difficulty::next_strategic_admission_tick(fixture.obs.tick);
        deficient
            .my_units
            .retain(|unit| unit.kind != UnitKind::Sentinel || unit.id.0 <= 23);
        assert_eq!(
            deficient
                .my_units
                .iter()
                .filter(|unit| unit.kind == UnitKind::Sentinel)
                .count(),
            4,
            "the Standard fixture has fallen below its five-equivalent core"
        );
        let context = StrategicUtilityContext::new(&[], &[], &[], &fixture.public_map, Vec::new());

        let intents = policy.think_with_intelligence(&fixture.dials, &deficient, &[], &[], context);

        assert!(
            policy.foundry_saving.is_none(),
            "survival release must persist beyond the current ledger"
        );
        assert!(intents.iter().any(|intent| matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Sentinel,
                ..
            }
        )));
        assert!(intents.iter().all(|intent| !matches!(
            intent,
            Intent::BuildWith {
                kind: BuildingKind::Foundry,
                ..
            }
        )));
    }

    #[test]
    fn unreachable_island_frontier_cannot_starve_the_player_facing_crucible_fund() {
        let mut obs = completed_tree();
        let crucible = obs
            .my_buildings
            .iter()
            .position(|building| building.kind == BuildingKind::Crucible)
            .unwrap();
        obs.my_buildings.remove(crucible);
        obs.my_queues.remove(crucible);
        let unreachable_frontier = TilePos::new(14, 8);
        obs.known_scrap = vec![(unreachable_frontier, 800)];
        let home = TilePos::new(1, 1);
        assert!(
            obs.my_buildings
                .iter()
                .filter(|building| building.kind == BuildingKind::Foundry)
                .all(|foundry| foundry.anchor.chebyshev(unreachable_frontier) > EXPANSION_RADIUS)
        );
        assert!(!UtilityPolicy::ground_route_known(
            &obs,
            home,
            unreachable_frontier
        ));

        let dials = Dials::balanced();
        let policy = UtilityPolicy::new();
        let crucible_fund = BuildingKind::Crucible
            .base_stats()
            .construction
            .expect("Crucible has construction stats")
            .cost
            + TECH_RESERVE;
        let foundry_fund = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundry has construction stats")
            .cost
            + TECH_RESERVE;

        assert_eq!(
            capital_reserve_for(&policy, &dials, &obs, home, true),
            crucible_fund,
            "a frontier the construction channel cannot reach must yield to the next legal tech rung"
        );
        assert_eq!(
            capital_reserve_for(&policy, &dials, &obs, home, false),
            foundry_fund,
            "the profile-free Overseer's historical reserve remains route-agnostic"
        );
    }

    #[test]
    fn active_air_work_buys_one_ordinary_airworks_after_the_tree_stands() {
        let mut obs = completed_tree();
        let cost = BuildingKind::Airworks
            .base_stats()
            .construction
            .expect("Airworks has construction stats")
            .cost;
        obs.scrap = cost + TECH_RESERVE;

        let (budget, intents) = capacity_decision(
            &obs,
            Some(AIRWORKS_ASSEMBLY_HORIZON_TICKS.saturating_add(1)),
        );
        let builds = airworks_builds(&intents);

        assert_eq!(builds.len(), 1, "one additional factory should be paid");
        assert!(
            !obs.my_buildings
                .iter()
                .any(|building| building.anchor == builds[0]),
            "capacity must use a fresh legal footprint"
        );
        assert_eq!(budget, TECH_RESERVE);
        assert!(
            intents
                .iter()
                .all(|intent| !matches!(intent, Intent::TrainAt { .. })),
            "the exact capacity fund must not be skimmed by routine production"
        );
    }

    #[test]
    fn active_airworks_capacity_preserves_the_opening_reinforcement_guard() {
        let mut obs = completed_tree();
        let demand = Some(AIRWORKS_ASSEMBLY_HORIZON_TICKS.saturating_add(1));
        let airworks_cost = BuildingKind::Airworks
            .base_stats()
            .construction
            .expect("Airworks has construction stats")
            .cost;
        let sentinel_cost = UnitKind::Sentinel.stats().cost;

        obs.scrap = airworks_cost + sentinel_cost - 1;
        let (short_budget, short) =
            guarded_capacity_decision(&obs, demand, sentinel_cost, Vec::new());
        assert!(airworks_builds(&short).is_empty());
        assert_eq!(short_budget, obs.scrap);

        obs.scrap += 1;
        let (exact_budget, exact) =
            guarded_capacity_decision(&obs, demand, sentinel_cost, Vec::new());
        assert_eq!(airworks_builds(&exact).len(), 1);
        assert_eq!(exact_budget, sentinel_cost);
        assert!(
            exact
                .iter()
                .all(|intent| !matches!(intent, Intent::TrainAt { .. })),
            "the exact unqueued reinforcement fund remains untouched after capacity: {exact:?}"
        );

        obs.scrap = airworks_cost;
        obs.my_queues[0] = vec![UnitKind::Sentinel];
        let (queued_budget, queued) = guarded_capacity_decision(&obs, demand, 0, Vec::new());
        assert_eq!(airworks_builds(&queued).len(), 1);
        assert_eq!(queued_budget, 0);

        obs.my_queues[0].clear();
        let planned_sentinel = Intent::TrainAt {
            building: obs.my_buildings[0].id,
            kind: UnitKind::Sentinel,
        };
        let (planned_budget, planned) =
            guarded_capacity_decision(&obs, demand, 0, vec![planned_sentinel.clone()]);
        assert_eq!(airworks_builds(&planned).len(), 1);
        assert_eq!(planned_budget, 0);
        assert!(planned.contains(&planned_sentinel));
    }

    #[test]
    fn inactive_or_single_factory_work_never_raises_speculative_capacity() {
        let mut obs = completed_tree();
        obs.scrap = 2_000;

        for demand in [None, Some(0), Some(AIRWORKS_ASSEMBLY_HORIZON_TICKS)] {
            let (_, intents) = capacity_decision(&obs, demand);
            assert!(
                airworks_builds(&intents).is_empty(),
                "demand {demand:?} is already served by the first Airworks"
            );
        }
    }

    #[test]
    fn a_partial_capacity_fund_is_protected_from_routine_units() {
        let mut obs = completed_tree();
        let cost = BuildingKind::Airworks
            .base_stats()
            .construction
            .expect("Airworks has construction stats")
            .cost;
        obs.scrap = cost + TECH_RESERVE - 1;

        let (budget, intents) = capacity_decision(
            &obs,
            Some(AIRWORKS_ASSEMBLY_HORIZON_TICKS.saturating_add(1)),
        );

        assert_eq!(budget, obs.scrap);
        assert!(airworks_builds(&intents).is_empty());
        assert!(
            intents
                .iter()
                .all(|intent| !matches!(intent, Intent::TrainAt { .. })),
            "ordinary production must leave the incomplete factory fund intact"
        );
    }

    #[test]
    fn a_full_airworks_queue_cannot_spend_the_recurring_capacity_fund() {
        let mut obs = completed_tree();
        let airworks = obs
            .my_buildings
            .iter()
            .position(|building| building.kind == BuildingKind::Airworks)
            .expect("the completed tree has an Airworks");
        obs.my_queues[airworks] = vec![UnitKind::Condor, UnitKind::Skyhook];
        let policy = UtilityPolicy::new();
        let commitment = policy.airworks_capacity_commitment(
            &Dials::balanced(),
            &obs,
            TilePos::new(1, 1),
            Some(AIRWORKS_ASSEMBLY_HORIZON_TICKS + 1),
            &[],
        );
        let expected = BuildingKind::Airworks
            .base_stats()
            .construction
            .expect("Airworks has construction stats")
            .cost
            + TECH_RESERVE;

        assert_eq!(commitment, expected);
        assert_eq!(0_u32.saturating_sub(commitment), 0);
        assert_eq!((expected - 1).saturating_sub(commitment), 0);
        assert_eq!(expected.saturating_sub(commitment), 0);
        assert_eq!((expected + 1).saturating_sub(commitment), 1);
    }

    #[test]
    fn capacity_commitment_respects_current_builder_claims_and_projected_sites() {
        let mut obs = completed_tree();
        let demand = Some(AIRWORKS_ASSEMBLY_HORIZON_TICKS + 1);
        let policy = UtilityPolicy::new();
        let home = TilePos::new(1, 1);
        let commitment = BuildingKind::Airworks
            .base_stats()
            .construction
            .expect("Airworks has construction stats")
            .cost
            + TECH_RESERVE;
        let harvesters: Vec<_> = obs
            .my_units
            .iter()
            .filter(|unit| unit.kind == UnitKind::Harvester)
            .map(|unit| unit.id)
            .collect();

        assert_eq!(
            policy.airworks_capacity_commitment(
                &Dials::balanced(),
                &obs,
                home,
                demand,
                &harvesters[..harvesters.len() - 1],
            ),
            commitment,
            "one unclaimed ordinary builder keeps the capacity fund active"
        );
        assert_eq!(
            policy.airworks_capacity_commitment(
                &Dials::balanced(),
                &obs,
                home,
                demand,
                &harvesters,
            ),
            0,
            "fully claimed builders make capacity construction ineligible"
        );

        obs.my_units[0].idle = false;
        obs.my_units[0].founding = Some((BuildingKind::Airworks, TilePos::new(10, 3)));
        assert_eq!(
            policy.airworks_capacity_commitment(&Dials::balanced(), &obs, home, demand, &[],),
            0,
            "a deferred Airworks already satisfies the projected-capacity boundary"
        );
    }

    #[test]
    fn capacity_waits_for_the_normal_tree_and_for_each_prior_factory() {
        let mut without_crucible = completed_tree();
        let crucible = without_crucible
            .my_buildings
            .iter()
            .position(|building| building.kind == BuildingKind::Crucible)
            .unwrap();
        without_crucible.my_buildings.remove(crucible);
        without_crucible.my_queues.remove(crucible);
        without_crucible.scrap = 2_000;
        let (_, intents) =
            capacity_decision(&without_crucible, Some(AIRWORKS_ASSEMBLY_HORIZON_TICKS * 3));
        assert!(
            airworks_builds(&intents).is_empty(),
            "capacity must not overtake the normal Crucible rung"
        );

        let mut site_in_progress = completed_tree();
        add_building(
            &mut site_in_progress,
            4,
            BuildingKind::Airworks,
            TilePos::new(10, 3),
            false,
        );
        site_in_progress.scrap = 2_000;
        let (_, intents) =
            capacity_decision(&site_in_progress, Some(AIRWORKS_ASSEMBLY_HORIZON_TICKS * 3));
        assert!(
            airworks_builds(&intents).is_empty(),
            "an unfinished second Airworks must finish before a third is promised"
        );

        site_in_progress.my_buildings.last_mut().unwrap().built = true;
        let (_, intents) =
            capacity_decision(&site_in_progress, Some(AIRWORKS_ASSEMBLY_HORIZON_TICKS * 3));
        assert_eq!(
            airworks_builds(&intents).len(),
            1,
            "the third Airworks becomes legal after the second stands"
        );
    }

    #[test]
    fn a_deferred_airworks_claim_prevents_duplicate_capacity_commands() {
        let mut obs = completed_tree();
        obs.scrap = 2_000;
        obs.my_units[0].idle = false;
        obs.my_units[0].founding = Some((BuildingKind::Airworks, TilePos::new(10, 3)));

        let (_, intents) = capacity_decision(&obs, Some(AIRWORKS_ASSEMBLY_HORIZON_TICKS * 3));

        assert!(
            airworks_builds(&intents).is_empty(),
            "a walking founder is already the one outstanding capacity build"
        );
    }

    #[test]
    fn player_facing_harvester_chooses_reachable_scrap_over_a_nearer_severed_node() {
        let mut obs = observation();
        let severed = TilePos::new(8, 4);
        let reachable = TilePos::new(1, 8);
        obs.known_scrap = vec![(severed, 100), (reachable, 100)];
        obs.known_scrap.sort_by_key(|(tile, _)| (tile.y, tile.x));
        let mut policy = UtilityPolicy::new();
        let mut intents = Vec::new();

        policy.economy(&obs, TilePos::new(1, 1), true, None, None, &mut intents);

        assert_eq!(
            intents,
            vec![Intent::AssignHarvest {
                unit: UnitId(3),
                node: reachable,
            }]
        );
    }

    #[test]
    fn player_facing_harvester_stays_idle_when_no_source_is_reachable() {
        let mut obs = observation();
        obs.known_wrecks = vec![(TilePos::new(8, 4), 100)];
        let mut policy = UtilityPolicy::new();
        let mut intents = Vec::new();

        policy.economy(&obs, TilePos::new(1, 1), true, None, None, &mut intents);

        assert!(intents.is_empty());
        assert!(policy.last_sent.is_empty());
    }

    #[test]
    fn player_facing_harvester_refuses_work_when_cut_off_from_every_drop_off() {
        let mut obs = observation();
        obs.my_units[0].tile = TilePos::new(8, 4);
        obs.known_wrecks = vec![(TilePos::new(9, 4), 100)];
        let mut policy = UtilityPolicy::new();
        let mut intents = Vec::new();

        policy.economy(&obs, TilePos::new(1, 1), true, None, None, &mut intents);

        assert!(intents.is_empty());
        assert!(policy.last_sent.is_empty());
    }

    #[test]
    fn ordinary_nearby_losses_do_not_turn_one_warning_into_an_economy_quarantine() {
        let mut obs = observation();
        obs.map_width = 24;
        obs.map_height = 16;
        obs.visible = vec![true; 24 * 16];
        obs.explored = vec![true; 24 * 16];
        obs.known_rock.clear();
        obs.my_units[0].tile = TilePos::new(3, 4);
        obs.my_units[0].carrying = 2;
        for (id, tile, carrying) in [
            (4, TilePos::new(4, 4), 7),
            (5, TilePos::new(3, 5), 3),
            (6, TilePos::new(4, 5), 4),
        ] {
            obs.my_units.push(UnitObs {
                id: UnitId(id),
                tile,
                carrying,
                ..obs.my_units[0].clone()
            });
        }
        obs.known_wrecks = vec![(TilePos::new(8, 2), 400), (TilePos::new(8, 3), 119)];
        obs.my_units[0].idle = false;
        obs.my_units[0].harvesting = Some(TilePos::new(8, 3));
        obs.enemy_units.push(UnitObs {
            id: UnitId(38),
            player: PlayerId(1),
            kind: UnitKind::Gnat,
            tile: TilePos::new(10, 9),
            hp: UnitKind::Gnat.stats().max_hp,
            idle: true,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        });
        let ordinary_loss = TilePos::new(8, 7);
        let mut policy = UtilityPolicy::new();

        policy.refresh_contested_harvest_regions(&obs, None, None);
        obs.tick = 12;
        obs.salvage_incidents = vec![ordinary_loss];
        policy.refresh_contested_harvest_regions(&obs, None, None);
        assert!(
            policy.contested_harvest_regions.is_empty(),
            "unchanged worker HP proves the anonymous loss was not a worker-route incident"
        );

        obs.tick += crate::stats::HARVEST_INCIDENT_MEMORY_TICKS + 1;
        obs.salvage_incidents.clear();
        obs.my_units[0].idle = true;
        obs.my_units[0].harvesting = None;
        let mut intents = Vec::new();
        policy.economy(&obs, TilePos::new(1, 1), true, None, None, &mut intents);

        let assigned: Vec<_> = intents
            .iter()
            .filter_map(|intent| match intent {
                Intent::AssignHarvest { unit, node } => Some((*unit, *node)),
                _ => None,
            })
            .collect();
        assert_eq!(
            assigned.len(),
            4,
            "every reachable idle worker should resume"
        );
        assert!(
            assigned.iter().all(|(_, node)| {
                matches!(*node, TilePos { x: 8, y: 2 } | TilePos { x: 8, y: 3 })
            })
        );
    }

    #[test]
    fn a_destroyed_active_harvester_keeps_replacements_out_of_the_kill_zone() {
        let incident = TilePos::new(7, 4);
        let wreck = TilePos::new(8, 4);
        let safe_fallback = TilePos::new(2, 9);
        let mut obs = observation();
        obs.map_width = 20;
        obs.map_height = 12;
        obs.visible = vec![true; 20 * 12];
        obs.explored = vec![true; 20 * 12];
        obs.known_rock.clear();
        obs.known_wrecks = vec![(wreck, 80)];
        obs.known_scrap = vec![(safe_fallback, 100)];
        obs.my_units[0].tile = incident;
        obs.my_units[0].idle = false;
        obs.my_units[0].harvesting = Some(wreck);
        obs.my_units.push(UnitObs {
            id: UnitId(4),
            tile: TilePos::new(2, 2),
            idle: true,
            harvesting: None,
            ..obs.my_units[0].clone()
        });
        let mut policy = UtilityPolicy::new();

        policy.refresh_contested_harvest_regions(&obs, None, None);
        obs.tick = 12;
        obs.my_units.remove(0);
        obs.visible.fill(false);
        obs.salvage_incidents = vec![incident];
        policy.refresh_contested_harvest_regions(&obs, None, None);

        assert!(policy.harvest_location_contested(wreck));
        assert_eq!(
            policy.contested_recon_target(&obs, TilePos::new(1, 1)),
            None,
            "recovery reconnaissance must not enter while the incident warning is live"
        );
        let mut intents = Vec::new();
        policy.economy(&obs, TilePos::new(1, 1), true, None, None, &mut intents);
        assert_eq!(
            intents,
            vec![Intent::AssignHarvest {
                unit: UnitId(4),
                node: safe_fallback,
            }]
        );

        obs.tick += crate::stats::HARVEST_INCIDENT_MEMORY_TICKS + 1;
        obs.salvage_incidents.clear();
        policy.refresh_contested_harvest_regions(&obs, None, None);
        intents.clear();
        policy.economy(&obs, TilePos::new(1, 1), true, None, None, &mut intents);
        assert_eq!(
            intents,
            vec![Intent::AssignHarvest {
                unit: UnitId(4),
                node: safe_fallback,
            }],
            "warning expiry in darkness must not route a replacement through the worker's death site"
        );
        assert!(
            policy
                .contested_recon_target(&obs, TilePos::new(1, 1))
                .is_some(),
            "the quiet expired region should now request bounded clearance"
        );
    }

    #[test]
    fn a_destroyed_worker_is_matched_to_its_active_source_beyond_its_last_tile() {
        let source = TilePos::new(16, 4);
        let last_seen = TilePos::new(3, 4);
        let mut obs = observation();
        obs.map_width = 24;
        obs.map_height = 12;
        obs.visible = vec![true; 24 * 12];
        obs.explored = vec![true; 24 * 12];
        obs.known_rock.clear();
        obs.my_units[0].tile = last_seen;
        obs.my_units[0].idle = false;
        obs.my_units[0].harvesting = Some(source);
        let mut policy = UtilityPolicy::new();

        policy.refresh_contested_harvest_regions(&obs, None, None);
        obs.tick += 1;
        obs.my_units.clear();
        obs.salvage_incidents = vec![source];
        policy.refresh_contested_harvest_regions(&obs, None, None);

        assert!(
            last_seen.chebyshev(source) > crate::stats::HARVEST_INCIDENT_DANGER_RADIUS,
            "the fixture must require active-source evidence rather than last-position proximity"
        );
        assert!(policy.harvest_location_contested(source));
        assert_eq!(
            policy.contested_harvest_regions,
            vec![ContestedHarvestRegion {
                center: source,
                last_evidence: obs.tick,
                sweep_started_at: None,
            }]
        );
    }

    #[test]
    fn a_visible_allied_worker_loss_seeds_the_team_shared_quarantine() {
        let incident = TilePos::new(12, 4);
        let mut obs = observation();
        obs.map_width = 24;
        obs.map_height = 12;
        obs.visible = vec![true; 24 * 12];
        obs.explored = vec![true; 24 * 12];
        obs.known_rock.clear();
        let mut ally = obs.my_units[0].clone();
        ally.id = UnitId(40);
        ally.player = PlayerId(1);
        ally.tile = incident;
        ally.idle = false;
        ally.harvesting = None;
        obs.ally_units.push(ally);
        let mut policy = UtilityPolicy::new();

        policy.refresh_contested_harvest_regions(&obs, None, None);
        obs.tick += 1;
        obs.ally_units.clear();
        obs.salvage_incidents = vec![incident];
        policy.refresh_contested_harvest_regions(&obs, None, None);

        assert!(
            policy.harvest_location_contested(incident),
            "allied units are always in team sight, so disappearance plus a team incident is real worker-loss evidence"
        );
    }

    #[test]
    fn completed_first_trip_does_not_hide_a_later_incident_from_replacement_work() {
        let source = TilePos::new(15, 4);
        let wreck = source.offset(1, 0);
        let safe_fallback = TilePos::new(1, 23);
        let mut obs = observation();
        obs.map_width = 40;
        obs.map_height = 24;
        obs.visible = vec![true; 40 * 24];
        obs.explored = vec![true; 40 * 24];
        obs.known_rock.clear();
        obs.known_scrap = vec![(source, 100)];
        let mut policy = UtilityPolicy::new();

        // The original Harvest command completed one full trip and continued
        // autonomously. Its immediate dispatch audit is long gone by the time
        // a later trip is hit; authoritative incident memory must carry the
        // warning instead of controller bookkeeping around the first load.
        policy.record_dispatched_harvest(&obs, UnitId(3), source);
        obs.tick = 8;
        obs.my_units[0].idle = false;
        obs.my_units[0].tile = source;
        policy.audit_harvests(&obs);
        policy.refresh_contested_harvest_regions(&obs, None, None);
        assert!(policy.last_sent.is_empty());

        obs.tick = 80;
        obs.known_scrap = vec![(safe_fallback, 100)];
        obs.known_wrecks = vec![(wreck, 45)];
        obs.salvage_incidents = vec![source];
        obs.my_units[0] = UnitObs {
            id: UnitId(4),
            idle: true,
            tile: TilePos::new(3, 4),
            ..obs.my_units[0].clone()
        };
        obs.visible.fill(false);

        assert!(UtilityPolicy::source_in_salvage_incident(&obs, wreck));
        policy.refresh_contested_harvest_regions(&obs, None, None);
        let mut intents = Vec::new();
        policy.economy(&obs, TilePos::new(1, 1), true, None, None, &mut intents);
        assert_eq!(
            intents,
            vec![Intent::AssignHarvest {
                unit: UnitId(4),
                node: safe_fallback,
            }],
            "the replacement must work elsewhere instead of entering the same kill zone"
        );

        // Observation contains only currently live incidents, but elapsed
        // warning time is not evidence that the raiders left.
        obs.tick += crate::stats::HARVEST_INCIDENT_MEMORY_TICKS + 1;
        obs.salvage_incidents.clear();
        policy.refresh_contested_harvest_regions(&obs, None, None);
        intents.clear();
        policy.economy(&obs, TilePos::new(1, 1), true, None, None, &mut intents);
        assert_eq!(
            intents,
            vec![Intent::AssignHarvest {
                unit: UnitId(4),
                node: safe_fallback,
            }],
            "warning expiry in darkness must not send replacements back into the kill zone"
        );

        let mut attacker = obs.my_units[0].clone();
        attacker.id = UnitId(90);
        attacker.player = PlayerId(1);
        attacker.kind = UnitKind::Sentinel;
        attacker.tile = source;
        attacker.hp = UnitKind::Sentinel.stats().max_hp;
        obs.enemy_units.push(attacker);
        obs.visible.fill(true);
        obs.tick += 1;
        policy.refresh_contested_harvest_regions(&obs, None, None);
        assert!(policy.harvest_location_contested(wreck));
        intents.clear();
        policy.economy(&obs, TilePos::new(1, 1), true, None, None, &mut intents);
        assert_eq!(
            intents,
            vec![Intent::AssignHarvest {
                unit: UnitId(4),
                node: safe_fallback,
            }],
            "full sight of an attacker is danger evidence, not clearance"
        );

        obs.enemy_units.clear();
        let unseen = source.offset(CONTESTED_RECON_RADIUS, CONTESTED_RECON_RADIUS);
        let unseen_index = usize::try_from(unseen.y * obs.map_width + unseen.x).unwrap();
        obs.visible[unseen_index] = false;
        obs.tick += 1;
        policy.refresh_contested_harvest_regions(&obs, None, None);
        assert!(
            policy.harvest_location_contested(wreck),
            "partial coverage after danger leaves must keep the kill zone closed"
        );

        obs.visible[unseen_index] = true;
        obs.tick += 1;
        policy.refresh_contested_harvest_regions(&obs, None, None);
        intents.clear();
        policy.economy(&obs, TilePos::new(1, 1), true, None, None, &mut intents);
        assert_eq!(
            intents,
            vec![Intent::AssignHarvest {
                unit: UnitId(4),
                node: wreck,
            }],
            "one complete recent danger-free sweep must make the salvage usable again"
        );
    }

    #[test]
    fn a_dispatched_retask_erases_the_superseded_bounce_probe() {
        let node = TilePos::new(5, 4);
        let mut obs = observation();
        obs.known_wrecks = vec![(node, 100)];
        let mut policy = UtilityPolicy::new();

        policy.record_dispatched_harvest(&obs, UnitId(3), node);
        assert_eq!(policy.last_sent.len(), 1);

        // A later queue-replacing Move/Scout owns the worker now. It must not
        // make the old source look like an immediate no-route bounce.
        policy.record_dispatched_retask(&[UnitId(3)]);
        assert!(policy.last_sent.is_empty());
        obs.my_units.clear();
        obs.visible.fill(false);
        policy.audit_harvests(&obs);

        assert!(policy.dead_nodes.is_empty());
    }

    #[test]
    fn a_stale_harvest_dispatch_cannot_create_false_bounce_evidence() {
        let node = TilePos::new(5, 4);
        let mut obs = observation();
        obs.known_wrecks = vec![(node, 100)];
        let mut policy = UtilityPolicy::new();

        policy.record_dispatched_harvest(&obs, UnitId(99), node);
        assert!(policy.last_sent.is_empty());

        obs.visible.fill(false);
        policy.audit_harvests(&obs);
        assert!(policy.dead_nodes.is_empty());
    }

    #[test]
    fn upgraded_turret_range_marks_a_source_as_known_danger() {
        let node = TilePos::new(5, 4);
        let mut obs = observation();
        let anchor = node.offset(7, 0);
        obs.enemy_buildings.push(BuildingObs {
            id: BuildingId(9),
            player: PlayerId(1),
            kind: BuildingKind::Turret,
            anchor,
            hp: BuildingKind::Turret.tier_stats(1).max_hp,
            built: true,
            seen: true,
            tier: 1,
        });

        assert!(
            UtilityPolicy::source_has_known_danger(&obs, node, None, None),
            "the tier-one weapon covers the source at seven tiles"
        );
        obs.enemy_buildings[0].tier = 0;
        obs.enemy_buildings[0].hp = BuildingKind::Turret.base_stats().max_hp;
        assert!(
            !UtilityPolicy::source_has_known_danger(&obs, node, None, None),
            "the base turret plus its safety margin stops at six tiles"
        );
    }

    #[test]
    fn visible_mobile_threat_redirects_work_until_it_leaves() {
        let threatened = TilePos::new(15, 4);
        let safe_fallback = TilePos::new(1, 23);
        let mut obs = observation();
        obs.map_width = 40;
        obs.map_height = 24;
        obs.visible = vec![true; 40 * 24];
        obs.explored = vec![true; 40 * 24];
        obs.known_rock.clear();
        obs.known_wrecks = vec![(threatened, 45)];
        obs.known_scrap = vec![(safe_fallback, 100)];
        obs.enemy_units.push(UnitObs {
            id: UnitId(9),
            player: PlayerId(1),
            kind: UnitKind::Sentinel,
            tile: threatened.offset(3, 0),
            hp: UnitKind::Sentinel.stats().max_hp,
            idle: false,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        });
        let mut policy = UtilityPolicy::new();
        let mut intents = Vec::new();

        policy.economy(&obs, TilePos::new(1, 1), true, None, None, &mut intents);
        assert_eq!(
            intents,
            vec![Intent::AssignHarvest {
                unit: UnitId(3),
                node: safe_fallback,
            }]
        );

        obs.enemy_units.clear();
        intents.clear();
        policy.economy(&obs, TilePos::new(1, 1), true, None, None, &mut intents);
        assert_eq!(
            intents,
            vec![Intent::AssignHarvest {
                unit: UnitId(3),
                node: threatened,
            }],
            "current mobile danger must disappear with current sight rather than becoming invented memory"
        );
    }

    #[test]
    fn remembered_mobile_threat_survives_the_sight_collapse_after_a_worker_loss() {
        let threatened = TilePos::new(15, 4);
        let safe_fallback = TilePos::new(1, 19);
        let last_seen = threatened.offset(3, 0);
        let mut obs = observation();
        obs.map_width = 40;
        obs.map_height = 24;
        obs.visible = vec![true; 40 * 24];
        obs.explored = vec![true; 40 * 24];
        obs.known_rock.clear();
        obs.tick = 100;
        obs.known_wrecks = vec![(threatened, 45)];
        obs.known_scrap = vec![(safe_fallback, 100)];
        obs.enemy_units.push(UnitObs {
            id: UnitId(9),
            player: PlayerId(1),
            kind: UnitKind::Sentinel,
            tile: last_seen,
            hp: UnitKind::Sentinel.stats().max_hp,
            idle: false,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        });
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&obs);

        // This is the replay failure shape: the last worker disappears, its
        // sight disappears with it, and the hostile unit is no longer in the
        // current observation on the very next controller tick.
        obs.tick += 1;
        obs.visible.fill(false);
        obs.enemy_units.clear();
        intelligence.update(&obs);
        let mut policy = UtilityPolicy::new();
        let mut intents = Vec::new();
        policy.economy(
            &obs,
            TilePos::new(1, 1),
            true,
            Some(intelligence.units()),
            Some(intelligence.buildings()),
            &mut intents,
        );
        assert_eq!(
            intents,
            vec![Intent::AssignHarvest {
                unit: UnitId(3),
                node: safe_fallback,
            }],
            "remembered mobile fire coverage must survive the victim's sight collapse"
        );

        // Looking at the unit's last tile and finding it empty is the honest
        // negative evidence that retires this positional memory.
        let seen_index = usize::try_from(last_seen.y * obs.map_width + last_seen.x).unwrap();
        obs.visible[seen_index] = true;
        intelligence.update(&obs);
        intents.clear();
        policy.economy(
            &obs,
            TilePos::new(1, 1),
            true,
            Some(intelligence.units()),
            Some(intelligence.buildings()),
            &mut intents,
        );
        assert_eq!(
            intents,
            vec![Intent::AssignHarvest {
                unit: UnitId(3),
                node: threatened,
            }],
            "fresh negative sight must release the stale mobile contact"
        );
    }

    #[test]
    fn a_worker_loss_quarantines_only_the_exact_kill_zone() {
        let incident = TilePos::new(20, 10);
        let edge_source = incident.offset(CONTESTED_HARVEST_RADIUS, 0);
        let outside = incident.offset(CONTESTED_HARVEST_RADIUS + 1, 0);
        let safe_fallback = TilePos::new(1, 23);
        let mut obs = observation();
        obs.map_width = 50;
        obs.map_height = 30;
        obs.visible = vec![true; 50 * 30];
        obs.explored = vec![true; 50 * 30];
        obs.known_rock.clear();
        obs.known_wrecks = vec![(edge_source, 45)];
        obs.known_scrap = vec![(safe_fallback, 100)];
        let mut policy = UtilityPolicy::new();
        let safe_worker_tile = obs.my_units[0].tile;
        obs.my_units[0].tile = incident;
        obs.my_units[0].idle = false;
        obs.my_units[0].harvesting = Some(edge_source);
        policy.refresh_contested_harvest_regions(&obs, None, None);
        obs.tick += 1;
        obs.my_units[0].tile = safe_worker_tile;
        obs.my_units[0].idle = true;
        obs.my_units[0].harvesting = None;
        obs.my_units[0].hp -= 1;
        obs.salvage_incidents = vec![incident];
        policy.refresh_contested_harvest_regions(&obs, None, None);

        assert!(policy.harvest_location_contested(edge_source));
        assert!(!policy.harvest_location_contested(outside));
        let mut intents = Vec::new();
        policy.economy(&obs, TilePos::new(1, 1), true, None, None, &mut intents);
        assert_eq!(
            intents,
            vec![Intent::AssignHarvest {
                unit: UnitId(3),
                node: safe_fallback,
            }],
            "a source inside the exact kill zone must stay closed while unrelated work continues"
        );
    }

    #[test]
    fn a_safe_source_is_refused_when_its_only_route_crosses_a_contested_region() {
        let incident = TilePos::new(20, 5);
        let source = TilePos::new(35, 5);
        let mut obs = observation();
        obs.map_width = 40;
        obs.map_height = 10;
        obs.visible = vec![true; 40 * 10];
        obs.explored = vec![true; 40 * 10];
        obs.known_rock.clear();
        obs.known_scrap = vec![(source, 100)];

        let mut clear_policy = UtilityPolicy::new();
        let mut intents = Vec::new();
        clear_policy.economy(&obs, TilePos::new(1, 1), true, None, None, &mut intents);
        assert_eq!(
            intents,
            vec![Intent::AssignHarvest {
                unit: UnitId(3),
                node: source,
            }],
            "the ordinary terrain route must make the isolated safety condition observable"
        );

        obs.salvage_incidents = vec![incident];
        let mut guarded_policy = UtilityPolicy::new();
        let safe_worker_tile = obs.my_units[0].tile;
        obs.salvage_incidents.clear();
        obs.my_units[0].tile = incident;
        obs.my_units[0].idle = false;
        obs.my_units[0].harvesting = Some(source);
        guarded_policy.refresh_contested_harvest_regions(&obs, None, None);
        obs.tick += 1;
        obs.my_units[0].tile = safe_worker_tile;
        obs.my_units[0].idle = true;
        obs.my_units[0].harvesting = None;
        obs.my_units[0].hp -= 1;
        obs.salvage_incidents = vec![incident];
        guarded_policy.refresh_contested_harvest_regions(&obs, None, None);
        intents.clear();
        guarded_policy.economy(&obs, TilePos::new(1, 1), true, None, None, &mut intents);
        assert!(
            intents.is_empty(),
            "safe endpoints must not authorize a work route through the quarantined kill zone"
        );
    }

    #[test]
    fn active_harvesters_and_builders_evacuate_once_then_retry_only_if_stuck() {
        let home = TilePos::new(1, 1);
        let incident = TilePos::new(16, 8);
        let pending = incident.offset(1, 0);
        let mut obs = observation();
        obs.map_width = 40;
        obs.map_height = 24;
        obs.visible = vec![true; 40 * 24];
        obs.explored = vec![true; 40 * 24];
        obs.known_rock.clear();
        obs.known_scrap = vec![(TilePos::new(5, 5), 100)];
        obs.my_units[0].tile = incident;
        obs.my_units[0].idle = false;
        obs.my_units[0].founding = Some((BuildingKind::Foundry, pending));
        obs.my_units.push(UnitObs {
            id: UnitId(4),
            tile: incident.offset(0, 1),
            idle: false,
            site: Some(BuildingId(12)),
            founding: None,
            ..obs.my_units[0].clone()
        });
        let mut policy = UtilityPolicy::new();
        policy.pending_sites.push(pending);
        policy.scout = Some(UnitId(3));
        policy.scout_dispatch = Some(ScoutDispatch::ordinary(UnitId(3), incident, pending));
        policy.refresh_contested_harvest_regions(&obs, None, None);
        obs.tick += 1;
        obs.my_units[0].hp -= 1;
        obs.salvage_incidents = vec![incident];
        policy.refresh_contested_harvest_regions(&obs, None, None);

        let mut intents = Vec::new();
        policy.evacuate_contested_workers(&obs, home, None, None, &mut intents);
        assert_eq!(
            intents,
            vec![
                Intent::MoveUnits {
                    units: vec![UnitId(3)],
                    goal: TilePos::new(16, 2),
                },
                Intent::MoveUnits {
                    units: vec![UnitId(4)],
                    goal: TilePos::new(16, 14),
                },
            ],
            "each worker must leave by its nearest safe edge instead of crossing deeper danger to group up"
        );
        assert!(policy.pending_sites.is_empty());
        assert_eq!(policy.scout, None);

        intents.clear();
        policy.evacuate_contested_workers(&obs, home, None, None, &mut intents);
        assert!(
            intents.is_empty(),
            "an in-flight escape must not be replaced every controller cadence"
        );

        obs.my_units[0].idle = true;
        policy.evacuate_contested_workers(&obs, home, None, None, &mut intents);
        policy.economy(&obs, home, true, None, None, &mut intents);
        assert_eq!(
            intents,
            vec![Intent::MoveUnits {
                units: vec![UnitId(3)],
                goal: TilePos::new(16, 2),
            }],
            "an evacuation that bounced while still in danger must be retried"
        );

        obs.my_units[0].tile = TilePos::new(1, 0);
        intents.clear();
        policy.evacuate_contested_workers(&obs, home, None, None, &mut intents);
        assert!(
            intents.is_empty(),
            "a worker accepted one tile short of a safe goal must leave the evacuation lifecycle"
        );
        assert!(!policy.evacuating_workers.contains(&UnitId(3)));
    }

    #[test]
    fn evacuation_stops_short_of_an_unrelated_kill_zone_instead_of_crossing_it() {
        let home = TilePos::new(3, 4);
        let barrier = TilePos::new(14, 4);
        let worker_region = TilePos::new(26, 4);
        let mut obs = observation();
        obs.map_width = 30;
        obs.map_height = 9;
        obs.visible = vec![true; 30 * 9];
        obs.explored = vec![true; 30 * 9];
        obs.known_rock.clear();
        obs.my_units[0].tile = worker_region;
        let policy = UtilityPolicy {
            contested_harvest_regions: vec![
                ContestedHarvestRegion {
                    center: barrier,
                    last_evidence: obs.tick,
                    sweep_started_at: None,
                },
                ContestedHarvestRegion {
                    center: worker_region,
                    last_evidence: obs.tick,
                    sweep_started_at: None,
                },
            ],
            ..UtilityPolicy::new()
        };
        let danger = policy.harvest_danger_projection(&obs, None, None);

        let goal = policy
            .worker_evacuation_goal(&obs, &obs.my_units[0], home, &danger)
            .expect("the worker can leave its own kill zone on the near side of the barrier");

        assert_eq!(goal, TilePos::new(20, 4));
        assert!(
            goal.x > barrier.x + CONTESTED_HARVEST_RADIUS,
            "the nearest home-side goal would make the ordinary Move route cross a second quarantine"
        );
        assert!(
            goal.x < worker_region.x - CONTESTED_HARVEST_RADIUS,
            "the worker should still leave the kill zone it currently occupies"
        );
    }

    #[test]
    fn evacuation_leaves_from_the_near_edge_instead_of_crossing_deeper_danger() {
        let center = TilePos::new(15, 8);
        let worker_tile = center.offset(0, -CONTESTED_HARVEST_RADIUS);
        let home = TilePos::new(15, 16);
        let mut obs = observation();
        obs.map_width = 30;
        obs.map_height = 20;
        obs.visible = vec![true; 30 * 20];
        obs.explored = vec![true; 30 * 20];
        obs.known_rock.clear();
        obs.my_units[0].tile = worker_tile;
        let policy = UtilityPolicy {
            contested_harvest_regions: vec![ContestedHarvestRegion {
                center,
                last_evidence: obs.tick,
                sweep_started_at: None,
            }],
            ..UtilityPolicy::new()
        };
        let danger = policy.harvest_danger_projection(&obs, None, None);

        let goal = policy
            .worker_evacuation_goal(&obs, &obs.my_units[0], home, &danger)
            .expect("open ground has a safe exit immediately away from the incident center");

        assert_eq!(goal, center.offset(0, -CONTESTED_HARVEST_RADIUS - 2));
        assert!(
            goal.manhattan(worker_tile) < home.manhattan(worker_tile),
            "recovery should minimize exposure before preferring the homeward direction"
        );
        assert!(
            goal.y < worker_tile.y,
            "the direct homeward route enters progressively deeper quarantine before leaving it"
        );
    }

    #[test]
    fn repair_does_not_recruit_an_evacuated_worker_back_into_quarantine() {
        let home = TilePos::new(1, 1);
        let incident = TilePos::new(20, 10);
        let patient = BuildingId(1);
        let mut obs = observation();
        obs.map_width = 40;
        obs.map_height = 24;
        obs.visible = vec![true; 40 * 24];
        obs.explored = vec![true; 40 * 24];
        obs.known_rock.clear();
        obs.scrap = 1_000;
        obs.my_units[0].tile = incident;
        obs.my_units[0].idle = false;
        add_building(
            &mut obs,
            patient.0,
            BuildingKind::Extractor,
            incident.offset(-2, -1),
            true,
        );
        obs.my_buildings[1].hp = BuildingKind::Extractor.base_stats().max_hp / 2;

        let mut policy = UtilityPolicy::new();
        policy.refresh_contested_harvest_regions(&obs, None, None);
        obs.tick += 1;
        obs.my_units[0].hp -= 1;
        obs.salvage_incidents = vec![incident];
        policy.refresh_contested_harvest_regions(&obs, None, None);
        let mut intents = Vec::new();
        policy.evacuate_contested_workers(&obs, home, None, None, &mut intents);
        assert!(matches!(
            intents.as_slice(),
            [Intent::MoveUnits { units, .. }] if units == &[UnitId(3)]
        ));

        // The escape completed just outside the quarantine. The worker is no
        // longer reserved by the in-flight move, but the patient remains in
        // the region that caused it to flee.
        obs.tick += 1;
        obs.salvage_incidents.clear();
        obs.visible.fill(false);
        obs.my_units[0].tile = home;
        obs.my_units[0].idle = true;
        intents.clear();
        policy.refresh_contested_harvest_regions(&obs, None, None);
        policy.evacuate_contested_workers(&obs, home, None, None, &mut intents);
        assert!(intents.is_empty());
        assert!(policy.worker_safety_reservations().is_empty());

        let mut budget = obs.scrap;
        policy.repairs(
            &Dials::full(),
            &obs,
            PolicyMode {
                player_facing: true,
                admit_voluntary_macro: true,
                unit_contacts: None,
                building_contacts: None,
                public_map: None,
            },
            &mut budget,
            &mut intents,
        );
        assert!(
            intents.is_empty(),
            "repair must not send the newly safe worker straight back into the durable kill-zone memory"
        );

        // Repair becomes eligible again after the same bounded clear sweep
        // that reopens harvesting in this region.
        obs.visible.fill(true);
        policy.refresh_contested_harvest_regions(&obs, None, None);
        policy.repairs(
            &Dials::full(),
            &obs,
            PolicyMode {
                player_facing: true,
                admit_voluntary_macro: true,
                unit_contacts: None,
                building_contacts: None,
                public_map: None,
            },
            &mut budget,
            &mut intents,
        );
        assert_eq!(intents, vec![Intent::Repair { building: patient }]);
    }

    #[test]
    fn player_facing_repair_does_not_dispatch_a_worker_into_visible_fire() {
        let patient = BuildingId(1);
        let patient_anchor = TilePos::new(10, 4);
        let mut obs = observation();
        obs.scrap = 1_000;
        obs.known_rock.clear();
        add_building(
            &mut obs,
            patient.0,
            BuildingKind::Extractor,
            patient_anchor,
            true,
        );
        obs.my_buildings[1].hp = BuildingKind::Extractor.base_stats().max_hp / 2;
        obs.enemy_units.push(UnitObs {
            id: UnitId(90),
            player: PlayerId(1),
            kind: UnitKind::Sentinel,
            tile: patient_anchor.offset(3, 0),
            hp: UnitKind::Sentinel.stats().max_hp,
            idle: false,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        });

        let mut policy = UtilityPolicy::new();
        let mut budget = obs.scrap;
        let mut intents = Vec::new();
        policy.repairs(
            &Dials::full(),
            &obs,
            PolicyMode {
                player_facing: true,
                admit_voluntary_macro: true,
                unit_contacts: None,
                building_contacts: None,
                public_map: None,
            },
            &mut budget,
            &mut intents,
        );
        assert!(
            intents.is_empty(),
            "worker safety must outrank repairing the structure under visible attack"
        );

        policy.repairs(
            &Dials::full(),
            &obs,
            PolicyMode {
                player_facing: false,
                admit_voluntary_macro: true,
                unit_contacts: None,
                building_contacts: None,
                public_map: None,
            },
            &mut budget,
            &mut intents,
        );
        assert_eq!(
            intents,
            vec![Intent::Repair { building: patient }],
            "the profile-free Overseer retains its frozen repair choice"
        );
    }

    #[test]
    fn remembered_emplacement_blocks_fresh_salvage_without_a_prior_worker_loss() {
        let node = TilePos::new(5, 4);
        let anchor = node.offset(7, 0);
        let mut obs = observation();
        obs.tick = 100;
        obs.known_wrecks = vec![(node, 45)];
        obs.enemy_buildings.push(BuildingObs {
            id: BuildingId(9),
            player: PlayerId(1),
            kind: BuildingKind::Turret,
            anchor,
            hp: BuildingKind::Turret.tier_stats(1).max_hp,
            built: true,
            seen: true,
            tier: 1,
        });
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&obs);

        obs.tick = 108;
        obs.visible.fill(false);
        obs.enemy_buildings[0].id = BuildingId(u32::MAX);
        obs.enemy_buildings[0].seen = false;
        obs.enemy_buildings[0].tier = 0;
        intelligence.update(&obs);
        let mut policy = UtilityPolicy::new();
        let mut intents = Vec::new();
        policy.economy(
            &obs,
            TilePos::new(1, 1),
            true,
            Some(intelligence.units()),
            Some(intelligence.buildings()),
            &mut intents,
        );
        assert!(
            intents.is_empty(),
            "the last personally observed upgraded range still covers the wreck"
        );

        obs.tick = 116;
        obs.enemy_buildings.clear();
        let anchor_index = usize::try_from(anchor.y * obs.map_width + anchor.x).unwrap();
        obs.visible[anchor_index] = true;
        intelligence.update(&obs);
        policy.economy(
            &obs,
            TilePos::new(1, 1),
            true,
            Some(intelligence.units()),
            Some(intelligence.buildings()),
            &mut intents,
        );
        assert_eq!(
            intents,
            vec![Intent::AssignHarvest {
                unit: UnitId(3),
                node,
            }],
            "fresh negative sight must retire the remembered emplacement warning"
        );
    }

    #[test]
    fn profile_free_harvester_keeps_the_nearest_route_agnostic_assignment() {
        let mut obs = observation();
        let severed = TilePos::new(8, 4);
        let reachable = TilePos::new(1, 8);
        obs.known_scrap = vec![(severed, 100), (reachable, 100)];
        obs.known_scrap.sort_by_key(|(tile, _)| (tile.y, tile.x));
        obs.salvage_incidents = vec![severed];
        let mut policy = UtilityPolicy::new();
        let mut intents = Vec::new();

        policy.economy(&obs, TilePos::new(1, 1), false, None, None, &mut intents);

        assert_eq!(
            intents,
            vec![Intent::AssignHarvest {
                unit: UnitId(3),
                node: severed,
            }]
        );
    }
}
