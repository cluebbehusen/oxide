//! Opt-in decision diagnostics. Traces are output only: never controller
//! memory, simulation state or replay input.

use crate::OwnEvent;
use crate::investments::{Investment, Step};
use crate::missions::MissionStatus;
use chassis::grid::TilePos;
use oxide_sim::{BuildingId, BuildingKind, PlayerId, UnitKind};
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
    /// Purchases, in command order.
    pub purchases: Vec<Purchase>,
    /// Orders issued to units, not counting purchases.
    pub unit_orders: u32,
    /// Unit orders the decision could have issued.
    pub allowance: u32,
    /// The investment the seat was saving for.
    pub target: Option<SavingTarget>,
    /// Scrap held back from ordinary spending after the decision.
    pub protected: u32,
    /// Missions after the decision, by id.
    pub missions: Vec<MissionStatus>,
}

/// One purchase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "purchase", rename_all = "snake_case")]
pub enum Purchase {
    /// A unit queued at a producer.
    Train {
        /// The producing building.
        building: BuildingId,
        /// The unit queued there.
        unit: UnitKind,
    },
    /// A building placed.
    Build {
        /// What was placed.
        building: BuildingKind,
        /// Its footprint anchor.
        anchor: TilePos,
    },
    /// A building upgraded.
    Upgrade {
        /// The upgraded building.
        building: BuildingId,
    },
}

/// A saving target as one decision saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SavingTarget {
    /// What the seat was saving for.
    pub investment: Investment,
    /// The next purchase toward it, unless a prerequisite was still being
    /// built.
    pub next: Option<NextPurchase>,
}

/// The next purchase toward a saving target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct NextPurchase {
    /// The purchase.
    pub step: Step,
    /// Its price.
    pub price: u32,
}
