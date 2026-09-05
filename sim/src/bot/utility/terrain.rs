//! Known-world routing, ferrying, and deterministic placement.

use super::*;
use crate::bot::routing;

type PlannedFootprint = (BuildingKind, TilePos);

/// Membership form of the known-road flood: one BFS from home answers
/// [`UtilityPolicy::ground_route_known`] for every candidate anchor.
/// `None` inside mirrors the degenerate-map and out-of-bounds cases
/// where the per-target flood reports nothing reachable.
pub(super) struct KnownRoadReach {
    component: Option<Vec<bool>>,
    width: i32,
    height: i32,
}

impl KnownRoadReach {
    /// Whether the 2x2 footprint at `anchor` touches home's known-road
    /// component — the exact question the per-anchor flood answered,
    /// including the home-inside-the-footprint case, because the flood
    /// seeds home as seen before consulting the enter predicate.
    pub(super) fn frame_reached(&self, anchor: TilePos) -> bool {
        self.component.as_ref().is_some_and(|seen| {
            (anchor.y..anchor.y + 2).any(|y| {
                (anchor.x..anchor.x + 2).any(|x| {
                    (0..self.width).contains(&x)
                        && (0..self.height).contains(&y)
                        && seen[(y * self.width + x) as usize]
                })
            })
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GroundProducerEgress {
    ring: Vec<TilePos>,
    witnesses: Vec<TilePos>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GroundEgressCertificate {
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
pub(super) struct GroundEgressCache {
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

impl UtilityPolicy {
    /// Whether known ground connects `home` to any tile of the 2x2
    /// footprint anchored at `anchor`. BFS over tiles not known
    /// impassable (rock, mesa, pit — `known_rock` carries all three);
    /// unexplored tiles count open, the same optimism every founding
    /// walk uses. Runs only when a frame claim is otherwise ready, so
    /// the flood's cost is paid a handful of times per match.
    pub(super) fn ground_reaches(obs: &Observation, home: TilePos, anchor: TilePos) -> bool {
        Self::ground_flood(obs, home, anchor, |t| !obs.known_rock_at(t))
    }

    /// Whether a ground road from `home` to `anchor` is actually KNOWN:
    /// the same flood, but unexplored tiles count blocked. This is the
    /// ferry's and the mining arm's route question — a base only ever
    /// seen from the sky is an island war until a walked road proves
    /// otherwise, and the optimistic flood above can wander through any
    /// unexplored gulf forever without ever proving severance.
    pub(super) fn ground_route_known(obs: &Observation, home: TilePos, anchor: TilePos) -> bool {
        Self::ground_flood(obs, home, anchor, |t| {
            obs.explored(t) && !obs.known_rock_at(t)
        })
    }

    /// The shared reachability flood: BFS from `home` through tiles
    /// `enter` admits, looking for the 2x2 footprint at `anchor`.
    fn ground_flood(
        obs: &Observation,
        home: TilePos,
        anchor: TilePos,
        enter: impl Fn(TilePos) -> bool,
    ) -> bool {
        let (w, h) = (obs.map_width, obs.map_height);
        if w <= 0 || h <= 0 {
            return false;
        }
        let idx = |t: TilePos| (t.y * w + t.x) as usize;
        let target = |t: TilePos| {
            (anchor.x..anchor.x + 2).contains(&t.x) && (anchor.y..anchor.y + 2).contains(&t.y)
        };
        let in_bounds = |t: TilePos| t.x >= 0 && t.y >= 0 && t.x < w && t.y < h;
        if !in_bounds(home) {
            return false;
        }
        let mut seen = vec![false; (w * h) as usize];
        let mut open = std::collections::VecDeque::new();
        seen[idx(home)] = true;
        open.push_back(home);
        while let Some(t) = open.pop_front() {
            if target(t) {
                return true;
            }
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let n = t.offset(dx, dy);
                if in_bounds(n) && !seen[idx(n)] && enter(n) {
                    seen[idx(n)] = true;
                    open.push_back(n);
                }
            }
        }
        false
    }

    /// The nearest known enemy building no KNOWN ground road reaches —
    /// the island war's objective — or `None` while every known site
    /// has a walked road. Candidates are tried nearest-first by
    /// (manhattan, y, x). One flood of home's known-road component
    /// answers every candidate: per-site reachability from a fixed
    /// origin is component membership, and the per-site BFS this
    /// replaces re-walked the same component once per known enemy
    /// building on any connected map.
    pub(super) fn island_target(obs: &Observation, home: TilePos) -> Option<TilePos> {
        let mut sites: Vec<(i32, i32, i32)> = obs
            .enemy_buildings
            .iter()
            .map(|b| (b.anchor.manhattan(home), b.anchor.y, b.anchor.x))
            .collect();
        sites.sort_unstable();
        if sites.is_empty() {
            return None;
        }
        let reach = Self::known_road_reach(obs, home);
        sites
            .into_iter()
            .map(|(_, y, x)| TilePos::new(x, y))
            .find(|anchor| !reach.frame_reached(*anchor))
    }

    /// Home's known-road component in membership form, answering
    /// [`Self::ground_route_known`] for any number of anchors with one
    /// flood. Use this wherever candidates are filtered by known ground
    /// reachability from a fixed origin: the per-anchor flood re-walks
    /// the same component once per candidate, which on frame-dense maps
    /// dominates the whole think.
    pub(super) fn known_road_reach(obs: &Observation, home: TilePos) -> KnownRoadReach {
        KnownRoadReach {
            component: Self::ground_component(obs, home, |t| {
                obs.explored(t) && !obs.known_rock_at(t)
            }),
            width: obs.map_width,
            height: obs.map_height,
        }
    }

    /// Home's full walkable component under `enter`, as a seen-tile
    /// grid — the membership form of [`Self::ground_flood`], flooded to
    /// exhaustion. `None` when the map is degenerate or `home` is out
    /// of bounds, where the per-target flood reports nothing reachable.
    fn ground_component(
        obs: &Observation,
        home: TilePos,
        enter: impl Fn(TilePos) -> bool,
    ) -> Option<Vec<bool>> {
        let (w, h) = (obs.map_width, obs.map_height);
        if w <= 0 || h <= 0 || home.x < 0 || home.y < 0 || home.x >= w || home.y >= h {
            return None;
        }
        let idx = |t: TilePos| (t.y * w + t.x) as usize;
        let in_bounds = |t: TilePos| t.x >= 0 && t.y >= 0 && t.x < w && t.y < h;
        let mut seen = vec![false; (w * h) as usize];
        let mut open = std::collections::VecDeque::new();
        seen[idx(home)] = true;
        open.push_back(home);
        while let Some(t) = open.pop_front() {
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let n = t.offset(dx, dy);
                if in_bounds(n) && !seen[idx(n)] && enter(n) {
                    seen[idx(n)] = true;
                    open.push_back(n);
                }
            }
        }
        Some(seen)
    }

    /// The per-unit goals a ground AttackMove would fan out over under the
    /// same known-world passability projection. Mirroring the simulation's
    /// spread matters at barriers: the snapped center can be reachable while
    /// a later unit's assigned tile is across the wall.
    pub(super) fn ground_attack_goals(
        &self,
        obs: &Observation,
        goal: TilePos,
        count: usize,
    ) -> Option<Vec<TilePos>> {
        routing::ground_command_goals(obs, goal, count)
    }

    /// A drop point beside the enemy base, from the target side's own
    /// known ground: the first ring-scanned tile ((r, y, x) order) that
    /// is not known rock, scrap, or a known building footprint —
    /// unexplored tiles count open, like every founding walk. The sim's
    /// unload scan handles exact placement around it; everything nearby
    /// known-blocked falls back to the anchor itself.
    fn unload_site(&self, obs: &Observation, target: TilePos) -> TilePos {
        for r in 2i32..=6 {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dy.abs()) != r {
                        continue;
                    }
                    let t = target.offset(dx, dy);
                    let in_bounds =
                        t.x >= 0 && t.y >= 0 && t.x < obs.map_width && t.y < obs.map_height;
                    if in_bounds && self.tile_open(obs, t) {
                        return t;
                    }
                }
            }
        }
        target
    }

    /// Runs the profile-free Overseer's frozen single-shuttle channel. The
    /// player-facing controller plans persistent multi-carrier waves before
    /// utility work reaches this layer.
    pub(super) fn ferry(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        armies: &[Army],
        home: TilePos,
        claims: FerryClaims<'_>,
        intents: &mut Vec<Intent>,
    ) {
        if !dials.ferry || claims.player_facing {
            return;
        }
        self.ferry_legacy(dials, obs, armies, home, claims.enlisted, intents);
    }

    /// The profile-free Overseer's frozen ferry policy. Its command stream is
    /// a stable QA baseline, so player-facing route and recovery refinements
    /// must not silently change it.
    fn ferry_legacy(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        armies: &[Army],
        home: TilePos,
        enlisted: &[UnitId],
        intents: &mut Vec<Intent>,
    ) {
        if !dials.ferry {
            return;
        }
        let Some(sky) = obs
            .my_units
            .iter()
            .filter(|unit| unit.kind.stats().transport_capacity > 0)
            .min_by_key(|unit| unit.id)
        else {
            self.ferry_boarding.clear();
            return;
        };
        let Some(target) = Self::island_target(obs, home).or_else(|| {
            (self.desperate && !self.desperate_road)
                .then(|| TilePos::new(obs.map_width - 1 - home.x, obs.map_height - 1 - home.y))
        }) else {
            return;
        };
        self.ferry_boarding
            .retain(|id| obs.my_units.iter().any(|unit| unit.id == *id && !unit.idle));
        if sky.cargo > 0 {
            if sky.idle && self.ferry_boarding.is_empty() {
                intents.push(Intent::Unload {
                    transport: sky.id,
                    at: self.unload_site(obs, target),
                });
            }
            return;
        }
        if !sky.idle {
            return;
        }
        let staging: Vec<UnitId> = armies
            .iter()
            .filter(|army| army.state == ArmyState::Staging)
            .flat_map(|army| army.members.iter().copied())
            .collect();
        let pool: Vec<&UnitObs> = obs
            .my_units
            .iter()
            .filter(|unit| {
                let stats = unit.kind.stats();
                stats.domain == Domain::Ground
                    && stats.can_fight()
                    && stats.transport_size > 0
                    && unit.idle
                    && (!enlisted.contains(&unit.id) || staging.contains(&unit.id))
            })
            .collect();
        if pool.len() < FERRY_SQUAD {
            return;
        }
        let mut ranked: Vec<(i32, UnitId, u8)> = pool
            .iter()
            .map(|unit| {
                (
                    unit.tile.chebyshev(sky.tile),
                    unit.id,
                    unit.kind.stats().transport_size,
                )
            })
            .collect();
        ranked.sort_unstable();
        let mut room = sky.kind.stats().transport_capacity;
        let mut riders = Vec::new();
        for (_, id, size) in ranked {
            if size > 0 && size <= room {
                room -= size;
                riders.push(id);
            }
        }
        if riders.is_empty() {
            return;
        }
        self.ferry_boarding = riders.clone();
        intents.push(Intent::Load {
            transport: sky.id,
            riders,
        });
    }
    /// Nearest known scrap by (manhattan, y, x), skipping bounced nodes.
    pub(super) fn nearest_scrap(&self, obs: &Observation, from: TilePos) -> Option<TilePos> {
        obs.known_scrap
            .iter()
            .filter(|(pos, amount)| *amount > 0 && !self.dead_nodes.contains(pos))
            .map(|(pos, _)| (pos.manhattan(from), pos.y, pos.x))
            .min()
            .map(|(_, y, x)| TilePos::new(x, y))
    }

    /// First anchor for `kind` ring-scanned outward from `near` whose
    /// footprint and doorstep ring are clear of everything the
    /// observation knows about — the sim's `can_place` still has the
    /// final word, and refusals land in [`Self::dead_anchors`].
    pub(super) fn placement_near(
        &self,
        obs: &Observation,
        kind: BuildingKind,
        near: TilePos,
    ) -> Option<TilePos> {
        self.placement_near_where(obs, kind, near, |_| true)
    }

    pub(super) fn placement_near_where(
        &self,
        obs: &Observation,
        kind: BuildingKind,
        near: TilePos,
        final_check: impl FnMut(TilePos) -> bool,
    ) -> Option<TilePos> {
        let candidates = (3i32..=7).flat_map(|radius| {
            (-radius..=radius).flat_map(move |dy| {
                (-radius..=radius)
                    .filter(move |dx| dx.abs().max(dy.abs()) == radius)
                    .map(move |dx| near.offset(dx, dy))
            })
        });
        self.first_valid_placement_where(obs, kind, candidates, final_check)
    }

    pub(super) fn first_valid_placement(
        &self,
        obs: &Observation,
        kind: BuildingKind,
        candidates: impl IntoIterator<Item = TilePos>,
    ) -> Option<TilePos> {
        self.first_valid_placement_where(obs, kind, candidates, |_| true)
    }

    pub(super) fn first_valid_placement_where(
        &self,
        obs: &Observation,
        kind: BuildingKind,
        candidates: impl IntoIterator<Item = TilePos>,
        mut final_check: impl FnMut(TilePos) -> bool,
    ) -> Option<TilePos> {
        self.prepare_ground_producer_egress(obs);
        candidates.into_iter().find(|anchor| {
            self.placement_valid_prepared(obs, kind, *anchor) && final_check(*anchor)
        })
    }

    /// One anchor's placement validity against an egress cache the caller
    /// has already prepared for this exact observation via
    /// [`Self::prepare_ground_producer_egress`]. Site scans ask this once
    /// per candidate anchor, and each unprepared ask re-derives the whole
    /// egress layout comparison; the layout cannot change while the
    /// scan's observation is borrowed.
    pub(super) fn placement_valid_prepared(
        &self,
        obs: &Observation,
        kind: BuildingKind,
        anchor: TilePos,
    ) -> bool {
        self.placement_geometry_valid(obs, kind, anchor)
            && self.preserves_ground_producer_egress_prepared(&[], (kind, anchor))
    }

    pub(super) fn placement_geometry_valid(
        &self,
        obs: &Observation,
        kind: BuildingKind,
        anchor: TilePos,
    ) -> bool {
        self.placement_geometry_valid_except(obs, kind, anchor, None)
    }

    pub(super) fn placement_geometry_valid_except(
        &self,
        obs: &Observation,
        kind: BuildingKind,
        anchor: TilePos,
        retained: Option<(BuildingKind, TilePos)>,
    ) -> bool {
        if self.dead_anchors.contains(&anchor)
            || (retained != Some((kind, anchor)) && self.pending_sites.contains(&anchor))
        {
            return false;
        }
        if kind == BuildingKind::Extractor
            && (!obs.known_frames.contains(&anchor)
                || !self.player_can_plan_frame_restoration(obs, anchor))
        {
            return false;
        }
        let (width, height) = kind.base_stats().size;
        let in_bounds = |tile: TilePos| {
            tile.x >= 0 && tile.y >= 0 && tile.x < obs.map_width && tile.y < obs.map_height
        };
        let footprint_ok = (0..width).all(|dx| {
            (0..height).all(|dy| {
                let tile = anchor.offset(dx, dy);
                in_bounds(tile)
                    && obs.explored(tile)
                    && if kind == BuildingKind::Extractor {
                        self.tile_open(obs, tile)
                    } else {
                        self.placement_tile_open_except(obs, tile, retained)
                    }
            })
        });
        if !footprint_ok {
            return false;
        }
        (-1..=width).any(|dx| {
            (-1..=height).any(|dy| {
                let core = (0..width).contains(&dx) && (0..height).contains(&dy);
                let tile = anchor.offset(dx, dy);
                !core && in_bounds(tile) && obs.explored(tile) && self.tile_open(obs, tile)
            })
        })
    }

    /// A placement must not turn a producer's deterministic ground spawn into
    /// an inner pocket. The witness is chosen from the producer's current
    /// movement component, so island bases preserve their island egress rather
    /// than being compared with an unreachable global point.
    #[cfg(test)]
    pub(super) fn preserves_ground_producer_egress(
        &self,
        obs: &Observation,
        accepted: &[PlannedFootprint],
        candidate: PlannedFootprint,
    ) -> bool {
        self.prepare_ground_producer_egress(obs);
        self.preserves_ground_producer_egress_prepared(accepted, candidate)
    }

    pub(super) fn prepare_ground_producer_egress(&self, obs: &Observation) {
        let layout = GroundEgressLayout::from_observation(obs);
        let mut slot = self.ground_egress_cache.borrow_mut();
        let layout_changed = slot.as_ref().is_none_or(|cache| cache.layout != layout);
        if layout_changed {
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

    pub(super) fn preserves_ground_producer_egress_prepared(
        &self,
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

        let mut slot = self.ground_egress_cache.borrow_mut();
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

    fn certificate_routes_blocked(
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
        Self::planned_ground_path(open, map_size, spawn, witness)
    }

    #[cfg(test)]
    fn planned_footprints_block(planned: &[PlannedFootprint], tile: TilePos) -> bool {
        planned
            .iter()
            .any(|(kind, anchor)| Self::candidate_blocks(*kind, *anchor, tile))
    }

    fn ground_egress_base_open(obs: &Observation) -> Vec<bool> {
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
        let labels = Self::ground_egress_components(base_open, map_size);
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

    fn ground_egress_components(open: &[bool], map_size: (i32, i32)) -> Vec<u32> {
        let index = |tile: TilePos| (tile.y * map_size.0 + tile.x) as usize;
        let mut labels = vec![0; open.len()];
        let mut next_label = 1u32;
        for start_index in 0..open.len() {
            if !open[start_index] || labels[start_index] != 0 {
                continue;
            }
            let start = TilePos::new(
                start_index as i32 % map_size.0,
                start_index as i32 / map_size.0,
            );
            labels[start_index] = next_label;
            let mut frontier = std::collections::VecDeque::from([start]);
            while let Some(tile) = frontier.pop_front() {
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let next = tile.offset(dx, dy);
                    if next.x < 0 || next.y < 0 || next.x >= map_size.0 || next.y >= map_size.1 {
                        continue;
                    }
                    let next_index = index(next);
                    if open[next_index] && labels[next_index] == 0 {
                        labels[next_index] = next_label;
                        frontier.push_back(next);
                    }
                }
            }
            next_label = next_label
                .checked_add(1)
                .expect("a ground map cannot contain u32::MAX components");
        }
        labels
    }

    fn planned_ground_path(
        open_tiles: &[bool],
        map_size: (i32, i32),
        start: TilePos,
        goal: TilePos,
    ) -> Option<Vec<TilePos>> {
        let in_bounds = |tile: TilePos| {
            tile.x >= 0 && tile.y >= 0 && tile.x < map_size.0 && tile.y < map_size.1
        };
        if !in_bounds(start) || !in_bounds(goal) {
            return None;
        }
        let index = |tile: TilePos| (tile.y * map_size.0 + tile.x) as usize;
        if !open_tiles[index(start)] || !open_tiles[index(goal)] {
            return None;
        }
        let tile =
            |index: usize| TilePos::new(index as i32 % map_size.0, index as i32 / map_size.0);
        let start_index = index(start);
        let goal_index = index(goal);
        let mut parent = vec![usize::MAX; open_tiles.len()];
        let mut open = std::collections::VecDeque::from([start]);
        parent[start_index] = start_index;
        while let Some(current) = open.pop_front() {
            if current == goal {
                break;
            }
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let next = current.offset(dx, dy);
                if !in_bounds(next) || !open_tiles[index(next)] {
                    continue;
                }
                let next_index = index(next);
                if parent[next_index] != usize::MAX {
                    continue;
                }
                parent[next_index] = index(current);
                open.push_back(next);
            }
        }
        if parent[goal_index] == usize::MAX {
            return None;
        }
        let mut route = Vec::new();
        let mut cursor = goal_index;
        loop {
            route.push(tile(cursor));
            if cursor == start_index {
                break;
            }
            cursor = parent[cursor];
        }
        route.reverse();
        Some(route)
    }

    #[cfg(test)]
    fn planned_ground_open(obs: &Observation, tile: TilePos, planned: &[PlannedFootprint]) -> bool {
        crate::bot::routing::ground_open(obs, tile)
            && !obs.my_units.iter().any(|unit| {
                unit.founding
                    .is_some_and(|(kind, anchor)| Self::candidate_blocks(kind, anchor, tile))
            })
            && !Self::planned_footprints_block(planned, tile)
    }

    fn candidate_blocks(kind: BuildingKind, anchor: TilePos, tile: TilePos) -> bool {
        if kind.is_stealthy() {
            return false;
        }
        let (width, height) = kind.base_stats().size;
        (anchor.x..anchor.x + width).contains(&tile.x)
            && (anchor.y..anchor.y + height).contains(&tile.y)
    }

    fn placement_tile_open_except(
        &self,
        obs: &Observation,
        tile: TilePos,
        retained: Option<(BuildingKind, TilePos)>,
    ) -> bool {
        if !self.tile_open(obs, tile) {
            return false;
        }
        // Nothing may pave over a derelict Extractor frame: the sim
        // refuses the whole footprint as FrameBlocked, and an anchor the
        // scorer keeps proposing anyway feeds the dead-anchor ledger for
        // a refusal the bot could have predicted. (Frames are map data;
        // this check lives here rather than in `tile_open` because that
        // predicate also serves transient movement goals. Durable combat
        // rallies apply their own frame exclusion so a later restoration
        // cannot invalidate an army's standing destination.)
        if obs.known_frames.iter().any(|frame| {
            tile.x >= frame.x && tile.x < frame.x + 2 && tile.y >= frame.y && tile.y < frame.y + 2
        }) {
            return false;
        }
        let claimed = obs.my_units.iter().any(|unit| {
            unit.founding.is_some_and(|(kind, anchor)| {
                if retained == Some((kind, anchor)) {
                    return false;
                }
                let (width, height) = kind.base_stats().size;
                tile.x >= anchor.x
                    && tile.x < anchor.x + width
                    && tile.y >= anchor.y
                    && tile.y < anchor.y + height
            })
        });
        !claimed
            && !obs
                .enemy_units
                .iter()
                .any(|unit| unit.body_domain() == Domain::Ground && unit.tile == tile)
    }

    /// Known-buildable: not rock, not scrap, not under any known
    /// building footprint.
    fn tile_open(&self, obs: &Observation, t: TilePos) -> bool {
        if self.rock_at(obs, t) || obs.known_scrap_at(t) {
            return false;
        }
        let covered = |b: &crate::bot::observation::BuildingObs| {
            let (w, h) = b.kind.base_stats().size;
            t.x >= b.anchor.x && t.x < b.anchor.x + w && t.y >= b.anchor.y && t.y < b.anchor.y + h
        };
        !obs.my_buildings.iter().any(covered)
            && !obs.ally_buildings.iter().any(covered)
            && !obs.enemy_buildings.iter().any(covered)
    }

    fn rock_at(&self, obs: &Observation, t: TilePos) -> bool {
        obs.known_rock_at(t)
    }

    /// The nearest known-open tile to `want`, for rally points that should not
    /// sit inside a rock formation. Check the common local case first, then
    /// preserve the same `(radius, y, x)` order across the complete map.
    pub(super) fn passable_near(&self, obs: &Observation, want: TilePos) -> TilePos {
        for r in 0i32..=3 {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dy.abs()) != r {
                        continue;
                    }
                    let t = want.offset(dx, dy);
                    if t.x >= 0
                        && t.y >= 0
                        && t.x < obs.map_width
                        && t.y < obs.map_height
                        && self.tile_open(obs, t)
                    {
                        return t;
                    }
                }
            }
        }
        (0..obs.map_height)
            .flat_map(|y| (0..obs.map_width).map(move |x| TilePos::new(x, y)))
            .filter(|tile| self.tile_open(obs, *tile))
            .min_by_key(|tile| (tile.chebyshev(want), tile.y, tile.x))
            .unwrap_or(want)
    }

    /// The nearest known-open tile that cannot later be claimed by an
    /// Extractor restoration. This is intentionally narrower than
    /// [`Self::passable_near`]: ordinary movement may cross or stop briefly on
    /// a bare frame, while an army rally persists across construction plans.
    pub(super) fn durable_rally_near(&self, obs: &Observation, want: TilePos) -> TilePos {
        let frame_covers = |tile: TilePos| {
            obs.known_frames.iter().any(|frame| {
                tile.x >= frame.x
                    && tile.x < frame.x + 2
                    && tile.y >= frame.y
                    && tile.y < frame.y + 2
            })
        };
        let max_radius = obs.map_width.max(obs.map_height).max(0);
        for radius in 0..=max_radius {
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    if dx.abs().max(dy.abs()) != radius {
                        continue;
                    }
                    let tile = want.offset(dx, dy);
                    if tile.x >= 0
                        && tile.y >= 0
                        && tile.x < obs.map_width
                        && tile.y < obs.map_height
                        && self.tile_open(obs, tile)
                        && !frame_covers(tile)
                    {
                        return tile;
                    }
                }
            }
        }
        want
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::observation::{BuildingObs, UnitObs};
    use crate::ids::{BuildingId, PlayerId, UnitId};

