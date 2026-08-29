//! Fog-honest projection of movement-command routing.

use super::observation::{BuildingObs, Observation, UnitObs};
use crate::ids::UnitId;
use crate::stats::{Domain, GOAL_SNAP_RADIUS};
use chassis::grid::TilePos;
use std::collections::VecDeque;

/// Lazily labeled connected components for one movement domain. The first
/// query from a component floods it once; later units and candidate goals use
/// constant-time membership checks instead of repeating a map-sized search.
pub(super) struct RouteProjection<'a> {
    obs: &'a Observation,
    domain: Domain,
    require_explored: bool,
    blocked_ground_rect: Option<(TilePos, (i32, i32))>,
    blocked_ground_tiles: Vec<bool>,
    has_blocked_ground_tiles: bool,
    labels: Vec<u32>,
    next_label: u32,
}

impl<'a> RouteProjection<'a> {
    pub(super) fn new(obs: &'a Observation, domain: Domain) -> Self {
        let cells = usize::try_from(obs.map_width)
            .ok()
            .and_then(|width| {
                usize::try_from(obs.map_height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .unwrap_or(0);
        Self {
            obs,
            domain,
            require_explored: false,
            blocked_ground_rect: None,
            blocked_ground_tiles: vec![false; cells],
            has_blocked_ground_tiles: false,
            labels: vec![0; cells],
            next_label: 1,
        }
    }

    /// Ground routes whose complete traversable path is already explored.
    /// This is stricter than the default optimistic projection and is used
    /// when a ground unit must not chase a unit ferried onto another island.
    pub(super) fn known_ground(obs: &'a Observation) -> Self {
        let mut projection = Self::new(obs, Domain::Ground);
        projection.require_explored = true;
        projection
    }

    /// Ground routes that refuse every tile selected by `blocked`. This is
    /// used for work assignments whose endpoints may both be safe while the
    /// only path between them crosses a remembered kill zone.
    pub(super) fn ground_avoiding(
        obs: &'a Observation,
        mut blocked: impl FnMut(TilePos) -> bool,
    ) -> Self {
        let mut projection = Self::new(obs, Domain::Ground);
        for y in 0..obs.map_height {
            for x in 0..obs.map_width {
                let tile = TilePos::new(x, y);
                let index = projection.index(tile);
                let is_blocked = blocked(tile);
                projection.blocked_ground_tiles[index] = is_blocked;
                projection.has_blocked_ground_tiles |= is_blocked;
            }
        }
        projection
    }

    pub(super) fn unit_reaches(&mut self, unit: &UnitObs, goal: TilePos) -> bool {
        unit.kind.stats().domain == self.domain && self.reaches(unit.tile, goal)
    }

    /// Whether the direct command corridor stays outside every projected
    /// blocked tile. Pair this with [`Self::reaches`] so endpoints also share
    /// a danger-free component. The corridor check prevents ordinary movement
    /// from choosing a shorter route straight through a remembered kill zone.
    pub(super) fn direct_line_avoids_blocked(&self, from: TilePos, to: TilePos) -> bool {
        !chassis::path::line_blocked(from.center(), to.center(), |tile| {
            !in_bounds(self.obs, tile)
                || self.domain != Domain::Ground
                || !self.blocked_ground_tiles[self.index(tile)]
        })
    }

    /// Whether the canonical route chosen by an ordinary movement command
    /// stays outside every projected blocked tile. Callers first prove safe
    /// connectivity and a clear direct danger corridor. A map with no blocked
    /// tiles needs no search; otherwise reproduce the simulation's A* path
    /// because even unobstructed terrain can have several equally short routes.
    pub(super) fn command_path_avoids_blocked(&self, from: TilePos, to: TilePos) -> bool {
        if !self.has_blocked_ground_tiles {
            return true;
        }
        if !in_bounds(self.obs, from) || !domain_open(self.obs, self.domain, to) {
            return false;
        }
        chassis::path::astar(
            self.obs.map_width,
            self.obs.map_height,
            from,
            to,
            |tile| {
                domain_open(self.obs, self.domain, tile)
                    && (!self.require_explored || self.obs.explored(tile))
            },
            crate::stats::PATH_EXPANSION_CAP,
        )
        .is_some_and(|path| path.into_iter().all(|tile| self.open(tile)))
    }

    pub(super) fn group_reaches_command_goal(&mut self, units: &[UnitId], goal: TilePos) -> bool {
        let members = canonical_members(self.obs, units);
        if members.is_empty()
            || members
                .iter()
                .any(|unit| unit.kind.stats().domain != self.domain)
        {
            return false;
        }
        command_goals(
            self.obs,
            goal,
            members.len(),
            self.domain,
            self.require_explored,
        )
        .is_some_and(|goals| {
            members
                .into_iter()
                .zip(goals)
                .all(|(unit, assigned)| self.reaches(unit.tile, assigned))
        })
    }

    pub(super) fn reaches(&mut self, from: TilePos, to: TilePos) -> bool {
        if !in_bounds(self.obs, from) || !self.open(to) {
            return false;
        }
        if from == to {
            return true;
        }
        let Some(target_label) = self.label(to) else {
            return false;
        };
        if self.open(from) {
            return self.label(from) == Some(target_label);
        }
        [(1, 0), (-1, 0), (0, 1), (0, -1)]
            .into_iter()
            .map(|(dx, dy)| from.offset(dx, dy))
            .any(|neighbor| self.label(neighbor) == Some(target_label))
    }

    fn label(&mut self, tile: TilePos) -> Option<u32> {
        if !self.open(tile) || self.labels.is_empty() {
            return None;
        }
        let index = self.index(tile);
        if self.labels[index] != 0 {
            return Some(self.labels[index]);
        }
        let label = self.next_label;
        self.next_label = self
            .next_label
            .checked_add(1)
            .expect("map has fewer components than u32 labels");
        let mut open = VecDeque::from([tile]);
        self.labels[index] = label;
        while let Some(current) = open.pop_front() {
            // Cardinal connectivity is sufficient even though simulation A*
            // also walks diagonally: no-corner-cut diagonals require both
            // cardinal companions, so they never join two cardinal components.
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let next = current.offset(dx, dy);
                if !self.open(next) {
                    continue;
                }
                let next_index = self.index(next);
                if self.labels[next_index] == 0 {
                    self.labels[next_index] = label;
                    open.push_back(next);
                }
            }
        }
        Some(label)
    }

    fn open(&self, tile: TilePos) -> bool {
        let blocked_by_candidate = self.domain == Domain::Ground
            && self
                .blocked_ground_rect
                .is_some_and(|(anchor, (width, height))| {
                    (anchor.x..anchor.x + width).contains(&tile.x)
                        && (anchor.y..anchor.y + height).contains(&tile.y)
                });
        let blocked_by_tile = self.domain == Domain::Ground
            && in_bounds(self.obs, tile)
            && self.blocked_ground_tiles[self.index(tile)];
        domain_open(self.obs, self.domain, tile)
            && !blocked_by_candidate
            && !blocked_by_tile
            && (!self.require_explored || self.obs.explored(tile))
    }

    fn index(&self, tile: TilePos) -> usize {
        (tile.y * self.obs.map_width + tile.x) as usize
    }
}

/// Whether this ground unit can reach a doorstep after the proposed
/// footprint becomes blocking terrain. Checking the current route to the
/// anchor is insufficient when the builder is standing in, or must cross,
/// the tiles the new site will claim.
pub(super) fn unit_reaches_build_site(
    obs: &Observation,
    unit: &UnitObs,
    anchor: TilePos,
    size: (i32, i32),
) -> bool {
    if unit.kind.stats().domain != Domain::Ground {
        return false;
    }
    let mut routes = RouteProjection::new(obs, Domain::Ground);
    routes.blocked_ground_rect = Some((anchor, size));
    for tile in crate::tick::rect_adjacent_tiles(anchor, size) {
        if routes.open(tile) && (unit.tile == tile || routes.reaches(unit.tile, tile)) {
            return true;
        }
    }
    false
}

/// Whether the route an ordinary Build command will choose avoids every tile
/// selected by `blocked`.
///
/// Construction approaches the nearest passable doorsteps, rotating the first
/// four by owner-local unit rank to spread a crew around the footprint.
/// Reproduce that exact choice here: accepting a merely available safe detour
/// is insufficient when the simulation will choose a shorter route through a
/// remembered kill zone.
pub(super) fn build_command_path_avoids(
    obs: &Observation,
    unit: &UnitObs,
    anchor: TilePos,
    size: (i32, i32),
    defer: bool,
    mut blocked: impl FnMut(TilePos) -> bool,
) -> bool {
    if unit.kind.stats().domain != Domain::Ground || !in_bounds(obs, unit.tile) {
        return false;
    }
    let inside = |tile: TilePos| {
        tile.x >= anchor.x
            && tile.x < anchor.x + size.0
            && tile.y >= anchor.y
            && tile.y < anchor.y + size.1
    };
    let open = |tile: TilePos| domain_open(obs, Domain::Ground, tile) && (defer || !inside(tile));
    let mut candidates: Vec<_> = crate::tick::rect_adjacent_tiles(anchor, size)
        .filter(|tile| open(*tile))
        .collect();
    candidates.sort_by_key(|tile| crate::tick::rect_approach_key(unit.tile, anchor, size, *tile));
    let near = candidates.len().min(4);
    if near > 1 {
        let rank = crate::ids::owner_local_unit_rank(
            unit.id,
            unit.player,
            obs.my_units
                .iter()
                .map(|candidate| (candidate.id, candidate.player)),
        );
        candidates[..near].rotate_left(rank % near);
    }
    for goal in candidates {
        let Some(path) = chassis::path::astar(
            obs.map_width,
            obs.map_height,
            unit.tile,
            goal,
            open,
            crate::stats::PATH_EXPANSION_CAP,
        ) else {
            continue;
        };
        return !blocked(unit.tile)
            && !blocked(goal)
            && path.into_iter().all(|tile| !blocked(tile));
    }
    false
}

/// First fixed-size subgroup, in candidate preference order, whose exact
/// command spread remains reachable. Returned ids are canonical for lowering.
pub(super) fn first_reachable_group(
    routes: &mut RouteProjection<'_>,
    candidates: &[UnitId],
    size: usize,
    goal: TilePos,
) -> Option<Vec<UnitId>> {
    fn search(
        routes: &mut RouteProjection<'_>,
        candidates: &[UnitId],
        size: usize,
        goal: TilePos,
        start: usize,
        chosen: &mut Vec<UnitId>,
    ) -> Option<Vec<UnitId>> {
        if chosen.len() == size {
            if !routes.group_reaches_command_goal(chosen, goal) {
                return None;
            }
            let mut group = chosen.clone();
            group.sort_unstable();
            return Some(group);
        }
        let needed = size - chosen.len();
        for index in start..=candidates.len().saturating_sub(needed) {
            chosen.push(candidates[index]);
            if let Some(group) = search(routes, candidates, size, goal, index + 1, chosen) {
                return Some(group);
            }
            chosen.pop();
        }
        None
    }

    (candidates.len() >= size).then_some(())?;
    search(
        routes,
        candidates,
        size,
        goal,
        0,
        &mut Vec::with_capacity(size),
    )
}

/// The largest canonical subset that can accept one mixed-domain Move or
/// AttackMove. Removing a refused member changes later spread goals, so repeat
/// until every remaining member reaches the goal it would actually receive.
pub(super) fn routable_command_subset(
    obs: &Observation,
    units: &[UnitId],
    goal: TilePos,
) -> Vec<UnitId> {
    let mut members = canonical_members(obs, units);
    let mut ground = RouteProjection::new(obs, Domain::Ground);
    let mut air = RouteProjection::new(obs, Domain::Air);
    loop {
        let before = members.len();
        let mut retained = Vec::with_capacity(before);
        for domain in [Domain::Ground, Domain::Air] {
            let domain_members: Vec<_> = members
                .iter()
                .copied()
                .filter(|unit| unit.kind.stats().domain == domain)
                .collect();
            let Some(goals) = command_goals(obs, goal, domain_members.len(), domain, false) else {
                continue;
            };
            retained.extend(
                domain_members
                    .into_iter()
                    .zip(goals)
                    .filter(|(unit, assigned)| match domain {
                        Domain::Ground => ground.reaches(unit.tile, *assigned),
                        Domain::Air => air.reaches(unit.tile, *assigned),
                    })
                    .map(|(unit, _)| unit),
            );
        }
        retained.sort_unstable_by_key(|unit| unit.id);
        members = retained;
        if members.len() == before {
            return members.into_iter().map(|unit| unit.id).collect();
        }
    }
}

/// Whether projected ground movement may enter a tile. Unexplored terrain is
/// optimistically open; known static terrain, live or remembered scrap, and
/// non-stealth building footprints remain closed.
pub(super) fn ground_open(obs: &Observation, tile: TilePos) -> bool {
    in_bounds(obs, tile)
        && !obs.known_rock_at(tile)
        && !obs.known_scrap_at(tile)
        && !known_building_covers(obs, tile)
}

/// The projected ground goals assigned by Move or AttackMove.
pub(super) fn ground_command_goals(
    obs: &Observation,
    goal: TilePos,
    count: usize,
) -> Option<Vec<TilePos>> {
    command_goals(obs, goal, count, Domain::Ground, false)
}

fn canonical_members<'a>(obs: &'a Observation, units: &[UnitId]) -> Vec<&'a UnitObs> {
    let mut ids = units.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let mut members: Vec<_> = obs
        .my_units
        .iter()
        .filter(|unit| ids.binary_search(&unit.id).is_ok())
        .collect();
    members.sort_unstable_by_key(|unit| unit.id);
    members
}

