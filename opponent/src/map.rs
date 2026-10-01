//! Immutable map knowledge built once per match from the scenario's public
//! map. Terrain and the authored starts never change, so nothing here depends
//! on what any seat has seen, and every seat of a match shares one model.

use crate::frame::{HomeFrame, doubled, footprint_centre, gap};
use chassis::grid::{Grid, TilePos};
use oxide_sim::map::Map;
use oxide_sim::scenario::ScenarioError;
use oxide_sim::{BuildingKind, PlayerId, Scenario};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Empty tiles between a home spot and the start Foundry.
const SPOT_GAPS: std::ops::RangeInclusive<i32> = 2..=5;

/// Chebyshev reach from a start that counts as its home scrap.
const HOME_REACH: i32 = 12;

/// A field closer than this to any start belongs to that home, not to an
/// expansion.
const SITE_CLEARANCE: i32 = 8;

/// Expansion sites kept per map.
const SITE_CAP: usize = 64;

/// Foundry anchors kept per site.
const SITE_ANCHORS: usize = 4;

/// Empty tiles between a site anchor and its nearest node.
const SITE_GAPS: std::ops::RangeInclusive<i32> = 1..=3;

/// Chebyshev reach from a site's best anchor that counts its frames.
const FRAME_REACH: i32 = 8;

/// Distance to a tile no ground route reaches.
pub(crate) const UNREACHABLE: u16 = u16::MAX;

/// Public map facts one match's `oxide-opponent` seats share.
#[derive(Debug)]
pub struct MapModel {
    /// Ground component per tile, numbered from one; zero where terrain blocks
    /// ground units.
    components: Grid<u32>,
    /// What terrain does to a shot crossing each tile.
    cover: Grid<Cover>,
    /// Each seat's authored Foundry anchor, by player index.
    starts: Vec<Option<TilePos>>,
    /// Each seat's home building spots, by player index.
    spots: Vec<Vec<TilePos>>,
    /// Each seat's starting scrap nodes near its start, with their amounts.
    home_nodes: Vec<Vec<(TilePos, u32)>>,
    /// Ground distance from each seat's start, in tenths of a tile.
    distances: Vec<Option<Grid<u16>>>,
    /// Each seat's team; `None` is a team of one.
    teams: Vec<Option<u8>>,
    /// Starting scrap fields away from every start.
    sites: Vec<Site>,
}

/// What terrain does to a shot crossing a tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cover {
    /// Every shot crosses.
    Open,
    /// Stops direct fire between two ground units.
    Partial,
    /// Stops every shot.
    Full,
}

/// A field of starting scrap away from every start, where a Foundry could
/// expand.
#[derive(Debug)]
pub(crate) struct Site {
    /// The field's nodes with their starting amounts.
    pub(crate) nodes: Vec<(TilePos, u32)>,
    /// Foundry anchors beside the field, nearest its middle first.
    pub(crate) anchors: Vec<TilePos>,
    /// Extractor frames near the best anchor.
    pub(crate) frames: Vec<TilePos>,
}

impl MapModel {
    /// Builds the model from the scenario's authored map.
    pub fn from_scenario(scenario: &Scenario) -> Result<Self, ScenarioError> {
        let (map, anchors) = scenario.parse_map_and_anchors()?;
        let teams = scenario.players.iter().map(|player| player.team).collect();
        Ok(Self::new(&map, &anchors, teams))
    }

    fn new(map: &Map, anchors: &[(PlayerId, TilePos)], teams: Vec<Option<u8>>) -> Self {
        let seats = teams.len();
        let components = components(map);
        let mut cover = Grid::new(map.width(), map.height(), Cover::Open);
        for (tile, cell) in map.iter() {
            if let Some(slot) = cover.get_mut(tile) {
                *slot = if cell.terrain.blocks_all_fire() {
                    Cover::Full
                } else if cell.terrain.blocks_direct_fire() {
                    Cover::Partial
                } else {
                    Cover::Open
                };
            }
        }
        let mut starts = vec![None; seats];
        let mut spots = vec![Vec::new(); seats];
        let mut home_nodes = vec![Vec::new(); seats];
        let mut distances = vec![None; seats];
        for (player, anchor) in anchors {
            let seat = usize::from(player.0);
            starts[seat] = Some(*anchor);
            spots[seat] = home_spots(map, &components, *anchor);
            home_nodes[seat] = map
                .iter()
                .filter(|(tile, cell)| {
                    cell.scrap > 0 && gap(*anchor, (2, 2), *tile, (1, 1)) < HOME_REACH
                })
                .map(|(tile, cell)| (tile, cell.scrap))
                .collect();
            distances[seat] = Some(distance_field(&components, *anchor));
        }
        let sites = sites(map, &components, anchors);
        Self {
            components,
            cover,
            starts,
            spots,
            home_nodes,
            distances,
            teams,
            sites,
        }
    }

    /// The map's width and height in tiles.
    pub(crate) fn size(&self) -> (i32, i32) {
        (self.components.width(), self.components.height())
    }

    /// Expansion sites, in a fixed order whose index names a site.
    pub(crate) fn sites(&self) -> &[Site] {
        &self.sites
    }