    fn observation() -> Observation {
        let width = 18;
        let height = 14;
        let mut obs = Observation {
            tick: 0,
            map_width: width,
            map_height: height,
            visible: vec![true; (width * height) as usize],
            explored: vec![true; (width * height) as usize],
            ..Observation::default()
        };
        add_building(&mut obs, BuildingKind::Foundry, TilePos::new(7, 5));
        obs
    }

    fn add_building(obs: &mut Observation, kind: BuildingKind, anchor: TilePos) {
        let id = BuildingId(obs.my_buildings.len() as u32);
        obs.my_buildings.push(BuildingObs {
            id,
            player: PlayerId(0),
            kind,
            anchor,
            hp: kind.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        });
        obs.my_queues.push(Vec::new());
    }

    fn placement_valid(
        policy: &UtilityPolicy,
        obs: &Observation,
        kind: BuildingKind,
        anchor: TilePos,
    ) -> bool {
        policy.first_valid_placement(obs, kind, [anchor]) == Some(anchor)
    }

    #[test]
    fn a_parked_hostile_airframe_closes_a_placement_tile_until_it_lifts_off() {
        let mut obs = observation();
        let policy = UtilityPolicy::new();
        let tile = (0..obs.map_height)
            .flat_map(|y| (0..obs.map_width).map(move |x| TilePos::new(x, y)))
            .find(|t| policy.placement_tile_open_except(&obs, *t, None))
            .expect("the fixture has open ground");
        obs.enemy_units.push(UnitObs {
            id: UnitId(90),
            player: PlayerId(1),
            kind: UnitKind::Condor,
            tile,
            hp: UnitKind::Condor.stats().max_hp,
            idle: true,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: true,
        });
        assert!(
            !policy.placement_tile_open_except(&obs, tile, None),
            "a parked airframe is a ground body the sim would refuse a footprint over"
        );
        obs.enemy_units.last_mut().unwrap().grounded = false;
        assert!(
            policy.placement_tile_open_except(&obs, tile, None),
            "the same airframe in the air leaves the tile open"
        );
    }

