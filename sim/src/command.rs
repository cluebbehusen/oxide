//! Player commands — the sim's only input.
//!
//! Whatever produces a command (mouse clicks, a bot, an agent on the debug
//! socket, a replay file), it arrives here as the same data, stamped with the
//! tick it executes on. That single funnel is what makes any session
//! reproducible.
//!
//! Commands are *requests*: the sim validates them against the current state
//! and drops the invalid ones with a [`crate::Event::CommandRejected`], never
//! with a panic — a stale command (target died in transit) is normal RTS
//! traffic, not a bug.

use crate::ids::{BuildingId, PlayerId, Target, UnitId};
use crate::stats::UnitKind;
use chassis::grid::TilePos;
use serde::{Deserialize, Serialize};

/// An instruction from one player.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    /// Send units walking to a tile. Impassable goals snap to the nearest
    /// open tile within [`crate::stats::GOAL_SNAP_RADIUS`].
    Move {
        /// The units to move (non-owned and dead ids are skipped).
        units: Vec<UnitId>,
        /// Destination tile.
        goal: TilePos,
    },
    /// Attack one enemy. Units that cannot fight walk there instead.
    /// Rejected unless the issuer can currently see the target.
    Attack {
        /// The units to commit.
        units: Vec<UnitId>,
        /// An enemy unit or building.
        target: Target,
    },
    /// March to a tile engaging everything on the way. Units that cannot
    /// fight walk there obliviously instead.
    AttackMove {
        /// The units to commit.
        units: Vec<UnitId>,
        /// Destination tile (snapped like a move goal).
        goal: TilePos,
    },
    /// Put harvesters to work on a scrap node.
    Harvest {
        /// The units to commit (only harvesters are accepted).
        units: Vec<UnitId>,
        /// A tile that currently holds scrap.
        node: TilePos,
    },
    /// Clear orders; units stop in place.
    Stop {
        /// The units to halt.
        units: Vec<UnitId>,
    },
    /// Queue a unit at a Foundry. Scrap is deducted on enqueue.
    Train {
        /// The producing building.
        building: BuildingId,
        /// What to build.
        kind: UnitKind,
    },
    /// Point a building's fresh units somewhere (`None` clears the rally).
    SetRally {
        /// The building.
        building: BuildingId,
        /// Rally tile, or `None` for the doorstep default.
        rally: Option<TilePos>,
    },
}

/// A command attributed to its issuing player. Ownership checks are made
/// against this player, so the shell/driver must attribute honestly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerCommand {
    /// Who issued it.
    pub player: PlayerId,
    /// What they asked for.
    pub command: Command,
}

/// Why a command was dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    /// No referenced unit was alive and owned by the issuer.
    NoValidUnits,
    /// The referenced building is gone or not the issuer's.
    NotYourBuilding,
    /// The target entity is gone, or is not an enemy.
    InvalidTarget,
    /// The tile can't be walked to (no open tile near it).
    UnreachableGoal,
    /// A coordinate lies outside the map's command envelope. Hostile or
    /// corrupt input — honest clients clamp to the map.
    OutOfBounds,
    /// The issuer has been eliminated (no buildings left); spectators
    /// don't give orders.
    Eliminated,
    /// The named tile holds no scrap.
    NotANode,
    /// Not enough scrap banked.
    NotEnoughScrap,
    /// The production queue is at capacity.
    QueueFull,
}
