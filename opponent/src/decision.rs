//! One decision: the precedence it follows, its running total and its
//! unit-order allowance.

use crate::frame::{HomeFrame, footprint_centre};
use crate::map::MapModel;
use crate::trace::Purchase;
use crate::workers;
use oxide_sim::observation::{BuildingObs, ObservationData};
use oxide_sim::scenario::BotDifficulty;
use oxide_sim::{BuildingId, BuildingKind, Command, PlayerCommand, PlayerId, UnitKind};

/// What one decision emits.
#[derive(Default)]
pub(crate) struct Decision {
    pub(crate) commands: Vec<PlayerCommand>,
    pub(crate) spent: u32,
    pub(crate) purchases: Vec<Purchase>,
    pub(crate) unit_orders: u32,
    pub(crate) allowance: u32,
}

/// The decision's running total and unit-order allowance. It lives only while
/// one decision runs.
pub(crate) struct Ledger {
    me: PlayerId,
    bank: u32,
    decision: Decision,
}

impl Ledger {
    fn new(me: PlayerId, bank: u32, allowance: u32) -> Self {
        Self {
            me,
            bank,
            decision: Decision {
                allowance,
                ..Decision::default()
            },
        }
    }

    /// Scrap this decision has not committed.
    pub(crate) fn spendable(&self) -> u32 {
        self.bank - self.decision.spent
    }

    /// Queues `kind` at `building` when the running total covers it.
    pub(crate) fn train(&mut self, building: BuildingId, kind: UnitKind) -> bool {
        let cost = kind.stats().cost;
        if self.spendable() < cost {
            return false;
        }
        self.decision.spent += cost;
        self.decision.purchases.push(Purchase { building, kind });
        self.push(Command::Train { building, kind });
        true
    }

    /// Issues one unit order while the allowance lasts. Purchases never count
    /// against it.
    pub(crate) fn order(&mut self, command: Command) -> bool {
        if self.decision.unit_orders == self.decision.allowance {
            return false;
        }
        self.decision.unit_orders += 1;
        self.push(command);
        true
    }

    /// Whether this decision already queued something at `building`.
    pub(crate) fn queued_at(&self, building: BuildingId) -> bool {
        self.decision
            .purchases
            .iter()
            .any(|purchase| purchase.building == building)
    }

    /// Units of `kind` this decision queued.
    pub(crate) fn queued(&self, kind: UnitKind) -> usize {
        self.decision
            .purchases
            .iter()
            .filter(|purchase| purchase.kind == kind)
            .count()
    }

    fn push(&mut self, command: Command) {
        self.decision.commands.push(PlayerCommand {
            player: self.me,
            command,
        });
    }
}

/// A built own Foundry, and whether its queue was empty when the decision
/// began.
#[derive(Clone, Copy)]
pub(crate) struct Foundry<'a> {
    pub(crate) building: &'a BuildingObs,
    pub(crate) idle: bool,
}

/// Emergencies first, then workers, then production. Missions will sit
/// between workers and production.
pub(crate) fn decide(
    observation: &ObservationData,
    map: &MapModel,
    difficulty: BotDifficulty,
) -> Decision {
    let mut ledger = Ledger::new(observation.me, observation.scrap, allowance(difficulty));
    let Some(frame) = HomeFrame::of(observation, map) else {
        return ledger.decision;
    };
    let foundries = foundries(observation, frame);
    workers::recover(observation, &foundries, &mut ledger);
    workers::run(observation, map, frame, &foundries, &mut ledger);
    produce(&foundries, &mut ledger);
    ledger.decision
}

/// Built Foundries, nearest home first.
fn foundries(observation: &ObservationData, frame: HomeFrame) -> Vec<Foundry<'_>> {
    let mut foundries: Vec<Foundry<'_>> = observation
        .my_buildings
        .iter()
        .zip(&observation.my_queues)
        .filter(|(building, _)| building.kind == BuildingKind::Foundry && building.built)
        .map(|(building, queue)| Foundry {
            building,
            idle: queue.is_empty(),
        })
        .collect();
    foundries.sort_by_key(|foundry| {
        let centre = footprint_centre(BuildingKind::Foundry, foundry.building.anchor);
        (frame.rank(frame.home, centre), foundry.building.id)
    });
    foundries
}

/// Queues a Sentinel at every Foundry that is still idle.
fn produce(foundries: &[Foundry<'_>], ledger: &mut Ledger) {
    for foundry in foundries {
        if foundry.idle && !ledger.queued_at(foundry.building.id) {
            ledger.train(foundry.building.id, UnitKind::Sentinel);
        }
    }
}

/// Unit orders one decision may issue.
fn allowance(difficulty: BotDifficulty) -> u32 {
    match difficulty {
        BotDifficulty::Scrapheap => 3,
        BotDifficulty::Standard => 6,
        BotDifficulty::Veteran => 8,
        BotDifficulty::Prime => 10,
    }
}