    #[test]
    fn dense_placement_may_replace_the_first_spawn_when_an_egress_remains() {
        let obs = observation();
        let policy = UtilityPolicy::new();
        let foundry = &obs.my_buildings[0];
        let first_spawn =
            crate::tick::rect_adjacent_tiles(foundry.anchor, foundry.kind.base_stats().size)
                .find(|tile| UtilityPolicy::planned_ground_open(&obs, *tile, &[]))
                .expect("the open Foundry has a spawn tile");

        assert!(placement_valid(
            &policy,
            &obs,
            BuildingKind::Reclaimer,
            first_spawn
        ));
    }

    #[test]
    fn active_scuttle_charge_foundation_is_nonblocking_in_cold_and_cached_egress() {
        let obs = observation();
        let foundry = &obs.my_buildings[0];
        let mine_anchor =
            crate::tick::rect_adjacent_tiles(foundry.anchor, foundry.kind.base_stats().size)
                .find(|tile| UtilityPolicy::planned_ground_open(&obs, *tile, &[]))
                .expect("the open Foundry has a canonical spawn");
        let mut with_mine = obs.clone();
        with_mine.my_units.push(UnitObs {
            id: UnitId(100),
            player: PlayerId(0),
            kind: UnitKind::Harvester,
            tile: TilePos::new(2, 2),
            hp: UnitKind::Harvester.stats().max_hp,
            idle: false,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: Some((BuildingKind::ScuttleCharge, mine_anchor)),
            repairing: false,
            grounded: false,
        });
        assert_eq!(
            GroundEgressLayout::from_observation(&with_mine),
            GroundEgressLayout::from_observation(&obs),
            "a nonblocking foundation must not invalidate the egress cache"
        );

        let cold = UtilityPolicy::new();
        cold.prepare_ground_producer_egress(&with_mine);
        {
            let cache = cold.ground_egress_cache.borrow();
            let cache = cache.as_ref().expect("cold egress is prepared");
            let index = (mine_anchor.y * with_mine.map_width + mine_anchor.x) as usize;
            assert!(cache.base_open[index]);
            let certificate = cache
                .decisions
                .get(&Vec::new())
                .and_then(Clone::clone)
                .expect("the founding mine leaves a cold route certificate");
            assert_eq!(certificate.routes[0].first(), Some(&mine_anchor));
        }

        let cached = UtilityPolicy::new();
        cached.prepare_ground_producer_egress(&obs);
        let before = cached
            .ground_egress_cache
            .borrow()
            .as_ref()
            .expect("baseline egress is prepared")
            .decisions
            .get(&Vec::new())
            .and_then(Clone::clone)
            .expect("the baseline has a route certificate");
        cached.prepare_ground_producer_egress(&with_mine);
        let after = cached
            .ground_egress_cache
            .borrow()
            .as_ref()
            .expect("cached egress remains prepared")
            .decisions
            .get(&Vec::new())
            .and_then(Clone::clone)
            .expect("the cached certificate remains valid");
        assert!(std::sync::Arc::ptr_eq(&before, &after));
    }