    /// Ground distance from the seat's start to `tile`, in tenths of a tile,
    /// or [`UNREACHABLE`].
    pub(crate) fn distance(&self, player: PlayerId, tile: TilePos) -> u16 {
        self.distances
            .get(usize::from(player.0))
            .and_then(Option::as_ref)
            .and_then(|field| field.get(tile).copied())
            .unwrap_or(UNREACHABLE)
    }

    /// The nearest distance from any hostile seat's start to `tile`.
    pub(crate) fn hostile_distance(&self, player: PlayerId, tile: TilePos) -> u16 {
        self.hostiles(player)
            .map(|other| self.distance(other, tile))
            .min()
            .unwrap_or(UNREACHABLE)
    }

    /// Every seat hostile to `player`, in seat order.
    pub(crate) fn hostiles(&self, player: PlayerId) -> impl Iterator<Item = PlayerId> + '_ {
        (0..self.teams.len())
            .map(|seat| PlayerId(seat as u8))
            .filter(move |other| self.hostile(player, *other))
    }

    fn hostile(&self, a: PlayerId, b: PlayerId) -> bool {
        let team = |player: PlayerId| self.teams.get(usize::from(player.0)).copied().flatten();
        a != b && (team(a).is_none() || team(a) != team(b))
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

    /// The ground component holding `tile`, or `None` where terrain blocks
    /// ground units. Scrap nodes and building footprints keep their ground's
    /// component.
    pub(crate) fn component(&self, tile: TilePos) -> Option<u32> {
        self.components
            .get(tile)
            .copied()
            .filter(|component| *component != 0)
    }

    /// Whether a shot may cross `tile`: peaks stop every shot, and rock also
    /// stops `direct` fire between two ground units.
    pub(crate) fn shot_crosses(&self, tile: TilePos, direct: bool) -> bool {
        match self.cover.get(tile) {
            Some(Cover::Open) => true,
            Some(Cover::Partial) => !direct,
            Some(Cover::Full) | None => false,
        }
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

/// Ground distance from a start Foundry's footprint over open ground, with
/// diagonal steps that never cut a corner. Integer costs keep mirrored starts'
/// fields mirrored exactly.
fn distance_field(components: &Grid<u32>, start: TilePos) -> Grid<u16> {
    let (width, height) = (components.width(), components.height());
    let mut field = Grid::new(width, height, UNREACHABLE);
    let mut queue = BinaryHeap::new();
    let open = |tile: TilePos| components.get(tile).is_some_and(|label| *label != 0);
    for tile in (0..2).flat_map(|dy| (0..2).map(move |dx| start.offset(dx, dy))) {
        if open(tile)
            && let Some(cell) = field.get_mut(tile)
        {
            *cell = 0;
            queue.push(Reverse((0_u32, tile.y, tile.x)));
        }
    }
    while let Some(Reverse((cost, y, x))) = queue.pop() {
        let tile = TilePos::new(x, y);
        if field
            .get(tile)
            .is_some_and(|known| u32::from(*known) < cost)
        {
            continue;
        }
        for (dx, dy) in [
            (1, 0),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (1, -1),
            (-1, 1),
            (-1, -1),
        ] {
            let next = tile.offset(dx, dy);
            let diagonal = dx != 0 && dy != 0;
            if !open(next) || (diagonal && !(open(tile.offset(dx, 0)) && open(tile.offset(0, dy))))
            {
                continue;
            }
            let step = if diagonal { 14 } else { 10 };
            let reached = (cost + step).min(u32::from(UNREACHABLE) - 1);
            if let Some(cell) = field.get_mut(next)
                && reached < u32::from(*cell)
            {
                *cell = reached as u16;
                queue.push(Reverse((reached, next.y, next.x)));
            }
        }
    }
    field
}

/// Expansion sites: 8-connected fields of starting scrap more than
/// [`SITE_CLEARANCE`] tiles from every start, each with Foundry anchors a tile
/// or three from its nodes. Anchors rank by distance to the field's middle,
/// with ties broken relative to the map centre so mirrored fields rank
/// mirrored anchors alike.
fn sites(map: &Map, components: &Grid<u32>, anchors: &[(PlayerId, TilePos)]) -> Vec<Site> {
    let mut seen = Grid::new(map.width(), map.height(), false);
    let mut sites = Vec::new();
    for (seed, cell) in map.iter() {
        if cell.scrap == 0 || seen.get(seed) == Some(&true) {
            continue;
        }
        let mut nodes = Vec::new();
        let mut stack = vec![seed];
        if let Some(mark) = seen.get_mut(seed) {
            *mark = true;
        }
        while let Some(tile) = stack.pop() {
            nodes.push((tile, map.scrap_at(tile)));
            for (dx, dy) in [
                (1, 0),
                (-1, 0),
                (0, 1),
                (0, -1),
                (1, 1),
                (1, -1),
                (-1, 1),
                (-1, -1),
            ] {
                let next = tile.offset(dx, dy);
                if map.scrap_at(next) > 0
                    && let Some(mark) = seen.get_mut(next).filter(|mark| !**mark)
                {
                    *mark = true;
                    stack.push(next);
                }
            }
        }
        nodes.sort_by_key(|(tile, _)| (tile.y, tile.x));
        let near_start = anchors.iter().any(|(_, start)| {
            nodes
                .iter()
                .any(|(node, _)| gap(*start, (2, 2), *node, (1, 1)) < SITE_CLEARANCE)
        });
        if near_start {
            continue;
        }
        let anchors = site_anchors(map, components, &nodes);
        let Some(best) = anchors.first().copied() else {
            continue;
        };
        let frames = map
            .extractor_frames()
            .iter()
            .copied()
            .filter(|frame| gap(best, (2, 2), *frame, (2, 2)) < FRAME_REACH)
            .collect();
        sites.push(Site {
            nodes,
            anchors,
            frames,
        });
        if sites.len() == SITE_CAP {
            break;
        }
    }
    sites
}

fn site_anchors(map: &Map, components: &Grid<u32>, nodes: &[(TilePos, u32)]) -> Vec<TilePos> {
    let count = nodes.len() as i64;
    let sum = nodes.iter().fold((0, 0), |sum, (node, _)| {
        let (x, y) = doubled(*node);
        (sum.0 + x, sum.1 + y)
    });
    let frame = HomeFrame::around(
        sum,
        (
            sum.0 - count * i64::from(map.width()),
            sum.1 - count * i64::from(map.height()),
        ),
    );
    let (min_x, max_x, min_y, max_y) = nodes.iter().fold(
        (i32::MAX, i32::MIN, i32::MAX, i32::MIN),
        |(min_x, max_x, min_y, max_y), (node, _)| {
            (
                min_x.min(node.x),
                max_x.max(node.x),
                min_y.min(node.y),
                max_y.max(node.y),
            )
        },
    );
    let reach = SITE_GAPS.end() + 2;
    let mut anchors = Vec::new();
    for y in min_y - reach..=max_y + reach {
        for x in min_x - reach..=max_x + reach {
            let anchor = TilePos::new(x, y);
            let footprint: Vec<TilePos> = (0..2)
                .flat_map(|dy| (0..2).map(move |dx| anchor.offset(dx, dy)))
                .collect();
            let Some(component) = components.get(anchor).copied().filter(|label| *label != 0)
            else {
                continue;
            };
            let open = footprint.iter().all(|tile| {
                components.get(*tile) == Some(&component) && !map.tile_in_extractor_frame(*tile)
            });
            let clear =
                (-1..=2).all(|dy| (-1..=2).all(|dx| map.scrap_at(anchor.offset(dx, dy)) == 0));
            let gap = nodes
                .iter()
                .map(|(node, _)| {
                    let dx = (node.x - anchor.x - 1).max(anchor.x - node.x) - 1;
                    let dy = (node.y - anchor.y - 1).max(anchor.y - node.y) - 1;
                    dx.max(dy)
                })
                .min()
                .unwrap_or(i32::MAX);
            if open && clear && SITE_GAPS.contains(&gap) {
                anchors.push(anchor);
            }
        }
    }
    anchors.sort_by_key(|anchor| {
        let centre = footprint_centre(BuildingKind::Foundry, *anchor);
        frame.rank(sum, (centre.0 * count, centre.1 * count))
    });
    anchors.truncate(SITE_ANCHORS);
    anchors
}

/// Labels 4-connected ground. Diagonal steps never cut corners, so this is
/// the connectivity of ground movement. Scrap counts as ground: mining it out
/// opens the way, and a harvest order finds the reachable nodes of a field.
fn components(map: &Map) -> Grid<u32> {
    let ground = |tile: TilePos| {
        map.tile(tile)
            .is_some_and(|cell| !cell.terrain.blocks_ground())
    };
    let mut labels = Grid::new(map.width(), map.height(), 0_u32);
    let mut next = 0;
    let mut stack = Vec::new();
    for y in 0..map.height() {
        for x in 0..map.width() {
            let seed = TilePos::new(x, y);
            if labels.get(seed) != Some(&0) || !ground(seed) {
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
                    if !ground(neighbour) {
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
        MapModel::new(&map, &anchors, vec![None; 2])
    }

    #[test]
    fn components_split_at_walls_and_scrap_touches_its_side() {
        let model = model(&SPLIT);
        let west = model.component(TilePos::new(1, 4)).unwrap();
        let east = model.component(TilePos::new(8, 4)).unwrap();
        assert_ne!(west, east);
        assert_eq!(model.component(TilePos::new(4, 3)), None, "rock");
        assert_eq!(model.component(TilePos::new(2, 4)), Some(west), "scrap");
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
    fn a_scrap_choke_joins_what_it_will_open_once_mined() {
        const CHOKE: [&str; 5] = [
            "##########",
            "#1..#....#",
            "#...s..2.#",
            "#...#....#",
            "##########",
        ];
        let model = model(&CHOKE);
        assert_eq!(
            model.component(TilePos::new(1, 3)),
            model.component(TilePos::new(6, 1))
        );
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
