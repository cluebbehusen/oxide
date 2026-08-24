//! Economy and production decision channels.

use super::*;

impl UtilityPolicy {
    /// Economy channel: idle harvesters back to work on the nearest
    /// known node that hasn't bounced anyone. A node only qualifies if
    /// it sits no deeper in their half than ours — a returning scout
    /// must not be "efficiently" assigned to mine at the enemy's
    /// doorstep.
    pub(super) fn economy(&mut self, obs: &Observation, home: TilePos, intents: &mut Vec<Intent>) {
        let enemy_base = obs
            .enemy_buildings
            .iter()
            .map(|b| (b.anchor.manhattan(home), b.anchor.y, b.anchor.x))
            .min()
            .map(|(_, y, x)| TilePos::new(x, y));
        for u in obs
            .my_units
            .iter()
            .filter(|u| u.kind.stats().harvest.is_some() && u.idle && Some(u.id) != self.scout)
        {
            let node = obs
                .known_scrap
                .iter()
                .chain(obs.known_wrecks.iter())
                .filter(|(pos, amount)| {
                    *amount > 0
                        && !self.dead_nodes.contains(pos)
                        && enemy_base.is_none_or(|eb| pos.manhattan(home) <= pos.manhattan(eb))
                })
                .map(|(pos, _)| (pos.manhattan(u.tile), pos.y, pos.x))
                .min()
                .map(|(_, y, x)| TilePos::new(x, y));
            if let Some(node) = node {
                intents.push(Intent::AssignHarvest { unit: u.id, node });
                self.last_sent.push((u.id, node, u.tile));
            }
        }
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
        budget: &mut u32,
        intents: &mut Vec<Intent>,
    ) {
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
        let capital = if screen < 3 || (self.desperate && self.desperate_road) {
            0
        } else {
            Self::capital_reserve(dials, obs)
        };

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
        if dials.ferry
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
        if dials.deep_tech {
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
            if harvesters < dials.harvester_target as usize
                && *budget >= UnitKind::Harvester.stats().cost
            {
                *budget -= UnitKind::Harvester.stats().cost;
                intents.push(Intent::TrainAt {
                    building: foundry.id,
                    kind: UnitKind::Harvester,
                });
            } else if *budget >= UnitKind::Sentinel.stats().cost + capital {
                *budget -= UnitKind::Sentinel.stats().cost;
                intents.push(Intent::TrainAt {
                    building: foundry.id,
                    kind: UnitKind::Sentinel,
                });
            }
        }

        if !dials.tech {
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
            use crate::stats::{Domain, Role};
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
                .filter(|u| u.kind.stats().domain == Domain::Air)
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
            } else if alive(UnitKind::Scuttler) < 4
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
            } else if dials.air_harass
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
            } else if fab_open && *budget >= lancer + reserve + capital {
                *budget -= lancer;
                intents.push(Intent::TrainAt {
                    building: fab.id,
                    kind: UnitKind::Lancer,
                });
            }
        }
    }
    /// The next owed tech rung's price plus the fighting reserve — the
    /// fund the unbounded military drip must leave untouched so the
    /// construction channel can ever afford to climb. Zero once the
    /// dials' tree is fully raised (a standing site counts: its cost is
    /// already spent).
    fn capital_reserve(dials: &Dials, obs: &Observation) -> u32 {
        if !dials.tech {
            return 0;
        }
        let have = |kind: BuildingKind| obs.my_buildings.iter().any(|b| b.kind == kind);
        let price =
            |kind: BuildingKind| kind.base_stats().construction.map(|c| c.cost).unwrap_or(0);
        let mut rungs = vec![BuildingKind::Fabricator];
        if dials.deep_tech {
            rungs.push(BuildingKind::Airworks);
        }
        // The expansion Foundry is a capital rung too. Use any known
        // salvage beyond the radius as a cheap frontier proxy; the
        // construction channel still proves reachability before claiming.
        if dials.expansion && (!dials.deep_tech || have(BuildingKind::Airworks)) {
            let foundries: Vec<TilePos> = obs
                .my_buildings
                .iter()
                .filter(|b| b.kind == BuildingKind::Foundry)
                .map(|b| b.anchor)
                .collect();
            let frontier = obs
                .known_scrap
                .iter()
                .filter(|(_, amount)| *amount > 0)
                .map(|(tile, _)| *tile)
                .chain(obs.known_frames.iter().copied())
                .any(|tile| {
                    foundries
                        .iter()
                        .all(|f| f.chebyshev(tile) > EXPANSION_RADIUS)
                });
            if frontier && foundries.len() < 2 && have(BuildingKind::Foundry) {
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
