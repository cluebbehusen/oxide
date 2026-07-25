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
        /// Append behind the units' current orders instead of replacing
        /// them (shift-click in the shell).
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        queue: bool,
    },
    /// Attack one enemy. Units that cannot fight walk there instead.
    /// Rejected unless the issuer can currently see the target.
    Attack {
        /// The units to commit.
        units: Vec<UnitId>,
        /// An enemy unit or building.
        target: Target,
        /// Append instead of replace (see [`Command::Move::queue`]).
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        queue: bool,
    },
    /// March to a tile engaging everything on the way. Units that cannot
    /// fight walk there obliviously instead.
    AttackMove {
        /// The units to commit.
        units: Vec<UnitId>,
        /// Destination tile (snapped like a move goal).
        goal: TilePos,
        /// Append instead of replace (see [`Command::Move::queue`]).
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        queue: bool,
    },
    /// Put harvesters to work on a scrap node.
    Harvest {
        /// The units to commit (only harvesters are accepted).
        units: Vec<UnitId>,
        /// A tile that currently holds scrap.
        node: TilePos,
        /// Append instead of replace (see [`Command::Move::queue`]).
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        queue: bool,
    },
    /// Walk a looping circuit of waypoints, engaging everything met on the
    /// way (legs are attack-moves). The route repeats until another
    /// command replaces it.
    Patrol {
        /// The units to commit.
        units: Vec<UnitId>,
        /// The circuit, visited in order and then from the top.
        waypoints: Vec<TilePos>,
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
    /// Start a construction site and send a harvester to stand it up.
    /// The full price is paid on placement; cancelling salvages
    /// `cost x hp / max_hp`.
    Build {
        /// Candidate builders. Fresh placement sends the first accepted
        /// harvester; resuming an existing site sends every accepted
        /// harvester (builders stack).
        units: Vec<UnitId>,
        /// What to construct.
        kind: crate::stats::BuildingKind,
        /// Top-left tile of the footprint.
        anchor: TilePos,
        /// Append behind current orders instead of replacing them.
        /// Payment and the ground claim still happen NOW — the queue
        /// defers only the walk-and-work leg.
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        queue: bool,
    },
    /// Scrap an own unfinished site for a partial refund.
    Cancel {
        /// The site to abandon.
        building: BuildingId,
    },
    /// Send harvesters to weld a damaged own built building back toward
    /// full. Repair bills per hp welded, prepaid at whole-scrap
    /// boundaries.
    Repair {
        /// The units to commit (only harvesters are accepted).
        units: Vec<UnitId>,
        /// The patient.
        building: BuildingId,
        /// Append behind current orders instead of replacing them.
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        queue: bool,
    },
    /// Send harvesters to strip an own BUILT building for a partial
    /// refund, as labor (unbuilt sites keep [`Command::Cancel`];
    /// Foundries refuse). Issuing this clears repair orders on the same
    /// building — the two verbs never share a target.
    Salvage {
        /// The units to commit (only harvesters are accepted).
        units: Vec<UnitId>,
        /// The building coming down.
        building: BuildingId,
        /// Append behind current orders instead of replacing them.
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        queue: bool,
    },
    /// Remove one queued unit from a producer, refunding its full cost
    /// (scrap was paid on enqueue; training spends only time). Cancelling
    /// the head also resets its progress.
    CancelTrain {
        /// The producing building.
        building: BuildingId,
        /// Queue position (0 = currently training).
        index: u8,
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
    /// The building can't train that unit kind (or isn't finished yet).
    CannotProduce,
    /// The footprint isn't fully explored, open, and unoccupied — or the
    /// kind isn't buildable at all.
    BadSite,
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
    /// The unit kind belongs to the other faction's roster.
    WrongFaction,
}
