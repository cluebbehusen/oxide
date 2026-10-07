//! What aircraft need to know about known enemy fire: where it reaches, how
//! much of it a straight flight crosses, and a detour around it.

use super::attack::building_value;
use crate::frame::{HomeFrame, doubled, footprint_centre, ring};
use crate::map::MapModel;
use crate::memory::Memory;
use chassis::grid::TilePos;
use oxide_sim::BuildingKind;
use oxide_sim::observation::ObservationData;
use oxide_sim::stats::Domain;

/// Tiles a detour swings the route out from its middle.
const DETOUR: i32 = 8;

/// Tiles beyond a weapon's reach a route or landing keeps clear of.
const CLEARANCE: i32 = 2;

/// Something that shoots at a domain, as a disc in doubled coordinates.
#[derive(Clone, Copy)]
pub(crate) struct Hazard {
    centre: (i64, i64),
    reach: i64,
    pub(super) value: u64,
}

impl Hazard {
    /// Whether `point`, in doubled coordinates, lies within reach.
    pub(crate) fn covers(&self, point: (i64, i64)) -> bool {
        distance2(self.centre, point) <= self.reach * self.reach
    }

    /// The tiles whose centres lie within reach.
    pub(crate) fn tiles(&self) -> impl Iterator<Item = TilePos> + '_ {
        let span = |centre: i64| {
            let low = (centre - self.reach).div_euclid(2);
            let high = (centre + self.reach).div_euclid(2);
            (low as i32)..=(high as i32)
        };
        span(self.centre.1)
            .flat_map(move |y| span(self.centre.0).map(move |x| TilePos::new(x, y)))
            .filter(|tile| self.covers(doubled(*tile)))
    }
}

/// Where aircraft leave from and come back to: beside the seat's start, on
/// its side nearest `toward`, in doubled coordinates.
pub(super) fn pad(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    toward: (i64, i64),
) -> Option<TilePos> {
    let start = map.start(observation.me)?;
    let home = map.component(start)?;
    ring(start, BuildingKind::Foundry.base_stats().size)
        .filter(|tile| map.component(*tile) == Some(home))
        .min_by_key(|tile| {
            let (x, y) = doubled(*tile);
            (
                (x - toward.0).abs().max((y - toward.1).abs()),
                frame.rank(frame.home, (x, y)),
            )
        })
}

/// A via-point around known anti-air between `from` and `to`, or `None`
/// when the straight line is clear or nothing does better. A route that
/// cannot be made clear is flown anyway.
pub(super) fn route(
    observation: &ObservationData,
    frame: HomeFrame,
    hazards: &[Hazard],
    from: TilePos,
    to: TilePos,
) -> Option<TilePos> {
    let (a, b) = (doubled(from), doubled(to));
    let direct = exposure(hazards, a, b);
    if direct == 0 {
        return None;
    }
    let (width, height) = (observation.map_width, observation.map_height);
    let xs = [i32::midpoint(from.x, to.x), (from.x + to.x + 1) / 2];
    let ys = [i32::midpoint(from.y, to.y), (from.y + to.y + 1) / 2];
    let offsets = [
        (-DETOUR, -DETOUR),
        (0, -DETOUR),
        (DETOUR, -DETOUR),
        (-DETOUR, 0),
        (DETOUR, 0),
        (-DETOUR, DETOUR),
        (0, DETOUR),
        (DETOUR, DETOUR),
    ];
    xs.into_iter()
        .flat_map(|x| ys.into_iter().map(move |y| TilePos::new(x, y)))
        .flat_map(|middle| {
            offsets
                .into_iter()
                .map(move |(dx, dy)| middle.offset(dx, dy))
        })
        .map(|via| TilePos::new(via.x.clamp(0, width - 1), via.y.clamp(0, height - 1)))
        .map(|via| {
            let v = doubled(via);
            let risk = exposure(hazards, a, v) + exposure(hazards, v, b);
            let length = from.chebyshev(via) + via.chebyshev(to);
            (via, risk, length)
        })
        .filter(|(_, risk, _)| *risk < direct)
        .min_by_key(|(via, risk, length)| (*risk, *length, frame.rank(frame.home, doubled(*via))))
        .map(|(via, _, _)| via)
}

/// Known enemies that fire at `domain`: remembered units by confidence and
/// known buildings by health, each reaching its weapon range plus clearance.
/// A site in sight cannot fire yet; a remembered one may have been finished
/// since it was seen.
pub(crate) fn hazards(
    observation: &ObservationData,
    map: &MapModel,
    memory: &Memory,
    domain: Domain,
) -> Vec<Hazard> {
    let now = observation.tick;
    let home = map
        .start(observation.me)
        .and_then(|start| map.component(start));
    let reach = |weapons: &[oxide_sim::stats::WeaponStats]| {
        weapons
            .iter()
            .filter(|weapon| weapon.targets.covers(domain))
            .map(|weapon| weapon.range.ceil().to_num::<i32>())
            .max()
    };
    let units = memory.units().iter().filter_map(|unit| {
        let range = reach(unit.kind.stats().weapons)?;
        Some(Hazard {
            centre: doubled(unit.tile),
            reach: i64::from(2 * (range + CLEARANCE)),
            value: super::remembered(unit, map, home, now),
        })
    });
    let buildings = observation.enemy_buildings.iter().filter_map(|building| {
        if !building.built && building.seen {
            return None;
        }
        let range = reach(building.kind.tier_stats(building.tier).weapons)?;
        let (width, height) = building.kind.base_stats().size;
        Some(Hazard {
            centre: footprint_centre(building.kind, building.anchor),
            reach: i64::from(2 * (range + CLEARANCE) + width.max(height)),
            value: building_value(building),
        })
    });
    units.chain(buildings).collect()
}

/// Value of the hazards whose reach the segment from `a` to `b` crosses, in
/// doubled coordinates. Exact integer arithmetic keeps it mirror-symmetric.
pub(super) fn exposure(hazards: &[Hazard], a: (i64, i64), b: (i64, i64)) -> u64 {
    hazards
        .iter()
        .filter(|hazard| meets(hazard, a, b))
        .map(|hazard| hazard.value.max(1))
        .sum()
}

fn meets(hazard: &Hazard, a: (i64, i64), b: (i64, i64)) -> bool {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (px, py) = (hazard.centre.0 - a.0, hazard.centre.1 - a.1);
    let reach2 = hazard.reach * hazard.reach;
    let length2 = dx * dx + dy * dy;
    let along = px * dx + py * dy;
    if length2 == 0 || along <= 0 {
        return px * px + py * py <= reach2;
    }
    if along >= length2 {
        return distance2(hazard.centre, b) <= reach2;
    }
    let cross = px * dy - py * dx;
    cross * cross <= reach2 * length2
}

fn distance2(a: (i64, i64), b: (i64, i64)) -> i64 {
    (a.0 - b.0).pow(2) + (a.1 - b.1).pow(2)
}
