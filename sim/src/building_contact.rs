//! Coarse, static work surfaces. Placement and travel still use rectangular tiles.
use crate::{Building, BuildingKind, State};
use chassis::fx::{Fx, Vec2Fx};
use chassis::grid::TilePos;

/// A building's approximate ground surface, without a navigation mesh or cache.
#[derive(Clone, Copy)]
pub struct Surface {
    kind: BuildingKind,
    anchor: TilePos,
    size: (i32, i32),
}

impl Surface {
    /// Resolve the fixed outline at a building's world position.
    pub fn new(building: &Building) -> Self {
        let size = building.stats().size;
        Self {
            kind: building.kind,
            anchor: building.anchor,
            size,
        }
    }

    fn vertices(self) -> impl Iterator<Item = Vec2Fx> + Clone {
        let outline: &[(i32, i32)] = match self.kind {
            BuildingKind::Fabricator => &[
                (11, 10),
                (52, 10),
                (52, 33),
                (124, 33),
                (124, 115),
                (11, 115),
            ],
            BuildingKind::Foundry => &[
                (8, 24),
                (37, 24),
                (37, 7),
                (89, 7),
                (89, 24),
                (120, 24),
                (120, 119),
                (8, 119),
            ],
            BuildingKind::Bastion => &[(18, 28), (109, 28), (109, 111), (18, 111)],
            BuildingKind::RepairBay => &[
                (10, 25),
                (29, 14),
                (106, 14),
                (119, 26),
                (119, 105),
                (100, 117),
                (27, 117),
                (10, 107),
            ],
            BuildingKind::Turret => &[(14, 25), (114, 25), (114, 110), (14, 110)],
            BuildingKind::FlakTurret => &[(14, 21), (114, 21), (114, 114), (14, 114)],
            BuildingKind::Array => &[(17, 19), (111, 19), (111, 110), (17, 110)],
            BuildingKind::Barricade => &[(0, 26), (128, 26), (128, 111), (0, 111)],
            BuildingKind::ScuttleCharge => &[(23, 36), (105, 36), (105, 94), (23, 94)],
            _ => &[(8, 8), (120, 8), (120, 120), (8, 120)],
        };
        outline.iter().map(move |&(x, y)| {
            Vec2Fx::new(
                Fx::from_num(self.anchor.x) + Fx::from_num(x * self.size.0) / 128,
                Fx::from_num(self.anchor.y) + Fx::from_num(y * self.size.1) / 128,
            )
        })
    }

    fn edges(self) -> impl Iterator<Item = (Vec2Fx, Vec2Fx)> {
        let vertices = self.vertices();
        vertices.clone().zip(vertices.cycle().skip(1))
    }

    /// Nearest coarse surface, with ties relative to the incoming direction.
    pub fn closest(self, from: Vec2Fx) -> Vec2Fx {
        let center = Vec2Fx::new(Fx::from_num(self.anchor.x), Fx::from_num(self.anchor.y))
            + Vec2Fx::new(Fx::from_num(self.size.0), Fx::from_num(self.size.1)) / Fx::from_num(2);
        self.edges()
            .map(|(a, b)| closest_on_segment(from, a, b))
            .min_by_key(|p| (from.dist_sq(*p), cross(from - center, *p - center)))
            .expect("nonempty outline")
    }

    /// Final contact point approached from an ordinary perimeter tile.
    pub fn stance(self, from: Vec2Fx, clearance: Fx) -> Vec2Fx {
        let hit = self.closest(from);
        let outward = from - hit;
        if outward == Vec2Fx::ZERO {
            from
        } else {
            hit + outward * (clearance / outward.length())
        }
    }

    fn contains(self, point: Vec2Fx) -> bool {
        let mut inside = false;
        for (a, b) in self.edges() {
            if (a.y > point.y) != (b.y > point.y)
                && point.x < a.x + (point.y - a.y) * (b.x - a.x) / (b.y - a.y)
            {
                inside = !inside;
            }
        }
        inside
    }

    /// A short, swept chassis movement cannot cross the occupied body.
    pub(crate) fn clear(self, from: Vec2Fx, to: Vec2Fx, radius: Fx) -> bool {
        if from == to {
            return !self.contains(from) && from.dist_sq(self.closest(from)) >= radius * radius;
        }
        !self.contains(from)
            && !self.contains(to)
            && self.edges().all(|(a, b)| {
                if segments_cross(from, to, a, b) {
                    return false;
                }
                [
                    from.dist_sq(closest_on_segment(from, a, b)),
                    to.dist_sq(closest_on_segment(to, a, b)),
                    a.dist_sq(closest_on_segment(a, from, to)),
                    b.dist_sq(closest_on_segment(b, from, to)),
                ]
                .into_iter()
                .all(|d| d >= radius * radius)
            })
    }

    pub(crate) fn covers(self, tile: TilePos) -> bool {
        tile.x >= self.anchor.x
            && tile.x < self.anchor.x + self.size.0
            && tile.y >= self.anchor.y
            && tile.y < self.anchor.y + self.size.1
    }
}

fn cross(a: Vec2Fx, b: Vec2Fx) -> Fx {
    a.x * b.y - a.y * b.x
}
fn dot(a: Vec2Fx, b: Vec2Fx) -> Fx {
    a.x * b.x + a.y * b.y
}
fn closest_on_segment(p: Vec2Fx, a: Vec2Fx, b: Vec2Fx) -> Vec2Fx {
    let d = b - a;
    if dot(d, d) == Fx::ZERO {
        a
    } else {
        a + d * (dot(p - a, d) / dot(d, d)).clamp(Fx::ZERO, Fx::ONE)
    }
}
fn segments_cross(a: Vec2Fx, b: Vec2Fx, c: Vec2Fx, d: Vec2Fx) -> bool {
    let opposite = |x: Fx, y: Fx| (x < Fx::ZERO && y > Fx::ZERO) || (x > Fx::ZERO && y < Fx::ZERO);
    opposite(cross(b - a, c - a), cross(b - a, d - a))
        && opposite(cross(d - c, a - c), cross(d - c, b - c))
}

impl State {
    /// Whether a worker's tool can reach the actual building surface.
    pub fn in_building_work_reach(&self, unit: &crate::Unit, building: crate::BuildingId) -> bool {
        let Some(b) = self.building(building) else {
            return false;
        };
        let reach =
            unit.kind.stats().radius + crate::stats::WORK_FOOTPRINT_GAP + crate::stats::WORK_REACH
                - crate::stats::WORK_APPROACH_GAP
                + Fx::lit("0.04");
        let surface = self.contact_surface(b);
        unit.pos.dist_sq(surface.closest(unit.pos)) <= reach * reach
            && surface.clear(unit.pos, unit.pos, unit.kind.stats().radius)
    }

    /// Contact artwork and tools use this surface; ranged distance remains rectangular.
    pub fn contact_surface(&self, building: &Building) -> Surface {
        Surface::new(building)
    }
}