fn command_goals(
    obs: &Observation,
    goal: TilePos,
    count: usize,
    domain: Domain,
    require_explored: bool,
) -> Option<Vec<TilePos>> {
    if count == 0 {
        return Some(Vec::new());
    }
    let center = command_center(obs, goal, domain, require_explored)?;
    let mut goals = Vec::with_capacity(count);
    'scan: for radius in 0..=GOAL_SNAP_RADIUS + 3 {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx.abs().max(dy.abs()) != radius {
                    continue;
                }
                let tile = center.offset(dx, dy);
                if domain_open(obs, domain, tile) && (!require_explored || obs.explored(tile)) {
                    goals.push(tile);
                    if goals.len() == count {
                        break 'scan;
                    }
                }
            }
        }
    }
    while goals.len() < count {
        goals.push(goals.last().copied().unwrap_or(center));
    }
    Some(goals)
}

fn command_center(
    obs: &Observation,
    goal: TilePos,
    domain: Domain,
    require_explored: bool,
) -> Option<TilePos> {
    match domain {
        Domain::Ground => ring_open(
            obs,
            goal,
            GOAL_SNAP_RADIUS,
            Domain::Ground,
            require_explored,
        ),
        Domain::Air => {
            if obs.map_width <= 0 || obs.map_height <= 0 {
                return None;
            }
            let clamped = TilePos::new(
                goal.x.clamp(0, obs.map_width - 1),
                goal.y.clamp(0, obs.map_height - 1),
            );
            (domain_open(obs, Domain::Air, clamped) && (!require_explored || obs.explored(clamped)))
                .then_some(clamped)
                .or_else(|| {
                    ring_open(
                        obs,
                        clamped,
                        GOAL_SNAP_RADIUS + 3,
                        Domain::Air,
                        require_explored,
                    )
                })
        }
    }
}

