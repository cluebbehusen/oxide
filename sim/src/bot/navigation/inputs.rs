//! Derived route inputs owned by an observation, shared by independent queries.

use super::super::{PublicMapBriefing, observation::Observation, orient::Orientation};
#[cfg(test)]
use crate::bot::observation::ObservationData;
use crate::bot::query_work::{QueryOperation, QueryPurpose};
use crate::{map::Terrain, stats::Domain};
use chassis::grid::TilePos;
use std::sync::{Arc, OnceLock};

#[derive(Default)]
pub(in crate::bot) struct NavigationInputs {
    ordinary: [OnceLock<Arc<Surface>>; 2],
    public: OnceLock<PublicSurfaces>,
}

struct PublicSurfaces {
    dimensions: (i32, i32),
    terrain: Vec<(TilePos, Terrain)>,
    domains: [OnceLock<Arc<Surface>>; 2],
}

pub(in crate::bot) struct Surface {
    pub open: Vec<bool>,
    labels: [OnceLock<Arc<[u32]>>; 2],
    commands: [OnceLock<CommandSurface>; 2],
}

struct CommandSurface {
    orientation: Option<Orientation>,
    blocked: Arc<[bool]>,
}

impl Surface {
    fn new(open: Vec<bool>) -> Self {
        Self {
            open,
            labels: Default::default(),
            commands: Default::default(),
        }
    }

    pub fn labels(&self, purpose: QueryPurpose, obs: &Observation, explored: bool) -> Arc<[u32]> {
        Arc::clone(self.labels[usize::from(explored)].get_or_init(|| {
            let open = self
                .open
                .iter()
                .enumerate()
                .map(|(index, &open)| {
                    open && (!explored
                        || obs.explored(TilePos::new(
                            index as i32 % obs.map_width,
                            index as i32 / obs.map_width,
                        )))
                })
                .collect();
            super::components::labels(purpose, (obs.map_width, obs.map_height), open)
        }))
    }

    pub fn command_surface(
        &self,
        obs: &Observation,
        explored: bool,
        orientation: Option<Orientation>,
    ) -> Arc<[bool]> {
        let build = || CommandSurface {
            orientation,
            blocked: (0..obs.map_height)
                .flat_map(|y| (0..obs.map_width).map(move |x| TilePos::new(x, y)))
                .map(|tile| {
                    let tile = orientation.map_or(tile, |orientation| orientation.tile(tile));
                    super::flood::tile_index(obs.map_width, obs.map_height, tile)
                        .is_none_or(|index| !self.open[index] || (explored && !obs.explored(tile)))
                })
                .collect(),
        };
        let cached = self.commands[usize::from(explored)].get_or_init(build);
        if cached.orientation == orientation {
            Arc::clone(&cached.blocked)
        } else {
            build().blocked
        }
    }
}

impl NavigationInputs {
    pub fn surface(
        &self,
        purpose: QueryPurpose,
        obs: &Observation,
        domain: Domain,
        map: Option<&PublicMapBriefing>,
    ) -> Arc<Surface> {
        let index = usize::from(domain == Domain::Air);
        let ordinary = self.ordinary[index]
            .get_or_init(|| Arc::new(Surface::new(prepare(purpose, obs, domain))));
        let Some(map) = map else {
            return Arc::clone(ordinary);
        };
        let public = self.public.get_or_init(|| PublicSurfaces {
            dimensions: (map.map_width, map.map_height),
            terrain: map.non_ground_terrain.clone(),
            domains: Default::default(),
        });
        let build = || {
            crate::bot::query_work::record(
                purpose,
                QueryOperation::PrepareSurface,
                ordinary.open.len(),
            );
            let mut open = ordinary.open.clone();
            if map.map_width < obs.map_width || map.map_height < obs.map_height {
                for y in 0..obs.map_height {
                    for x in 0..obs.map_width {
                        if x >= map.map_width || y >= map.map_height {
                            open[(y * obs.map_width + x) as usize] = false;
                        }
                    }
                }
            }
            for &(tile, terrain) in &map.non_ground_terrain {
                let blocked = match domain {
                    Domain::Ground => terrain.blocks_ground(),
                    Domain::Air => terrain.blocks_air(),
                };
                if blocked
                    && let Some(index) =
                        super::flood::tile_index(obs.map_width, obs.map_height, tile)
                {
                    open[index] = false;
                }
            }
            Arc::new(Surface::new(open))
        };
        if public.dimensions == (map.map_width, map.map_height)
            && public.terrain == map.non_ground_terrain
        {
            Arc::clone(public.domains[index].get_or_init(build))
        } else {
            // Alternate hypothetical briefings cannot replace the decision's retained inputs.
            build()
        }
    }
}

