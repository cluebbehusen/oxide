//! Whether the seat's own knowledge allows a building footprint. The check
//! reads only the fog-honest observation, so hidden units and unseen buildings
//! can never change its verdict; the simulation re-checks on arrival and a
//! refused order is re-planned.

use chassis::grid::TilePos;
use oxide_sim::BuildingKind;
use oxide_sim::observation::{BuildingObs, ObservationData};
use oxide_sim::stats::Domain;

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

/// A footprint the seat may claim, and whether it must be claimed as a
/// provisional scaffold because part of it is out of sight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Allowed {
    pub(crate) defer: bool,
}

/// Checks `kind` at `anchor` against the seat's knowledge and the footprints
/// this decision already planned.
pub(crate) fn check(
    observation: &ObservationData,
    kind: BuildingKind,
    anchor: TilePos,
    planned: &[(BuildingKind, TilePos)],
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
    if observation
        .my_buildings
        .iter()
        .any(|building| clearance.overlaps(building_footprint(building)))
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
        let claim = Footprint::of(claimed, at);
        if clearance.overlaps(claim) {
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