    #[test]
    fn an_already_sealed_producer_does_not_paralyze_other_placement() {
        let mut obs = observation();
        let sealed = obs.my_buildings[0].clone();
        add_building(&mut obs, BuildingKind::Fabricator, TilePos::new(13, 8));
        let usable = obs.my_buildings[1].clone();

        for anchor in crate::tick::rect_adjacent_tiles(sealed.anchor, sealed.kind.base_stats().size)
        {
            add_building(&mut obs, BuildingKind::Barricade, anchor);
        }
        let usable_ring: Vec<_> =
            crate::tick::rect_adjacent_tiles(usable.anchor, usable.kind.base_stats().size)
                .collect();
        let (&last_spawn, occupied_ring) = usable_ring
            .split_last()
            .expect("the usable producer has a doorstep ring");
        for &anchor in occupied_ring {
            add_building(&mut obs, BuildingKind::Barricade, anchor);
        }

        let policy = UtilityPolicy::new();
        policy.prepare_ground_producer_egress(&obs);
        {
            let cache = policy.ground_egress_cache.borrow();
            let cache = cache.as_ref().expect("egress is prepared");
            assert_eq!(
                cache.producers.len(),
                1,
                "the irrecoverably sealed producer is omitted"
            );
            let certificate = cache
                .decisions
                .get(&Vec::new())
                .and_then(Clone::clone)
                .expect("the usable producer still has a certificate");
            assert_eq!(certificate.routes.len(), 1);
            assert_eq!(certificate.routes[0].first(), Some(&last_spawn));
        }

        assert!(placement_valid(
            &policy,
            &obs,
            BuildingKind::Reclaimer,
            TilePos::new(1, 1),
        ));
        assert!(
            !policy.preserves_ground_producer_egress(
                &obs,
                &[],
                (BuildingKind::Barricade, last_spawn),
            ),
            "unrelated construction remains possible without sacrificing the usable producer"
        );
    }

