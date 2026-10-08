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

/// The layout of a base around each Foundry: blocks four tiles a side, with
/// a lane one tile wide between them every `PERIOD` tiles each way, laid
/// from the Foundry's anchor. The Foundry's own block is the Foundry and its
/// ring, and every other block holds four two-by-two slots, each touching
/// two lanes. A shape, not a count: residues map `r` to `1 - r` under
/// mirroring, so mirrored seats lay mirrored bases.
const PERIOD: i32 = 5;

/// The residue of a lane in the layout.
const LANE: i32 = 3;

/// Empty tiles between a Foundry and the nearest slot outside its block.
const FIRST_GAP: i32 = PERIOD - 3;

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

/// Tiles from a start inside which no cut counts: its own block and the
/// lanes around it.
const CUT_FROM: u16 = 4;

/// Tiles from a start out to which cuts are looked for, and never past half
/// the way to the hostile start: ground a base can hold.
const CUT_REACH: u16 = 24;

/// Tiles across beyond which a passage is open ground, not a gate: a few
/// units abreast. Also bounds each passage's flood fill.
const GATE_WIDTH: usize = 8;

/// Passages beyond which a cut is open ground broken up, not gates to hold.
const CUT_GATES: usize = 3;

/// Tiles a route between two starts may run longer than the shortest and
/// still count as one an attack would take.
const CUT_SLACK: u16 = 20;

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
    /// Anchors of two-by-two footprints of open ground on one component,
    /// off every frame and a tile clear of starting scrap.
    spot: Grid<bool>,
    /// The same for one-tile footprints.
    tile_spot: Grid<bool>,
    /// Each ground component's bounding box, as its lowest and highest
    /// corners, by component.
    extents: Vec<(TilePos, TilePos)>,
    /// Each seat's starting scrap nodes near its start, with their amounts.
    home_nodes: Vec<Vec<(TilePos, u32)>>,
    /// Ground distance from each seat's start, in tenths of a tile.
    distances: Vec<Option<Grid<u16>>>,
    /// Each seat's team; `None` is a team of one.
    teams: Vec<Option<u8>>,
    /// Starting scrap fields away from every start.
    sites: Vec<Site>,
    /// Each seat's cuts, by player index.
    cuts: Vec<Vec<Cut>>,
    /// Each seat's gate tiles, sorted, where it never builds.
    gated: Vec<Vec<TilePos>>,
}

/// A few narrow passages that every ground route from a seat's start to a
/// hostile start runs through, at one distance from the seat's start.
#[derive(Debug)]
pub(crate) struct Cut {
    /// The hostile seat whose way in it lies across.
    pub(crate) hostile: PlayerId,
    /// Ground distance from the seat's start to its near side, in tenths of
    /// a tile.
    pub(crate) distance: u16,
    /// Its passages, in tile order.
    pub(crate) gates: Vec<Gate>,
    /// The ground the seat's start reaches without crossing it, one bit a
    /// tile in row order.
    home: Vec<u64>,
    /// The map's width, to find a tile's bit.
    width: i32,
}

impl Cut {
    /// Whether `tile` lies on the start's side of the cut.
    pub(crate) fn holds(&self, tile: TilePos) -> bool {
        (0..self.width).contains(&tile.x)
            && tile.y >= 0
            && usize::try_from(tile.y * self.width + tile.x).is_ok_and(|bit| {
                self.home
                    .get(bit / 64)
                    .is_some_and(|word| word >> (bit % 64) & 1 == 1)
            })
    }

    /// Tiles across all its passages together.
    pub(crate) fn width(&self) -> usize {
        self.gates.iter().map(Gate::width).sum()
    }
}

/// One passage of a cut: two tiles deep across the way.
#[derive(Debug, Clone)]
pub(crate) struct Gate {
    /// Its tiles, sorted.
    pub(crate) tiles: Vec<TilePos>,
    /// Its middle, in doubled coordinates: the centre of its bounding box.
    pub(crate) centre: (i64, i64),
}

impl Gate {
    /// Tiles across it.
    pub(crate) fn width(&self) -> usize {
        self.tiles.len().div_ceil(2)
    }
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
        let mut home_nodes = vec![Vec::new(); seats];
        let mut distances = vec![None; seats];
        for (player, anchor) in anchors {
            let seat = usize::from(player.0);
            starts[seat] = Some(*anchor);
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
        let spot = spot_grid(map, &components, 2);
        let tile_spot = spot_grid(map, &components, 1);
        let extents = extents(&components);
        // Ground the map fixes buildings on: Extractor frames and expansion
        // anchors, which no gate may hold.
        let mut fixed: Vec<TilePos> = map
            .iter()
            .filter(|(tile, _)| map.tile_in_extractor_frame(*tile))
            .map(|(tile, _)| tile)
            .chain(sites.iter().flat_map(|site| {
                site.anchors.iter().flat_map(|anchor| {
                    (0..2).flat_map(move |dy| (0..2).map(move |dx| anchor.offset(dx, dy)))
                })
            }))
            .collect();
        fixed.sort_unstable();
        fixed.dedup();
        let mut model = Self {
            components,
            cover,
            starts,
            spot,
            tile_spot,
            extents,
            home_nodes,
            distances,
            teams,
            sites,
            cuts: Vec::new(),
            gated: Vec::new(),
        };
        model.cuts = (0..seats)
            .map(|seat| model.find_cuts(PlayerId::from_index(seat), &fixed))
            .collect();
        model.gated = model
            .cuts
            .iter()
            .map(|cuts| {
                let mut tiles: Vec<TilePos> = cuts
                    .iter()
                    .flat_map(|cut| &cut.gates)
                    .flat_map(|gate| gate.tiles.iter().copied())
                    .collect();
                tiles.sort_unstable();
                tiles.dedup();
                tiles
            })
            .collect();
        model
    }

