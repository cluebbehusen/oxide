//! One decision: the precedence it follows, its running total and its
//! unit-order allowance.

use crate::composition::{self, Needs};
use crate::defenses;
use crate::frame::{HomeFrame, footprint_centre, gap};
use crate::income::Income;
use crate::investments::{self, Situation, Step};
use crate::map::MapModel;
use crate::memory::Memory;
use crate::missions::{Missions, Scratch};
use crate::placement;
use crate::profile::ResolvedProfile;
use crate::saving::Saving;
use crate::trace::{NextPurchase, Purchase, SavingTarget};
use crate::workers;
use chassis::grid::TilePos;
use oxide_sim::observation::{BuildingObs, ObservationData};
use oxide_sim::scenario::{BotDifficulty, BotStance};
use oxide_sim::stats::{Domain, Role};
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
                | Command::Run { units, .. }
                | Command::Repair { units, .. }
                | Command::RepairUnit { units, .. }
                | Command::Salvage { units, .. } => units.contains(&unit),
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

/// A built own producer, and whether its queue runs out before the next
/// decision, so a unit queued now keeps it working. Every unit trains for
/// longer than a decision interval, so one unit a decision keeps a ready
/// producer busy.
#[derive(Clone, Copy)]
pub(crate) struct Producer<'a> {
    pub(crate) building: &'a BuildingObs,
    pub(crate) ready: bool,
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
    let producers = producers(
        observation,
        frame,
        crate::decision_interval(profile.difficulty),
    );
    let foundries: Vec<Producer<'_>> = producers
        .iter()
        .copied()
        .filter(|producer| producer.building.kind == BuildingKind::Foundry)
        .collect();
    let staffing = workers::staffing(observation, map, frame, &foundries);
    let earned = persistent.income.observe(tick, observation.scrap, rejected);
    persistent.memory.forget(tick);
    persistent.memory.observe(observation);
    let mut scratch = Scratch::new(
        observation,
        map,
        frame,
        &persistent.memory,
        profile.stance,
        &persistent.missions,
    );
    let airworks = observation
        .my_buildings
        .iter()
        .any(|building| building.kind == BuildingKind::Airworks && building.built);
    let income = persistent.income.per_minute();
    // A seat whose ground reaches no enemy delivers ground units only by
    // lift, so until an Airworks stands its army is aircraft, and line units
    // only against invaders already on its ground.
    let outlet = composition::Outlet {
        ground: !scratch.severed || airworks,
        invaders: scratch.invaders,
        air_strikes: airworks || scratch.severed,
        strike: if scratch.severed {
            crate::missions::strike_need(observation, &persistent.memory, profile, &scratch)
        } else {
            0
        },
    };
    let mut needs = composition::needs(
        observation,
        &persistent.memory,
        profile.traits,
        income,
        outlet,
    );
    // A lift carries at least the stance's minimum army, so until the seat has
    // that much a lift could take it neither pulls toward an Airworks nor holds
    // production for carriers: an army and home defense come first.
    let minimum = crate::missions::minimum(profile.stance);
    let exposed = army(observation) < minimum;
    let carryable = scratch.payload >= minimum;
    let lift = carryable && scratch.severed;
    let mut pull = needs.pull(observation);
    if lift {
        pull.push((BuildingKind::Airworks, LIFT_PULL));
    }
    let situation = Situation {
        wanted: needs.wanted(),
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
    scratch.rival = persistent
        .missions
        .rival(&scratch, observation, map, profile.traits);
    workers::recover(observation, &foundries, &mut ledger);
    if short {
        ledger.protected = 0;
        defenses::emergency(observation, map, frame, &persistent.memory, &mut ledger);
    } else {
        buy(observation, map, frame, persistent, &mut ledger);
    }
    // A short defense leaves scrap to the army: only the recovery Harvester
    // above is trained while it lasts.
    if !short {
        workers::train(
            observation,
            &foundries,
            &staffing,
            profile.traits.greed,
            &mut ledger,
        );
    }
    workers::run(
        observation,
        map,
        frame,
        &scratch.ground,
        &staffing,
        &mut ledger,
    );
    for (kind, anchor) in persistent.missions.lift(
        observation,
        map,
        profile,
        &persistent.memory,
        &scratch,
        &mut ledger,
    ) {
        persistent.memory.abandon(kind, anchor, tick);
    }
    for (kind, anchor) in persistent.missions.attack(
        observation,
        map,
        profile,
        &persistent.memory,
        &scratch,
        &mut ledger,
    ) {
        persistent.memory.abandon(kind, anchor, tick);
    }
    for (kind, anchor) in persistent.missions.strike(
        observation,
        map,
        profile,
        &persistent.memory,
        &scratch,
        &mut ledger,
    ) {
        persistent.memory.abandon(kind, anchor, tick);
    }
    for (kind, anchor) in persistent.missions.raid(
        observation,
        map,
        profile,
        &persistent.memory,
        &scratch,
        &mut ledger,
    ) {
        persistent.memory.raid(kind, anchor, tick);
    }
    persistent
        .missions
        .focus(observation, frame, profile.difficulty, &mut ledger);
    persistent
        .missions
        .tend(observation, map, frame, &mut ledger);
    let scout = persistent.missions.scout(
        observation,
        map,
        frame,
        &mut persistent.memory,
        &scratch,
        &mut ledger,
    );
    let carrying = lift
        && !short
        && train_carriers(
            observation,
            map,
            profile,
            persistent,
            &scratch,
            &producers,
            &mut ledger,
        );
    if !carrying {
        if scout {
            train_scout(observation, &producers, &mut ledger);
        }
        if !short {
            train_tenders(observation, profile, &producers, &mut ledger);
            train_raiders(observation, profile, income, &producers, &mut ledger);
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

/// Keeps enough carriers, alive and queued, to lift what the best landing
/// needs with the free units at home, training one at a ready Airworks when
/// short. It is a stock, like the Harvesters: no mission is promised the
/// carriers it buys. Returns whether a ready Airworks waits for the scrap to
/// train one while riders at home already fill every carrier, so that
/// cheaper units do not spend it first.
fn train_carriers(
    observation: &ObservationData,
    map: &MapModel,
    profile: &ResolvedProfile,
    persistent: &Persistent,
    scratch: &Scratch,
    producers: &[Producer<'_>],
    ledger: &mut Ledger,
) -> bool {
    let carriers = (observation
        .my_units
        .iter()
        .map(|unit| unit.kind)
        .chain(observation.my_queues.iter().flatten().copied())
        .filter(|kind| crate::missions::carrier(*kind))
        .count()
        + ledger.queued(UnitKind::Skyhook)) as u64;
    let Some(waiting) = persistent.missions.carriers_short(
        observation,
        map,
        profile,
        &persistent.memory,
        scratch,
        carriers,
    ) else {
        return false;
    };
    producers
        .iter()
        .find(|producer| {
            producer.building.kind == BuildingKind::Airworks
                && producer.ready
                && !ledger.queued_at(producer.building.id)
        })
        .is_some_and(|airworks| !ledger.train(airworks.building.id, UnitKind::Skyhook) && waiting)
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

/// Keeps a Tender, alive or queued, for so much missing health among the
/// seat's armed ground units, more the more it leans on support, up to two,
/// training one at a ready producer that can.
fn train_tenders(
    observation: &ObservationData,
    profile: &ResolvedProfile,
    producers: &[Producer<'_>],
    ledger: &mut Ledger,
) {
    let wounds: u64 = observation
        .my_units
        .iter()
        .filter(|unit| {
            let stats = unit.kind.stats();
            stats.domain == Domain::Ground && !stats.weapons.is_empty()
        })
        .map(|unit| {
            let stats = unit.kind.stats();
            let max = u64::from(stats.max_hp.max(1));
            u64::from(stats.cost) * max.saturating_sub(u64::from(unit.hp)) / max
        })
        .sum();
    let per = WOUNDS_PER_TENDER * 1_000 / composition::weight(profile.traits.support);
    let wanted = (wounds / per).min(TENDERS);
    let have = observation
        .my_units
        .iter()
        .map(|unit| unit.kind)
        .chain(observation.my_queues.iter().flatten().copied())
        .filter(|kind| *kind == UnitKind::Tender)
        .count() as u64
        + ledger.queued(UnitKind::Tender) as u64;
    if have >= wanted {
        return;
    }
    let producer = producers.iter().find(|producer| {
        producer.ready
            && !ledger.queued_at(producer.building.id)
            && producer
                .building
                .kind
                .base_stats()
                .produces
                .contains(&UnitKind::Tender)
    });
    if let Some(producer) = producer {
        ledger.train(producer.building.id, UnitKind::Tender);
    }
}

/// Keeps two Scuttlers, alive or queued, for raiding once income reaches a
/// level that falls with guile, and a Sapper for each known enemy defense
/// that can hit ground, up to an attack's worth, once the seat has scrap to
/// spare, less the more it leans on siege. Each trains at a ready producer
/// that can.
fn train_raiders(
    observation: &ObservationData,
    profile: &ResolvedProfile,
    income: u32,
    producers: &[Producer<'_>],
    ledger: &mut Ledger,
) {
    let scuttlers = if income.saturating_add(4 * u32::from(profile.traits.guile)) >= RAID_INCOME {
        SCUTTLERS
    } else {
        0
    };
    let defenses = observation
        .enemy_buildings
        .iter()
        .filter(|building| {
            building
                .kind
                .base_stats()
                .weapons
                .iter()
                .any(|weapon| weapon.targets.ground)
        })
        .count();
    let sappers = defenses.min(crate::missions::SAPPERS);
    let spare = 2 * (100 - u32::from(profile.traits.siege.min(100)));
    for (kind, wanted, spare) in [
        (UnitKind::Scuttler, scuttlers, 0),
        (UnitKind::Sapper, sappers, spare),
    ] {
        let have = observation
            .my_units
            .iter()
            .map(|unit| unit.kind)
            .chain(observation.my_queues.iter().flatten().copied())
            .filter(|owned| *owned == kind)
            .count()
            + ledger.queued(kind);
        if have >= wanted || ledger.spendable() < kind.stats().cost + spare {
            continue;
        }
        let producer = producers.iter().find(|producer| {
            producer.ready
                && !ledger.queued_at(producer.building.id)
                && producer.building.kind.base_stats().produces.contains(&kind)
        });
        if let Some(producer) = producer {
            ledger.train(producer.building.id, kind);
        }
    }
}

/// Built producers, nearest home first, each ready when the work left in its
/// queue ends within `interval` ticks.
fn producers(observation: &ObservationData, frame: HomeFrame, interval: u64) -> Vec<Producer<'_>> {
    let mut producers: Vec<Producer<'_>> = observation
        .my_buildings
        .iter()
        .zip(&observation.my_queues)
        .enumerate()
        .filter(|(_, (building, _))| {
            building.built && !building.kind.base_stats().produces.is_empty()
        })
        .map(|(index, (building, queue))| {
            let queued: u64 = queue
                .iter()
                .map(|kind| u64::from(kind.stats().train_ticks))
                .sum();
            let done = u64::from(observation.own_queue_progress(index).unwrap_or(0));
            Producer {
                building,
                ready: queued.saturating_sub(done) < interval,
            }
        })
        .collect();
    producers.sort_by_key(|producer| {
        let centre = footprint_centre(producer.building.kind, producer.building.anchor);
        (frame.rank(frame.home, centre), producer.building.id)
    });
    producers
}

/// Gives the most wanted role first claim on ready producers, from
/// unprotected scrap: in turn, the nearest ready producer that can afford a
/// unit for it queues the best one, and a role no ready producer can afford
/// gives way to the next. Ready producers left with no wanted role train line
/// units while ground units can reach an enemy.
fn produce(
    observation: &ObservationData,
    producers: &[Producer<'_>],
    needs: &mut Needs,
    ledger: &mut Ledger,
) {
    let mut idle: Vec<&Producer<'_>> = producers
        .iter()
        .filter(|producer| producer.ready && !ledger.queued_at(producer.building.id))
        .collect();
    while let Some((index, kind)) = needs.wanted().into_iter().find_map(|role| {
        idle.iter().enumerate().find_map(|(index, producer)| {
            needs
                .unit(
                    observation,
                    producer.building.kind,
                    role,
                    ledger.spendable(),
                )
                .map(|kind| (index, kind))
        })
    }) {
        let producer = idle.remove(index);
        if ledger.train(producer.building.id, kind) {
            needs.queued(kind);
        }
    }
    if !needs.fallback() {
        return;
    }
    for producer in idle {
        let kind = producer.building.kind;
        let wanted = needs.wanted();
        if wanted
            .iter()
            .any(|role| composition::serves(observation, kind, *role))
        {
            continue;
        }
        if let Some(unit) = needs.unit(
            observation,
            kind,
            composition::Role::Line,
            ledger.spendable(),
        ) && ledger.train(producer.building.id, unit)
        {
            needs.queued(unit);
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

/// Scrap of missing health among the seat's armed ground units per Tender
/// a seat of middling support keeps.
const WOUNDS_PER_TENDER: u64 = 500;

/// Tenders a seat keeps, at most.
const TENDERS: u64 = 2;

/// Scuttlers a seat keeps for raiding.
const SCUTTLERS: usize = 2;

/// Income per minute, less four for each point of guile, at which the seat
/// starts keeping Scuttlers for raiding.
const RAID_INCOME: u32 = 900;

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
