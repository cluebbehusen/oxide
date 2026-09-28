//! One decision: the precedence it follows, its running total and its
//! unit-order allowance.

use crate::frame::{HomeFrame, footprint_centre};
use crate::income::Income;
use crate::investments::{self, Situation, Step};
use crate::map::MapModel;
use crate::memory::Memory;
use crate::placement;
use crate::profile::ResolvedProfile;
use crate::saving::Saving;
use crate::trace::{NextPurchase, Purchase, SavingTarget};
use crate::workers;
use chassis::grid::TilePos;
use oxide_sim::observation::{BuildingObs, ObservationData};
use oxide_sim::scenario::{BotDifficulty, BotStance};
use oxide_sim::{BuildingId, BuildingKind, Command, PlayerCommand, PlayerId, UnitId, UnitKind};

/// What one decision emits.
#[derive(Default)]
pub(crate) struct Decision {
    pub(crate) commands: Vec<PlayerCommand>,
    pub(crate) spent: u32,
    pub(crate) purchases: Vec<Purchase>,
    pub(crate) unit_orders: u32,
    pub(crate) allowance: u32,
    pub(crate) target: Option<SavingTarget>,
    pub(crate) protected: u32,
}

/// What carries from one decision to the next. Checkpoints save exactly this.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Persistent {
    pub(crate) memory: Memory,
    pub(crate) income: Income,
    pub(crate) saving: Saving,
}

/// The decision's running total and unit-order allowance. It lives only while
/// one decision runs.
pub(crate) struct Ledger {
    me: PlayerId,
    bank: u32,
    protected: u32,
    planned: Vec<(BuildingKind, TilePos)>,
    decision: Decision,
}

impl Ledger {
    fn new(me: PlayerId, bank: u32, allowance: u32) -> Self {
        Self {
            me,
            bank,
            protected: 0,
            planned: Vec::new(),
            decision: Decision {
                allowance,
                ..Decision::default()
            },
        }
    }

    /// Scrap this decision has not committed.
    fn available(&self) -> u32 {
        self.bank - self.decision.spent
    }

    /// Uncommitted scrap that is not protected for the saving target.
    pub(crate) fn spendable(&self) -> u32 {
        let available = self.available();
        available - self.protected.min(available)
    }

    /// Queues `kind` at `building` from unprotected scrap.
    pub(crate) fn train(&mut self, building: BuildingId, kind: UnitKind) -> bool {
        self.spendable() >= kind.stats().cost && self.train_from_all(building, kind)
    }

    /// Queues `kind` at `building`, spending protected scrap if it must.
    pub(crate) fn train_urgently(&mut self, building: BuildingId, kind: UnitKind) -> bool {
        self.available() >= kind.stats().cost && self.train_from_all(building, kind)
    }

    fn train_from_all(&mut self, building: BuildingId, kind: UnitKind) -> bool {
        self.decision.spent += kind.stats().cost;
        self.decision.purchases.push(Purchase::Train {
            building,
            unit: kind,
        });
        self.push(Command::Train { building, kind });
        true
    }

    fn build(
        &mut self,
        builder: UnitId,
        kind: BuildingKind,
        anchor: TilePos,
        defer: bool,
        price: u32,
    ) {
        self.decision.spent += price;
        self.decision.purchases.push(Purchase::Build {
            building: kind,
            anchor,
        });
        self.planned.push((kind, anchor));
        self.push(Command::Build {
            units: vec![builder],
            kind,
            anchor,
            queue: false,
            defer,
        });
    }

    fn upgrade(&mut self, building: BuildingId, price: u32) {
        self.decision.spent += price;
        self.decision.purchases.push(Purchase::Upgrade { building });
        self.push(Command::UpgradeBuilding { building });
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
        self.decision.purchases.iter().any(
            |purchase| matches!(purchase, Purchase::Train { building: at, .. } if *at == building),
        )
    }

    /// Units of `kind` this decision queued.
    pub(crate) fn queued(&self, kind: UnitKind) -> usize {
        self.decision
            .purchases
            .iter()
            .filter(|purchase| matches!(purchase, Purchase::Train { unit, .. } if *unit == kind))
            .count()
    }