    #[test]
    fn final_placement_check_only_runs_after_cheap_validity_checks() {
        let obs = observation();
        let policy = UtilityPolicy::new();
        let blocked = obs.my_buildings[0].anchor;
        let valid = TilePos::new(1, 1);
        assert_eq!(
            policy.first_valid_placement(&obs, BuildingKind::Foundry, [valid]),
            Some(valid),
            "the route-check candidate must be buildable"
        );
        let calls = std::cell::Cell::new(0);

        let selected = policy.first_valid_placement_where(
            &obs,
            BuildingKind::Foundry,
            [blocked, valid],
            |_| {
                calls.set(calls.get() + 1);
                true
            },
        );

        assert_eq!(selected, Some(valid));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn generic_placement_skips_every_tile_of_an_extractor_frame() {
        let mut obs = observation();
        let frame = TilePos::new(2, 2);
        let safe = TilePos::new(4, 2);
        obs.known_frames.push(frame);
        let policy = UtilityPolicy::new();

        for overlap in [
            frame,
            frame.offset(1, 0),
            frame.offset(0, 1),
            frame.offset(1, 1),
        ] {
            assert_eq!(
                policy.first_valid_placement(&obs, BuildingKind::Reclaimer, [overlap, safe],),
                Some(safe),
                "a generic building candidate on frame tile {overlap:?} must be skipped"
            );
        }
    }

    #[test]
    fn passable_near_searches_past_the_local_radius_before_returning_a_blocked_goal() {
        let mut obs = observation();
        let want = TilePos::new(5, 7);
        obs.known_rock = (0..obs.map_height)
            .flat_map(|y| (0..obs.map_width).map(move |x| TilePos::new(x, y)))
            .filter(|tile| tile.chebyshev(want) <= 3)
            .collect();
        let expected = want.offset(-4, -4);

        let selected = UtilityPolicy::new().passable_near(&obs, want);

        assert_eq!(selected, expected, "the first open tile is on radius four");
        assert!(!obs.known_rock_at(selected));
    }

    #[test]
    fn legacy_ferry_waits_for_partial_boarding_before_unloading_once() {
        let mut obs = observation();
        obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(10, y)).collect();
        obs.enemy_buildings.push(BuildingObs {
            id: BuildingId(20),
            player: PlayerId(1),
            kind: BuildingKind::Foundry,
            anchor: TilePos::new(14, 5),
            hp: BuildingKind::Foundry.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        });
        let unit = |id, kind, tile| UnitObs {
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
        };
        obs.my_units = vec![
            unit(1, UnitKind::Sentinel, TilePos::new(3, 3)),
            unit(2, UnitKind::Sentinel, TilePos::new(4, 3)),
            unit(3, UnitKind::Sentinel, TilePos::new(5, 3)),
            unit(10, UnitKind::Skyhook, TilePos::new(4, 4)),
        ];
        let dials = Dials::overseer();
        let mut policy = UtilityPolicy::new();
        let mut intents = Vec::new();
        policy.ferry(
            &dials,
            &obs,
            &[],
            TilePos::new(7, 5),
            FerryClaims {
                enlisted: &[],
                player_facing: false,
            },
            &mut intents,
        );
        assert_eq!(
            intents,
            vec![Intent::Load {
                transport: UnitId(10),
                riders: vec![UnitId(1), UnitId(2), UnitId(3)],
            }]
        );

        let one_rider = UnitKind::Sentinel.stats().transport_size;
        obs.my_units.retain(|unit| unit.id != UnitId(1));
        for rider in obs
            .my_units
            .iter_mut()
            .filter(|unit| matches!(unit.id, UnitId(2) | UnitId(3)))
        {
            rider.idle = false;
        }
        let skyhook = obs
            .my_units
            .iter_mut()
            .find(|unit| unit.id == UnitId(10))
            .expect("the shuttle remains visible");
        skyhook.cargo = one_rider;
        skyhook.idle = true;
        intents.clear();
        policy.ferry(
            &dials,
            &obs,
            &[],
            TilePos::new(7, 5),
            FerryClaims {
                enlisted: &[],
                player_facing: false,
            },
            &mut intents,
        );
        assert!(
            intents.is_empty(),
            "an idle shuttle must not unload while commanded riders are still boarding"
        );

        obs.my_units.retain(|unit| unit.id == UnitId(10));
        let skyhook = obs
            .my_units
            .first_mut()
            .expect("the loaded shuttle remains visible");
        skyhook.cargo = one_rider * 3;
        skyhook.idle = true;
        policy.ferry(
            &dials,
            &obs,
            &[],
            TilePos::new(7, 5),
            FerryClaims {
                enlisted: &[],
                player_facing: false,
            },
            &mut intents,
        );
        assert!(matches!(
            intents.as_slice(),
            [Intent::Unload {
                transport: UnitId(10),
                ..
            }]
        ));
    }

