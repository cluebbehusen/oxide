//! Which order of a unit's program a player points at.

use super::{Order, State};
use crate::ids::{AttackTarget, BuildingId, PlayerId, UnitId};
use crate::stats::BuildingKind;
use chassis::grid::TilePos;
use serde::{Deserialize, Serialize};

/// An order's identity as its owner sees it, for naming one order of a
/// program in [`crate::Command::CancelOrder`].
///
/// A key survives the rewrites the simulation makes to an order while it
/// runs: a walk keeps its clicked tile when its goal takes a slot or an
/// endpoint, when an attack-move stops to fight on the way, or when an
/// airframe lands in its place, and a harvest keeps its clicked source as
/// it moves between nodes. A few rewrites do change the key: a landing
/// that turns into a fight, and a planned site that becomes a build once
/// its ground is verified. One command can also leave different keys: a
/// pacifist walks where the rest of its group attacks.
///
/// Some keys can change while a cancellation is in flight. A fight a unit
/// picked for itself may hold a raw unit or building, whose key flips
/// between that and the contact or remembered footprint it normalizes to
/// as sight comes and goes, and an automatic landing's key changes when its
/// pad is planned again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "order", rename_all = "snake_case")]
pub enum OrderKey {
    /// A Move, AttackMove, or Advance to this clicked tile, the march an
    /// engagement resumes, or the walk a landing took over.
    Walk {
        /// The clicked tile.
        tile: TilePos,
    },
    /// An engagement with no march to resume.
    Attack {
        /// The objective, normalized through its owner's knowledge.
        objective: AttackTarget,
    },
    /// A landing that took over no walk.
    Land {
        /// The tile to park on.
        pad: TilePos,
    },
    /// A drop at this clicked tile.
    Unload {
        /// The clicked tile.
        tile: TilePos,
    },
    /// Work in the zone around this clicked source.
    Harvest {
        /// The clicked source.
        anchor: TilePos,
    },
    /// A cargo delivery, to whichever Foundry.
    ReturnCargo,
    /// Standing up this site.
    Build {
        /// The site.
        site: BuildingId,
    },
    /// Approaching this paid, unverified site.
    Found {
        /// What the site will be.
        kind: BuildingKind,
        /// The site's top-left tile.
        anchor: TilePos,
    },
    /// Welding this building.
    Repair {
        /// The patient.
        building: BuildingId,
    },
    /// Stripping this building.
    Salvage {
        /// The building coming down.
        building: BuildingId,
    },
    /// Welding this unit.
    RepairUnit {
        /// The patient.
        unit: UnitId,
    },
    /// Climbing aboard this transport.
    Board {
        /// The carrier.
        transport: UnitId,
    },
}

impl Order {
    /// This order's key for `player`, who owns it. An engagement's
    /// objective normalizes through `player`'s current knowledge, so an
    /// order holding a visible unit and one holding that unit's contact
    /// share a key. [`Order::Idle`] has none.
    pub fn key(&self, state: &State, player: PlayerId) -> Option<OrderKey> {
        Some(match *self {
            Order::Idle => return None,
            Order::Move { goal } | Order::AttackMove { goal } | Order::Advance { goal } => {
                OrderKey::Walk { tile: goal.tile() }
            }
            Order::Attack {
                resume: Some(goal), ..
            } => OrderKey::Walk { tile: goal.tile() },
            Order::Attack {
                target,
                resume: None,
                ..
            } => OrderKey::Attack {
                objective: state.attack_objective(player, target).unwrap_or(target),
            },
            Order::Land {
                from: Some(tile), ..
            } => OrderKey::Walk { tile },
            Order::Land { goal, from: None } => OrderKey::Land { pad: goal },
            Order::Unload { at } => OrderKey::Unload { tile: at.tile() },
            Order::Harvest { node, anchor, .. } => OrderKey::Harvest {
                anchor: anchor.unwrap_or(node),
            },
            Order::ReturnCargo { .. } => OrderKey::ReturnCargo,
            Order::Build { site } => OrderKey::Build { site },
            Order::Found { kind, anchor } => OrderKey::Found { kind, anchor },
            Order::Repair { building } => OrderKey::Repair { building },
            Order::Salvage { building } => OrderKey::Salvage { building },
            Order::RepairUnit { unit } => OrderKey::RepairUnit { unit },
            Order::Board { transport } => OrderKey::Board { transport },
        })
    }
}