fn prepare(purpose: QueryPurpose, obs: &Observation, domain: Domain) -> Vec<bool> {
    let cells = usize::try_from(obs.map_width)
        .ok()
        .and_then(|width| {
            usize::try_from(obs.map_height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .unwrap_or(0);
    crate::bot::query_work::record(purpose, QueryOperation::PrepareSurface, cells);
    let mut open = vec![true; cells];
    let mut block = |tile| {
        if let Some(index) = super::flood::tile_index(obs.map_width, obs.map_height, tile) {
            open[index] = false;
        }
    };
    match domain {
        Domain::Ground => {
            for tile in obs
                .known_rock
                .iter()
                .copied()
                .chain(obs.known_scrap.iter().map(|(tile, _)| *tile))
            {
                block(tile);
            }
            for building in obs
                .my_buildings
                .iter()
                .chain(&obs.ally_buildings)
                .chain(&obs.enemy_buildings)
                .filter(|building| !building.provisional && !building.kind.is_stealthy())
            {
                let (width, height) = building.kind.base_stats().size;
                for dy in 0..height {
                    for dx in 0..width {
                        block(building.anchor.offset(dx, dy));
                    }
                }
            }
        }
        Domain::Air => {
            for &tile in &obs.known_peaks {
                block(tile);
            }
        }
    }
    open
}

#[cfg(test)]
mod tests {
    use super::*;
    use ObservationData;

    fn observation() -> Observation {
        Observation::from_data(ObservationData {
            map_width: 5,
            map_height: 4,
            explored: vec![true; 20],
            known_rock: vec![TilePos::new(2, 1)],
            known_peaks: vec![TilePos::new(2, 1)],
            ..Default::default()
        })
    }

    fn surface(obs: &Observation, domain: Domain, map: Option<&PublicMapBriefing>) -> Arc<Surface> {
        obs.navigation()
            .surface(QueryPurpose::NavigationTest, obs, domain, map)
    }

    #[test]
    fn clones_share_inputs_until_mutation_and_serialization_carries_only_knowledge() {
        let mut obs = observation();
        let encoded = serde_json::to_vec(&obs).unwrap();
        let ground = surface(&obs, Domain::Ground, None);
        let air = surface(&obs, Domain::Air, None);
        let clone = obs.clone();
        assert!(Arc::ptr_eq(&ground, &surface(&clone, Domain::Ground, None)));
        assert!(Arc::ptr_eq(&air, &surface(&clone, Domain::Air, None)));
        assert_eq!(encoded, serde_json::to_vec(&obs).unwrap());
        let cold: Observation = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(obs, cold);
        assert!(!Arc::ptr_eq(&ground, &surface(&cold, Domain::Ground, None)));

        // Equal ticks do not identify equal knowledge or hypothetical occupancy.
        obs.known_rock.clear();
        obs.known_peaks.clear();
        obs.explored[0] = false;
        let changed = surface(&obs, Domain::Ground, None);
        assert!(changed.open[7]);
        assert!(!ground.open[7]);
        assert!(surface(&obs, Domain::Air, None).open[7]);
        assert_eq!(
            changed.labels(QueryPurpose::NavigationTest, &obs, true)[0],
            0
        );
        assert_ne!(
            ground.labels(QueryPurpose::NavigationTest, &clone, true)[0],
            0
        );
        assert!(Arc::ptr_eq(&ground, &surface(&clone, Domain::Ground, None)));
    }

    #[test]
    fn public_terrain_and_command_frames_cannot_alias() {
        let obs = observation();
        let mut map = PublicMapBriefing {
            regions: Default::default(),
            map_width: 5,
            map_height: 4,
            starting_foundries: Vec::new(),
            teams: Vec::new(),
            non_ground_terrain: vec![(TilePos::new(1, 0), Terrain::Pit)],
            extractor_frames: Vec::new(),
            initial_scrap: Vec::new(),
        };
        let public = surface(&obs, Domain::Ground, Some(&map));
        assert!(!public.open[1]);
        assert!(surface(&obs, Domain::Ground, None).open[1]);
        assert!(surface(&obs, Domain::Air, Some(&map)).open[1]);
        assert!(Arc::ptr_eq(
            &public,
            &surface(&obs, Domain::Ground, Some(&map))
        ));
        map.non_ground_terrain.clear();
        assert!(surface(&obs, Domain::Ground, Some(&map)).open[1]);
        map.non_ground_terrain
            .push((TilePos::new(1, 0), Terrain::Pit));
        assert!(Arc::ptr_eq(
            &public,
            &surface(&obs, Domain::Ground, Some(&map))
        ));

        let flip = Orientation::for_home(&obs, TilePos::new(4, 3));
        let raw = public.command_surface(&obs, false, None);
        let flipped = public.command_surface(&obs, false, Some(flip));
        assert!(raw[1]);
        assert!(!flipped[1]);
        assert!(flipped[18]);
        assert!(Arc::ptr_eq(
            &raw,
            &public.command_surface(&obs, false, None)
        ));
        let oriented = flip.observe(&obs);
        assert!(!surface(&oriented, Domain::Ground, None).open[12]);
        assert!(surface(&obs, Domain::Ground, None).open[12]);
    }

    #[test]
    fn workers_share_preparation_and_keep_search_results_ordered() {
        use super::super::commands::RouteProjection;
        let obs = observation();
        let goals: Vec<_> = (0..4)
            .flat_map(|y| (0..5).map(move |x| TilePos::new(x, y)))
            .collect();
        let query = |goal| {
            let routes = RouteProjection::new(QueryPurpose::NavigationTest, &obs, Domain::Ground);
            routes.command_route(TilePos::new(0, 0), goal)
        };
        let expected: Vec<_> = goals.iter().copied().map(query).collect();
        let actual = std::thread::scope(|scope| {
            let workers: Vec<_> = goals
                .chunks(5)
                .map(|chunk| scope.spawn(|| chunk.iter().copied().map(query).collect::<Vec<_>>()))
                .collect();
            workers
                .into_iter()
                .flat_map(|worker| worker.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(expected, actual);
    }
}