    /// The cuts across the seat's ground ways to each hostile start: for
    /// every two-tile-deep band of ground at one distance from its start,
    /// where the tiles on routes to that hostile start no more than
    /// [`CUT_SLACK`] longer than the shortest fall in at most [`CUT_GATES`]
    /// connected pieces of the band, each no more than [`GATE_WIDTH`]
    /// across, together narrower than their distance from the start, off
    /// the `fixed` ground the map puts buildings on, and with them closed no
    /// ground way left from the start to that hostile start at all; a piece
    /// the others close every way without is left out. Open
    /// ground around a start at that distance is wider than that, even in a
    /// corner.
    fn find_cuts(&self, player: PlayerId, fixed: &[TilePos]) -> Vec<Cut> {
        let seat = usize::from(player.0);
        let (Some(home), Some(start)) = (
            self.distances.get(seat).and_then(Option::as_ref),
            self.start(player),
        ) else {
            return Vec::new();
        };
        let ground = self.component(start);
        let (width, height) = self.size();
        let tiles = || (0..height).flat_map(move |y| (0..width).map(move |x| TilePos::new(x, y)));
        let near = |tile: TilePos| home.get(tile).copied().unwrap_or(UNREACHABLE);
        let mut cuts = Vec::new();
        // Hostile starts often share a cut's gates: each closed set is
        // flooded once.
        let mut flooded: Vec<(Vec<TilePos>, Vec<u64>)> = Vec::new();
        for hostile in self.hostiles(player) {
            let Some(away) = self
                .distances
                .get(usize::from(hostile.0))
                .and_then(Option::as_ref)
            else {
                continue;
            };
            if self
                .start(hostile)
                .and_then(|anchor| self.component(anchor))
                != ground
            {
                continue;
            }
            let far = |tile: TilePos| away.get(tile).copied().unwrap_or(UNREACHABLE);
            let route = |tile: TilePos| {
                (near(tile) != UNREACHABLE && far(tile) != UNREACHABLE)
                    .then(|| u32::from(near(tile)) + u32::from(far(tile)))
            };
            let Some(shortest) = tiles().filter_map(route).min() else {
                continue;
            };
            let band: Vec<TilePos> = tiles()
                .filter(|tile| {
                    route(*tile)
                        .is_some_and(|length| length <= shortest + 10 * u32::from(CUT_SLACK))
                })
                .collect();
            // Never past half the way to the hostile start, in tiles.
            let half = u16::try_from(shortest / 20).unwrap_or(u16::MAX);
            for level in CUT_FROM..=CUT_REACH.min(half.saturating_sub(2)) {
                let shell = |tile: TilePos| {
                    self.component(tile) == ground
                        && (level..level + 2).contains(&(near(tile) / 10))
                };
                let Some(mut gates) = self.gates_across(&band, shell) else {
                    continue;
                };
                let Some(their) = self.start(hostile) else {
                    continue;
                };
                // The ground the start reaches with `gates` closed, if that
                // leaves no way to the hostile start.
                let mut separate = |gates: &[Gate]| {
                    let mut tiles: Vec<TilePos> = gates
                        .iter()
                        .flat_map(|gate| gate.tiles.iter().copied())
                        .collect();
                    tiles.sort_unstable();
                    let home = if let Some((_, home)) =
                        flooded.iter().find(|(closed, _)| *closed == tiles)
                    {
                        home.clone()
                    } else {
                        let home = self.reach_without(start, &tiles);
                        flooded.push((tiles, home.clone()));
                        home
                    };
                    let cut = Cut {
                        hostile,
                        distance: level * 10,
                        gates: Vec::new(),
                        home,
                        width,
                    };
                    (!cut.holds(their)).then_some(cut.home)
                };
                let Some(mut home) = separate(&gates) else {
                    continue;
                };
                // A gate the others separate the starts without, such as a
                // dead end's mouth, holds no way in.
                let mut index = 0;
                while index < gates.len() && gates.len() > 1 {
                    let mut others = gates.clone();
                    others.remove(index);
                    match separate(&others) {
                        Some(without) => {
                            gates = others;
                            home = without;
                        }
                        None => index += 1,
                    }
                }
                if gates.iter().map(Gate::width).sum::<usize>() >= usize::from(level)
                    || gates
                        .iter()
                        .flat_map(|gate| &gate.tiles)
                        .any(|tile| fixed.binary_search(tile).is_ok())
                {
                    continue;
                }
                cuts.push(Cut {
                    hostile,
                    distance: level * 10,
                    gates,
                    home,
                    width,
                });
            }
        }
        cuts
    }

