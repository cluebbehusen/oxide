use super::{
    DefenseDomain, GroundKnowledge, PATH_EXPANSION_CAP, PlacementFootprint, octile_cost, tile_index,
};
use chassis::{grid::TilePos, path::AstarScratch};
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BinaryHeap, VecDeque},
    mem::size_of,
};

const MIB: usize = 1024 * 1024;
const ENTRY_ALLOWANCE: usize = 1024;

type PathKey = (Option<PlacementFootprint>, TilePos, TilePos);

/// Bot-owned derived routes. Hypothetical layouts cannot evict normal boards;
/// ground-only edits also leave air routes intact.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::bot::utility) struct DefenseRoutingCache {
    ground: Vec<Generation>,
    air: Vec<Generation>,
    hypothetical: Vec<Generation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Generation {
    width: i32,
    height: i32,
    blocked: Box<[bool]>,
    paths: BTreeMap<PathKey, Box<[TilePos]>>,
    path_order: VecDeque<PathKey>,
    distances: BTreeMap<TilePos, Box<[u32]>>,
    distance_order: VecDeque<TilePos>,
    path_bytes: usize,
    distance_bytes: usize,
    budget: usize,
}

impl Generation {
    fn payload_budget(&self) -> usize {
        self.budget.saturating_sub(self.blocked.len()) / 2
    }

    fn reserve_path(&mut self, bytes: usize) -> bool {
        let budget = self.payload_budget();
        if bytes > budget {
            return false;
        }
        while self.path_bytes.saturating_add(bytes) > budget {
            let key = self.path_order.pop_front().expect("retained route budget");
            let path = self.paths.remove(&key).expect("retained route key");
            self.path_bytes -= path.len() * size_of::<TilePos>() + ENTRY_ALLOWANCE;
        }
        self.path_bytes += bytes;
        true
    }

    fn reserve_distance(&mut self, bytes: usize) -> bool {
        let budget = self.payload_budget();
        if bytes > budget {
            return false;
        }
        // Full-map distance fields must not evict the endpoint routes they
        // help prune. Each payload class owns half the retained budget.
        while self.distance_bytes.saturating_add(bytes) > budget {
            let goal = self
                .distance_order
                .pop_front()
                .expect("retained distance budget");
            let field = self
                .distances
                .remove(&goal)
                .expect("retained distance goal");
            self.distance_bytes -= field.len() * size_of::<u32>() + ENTRY_ALLOWANCE;
        }
        self.distance_bytes += bytes;
        true
    }
}

impl DefenseRoutingCache {
    fn generation(
        &mut self,
        ground: &GroundKnowledge<'_>,
        domain: DefenseDomain,
    ) -> Option<&mut Generation> {
        let (entries, blocked, budget, limit) = match domain {
            DefenseDomain::Air => (&mut self.air, &ground.air_blocked, 8 * MIB, 1),
            DefenseDomain::Ground if ground.hypothetical => {
                (&mut self.hypothetical, &ground.ground_blocked, MIB, 2)
            }
            DefenseDomain::Ground => (&mut self.ground, &ground.ground_blocked, 8 * MIB, 1),
        };
        if blocked.len() > budget {
            return None;
        }
        if let Some(index) = entries.iter().position(|entry| {
            entry.width == ground.obs.map_width
                && entry.height == ground.obs.map_height
                && entry.blocked.as_ref() == blocked.as_slice()
        }) {
            let entry = entries.remove(index);
            entries.push(entry);
        } else {
            if entries.len() == limit {
                entries.remove(0);
            }
            entries.push(Generation {
                width: ground.obs.map_width,
                height: ground.obs.map_height,
                blocked: blocked.clone().into_boxed_slice(),
                paths: BTreeMap::new(),
                path_order: VecDeque::new(),
                distances: BTreeMap::new(),
                distance_order: VecDeque::new(),
                path_bytes: 0,
                distance_bytes: 0,
                budget,
            });
        }
        entries.last_mut()
    }
}