fn ring_open(
    obs: &Observation,
    goal: TilePos,
    radius_limit: i32,
    domain: Domain,
    require_explored: bool,
) -> Option<TilePos> {
    for radius in 0..=radius_limit {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx.abs().max(dy.abs()) != radius {
                    continue;
                }
                let tile = goal.offset(dx, dy);
                if domain_open(obs, domain, tile) && (!require_explored || obs.explored(tile)) {
                    return Some(tile);
                }
            }
        }
    }
    None
}

fn domain_open(obs: &Observation, domain: Domain, tile: TilePos) -> bool {
    match domain {
        Domain::Ground => ground_open(obs, tile),
        Domain::Air => in_bounds(obs, tile) && !known_peak(obs, tile),
    }
}

fn known_peak(obs: &Observation, tile: TilePos) -> bool {
    obs.known_peaks
        .binary_search_by_key(&(tile.y, tile.x), |peak| (peak.y, peak.x))
        .is_ok()
}

fn known_building_covers(obs: &Observation, tile: TilePos) -> bool {
    let covers = |building: &BuildingObs| {
        if building.kind.is_stealthy() {
            return false;
        }
        let (width, height) = building.kind.base_stats().size;
        (building.anchor.x..building.anchor.x + width).contains(&tile.x)
            && (building.anchor.y..building.anchor.y + height).contains(&tile.y)
    };
    obs.my_buildings.iter().any(covers)
        || obs.ally_buildings.iter().any(covers)
        || obs.enemy_buildings.iter().any(covers)
}