    /// The ground a unit at `start` reaches with `closed` tiles, sorted,
    /// walled off, one bit a tile in row order. [`distance_field`] steps
    /// diagonally only between two open sides, so steps along the axes reach
    /// the same ground.
    fn reach_without(&self, start: TilePos, closed: &[TilePos]) -> Vec<u64> {
        let (width, height) = self.size();
        let bit = |tile: TilePos| usize::try_from(tile.y * width + tile.x).ok();
        let mut reached = vec![0_u64; usize::try_from(width * height).unwrap_or(0).div_ceil(64)];
        let open =
            |tile: TilePos| self.component(tile).is_some() && closed.binary_search(&tile).is_err();
        let mut frontier: Vec<TilePos> = (0..2)
            .flat_map(|dy| (0..2).map(move |dx| start.offset(dx, dy)))
            .filter(|tile| open(*tile))
            .collect();
        let mark = |reached: &mut Vec<u64>, tile: TilePos| {
            bit(tile).is_some_and(|at| {
                let fresh = reached[at / 64] >> (at % 64) & 1 == 0;
                reached[at / 64] |= 1 << (at % 64);
                fresh
            })
        };
        for tile in &frontier {
            mark(&mut reached, *tile);
        }
        while let Some(tile) = frontier.pop() {
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let next = tile.offset(dx, dy);
                if open(next) && mark(&mut reached, next) {
                    frontier.push(next);
                }
            }
        }
        reached
    }

    /// The pieces of the band `shell` holds that the tiles of `band` in it
    /// fall in, as gates, unless there are none, more than [`CUT_GATES`], or
    /// one is wider than [`GATE_WIDTH`].
    fn gates_across(&self, band: &[TilePos], shell: impl Fn(TilePos) -> bool) -> Option<Vec<Gate>> {
        let mut gates: Vec<Gate> = Vec::new();
        for first in band.iter().copied().filter(|tile| shell(*tile)) {
            if gates.iter().any(|gate| gate.tiles.contains(&first)) {
                continue;
            }
            if gates.len() == CUT_GATES {
                return None;
            }
            let mut piece = vec![first];
            let mut index = 0;
            while index < piece.len() {
                if piece.len() > 2 * GATE_WIDTH {
                    return None;
                }
                let tile = piece[index];
                index += 1;
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
                    let squeezed = dx != 0
                        && dy != 0
                        && !(self.component(tile.offset(dx, 0)).is_some()
                            && self.component(tile.offset(0, dy)).is_some());
                    if shell(next) && !squeezed && !piece.contains(&next) {
                        piece.push(next);
                    }
                }
            }
            if piece.len() > 2 * GATE_WIDTH {
                return None;
            }
            piece.sort_unstable();
            let (low, high) = piece
                .iter()
                .fold((piece[0], piece[0]), |(low, high), tile| {
                    (
                        TilePos::new(low.x.min(tile.x), low.y.min(tile.y)),
                        TilePos::new(high.x.max(tile.x), high.y.max(tile.y)),
                    )
                });
            gates.push(Gate {
                centre: (i64::from(low.x + high.x + 1), i64::from(low.y + high.y + 1)),
                tiles: piece,
            });
        }
        gates.sort_by_key(|gate| gate.tiles[0]);
        (!gates.is_empty()).then_some(gates)
    }

    /// The narrowest of the seat's cuts across its way to `hostile` whose
    /// near side lies further than `beyond` tenths of a tile from its start,
    /// the nearest of equals.
    pub(crate) fn cut(&self, player: PlayerId, hostile: PlayerId, beyond: u16) -> Option<&Cut> {
        self.cuts
            .get(usize::from(player.0))?
            .iter()
            .filter(|cut| cut.hostile == hostile && cut.distance > beyond)
            .min_by_key(|cut| (cut.width(), cut.distance))
    }

    /// Whether `tile` lies in one of the seat's gates, where it never builds.
    pub(crate) fn gated(&self, player: PlayerId, tile: TilePos) -> bool {
        self.gated
            .get(usize::from(player.0))
            .is_some_and(|tiles| tiles.binary_search(&tile).is_ok())
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
            .map(PlayerId::from_index)
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

    /// Anchors for a `kind` of the seat's buildings in the blocks beside
    /// `foundries`, nearest first: every spot at one gap from each Foundry
    /// in turn before any further from any, out to the edge of that
    /// Foundry's ground, each in the layout of its nearest Foundry on its
    /// ground. A Foundry takes the centre of an empty block, another
    /// two-by-two building a slot, and a smaller one a slot's tiles beside a
    /// lane, slot by slot. Once those run out, any place off the lanes
    /// follows, so cramped ground still takes every building that fits. A
    /// spot is open ground on its Foundry's ground, off every frame and the
    /// seat's gates and a tile clear of starting scrap. Within a gap, spots go in the seat's
    /// frame, so mirrored seats list mirrored spots.
    pub(crate) fn spots(
        &self,
        player: PlayerId,
        foundries: Vec<TilePos>,
        kind: BuildingKind,
    ) -> impl Iterator<Item = TilePos> + '_ {
        let foundries: Vec<(TilePos, u32, i32)> = foundries
            .into_iter()
            .filter_map(|foundry| {
                let ground = self.component(foundry)?;
                let (low, high) = self.extents.get(ground as usize - 1)?;
                let reach = (foundry.x - low.x)
                    .max(high.x - foundry.x)
                    .max(foundry.y - low.y)
                    .max(high.y - foundry.y);
                Some((foundry, ground, reach))
            })
            .collect();
        self.layer(player, foundries.clone(), kind, true)
            .chain(self.layer(player, foundries, kind, false))
    }

    /// The spots of [`Self::spots`] in the layout's own places when `packed`,
    /// and otherwise those anywhere else off the lanes.
    fn layer(
        &self,
        player: PlayerId,
        foundries: Vec<(TilePos, u32, i32)>,
        kind: BuildingKind,
        packed: bool,
    ) -> impl Iterator<Item = TilePos> + '_ {
        let (width, height) = self.size();
        let frame = self
            .start(player)
            .map(|start| HomeFrame::at(start, width, height));
        let furthest = foundries
            .iter()
            .map(|(_, _, reach)| *reach)
            .max()
            .unwrap_or(0);
        let size = kind.base_stats().size;
        let small = size.0 < 2 || size.1 < 2;
        let foundry_size = BuildingKind::Foundry.base_stats().size;
        // A smaller building packs into a slot's tiles, listed slot by slot.
        let listed = if packed && small { foundry_size } else { size };
        (FIRST_GAP..=furthest).flat_map(move |gap| {
            let mut spots: Vec<TilePos> = Vec::new();
            let Some(frame) = frame else {
                return spots;
            };
            for (foundry, ground, reach) in &foundries {
                if gap > *reach {
                    continue;
                }
                // Only the nearest Foundry on a footprint's ground claims it.
                let claims = |anchor: TilePos, size: (i32, i32)| {
                    let own = crate::frame::gap(*foundry, foundry_size, anchor, size);
                    self.component(anchor) == Some(*ground)
                        && foundries
                            .iter()
                            .filter(|(_, other, _)| other == ground)
                            .all(|(other, _, _)| {
                                crate::frame::gap(*other, foundry_size, anchor, size) >= own
                            })
                };
                let open = |grid: &Grid<bool>, anchor: TilePos, size: (i32, i32)| {
                    grid.get(anchor) == Some(&true)
                        && claims(anchor, size)
                        && (0..size.1)
                            .flat_map(|dy| (0..size.0).map(move |dx| anchor.offset(dx, dy)))
                            .all(|tile| !self.gated(player, tile))
                };
                let laid_slot = |anchor: TilePos| {
                    slot(*foundry, anchor) && open(&self.spot, anchor, foundry_size)
                };
                let rank = |kind: BuildingKind| {
                    move |anchor: &TilePos| frame.rank(frame.home, footprint_centre(kind, *anchor))
                };
                let around = around(*foundry, foundry_size, listed, gap);
                let mut found: Vec<TilePos> = match (packed, small) {
                    (true, true) => {
                        let mut slots: Vec<TilePos> =
                            around.filter(|anchor| laid_slot(*anchor)).collect();
                        slots.sort_by_key(rank(BuildingKind::Foundry));
                        slots
                            .into_iter()
                            .flat_map(|slot| {
                                let mut tiles = edge_tiles(*foundry, slot);
                                tiles.sort_by_key(rank(kind));
                                tiles
                            })
                            .collect()
                    }
                    (true, false) => around
                        .filter(|anchor| {
                            let shaped = if kind == BuildingKind::Foundry {
                                block_centre(*foundry, *anchor)
                            } else {
                                slot(*foundry, *anchor)
                            };
                            shaped && open(&self.spot, *anchor, size)
                        })
                        .collect(),
                    // Any tile off the lanes but those of the slots above.
                    (false, true) => around
                        .filter(|anchor| {
                            !lane(*foundry, *anchor)
                                && open(&self.tile_spot, *anchor, size)
                                && !(edge_tile(*foundry, *anchor)
                                    && laid_slot(slot_of(*foundry, *anchor)))
                        })
                        .collect(),
                    (false, false) => around
                        .filter(|anchor| {
                            off_lanes(*foundry, *anchor, size)
                                && !(slot(*foundry, *anchor) || block_centre(*foundry, *anchor))
                                && open(&self.spot, *anchor, size)
                        })
                        .collect(),
                };
                if !(packed && small) {
                    found.sort_by_key(rank(kind));
                }
                found.retain(|spot| !spots.contains(spot));
                spots.extend(found);
            }
            spots
        })
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

