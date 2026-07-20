//! Events: what a tick did, reported outward.
//!
//! Events are *output only* — the sim never reads them back, so they can be
//! dropped freely (headless fast-forward ignores them; the shell turns them
//! into flashes and sounds; tests assert on them). They are not part of the
//! state hash.

use crate::command::RejectReason;
use crate::ids::{BuildingId, PlayerId, Target, UnitId};
use crate::state::GameResult;
use crate::stats::UnitKind;
use chassis::Tick;
use chassis::fx::Vec2Fx;
use chassis::grid::TilePos;
use serde::{Deserialize, Serialize};

/// Something noteworthy that happened during a tick.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// A Foundry finished a unit.
    UnitTrained {
        /// The new unit.
        unit: UnitId,
        /// Its kind.
        kind: UnitKind,
        /// Its owner.
        player: PlayerId,
    },
    /// A unit was destroyed.
    UnitDied {
        /// The casualty.
        unit: UnitId,
        /// Its kind.
        kind: UnitKind,
        /// Its owner.
        player: PlayerId,
        /// Where it died (for shell effects).
        pos: Vec2Fx,
    },
    /// A building was destroyed.
    BuildingDestroyed {
        /// The casualty.
        building: BuildingId,
        /// Its owner.
        player: PlayerId,
        /// Its center (for shell effects).
        pos: Vec2Fx,
    },
    /// An attack landed this tick. Positions ride along because the victim
    /// may be gone by the time a renderer resolves the ids — a lethal hit
    /// deserves its beam too.
    AttackHit {
        /// Who fired.
        attacker: UnitId,
        /// Who was hit.
        target: Target,
        /// Muzzle position at fire time.
        attacker_pos: Vec2Fx,
        /// Impact point at fire time.
        target_pos: Vec2Fx,
    },
    /// A harvester delivered its load.
    ScrapDeposited {
        /// The receiving player.
        player: PlayerId,
        /// Scrap added to the bank.
        amount: u32,
    },
    /// A scrap node ran out.
    NodeDepleted {
        /// The now-empty tile.
        pos: TilePos,
    },
    /// A turret fired (positions carried for presentation, like
    /// [`Event::AttackHit`]).
    TurretFired {
        /// The firing turret.
        turret: crate::ids::BuildingId,
        /// The unit hit.
        target: crate::ids::UnitId,
        /// Muzzle position.
        turret_pos: chassis::fx::Vec2Fx,
        /// Impact position.
        target_pos: chassis::fx::Vec2Fx,
    },
    /// A construction site reached full function.
    BuildingCompleted {
        /// The finished building.
        building: crate::ids::BuildingId,
        /// Its owner.
        player: crate::ids::PlayerId,
        /// What it is.
        kind: crate::stats::BuildingKind,
    },
    /// An unfinished site was scrapped; the owner salvaged part of the
    /// price back.
    BuildCancelled {
        /// The removed site.
        building: crate::ids::BuildingId,
        /// Its owner.
        player: crate::ids::PlayerId,
        /// Scrap returned (cost x hp / max_hp at cancel time).
        refund: u32,
    },
    /// A command was dropped instead of applied.
    CommandRejected {
        /// The issuer.
        player: PlayerId,
        /// Why.
        reason: RejectReason,
    },
    /// A unit silently gave up an order because no route exists. Fired so
    /// shells can say so instead of leaving the player wondering.
    OrderStalled {
        /// The unit that gave up.
        unit: UnitId,
        /// Its owner (shells filter to the local player).
        player: PlayerId,
        /// Where it stood when it gave up.
        pos: Vec2Fx,
    },
    /// The match ended.
    GameOver {
        /// The final outcome.
        result: GameResult,
    },
}

/// Everything [`crate::State::tick`] has to say about one tick.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TickReport {
    /// The tick that just ran (i.e. `state.tick` before the call).
    pub tick: Tick,
    /// Events in the order they occurred.
    pub events: Vec<Event>,
}
