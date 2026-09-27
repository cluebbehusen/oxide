//! Movement, staging, and firing geometry shared by air operation tactics
//! and campaign route planning.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct SuppressionOrigin {
    pub(super) tile: TilePos,
    pub(super) kind: UnitKind,
}

pub(super) fn artillery_firing_assignments(
    obs: &Observation,
    intel: &StrategicIntelligence,
    artillery: &[UnitId],
    target: Target,
    public_map: Option<&PublicMapBriefing>,
    orientation: Orientation,
) -> Option<Vec<(UnitId, TilePos)>> {
    if artillery.is_empty() {
        return None;
    }
    let mut members = artillery.to_vec();
    members.sort_unstable();
    members.dedup();
    let origins: Option<Vec<_>> = members
        .iter()
        .map(|id| {
            unit(obs, *id).map(|member| SuppressionOrigin {
                tile: member.tile,
                kind: member.kind,
            })
        })
        .collect();
    let firing_stands =
        suppression_firing_assignment(obs, intel, &origins?, target, public_map, orientation)?;
    Some(members.into_iter().zip(firing_stands).collect())
}

pub(super) fn suppression_firing_assignment(
    obs: &Observation,
    intel: &StrategicIntelligence,
    origins: &[SuppressionOrigin],
    target: Target,
    public_map: Option<&PublicMapBriefing>,
    orientation: Orientation,
) -> Option<Vec<TilePos>> {
    if origins.is_empty() {
        return None;
    }
    let routes = route_projection_with_orientation(obs, Domain::Ground, public_map, orientation);
    if origins.iter().all(|origin| *origin == origins[0]) {
        let mut stands: Vec<_> =
            suppression_firing_stands(&routes, obs, origins[0], target, intel, public_map)
                .take(origins.len())
                .collect();
        if stands.len() != origins.len() {
            return None;
        }
        // With identical options, each augmenting member displaces the earlier
        // members by one stand. Preserve that exact assignment order directly.
        stands.reverse();
        return Some(stands);
    }
    let mut by_origin = BTreeMap::new();
    let stand_options: Vec<Vec<TilePos>> = origins
        .iter()
        .map(|origin| {
            by_origin
                .entry(*origin)
                .or_insert_with(|| {
                    suppression_firing_stands(&routes, obs, *origin, target, intel, public_map)
                        .collect::<Vec<_>>()
                })
                .clone()
        })
        .collect();
    assign_suppression_stands(origins.len(), stand_options)
}

pub(super) fn assign_suppression_stands(
    member_count: usize,
    stand_options: Vec<Vec<TilePos>>,
) -> Option<Vec<TilePos>> {
    if stand_options.iter().any(Vec::is_empty) {
        return None;
    }

    let mut stands: Vec<_> = stand_options.iter().flatten().copied().collect();
    stands.sort_unstable_by_key(|stand| (stand.y, stand.x));
    stands.dedup();
    let options: Vec<Vec<usize>> = stand_options
        .iter()
        .map(|member_options| {
            member_options
                .iter()
                .map(|stand| {
                    stands
                        .binary_search_by_key(&(stand.y, stand.x), |candidate| {
                            (candidate.y, candidate.x)
                        })
                        .expect("every firing option came from the canonical stand set")
                })
                .collect()
        })
        .collect();
    let mut owner_by_stand = vec![None; stands.len()];
    for member in 0..member_count {
        let mut visited = vec![false; stands.len()];
        if !augment_suppression_assignment(member, &options, &mut visited, &mut owner_by_stand) {
            return None;
        }
    }
    let mut assigned = vec![None; member_count];
    for (stand, owner) in owner_by_stand.into_iter().enumerate() {
        if let Some(member) = owner {
            assigned[member] = Some(stands[stand]);
        }
    }
    assigned.into_iter().collect()
}

pub(super) fn augment_suppression_assignment(
    member: usize,
    options: &[Vec<usize>],
    visited: &mut [bool],
    owner_by_stand: &mut [Option<usize>],
) -> bool {
    for &stand in &options[member] {
        if visited[stand] {
            continue;
        }
        visited[stand] = true;
        if owner_by_stand[stand].is_none_or(|owner| {
            augment_suppression_assignment(owner, options, visited, owner_by_stand)
        }) {
            owner_by_stand[stand] = Some(member);
            return true;
        }
    }
    false
}

pub(super) fn exact_attack_group_reaches(
    routes: &mut RouteProjection<'_>,
    obs: &Observation,
    units: &[UnitId],
    target: TilePos,
) -> bool {
    !units.is_empty()
        && units
            .iter()
            .all(|id| unit(obs, *id).is_some_and(|member| routes.unit_reaches(member, target)))
}

