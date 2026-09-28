//! Opt-in decision diagnostics. Traces are output only: never controller
//! memory, simulation state or replay input.

use crate::OwnEvent;
use oxide_sim::{BuildingId, PlayerId, UnitKind};
use serde::Serialize;

/// What one decision saw and did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Trace {
    /// Simulation tick the decision observed.
    pub tick: u64,
    /// Seat that decided.
    pub player: PlayerId,
    /// Scrap on hand when the decision started.
    pub bank: u32,
    /// The seat's own order failures since its previous decision, oldest first.
    pub events: Vec<OwnEvent>,
    /// Scrap the decision's purchases committed.
    pub spent: u32,
    /// Units queued, in command order.
    pub purchases: Vec<Purchase>,
    /// Orders issued to units, not counting purchases.
    pub unit_orders: u32,
}

/// One queued unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Purchase {
    /// The producing building.
    pub building: BuildingId,
    /// The unit queued there.
    pub kind: UnitKind,
}