    #[test]
    fn nonintersecting_placement_reuses_the_cached_route_certificate() {
        let obs = observation();
        let policy = UtilityPolicy::new();
        policy.prepare_ground_producer_egress(&obs);
        let (baseline, candidate) = {
            let cache = policy.ground_egress_cache.borrow();
            let cache = cache.as_ref().expect("egress cache is prepared");
            let baseline = cache
                .decisions
                .get(&Vec::new())
                .and_then(Clone::clone)
                .expect("the open fixture has a route certificate");
            let candidate = (0..obs.map_height)
                .flat_map(|y| (0..obs.map_width).map(move |x| TilePos::new(x, y)))
                .map(|anchor| (BuildingKind::Reclaimer, anchor))
                .find(|candidate| {
                    UtilityPolicy::certificate_routes_blocked(
                        &baseline,
                        *candidate,
                        cache.layout.map_size,
                    )
                    .is_empty()
                })
                .expect("some tile lies outside the canonical route");
            (baseline, candidate)
        };

        assert!(policy.preserves_ground_producer_egress_prepared(&[], candidate));

        let cache = policy.ground_egress_cache.borrow();
        let certificate = cache
            .as_ref()
            .expect("egress cache remains prepared")
            .decisions
            .get(&vec![candidate])
            .and_then(Clone::clone)
            .expect("the accepted candidate has a certificate");
        assert!(std::sync::Arc::ptr_eq(&baseline, &certificate));
    }