#[derive(Debug, Clone, Copy)]
enum SuppressionTargetGeometry {
    Unit(TilePos),
    Building { anchor: TilePos, size: (i32, i32) },
}

impl SuppressionTargetGeometry {
    fn tile_bounds(self) -> (TilePos, TilePos) {
        match self {
            Self::Unit(tile) => (tile, tile),
            Self::Building {
                anchor,
                size: (width, height),
            } => (anchor, anchor.offset(width - 1, height - 1)),
        }
    }

    fn aim_point(self, shooter: Vec2Fx) -> Vec2Fx {
        match self {
            Self::Unit(tile) => tile.center(),
            Self::Building {
                anchor,
                size: (width, height),
            } => {
                let min = anchor.center() - Vec2Fx::new(HALF, HALF);
                let max = min + Vec2Fx::new(Fx::from_num(width), Fx::from_num(height));
                Vec2Fx::new(shooter.x.clamp(min.x, max.x), shooter.y.clamp(min.y, max.y))
            }
        }
    }
}

fn suppression_target_geometry(
    intel: &StrategicIntelligence,
    target: Target,
) -> Option<SuppressionTargetGeometry> {
    match target {
        Target::Unit(id) => intel
            .units()
            .iter()
            .find(|contact| {
                contact.id == id
                    && contact.evidence == ContactEvidence::Current
                    && contact.hp > 0
                    && contact.body_domain() == Domain::Ground
            })
            .map(|contact| SuppressionTargetGeometry::Unit(contact.tile)),
        Target::Building(id) => intel
            .buildings()
            .iter()
            .find(|contact| {
                contact.id == Some(id)
                    && contact.evidence == ContactEvidence::Current
                    && contact.built
                    && contact.hp > 0
            })
            .map(|contact| SuppressionTargetGeometry::Building {
                anchor: contact.anchor,
                size: contact.kind.tier_stats(contact.tier).size,
            }),
    }
}

fn suppression_weapon(kind: UnitKind) -> Option<&'static WeaponStats> {
    if !is_artillery(kind) {
        return None;
    }
    kind.stats()
        .weapons
        .iter()
        .find(|weapon| weapon.targets.covers(Domain::Ground))
}

pub(super) fn suppression_firing_stands(
    routes: &RouteProjection<'_>,
    obs: &Observation,
    origin: SuppressionOrigin,
    target: Target,
    intel: &StrategicIntelligence,
    public_map: Option<&PublicMapBriefing>,
) -> impl Iterator<Item = TilePos> {
    let mut stands = legal_suppression_stands(obs, origin, target, intel, public_map);
    stands.retain(|stand| routes.ground_command_reaches(origin.tile, *stand));
    stands.into_iter()
}

pub(super) fn legal_suppression_stands(
    obs: &Observation,
    origin: SuppressionOrigin,
    target: Target,
    intel: &StrategicIntelligence,
    public_map: Option<&PublicMapBriefing>,
) -> Vec<TilePos> {
    let mut stands = legal_suppression_tiles(obs, origin.kind, target, intel, public_map, |tile| {
        public_ground_open(obs, tile, public_map)
    });
    stands.sort_unstable_by_key(|stand| (stand.chebyshev(origin.tile), stand.y, stand.x));
    stands
}

pub(super) fn legal_suppression_tiles(
    obs: &Observation,
    kind: UnitKind,
    target: Target,
    intel: &StrategicIntelligence,
    public_map: Option<&PublicMapBriefing>,
    open: impl Fn(TilePos) -> bool,
) -> Vec<TilePos> {
    let mut stands = Vec::new();
    let Some(weapon) = suppression_weapon(kind) else {
        return stands;
    };
    let Some(geometry) = suppression_target_geometry(intel, target) else {
        return stands;
    };
    let (near, far) = geometry.tile_bounds();
    let radius = weapon.range.ceil().to_num::<i32>();
    for y in near.y.saturating_sub(radius)..=far.y.saturating_add(radius) {
        for x in near.x.saturating_sub(radius)..=far.x.saturating_add(radius) {
            let stand = TilePos::new(x, y);
            if !open(stand) || !suppression_shot_is_legal(obs, public_map, weapon, stand, geometry)
            {
                continue;
            }
            stands.push(stand);
        }
    }
    stands
}

