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
    /// Start a construction site and send harvesters to stand it up.
    /// The full price is paid on placement; cancelling salvages
    /// `cost x hp / max_hp`.
    Build {
        /// Candidate builders. Every accepted harvester joins the crew,
        /// fresh placement and resume alike (builders stack); on a
        /// fresh placement the first accepted harvester founds the
        /// site — it pays and proves the doorstep, and its full queue
        /// rejects the whole command.
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
        /// Claim on arrival instead of now: validate against the
        /// issuer's *knowledge* ([`crate::State::place_intent_refusal`])
        /// and hand the crew [`crate::state::Order::Found`] — nothing
        /// placed, nothing charged, no route demanded until the founder
        /// stands beside ground it can see (an honest stall later, like
        /// a Move into fog). The shell arms this for explored-but-unseen
        /// ground; the gym bot emits it exactly where the shell
        /// would (a footprint tile out of current sight); the scripted
        /// tiers never do, which is what keeps their anchors frozen.
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        defer: bool,
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
    /// Concede the seat. The issuer's Foundries stop counting toward the
    /// team-scoped victory check and its future commands reject as
    /// [`RejectReason::Eliminated`]; units already in the world keep
    /// executing their brains, like any eliminated seat's remnants.
    Surrender,
    /// Send harvesters to weld a wounded own GROUND unit back toward
    /// full. Billed per hp against the patient's cost at repair
    /// pricing, prepaid at whole-scrap boundaries like a building weld.
    /// Air patients refuse — a harvester cannot service a machine
    /// hovering where it cannot stand. The patient's own orders are
    /// untouched: a fleeing machine keeps fleeing and simply goes
    /// unwelded while out of reach. (Last variant by appending
    /// discipline: earlier discriminants keep their serialized bytes.)
    RepairUnit {
        /// The units to commit (only harvesters are accepted; the
        /// patient itself never joins its own crew).
        units: Vec<UnitId>,
        /// The wounded machine.
        target: UnitId,
        /// Append behind current orders instead of replacing them.
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        queue: bool,
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
