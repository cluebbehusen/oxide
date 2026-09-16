//! Retained producer-exit certificates for hypothetical construction layouts.
use crate::bot::observation::Observation;
use crate::stats::{BuildingKind, Domain};
use chassis::grid::TilePos;
type PlannedFootprint = (BuildingKind, TilePos);

#[derive(Debug, Clone, PartialEq, Eq)]
struct GroundProducerEgress {
    anchor: TilePos,
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
    /// A positive answer reuses a route proof; a negative answer needs refinement.
    pub(in crate::bot) fn certifies(&self, candidate: PlannedFootprint) -> bool {
        if candidate.0.is_stealthy() {
            return true;
        }
        self.decisions
            .get(&Vec::new())
            .and_then(Option::as_ref)
            .is_some_and(|certificate| {
                let (width, height) = candidate.0.base_stats().size;
                (0..height).all(|dy| {
                    (0..width).all(|dx| {
                        let tile = candidate.1.offset(dx, dy);
                        super::flood::tile_index(
                            self.layout.map_size.0,
                            self.layout.map_size.1,
                            tile,
                        )
                        .is_none_or(|index| {
                            certificate.route_tiles[index / 64] & (1 << (index % 64)) == 0
                        })
                    })
                })
            })
    }

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
                let Some(route) = Self::repair_route(
                    &open,
                    cache.layout.map_size,
                    &routes[producer_index],
                    candidate,
                )
                .or_else(|| {
                    Self::ground_producer_route(
                        &open,
                        cache.layout.map_size,
                        &cache.producers[producer_index],
                    )
                }) else {
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

    fn repair_route(
        open: &[bool],
        map_size: (i32, i32),
        route: &[TilePos],
        candidate: PlannedFootprint,
    ) -> Option<Vec<TilePos>> {
        let blocked = |tile: &TilePos| Self::candidate_blocks(candidate.0, candidate.1, *tile);
        let first = route.iter().position(blocked)?;
        let last = route.iter().rposition(blocked)?;
        // A blocked endpoint requires choosing the producer's next canonical door or witness.
        let before = first.checked_sub(1)?;
        let after = last + 1;
        let goal = *route.get(after)?;
        let start = route[before];
        let size = candidate.0.base_stats().size;
        let left = (candidate.1.x - 2).max(0);
        let top = (candidate.1.y - 2).max(0);
        let right = (candidate.1.x + size.0 + 2).min(map_size.0);
        let bottom = (candidate.1.y + size.1 + 2).min(map_size.1);
        let local_size = (right - left, bottom - top);
        let local_open: Vec<_> = (top..bottom)
            .flat_map(|y| (left..right).map(move |x| open[(y * map_size.0 + x) as usize]))
            .collect();
        let repair = crate::bot::navigation::flood::cardinal_path(
            &local_open,
            local_size,
            start.offset(-left, -top),
            goal.offset(-left, -top),
        )?;
        Some(
            route[..before]
                .iter()
                .copied()
                .chain(repair.into_iter().map(|tile| tile.offset(left, top)))
                .chain(route[after + 1..].iter().copied())
                .collect(),
        )
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
            .filter(|tile| open[index(*tile)])
            .min_by_key(|tile| {
                (
                    std::cmp::Reverse(tile.chebyshev(producer.anchor)),
                    tile.y,
                    tile.x,
                )
            })?;
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
                let witnesses: Vec<_> = labels
                    .iter()
                    .enumerate()
                    .filter(|(_, label)| **label == component)
                    .map(|(index, _)| {
                        TilePos::new(index as i32 % obs.map_width, index as i32 / obs.map_width)
                    })
                    .collect();
                Some(GroundProducerEgress {
                    anchor: producer.anchor,
                    ring,
                    witnesses,
                })
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
    fn interior_route_repairs_have_local_work_on_large_maps() {
        let map_size = (160, 160);
        let mut open = vec![true; 160 * 160];
        let route: Vec<_> = (0..160).map(|x| TilePos::new(x, 80)).collect();
        let candidate = (BuildingKind::Barricade, TilePos::new(80, 80));
        open[80 * 160 + 80] = false;
        let (repaired, work) = crate::bot::navigation::work::measure(|| {
            GroundEgressCache::repair_route(&open, map_size, &route, candidate).unwrap()
        });
        assert_eq!(work.searches, 1);
        assert!(work.expanded <= 25, "{work:?}");
        assert_eq!(repaired.first(), route.first());
        assert_eq!(repaired.last(), route.last());
        assert!(
            repaired
                .iter()
                .all(|tile| open[(tile.y * 160 + tile.x) as usize])
        );
        assert!(
            repaired
                .windows(2)
                .all(|pair| pair[0].manhattan(pair[1]) == 1)
        );
    }

    #[test]
    fn witness_selection_matches_the_sorted_reference_after_blocking_candidates() {
        let map_size = (8, 8);
        let witnesses: Vec<_> = (0..64).rev().map(|i| TilePos::new(i % 8, i / 8)).collect();
        for anchor in [TilePos::new(0, 0), TilePos::new(4, 4), TilePos::new(7, 7)] {
            let producer = GroundProducerEgress {
                anchor,
                ring: vec![TilePos::new(3, 3)],
                witnesses: witnesses.clone(),
            };
            let mut sorted = witnesses.clone();
            sorted.sort_unstable_by_key(|tile| {
                (std::cmp::Reverse(tile.chebyshev(anchor)), tile.y, tile.x)
            });
            let mut open = vec![true; 64];
            for blocked in sorted.iter().take(24) {
                open[(blocked.y * 8 + blocked.x) as usize] = false;
                let expected = sorted
                    .iter()
                    .copied()
                    .find(|tile| open[(tile.y * 8 + tile.x) as usize])
                    .unwrap();
                let expected_route = crate::bot::navigation::flood::cardinal_path(
                    &open,
                    map_size,
                    producer.ring[0],
                    expected,
                );
                assert_eq!(
                    GroundEgressCache::ground_producer_route(&open, map_size, &producer),
                    expected_route
                );
            }
        }
    }

    #[test]
    fn local_repair_failure_preserves_endpoint_selection_and_global_detours() {
        let map_size = (12, 12);
        let mut open = vec![true; 144];
        let route: Vec<_> = (0..12).map(|x| TilePos::new(x, 6)).collect();
        let producer = GroundProducerEgress {
            anchor: TilePos::new(11, 0),
            ring: vec![route[0], TilePos::new(0, 5)],
            witnesses: vec![route[11], TilePos::new(11, 5)],
        };
        for y in 2..11 {
            open[y * 12 + 6] = y == 6;
        }
        open[6 * 12 + 6] = false;
        assert!(
            GroundEgressCache::repair_route(
                &open,
                map_size,
                &route,
                (BuildingKind::Barricade, route[6])
            )
            .is_none()
        );
        let detour = GroundEgressCache::ground_producer_route(&open, map_size, &producer).unwrap();
        assert!(detour.iter().any(|tile| tile.y == 1 || tile.y == 11));
        for endpoint in [route[0], route[11]] {
            let mut blocked_endpoint = vec![true; 144];
            blocked_endpoint[(endpoint.y * 12 + endpoint.x) as usize] = false;
            assert!(
                GroundEgressCache::repair_route(
                    &blocked_endpoint,
                    map_size,
                    &route,
                    (BuildingKind::Barricade, endpoint)
                )
                .is_none()
            );
            let alternate =
                GroundEgressCache::ground_producer_route(&blocked_endpoint, map_size, &producer)
                    .unwrap();
            assert_eq!(
                alternate.first(),
                Some(&if endpoint == route[0] {
                    producer.ring[1]
                } else {
                    route[0]
                })
            );
            assert_eq!(
                alternate.last(),
                Some(&if endpoint == route[11] {
                    producer.witnesses[1]
                } else {
                    route[11]
                })
            );
        }
    }

    #[test]
    fn repaired_certificates_match_full_connectivity_across_obstacles_and_placements() {
        let map_size = (6, 6);
        let producer = GroundProducerEgress {
            anchor: TilePos::new(0, 2),
            ring: vec![TilePos::new(0, 2), TilePos::new(0, 3)],
            witnesses: vec![TilePos::new(5, 2), TilePos::new(5, 3)],
        };
        for mask in 0..512 {
            let mut base = vec![true; 36];
            for bit in 0..9 {
                base[(bit / 3 + 1) * 6 + bit % 3 + 1] = mask & (1 << bit) == 0;
            }
            let Some(route) = GroundEgressCache::ground_producer_route(&base, map_size, &producer)
            else {
                continue;
            };
            for index in 0..36 {
                let candidate = (BuildingKind::Barricade, TilePos::new(index % 6, index / 6));
                let mut open = base.clone();
                open[index as usize] = false;
                let expected = GroundEgressCache::ground_producer_route(&open, map_size, &producer);
                let repair = GroundEgressCache::repair_route(&open, map_size, &route, candidate);
                if let Some(repair) = &repair {
                    assert!(expected.is_some());
                    assert_eq!(repair.first(), expected.as_ref().unwrap().first());
                    assert_eq!(repair.last(), expected.as_ref().unwrap().last());
                    assert!(
                        repair
                            .iter()
                            .all(|tile| open[(tile.y * 6 + tile.x) as usize])
                    );
                    assert!(
                        repair
                            .windows(2)
                            .all(|pair| pair[0].manhattan(pair[1]) == 1)
                    );
                }
            }
        }
    }

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
