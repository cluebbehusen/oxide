//! Seat orientation: the fairness transform.
//!
//! Every deterministic tie-break in a policy — ring-scan order, `(y, x)`
//! sort keys, "lean toward the enemy" arithmetic — has a compass
//! direction baked in, and on a 180°-symmetric map the two seats
//! experience those directions differently: the northwest seat's
//! placement scan probes its own rear while the southeast seat's probes
//! its front line. Chasing each skew individually is endless; instead
//! the brain *orients* its world. A policy whose home sits in the
//! flipped half sees a flipped observation, thinks exactly the logic
//! its opponent thinks, and its intents are flipped back on the way
//! out. Policy-level seat-symmetry by construction: measured on mirror
//! matches, it removed a 20/0 policy-side sweep (what remains is the
//! sim's own id-order micro, tracked in AGENTS as the open seat issue).
//!
//! The flip is per-axis (x when home is in the east half, y when in the
//! south half), which also orients the corner seats of future 4-player
//! maps.

use super::executive::Intent;
use super::observation::Observation;
use chassis::grid::TilePos;

/// Which axes a brain flips to think in home-in-the-northwest space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Orientation {
    flip_x: bool,
    flip_y: bool,
    width: i32,
    height: i32,
}

impl Orientation {
    /// Orientation for a brain whose home footprint anchors at `home`
    /// on a `width` × `height` map: flip whichever axes put home in the
    /// southeast, so the policy always reasons from the northwest.
    pub fn for_home(obs: &Observation, home: TilePos) -> Self {
        Self {
            flip_x: 2 * home.x >= obs.map_width,
            flip_y: 2 * home.y >= obs.map_height,
            width: obs.map_width,
            height: obs.map_height,
        }
    }

    /// True when this orientation changes anything at all.
    pub fn is_identity(&self) -> bool {
        !self.flip_x && !self.flip_y
    }

    /// Maps a tile into (or back out of — the flip is an involution)
    /// oriented space.
    pub fn tile(&self, t: TilePos) -> TilePos {
        TilePos::new(
            if self.flip_x {
                self.width - 1 - t.x
            } else {
                t.x
            },
            if self.flip_y {
                self.height - 1 - t.y
            } else {
                t.y
            },
        )
    }

    /// Maps a footprint anchor (top-left) of a `size` building: flipping
    /// a span moves its anchor to what was its far corner.
    pub fn anchor(&self, a: TilePos, size: (i32, i32)) -> TilePos {
        TilePos::new(
            if self.flip_x {
                self.width - size.0 - a.x
            } else {
                a.x
            },
            if self.flip_y {
                self.height - size.1 - a.y
            } else {
                a.y
            },
        )
    }

    /// A copy of the observation with every position oriented. Sorted
    /// fields are re-sorted so iteration order is oriented too — that
    /// is the point.
    pub fn observe(&self, obs: &Observation) -> Observation {
        if self.is_identity() {
            return obs.clone();
        }
        let mut o = obs.clone();
        for u in o
            .my_units
            .iter_mut()
            .chain(o.ally_units.iter_mut())
            .chain(o.enemy_units.iter_mut())
        {
            u.tile = self.tile(u.tile);
        }
        for b in o
            .my_buildings
            .iter_mut()
            .chain(o.ally_buildings.iter_mut())
            .chain(o.enemy_buildings.iter_mut())
        {
            b.anchor = self.anchor(b.anchor, {
                let (w, h) = b.kind.stats().size;
                (w, h)
            });
        }
        for (pos, _) in o.known_scrap.iter_mut().chain(o.known_wrecks.iter_mut()) {
            *pos = self.tile(*pos);
        }
        for pos in o
            .known_rock
            .iter_mut()
            .chain(o.blips.iter_mut())
            .chain(o.incoming_shells.iter_mut())
        {
            *pos = self.tile(*pos);
        }
        o.known_scrap.sort_by_key(|(p, _)| (p.y, p.x));
        o.known_wrecks.sort_by_key(|(p, _)| (p.y, p.x));
        o.known_rock.sort_by_key(|p| (p.y, p.x));
        o.blips.sort_by_key(|p| (p.y, p.x));
        o.incoming_shells.sort_by_key(|p| (p.y, p.x));
        o.enemy_buildings
            .sort_by_key(|b| (b.anchor.y, b.anchor.x, b.player));
        o
    }

    /// An army as the oriented policy should see it.
    pub fn army(&self, mut a: super::executive::Army) -> super::executive::Army {
        a.staging = self.tile(a.staging);
        a.target = a.target.map(|t| self.tile(t));
        a
    }

    /// Maps a think's intents back into world space.
    pub fn emit(&self, intents: Vec<Intent>) -> Vec<Intent> {
        if self.is_identity() {
            return intents;
        }
        intents
            .into_iter()
            .map(|i| match i {
                Intent::Build { kind, anchor } => Intent::Build {
                    kind,
                    anchor: self.anchor(anchor, {
                        let (w, h) = kind.stats().size;
                        (w, h)
                    }),
                },
                Intent::FormArmy { staging, size } => Intent::FormArmy {
                    staging: self.tile(staging),
                    size,
                },
                Intent::PushArmy { army, target } => Intent::PushArmy {
                    army,
                    target: self.tile(target),
                },
                Intent::AssignHarvest { unit, node } => Intent::AssignHarvest {
                    unit,
                    node: self.tile(node),
                },
                Intent::Scout { unit, to } => Intent::Scout {
                    unit,
                    to: self.tile(to),
                },
                Intent::RaidAir { target } => Intent::RaidAir {
                    target: self.tile(target),
                },
                // Positionless intents pass through — and the match stays
                // exhaustive on purpose: a new positioned intent that
                // slips through unflipped is a silent seat-bias
                // regression, so adding a variant must break this match.
                keep @ (Intent::TrainAt { .. }
                | Intent::RecallArmy { .. }
                | Intent::Repair { .. }
                | Intent::Salvage { .. }
                | Intent::RepairUnit { .. }) => keep,
            })
            .collect()
    }
}