fn in_bounds(obs: &Observation, tile: TilePos) -> bool {
    (0..obs.map_width).contains(&tile.x) && (0..obs.map_height).contains(&tile.y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::observation::OBSERVATION_VERSION;
    use crate::ids::PlayerId;
    use crate::state::Faction;
    use crate::stats::{BuildingKind, UnitKind};

    fn observation() -> Observation {
        Observation {
            version: OBSERVATION_VERSION,
            tick: 0,
            me: PlayerId(0),
            scrap: 0,
            map_width: 12,
            map_height: 8,
            my_units: vec![unit(1, Domain::Ground), unit(2, Domain::Ground)],
            my_buildings: Vec::new(),
            my_queues: Vec::new(),
            ally_units: Vec::new(),
            ally_buildings: Vec::new(),
            enemy_units: Vec::new(),
            enemy_buildings: Vec::new(),
            visible: vec![false; 12 * 8],
            explored: vec![false; 12 * 8],
            known_scrap: Vec::new(),
            known_rock: Vec::new(),
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

    fn unit(id: u32, domain: Domain) -> UnitObs {
        let kind = match domain {
            Domain::Ground => UnitKind::Sentinel,
            Domain::Air => UnitKind::Buzzard,
        };
        UnitObs {
            id: UnitId(id),
            player: PlayerId(0),
            kind,
            tile: TilePos::new(2, 3 + id as i32 - 1),
            hp: kind.stats().max_hp,
            idle: true,
            carrying: 0,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        }
    }

    fn building(
        id: u32,
        player: u8,
        kind: BuildingKind,
        anchor: TilePos,
        seen: bool,
    ) -> BuildingObs {
        BuildingObs {
            id: crate::ids::BuildingId(id),
            player: PlayerId(player),
            kind,
            anchor,
            hp: kind.base_stats().max_hp,
            built: true,
            seen,
            tier: 0,
        }
    }

    #[test]
    fn known_wall_refuses_the_group_but_a_gap_restores_the_exact_route() {
        let mut obs = observation();
        obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(6, y)).collect();
        let mut routes = RouteProjection::new(&obs, Domain::Ground);
        assert!(!routes.group_reaches_command_goal(&[UnitId(1), UnitId(2)], TilePos::new(9, 4)));

        obs.known_rock.retain(|tile| tile.y != 4);
        let mut routes = RouteProjection::new(&obs, Domain::Ground);
        assert!(routes.group_reaches_command_goal(&[UnitId(1), UnitId(2)], TilePos::new(9, 4)));
    }

    #[test]
    fn routable_subset_drops_only_the_aircraft_behind_a_known_peak_wall() {
        let mut obs = observation();
        obs.my_units.push(UnitObs {
            tile: TilePos::new(9, 3),
            ..unit(3, Domain::Air)
        });
        obs.my_units.sort_by_key(|unit| unit.id);
        obs.known_peaks = (0..obs.map_height).map(|y| TilePos::new(6, y)).collect();

        assert_eq!(
            routable_command_subset(&obs, &[UnitId(1), UnitId(2), UnitId(3)], TilePos::new(2, 3)),
            vec![UnitId(1), UnitId(2)]
        );
    }

    #[test]
    fn mixed_domain_pruning_is_canonical_and_ignores_duplicate_or_unknown_ids() {
        let mut obs = observation();
        obs.my_units[1].tile = TilePos::new(9, 4);
        obs.my_units.extend([
            UnitObs {
                tile: TilePos::new(9, 3),
                ..unit(3, Domain::Air)
            },
            UnitObs {
                tile: TilePos::new(2, 5),
                ..unit(4, Domain::Air)
            },
        ]);
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(6, y)).collect();
        obs.known_peaks = obs.known_rock.clone();

        let retained = routable_command_subset(
            &obs,
            &[
                UnitId(4),
                UnitId(3),
                UnitId(2),
                UnitId(1),
                UnitId(4),
                UnitId(999),
            ],
            TilePos::new(2, 3),
        );

        assert_eq!(retained, vec![UnitId(1), UnitId(4)]);
        let mut ground = RouteProjection::new(&obs, Domain::Ground);
        assert!(
            ground.group_reaches_command_goal(
                &[UnitId(1), UnitId(1), UnitId(999)],
                TilePos::new(2, 3),
            )
        );
        assert!(!ground.group_reaches_command_goal(&[UnitId(999)], TilePos::new(2, 3)));
    }

    #[test]
    fn known_ground_requires_explored_connectivity_while_default_routing_is_optimistic() {
        let mut obs = observation();
        let from = TilePos::new(2, 3);
        let goal = TilePos::new(9, 3);

        assert!(RouteProjection::new(&obs, Domain::Ground).reaches(from, goal));
        assert!(!RouteProjection::known_ground(&obs).reaches(from, goal));

        obs.explored.fill(true);
        assert!(RouteProjection::known_ground(&obs).reaches(from, goal));

        obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(6, y)).collect();
        assert!(!RouteProjection::new(&obs, Domain::Ground).reaches(from, goal));
        assert!(!RouteProjection::known_ground(&obs).reaches(from, goal));
    }

    #[test]
    fn projected_building_footprints_block_ground_for_every_owner_except_stealth() {
        let mut obs = observation();
        obs.my_buildings.push(building(
            10,
            0,
            BuildingKind::Turret,
            TilePos::new(4, 2),
            true,
        ));
        obs.ally_buildings.push(building(
            11,
            2,
            BuildingKind::RepairBay,
            TilePos::new(6, 2),
            true,
        ));
        obs.enemy_buildings.push(building(
            u32::MAX,
            1,
            BuildingKind::Foundry,
            TilePos::new(8, 2),
            false,
        ));
        obs.enemy_buildings.push(building(
            12,
            1,
            BuildingKind::ScuttleCharge,
            TilePos::new(3, 6),
            true,
        ));

        assert!(!ground_open(&obs, TilePos::new(4, 2)));
        assert!(!ground_open(&obs, TilePos::new(6, 2)));
        assert!(!ground_open(&obs, TilePos::new(9, 3)));
        assert!(
            ground_open(&obs, TilePos::new(3, 6)),
            "a detected Scuttle Charge does not become a movement blocker"
        );
    }

    #[test]
    fn first_reachable_group_skips_preferred_combinations_that_cannot_share_the_goal() {
        let mut obs = observation();
        obs.my_units.push(UnitObs {
            tile: TilePos::new(9, 3),
            ..unit(3, Domain::Ground)
        });
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(6, y)).collect();
        let mut routes = RouteProjection::new(&obs, Domain::Ground);

        let selected = first_reachable_group(
            &mut routes,
            &[UnitId(3), UnitId(1), UnitId(2)],
            2,
            TilePos::new(2, 3),
        );

        assert_eq!(selected, Some(vec![UnitId(1), UnitId(2)]));
    }

    #[test]
    fn build_routing_accounts_for_the_footprint_before_selecting_a_founder() {
        let mut obs = observation();
        obs.my_units = vec![
            UnitObs {
                kind: UnitKind::Harvester,
                tile: TilePos::new(5, 3),
                ..unit(1, Domain::Ground)
            },
            UnitObs {
                kind: UnitKind::Harvester,
                tile: TilePos::new(8, 3),
                ..unit(2, Domain::Ground)
            },
        ];
        obs.known_rock = vec![TilePos::new(5, 2), TilePos::new(4, 3)];
        let anchor = TilePos::new(5, 3);
        let size = (2, 2);

        let mut current = RouteProjection::new(&obs, Domain::Ground);
        assert!(
            current.group_reaches_command_goal(&[UnitId(1)], anchor),
            "the current map makes the trapped founder look eligible"
        );
        assert!(
            !unit_reaches_build_site(&obs, &obs.my_units[0], anchor, size),
            "placing the footprint seals the nearer founder in"
        );
        assert!(
            unit_reaches_build_site(&obs, &obs.my_units[1], anchor, size),
            "a founder already outside the sealed pocket can reach a doorstep"
        );
    }

    #[test]
    fn visible_build_routes_match_post_footprint_reachability_but_deferred_sites_allow_escape() {
        let mut obs = observation();
        obs.my_units = vec![
            UnitObs {
                kind: UnitKind::Harvester,
                tile: TilePos::new(5, 3),
                ..unit(1, Domain::Ground)
            },
            UnitObs {
                kind: UnitKind::Harvester,
                tile: TilePos::new(8, 3),
                ..unit(2, Domain::Ground)
            },
        ];
        obs.known_rock = vec![TilePos::new(5, 2), TilePos::new(4, 3)];
        let anchor = TilePos::new(5, 3);
        let size = (2, 2);

        for builder in &obs.my_units {
            assert_eq!(
                build_command_path_avoids(&obs, builder, anchor, size, false, |_| false),
                unit_reaches_build_site(&obs, builder, anchor, size),
                "visible construction and founder selection must agree for {:?}",
                builder.id
            );
        }
        assert!(
            build_command_path_avoids(&obs, &obs.my_units[0], anchor, size, true, |_| false),
            "an unseen deferred footprint must let its builder leave before the site materializes"
        );
    }

    #[test]
    fn build_path_danger_uses_owner_local_doorstep_rank_across_sparse_global_ids() {
        let mut template = observation();
        let origin = TilePos::new(2, 3);
        let anchor = TilePos::new(8, 3);
        let size = (2, 2);
        template.my_units.clear();
        template.explored.fill(true);

        let mut doorsteps: Vec<_> = crate::tick::rect_adjacent_tiles(anchor, size).collect();
        doorsteps.sort_by_key(|tile| crate::tick::rect_approach_key(origin, anchor, size, *tile));
        assert_eq!(
            &doorsteps[..4],
            &[
                TilePos::new(7, 2),
                TilePos::new(7, 3),
                TilePos::new(7, 4),
                TilePos::new(7, 5),
            ],
            "the first four command doorsteps are stable in the builder's approach frame"
        );
        let inside = |tile: TilePos| {
            tile.x >= anchor.x
                && tile.x < anchor.x + size.0
                && tile.y >= anchor.y
                && tile.y < anchor.y + size.1
        };
        let path_to = |goal| {
            chassis::path::astar(
                template.map_width,
                template.map_height,
                origin,
                goal,
                |tile| ground_open(&template, tile) && !inside(tile),
                crate::stats::PATH_EXPANSION_CAP,
            )
            .expect("each canonical doorstep is reachable")
        };
        let rank_zero_path = path_to(doorsteps[0]);
        let rank_one_path = path_to(doorsteps[1]);
        let danger = rank_zero_path
            .iter()
            .copied()
            .find(|tile| !rank_one_path.contains(tile))
            .expect("the two canonical doorstep paths diverge");
        assert!(!rank_one_path.contains(&danger));

        let outcomes = |ids: [u32; 2], interleaved_enemy_ids: &[u32]| {
            let mut obs = template.clone();
            let builder = |id| {
                let mut unit = unit(1, Domain::Ground);
                unit.id = UnitId(id);
                unit.kind = UnitKind::Harvester;
                unit.tile = origin;
                unit.hp = UnitKind::Harvester.stats().max_hp;
                unit
            };
            obs.my_units = ids.into_iter().map(builder).collect();
            obs.enemy_units = interleaved_enemy_ids
                .iter()
                .copied()
                .map(|id| {
                    let mut enemy = unit(1, Domain::Ground);
                    enemy.id = UnitId(id);
                    enemy.player = PlayerId(1);
                    enemy.tile = TilePos::new(10, 6);
                    enemy
                })
                .collect();
            [
                build_command_path_avoids(&obs, &obs.my_units[0], anchor, size, false, |tile| {
                    tile == danger
                }),
                build_command_path_avoids(&obs, &obs.my_units[1], anchor, size, false, |tile| {
                    tile == danger
                }),
            ]
        };

        assert_eq!(outcomes([1, 2], &[]), [false, true]);
        assert_eq!(
            outcomes([4, 20], &[7, 12]),
            [false, true],
            "other seats' interleaved global ids must not alter either owner's local rank"
        );
    }

    #[test]
    fn invalid_map_dimensions_and_far_off_map_goals_fail_closed() {
        for (width, height) in [(0, 8), (-1, 8), (12, 0), (12, -1)] {
            let mut obs = observation();
            obs.map_width = width;
            obs.map_height = height;
            obs.visible.clear();
            obs.explored.clear();

            let mut ground = RouteProjection::new(&obs, Domain::Ground);
            assert!(!ground.reaches(TilePos::new(0, 0), TilePos::new(1, 1)));
            assert_eq!(
                routable_command_subset(&obs, &[UnitId(1), UnitId(2)], TilePos::new(1, 1)),
                Vec::<UnitId>::new()
            );
            assert_eq!(ground_command_goals(&obs, TilePos::new(1, 1), 2), None);
        }

        let obs = observation();
        assert_eq!(ground_command_goals(&obs, TilePos::new(-100, 100), 1), None);
    }

    #[test]
    fn air_goal_on_a_peak_snaps_from_the_clamped_edge_in_canonical_order() {
        let mut obs = observation();
        let clamped = TilePos::new(0, obs.map_height - 1);
        obs.known_peaks.push(clamped);

        assert_eq!(
            command_goals(&obs, TilePos::new(-100, 100), 1, Domain::Air, false,),
            Some(vec![clamped.offset(0, -1)]),
            "air goals clamp in-bounds, then ring-scan around an impassable peak"
        );
    }

    #[test]
    fn too_few_air_slots_reuse_the_last_canonical_goal_for_the_whole_group() {
        let mut obs = observation();
        let only_open = TilePos::new(6, 4);
        obs.known_peaks = (0..obs.map_height)
            .flat_map(|y| (0..obs.map_width).map(move |x| TilePos::new(x, y)))
            .filter(|tile| *tile != only_open)
            .collect();

        assert_eq!(
            command_goals(&obs, only_open, 3, Domain::Air, false),
            Some(vec![only_open; 3]),
            "a legal group order still needs one deterministic goal per member when open sky is scarce"
        );
    }

    #[test]
    fn safe_connectivity_does_not_make_a_cross_zone_command_corridor_safe() {
        let obs = observation();
        let blocked = TilePos::new(6, 3);
        let mut routes = RouteProjection::ground_avoiding(&obs, |tile| tile == blocked);

        assert!(
            routes.reaches(TilePos::new(2, 3), TilePos::new(9, 3)),
            "a safe detour still connects the two sides"
        );
        assert!(
            !routes.direct_line_avoids_blocked(TilePos::new(2, 3), TilePos::new(9, 3)),
            "the ordinary command corridor crosses the blocked tile"
        );
    }

    #[test]
    fn obstacle_detour_is_checked_against_the_exact_command_path() {
        let mut obs = observation();
        obs.known_rock = (0..obs.map_height)
            .filter(|y| !matches!(*y, 2 | 6))
            .map(|y| TilePos::new(6, y))
            .collect();
        let blocked = TilePos::new(6, 2);
        let mut routes = RouteProjection::ground_avoiding(&obs, |tile| tile == blocked);

        assert!(
            routes.reaches(TilePos::new(2, 3), TilePos::new(9, 3)),
            "the lower gap leaves a safe detour"
        );
        assert!(
            routes.direct_line_avoids_blocked(TilePos::new(2, 3), TilePos::new(9, 3)),
            "the blocked upper gap is not on the direct corridor"
        );
        assert!(
            !routes.command_path_avoids_blocked(TilePos::new(2, 3), TilePos::new(9, 3)),
            "ordinary A* prefers the shorter upper gap through the blocked tile"
        );
    }

    #[test]
    fn build_command_accepts_an_ordinary_path_that_avoids_unrelated_danger() {
        let mut obs = observation();
        obs.my_units[0].kind = UnitKind::Harvester;
        let unit = &obs.my_units[0];

        assert!(build_command_path_avoids(
            &obs,
            unit,
            TilePos::new(8, 3),
            (2, 2),
            false,
            |tile| tile == TilePos::new(5, 6),
        ));
    }

    #[test]
    fn build_command_rejects_a_blocked_canonical_path_despite_a_safe_detour() {
        let mut obs = observation();
        obs.my_units[0].kind = UnitKind::Harvester;
        let unit = &obs.my_units[0];
        let anchor = TilePos::new(8, 3);
        let size = (2, 2);
        let goal = TilePos::new(7, 2);
        let blocked = TilePos::new(5, 2);
        let inside = |tile: TilePos| {
            tile.x >= anchor.x
                && tile.x < anchor.x + size.0
                && tile.y >= anchor.y
                && tile.y < anchor.y + size.1
        };
        assert!(
            chassis::path::astar(
                obs.map_width,
                obs.map_height,
                unit.tile,
                goal,
                |tile| ground_open(&obs, tile) && !inside(tile) && tile != blocked,
                crate::stats::PATH_EXPANSION_CAP,
            )
            .is_some(),
            "a safe detour to the canonical doorstep exists"
        );

        assert!(!build_command_path_avoids(
            &obs,
            unit,
            anchor,
            size,
            false,
            |tile| tile == blocked,
        ));
    }
}
