//! Whether the seat's own knowledge allows a building footprint. The check
//! reads only the fog-honest observation, so hidden units and unseen buildings
//! can never change its verdict; the simulation re-checks on arrival and a
//! refused order is re-planned.

use crate::memory::Memory;
use chassis::fx::Fx;
use chassis::grid::TilePos;
use oxide_sim::BuildingKind;
use oxide_sim::observation::{BuildingObs, ObservationData};
use oxide_sim::stats::{Domain, HARVEST_INCIDENT_DANGER_RADIUS};

/// Why the seat's knowledge rules a footprint out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// A required building is not complete.
    Prerequisite,
    /// Part of the footprint is off the map or unexplored.
    Unexplored,
    /// An Extractor off a frame, or anything else on one.
    Frame,
    /// Known rock, pit, peak or scrap.
    Terrain,
    /// A known building stands there.
    Occupied,
    /// It would touch one of the seat's own buildings, which could wall in a
    /// producer.
    Crowded,
    /// A visible hostile ground unit stands there.
    HostileUnit,
    /// An own construction claim or this decision's plan holds the ground.
    Claimed,
    /// No explored open tile borders it.
    NoEgress,
}

/// How near the seat's own buildings a footprint may stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    /// A tile clear of every own building, as a Foundry needs for the
    /// workers and new units around it, and a defense on its own.
    Apart,
    /// Packed into a block of the base's layout, whose lanes keep the way
    /// open: clear only of own Foundries and their rings.
    Packed,
}

/// A footprint the seat may claim, and whether it must be claimed as a
/// provisional scaffold because part of it is out of sight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Allowed {
    pub(crate) defer: bool,
}

/// Checks `kind` at `anchor` against the seat's knowledge and the footprints
/// this decision already planned, standing as `layout` allows. A buried own
/// Scuttle Charge blocks nothing, so it keeps no other building away.
pub(crate) fn check(
    observation: &ObservationData,
    kind: BuildingKind,
    anchor: TilePos,
    planned: &[(BuildingKind, TilePos)],
    layout: Layout,
) -> Result<Allowed, Refusal> {
    let requires = kind
        .base_stats()
        .construction
        .as_ref()
        .map_or(&[][..], |construction| construction.requires);
    let built = |required: &BuildingKind| {
        observation
            .my_buildings
            .iter()
            .any(|building| building.kind == *required && building.built)
    };
    if !requires.iter().all(built) {
        return Err(Refusal::Prerequisite);
    }
    let footprint = Footprint::of(kind, anchor);
    if !footprint.tiles().all(|tile| observation.explored(tile)) {
        return Err(Refusal::Unexplored);
    }
    let frame = |tile: TilePos| {
        observation
            .known_frames
            .iter()
            .any(|frame| Footprint::of(BuildingKind::Extractor, *frame).contains(tile))
    };
    let on_frame = if kind == BuildingKind::Extractor {
        !observation.known_frames.contains(&anchor)
    } else {
        footprint.tiles().any(frame)
    };
    if on_frame {
        return Err(Refusal::Frame);
    }
    if footprint.tiles().any(|tile| blocked(observation, tile)) {
        return Err(Refusal::Terrain);
    }
    let known = observation
        .my_buildings
        .iter()
        .chain(&observation.ally_buildings)
        .chain(&observation.enemy_buildings);
    if known
        .clone()
        .any(|building| footprint.overlaps(building_footprint(building)))
    {
        return Err(Refusal::Occupied);
    }
    // Frames sit where the map put them, so an Extractor keeps no gap.
    let clearance = if kind == BuildingKind::Extractor {
        footprint
    } else {
        footprint.grown()
    };
    // Whether an own building or claim keeps this footprint away. A buried
    // charge stands anywhere a building could not.
    let charge = kind == BuildingKind::ScuttleCharge;
    let keeps = |other_kind: BuildingKind, other_anchor: TilePos| {
        let other = Footprint::of(other_kind, other_anchor);
        match layout {
            _ if charge => false,
            Layout::Apart => clearance.overlaps(other),
            Layout::Packed if other_kind == BuildingKind::Foundry => {
                footprint.overlaps(other.grown())
            }
            Layout::Packed => footprint.overlaps(other),
        }
    };
    if observation
        .my_buildings
        .iter()
        .filter(|building| building.kind != BuildingKind::ScuttleCharge)
        .any(|building| keeps(building.kind, building.anchor))
    {
        return Err(Refusal::Crowded);
    }
    if observation
        .enemy_units
        .iter()
        .any(|unit| unit.body_domain() == Domain::Ground && footprint.contains(unit.tile))
    {
        return Err(Refusal::HostileUnit);
    }
    let founding = observation
        .my_units
        .iter()
        .filter_map(|unit| unit.founding)
        .chain(planned.iter().copied());
    for (claimed, at) in founding {
        if keeps(claimed, at) {
            return Err(Refusal::Claimed);
        }
    }
    let egress = footprint.grown().tiles().any(|tile| {
        !footprint.contains(tile)
            && observation.explored(tile)
            && !blocked(observation, tile)
            && !known
                .clone()
                .any(|building| building_footprint(building).contains(tile))
    });
    if !egress {
        return Err(Refusal::NoEgress);
    }
    Ok(Allowed {
        defer: !footprint.tiles().all(|tile| observation.visible(tile)),
    })
}

