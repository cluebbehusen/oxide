//! Shared resource-access evidence for defense and support placement.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResourceInputs {
    map_size: (i32, i32),
    blocked: Vec<bool>,
    nodes: Vec<TilePos>,
    harvest_targets: Vec<TilePos>,
    foundries: Vec<TilePos>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::bot::utility) struct ResourceAssets {
    inputs: ResourceInputs,
    assets: Vec<DefendedAsset>,
}

pub(super) fn scrap_assets(
    policy: &UtilityPolicy,
    ground: &GroundKnowledge<'_>,
    foundries: &[TilePos],
) -> Vec<DefendedAsset> {
    let inputs = ResourceInputs {
        map_size: (ground.obs.map_width, ground.obs.map_height),
        blocked: ground.ground_blocked.clone(),
        nodes: ground
            .scrap
            .iter()
            .filter(|(tile, amount)| **amount > 0 && !policy.dead_nodes.contains(tile))
            .map(|(tile, _)| *tile)
            .collect(),
        harvest_targets: sorted_tiles(
            ground
                .obs
                .my_units
                .iter()
                .filter(|unit| unit.kind.stats().harvest.is_some())
                .filter_map(|unit| unit.harvesting),
        ),
        foundries: foundries.to_vec(),
    };
    if let Some(cached) = policy.resource_assets.borrow().as_ref()
        && cached.inputs == inputs
    {
        let mut assets = cached.assets.clone();
        for asset in &mut assets {
            let AssetShape::Scrap { tiles, .. } = &asset.shape else {
                unreachable!()
            };
            asset.value = resource_value(ground, tiles);
        }
        return assets;
    }
    let assets = collect_assets(policy, ground, foundries);
    *policy.resource_assets.borrow_mut() = Some(ResourceAssets {
        inputs,
        assets: assets.clone(),
    });
    assets
}

fn collect_assets(
    policy: &UtilityPolicy,
    ground: &GroundKnowledge<'_>,
    foundries: &[TilePos],
) -> Vec<DefendedAsset> {
    let mut remaining: BTreeSet<_> = ground
        .scrap
        .iter()
        .filter(|(tile, amount)| **amount > 0 && !policy.dead_nodes.contains(tile))
        .map(|(tile, _)| *tile)
        .collect();
    let mut clusters = Vec::new();
    while let Some(seed) = remaining
        .iter()
        .min_by_key(|tile| (tile.y, tile.x))
        .copied()
    {
        remaining.remove(&seed);
        let mut open = VecDeque::from([seed]);
        let mut tiles = Vec::new();
        while let Some(tile) = open.pop_front() {
            tiles.push(tile);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if (dx != 0 || dy != 0) && remaining.remove(&tile.offset(dx, dy)) {
                        open.push_back(tile.offset(dx, dy));
                    }
                }
            }
        }
        tiles.sort_by_key(|tile| (tile.y, tile.x));
        let work_tiles = scrap_work_tiles(ground, &tiles, None);
        if !resource_region_is_active(ground.obs, &tiles) {
            continue;
        }
        let support = foundries
            .iter()
            .filter(|foundry| {
                let own_distance = tiles
                    .iter()
                    .map(|tile| tile.chebyshev(**foundry))
                    .min()
                    .unwrap_or(i32::MAX);
                own_distance <= HOME_SALVAGE_RADIUS
            })
            .filter_map(|foundry| {
                shortest_path_between(
                    ground,
                    &building_doorsteps(ground, *foundry, BuildingKind::Foundry.base_stats().size),
                    &work_tiles,
                    None,
                    DefenseDomain::Ground,
                )
                .map(|(_, _goal, path)| AccessRoute {
                    foundry: *foundry,
                    work_tiles: work_tiles.clone(),
                    path,
                })
            })
            .min_by_key(|route| {
                (
                    route.path.len(),
                    route.foundry.y,
                    route.foundry.x,
                    route.path.clone(),
                )
            });
        let Some(access) = support else { continue };
        let value = resource_value(ground, &tiles);
        if value > 0 {
            clusters.push(DefendedAsset {
                value,
                shape: AssetShape::Scrap { tiles, work_tiles },
                access: Some(access),
            });
        }
    }
    clusters
}

fn resource_region_is_active(obs: &Observation, resource_tiles: &[TilePos]) -> bool {
    obs.my_units.iter().any(|unit| {
        unit.kind.stats().harvest.is_some()
            && unit.harvesting.is_some_and(|target| {
                resource_tiles
                    .binary_search_by_key(&(target.y, target.x), |tile| (tile.y, tile.x))
                    .is_ok()
            })
    })
}

fn resource_value(ground: &GroundKnowledge<'_>, tiles: &[TilePos]) -> u32 {
    tiles
        .iter()
        .fold(0u32, |sum, tile| {
            sum.saturating_add(ground.scrap.get(tile).copied().unwrap_or(0))
        })
        .div_ceil(SCRAP_NODE_AMOUNT)
        .min(8)
}