fn suppression_shot_is_legal(
    obs: &Observation,
    public_map: Option<&PublicMapBriefing>,
    weapon: &WeaponStats,
    stand: TilePos,
    target: SuppressionTargetGeometry,
) -> bool {
    let shooter = stand.center();
    let aim = target.aim_point(shooter);
    let distance_sq = shooter.dist_sq(aim);
    if distance_sq > weapon.range * weapon.range
        || distance_sq < weapon.minimum_range * weapon.minimum_range
    {
        return false;
    }
    let shot_open = |tile| suppression_shot_tile_open(obs, public_map, weapon, tile);
    shot_open(TilePos::containing(aim)) && !chassis::path::line_blocked(shooter, aim, shot_open)
}

fn suppression_shot_tile_open(
    obs: &Observation,
    public_map: Option<&PublicMapBriefing>,
    weapon: &WeaponStats,
    tile: TilePos,
) -> bool {
    if !routing::in_bounds(obs, tile) {
        return false;
    }
    if let Some(map) = public_map {
        return map.terrain_at(tile).is_some_and(|terrain| {
            !terrain.blocks_all_fire() && (weapon.indirect || !terrain.blocks_direct_fire())
        });
    }
    if obs
        .known_peaks
        .binary_search_by_key(&(tile.y, tile.x), |peak| (peak.y, peak.x))
        .is_ok()
    {
        return false;
    }
    weapon.indirect || !obs.known_rock_at(tile)
}

/// Nearest and farthest Chebyshev rings around the home anchor searched for
/// a landing pad. Ring 1 is skipped so the pad never hugs the Foundry
/// doorstep that production spawns and harvest traffic use.
const LANDING_PAD_RINGS: core::ops::RangeInclusive<i32> = 2..=6;

/// A parking tile for the held wing: the first tile by ring, then (y, x),
/// around the home anchor that is on the map, not known impassable, and
/// not under an own or allied footprint. The sim still snaps a landing to
/// landable ground, so this only has to be a sensible, stable choice.
pub(super) fn landing_pad(obs: &Observation, home: TilePos) -> Option<TilePos> {
    let footprints: Vec<(TilePos, (i32, i32))> = obs
        .my_buildings
        .iter()
        .chain(obs.ally_buildings.iter())
        .map(|building| (building.anchor, building.kind.base_stats().size))
        .collect();
    let under_footprint = |tile: TilePos| {
        footprints.iter().any(|(anchor, (width, height))| {
            tile.x >= anchor.x
                && tile.x < anchor.x + width
                && tile.y >= anchor.y
                && tile.y < anchor.y + height
        })
    };
    LANDING_PAD_RINGS
        .flat_map(|ring| {
            (-ring..=ring).flat_map(move |dy| {
                (-ring..=ring)
                    .filter(move |dx| dx.abs().max(dy.abs()) == ring)
                    .map(move |dx| home.offset(dx, dy))
            })
        })
        .find(|tile| {
            routing::in_bounds(obs, *tile) && !obs.known_rock_at(*tile) && !under_footprint(*tile)
        })
}

pub(super) fn known_ground_connection(
    obs: &Observation,
    home: TilePos,
    target: TilePos,
    target_size: (i32, i32),
    public_map: Option<&PublicMapBriefing>,
) -> Option<bool> {
    let starts = home_ground_starts(obs, home, public_map);
    let goals: Vec<_> = oxide_sim::geometry::rect_adjacent_tiles(target, target_size)
        .filter(|tile| public_ground_open(obs, *tile, public_map))
        .collect();
    if starts.is_empty() || goals.is_empty() {
        return None;
    }

    if let Some(public_map) = public_map {
        return Some(public_map.regions().connects(&starts, &goals));
    }

    let optimistic = RouteProjection::new(QueryPurpose::AirOperation, obs, Domain::Ground);
    if !starts
        .iter()
        .any(|start| goals.iter().any(|goal| optimistic.reaches(*start, *goal)))
    {
        return Some(false);
    }

    let routes = RouteProjection::known_ground(QueryPurpose::AirOperation, obs);
    starts
        .iter()
        .any(|start| goals.iter().any(|goal| routes.reaches(*start, *goal)))
        .then_some(true)
}

pub(super) fn staging(home: TilePos, target: TilePos) -> TilePos {
    TilePos::new(
        home.x + (target.x - home.x) / 3,
        home.y + (target.y - home.y) / 3,
    )
}

pub(super) fn artillery_staging_candidates(
    obs: &Observation,
    home: TilePos,
    target: TilePos,
    public_map: Option<&PublicMapBriefing>,
) -> Vec<TilePos> {
    let ideal = staging(home, target);
    let mut candidates = Vec::new();
    for radius in 0i32..=3 {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx.abs().max(dy.abs()) != radius {
                    continue;
                }
                let candidate = ideal.offset(dx, dy);
                if public_ground_open(obs, candidate, public_map) {
                    candidates.push(candidate);
                }
            }
        }
    }
    candidates
}

