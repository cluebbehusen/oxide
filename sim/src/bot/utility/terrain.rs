//! Known-world routing, ferrying, and deterministic placement.

use super::*;
use crate::bot::navigation::egress::GroundEgressCache;
use crate::bot::query_work::QueryPurpose;

type PlannedFootprint = (BuildingKind, TilePos);

pub(super) struct PlacementGeometry<'a> {
    obs: &'a Observation,
    open: Vec<bool>,
    unclaimed: Vec<bool>,
}

impl<'a> PlacementGeometry<'a> {
    pub(super) fn new(obs: &'a Observation) -> Self {
        Self::after_cancellations(obs, FoundationCancellations::default())
    }

    pub(super) fn after_cancellations(
        obs: &'a Observation,
        cancellations: FoundationCancellations<'_>,
    ) -> Self {
        let cells = (obs.map_width.max(0) as usize) * (obs.map_height.max(0) as usize);
        let mut open = vec![true; cells];
        let index = |tile: TilePos| {
            (tile.x >= 0 && tile.y >= 0 && tile.x < obs.map_width && tile.y < obs.map_height)
                .then(|| (tile.y * obs.map_width + tile.x) as usize)
        };
        for tile in obs
            .known_rock
            .iter()
            .copied()
            .chain(obs.known_scrap.iter().map(|(tile, _)| *tile))
        {
            if let Some(index) = index(tile) {
                open[index] = false;
            }
        }
        let block = |grid: &mut Vec<bool>, anchor: TilePos, size: (i32, i32)| {
            for dy in 0..size.1 {
                for dx in 0..size.0 {
                    if let Some(index) = index(anchor.offset(dx, dy)) {
                        grid[index] = false;
                    }
                }
            }
        };
        for building in obs
            .my_buildings
            .iter()
            .chain(&obs.ally_buildings)
            .chain(&obs.enemy_buildings)
        {
            block(&mut open, building.anchor, building.kind.base_stats().size);
        }
        let mut unclaimed = open.clone();
        for anchor in &obs.known_frames {
            block(&mut unclaimed, *anchor, (2, 2));
        }
        for (kind, anchor) in obs
            .my_units
            .iter()
            .filter_map(|unit| cancellations.retained(unit))
        {
            block(&mut unclaimed, anchor, kind.base_stats().size);
        }
        for unit in obs
            .enemy_units
            .iter()
            .filter(|unit| unit.body_domain() == Domain::Ground)
        {
            block(&mut unclaimed, unit.tile, (1, 1));
        }
        Self {
            obs,
            open,
            unclaimed,
        }
    }

