//! Retained producer-exit certificates for hypothetical construction layouts.
use crate::observation::Observation;
#[cfg(test)]
use crate::observation::ObservationData;
use crate::query_work::QueryPurpose;
use crate::resources::FoundationCancellations;
use chassis::grid::TilePos;
use oxide_sim::stats::{BuildingKind, Domain};
type PlannedFootprint = (BuildingKind, TilePos);

#[derive(Debug, Clone, PartialEq, Eq)]
struct ComponentWitnesses {
    rows: Vec<Vec<TilePos>>,
    columns: Vec<Vec<TilePos>>,
}

impl ComponentWitnesses {
    fn new(mut tiles: Vec<TilePos>) -> Self {
        tiles.sort_unstable_by_key(|tile| (tile.y, tile.x));
        let width = tiles.iter().map(|tile| tile.x + 1).max().unwrap_or(0) as usize;
        let height = tiles.iter().map(|tile| tile.y + 1).max().unwrap_or(0) as usize;
        let mut rows = vec![Vec::new(); height];
        let mut columns = vec![Vec::new(); width];
        for tile in tiles {
            rows[tile.y as usize].push(tile);
            columns[tile.x as usize].push(tile);
        }
        Self { rows, columns }
    }

    fn farthest(&self, anchor: TilePos, passable: impl Fn(&TilePos) -> bool) -> Option<TilePos> {
        let first_open = |line: &Vec<TilePos>| line.iter().copied().find(&passable);
        // Chebyshev distance reaches its maximum on a coordinate extremum.
        [
            self.rows.iter().find_map(first_open),
            self.rows.iter().rev().find_map(first_open),
            self.columns.iter().find_map(first_open),
            self.columns.iter().rev().find_map(first_open),
        ]
        .into_iter()
        .flatten()
        .min_by_key(|tile| (std::cmp::Reverse(tile.chebyshev(anchor)), tile.y, tile.x))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GroundProducerEgress {
    anchor: TilePos,
    ring: Vec<TilePos>,
    witnesses: std::sync::Arc<ComponentWitnesses>,
}

impl GroundProducerEgress {
    #[cfg(test)]
    fn new(anchor: TilePos, ring: Vec<TilePos>, witnesses: Vec<TilePos>) -> Self {
        Self {
            anchor,
            ring,
            witnesses: std::sync::Arc::new(ComponentWitnesses::new(witnesses)),
        }
    }

    fn endpoints(&self, open: &[bool], map_size: (i32, i32)) -> Option<(TilePos, TilePos)> {
        let passable = |tile: &TilePos| {
            super::flood::tile_index(map_size.0, map_size.1, *tile).is_some_and(|index| open[index])
        };
        let spawn = self.ring.iter().copied().find(passable)?;
        let witness = self.witnesses.farthest(self.anchor, passable)?;
        Some((spawn, witness))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GroundEgressCertificate {
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
pub(crate) struct GroundEgressCache {
    layout: GroundEgressLayout,
    base_open: Vec<bool>,
    producers: Vec<GroundProducerEgress>,
    decisions: std::collections::BTreeMap<
        Vec<PlannedFootprint>,
        Option<std::sync::Arc<GroundEgressCertificate>>,
    >,
}

const MAX_LAYOUT_DECISIONS: usize = 256;

impl GroundEgressLayout {
    fn from_observation(obs: &Observation, cancellations: FoundationCancellations<'_>) -> Self {
        let known_scrap = obs.known_scrap.iter().map(|(tile, _)| *tile).collect();
        let mut blocking_buildings: Vec<_> = obs
            .my_buildings
            .iter()
            .chain(obs.ally_buildings.iter())
            .chain(obs.enemy_buildings.iter())
            .filter(|building| !building.provisional && !building.kind.is_stealthy())
            .map(|building| (building.kind, building.anchor))
            .collect();
        blocking_buildings.sort_unstable();
        blocking_buildings.dedup();

        let mut founding: Vec<_> = obs
            .my_units
            .iter()
            .filter_map(|unit| cancellations.retained(unit))
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
    pub(crate) fn certifies(&self, candidate: PlannedFootprint) -> bool {
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

    pub(crate) fn prepare(query_purpose: QueryPurpose, slot: &mut Option<Self>, obs: &Observation) {
        Self::prepare_after(query_purpose, slot, obs, FoundationCancellations::default());
    }

    pub(crate) fn prepare_after(
        query_purpose: QueryPurpose,
        slot: &mut Option<Self>,
        obs: &Observation,
        cancellations: FoundationCancellations<'_>,
    ) {
        let layout = GroundEgressLayout::from_observation(obs, cancellations);
        let layout_changed = slot.as_ref().is_none_or(|cache| cache.layout != layout);
        if layout_changed {
            #[cfg(test)]
            super::work::record(|work| {
                work.generations += 1;
            });

            let base_open = Self::ground_egress_base_open(obs, cancellations);
            let producers = Self::ground_producer_egress(query_purpose, obs, &base_open);
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

    pub(crate) fn preserves(
        slot: &mut Option<Self>,
        accepted: &[PlannedFootprint],
        candidate: PlannedFootprint,
    ) -> bool {
        #[cfg(test)]
        super::work::record(|work| work.egress_checks += 1);
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
            cache.retain_decision(accepted.clone(), certificate.clone());
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
                    &cache.producers[producer_index],
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
        cache.retain_decision(planned, certificate);
        result
    }

    fn retain_decision(
        &mut self,
        planned: Vec<PlannedFootprint>,
        certificate: Option<std::sync::Arc<GroundEgressCertificate>>,
    ) {
        if self.decisions.len() >= MAX_LAYOUT_DECISIONS {
            // The empty base layout sorts first and is needed by every fresh placement.
            self.decisions.pop_last();
        }
        self.decisions.insert(planned, certificate);
    }

    fn repair_route(
        open: &[bool],
        map_size: (i32, i32),
        route: &[TilePos],
        candidate: PlannedFootprint,
        producer: &GroundProducerEgress,
    ) -> Option<Vec<TilePos>> {
        let blocked = |tile: &TilePos| Self::candidate_blocks(candidate.0, candidate.1, *tile);
        let first = route.iter().position(blocked)?;
        let last = route.iter().rposition(blocked)?;
        let before = first.saturating_sub(1);
        let after = (last + 2).min(route.len());
        let (spawn, witness) = producer.endpoints(open, map_size)?;
        let start = if first == 0 { spawn } else { route[before] };
        let goal = route.get(last + 1).copied().unwrap_or(witness);
        let size = candidate.0.base_stats().size;
        let left = (candidate.1.x - 2).max(0);
        let top = (candidate.1.y - 2).max(0);
        let right = (candidate.1.x + size.0 + 2).min(map_size.0);
        let bottom = (candidate.1.y + size.1 + 2).min(map_size.1);
        let local_size = (right - left, bottom - top);
        let local_open: Vec<_> = (top..bottom)
            .flat_map(|y| (left..right).map(move |x| open[(y * map_size.0 + x) as usize]))
            .collect();
        let repair = crate::navigation::flood::cardinal_path(
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
                .chain(route[after..].iter().copied())
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
        let cells = super::flood::area(map_size.0, map_size.1);
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

    pub(crate) fn certificate_routes_blocked(
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
        let (spawn, witness) = producer.endpoints(open, map_size)?;
        crate::navigation::flood::cardinal_path(open, map_size, spawn, witness)
    }

    #[cfg(test)]
    pub(crate) fn planned_footprints_block(planned: &[PlannedFootprint], tile: TilePos) -> bool {
        planned
            .iter()
            .any(|(kind, anchor)| Self::candidate_blocks(*kind, *anchor, tile))
    }

    pub(crate) fn ground_egress_base_open(
        obs: &Observation,
        cancellations: FoundationCancellations<'_>,
    ) -> Vec<bool> {
        let cells = super::flood::area(obs.map_width, obs.map_height);
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
            .filter(|building| !building.provisional && !building.kind.is_stealthy())
        {
            let (width, height) = building.kind.base_stats().size;
            for dy in 0..height {
                for dx in 0..width {
                    block(building.anchor.offset(dx, dy));
                }
            }
        }
        for (kind, anchor) in obs
            .my_units
            .iter()
            .filter_map(|unit| cancellations.retained(unit))
        {
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

    fn ground_producer_egress(
        query_purpose: QueryPurpose,
        obs: &Observation,
        base_open: &[bool],
    ) -> Vec<GroundProducerEgress> {
        let map_size = (obs.map_width, obs.map_height);
        let labels = crate::navigation::flood::labels(query_purpose, base_open, map_size);
        let mut components = std::collections::BTreeMap::new();
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
                let ring: Vec<_> = oxide_sim::geometry::rect_adjacent_tiles(
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
                let witnesses = components.entry(component).or_insert_with(|| {
                    let tiles = labels
                        .iter()
                        .enumerate()
                        .filter(|(_, label)| **label == component)
                        .map(|(index, _)| {
                            TilePos::new(index as i32 % obs.map_width, index as i32 / obs.map_width)
                        })
                        .collect();
                    std::sync::Arc::new(ComponentWitnesses::new(tiles))
                });
                Some(GroundProducerEgress {
                    anchor: producer.anchor,
                    ring,
                    witnesses: std::sync::Arc::clone(witnesses),
                })
            })
            .collect()
    }

    pub(crate) fn candidate_blocks(kind: BuildingKind, anchor: TilePos, tile: TilePos) -> bool {
        if kind.is_stealthy() {
            return false;
        }
        let (width, height) = kind.base_stats().size;
        (anchor.x..anchor.x + width).contains(&tile.x)
            && (anchor.y..anchor.y + height).contains(&tile.y)
    }

    #[cfg(test)]
    pub(crate) fn same_layout(a: &Observation, b: &Observation) -> bool {
        GroundEgressLayout::from_observation(a, FoundationCancellations::default())
            == GroundEgressLayout::from_observation(b, FoundationCancellations::default())
    }
    #[cfg(test)]
    pub(crate) fn base_open(&self) -> &[bool] {
        &self.base_open
    }
    #[cfg(test)]
    pub(crate) fn producer_count(&self) -> usize {
        self.producers.len()
    }
    #[cfg(test)]
    pub(crate) fn map_size(&self) -> (i32, i32) {
        self.layout.map_size
    }
    #[cfg(test)]
    pub(crate) fn decisions(
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
    pub(crate) fn routes(&self) -> &[Vec<TilePos>] {
        &self.routes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hypothetical_layout_eviction_preserves_cold_egress_answers() {
        use crate::observation::BuildingObs;
        use oxide_sim::ids::PlayerId;

        let obs = Observation::from_data(ObservationData {
            map_width: 32,
            map_height: 32,
            my_buildings: vec![BuildingObs {
                hp: 1,
                ..crate::test_support::building(
                    0,
                    PlayerId(0),
                    BuildingKind::Foundry,
                    TilePos::new(2, 2),
                )
            }],
            ..crate::test_support::observation_data()
        });
        let mut retained = None;
        GroundEgressCache::prepare(QueryPurpose::NavigationTest, &mut retained, &obs);
        let cold = retained.clone();
        for x in (5..25).chain((5..25).rev()) {
            for y in 5..25 {
                let candidate = (BuildingKind::Barricade, TilePos::new(x, y));
                assert_eq!(
                    GroundEgressCache::preserves(&mut retained, &[], candidate),
                    GroundEgressCache::preserves(&mut cold.clone(), &[], candidate)
                );
                let cache = retained.as_ref().unwrap();
                assert!(cache.decisions.len() <= MAX_LAYOUT_DECISIONS);
                assert!(cache.decisions.contains_key(&Vec::new()));
            }
        }
        assert_eq!(retained.unwrap().decisions.len(), MAX_LAYOUT_DECISIONS);
    }

    #[test]
    fn provisional_sites_preserve_egress_until_activation() {
        use crate::navigation::commands::RouteProjection;
        use crate::observation::BuildingObs;
        use oxide_sim::ids::PlayerId;

        for owner in 0..3 {
            let mut obs = Observation::from_data(ObservationData {
                map_width: 16,
                map_height: 16,
                my_buildings: vec![BuildingObs {
                    hp: 1,
                    ..crate::test_support::building(
                        0,
                        PlayerId(0),
                        BuildingKind::Foundry,
                        TilePos::new(2, 2),
                    )
                }],
                ..crate::test_support::observation_data()
            });
            let mut cache = None;
            GroundEgressCache::prepare(QueryPurpose::NavigationTest, &mut cache, &obs);
            let before = cache.clone().unwrap();
            let site = BuildingObs {
                hp: 1,
                built: false,
                provisional: true,
                ..crate::test_support::building(
                    1,
                    PlayerId(owner),
                    BuildingKind::Barricade,
                    TilePos::new(8, 8),
                )
            };
            let buildings = match owner {
                0 => &mut obs.my_buildings,
                1 => &mut obs.ally_buildings,
                _ => &mut obs.enemy_buildings,
            };
            buildings.push(site);
            let (_, work) = super::super::work::measure(|| {
                GroundEgressCache::prepare(QueryPurpose::NavigationTest, &mut cache, &obs);
            });
            let routes = RouteProjection::new(QueryPurpose::NavigationTest, &obs, Domain::Ground);
            assert!(routes.open(TilePos::new(8, 8)));
            assert_eq!(
                GroundEgressCache::ground_egress_base_open(
                    &obs,
                    FoundationCancellations::default()
                ),
                before.base_open
            );
            assert_eq!(work.generations, 0);
            assert_eq!(cache.as_ref().unwrap(), &before);

            match owner {
                0 => obs.my_buildings.last_mut(),
                1 => obs.ally_buildings.last_mut(),
                _ => obs.enemy_buildings.last_mut(),
            }
            .unwrap()
            .provisional = false;
            let (_, work) = super::super::work::measure(|| {
                GroundEgressCache::prepare(QueryPurpose::NavigationTest, &mut cache, &obs);
            });
            assert_eq!(work.generations, 1);
            assert!(!cache.as_ref().unwrap().base_open[8 * 16 + 8]);
            let routes = RouteProjection::new(QueryPurpose::NavigationTest, &obs, Domain::Ground);
            assert!(!routes.open(TilePos::new(8, 8)));
        }
    }

    #[test]
    fn producers_and_controller_clones_share_only_their_current_component_index() {
        let mut obs = Observation::from_data(ObservationData {
            map_width: 128,
            map_height: 128,
            my_buildings: (0..8)
                .map(|id| crate::observation::BuildingObs {
                    hp: 1,
                    ..crate::test_support::building(
                        id,
                        oxide_sim::ids::PlayerId(0),
                        BuildingKind::Foundry,
                        TilePos::new(4 + id as i32 * 16, 10),
                    )
                })
                .collect(),
            ..crate::test_support::observation_data()
        });
        let mut cache = None;
        GroundEgressCache::prepare(QueryPurpose::NavigationTest, &mut cache, &obs);
        let prior = cache.as_ref().unwrap().clone();
        assert_eq!(prior.producers.len(), 8);
        for producer in &cache.as_ref().unwrap().producers {
            assert!(std::sync::Arc::ptr_eq(
                &producer.witnesses,
                &prior.producers[0].witnesses
            ));
        }
        obs.known_rock = (0..128).map(|y| TilePos::new(64, y)).collect();
        GroundEgressCache::prepare(QueryPurpose::NavigationTest, &mut cache, &obs);
        let next = cache.unwrap();
        for (index, producer) in next.producers.iter().enumerate() {
            let representative = if index < 4 { 0 } else { 4 };
            assert!(std::sync::Arc::ptr_eq(
                &producer.witnesses,
                &next.producers[representative].witnesses
            ));
            assert!(!std::sync::Arc::ptr_eq(
                &producer.witnesses,
                &prior.producers[0].witnesses
            ));
        }
        assert!(!std::sync::Arc::ptr_eq(
            &next.producers[0].witnesses,
            &next.producers[4].witnesses
        ));
    }

    #[test]
    fn component_extrema_match_full_ranking_after_arbitrary_removals() {
        for mask in 0..512 {
            let tiles: Vec<_> = (0..9)
                .rev()
                .filter(|i| mask & (1 << i) == 0)
                .map(|i| TilePos::new(i % 3, i / 3))
                .collect();
            let witnesses = ComponentWitnesses::new(tiles.clone());
            for anchor in (0..9).map(|i| TilePos::new(i % 3, i / 3)) {
                for removed in 0..10 {
                    let passable = |tile: &TilePos| tile.y * 3 + tile.x != removed;
                    let expected = tiles.iter().copied().filter(passable).min_by_key(|tile| {
                        (std::cmp::Reverse(tile.chebyshev(anchor)), tile.y, tile.x)
                    });
                    assert_eq!(witnesses.farthest(anchor, passable), expected);
                }
            }
        }
    }

    #[test]
    fn large_component_queries_only_inspect_the_four_live_extrema() {
        let witnesses = ComponentWitnesses::new(
            (0..160 * 160)
                .map(|i| TilePos::new(i % 160, i / 160))
                .collect(),
        );
        for blocked_corner in [false, true] {
            let inspected = std::cell::Cell::new(0);
            let chosen = witnesses.farthest(TilePos::new(80, 80), |tile| {
                inspected.set(inspected.get() + 1);
                !blocked_corner || tile.x >= 4 || tile.y >= 4
            });
            assert_eq!(
                chosen,
                Some(if blocked_corner {
                    TilePos::new(4, 0)
                } else {
                    TilePos::new(0, 0)
                })
            );
            assert!(inspected.get() <= 12, "inspected {} cells", inspected.get());
        }
    }

    #[test]
    fn interior_route_repairs_have_local_work_on_large_maps() {
        let map_size = (160, 160);
        let mut open = vec![true; 160 * 160];
        let route: Vec<_> = (0..160).map(|x| TilePos::new(x, 80)).collect();
        let producer = GroundProducerEgress::new(route[0], vec![route[0]], vec![route[159]]);
        let candidate = (BuildingKind::Barricade, TilePos::new(80, 80));
        open[80 * 160 + 80] = false;
        let (repaired, work) = crate::navigation::work::measure(|| {
            GroundEgressCache::repair_route(&open, map_size, &route, candidate, &producer).unwrap()
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
    fn endpoint_repairs_reuse_the_long_route_when_the_next_exit_is_nearby() {
        let map_size = (160, 160);
        let route: Vec<_> = (0..160).map(|x| TilePos::new(x, 80)).collect();
        let producer = GroundProducerEgress::new(
            TilePos::new(0, 0),
            vec![route[0], TilePos::new(0, 81)],
            vec![route[159], TilePos::new(159, 81)],
        );
        for endpoint in [route[0], route[159]] {
            let mut open = vec![true; 160 * 160];
            open[(endpoint.y * 160 + endpoint.x) as usize] = false;
            let (repaired, work) = crate::navigation::work::measure(|| {
                GroundEgressCache::repair_route(
                    &open,
                    map_size,
                    &route,
                    (BuildingKind::Barricade, endpoint),
                    &producer,
                )
                .unwrap()
            });
            let (spawn, witness) = producer.endpoints(&open, map_size).unwrap();
            assert_eq!(repaired.first(), Some(&spawn));
            assert_eq!(repaired.last(), Some(&witness));
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
            assert_eq!(work.searches, 1);
            assert!(work.expanded <= 25, "{work:?}");
        }
    }

    #[test]
    fn witness_selection_matches_the_sorted_reference_after_blocking_candidates() {
        let map_size = (8, 8);
        let witnesses: Vec<_> = (0..64).rev().map(|i| TilePos::new(i % 8, i / 8)).collect();
        for anchor in [TilePos::new(0, 0), TilePos::new(4, 4), TilePos::new(7, 7)] {
            let producer =
                GroundProducerEgress::new(anchor, vec![TilePos::new(3, 3)], witnesses.clone());
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
                let expected_route = crate::navigation::flood::cardinal_path(
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
        let producer = GroundProducerEgress::new(
            TilePos::new(11, 0),
            vec![route[0], TilePos::new(0, 5)],
            vec![route[11], TilePos::new(11, 5)],
        );
        for y in 2..11 {
            open[y * 12 + 6] = y == 6;
        }
        open[6 * 12 + 6] = false;
        assert!(
            GroundEgressCache::repair_route(
                &open,
                map_size,
                &route,
                (BuildingKind::Barricade, route[6]),
                &producer,
            )
            .is_none()
        );
        let detour = GroundEgressCache::ground_producer_route(&open, map_size, &producer).unwrap();
        assert!(detour.iter().any(|tile| tile.y == 1 || tile.y == 11));
        for endpoint in [route[0], route[11]] {
            let mut blocked_endpoint = vec![true; 144];
            blocked_endpoint[(endpoint.y * 12 + endpoint.x) as usize] = false;
            let alternate = GroundEgressCache::repair_route(
                &blocked_endpoint,
                map_size,
                &route,
                (BuildingKind::Barricade, endpoint),
                &producer,
            )
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
                    TilePos::new(11, 5)
                } else {
                    route[11]
                })
            );
        }
    }

    #[test]
    fn repaired_certificates_match_full_connectivity_across_obstacles_and_placements() {
        let map_size = (6, 6);
        let producer = GroundProducerEgress::new(
            TilePos::new(0, 2),
            vec![TilePos::new(0, 2), TilePos::new(0, 3)],
            vec![TilePos::new(5, 2), TilePos::new(5, 3)],
        );
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
                let repair =
                    GroundEgressCache::repair_route(&open, map_size, &route, candidate, &producer);
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