pub(super) fn artillery_staging_with_routes(
    obs: &Observation,
    home: TilePos,
    target: TilePos,
    public_map: Option<&PublicMapBriefing>,
    routes: &RouteProjection<'_>,
) -> Option<TilePos> {
    let starts = home_ground_starts(obs, home, public_map);
    artillery_staging_candidates(obs, home, target, public_map)
        .into_iter()
        .find(|candidate| {
            starts
                .iter()
                .any(|start| routes.reaches(*start, *candidate))
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ArtilleryStaging {
    Ready(TilePos),
    NeedsRecon(TilePos),
}

/// A nearby staging tile every assigned artillery unit can reach through the
/// bot's optimistic known ground. The raw one-third point can fall inside a
/// mapped gulf; searching only its local ring preserves the intended line and
/// aborts when the operation actually requires a ferry. An unexplored candidate
/// must be reconnoitered before a ground command treats that optimism as fact.
pub(super) fn artillery_staging(
    op: &AirOperation,
    obs: &Observation,
    home: TilePos,
    target: TilePos,
    public_map: Option<&PublicMapBriefing>,
    orientation: Orientation,
) -> Option<ArtilleryStaging> {
    let routes = route_projection_with_orientation(obs, Domain::Ground, public_map, orientation);
    for candidate in artillery_staging_candidates(obs, home, target, public_map) {
        if routes.group_reaches_command_goal(&op.artillery, candidate) {
            return Some(if obs.explored(candidate) {
                ArtilleryStaging::Ready(candidate)
            } else {
                ArtilleryStaging::NeedsRecon(candidate)
            });
        }
    }
    None
}

pub(super) fn connected_public_map<'a>(
    plan: &AirPlan,
    public_map: Option<&'a PublicMapBriefing>,
) -> Option<&'a PublicMapBriefing> {
    if plan.airborne() { None } else { public_map }
}

pub(super) fn route_projection<'a>(
    obs: &'a Observation,
    domain: Domain,
    public_map: Option<&'a PublicMapBriefing>,
) -> RouteProjection<'a> {
    public_map.map_or_else(
        || RouteProjection::new(QueryPurpose::AirOperation, obs, domain),
        |map| RouteProjection::with_public_terrain(QueryPurpose::AirOperation, obs, domain, map),
    )
}

pub(super) fn route_projection_with_orientation<'a>(
    obs: &'a Observation,
    domain: Domain,
    public_map: Option<&'a PublicMapBriefing>,
    orientation: Orientation,
) -> RouteProjection<'a> {
    public_map.map_or_else(
        || RouteProjection::with_orientation(QueryPurpose::AirOperation, obs, domain, orientation),
        |map| {
            RouteProjection::with_public_terrain_and_orientation(
                QueryPurpose::AirOperation,
                obs,
                domain,
                map,
                orientation,
            )
        },
    )
}

pub(super) fn operation_route_projection<'a>(
    plan: &AirPlan,
    obs: &'a Observation,
    domain: Domain,
    public_map: Option<&'a PublicMapBriefing>,
    orientation: Orientation,
) -> RouteProjection<'a> {
    if matches!(plan, AirPlan::Connected(_)) {
        route_projection_with_orientation(obs, domain, public_map, orientation)
    } else {
        route_projection(obs, domain, public_map)
    }
}

/// Open ground around the home Foundry's footprint.
fn home_ground_starts(
    obs: &Observation,
    home: TilePos,
    public_map: Option<&PublicMapBriefing>,
) -> Vec<TilePos> {
    let home_size = obs
        .my_buildings
        .iter()
        .find(|building| {
            building.built && building.kind == BuildingKind::Foundry && building.anchor == home
        })
        .map_or(BuildingKind::Foundry.base_stats().size, |building| {
            building.kind.base_stats().size
        });
    oxide_sim::geometry::rect_adjacent_tiles(home, home_size)
        .filter(|tile| public_ground_open(obs, *tile, public_map))
        .collect()
}

pub(super) fn public_ground_open(
    obs: &Observation,
    tile: TilePos,
    public_map: Option<&PublicMapBriefing>,
) -> bool {
    routing::ground_open(QueryPurpose::AirOperation, obs, tile)
        && public_map.is_none_or(|map| {
            map.terrain_at(tile)
                .is_some_and(|terrain| !terrain.blocks_ground())
        })
}