    #[test]
    fn intersecting_placement_caches_an_exact_alternate_route() {
        let obs = observation();
        let policy = UtilityPolicy::new();
        policy.prepare_ground_producer_egress(&obs);
        let (baseline, candidate) = {
            let cache = policy.ground_egress_cache.borrow();
            let cache = cache.as_ref().expect("egress cache is prepared");
            let baseline = cache
                .decisions
                .get(&Vec::new())
                .and_then(Clone::clone)
                .expect("the open fixture has a route certificate");
            let spawn = baseline.routes[0][0];
            let candidate = (BuildingKind::Reclaimer, spawn);
            assert_eq!(
                UtilityPolicy::certificate_routes_blocked(
                    &baseline,
                    candidate,
                    cache.layout.map_size,
                ),
                vec![0]
            );
            (baseline, candidate)
        };

        assert!(policy.preserves_ground_producer_egress_prepared(&[], candidate));

        let cache = policy.ground_egress_cache.borrow();
        let certificate = cache
            .as_ref()
            .expect("egress cache remains prepared")
            .decisions
            .get(&vec![candidate])
            .and_then(Clone::clone)
            .expect("an alternate route exists");
        assert!(!std::sync::Arc::ptr_eq(&baseline, &certificate));
        assert!(
            certificate.routes[0]
                .iter()
                .all(|tile| !UtilityPolicy::candidate_blocks(candidate.0, candidate.1, *tile))
        );
    }

    #[test]
    fn accepted_footprints_are_certified_cold_and_a_cached_seal_stays_rejected() {
        let obs = observation();
        let policy = UtilityPolicy::new();
        let producer = &obs.my_buildings[0];
        assert_eq!(BuildingKind::Barricade.base_stats().size, (1, 1));
        assert!(!BuildingKind::Barricade.is_stealthy());

        let ring: Vec<_> =
            crate::tick::rect_adjacent_tiles(producer.anchor, producer.kind.base_stats().size)
                .collect();
        let (&closing_anchor, open_ring) = ring
            .split_last()
            .expect("a ground producer has a doorstep ring");
        let accepted: Vec<_> = open_ring
            .iter()
            .copied()
            .map(|anchor| (BuildingKind::Barricade, anchor))
            .collect();
        let closing = (BuildingKind::Barricade, closing_anchor);

        assert!(
            !policy.preserves_ground_producer_egress(&obs, &accepted, closing),
            "the first query must certify the accepted partial ring, then reject its closing footprint"
        );
        assert!(
            policy.preserves_ground_producer_egress(&obs, &accepted, accepted[0]),
            "the accepted footprints themselves must retain the one remaining doorstep"
        );
        let cached_decisions = policy
            .ground_egress_cache
            .borrow()
            .as_ref()
            .expect("the first query prepares the egress cache")
            .decisions
            .len();

        assert!(
            !policy.preserves_ground_producer_egress(&obs, &accepted, closing),
            "the cached answer must not admit the same producer seal on a later query"
        );
        assert_eq!(
            policy
                .ground_egress_cache
                .borrow()
                .as_ref()
                .expect("the cache remains prepared")
                .decisions
                .len(),
            cached_decisions,
            "the repeated decision must reuse both the accepted certificate and rejected full plan"
        );
    }