    pub(super) fn valid(
        &self,
        policy: &UtilityPolicy,
        kind: BuildingKind,
        anchor: TilePos,
    ) -> bool {
        #[cfg(test)]
        crate::bot::navigation::work::record(|work| work.placement_checks += 1);
        let index = |tile: TilePos| (tile.y * self.obs.map_width + tile.x) as usize;
        policy.placement_geometry_valid_with(
            self.obs,
            kind,
            anchor,
            None,
            |tile| self.open[index(tile)],
            |tile| self.unclaimed[index(tile)],
        )
    }
}

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
        crate::bot::navigation::flood::reaches_any(
            obs.map_width,
            obs.map_height,
            [home],
            enter,
            |tile| {
                (anchor.x..anchor.x + 2).contains(&tile.x)
                    && (anchor.y..anchor.y + 2).contains(&tile.y)
            },
        )
    }

    /// Home's known-road component in membership form, answering
    /// [`Self::ground_route_known`] for any number of anchors with one
    /// flood. Use this wherever candidates are filtered by known ground
    /// reachability from a fixed origin: the per-anchor flood re-walks
    /// the same component once per candidate, which on frame-dense maps
    /// dominates the whole think.
    pub(super) fn known_road_reach(obs: &Observation, home: TilePos) -> KnownRoadReach {
        let cells = (obs.map_width.max(0) as usize) * (obs.map_height.max(0) as usize);
        let mut open = obs.explored.clone();
        open.resize(cells, false);
        for &tile in &obs.known_rock {
            if let Some(index) =
                crate::bot::navigation::flood::tile_index(obs.map_width, obs.map_height, tile)
            {
                open[index] = false;
            }
        }
        KnownRoadReach {
            component: Self::ground_component(obs, home, |t| {
                open[(t.y * obs.map_width + t.x) as usize]
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
        crate::bot::navigation::flood::component(obs.map_width, obs.map_height, home, enter)
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
        routing::ground_command_goals(QueryPurpose::ConstructionAccess, obs, goal, count)
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
        let geometry = PlacementGeometry::new(obs);
        candidates.into_iter().find(|anchor| {
            geometry.valid(self, kind, *anchor)
                && self.preserves_ground_producer_egress_prepared(&[], (kind, *anchor))
                && final_check(*anchor)
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
        cancellations: FoundationCancellations<'_>,
    ) -> bool {
        self.placement_geometry_valid_except(obs, kind, anchor, None, cancellations)
            && self.preserves_ground_producer_egress_prepared(&[], (kind, anchor))
    }

    pub(super) fn placement_geometry_valid_except(
        &self,
        obs: &Observation,
        kind: BuildingKind,
        anchor: TilePos,
        retained: Option<(BuildingKind, TilePos)>,
        cancellations: FoundationCancellations<'_>,
    ) -> bool {
        self.placement_geometry_valid_with(
            obs,
            kind,
            anchor,
            retained,
            |tile| self.tile_open(obs, tile),
            |tile| self.placement_tile_open_except(obs, tile, retained, cancellations),
        )
    }

    fn placement_geometry_valid_with(
        &self,
        obs: &Observation,
        kind: BuildingKind,
        anchor: TilePos,
        retained: Option<(BuildingKind, TilePos)>,
        open: impl Fn(TilePos) -> bool,
        unclaimed: impl Fn(TilePos) -> bool,
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
                    && (kind.is_stealthy()
                        || !self.work_experience.construction_work_tiles.contains(&tile))
                    && if kind == BuildingKind::Extractor {
                        open(tile)
                    } else {
                        unclaimed(tile)
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
                !core && in_bounds(tile) && obs.explored(tile) && open(tile)
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
        GroundEgressCache::prepare(
            QueryPurpose::ConstructionAccess,
            &mut self.ground_egress_cache.borrow_mut(),
            obs,
        );
    }
    pub(super) fn prepare_ground_producer_egress_after(
        &self,
        obs: &Observation,
        cancellations: FoundationCancellations<'_>,
    ) {
        GroundEgressCache::prepare_after(
            QueryPurpose::ConstructionAccess,
            &mut self.ground_egress_cache.borrow_mut(),
            obs,
            cancellations,
        );
    }
    pub(super) fn preserves_ground_producer_egress_prepared(
        &self,
        accepted: &[PlannedFootprint],
        candidate: PlannedFootprint,
    ) -> bool {
        GroundEgressCache::preserves(
            &mut self.ground_egress_cache.borrow_mut(),
            accepted,
            candidate,
        )
    }

    #[cfg(test)]
    fn planned_ground_open(obs: &Observation, tile: TilePos, planned: &[PlannedFootprint]) -> bool {
        crate::bot::navigation::commands::ground_open(QueryPurpose::NavigationTest, obs, tile)
            && !obs.my_units.iter().any(|unit| {
                unit.founding.is_some_and(|(kind, anchor)| {
                    GroundEgressCache::candidate_blocks(kind, anchor, tile)
                })
            })
            && !GroundEgressCache::planned_footprints_block(planned, tile)
    }

    fn placement_tile_open_except(
        &self,
        obs: &Observation,
        tile: TilePos,
        retained: Option<(BuildingKind, TilePos)>,
        cancellations: FoundationCancellations<'_>,
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
            cancellations.retained(unit).is_some_and(|(kind, anchor)| {
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
            provisional: false,
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
    fn indexed_placement_matches_scalar_geometry_across_known_obstacles() {
        let mut obs = observation();
        obs.known_rock = vec![TilePos::new(3, 3), TilePos::new(3, 4)];
        obs.known_scrap = vec![(TilePos::new(4, 3), 10)];
        obs.known_frames = vec![TilePos::new(12, 8)];
        obs.explored[0] = false;
        add_building(&mut obs, BuildingKind::ScuttleCharge, TilePos::new(5, 5));
        obs.enemy_units.push(UnitObs {
            id: UnitId(90),
            player: PlayerId(1),
            kind: UnitKind::Sentinel,
            tile: TilePos::new(10, 10),
            hp: 60,
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
        let mut founder = obs.enemy_units[0].clone();
        founder.id = UnitId(91);
        founder.player = obs.me;
        founder.kind = UnitKind::Harvester;
        founder.founding = Some((BuildingKind::Fabricator, TilePos::new(2, 8)));
        obs.my_units.push(founder);
        let geometry = PlacementGeometry::new(&obs);
        let mut policy = UtilityPolicy::new();
        policy.pending_sites.push(TilePos::new(8, 10));
        policy.dead_anchors.push(TilePos::new(10, 2));
        policy
            .work_experience
            .construction_work_tiles
            .insert(TilePos::new(4, 9));
        for kind in BuildingKind::ALL {
            for y in -1..=obs.map_height {
                for x in -1..=obs.map_width {
                    let anchor = TilePos::new(x, y);
                    assert_eq!(
                        geometry.valid(&policy, kind, anchor),
                        policy.placement_geometry_valid_except(
                            &obs,
                            kind,
                            anchor,
                            None,
                            FoundationCancellations::default()
                        ),
                        "{kind:?} at {anchor:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn construction_placement_respects_a_walking_founders_promised_footprint() {
        let mut obs = observation();
        let anchor = TilePos::new(2, 2);
        let policy = UtilityPolicy::new();
        assert!(placement_valid(
            &policy,
            &obs,
            BuildingKind::Fabricator,
            anchor
        ));
        obs.my_units.push(UnitObs {
            id: UnitId(70),
            player: obs.me,
            kind: UnitKind::Harvester,
            tile: TilePos::new(12, 10),
            hp: 60,
            idle: false,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: Some((BuildingKind::Fabricator, anchor)),
            repairing: false,
            grounded: false,
        });
        assert!(!placement_valid(
            &policy,
            &obs,
            BuildingKind::Fabricator,
            anchor
        ));
        assert!(placement_valid(
            &policy,
            &obs,
            BuildingKind::Fabricator,
            TilePos::new(2, 9)
        ));
        let canceled = [(BuildingKind::Fabricator, anchor)];
        let cancellations = FoundationCancellations(&canceled);
        let founder = &obs.my_units[0];
        assert!(!builder_is_free(&obs, founder));
        assert!(cancellations.builder_is_free(&obs, founder));
        let geometry = PlacementGeometry::after_cancellations(&obs, cancellations);
        assert!(geometry.valid(&policy, BuildingKind::Fabricator, anchor));
        assert!(policy.placement_geometry_valid_except(
            &obs,
            BuildingKind::Fabricator,
            anchor,
            None,
            cancellations
        ));
        let index = (anchor.y * obs.map_width + anchor.x) as usize;
        assert!(
            !GroundEgressCache::ground_egress_base_open(&obs, FoundationCancellations::default())
                [index]
        );
        assert!(GroundEgressCache::ground_egress_base_open(&obs, cancellations)[index]);
        policy.prepare_ground_producer_egress_after(&obs, cancellations);
        assert!(policy.placement_valid_prepared(
            &obs,
            BuildingKind::Fabricator,
            anchor,
            cancellations
        ));
        assert!(!placement_valid(
            &policy,
            &obs,
            BuildingKind::Fabricator,
            anchor
        ));
        assert_eq!(obs.my_units[0].founding, Some(canceled[0]));
        obs.my_units[0].site = Some(obs.my_buildings[0].id);
        assert!(!cancellations.builder_is_free(&obs, &obs.my_units[0]));
        obs.my_units[0].site = None;
        obs.my_queued_units.push(obs.my_units[0].id);
        assert!(!cancellations.builder_is_free(&obs, &obs.my_units[0]));
    }

    #[test]
    fn fresh_foundations_do_not_displace_an_active_builder_from_its_work_tile() {
        let mut obs = observation();
        let site = TilePos::new(10, 5);
        let work = TilePos::new(10, 6);
        add_building(&mut obs, BuildingKind::Reclaimer, site);
        obs.my_buildings[1].built = false;
        obs.my_units.push(UnitObs {
            id: UnitId(70),
            player: obs.me,
            kind: UnitKind::Harvester,
            tile: work,
            hp: 60,
            idle: false,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: Some(obs.my_buildings[1].id),
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        });
        let unobserved = UtilityPolicy::new();
        assert!(placement_valid(
            &unobserved,
            &obs,
            BuildingKind::Reclaimer,
            work
        ));

        let mut policy = UtilityPolicy::new();
        policy.observe_work_experience(&obs);
        assert!(policy.tile_open(&obs, work));
        assert!(!placement_valid(
            &policy,
            &obs,
            BuildingKind::Reclaimer,
            work
        ));
        assert!(placement_valid(
            &policy,
            &obs,
            BuildingKind::ScuttleCharge,
            work
        ));
        assert!(placement_valid(
            &policy,
            &obs,
            BuildingKind::Reclaimer,
            TilePos::new(12, 9)
        ));

        obs.tick += 12;
        obs.my_units[0].tile = TilePos::new(12, 9);
        policy.observe_work_experience(&obs);
        assert!(placement_valid(
            &policy,
            &obs,
            BuildingKind::Reclaimer,
            work
        ));
        assert!(!placement_valid(
            &policy,
            &obs,
            BuildingKind::Reclaimer,
            obs.my_units[0].tile
        ));

        obs.tick += 12;
        obs.my_buildings[1].built = true;
        obs.my_units[0].site = None;
        obs.my_units[0].idle = true;
        policy.observe_work_experience(&obs);
        assert!(placement_valid(
            &policy,
            &obs,
            BuildingKind::Reclaimer,
            obs.my_units[0].tile
        ));
    }

    #[test]
    fn a_parked_hostile_airframe_closes_a_placement_tile_until_it_lifts_off() {
        let mut obs = observation();
        let policy = UtilityPolicy::new();
        let tile = (0..obs.map_height)
            .flat_map(|y| (0..obs.map_width).map(move |x| TilePos::new(x, y)))
            .find(|t| {
                policy.placement_tile_open_except(
                    &obs,
                    *t,
                    None,
                    FoundationCancellations::default(),
                )
            })
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
            !policy.placement_tile_open_except(
                &obs,
                tile,
                None,
                FoundationCancellations::default()
            ),
            "a parked airframe is a ground body the sim would refuse a footprint over"
        );
        obs.enemy_units.last_mut().unwrap().grounded = false;
        assert!(
            policy.placement_tile_open_except(&obs, tile, None, FoundationCancellations::default()),
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
        assert!(
            GroundEgressCache::same_layout(&with_mine, &obs),
            "a nonblocking foundation must not invalidate the egress cache"
        );

        let cold = UtilityPolicy::new();
        cold.prepare_ground_producer_egress(&with_mine);
        {
            let cache = cold.ground_egress_cache.borrow();
            let cache = cache.as_ref().expect("cold egress is prepared");
            let index = (mine_anchor.y * with_mine.map_width + mine_anchor.x) as usize;
            assert!(cache.base_open()[index]);
            let certificate = cache
                .decisions()
                .get(&Vec::new())
                .and_then(Clone::clone)
                .expect("the founding mine leaves a cold route certificate");
            assert_eq!(certificate.routes()[0].first(), Some(&mine_anchor));
        }

        let cached = UtilityPolicy::new();
        cached.prepare_ground_producer_egress(&obs);
        let before = cached
            .ground_egress_cache
            .borrow()
            .as_ref()
            .expect("baseline egress is prepared")
            .decisions()
            .get(&Vec::new())
            .and_then(Clone::clone)
            .expect("the baseline has a route certificate");
        cached.prepare_ground_producer_egress(&with_mine);
        let after = cached
            .ground_egress_cache
            .borrow()
            .as_ref()
            .expect("cached egress remains prepared")
            .decisions()
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
                cache.producer_count(),
                1,
                "the irrecoverably sealed producer is omitted"
            );
            let certificate = cache
                .decisions()
                .get(&Vec::new())
                .and_then(Clone::clone)
                .expect("the usable producer still has a certificate");
            assert_eq!(certificate.routes().len(), 1);
            assert_eq!(certificate.routes()[0].first(), Some(&last_spawn));
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
    fn nonintersecting_placement_reuses_the_cached_route_certificate() {
        let obs = observation();
        let policy = UtilityPolicy::new();
        policy.prepare_ground_producer_egress(&obs);
        let (baseline, candidate) = {
            let cache = policy.ground_egress_cache.borrow();
            let cache = cache.as_ref().expect("egress cache is prepared");
            let baseline = cache
                .decisions()
                .get(&Vec::new())
                .and_then(Clone::clone)
                .expect("the open fixture has a route certificate");
            let candidate = (0..obs.map_height)
                .flat_map(|y| (0..obs.map_width).map(move |x| TilePos::new(x, y)))
                .map(|anchor| (BuildingKind::Reclaimer, anchor))
                .find(|candidate| {
                    GroundEgressCache::certificate_routes_blocked(
                        &baseline,
                        *candidate,
                        cache.map_size(),
                    )
                    .is_empty()
                })
                .expect("some tile lies outside the canonical route");
            assert!(cache.certifies(candidate));
            (baseline, candidate)
        };

        assert!(policy.preserves_ground_producer_egress_prepared(&[], candidate));

        let cache = policy.ground_egress_cache.borrow();
        let certificate = cache
            .as_ref()
            .expect("egress cache remains prepared")
            .decisions()
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
                .decisions()
                .get(&Vec::new())
                .and_then(Clone::clone)
                .expect("the open fixture has a route certificate");
            let spawn = baseline.routes()[0][0];
            let candidate = (BuildingKind::Reclaimer, spawn);
            assert_eq!(
                GroundEgressCache::certificate_routes_blocked(
                    &baseline,
                    candidate,
                    cache.map_size(),
                ),
                vec![0]
            );
            assert!(!cache.certifies(candidate));
            (baseline, candidate)
        };

        assert!(policy.preserves_ground_producer_egress_prepared(&[], candidate));

        let cache = policy.ground_egress_cache.borrow();
        let certificate = cache
            .as_ref()
            .expect("egress cache remains prepared")
            .decisions()
            .get(&vec![candidate])
            .and_then(Clone::clone)
            .expect("an alternate route exists");
        assert!(!std::sync::Arc::ptr_eq(&baseline, &certificate));
        assert!(
            certificate.routes()[0]
                .iter()
                .all(|tile| !GroundEgressCache::candidate_blocks(candidate.0, candidate.1, *tile))
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
            .decisions()
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
                .decisions()
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
            provisional: false,
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
        let open =
            GroundEgressCache::ground_egress_base_open(&obs, FoundationCancellations::default());

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
            policy.placement_geometry_valid_except(
                &obs,
                BuildingKind::Reclaimer,
                last,
                None,
                FoundationCancellations::default()
            ),
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

        let mut with_rock = obs.clone();
        with_rock.known_rock.push(TilePos::new(1, 1));
        assert!(!GroundEgressCache::same_layout(&obs, &with_rock));

        let mut with_scrap = obs.clone();
        with_scrap.known_scrap.push((TilePos::new(1, 1), 10));
        assert!(!GroundEgressCache::same_layout(&obs, &with_scrap));

        let mut with_building = obs.clone();
        with_building.enemy_buildings.push(BuildingObs {
            provisional: false,
            id: BuildingId(100),
            player: PlayerId(1),
            kind: BuildingKind::Turret,
            anchor: TilePos::new(1, 1),
            hp: BuildingKind::Turret.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        });
        assert!(!GroundEgressCache::same_layout(&obs, &with_building));

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
        assert!(!GroundEgressCache::same_layout(&obs, &with_founding));

        let mut without_producer = obs.clone();
        without_producer.my_buildings[0].built = false;
        assert!(!GroundEgressCache::same_layout(&obs, &without_producer));
    }
}
