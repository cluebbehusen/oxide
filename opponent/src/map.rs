//! Immutable map knowledge built once per match from the scenario's public
//! map. Terrain and the authored starts never change, so nothing here depends
//! on what any seat has seen, and every seat of a match shares one model.

use chassis::grid::{Grid, TilePos};
use oxide_sim::map::Map;
use oxide_sim::scenario::ScenarioError;
use oxide_sim::{PlayerId, Scenario};

/// Public map facts one match's `oxide-opponent` seats share.
#[derive(Debug)]
pub struct MapModel {
    /// Ground component per tile, numbered from one; zero where terrain or a
    /// starting scrap node blocks ground units.
    components: Grid<u32>,
    /// Each seat's authored Foundry anchor, by player index.
    starts: Vec<Option<TilePos>>,
}

impl MapModel {
    /// Builds the model from the scenario's authored map.
    pub fn from_scenario(scenario: &Scenario) -> Result<Self, ScenarioError> {
        let (map, anchors) = scenario.parse_map_and_anchors()?;
        let mut starts = vec![None; scenario.players.len()];
        for (player, anchor) in anchors {
            starts[usize::from(player.0)] = Some(anchor);
        }
        Ok(Self {
            components: components(&map),
            starts,
        })
    }

    /// The seat's authored Foundry anchor, if the scenario places one.
    pub(crate) fn start(&self, player: PlayerId) -> Option<TilePos> {
        self.starts.get(usize::from(player.0)).copied().flatten()
    }

    /// The ground component holding `tile`, or `None` where ground units
    /// cannot stand. Building footprints keep their terrain's component.
    pub(crate) fn component(&self, tile: TilePos) -> Option<u32> {
        self.components
            .get(tile)
            .copied()
            .filter(|component| *component != 0)
    }

    /// Whether a ground unit in `component` can stand beside `tile`, which is
    /// how a Harvester reaches a scrap node.
    pub(crate) fn touches(&self, tile: TilePos, component: u32) -> bool {
        (-1..=1).any(|dy| {
            (-1..=1).any(|dx| {
                (dx, dy) != (0, 0) && self.component(tile.offset(dx, dy)) == Some(component)
            })
        })
    }
}

/// Labels 4-connected open ground. Diagonal steps never cut corners, so this
/// is exactly the connectivity of ground movement. Starting scrap blocks until
/// mined out, which only ever joins components.
fn components(map: &Map) -> Grid<u32> {
    let mut labels = Grid::new(map.width(), map.height(), 0_u32);
    let mut next = 0;
    let mut stack = Vec::new();
    for y in 0..map.height() {
        for x in 0..map.width() {
            let seed = TilePos::new(x, y);
            if labels.get(seed) != Some(&0) || !map.terrain_passable(seed) {
                continue;
            }
            next += 1;
            if let Some(label) = labels.get_mut(seed) {
                *label = next;
            }
            stack.push(seed);
            while let Some(tile) = stack.pop() {
                for step in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let neighbour = tile.offset(step.0, step.1);
                    if !map.terrain_passable(neighbour) {
                        continue;
                    }
                    if let Some(label) = labels.get_mut(neighbour).filter(|label| **label == 0) {
                        *label = next;
                        stack.push(neighbour);
                    }
                }
            }
        }
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPLIT: [&str; 7] = [
        "##########",
        "#1..#....#",
        "#...#..s.#",
        "#...#s...#",
        "#.s.#....#",
        "#...#..2.#",
        "##########",
    ];

    fn model(rows: &[&str]) -> MapModel {
        let (map, anchors) = Map::parse(rows).unwrap();
        let mut starts = vec![None; 2];
        for (player, anchor) in anchors {
            starts[usize::from(player.0)] = Some(anchor);
        }
        MapModel {
            components: components(&map),
            starts,
        }
    }

    #[test]
    fn components_split_at_walls_and_scrap_touches_its_side() {
        let model = model(&SPLIT);
        let west = model.component(TilePos::new(1, 4)).unwrap();
        let east = model.component(TilePos::new(8, 4)).unwrap();
        assert_ne!(west, east);
        assert_eq!(model.component(TilePos::new(4, 3)), None, "rock");
        assert_eq!(model.component(TilePos::new(2, 4)), None, "scrap");
        assert_eq!(
            model.component(model.start(PlayerId(0)).unwrap()),
            Some(west),
            "a Foundry footprint keeps its ground component"
        );
        assert!(model.touches(TilePos::new(2, 4), west));
        assert!(!model.touches(TilePos::new(2, 4), east));
        assert!(
            model.touches(TilePos::new(5, 3), east),
            "diagonal doorsteps count"
        );
        assert!(!model.touches(TilePos::new(5, 3), west), "not through rock");
        assert_eq!(model.start(PlayerId(1)), Some(TilePos::new(7, 5)));
        assert_eq!(model.start(PlayerId(2)), None);
    }

    #[test]
    fn every_shipped_scenario_builds_a_model_with_every_start_on_ground() {
        let scenarios = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../scenarios"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            });
        for path in scenarios {
            let scenario = Scenario::load(&path).unwrap();
            let model = MapModel::from_scenario(&scenario).unwrap();
            for seat in 0..scenario.players.len() {
                let start = model.start(PlayerId(seat as u8)).unwrap();
                assert!(model.component(start).is_some(), "{}", path.display());
            }
        }
    }
}
