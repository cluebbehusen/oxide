//! Immutable map knowledge built once per match from the scenario's public
//! map. Terrain and the authored starts never change, so nothing here depends
//! on what any seat has seen, and every seat of a match shares one model.

use crate::frame::{HomeFrame, footprint_centre};
use chassis::grid::{Grid, TilePos};
use oxide_sim::map::Map;
use oxide_sim::scenario::ScenarioError;
use oxide_sim::{BuildingKind, PlayerId, Scenario};

/// Empty tiles between a home spot and the start Foundry.
const SPOT_GAPS: std::ops::RangeInclusive<i32> = 2..=5;

/// Chebyshev reach from a start that counts as its home scrap.
const HOME_REACH: i32 = 12;

/// Public map facts one match's `oxide-opponent` seats share.
#[derive(Debug)]
pub struct MapModel {
    /// Ground component per tile, numbered from one; zero where terrain or a
    /// starting scrap node blocks ground units.
    components: Grid<u32>,
    /// Each seat's authored Foundry anchor, by player index.
    starts: Vec<Option<TilePos>>,
    /// Each seat's home building spots, by player index.
    spots: Vec<Vec<TilePos>>,
    /// Each seat's starting scrap nodes near its start, with their amounts.
    home_nodes: Vec<Vec<(TilePos, u32)>>,
}

impl MapModel {
    /// Builds the model from the scenario's authored map.
    pub fn from_scenario(scenario: &Scenario) -> Result<Self, ScenarioError> {
        let (map, anchors) = scenario.parse_map_and_anchors()?;
        Ok(Self::new(&map, &anchors, scenario.players.len()))
    }

    fn new(map: &Map, anchors: &[(PlayerId, TilePos)], seats: usize) -> Self {
        let components = components(map);
        let mut starts = vec![None; seats];
        let mut spots = vec![Vec::new(); seats];
        let mut home_nodes = vec![Vec::new(); seats];
        for (player, anchor) in anchors {
            let seat = usize::from(player.0);
            starts[seat] = Some(*anchor);
            spots[seat] = home_spots(map, &components, *anchor);
            home_nodes[seat] = map
                .iter()
                .filter(|(tile, cell)| cell.scrap > 0 && tile.chebyshev(*anchor) <= HOME_REACH)
                .map(|(tile, cell)| (tile, cell.scrap))
                .collect();
        }
        Self {
            components,
            starts,
            spots,
            home_nodes,
        }
    }

    /// The seat's authored Foundry anchor, if the scenario places one.
    pub(crate) fn start(&self, player: PlayerId) -> Option<TilePos> {
        self.starts.get(usize::from(player.0)).copied().flatten()
    }

    /// Anchors for the seat's home buildings, nearest its start first. Each
    /// is a two-by-two footprint of open ground in the start's component, off
    /// every frame and a tile clear of starting scrap; smaller buildings use
    /// its top-left corner.
    pub(crate) fn spots(&self, player: PlayerId) -> &[TilePos] {
        self.spots
            .get(usize::from(player.0))
            .map_or(&[], Vec::as_slice)
    }

    /// The seat's starting scrap nodes near its start, with their starting
    /// amounts.
    pub(crate) fn home_nodes(&self, player: PlayerId) -> &[(TilePos, u32)] {
        self.home_nodes
            .get(usize::from(player.0))
            .map_or(&[], Vec::as_slice)
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

/// Home spots around `start`, ordered by their gap to the start Foundry and
/// then in the seat's frame, so mirrored seats list mirrored spots.
fn home_spots(map: &Map, components: &Grid<u32>, start: TilePos) -> Vec<TilePos> {
    let Some(home) = components.get(start).copied().filter(|label| *label != 0) else {
        return Vec::new();
    };
    let frame = HomeFrame::at(start, map.width(), map.height());
    let reach = SPOT_GAPS.end() + 2;
    let mut spots: Vec<(i32, TilePos)> = Vec::new();
    for y in start.y - reach..=start.y + reach {
        for x in start.x - reach..=start.x + reach {
            let anchor = TilePos::new(x, y);
            let gap = (anchor.x - start.x).abs().max((anchor.y - start.y).abs()) - 2;
            let footprint = (0..2).flat_map(|dy| (0..2).map(move |dx| anchor.offset(dx, dy)));
            let open = footprint.clone().all(|tile| {
                components.get(tile) == Some(&home) && !map.tile_in_extractor_frame(tile)
            });
            let clear =
                (-1..=2).all(|dy| (-1..=2).all(|dx| map.scrap_at(anchor.offset(dx, dy)) == 0));
            if SPOT_GAPS.contains(&gap) && open && clear {
                spots.push((gap, anchor));
            }
        }
    }
    spots.sort_by_key(|(gap, anchor)| {
        let centre = footprint_centre(BuildingKind::Foundry, *anchor);
        (*gap, frame.rank(frame.home, centre))
    });
    spots.into_iter().map(|(_, anchor)| anchor).collect()
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
        MapModel::new(&map, &anchors, 2)
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
    fn home_spots_leave_room_around_the_start_and_mirror_between_seats() {
        const ROOM: [&str; 16] = [
            "####################",
            "#..................#",
            "#..................#",
            "#..s...............#",
            "#..................#",
            "#....1.............#",
            "#..................#",
            "#..................#",
            "#..................#",
            "#............2.....#",
            "#..................#",
            "#..................#",
            "#...............s..#",
            "#..................#",
            "#..................#",
            "####################",
        ];
        let model = model(&ROOM);
        let west = model.spots(PlayerId(0));
        let east = model.spots(PlayerId(1));
        assert!(!west.is_empty());
        let start = model.start(PlayerId(0)).unwrap();
        for spot in west {
            let gap = (spot.x - start.x).abs().max((spot.y - start.y).abs()) - 2;
            assert!(SPOT_GAPS.contains(&gap), "{spot:?}");
            let crowds =
                (spot.x - 1..=spot.x + 2).contains(&3) && (spot.y - 1..=spot.y + 2).contains(&3);
            assert!(!crowds, "{spot:?} crowds the scrap");
        }
        let rotate = |anchor: TilePos| TilePos::new(20 - 2 - anchor.x, 16 - 2 - anchor.y);
        assert_eq!(west.iter().copied().map(rotate).collect::<Vec<_>>(), east);
        assert_eq!(model.spots(PlayerId(2)), &[] as &[TilePos]);
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
                assert!(
                    model.spots(PlayerId(seat as u8)).len() >= 8,
                    "{} seat {seat}",
                    path.display()
                );
            }
        }
    }
}