    /// Whether this decision already gave `unit` work.
    pub(crate) fn employs(&self, unit: UnitId) -> bool {
        self.decision
            .commands
            .iter()
            .any(|command| match &command.command {
                Command::Harvest { units, .. } | Command::Build { units, .. } => {
                    units.contains(&unit)
                }
                _ => false,
            })
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

/// Emergencies first, then an affordable saving target, then workers, then
/// production. Missions will sit between workers and production.
pub(crate) fn decide(
    observation: &ObservationData,
    rejected: bool,
    map: &MapModel,
    profile: &ResolvedProfile,
    persistent: &mut Persistent,
) -> Decision {
    let mut ledger = Ledger::new(
        observation.me,
        observation.scrap,
        allowance(profile.difficulty),
    );
    let Some(frame) = HomeFrame::of(observation, map) else {
        return ledger.decision;
    };
    let tick = observation.tick;
    let foundries = foundries(observation, frame);
    let staffing = workers::staffing(observation, map, frame, &foundries);
    let earned = persistent.income.observe(tick, observation.scrap, rejected);
    persistent.memory.forget(tick);
    let situation = Situation {
        observation,
        traits: profile.traits,
        saturation: staffing.saturation(),
        income: persistent.income.per_minute(),
        depletion: depletion(observation, map),
    };
    let candidates = investments::candidates(&situation);
    let share = share(observation, profile);
    persistent.saving.settle(
        observation,
        &candidates,
        share,
        earned,
        &mut persistent.memory,
    );
    ledger.protected = persistent.saving.protected();
    let target = persistent
        .saving
        .investment()
        .map(|investment| SavingTarget {
            investment,
            next: investments::step(observation, investment)
                .map(|(step, price)| NextPurchase { step, price }),
        });

    workers::recover(observation, &foundries, &mut ledger);
    buy(observation, map, frame, persistent, &mut ledger);
    workers::run(observation, map, frame, &foundries, &staffing, &mut ledger);
    produce(&foundries, &mut ledger);

    persistent.saving.keep_at_most(ledger.available());
    persistent
        .income
        .record(tick, observation.scrap, ledger.decision.spent);
    Decision {
        target,
        protected: persistent.saving.protected(),
        ..ledger.decision
    }
}

/// Buys the saving target's next step once the whole uncommitted bank covers
/// it: an upgrade, or a building on the first home spot the seat's knowledge
/// allows, raised by the nearest free Harvester.
fn buy(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    persistent: &mut Persistent,
    ledger: &mut Ledger,
) {
    let Some(investment) = persistent.saving.investment() else {
        return;
    };
    let Some((step, price)) = investments::step(observation, investment) else {
        return;
    };
    if ledger.available() < price {
        return;
    }
    let tick = observation.tick;
    match step {
        Step::Upgrade(id) => {
            let Some(building) = observation
                .my_buildings
                .iter()
                .find(|building| building.id == id)
            else {
                return;
            };
            ledger.upgrade(id, price);
            ledger.protected = 0;
            persistent.saving.attempted(step, building.anchor, tick);
        }
        Step::Build(kind) => {
            let site = map
                .spots(observation.me)
                .iter()
                .copied()
                .filter(|anchor| !persistent.memory.failed(kind, *anchor, tick))
                .find_map(|anchor| {
                    placement::check(observation, kind, anchor, &ledger.planned)
                        .ok()
                        .map(|allowed| (anchor, allowed))
                });
            let Some((anchor, allowed)) = site else {
                return;
            };
            let centre = footprint_centre(kind, anchor);
            let Some(builder) = workers::builder(observation, frame, centre, ledger) else {
                return;
            };
            ledger.build(builder, kind, anchor, allowed.defer, price);
            ledger.protected = 0;
            persistent.saving.attempted(step, anchor, tick);
        }
    }
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

/// Per mille of income protected for the saving target. Stance and greed set
/// it; visible hostile combat near the seat's buildings lowers it, so the
/// seat spends on defense when it is threatened.
fn share(observation: &ObservationData, profile: &ResolvedProfile) -> u32 {
    let base = match profile.stance {
        BotStance::Turtle => 550,
        BotStance::Balanced => 500,
        BotStance::Aggressive => 400,
    };
    let greed = 3 * (i32::from(profile.traits.greed) - 50);
    let armed = |kind: UnitKind| !kind.stats().weapons.is_empty();
    let threat: u32 = observation
        .enemy_units
        .iter()
        .filter(|unit| armed(unit.kind))
        .filter(|unit| {
            observation
                .my_buildings
                .iter()
                .any(|building| building.anchor.chebyshev(unit.tile) <= 12)
        })
        .map(|unit| unit.kind.stats().cost)
        .sum();
    let army: u32 = observation
        .my_units
        .iter()
        .filter(|unit| armed(unit.kind))
        .map(|unit| unit.kind.stats().cost)
        .sum();
    let cut = if threat == 0 {
        0
    } else if threat >= army {
        300
    } else if 2 * threat >= army {
        150
    } else {
        0
    };
    (base + greed - cut).clamp(200, 800) as u32
}

/// Per mille of the scrap that started around the seat's home already gone.
/// Unexplored nodes count as untouched.
fn depletion(observation: &ObservationData, map: &MapModel) -> u32 {
    let nodes = map.home_nodes(observation.me);
    let initial: u64 = nodes.iter().map(|(_, amount)| u64::from(*amount)).sum();
    if initial == 0 {
        return 0;
    }
    let left: u64 = nodes
        .iter()
        .map(|(node, amount)| {
            if !observation.explored(*node) {
                return u64::from(*amount);
            }
            observation
                .known_scrap
                .binary_search_by_key(&(node.y, node.x), |(tile, _)| (tile.y, tile.x))
                .map_or(0, |index| u64::from(observation.known_scrap[index].1))
        })
        .sum();
    (1_000 - left.min(initial) * 1_000 / initial) as u32
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