/// Anchors of two-by-two footprints of open ground on one component, off
/// every frame and a tile clear of starting scrap, where Harvesters still
/// get through.
fn spot_grid(map: &Map, components: &Grid<u32>, size: i32) -> Grid<bool> {
    let mut spot = Grid::new(map.width(), map.height(), false);
    for y in 0..map.height() {
        for x in 0..map.width() {
            let anchor = TilePos::new(x, y);
            let ground = components.get(anchor).copied().filter(|label| *label != 0);
            let open = (0..size).all(|dy| {
                (0..size).all(|dx| {
                    let tile = anchor.offset(dx, dy);
                    ground.is_some()
                        && components.get(tile).copied() == ground
                        && !map.tile_in_extractor_frame(tile)
                })
            });
            let clear = (-1..=size)
                .all(|dy| (-1..=size).all(|dx| map.scrap_at(anchor.offset(dx, dy)) == 0));
            if open
                && clear
                && let Some(slot) = spot.get_mut(anchor)
            {
                *slot = true;
            }
        }
    }
    spot
}

/// Each ground component's bounding box, by component.
fn extents(components: &Grid<u32>) -> Vec<(TilePos, TilePos)> {
    let mut extents: Vec<(TilePos, TilePos)> = Vec::new();
    for y in 0..components.height() {
        for x in 0..components.width() {
            let tile = TilePos::new(x, y);
            let Some(label) = components.get(tile).copied().filter(|label| *label != 0) else {
                continue;
            };
            let index = label as usize - 1;
            if extents.len() <= index {
                extents.resize(index + 1, (tile, tile));
            }
            let (low, high) = &mut extents[index];
            *low = TilePos::new(low.x.min(x), low.y.min(y));
            *high = TilePos::new(high.x.max(x), high.y.max(y));
        }
    }
    extents
}

