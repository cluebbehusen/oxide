//! One decision: the precedence it follows, its running total and its
//! unit-order allowance.

use crate::composition::{self, Needs};
use crate::defenses;
use crate::frame::{HomeFrame, footprint_centre, gap};
use crate::income::Income;
use crate::investments::{self, Situation, Step};
use crate::map::MapModel;
use crate::memory::Memory;
use crate::missions::Missions;
use crate::placement;
use crate::profile::ResolvedProfile;
use crate::saving::Saving;
use crate::trace::{NextPurchase, Purchase, SavingTarget};
use crate::workers;
use chassis::grid::TilePos;
use oxide_sim::observation::{BuildingObs, ObservationData};
use oxide_sim::scenario::{BotDifficulty, BotStance};
use oxide_sim::stats::Role;
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
    pub(crate) missions: Missions,
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

    pub(crate) fn build(
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

    /// Unit orders this decision may still issue.
    pub(crate) fn room(&self) -> u32 {
        self.decision.allowance - self.decision.unit_orders
    }

    /// Unit orders the whole decision may issue.
    pub(crate) fn allowance(&self) -> u32 {
        self.decision.allowance
    }

    /// Footprints this decision already committed to.
    pub(crate) fn planned(&self) -> &[(BuildingKind, TilePos)] {
        &self.planned
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
                Command::Harvest { units, .. }
                | Command::Build { units, .. }
                | Command::Run { units, .. } => units.contains(&unit),
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

/// A built own producer, and whether its queue was empty when the decision
/// began.
#[derive(Clone, Copy)]
pub(crate) struct Producer<'a> {
    pub(crate) building: &'a BuildingObs,
    pub(crate) idle: bool,
}

/// Defense first, then worker recovery, then an affordable saving target
/// unless a defense is short, then workers, then lifts, attacks, strikes,
/// focus fire and scouting, then production. A short defense instead buys an
/// emergency static defense, trains no more Harvesters, and frees protected
/// scrap for this decision's production.
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
    let producers = producers(observation, frame);
    let foundries: Vec<Producer<'_>> = producers
        .iter()
        .copied()
        .filter(|producer| producer.building.kind == BuildingKind::Foundry)
        .collect();
    let staffing = workers::staffing(observation, map, frame, &foundries);
    let earned = persistent.income.observe(tick, observation.scrap, rejected);
    persistent.memory.forget(tick);
    persistent.memory.observe(observation);
    let air_strikes = observation
        .my_buildings
        .iter()
        .any(|building| building.kind == BuildingKind::Airworks && building.built);
    let income = persistent.income.per_minute();
    let mut needs = composition::needs(
        observation,
        &persistent.memory,
        profile.traits,
        income,
        air_strikes,
    );
    // A lift carries at least the stance's minimum army, so until the seat has
    // that much a lift could take it neither pulls toward an Airworks nor holds
    // production for carriers: an army and home defense come first.
    let minimum = crate::missions::minimum(profile.stance);
    let exposed = army(observation) < minimum;
    let carryable = crate::missions::payload(observation, map).0 >= minimum;
    let lift = carryable && crate::missions::lift_needed(observation, map, frame);
    let mut pull = needs.pull(observation);
    if lift {
        pull.push((BuildingKind::Airworks, LIFT_PULL));
    }
    let situation = Situation {
        observation,
        map,
        memory: &persistent.memory,
        traits: profile.traits,
        saturation: staffing.saturation(),
        income,
        depletion: depletion(observation, map),
        pull,
        exposed,
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

    persistent
        .missions
        .prune(observation, &mut persistent.memory);
    let short = persistent
        .missions
        .defend(observation, map, frame, &mut ledger);
    workers::recover(observation, &foundries, &mut ledger);
    if short {
        ledger.protected = 0;
        defenses::emergency(observation, map, frame, &persistent.memory, &mut ledger);
    } else {
        buy(observation, map, frame, persistent, &mut ledger);
    }
    // A short defense leaves scrap to the army: only the recovery Harvester
    // above is trained while it lasts.
    workers::run(
        observation,
        map,
        frame,
        &foundries,
        &staffing,
        &mut ledger,
        !short,
    );
    if let Some((kind, anchor)) = persistent.missions.lift(
        observation,
        map,
        frame,
        profile,
        &persistent.memory,
        &mut ledger,
    ) {
        persistent.memory.abandon(kind, anchor, tick);
    }
    persistent.missions.attack(
        observation,
        map,
        frame,
        profile,
        &mut persistent.memory,
        &mut ledger,
    );
    if let Some((kind, anchor)) = persistent.missions.strike(
        observation,
        map,
        frame,
        profile,
        &persistent.memory,
        &mut ledger,
    ) {
        persistent.memory.abandon(kind, anchor, tick);
    }
    persistent
        .missions
        .focus(observation, frame, profile.difficulty, &mut ledger);
    let scout =
        persistent
            .missions
            .scout(observation, map, frame, &mut persistent.memory, &mut ledger);
    let carrying =
        lift && !short && train_carriers(observation, map, profile, &producers, &mut ledger);
    if !carrying {
        if scout {
            train_scout(observation, &producers, &mut ledger);
        }
        produce(observation, &producers, &mut needs, &mut ledger);
    }

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
    if persistent.saving.pending() {
        return;
    }
    let Some((step, price)) = investments::step(observation, investment) else {
        return;
    };
    let affordable = ledger.available() >= price;
    let tick = observation.tick;
    match step {
        Step::Upgrade(id) => {
            if !affordable {
                return;
            }
            let Some(building) = observation
                .my_buildings
                .iter()
                .find(|building| building.id == id)
            else {
                return;
            };
            ledger.upgrade(id, price);
            ledger.protected = 0;
            persistent.saving.attempted(step, building.anchor);
        }
        Step::Build(kind) => {
            let anchors: Vec<TilePos> = investments::anchors(map, observation, investment, kind)
                .into_iter()
                .filter(|anchor| !persistent.memory.failed(kind, *anchor, tick))
                .collect();
            let site = anchors.iter().copied().find_map(|anchor| {
                placement::check(observation, kind, anchor, &ledger.planned)
                    .ok()
                    .map(|allowed| (anchor, allowed))
            });
            let Some((anchor, allowed)) = site else {
                // With nowhere left to look, protecting scrap for a building
                // that cannot be placed would starve production. This is
                // checked before the bank covers the price, or the protection
                // would keep building up toward it.
                match unexplored(observation, &anchors, kind) {
                    None => {
                        ledger.protected = 0;
                        persistent.saving.keep_at_most(0);
                    }
                    Some(anchor) if affordable => {
                        explore(observation, map, frame, anchor, kind, ledger);
                    }
                    Some(_) => {}
                }
                return;
            };
            if !affordable {
                return;
            }
            if kind == BuildingKind::Barricade && !defenses::keeps_paths(observation, map, anchor) {
                persistent.memory.fail(kind, anchor, tick);
                ledger.protected = 0;
                persistent.saving.keep_at_most(0);
                return;
            }
            let centre = footprint_centre(kind, anchor);
            let Some(builder) = workers::builder(observation, map, frame, anchor, centre, ledger)
            else {
                return;
            };
            ledger.build(builder, kind, anchor, allowed.defer, price);
            ledger.protected = 0;
            persistent.saving.attempted(step, anchor);
        }
    }
}

/// The first of `anchors` whose footprint the seat has not fully seen, which
/// may turn out placeable once explored.
fn unexplored(
    observation: &ObservationData,
    anchors: &[TilePos],
    kind: BuildingKind,
) -> Option<TilePos> {
    let (width, height) = kind.base_stats().size;
    anchors.iter().copied().find(|anchor| {
        (0..height).any(|dy| (0..width).any(|dx| !observation.explored(anchor.offset(dx, dy))))
    })
}

/// Sends the nearest free Harvester to look at `anchor`.
fn explore(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    anchor: TilePos,
    kind: BuildingKind,
    ledger: &mut Ledger,
) {
    let centre = footprint_centre(kind, anchor);
    if let Some(builder) = workers::builder(observation, map, frame, anchor, centre, ledger) {
        ledger.order(Command::Run {
            units: vec![builder],
            goal: anchor,
            queue: false,
        });
    }
}

/// Keeps enough carriers, alive and queued, to lift the stance's minimum army
/// at the value per transport slot of the units at home a lift could take,
/// training one at an idle Airworks when short. It is a stock, like the
/// Harvesters: no mission is promised the carriers it buys. Returns whether
/// an idle Airworks waits for the scrap to train one, so that cheaper units
/// do not spend it first.
fn train_carriers(
    observation: &ObservationData,
    map: &MapModel,
    profile: &ResolvedProfile,
    producers: &[Producer<'_>],
    ledger: &mut Ledger,
) -> bool {
    let (value, slots) = crate::missions::payload(observation, map);
    let per_slot = value
        .checked_div(slots)
        .map_or(EMPTY_SLOT_VALUE, |value| value.max(1));
    let capacity = u64::from(UnitKind::Skyhook.stats().transport_capacity).max(1);
    let minimum = crate::missions::minimum(profile.stance);
    let wanted = minimum.div_ceil(capacity * per_slot).clamp(1, 4) as usize;
    let carriers = observation
        .my_units
        .iter()
        .map(|unit| unit.kind)
        .chain(observation.my_queues.iter().flatten().copied())
        .filter(|kind| crate::missions::carrier(*kind))
        .count()
        + ledger.queued(UnitKind::Skyhook);
    if carriers >= wanted {
        return false;
    }
    producers
        .iter()
        .find(|producer| {
            producer.building.kind == BuildingKind::Airworks
                && producer.idle
                && !ledger.queued_at(producer.building.id)
        })
        .is_some_and(|airworks| !ledger.train(airworks.building.id, UnitKind::Skyhook))
}

/// Trains the scout scouting wants unless the seat already has or is making
/// one: the faction's air scout at a built Airworks, else a Scuttler at a
/// Foundry. Once an air scout can be trained a Scuttler no longer counts,
/// since one that could reach a stale point would already be scouting.
fn train_scout(observation: &ObservationData, producers: &[Producer<'_>], ledger: &mut Ledger) {
    let air = Role::Scout.unit_for(observation.faction);
    let airworks = producers
        .iter()
        .find(|producer| producer.building.kind == BuildingKind::Airworks);
    let kinds: &[UnitKind] = if airworks.is_some() {
        &[air]
    } else {
        &[air, UnitKind::Scuttler]
    };
    let have = observation
        .my_units
        .iter()
        .map(|unit| unit.kind)
        .chain(observation.my_queues.iter().flatten().copied())
        .any(|kind| kinds.contains(&kind))
        || kinds.iter().any(|kind| ledger.queued(*kind) > 0);
    if have {
        return;
    }
    let (producer, kind) = match airworks {
        Some(producer) => (producer, air),
        None => {
            let Some(foundry) = producers
                .iter()
                .find(|producer| producer.building.kind == BuildingKind::Foundry)
            else {
                return;
            };
            (foundry, UnitKind::Scuttler)
        }
    };
    ledger.train(producer.building.id, kind);
}

/// Built producers, nearest home first.
fn producers(observation: &ObservationData, frame: HomeFrame) -> Vec<Producer<'_>> {
    let mut producers: Vec<Producer<'_>> = observation
        .my_buildings
        .iter()
        .zip(&observation.my_queues)
        .filter(|(building, _)| building.built && !building.kind.base_stats().produces.is_empty())
        .map(|(building, queue)| Producer {
            building,
            idle: queue.is_empty(),
        })
        .collect();
    producers.sort_by_key(|producer| {
        let centre = footprint_centre(producer.building.kind, producer.building.anchor);
        (frame.rank(frame.home, centre), producer.building.id)
    });
    producers
}

/// Has every idle producer queue the unit it can best train for the most
/// wanted role, from unprotected scrap.
fn produce(
    observation: &ObservationData,
    producers: &[Producer<'_>],
    needs: &mut Needs,
    ledger: &mut Ledger,
) {
    for producer in producers {
        if !producer.idle || ledger.queued_at(producer.building.id) {
            continue;
        }
        let Some(kind) = needs.choose(observation, producer.building.kind, ledger.spendable())
        else {
            continue;
        };
        if ledger.train(producer.building.id, kind) {
            needs.queued(kind);
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
            observation.my_buildings.iter().any(|building| {
                let size = building.kind.base_stats().size;
                gap(building.anchor, size, unit.tile, (1, 1)) < 12
            })
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

/// What the seat's armed units cost.
fn army(observation: &ObservationData) -> u64 {
    observation
        .my_units
        .iter()
        .filter(|unit| !unit.kind.stats().weapons.is_empty())
        .map(|unit| u64::from(unit.kind.stats().cost))
        .sum()
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

/// The value per transport slot assumed with no line or siege unit at home.
const EMPTY_SLOT_VALUE: u64 = 90;

/// What a needed lift adds to the Airworks' investment score.
const LIFT_PULL: u32 = 600;

/// Unit orders one decision may issue.
fn allowance(difficulty: BotDifficulty) -> u32 {
    match difficulty {
        BotDifficulty::Scrapheap => 3,
        BotDifficulty::Standard => 6,
        BotDifficulty::Veteran => 8,
        BotDifficulty::Prime => 10,
    }
}
