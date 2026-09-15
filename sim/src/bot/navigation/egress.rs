//! Retained producer-exit certificates for hypothetical construction layouts.
use crate::bot::observation::Observation;
use crate::stats::{BuildingKind, Domain};
use chassis::grid::TilePos;
type PlannedFootprint = (BuildingKind, TilePos);

#[derive(Debug, Clone, PartialEq, Eq)]
struct GroundProducerEgress {
    ring: Vec<TilePos>,
    witnesses: Vec<TilePos>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::bot) struct GroundEgressCertificate {
    routes: Vec<Vec<TilePos>>,
    route_tiles: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GroundEgressLayout {
    map_size: (i32, i32),
    known_rock: Vec<TilePos>,
    known_scrap: Vec<TilePos>,
    blocking_buildings: Vec<PlannedFootprint>,
    founding: Vec<PlannedFootprint>,
    producers: Vec<(BuildingKind, TilePos, u8)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::bot) struct GroundEgressCache {
    layout: GroundEgressLayout,
    base_open: Vec<bool>,
    producers: Vec<GroundProducerEgress>,
    decisions: std::collections::BTreeMap<
        Vec<PlannedFootprint>,
        Option<std::sync::Arc<GroundEgressCertificate>>,
    >,
}

impl GroundEgressLayout {
    fn from_observation(obs: &Observation) -> Self {
        let known_scrap = obs.known_scrap.iter().map(|(tile, _)| *tile).collect();
        let mut blocking_buildings: Vec<_> = obs
            .my_buildings
            .iter()
            .chain(obs.ally_buildings.iter())
            .chain(obs.enemy_buildings.iter())
            .filter(|building| !building.kind.is_stealthy())
            .map(|building| (building.kind, building.anchor))
            .collect();
        blocking_buildings.sort_unstable();
        blocking_buildings.dedup();

        let mut founding: Vec<_> = obs
            .my_units
            .iter()
            .filter_map(|unit| unit.founding)
            .filter(|(kind, _)| !kind.is_stealthy())
            .collect();
        founding.sort_unstable();
        founding.dedup();

        let mut producers: Vec<_> = obs
            .my_buildings
            .iter()
            .filter(|building| {
                building.built
                    && building
                        .kind
                        .tier_stats(building.tier)
                        .produces
                        .iter()
                        .any(|unit| unit.stats().domain == Domain::Ground)
            })
            .map(|building| (building.kind, building.anchor, building.tier))
            .collect();
        producers.sort_unstable();

        Self {
            map_size: (obs.map_width, obs.map_height),
            known_rock: obs.known_rock.clone(),
            known_scrap,
            blocking_buildings,
            founding,
            producers,
        }
    }
}

impl GroundEgressCache {
    pub(in crate::bot) fn prepare(slot: &mut Option<Self>, obs: &Observation) {
        let layout = GroundEgressLayout::from_observation(obs);
        let layout_changed = slot.as_ref().is_none_or(|cache| cache.layout != layout);
        if layout_changed {
            #[cfg(test)]
            super::work::record(|work| {
                work.generations += 1;
            });

            let base_open = Self::ground_egress_base_open(obs);
            let producers = Self::ground_producer_egress(obs, &base_open);
            let certificate =
                Self::ground_egress_certificate(&base_open, layout.map_size, &producers)
                    .map(std::sync::Arc::new);
            *slot = Some(GroundEgressCache {
                layout,
                base_open,
                producers,
                decisions: std::collections::BTreeMap::from([(Vec::new(), certificate)]),
            });
        }
    }

    pub(in crate::bot) fn preserves(
        slot: &mut Option<Self>,
        accepted: &[PlannedFootprint],
        candidate: PlannedFootprint,
    ) -> bool {
        if candidate.0.is_stealthy() {
            return true;
        }

        let mut accepted = accepted.to_vec();
        accepted.retain(|(kind, _)| !kind.is_stealthy());
        accepted.sort_unstable();
        accepted.dedup();

        let cache = slot
            .as_mut()
            .expect("ground-producer egress must be prepared before placement checks");
        let accepted_certificate = if let Some(certificate) = cache.decisions.get(&accepted) {
            certificate.clone()
        } else {
            let open = Self::ground_egress_open_with_plans(
                &cache.base_open,
                cache.layout.map_size,
                &accepted,
            );
            let certificate =
                Self::ground_egress_certificate(&open, cache.layout.map_size, &cache.producers)
                    .map(std::sync::Arc::new);
            cache
                .decisions
                .insert(accepted.clone(), certificate.clone());
            certificate
        };
        let Some(accepted_certificate) = accepted_certificate else {
            return false;
        };

        let mut planned = accepted.clone();
        planned.push(candidate);
        planned.sort_unstable();
        planned.dedup();
        if planned == accepted {
            return true;
        }
        if let Some(certificate) = cache.decisions.get(&planned) {
            return certificate.is_some();
        }

        let affected = Self::certificate_routes_blocked(
            &accepted_certificate,
            candidate,
            cache.layout.map_size,
        );
        let certificate = if affected.is_empty() {
            Some(accepted_certificate)
        } else {
            let open = Self::ground_egress_open_with_plans(
                &cache.base_open,
                cache.layout.map_size,
                &planned,
            );
            let mut routes = accepted_certificate.routes.clone();
            let mut valid = true;
            for producer_index in affected {
                let Some(route) = Self::ground_producer_route(
                    &open,
                    cache.layout.map_size,
                    &cache.producers[producer_index],
                ) else {
                    valid = false;
                    break;
                };
                routes[producer_index] = route;
            }
            valid.then(|| {
                std::sync::Arc::new(Self::ground_egress_certificate_from_routes(
                    routes,
                    cache.layout.map_size,
                ))
            })
        };
        let result = certificate.is_some();
        cache.decisions.insert(planned, certificate);
        result
    }

    fn ground_egress_certificate(
        open: &[bool],
        map_size: (i32, i32),
        producers: &[GroundProducerEgress],
    ) -> Option<GroundEgressCertificate> {
        let routes: Option<Vec<_>> = producers
            .iter()
            .map(|producer| Self::ground_producer_route(open, map_size, producer))
            .collect();
        routes.map(|routes| Self::ground_egress_certificate_from_routes(routes, map_size))
    }

    fn ground_egress_certificate_from_routes(
        routes: Vec<Vec<TilePos>>,
        map_size: (i32, i32),
    ) -> GroundEgressCertificate {
        let cells = usize::try_from(map_size.0)
            .ok()
            .and_then(|width| {
                usize::try_from(map_size.1)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .unwrap_or(0);
        let mut route_tiles = vec![0; cells.div_ceil(64)];
        for route in &routes {
            for tile in route {
                let index = (tile.y * map_size.0 + tile.x) as usize;
                route_tiles[index / 64] |= 1 << (index % 64);
            }
        }
        GroundEgressCertificate {
            routes,
            route_tiles,
        }
    }

    pub(in crate::bot) fn certificate_routes_blocked(
        certificate: &GroundEgressCertificate,
        candidate: PlannedFootprint,
        map_size: (i32, i32),
    ) -> Vec<usize> {
        let (kind, anchor) = candidate;
        let (width, height) = kind.base_stats().size;
        let intersects_route = (0..height).any(|dy| {
            (0..width).any(|dx| {
                let tile = anchor.offset(dx, dy);
                if tile.x < 0 || tile.y < 0 || tile.x >= map_size.0 || tile.y >= map_size.1 {
                    return false;
                }
                let index = (tile.y * map_size.0 + tile.x) as usize;
                certificate.route_tiles[index / 64] & (1 << (index % 64)) != 0
            })
        });
        if !intersects_route {
            return Vec::new();
        }
        certificate
            .routes
            .iter()
            .enumerate()
            .filter(|(_, route)| {
                route
                    .iter()
                    .any(|tile| Self::candidate_blocks(kind, anchor, *tile))
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn ground_producer_route(
        open: &[bool],
        map_size: (i32, i32),
        producer: &GroundProducerEgress,
    ) -> Option<Vec<TilePos>> {
        let index = |tile: TilePos| (tile.y * map_size.0 + tile.x) as usize;
        let in_bounds = |tile: TilePos| {
            tile.x >= 0 && tile.y >= 0 && tile.x < map_size.0 && tile.y < map_size.1
        };
        let witness = producer
            .witnesses
            .iter()
            .copied()
            .find(|tile| open[index(*tile)])?;
        let spawn = producer
            .ring
            .iter()
            .copied()
            .find(|tile| in_bounds(*tile) && open[index(*tile)])?;
        crate::bot::navigation::flood::cardinal_path(open, map_size, spawn, witness)
    }

    #[cfg(test)]
    pub(in crate::bot) fn planned_footprints_block(
        planned: &[PlannedFootprint],
        tile: TilePos,
    ) -> bool {
        planned
            .iter()
            .any(|(kind, anchor)| Self::candidate_blocks(*kind, *anchor, tile))
    }

    pub(in crate::bot) fn ground_egress_base_open(obs: &Observation) -> Vec<bool> {
        let cells = usize::try_from(obs.map_width)
            .ok()
            .and_then(|width| {
                usize::try_from(obs.map_height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .unwrap_or(0);
        let mut open = vec![true; cells];
        let mut block = |tile: TilePos| {
            if tile.x >= 0 && tile.y >= 0 && tile.x < obs.map_width && tile.y < obs.map_height {
                open[(tile.y * obs.map_width + tile.x) as usize] = false;
            }
        };
        for tile in &obs.known_rock {
            block(*tile);
        }
        for (tile, _) in &obs.known_scrap {
            block(*tile);
        }
        for building in obs
            .my_buildings
            .iter()
            .chain(obs.ally_buildings.iter())
            .chain(obs.enemy_buildings.iter())
            .filter(|building| !building.kind.is_stealthy())
        {
            let (width, height) = building.kind.base_stats().size;
            for dy in 0..height {
                for dx in 0..width {
                    block(building.anchor.offset(dx, dy));
                }
            }
        }
        for (kind, anchor) in obs.my_units.iter().filter_map(|unit| unit.founding) {
            if kind.is_stealthy() {
                continue;
            }
            let (width, height) = kind.base_stats().size;
            for dy in 0..height {
                for dx in 0..width {
                    block(anchor.offset(dx, dy));
                }
            }
        }
        open
    }

    fn ground_egress_open_with_plans(
        base_open: &[bool],
        map_size: (i32, i32),
        planned: &[PlannedFootprint],
    ) -> Vec<bool> {
        let mut open = base_open.to_vec();
        for (kind, anchor) in planned {
            if kind.is_stealthy() {
                continue;
            }
            let (width, height) = kind.base_stats().size;
            for dy in 0..height {
                for dx in 0..width {
                    let tile = anchor.offset(dx, dy);
                    if tile.x >= 0 && tile.y >= 0 && tile.x < map_size.0 && tile.y < map_size.1 {
                        open[(tile.y * map_size.0 + tile.x) as usize] = false;
                    }
                }
            }
        }
        open
    }

    fn ground_producer_egress(obs: &Observation, base_open: &[bool]) -> Vec<GroundProducerEgress> {
        let map_size = (obs.map_width, obs.map_height);
        let labels = crate::bot::navigation::flood::labels(base_open, map_size);
        let index = |tile: TilePos| (tile.y * obs.map_width + tile.x) as usize;
        obs.my_buildings
            .iter()
            .filter(|building| {
                building.built
                    && building
                        .kind
                        .tier_stats(building.tier)
                        .produces
                        .iter()
                        .any(|unit| unit.stats().domain == Domain::Ground)
            })
            .filter_map(|producer| {
                let ring: Vec<_> = crate::tick::rect_adjacent_tiles(
                    producer.anchor,
                    producer.kind.tier_stats(producer.tier).size,
                )
                .collect();
                let current_spawn = ring.iter().copied().find(|tile| {
                    tile.x >= 0
                        && tile.y >= 0
                        && tile.x < obs.map_width
                        && tile.y < obs.map_height
                        && base_open[index(*tile)]
                })?;
                let component = labels[index(current_spawn)];
                let mut witnesses: Vec<_> = labels
                    .iter()
                    .enumerate()
                    .filter(|(_, label)| **label == component)
                    .map(|(index, _)| {
                        TilePos::new(index as i32 % obs.map_width, index as i32 / obs.map_width)
                    })
                    .collect();
                witnesses.sort_unstable_by_key(|tile| {
                    (
                        std::cmp::Reverse(tile.chebyshev(producer.anchor)),
                        tile.y,
                        tile.x,
                    )
                });
                Some(GroundProducerEgress { ring, witnesses })
            })
            .collect()
    }

    pub(in crate::bot) fn candidate_blocks(
        kind: BuildingKind,
        anchor: TilePos,
        tile: TilePos,
    ) -> bool {
        if kind.is_stealthy() {
            return false;
        }
        let (width, height) = kind.base_stats().size;
        (anchor.x..anchor.x + width).contains(&tile.x)
            && (anchor.y..anchor.y + height).contains(&tile.y)
    }

    #[cfg(test)]
    pub(in crate::bot) fn same_layout(a: &Observation, b: &Observation) -> bool {
        GroundEgressLayout::from_observation(a) == GroundEgressLayout::from_observation(b)
    }
    #[cfg(test)]
    pub(in crate::bot) fn base_open(&self) -> &[bool] {
        &self.base_open
    }
    #[cfg(test)]
    pub(in crate::bot) fn producer_count(&self) -> usize {
        self.producers.len()
    }
    #[cfg(test)]
    pub(in crate::bot) fn map_size(&self) -> (i32, i32) {
        self.layout.map_size
    }
    #[cfg(test)]
    pub(in crate::bot) fn decisions(
        &self,
    ) -> &std::collections::BTreeMap<
        Vec<PlannedFootprint>,
        Option<std::sync::Arc<GroundEgressCertificate>>,
    > {
        &self.decisions
    }
}
#[cfg(test)]
impl GroundEgressCertificate {
    pub(in crate::bot) fn routes(&self) -> &[Vec<TilePos>] {
        &self.routes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nonblocking_candidate_needs_no_prepared_route_generation() {
        let mut cache = None;
        assert!(GroundEgressCache::preserves(
            &mut cache,
            &[],
            (BuildingKind::ScuttleCharge, TilePos::new(0, 0))
        ));
        assert!(cache.is_none());
    }
}