    #[test]
    fn dense_base_grid_matches_the_routing_projection() {
        let mut obs = observation();
        obs.known_rock.push(TilePos::new(1, 1));
        obs.known_scrap.push((TilePos::new(2, 2), 10));
        obs.enemy_buildings.push(BuildingObs {
            id: BuildingId(100),
            player: PlayerId(1),
            kind: BuildingKind::Turret,
            anchor: TilePos::new(3, 3),
            hp: BuildingKind::Turret.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        });
        obs.my_units.push(UnitObs {
            id: UnitId(100),
            player: PlayerId(0),
            kind: UnitKind::Harvester,
            tile: TilePos::new(1, 2),
            hp: UnitKind::Harvester.stats().max_hp,
            idle: false,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: Some((BuildingKind::RepairBay, TilePos::new(12, 2))),
            repairing: false,
            grounded: false,
        });
        let open = UtilityPolicy::ground_egress_base_open(&obs);

        for y in 0..obs.map_height {
            for x in 0..obs.map_width {
                let tile = TilePos::new(x, y);
                assert_eq!(
                    open[(y * obs.map_width + x) as usize],
                    UtilityPolicy::planned_ground_open(&obs, tile, &[]),
                    "dense occupancy disagrees at {tile:?}"
                );
            }
        }
    }

    #[test]
    fn successive_dense_placements_refuse_the_one_that_seals_a_producer() {
        let mut obs = observation();
        let policy = UtilityPolicy::new();
        let producer = obs.my_buildings[0].clone();
        let ring: Vec<_> =
            crate::tick::rect_adjacent_tiles(producer.anchor, producer.kind.base_stats().size)
                .collect();

        for &anchor in &ring[..ring.len() - 1] {
            assert!(
                placement_valid(&policy, &obs, BuildingKind::Reclaimer, anchor),
                "the still-open ring accepts the dense placement at {anchor:?}"
            );
            add_building(&mut obs, BuildingKind::Reclaimer, anchor);
        }

        let last = *ring.last().expect("a producer has a doorstep ring");
        assert!(
            policy.placement_geometry_valid(&obs, BuildingKind::Reclaimer, last),
            "the final footprint remains physically buildable with an outside doorstep"
        );
        assert!(
            !policy.preserves_ground_producer_egress(&obs, &[], (BuildingKind::Reclaimer, last),),
            "egress, rather than ordinary placement geometry, must reject the seal"
        );
        assert!(
            !placement_valid(&policy, &obs, BuildingKind::Reclaimer, last),
            "the final closing footprint must preserve the producer's outside component"
        );
    }

    #[test]
    fn every_ground_producer_keeps_egress_not_only_the_home_foundry() {
        let mut obs = observation();
        let policy = UtilityPolicy::new();
        let fabricator_anchor = TilePos::new(13, 8);
        add_building(&mut obs, BuildingKind::Fabricator, fabricator_anchor);
        let fabricator = obs.my_buildings[1].clone();
        assert!(
            fabricator
                .kind
                .base_stats()
                .produces
                .iter()
                .any(|kind| kind.stats().domain == Domain::Ground),
            "the fixture's secondary building must produce ground units"
        );
        let ring: Vec<_> =
            crate::tick::rect_adjacent_tiles(fabricator.anchor, fabricator.kind.base_stats().size)
                .collect();
        for &anchor in &ring[..ring.len() - 1] {
            add_building(&mut obs, BuildingKind::Reclaimer, anchor);
        }

        let last = *ring.last().expect("a producer has a doorstep ring");
        assert!(!placement_valid(
            &policy,
            &obs,
            BuildingKind::Reclaimer,
            last
        ));
    }

    #[test]
    fn cold_and_warm_egress_cache_make_the_same_decision() {
        let mut obs = observation();
        let producer = obs.my_buildings[0].clone();
        let ring: Vec<_> =
            crate::tick::rect_adjacent_tiles(producer.anchor, producer.kind.base_stats().size)
                .collect();
        for &anchor in &ring[..ring.len() - 1] {
            add_building(&mut obs, BuildingKind::Reclaimer, anchor);
        }
        let seal = *ring.last().expect("a producer has a doorstep ring");

        let cold = UtilityPolicy::new().preserves_ground_producer_egress(
            &obs,
            &[],
            (BuildingKind::Reclaimer, seal),
        );
        let warm_policy = UtilityPolicy::new();
        assert!(warm_policy.preserves_ground_producer_egress(
            &obs,
            &[],
            (BuildingKind::Reclaimer, TilePos::new(1, 1)),
        ));
        let warm = warm_policy.preserves_ground_producer_egress(
            &obs,
            &[],
            (BuildingKind::Reclaimer, seal),
        );
        let cached = warm_policy.preserves_ground_producer_egress(
            &obs,
            &[],
            (BuildingKind::Reclaimer, seal),
        );

        assert_eq!(cold, warm);
        assert_eq!(warm, cached);
        assert!(!cached);
    }

    #[test]
    fn egress_cache_layout_tracks_every_routing_input() {
        let obs = observation();
        let baseline = GroundEgressLayout::from_observation(&obs);

        let mut with_rock = obs.clone();
        with_rock.known_rock.push(TilePos::new(1, 1));
        assert_ne!(baseline, GroundEgressLayout::from_observation(&with_rock));

        let mut with_scrap = obs.clone();
        with_scrap.known_scrap.push((TilePos::new(1, 1), 10));
        assert_ne!(baseline, GroundEgressLayout::from_observation(&with_scrap));

        let mut with_building = obs.clone();
        with_building.enemy_buildings.push(BuildingObs {
            id: BuildingId(100),
            player: PlayerId(1),
            kind: BuildingKind::Turret,
            anchor: TilePos::new(1, 1),
            hp: BuildingKind::Turret.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        });
        assert_ne!(
            baseline,
            GroundEgressLayout::from_observation(&with_building)
        );

        let mut with_founding = obs.clone();
        with_founding.my_units.push(UnitObs {
            id: UnitId(100),
            player: PlayerId(0),
            kind: UnitKind::Harvester,
            tile: TilePos::new(1, 1),
            hp: UnitKind::Harvester.stats().max_hp,
            idle: false,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: Some((BuildingKind::Turret, TilePos::new(2, 2))),
            repairing: false,
            grounded: false,
        });
        assert_ne!(
            baseline,
            GroundEgressLayout::from_observation(&with_founding)
        );

        let mut without_producer = obs;
        without_producer.my_buildings[0].built = false;
        assert_ne!(
            baseline,
            GroundEgressLayout::from_observation(&without_producer)
        );
    }
}