fn blocked(observation: &ObservationData, tile: TilePos) -> bool {
    observation.known_rock_at(tile) || observation.known_scrap_at(tile)
}

fn building_footprint(building: &BuildingObs) -> Footprint {
    Footprint::of(building.kind, building.anchor)
}

/// A rectangle of tiles.
#[derive(Clone, Copy)]
struct Footprint {
    anchor: TilePos,
    width: i32,
    height: i32,
}

impl Footprint {
    fn of(kind: BuildingKind, anchor: TilePos) -> Self {
        let (width, height) = kind.base_stats().size;
        Self {
            anchor,
            width,
            height,
        }
    }

    /// The footprint and the ring of tiles around it.
    fn grown(self) -> Self {
        Self {
            anchor: self.anchor.offset(-1, -1),
            width: self.width + 2,
            height: self.height + 2,
        }
    }

    fn contains(self, tile: TilePos) -> bool {
        (self.anchor.x..self.anchor.x + self.width).contains(&tile.x)
            && (self.anchor.y..self.anchor.y + self.height).contains(&tile.y)
    }

    fn overlaps(self, other: Self) -> bool {
        self.anchor.x < other.anchor.x + other.width
            && other.anchor.x < self.anchor.x + self.width
            && self.anchor.y < other.anchor.y + other.height
            && other.anchor.y < self.anchor.y + self.height
    }

    fn tiles(self) -> impl Iterator<Item = TilePos> + Clone {
        (0..self.height)
            .flat_map(move |dy| (0..self.width).map(move |dx| self.anchor.offset(dx, dy)))
    }
}

/// What the seat knows can hit a building it would raise: known enemy guns,
/// remembered armed enemy units, and where it lately took losses. Built once
/// a decision, from the seat's own knowledge only.
pub(crate) struct Danger {
    /// Each known enemy gun that fires at ground: its centre and its reach
    /// either side, doubled.
    guns: Vec<((i64, i64), i64, i64)>,
    /// Each remembered armed enemy unit's tile centre and longest reach at
    /// buildings, doubled.
    units: Vec<((i64, i64), i64)>,
    /// Tiles where the seat lately took damage or losses.
    incidents: Vec<TilePos>,
}

impl Danger {
    pub(crate) fn of(observation: &ObservationData, memory: &Memory) -> Self {
        let doubled = |range: Fx| (range + range).to_num::<i64>();
        let guns = observation
            .enemy_buildings
            .iter()
            // A site seen still going up fires at nothing yet.
            .filter(|building| building.built || !building.seen)
            .flat_map(|building| {
                let stats = building.kind.tier_stats(building.tier);
                let (width, height) = stats.size;
                let centre = (
                    i64::from(2 * building.anchor.x + width),
                    i64::from(2 * building.anchor.y + height),
                );
                stats
                    .weapons
                    .iter()
                    .filter(|weapon| weapon.targets.ground)
                    .map(move |weapon| {
                        (centre, doubled(weapon.minimum_range), doubled(weapon.range))
                    })
            })
            .collect();
        let units = memory
            .units()
            .iter()
            .map(|unit| {
                (
                    (
                        i64::from(2 * unit.tile.x + 1),
                        i64::from(2 * unit.tile.y + 1),
                    ),
                    doubled(crate::defenses::reach(unit.kind)),
                )
            })
            .filter(|(_, reach)| *reach > 0)
            .collect();
        Danger {
            guns,
            units,
            incidents: observation.salvage_incidents.clone(),
        }
    }

    /// Whether something the seat knows of could hit a `kind` at `anchor`,
    /// or it lately took losses beside it.
    pub(crate) fn hits(&self, kind: BuildingKind, anchor: TilePos) -> bool {
        let footprint = Footprint::of(kind, anchor);
        let (low, high) = (
            (i64::from(2 * anchor.x), i64::from(2 * anchor.y)),
            (
                i64::from(2 * (anchor.x + footprint.width)),
                i64::from(2 * (anchor.y + footprint.height)),
            ),
        );
        // Squared doubled distances from `point` to the footprint's nearest
        // and furthest points.
        let near = |point: (i64, i64)| {
            let axis = |v: i64, low: i64, high: i64| (low - v).max(v - high).max(0);
            axis(point.0, low.0, high.0).pow(2) + axis(point.1, low.1, high.1).pow(2)
        };
        let far = |point: (i64, i64)| {
            let axis = |v: i64, low: i64, high: i64| (v - low).abs().max((high - v).abs());
            axis(point.0, low.0, high.0).pow(2) + axis(point.1, low.1, high.1).pow(2)
        };
        let gun = self.guns.iter().any(|(centre, minimum, range)| {
            near(*centre) <= range * range && far(*centre) >= minimum * minimum
        });
        let unit = self
            .units
            .iter()
            .any(|(at, reach)| near(*at) <= reach * reach);
        let incident = self.incidents.iter().any(|tile| {
            let gap_x = (footprint.anchor.x - tile.x)
                .max(tile.x - (footprint.anchor.x + footprint.width - 1));
            let gap_y = (footprint.anchor.y - tile.y)
                .max(tile.y - (footprint.anchor.y + footprint.height - 1));
            gap_x.max(gap_y) <= HARVEST_INCIDENT_DANGER_RADIUS
        });
        gun || unit || incident
    }
}
