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
    /// A building was taken apart by its own crew — deliberate, not a
    /// loss: no wreck, no scorch, and stat screens must not count it
    /// among casualties.
    BuildingSalvaged {
        /// The dismantled building.
        building: BuildingId,
        /// Its owner (also the refund's recipient).
        player: PlayerId,
        /// Its center (for shell effects).
        pos: Vec2Fx,
        /// Total scrap the whole salvage credited, refunded production
        /// queue excluded.
        refund: u32,
    },
    /// An attack landed this tick. Positions ride along because the victim
    /// may be gone by the time a renderer resolves the ids — a lethal hit
    /// deserves its beam too.
    AttackHit {
        /// Who fired.
        attacker: UnitId,
        /// What fired — carried in the event because presentation may run
        /// after the attacker itself died this tick.
        attacker_kind: crate::stats::UnitKind,
        /// Which weapon slot fired (0 = primary), so presentation reads
        /// the exact weapon's stats instead of guessing across the list.
        weapon: usize,
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
        /// What kind of turret — carried in the event because the
        /// building may be rubble by the time presentation looks it up
        /// (a Bastion destroyed the tick it fires still booms).
        kind: crate::stats::BuildingKind,
        /// The unit hit.
        target: crate::ids::UnitId,
        /// Muzzle position.
        turret_pos: chassis::fx::Vec2Fx,
        /// Impact position.
        target_pos: chassis::fx::Vec2Fx,
    },
    /// A shell left its gun: real flight, resolving at `arrival`.
    ShellLaunched {
        /// The gun itself — a unit's mount or a building's emplacement;
        /// presentation turns it toward the work.
        shooter: crate::ids::Target,
        /// The firing seat.
        player: PlayerId,
        /// Muzzle position.
        from: Vec2Fx,
        /// Where it will land.
        to: Vec2Fx,
        /// Ticks of flight ahead of it.
        flight: u64,
    },
    /// A shell arrived and resolved.
    ShellLanded {
        /// The seat that fired it — presentation uses this to tell a
        /// hostile impact from a harmless friendly one. Never names a
        /// sufferer; the impact itself was already visible by the arc
        /// renderer's rule.
        player: PlayerId,
        /// Which movement domains the splash can hurt — a ground-only
        /// shell near a flyer is noise, not an attack.
        targets: crate::stats::DomainMask,
        /// Impact point.
        at: Vec2Fx,
        /// Splash radius, when the shell splashes.
        splash: Option<chassis::fx::Fx>,
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
        /// Why the program died — own-state facts only (routes, banks,
        /// footing); a reason must never reveal what fog hides.
        reason: StallReason,
    },
    /// The match ended.
    GameOver {
        /// The final outcome.
        result: GameResult,
    },
    /// A seat conceded. Its machines play on as remnants; the victory
    /// check stops counting its Foundries the same tick.
    PlayerResigned {
        /// The seat that gave up.
        player: PlayerId,
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

/// Why an order program stalled. Every variant derives from the acting
/// player's own situation — pathing, funds, footing — never from
/// hidden enemy state, so the shell can voice any of these verbatim
/// without leaking intelligence through the fog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StallReason {
    /// No route to the goal (or to any doorstep of the work site).
    NoRoute,
    /// No standable tile within weapon reach of the victim.
    NoFiringPosition,
    /// The bank ran dry mid-job.
    InsufficientScrap,
}
