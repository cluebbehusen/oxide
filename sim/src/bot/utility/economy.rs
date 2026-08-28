//! Economy and production decision channels.

use super::*;

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
    pub(super) fn production(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        home: TilePos,
        claims: ConstructionClaims<'_>,
        budget: &mut u32,
        intents: &mut Vec<Intent>,
    ) {
        self.production_with_air_demand(
            dials,
            obs,
            ProductionContext {
                home,
                claims,
                outstanding_air_production_ticks: None,
            },
            budget,
            intents,
        );
    }

    pub(super) fn production_with_air_demand(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        context: ProductionContext<'_>,
        budget: &mut u32,
        intents: &mut Vec<Intent>,
    ) {
        let ProductionContext {
            home,
            claims,
            outstanding_air_production_ticks,
        } = context;
        let ConstructionClaims {
            player_facing,
            enlisted,
            reserved,
        } = claims;
        let queued = |kind: UnitKind| -> usize {
            obs.my_queues
                .iter()
                .flat_map(|q| q.iter())
                .filter(|k| **k == kind)
                .count()
        };
        let alive =
            |kind: UnitKind| -> usize { obs.my_units.iter().filter(|u| u.kind == kind).count() };
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
        let ordinary_capital = if screen < 3 || (self.desperate && self.desperate_road) {
            0
        } else {
            self.capital_reserve(dials, obs, home, player_facing, enlisted, reserved)
        };
        let ordinary_capital = if player_facing
            && dials.extractors
            && self.supported_frame_restoration_needed(obs, home)
        {
            ordinary_capital.max(
                BuildingKind::Extractor
                    .base_stats()
                    .construction
                    .map_or(0, |construction| construction.cost),
            )
        } else {
            ordinary_capital
        };

        let airworks_cost = BuildingKind::Airworks
            .base_stats()
            .construction
            .map_or(0, |construction| construction.cost);
        let mut capacity_site =
            self.airworks_capacity_site(dials, obs, home, claims, outstanding_air_production_ticks);
        if let Some(anchor) = capacity_site
            && *budget >= airworks_cost.saturating_add(TECH_RESERVE)
        {
            *budget -= airworks_cost;
            intents.push(Intent::Build {
                kind: BuildingKind::Airworks,
                anchor,
            });
            capacity_site = None;
        }
        let capacity_capital = if capacity_site.is_some() {
            airworks_cost.saturating_add(TECH_RESERVE)
        } else {
            0
        };
        let capital = ordinary_capital.max(capacity_capital);
        let allow_repeatable_ground =
            !player_facing || self.ordinary_ground_has_work(dials, obs, home);

        // A ground scout that bounced off a severed route has proved
        // that the next reconnaissance leg must fly. Keep exactly one
        // faction scout alive or queued once an Airworks can build it.
        if dials.scouting && self.air_scout_needed && !self.solo_air_scout_suspended {
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
            let airworks = obs
                .my_buildings
                .iter()
                .enumerate()
                .filter(|(_, building)| building.kind == BuildingKind::Airworks && building.built)
                .min_by_key(|(_, building)| building.id);
            if scout_count == 0
                && let Some((queue_index, airworks)) = airworks
                && obs.my_queues[queue_index]
                    .len()
                    .saturating_add(super::production::planned_at(intents, airworks.id))
                    < 2
            {
                let price = scout_kind.stats().cost;
                if *budget >= price {
                    *budget -= price;
                    intents.push(Intent::TrainAt {
                        building: airworks.id,
                        kind: scout_kind,
                    });
                } else {
                    // Reconnaissance is the prerequisite for every
                    // target-driven island purchase, so cheaper drips
                    // must not spend its partial fund.
                    *budget = 0;
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
            let airworks = obs
                .my_buildings
                .iter()
                .enumerate()
                .filter(|(_, b)| b.kind == BuildingKind::Airworks && b.built)
                .min_by_key(|(_, b)| b.id);
            if let Some((qi, airworks)) = airworks
                && (Self::island_target(obs, home).is_some()
                    || (self.desperate && !self.desperate_road))
            {
                let price = UnitKind::Skyhook.stats().cost + TECH_RESERVE;
                if *budget >= price && obs.my_queues[qi].len() < 2 {
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

        // One Warden per think from a standing Fabricator, and one
        // Breaker whenever the Crucible is idle and the bank can take it.
        // Deep-tech production runs before the basic military drip.
        if dials.deep_tech && !dials.adaptive_composition {
            let crucible = obs
                .my_buildings
                .iter()
                .enumerate()
                .filter(|(_, b)| b.kind == BuildingKind::Crucible && b.built)
                .min_by_key(|(_, b)| b.id);
            if let Some((qi, crucible)) = crucible
                && obs.my_queues[qi].is_empty()
                && alive(UnitKind::Breaker) + queued(UnitKind::Breaker) < 2
                && *budget >= UnitKind::Breaker.stats().cost + TECH_RESERVE
            {
                *budget -= UnitKind::Breaker.stats().cost;
                intents.push(Intent::TrainAt {
                    building: crucible.id,
                    kind: UnitKind::Breaker,
                });
            }
            let fabricator = obs
                .my_buildings
                .iter()
                .enumerate()
                .filter(|(_, b)| b.kind == BuildingKind::Fabricator && b.built)
                .min_by_key(|(_, b)| b.id);
            if let Some((qi, fabricator)) = fabricator
                && obs.my_queues[qi].len() < 2
                && alive(UnitKind::Warden) + queued(UnitKind::Warden) < 4
                && *budget >= UnitKind::Warden.stats().cost + UnitKind::Harvester.stats().cost
            {
                *budget -= UnitKind::Warden.stats().cost;
                intents.push(Intent::TrainAt {
                    building: fabricator.id,
                    kind: UnitKind::Warden,
                });
            }
            // Once the whole tree stands, a small bomber wing: the
            // payload that decides sieges — and island wars, where no
            // crawler ever crosses.
            {
                use crate::stats::Role;
                let bomber_kind = Role::Bomber.unit_for(obs.faction);
                let airworks = obs
                    .my_buildings
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| b.kind == BuildingKind::Airworks && b.built)
                    .min_by_key(|(_, b)| b.id);
                let crucible_stands = obs
                    .my_buildings
                    .iter()
                    .any(|b| b.kind == BuildingKind::Crucible && b.built);
                if let Some((qi, airworks)) = airworks
                    && crucible_stands
                    && obs.my_queues[qi].len() < 2
                    && alive(bomber_kind) + queued(bomber_kind) < 2
                    && *budget >= bomber_kind.stats().cost + TECH_RESERVE
                {
                    *budget -= bomber_kind.stats().cost;
                    intents.push(Intent::TrainAt {
                        building: airworks.id,
                        kind: bomber_kind,
                    });
                }
            }
        }

        let foundry = obs
            .my_buildings
            .iter()
            .enumerate()
            .filter(|(_, b)| b.kind == BuildingKind::Foundry && b.built)
            .min_by_key(|(_, b)| b.id);
        if let Some((qi, foundry)) = foundry
            && obs.my_queues[qi].len() < 2
        {
            if harvesters < immediate_harvester_target(dials) as usize
                && *budget >= UnitKind::Harvester.stats().cost
            {
                *budget -= UnitKind::Harvester.stats().cost;
                intents.push(Intent::TrainAt {
                    building: foundry.id,
                    kind: UnitKind::Harvester,
                });
            } else if !dials.adaptive_composition
                && allow_repeatable_ground
                && *budget >= UnitKind::Sentinel.stats().cost + capital
            {
                *budget -= UnitKind::Sentinel.stats().cost;
                intents.push(Intent::TrainAt {
                    building: foundry.id,
                    kind: UnitKind::Sentinel,
                });
            }
        }

        if !dials.tech {
            if dials.adaptive_composition {
                self.adaptive_production(
                    dials,
                    obs,
                    super::production::AdaptiveProductionContext::new(
                        reserved,
                        outstanding_air_production_ticks.is_none(),
                        capital,
                    )
                    .with_repeatable_ground(allow_repeatable_ground),
                    budget,
                    intents,
                );
            }
            return;
        }
        let fabricator = obs
            .my_buildings
            .iter()
            .enumerate()
            .filter(|(_, b)| b.kind == BuildingKind::Fabricator && b.built)
            .min_by_key(|(_, b)| b.id);
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
        if let Some((qi, fab)) = fabricator {
            use crate::stats::Role;
            let fab_open = obs.my_queues[qi].len() < 2;
            let foundry_open = foundry.filter(|(fqi, _)| obs.my_queues[*fqi].len() < 2);
            let airworks_open = obs
                .my_buildings
                .iter()
                .enumerate()
                .filter(|(_, b)| b.kind == BuildingKind::Airworks && b.built)
                .min_by_key(|(_, b)| b.id)
                .filter(|(aqi, _)| obs.my_queues[*aqi].len() < 2);
            let aa_kind = Role::AntiAir.unit_for(obs.faction);
            let wing_kind = Role::AirGround.unit_for(obs.faction);
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
                && fab_open
                && alive(aa_kind) + queued(aa_kind) < want_aa
                && *budget >= aa_kind.stats().cost
            {
                *budget -= aa_kind.stats().cost;
                intents.push(Intent::TrainAt {
                    building: fab.id,
                    kind: aa_kind,
                });
            } else if fab_open
                && enemy_turrets > alive(UnitKind::Lancer) + queued(UnitKind::Lancer)
                && *budget >= lancer
            {
                *budget -= lancer;
                intents.push(Intent::TrainAt {
                    building: fab.id,
                    kind: UnitKind::Lancer,
                });
            } else if !dials.adaptive_composition
                && alive(UnitKind::Scuttler) < 4
                && enemy_harvesters >= 2
                && *budget >= scuttler + reserve
                && let Some((_, raid_bay)) = foundry_open
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
                && let Some((_, airworks)) = airworks_open
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
                && fab_open
                && *budget >= lancer + reserve + capital
            {
                *budget -= lancer;
                intents.push(Intent::TrainAt {
                    building: fab.id,
                    kind: UnitKind::Lancer,
                });
            }
        }
        if dials.adaptive_composition {
            self.adaptive_production(
                dials,
                obs,
                super::production::AdaptiveProductionContext::new(
                    reserved,
                    outstanding_air_production_ticks.is_none(),
                    capital,
                )
                .with_repeatable_ground(allow_repeatable_ground),
                budget,
                intents,
            );
        }
    }

    /// Whether another ordinary ground reinforcement has an honestly known
    /// job it can reach. The minimum fighting core and bounded specialists are
    /// still produced without one; this gate only stops the perpetual
    /// Sentinel/Lancer stream from turning a completed island operation into
    /// hundreds of idle bodies. A current island objective also qualifies
    /// while ordinary transport capacity exists, but a dark ghost alone does
    /// not authorize an endless next wave.
    fn ordinary_ground_has_work(&self, dials: &Dials, obs: &Observation, home: TilePos) -> bool {
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
        home: TilePos,
        player_facing: bool,
        enlisted: &[UnitId],
        reserved: &[UnitId],
    ) -> u32 {
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
        // The expansion Foundry is a capital rung too. A remote own Extractor
        // outranks Airworks once the Fabricator prerequisite stands; ordinary
        // salvage frontiers retain the old deep-tech ordering. The construction
        // channel still proves exact placement before claiming.
        if dials.expansion {
            let (foundries, pending_foundries): (Vec<TilePos>, usize) = if player_facing {
                Self::projected_foundries(obs)
            } else {
                (
                    obs.my_buildings
                        .iter()
                        .filter(|building| building.kind == BuildingKind::Foundry)
                        .map(|building| building.anchor)
                        .collect(),
                    0,
                )
            };
            let ordinary_frontier_unlocked = !dials.deep_tech || have(BuildingKind::Airworks);
            let expansion_claim = if player_facing {
                let builders = self.construction_builders(obs, enlisted, reserved);
                self.player_facing_foundry_claim(
                    obs,
                    home,
                    &foundries,
                    &builders,
                    have(BuildingKind::Fabricator),
                    ordinary_frontier_unlocked,
                )
                .is_some()
            } else {
                ordinary_frontier_unlocked
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
                        })
            };
            let foundry_cap = if player_facing { dials.foundry_cap } else { 2 };
            if pending_foundries == 0
                && expansion_claim
                && foundries.len() < foundry_cap
                && (!player_facing
                    || super::production::extra_foundry_core_ready(obs, reserved, foundries.len()))
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
    use super::*;
    use crate::bot::intelligence::{ContactEvidence, StrategicIntelligence};
    use crate::bot::observation::{BuildingObs, OBSERVATION_VERSION};
    use crate::ids::{BuildingId, PlayerId};
    use crate::scenario::{BotConfig, BotDifficulty, BotStance};
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
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
        };
        Observation {
            version: OBSERVATION_VERSION,
            tick: 0,
            me: PlayerId(0),
            scrap: 0,
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
            ally_units: Vec::new(),
            ally_buildings: Vec::new(),
            enemy_units: Vec::new(),
            enemy_buildings: Vec::new(),
            visible: vec![true; 16 * 10],
            explored: vec![true; 16 * 10],
            known_scrap: Vec::new(),
            known_rock: (0..10).map(|y| TilePos::new(7, y)).collect(),
            known_frames: Vec::new(),
            known_peaks: Vec::new(),
            known_wrecks: Vec::new(),
            salvage_incidents: Vec::new(),
            blips: Vec::new(),
            faction: Faction::Ferrous,
            my_shells: 0,
            incoming_shells: Vec::new(),
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
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
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
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
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
            ProductionContext {
                home: TilePos::new(1, 1),
                claims: ConstructionClaims {
                    player_facing: true,
                    enlisted: &[],
                    reserved: &[],
                },
                outstanding_air_production_ticks: demand,
            },
            &mut budget,
            &mut intents,
        );
        (budget, intents)
    }

    fn airworks_builds(intents: &[Intent]) -> Vec<TilePos> {
        intents
            .iter()
            .filter_map(|intent| match intent {
                Intent::Build {
                    kind: BuildingKind::Airworks,
                    anchor,
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
        policy.air_scout_needed = true;
        let mut dials = Dials::balanced();
        dials.adaptive_composition = true;
        let mut budget = obs.scrap;

        policy.production_with_air_demand(
            &dials,
            &obs,
            ProductionContext {
                home: TilePos::new(1, 1),
                claims: ConstructionClaims {
                    player_facing: true,
                    enlisted: &[],
                    reserved: &[],
                },
                outstanding_air_production_ticks: Some(4_000),
            },
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
        let decide = |needs_air_scout: bool, scrap: u32| {
            let mut policy = UtilityPolicy::new();
            policy.air_scout_needed = needs_air_scout;
            let mut current = obs.clone();
            current.scrap = scrap;
            let mut budget = scrap;
            let mut intents = Vec::new();
            policy.production_with_air_demand(
                &dials,
                &current,
                ProductionContext {
                    home: TilePos::new(1, 1),
                    claims: ConstructionClaims {
                        player_facing: true,
                        enlisted: &[],
                        reserved: &[],
                    },
                    outstanding_air_production_ticks: None,
                },
                &mut budget,
                &mut intents,
            );
            (budget, intents)
        };

        assert_eq!(
            decide(false, scout_cost - 1),
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
            decide(true, scout_cost - 1),
            (0, Vec::new()),
            "an incomplete reconnaissance fund must not leak into a cheaper worker order"
        );
        assert_eq!(
            decide(true, scout_cost),
            (
                0,
                vec![Intent::TrainAt {
                    building: BuildingId(2),
                    kind: scout,
                }]
            ),
            "the completed fund must become exactly one faction scout at the Airworks"
        );
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
                ProductionContext {
                    home,
                    claims: ConstructionClaims {
                        player_facing: true,
                        enlisted: &[],
                        reserved: &[],
                    },
                    outstanding_air_production_ticks: None,
                },
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
        policy.scout_dispatch = Some((ground_scout, ground_start, enemy_base));
        policy.scouting(&obs, home, None, &[], &mut Vec::new());
        assert!(policy.air_scout_needed);
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
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
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
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
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

    #[test]
    fn locked_third_foundry_releases_its_capital_to_core_production() {
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
        add_unit(&mut obs, 23, UnitKind::Sentinel, TilePos::new(5, 8));

        let profile = BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, 1_616_304)
            .resolve_profile();
        let dials = Dials::scripted(
            &profile,
            DifficultyTuning::for_level(BotDifficulty::Standard),
        );
        assert_eq!((profile.traits.greed, dials.foundry_cap), (64, 3));

        let mut policy = UtilityPolicy::new();
        assert_eq!(
            policy.capital_reserve(&dials, &obs, home, true, &[], &[]),
            0,
            "four ordinary fighters must not hoard toward the third Foundry"
        );

        obs.scrap = UnitKind::Sentinel.stats().cost;
        let mut budget = obs.scrap;
        let mut intents = Vec::new();
        policy.production(
            &dials,
            &obs,
            home,
            ConstructionClaims {
                player_facing: true,
                enlisted: &[],
                reserved: &[],
            },
            &mut budget,
            &mut intents,
        );
        assert_eq!(
            intents,
            vec![Intent::TrainAt {
                building: BuildingId(0),
                kind: UnitKind::Sentinel,
            }],
            "the released finite bank should become the missing ordinary fighter"
        );
        assert_eq!(budget, 0);

        add_unit(&mut obs, 24, UnitKind::Sentinel, TilePos::new(6, 8));
        let second_foundry = obs
            .my_buildings
            .iter()
            .position(|building| building.id == BuildingId(10))
            .expect("the fixture has a second Foundry");
        obs.my_queues[second_foundry].push(UnitKind::Sentinel);
        let foundry_fund = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundry has construction stats")
            .cost
            + TECH_RESERVE;
        assert_eq!(
            policy.capital_reserve(&dials, &obs, home, true, &[], &[]),
            foundry_fund,
            "five live plus one queued ordinary fighter must unlock the exact capital reserve"
        );
    }

    #[test]
    fn third_foundry_capital_matches_the_exact_safe_construction_claim() {
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
        for id in 23..26 {
            add_unit(
                &mut obs,
                id,
                UnitKind::Sentinel,
                TilePos::new(5 + i32::try_from(id - 23).unwrap(), 8),
            );
        }
        obs.scrap = 10_000;

        let profile = BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, 1_616_304)
            .resolve_profile();
        let mut dials = Dials::scripted(
            &profile,
            DifficultyTuning::for_level(BotDifficulty::Standard),
        );
        dials.extractors = false;
        dials.upgrades = false;
        let fund = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundry has construction stats")
            .cost
            + TECH_RESERVE;

        let mut policy = UtilityPolicy::new();
        assert_eq!(
            policy.capital_reserve(&dials, &obs, home, true, &[], &[]),
            fund
        );
        let mut budget = obs.scrap;
        let mut intents = Vec::new();
        policy.construction(
            &dials,
            &obs,
            home,
            ConstructionClaims {
                player_facing: true,
                enlisted: &[],
                reserved: &[],
            },
            &mut budget,
            &mut intents,
        );
        assert!(matches!(
            intents.as_slice(),
            [Intent::BuildWith {
                builder,
                kind: BuildingKind::Foundry,
                anchor,
            }] if obs.my_units.iter().any(|unit| unit.id == *builder)
                && UtilityPolicy::foundry_supports_extractor(*anchor, extractor)
        ));

        obs.my_buildings
            .retain(|building| building.kind != BuildingKind::Extractor);
        obs.my_queues.truncate(obs.my_buildings.len());
        obs.known_scrap = vec![(extractor, 800)];
        add_enemy_building(
            &mut obs,
            90,
            BuildingKind::Foundry,
            extractor.offset(2, 0),
            true,
        );

        let mut refused = UtilityPolicy::new();
        assert_eq!(
            refused.capital_reserve(&dials, &obs, home, true, &[], &[]),
            0,
            "a generic frontier controlled by a nearer known enemy base must not strand the Foundry fund"
        );
        let mut budget = obs.scrap;
        let mut intents = Vec::new();
        refused.construction(
            &dials,
            &obs,
            home,
            ConstructionClaims {
                player_facing: true,
                enlisted: &[],
                reserved: &[],
            },
            &mut budget,
            &mut intents,
        );
        assert!(
            intents.iter().all(|intent| !matches!(
                intent,
                Intent::Build {
                    kind: BuildingKind::Foundry,
                    ..
                } | Intent::BuildWith {
                    kind: BuildingKind::Foundry,
                    ..
                }
            )),
            "capital refusal and construction must agree on the enemy-controlled frontier: {intents:?}"
        );
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
            policy.capital_reserve(&dials, &obs, home, true, &[], &[]),
            crucible_fund,
            "a frontier the construction channel cannot reach must yield to the next legal tech rung"
        );
        assert_eq!(
            policy.capital_reserve(&dials, &obs, home, false, &[], &[]),
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

        // A complete fresh look starts confirmation, but a scout merely
        // passing between recurring attacks must not reopen the route.
        obs.visible.fill(true);
        policy.refresh_contested_harvest_regions(&obs, None, None);
        intents.clear();
        policy.economy(&obs, TilePos::new(1, 1), true, None, None, &mut intents);
        assert_eq!(
            intents,
            vec![Intent::AssignHarvest {
                unit: UnitId(4),
                node: safe_fallback,
            }],
            "one clear pass must not immediately reopen a recurring kill zone"
        );

        obs.tick += CONTESTED_CLEAR_CONFIRM_TICKS - 1;
        policy.refresh_contested_harvest_regions(&obs, None, None);
        assert!(policy.harvest_location_contested(wreck));

        let mut attacker = obs.my_units[0].clone();
        attacker.id = UnitId(90);
        attacker.player = PlayerId(1);
        attacker.kind = UnitKind::Sentinel;
        attacker.tile = source;
        attacker.hp = UnitKind::Sentinel.stats().max_hp;
        obs.enemy_units.push(attacker);
        obs.tick += 1;
        policy.refresh_contested_harvest_regions(&obs, None, None);
        assert!(policy.harvest_location_contested(wreck));

        obs.enemy_units.clear();
        obs.tick += 1;
        policy.refresh_contested_harvest_regions(&obs, None, None);
        obs.tick += CONTESTED_CLEAR_CONFIRM_TICKS - 1;
        policy.refresh_contested_harvest_regions(&obs, None, None);
        assert!(
            policy.harvest_location_contested(wreck),
            "renewed danger must restart the uninterrupted-clear clock"
        );

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
            "sustained fresh clear reconnaissance must make the salvage usable again"
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
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
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
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
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
    fn a_contested_loss_covers_the_harvesters_entire_autonomous_work_zone() {
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
        obs.salvage_incidents = vec![incident];
        let mut policy = UtilityPolicy::new();
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
            "a safe source anchor cannot authorize autonomous wandering back into the incident"
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
        obs.salvage_incidents = vec![incident];
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
        policy.scout_dispatch = Some((UnitId(3), incident, pending));
        policy.refresh_contested_harvest_regions(&obs, None, None);

        let mut intents = Vec::new();
        policy.evacuate_contested_workers(&obs, home, None, None, &mut intents);
        assert_eq!(
            intents,
            vec![Intent::MoveUnits {
                units: vec![UnitId(3), UnitId(4)],
                goal: TilePos::new(2, 0),
            }]
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
                goal: TilePos::new(2, 0),
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
        obs.salvage_incidents = vec![incident];
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
            },
            &mut budget,
            &mut intents,
        );
        assert!(
            intents.is_empty(),
            "repair must not send the newly safe worker straight back into the durable kill-zone memory"
        );

        // Repair becomes eligible again only after the same sustained clear
        // reconnaissance that reopens harvesting in this region.
        obs.visible.fill(true);
        policy.refresh_contested_harvest_regions(&obs, None, None);
        obs.tick += CONTESTED_CLEAR_CONFIRM_TICKS;
        policy.refresh_contested_harvest_regions(&obs, None, None);
        policy.repairs(
            &Dials::full(),
            &obs,
            PolicyMode {
                player_facing: true,
                admit_voluntary_macro: true,
                unit_contacts: None,
                building_contacts: None,
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
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
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