/// The anchors of a footprint of `size` with `gap` empty tiles between it
/// and the one of `foundry_size` at `foundry`.
fn around(
    foundry: TilePos,
    foundry_size: (i32, i32),
    size: (i32, i32),
    gap: i32,
) -> impl Iterator<Item = TilePos> {
    crate::frame::ring(
        foundry.offset(1 - gap - size.0, 1 - gap - size.1),
        (
            2 * gap + size.0 + foundry_size.0 - 1,
            2 * gap + size.1 + foundry_size.1 - 1,
        ),
    )
}

/// `tile`'s place in the layout around a Foundry at `foundry`, per axis.
fn residue(foundry: TilePos, tile: TilePos) -> (i32, i32) {
    (
        (tile.x - foundry.x).rem_euclid(PERIOD),
        (tile.y - foundry.y).rem_euclid(PERIOD),
    )
}

/// Whether `tile` lies on a lane of the layout around a Foundry at `foundry`.
pub(crate) fn lane(foundry: TilePos, tile: TilePos) -> bool {
    let (x, y) = residue(foundry, tile);
    x == LANE || y == LANE
}

impl MapModel {
    /// Whether any of `foundries` stands on `tile`'s ground, laying it out.
    pub(crate) fn laid_out(&self, foundries: &[TilePos], tile: TilePos) -> bool {
        let ground = self.component(tile);
        ground.is_some()
            && foundries
                .iter()
                .any(|foundry| self.component(*foundry) == ground)
    }

    /// Whether `tile` lies on a lane of the layout of its nearest Foundry
    /// among `foundries` on its ground, any of them tied.
    pub(crate) fn lane_of(&self, foundries: &[TilePos], tile: TilePos) -> bool {
        let Some(ground) = self.component(tile) else {
            return false;
        };
        let mut nearest: Option<(i32, bool)> = None;
        for foundry in foundries {
            if self.component(*foundry) != Some(ground) {
                continue;
            }
            let distance = chebyshev(tile, *foundry);
            let laned = lane(*foundry, tile);
            nearest = match nearest {
                Some((best, any)) if best < distance => Some((best, any)),
                Some((best, any)) if best == distance => Some((best, any || laned)),
                _ => Some((distance, laned)),
            };
        }
        nearest.is_some_and(|(_, laned)| laned)
    }
}

/// Whether `anchor` is a two-by-two slot of the layout around a Foundry at
/// `foundry`: a corner of a block, touching two lanes.
fn slot(foundry: TilePos, anchor: TilePos) -> bool {
    let (x, y) = residue(foundry, anchor);
    matches!(x, 1 | 4) && matches!(y, 1 | 4)
}

/// Whether `anchor` is the centre of a block of the layout around a Foundry
/// at `foundry`, where another Foundry stands with its ring the block.
fn block_centre(foundry: TilePos, anchor: TilePos) -> bool {
    residue(foundry, anchor) == (0, 0) && anchor != foundry
}

/// A slot's three tiles beside a lane, all but the one inside its block.
fn edge_tiles(foundry: TilePos, slot: TilePos) -> Vec<TilePos> {
    (0..2)
        .flat_map(|dy| (0..2).map(move |dx| slot.offset(dx, dy)))
        .filter(|tile| edge_tile(foundry, *tile))
        .collect()
}

/// Whether `tile`, off the lanes, touches one.
fn edge_tile(foundry: TilePos, tile: TilePos) -> bool {
    let (x, y) = residue(foundry, tile);
    !lane(foundry, tile) && (!matches!(x, 0 | 1) || !matches!(y, 0 | 1))
}