pub(super) fn path(
    ground: &GroundKnowledge<'_>,
    start: TilePos,
    goal: TilePos,
    candidate: Option<PlacementFootprint>,
    domain: DefenseDomain,
    scratch: &mut AstarScratch,
) -> Option<Vec<TilePos>> {
    let key = (
        if domain == DefenseDomain::Air {
            None
        } else {
            candidate
        },
        start,
        goal,
    );
    if let Some(path) = ground
        .routing()
        .borrow_mut()
        .generation(ground, domain)
        .and_then(|generation| generation.paths.get(&key))
    {
        scratch.clear_search_evidence();
        return Some(path.to_vec());
    }
    let result = chassis::path::astar_with_scratch(
        ground.obs.map_width,
        ground.obs.map_height,
        start,
        goal,
        |tile| ground.open(tile, candidate, domain),
        PATH_EXPANSION_CAP,
        scratch,
    );
    // A failure may carry an exhausted component or only a capped search.
    // Retain neither as a bare absence without that query's scratch evidence.
    if let Some(path) = &result
        && let Some(generation) = ground.routing().borrow_mut().generation(ground, domain)
        && generation.reserve_path(path.len() * size_of::<TilePos>() + ENTRY_ALLOWANCE)
    {
        generation
            .paths
            .insert(key, path.clone().into_boxed_slice());
        generation.path_order.push_back(key);
    }
    result
}

pub(super) fn bound(
    ground: &GroundKnowledge<'_>,
    start: TilePos,
    goal: TilePos,
    domain: DefenseDomain,
) -> u32 {
    let fallback = octile_cost(start, goal);
    // A* permits leaving a blocked start. Such queries do not belong to the
    // undirected open-tile graph represented by a reverse distance field.
    let Some(index) = tile_index(ground.obs.map_width, ground.obs.map_height, start) else {
        return fallback;
    };
    if !ground.open(start, None, domain) {
        return fallback;
    }
    let mut cache = ground.routing().borrow_mut();
    let Some(generation) = cache.generation(ground, domain) else {
        return fallback;
    };
    if let Some(field) = generation.distances.get(&goal) {
        return field[index];
    }
    if !generation.reserve_distance(generation.blocked.len() * size_of::<u32>() + ENTRY_ALLOWANCE) {
        return fallback;
    }
    let field = distance_field(ground.obs.map_width, ground.obs.map_height, goal, |tile| {
        ground.open(tile, None, domain)
    });
    let result = field[index];
    generation.distances.insert(goal, field.into_boxed_slice());
    generation.distance_order.push_back(goal);
    result
}

fn distance_field(
    width: i32,
    height: i32,
    goal: TilePos,
    open: impl Fn(TilePos) -> bool,
) -> Vec<u32> {
    let mut distance = vec![u32::MAX; (width as usize) * (height as usize)];
    let Some(index) = tile_index(width, height, goal) else {
        return distance;
    };
    if !open(goal) {
        return distance;
    }
    distance[index] = 0;
    let mut queue = BinaryHeap::from([Reverse((0u32, index))]);
    while let Some(Reverse((cost, index))) = queue.pop() {
        if distance[index] != cost {
            continue;
        }
        let tile = TilePos::new(
            (index % width as usize) as i32,
            (index / width as usize) as i32,
        );
        for (dx, dy) in chassis::grid::CARDINALS
            .into_iter()
            .chain(chassis::grid::DIAGONALS)
        {
            let next = tile.offset(dx, dy);
            let Some(next_index) = tile_index(width, height, next) else {
                continue;
            };
            if !open(next)
                || (dx != 0 && dy != 0 && (!open(tile.offset(dx, 0)) || !open(tile.offset(0, dy))))
            {
                continue;
            }
            let next_cost = cost + if dx == 0 || dy == 0 { 10 } else { 14 };
            if next_cost < distance[next_index] {
                distance[next_index] = next_cost;
                queue.push(Reverse((next_cost, next_index)));
            }
        }
    }
    distance
}

#[cfg(test)]
mod tests;