/// The slot holding `tile`, off the lanes.
fn slot_of(foundry: TilePos, tile: TilePos) -> TilePos {
    let (x, y) = residue(foundry, tile);
    let back = |residue: i32| i32::from(matches!(residue, 0 | 2));
    tile.offset(-back(x), -back(y))
}

/// Whether a footprint of `size` at `anchor` keeps off every lane of the
/// layout around a Foundry at `foundry`.
fn off_lanes(foundry: TilePos, anchor: TilePos, (width, height): (i32, i32)) -> bool {
    (0..height)
        .flat_map(|dy| (0..width).map(move |dx| anchor.offset(dx, dy)))
        .all(|tile| !lane(foundry, tile))
}

fn chebyshev(a: TilePos, b: TilePos) -> i32 {
    (a.x - b.x).abs().max((a.y - b.y).abs())
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
                *cell = u16::try_from(reached).expect("reached stays below UNREACHABLE");
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
    let count = i64::try_from(nodes.len()).expect("node counts fit in i64");
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
pub(crate) mod tests {
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

    /// Whether a `kind` at `spot` stands where the layout of `foundries`
    /// packs it: a Foundry with its ring clear of lanes, a smaller building
    /// beside a lane, and a two-by-two one beside lanes both ways.
    fn packed(model: &MapModel, foundries: &[TilePos], kind: BuildingKind, spot: TilePos) -> bool {
        let (width, height) = kind.base_stats().size;
        let laned = |tile: TilePos| model.lane_of(foundries, tile);
        let across =
            (0..height).any(|dy| laned(spot.offset(-1, dy)) || laned(spot.offset(width, dy)));
        let along =
            (0..width).any(|dx| laned(spot.offset(dx, -1)) || laned(spot.offset(dx, height)));
        match kind {
            BuildingKind::Foundry => {
                crate::frame::ring(spot, (width, height)).all(|tile| !laned(tile))
            }
            _ if width == 1 => across || along,
            _ => across && along,
        }
    }

    #[test]
    fn spots_pack_blocks_beside_lanes_clear_of_foundry_rings_and_mirror_between_seats() {
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
        let start = model.start(PlayerId(0)).unwrap();
        let spots = |seat: u8, kind: BuildingKind| {
            let start = model.start(PlayerId(seat)).unwrap();
            model
                .spots(PlayerId(seat), vec![start], kind)
                .collect::<Vec<_>>()
        };
        let tiles = |anchor: TilePos, (width, height): (i32, i32)| {
            (0..height).flat_map(move |dy| (0..width).map(move |dx| anchor.offset(dx, dy)))
        };
        let foundry = BuildingKind::Foundry.base_stats().size;
        let laid = |kind: BuildingKind, spot: TilePos| packed(&model, &[start], kind, spot);
        for kind in [
            BuildingKind::Fabricator,
            BuildingKind::Reclaimer,
            BuildingKind::Foundry,
        ] {
            let size = kind.base_stats().size;
            let (west, east) = (spots(0, kind), spots(1, kind));
            let first = west.iter().take_while(|spot| laid(kind, **spot)).count();
            assert!(first > 0, "{kind:?}: {west:?}");
            for spot in &west {
                assert!(
                    tiles(*spot, size).all(|tile| !lane(start, tile)),
                    "{kind:?} at {spot:?} stands on a lane"
                );
                assert!(
                    crate::frame::gap(start, foundry, *spot, size) >= 1,
                    "{kind:?} at {spot:?} crowds the Foundry's ring"
                );
            }
            let distances: Vec<i32> = west[..first]
                .iter()
                .map(|spot| chebyshev(*spot, start))
                .collect();
            if size == (2, 2) {
                assert!(distances.is_sorted(), "nearest first: {distances:?}");
            }
            assert!(
                west[first..].iter().all(|spot| !laid(kind, *spot)) || size == (1, 1),
                "the layout's places come first: {west:?}"
            );
            let rotate =
                |anchor: TilePos| TilePos::new(20 - size.0 - anchor.x, 16 - size.1 - anchor.y);
            assert_eq!(
                west.iter().copied().map(rotate).collect::<Vec<_>>(),
                east,
                "{kind:?}"
            );
        }
        assert_eq!(
            model
                .spots(
                    PlayerId(2),
                    vec![TilePos::new(9, 7)],
                    BuildingKind::Fabricator
                )
                .count(),
            0,
            "a seat without a start lists none"
        );
    }

    #[test]
    fn the_layout_mirrors_under_a_reflection_too() {
        // West and east face each other across a vertical mirror line.
        const MIRROR: [&str; 12] = [
            "######################",
            "#....................#",
            "#....................#",
            "#....................#",
            "#....................#",
            "#..1.............2...#",
            "#....................#",
            "#....................#",
            "#....................#",
            "#....................#",
            "#....................#",
            "######################",
        ];
        let model = model(&MIRROR);
        let width = 22;
        let west = model.start(PlayerId(0)).unwrap();
        let east = model.start(PlayerId(1)).unwrap();
        assert_eq!(east, TilePos::new(width - 2 - west.x, west.y));
        for y in 0..12 {
            for x in 0..width {
                let tile = TilePos::new(x, y);
                let mirrored = TilePos::new(width - 1 - x, y);
                assert_eq!(lane(west, tile), lane(east, mirrored), "{tile:?}");
            }
        }
    }

    #[test]
    fn a_spot_belongs_to_the_nearest_foundry_on_its_own_ground() {
        // Rock splits the field: the west Foundry's spots run up to the
        // wall, nearer the east Foundry across it than the west one.
        const SPLIT: [&str; 10] = [
            "##############################",
            "#.............##.............#",
            "#.............##.............#",
            "#.............##.............#",
            "#..1..........##.............#",
            "#.............##.............#",
            "#.............##.............#",
            "#.............##.............#",
            "#.............##.............#",
            "##############################",
        ];
        let model = model(&SPLIT);
        let west = TilePos::new(5, 4);
        let east = TilePos::new(16, 4);
        assert_ne!(model.component(west), model.component(east));
        let beside_wall = TilePos::new(11, 5);
        assert!(
            chebyshev(beside_wall, east) < chebyshev(beside_wall, west),
            "premise: nearer the east Foundry"
        );
        assert!(
            model
                .spots(PlayerId(0), vec![west, east], BuildingKind::Fabricator)
                .any(|spot| spot == beside_wall)
        );
    }

    #[test]
    fn spots_beside_every_foundry_come_before_further_ones_beside_any() {
        const FIELD: [&str; 12] = [
            "########################################",
            "#......................................#",
            "#......................................#",
            "#......................................#",
            "#......................................#",
            "#...1..............................2...#",
            "#......................................#",
            "#......................................#",
            "#......................................#",
            "#......................................#",
            "#......................................#",
            "########################################",
        ];
        let model = model(&FIELD);
        let start = model.start(PlayerId(0)).unwrap();
        let expansion = TilePos::new(20, 5);
        let distance = |a: TilePos, b: TilePos| (a.x - b.x).abs().max((a.y - b.y).abs());
        let spots: Vec<TilePos> = model
            .spots(
                PlayerId(0),
                vec![start, expansion],
                BuildingKind::Fabricator,
            )
            .collect();
        let nearest = |spot: TilePos| distance(spot, start).min(distance(spot, expansion));
        let first = spots
            .iter()
            .take_while(|spot| {
                packed(
                    &model,
                    &[start, expansion],
                    BuildingKind::Fabricator,
                    **spot,
                )
            })
            .count();
        let rings: Vec<i32> = spots[..first].iter().map(|spot| nearest(*spot)).collect();
        assert!(rings.is_sorted(), "{rings:?}");
        assert!(
            spots
                .iter()
                .any(|spot| distance(*spot, expansion) == FIRST_GAP + 2),
            "the expansion has spots at the first gap too"
        );
        assert_eq!(
            spots.first().map(|spot| distance(*spot, start)),
            Some(FIRST_GAP + 2),
            "home goes first at a gap"
        );
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
                let start = model.start(PlayerId::from_index(seat)).unwrap();
                assert!(model.component(start).is_some(), "{}", path.display());
                assert!(
                    model
                        .spots(
                            PlayerId::from_index(seat),
                            vec![start],
                            BuildingKind::Fabricator
                        )
                        .take(8)
                        .count()
                        == 8,
                    "{} seat {seat}",
                    path.display()
                );
            }
        }
    }

    /// Two rooms joined by a corridor three tiles wide.
    pub(crate) const CORRIDOR: [&str; 15] = [
        "########################################",
        "#..............##########..............#",
        "#..............##########..............#",
        "#..s...........##########...........s..#",
        "#..............##########..............#",
        "#..............##########..............#",
        "#................................2.....#",
        "#....1.................................#",
        "#......................................#",
        "#..............##########..............#",
        "#..............##########..............#",
        "#..s...........##########...........s..#",
        "#..............##########..............#",
        "#..............##########..............#",
        "########################################",
    ];

    /// [`CORRIDOR`] with the corridor walled up and `rows` opened across the
    /// wall instead.
    pub(crate) fn ways(rows: &[usize]) -> Vec<String> {
        let mut map = CORRIDOR.map(str::to_owned);
        for row in [6, 7, 8] {
            let mut cells: Vec<char> = map[row].chars().collect();
            for cell in &mut cells[15..=24] {
                *cell = '#';
            }
            map[row] = cells.into_iter().collect();
        }
        for row in rows {
            map[*row] = map[*row].replace("##########", "..........");
        }
        map.to_vec()
    }

    /// Each seat's narrowest cut across its way to the other, from any
    /// distance.
    fn cuts(rows: &[String]) -> Vec<Option<(u16, Vec<Vec<TilePos>>)>> {
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        let model = model(&rows);
        (0..2)
            .map(|seat| {
                model.cut(PlayerId(seat), PlayerId(1 - seat), 0).map(|cut| {
                    (
                        cut.distance,
                        cut.gates.iter().map(|gate| gate.tiles.clone()).collect(),
                    )
                })
            })
            .collect()
    }

    /// `gates` turned a half-turn about the middle of a 40 by 15 map.
    fn rotated(gates: &[Vec<TilePos>]) -> Vec<Vec<TilePos>> {
        let mut rotated: Vec<Vec<TilePos>> = gates
            .iter()
            .map(|tiles| {
                let mut tiles: Vec<TilePos> = tiles
                    .iter()
                    .map(|tile| TilePos::new(39 - tile.x, 14 - tile.y))
                    .collect();
                tiles.sort_unstable();
                tiles
            })
            .collect();
        rotated.sort_by_key(|tiles| tiles[0]);
        rotated
    }

    #[test]
    fn a_corridor_between_two_rooms_is_a_cut_mirrored_between_seats() {
        let corridor: Vec<String> = CORRIDOR.map(str::to_owned).to_vec();
        let cuts = cuts(&corridor);
        let (distance, west) = cuts[0].clone().expect("the corridor is a cut");
        assert_eq!(west.len(), 1, "{west:?}");
        assert!(
            west[0]
                .iter()
                .all(|tile| (15..=24).contains(&tile.x) && (6..=8).contains(&tile.y)),
            "{west:?}"
        );
        assert_eq!(west[0].len(), 6, "three across, two deep: {west:?}");
        assert_eq!(cuts[1], Some((distance, rotated(&west))));
        let model = model(&CORRIDOR);
        assert!(model.gated(PlayerId(0), west[0][0]));
        assert!(!model.gated(PlayerId(0), TilePos::new(5, 3)));
        assert!(
            model
                .cut(PlayerId(0), PlayerId(1), distance + 200)
                .is_none(),
            "no cut lies past half the way"
        );
    }

    #[test]
    fn two_ways_through_a_wall_are_one_cut_of_two_gates() {
        let cuts = cuts(&ways(&[3, 4, 10, 11]));
        let (distance, west) = cuts[0].clone().expect("the two ways are a cut");
        assert_eq!(west.len(), 2, "{west:?}");
        for (gate, rows) in west.iter().zip([3..=4, 10..=11]) {
            assert!(
                gate.iter()
                    .all(|tile| (15..=24).contains(&tile.x) && rows.contains(&tile.y)),
                "{west:?}"
            );
        }
        assert_eq!(cuts[1], Some((distance, rotated(&west))));
    }

    #[test]
    fn a_corridor_with_a_long_way_round_is_no_cut() {
        // A path from the west room's corner down, along the bottom and up
        // into the east room, far longer than the corridor.
        let mut map: Vec<String> = CORRIDOR.map(str::to_owned).to_vec();
        let mut last: Vec<char> = map[14].chars().collect();
        last[1] = '.';
        last[38] = '.';
        map[14] = last.into_iter().collect();
        for _ in 0..8 {
            map.push(format!("#.{}.#", "#".repeat(36)));
        }
        map.push(format!("#{}#", ".".repeat(38)));
        map.push("#".repeat(40));
        let rows: Vec<&str> = map.iter().map(String::as_str).collect();
        let model = model(&rows);
        assert!(model.cut(PlayerId(0), PlayerId(1), 0).is_none());
    }

    #[test]
    fn no_gate_holds_an_extractor_frame() {
        let mut map: Vec<String> = CORRIDOR.map(str::to_owned).to_vec();
        let mut row: Vec<char> = map[7].chars().collect();
        row[15] = 'E';
        map[7] = row.into_iter().collect();
        let rows: Vec<&str> = map.iter().map(String::as_str).collect();
        let model = model(&rows);
        let cut = model
            .cut(PlayerId(0), PlayerId(1), 0)
            .expect("the corridor further on is still a cut");
        assert!(
            cut.gates
                .iter()
                .flat_map(|gate| &gate.tiles)
                .all(|tile| !(15..=16).contains(&tile.x) || !(7..=8).contains(&tile.y)),
            "{cut:?}"
        );
    }

    #[test]
    fn open_ground_and_many_ways_have_no_cut() {
        let open: Vec<String> = CORRIDOR
            .iter()
            .map(|row| row.replace("##########", ".........."))
            .collect();
        let narrow: Vec<String> = ARENA_ROWS.iter().map(|row| (*row).to_owned()).collect();
        for rows in [open, narrow, ways(&[2, 5, 9, 12])] {
            let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
            let model = model(&rows);
            for seat in 0..2 {
                assert!(
                    model.cut(PlayerId(seat), PlayerId(1 - seat), 0).is_none(),
                    "{rows:#?}"
                );
            }
        }
    }

    /// A field ten tiles across all the way: narrow, but no narrower anywhere.
    const ARENA_ROWS: [&str; 12] = [
        "########################",
        "#......................#",
        "#...............s......#",
        "#......s...............#",
        "#......................#",
        "#..1...............2...#",
        "#......................#",
        "#......................#",
        "#...............s......#",
        "#......s...............#",
        "#......................#",
        "########################",
    ];
}
