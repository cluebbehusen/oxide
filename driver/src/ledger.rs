//! The impact ledger: what every unit and building did over a match, valued
//! in scrap. It observes each tick's state and events, so it serves live
//! evaluation and replays alike, and it is omniscient QA evidence that never
//! reaches a controller.
//!
//! The simulation names a shooter on each hit but reports neither damage
//! amounts nor killers, so the ledger diffs every unit's and building's
//! health tick to tick, net of reported repairs, and splits each loss among
//! the sources that hit that body that tick: targeted and blind hits, turret
//! fire, shells matched from launch to landing across their splash, charge
//! detonations and Sapper blasts. A loss is valued at the victim's worth
//! times the fraction of its health lost. Two credits fall outside damage
//! dealt: repair, to the welder or Repair Bay that supplied it, and
//! spotting, to the nearest friendly body whose sight covered a target its
//! shooter could not see, approximated by vision radius. Crew repair of
//! buildings, radar warning and the harvester recovery trickle report
//! nothing the ledger can credit.

use chassis::grid::TilePos;
use oxide_sim::stats::{
    BuildingKind, CHARGE_ARRAY_DETECT_RADIUS, CHARGE_BASE_ARRAY_DETECT_RADIUS, CHARGE_BLAST_RADIUS,
    CRUCIBLE_SMELT_PERIOD, CRUCIBLE_SMELT_RADIUS, EXTRACTOR_REMOTE_YIELD,
    EXTRACTOR_SUPPORTED_YIELD, FOUNDRY_DRIP_PERIOD, FOUNDRY_DRIP_START_TICK, RECLAIMER_PERIOD,
    REFINERY_PERIOD, SAPPER_BLAST_RADIUS, UnitKind,
};
use oxide_sim::{
    BuildingId, Event, ExtractorIncome, State, Target, TickReport, UnitId, UnitRepairSource,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Ticks between net-worth samples.
pub const WORTH_PERIOD: u64 = 1_000;

/// Tiles from a seat's buildings, other than mines and Barricades scattered
/// across the map, inside which a fight counts as at its home.
const PLACE_TILES: i32 = 12;

/// Ticks per minute.
const MINUTE: u64 = oxide_sim::TICKS_PER_SECOND as u64 * 60;

/// Minutes at which the spending phases begin, after the first.
const PHASES: [u64; 3] = [5, 10, 20];

/// Damage, spotting and repair accumulate in thousandths of a scrap, so a
/// loss split over many small hits keeps its value, and are rounded to scrap
/// once, in [`ImpactLedger::finish`].
const MILLI: u64 = 1_000;

/// Ticks a launched shell may land late and still match its launch.
const SHELL_SLACK: u64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Unit(UnitId),
    Building(BuildingId),
}

/// What a body is, for valuing the damage it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Army,
    Worker,
    Other,
    Building,
}

/// A unit or building as the ledger last saw it.
#[derive(Debug, Clone, Copy)]
struct Body {
    key: Key,
    owner: u8,
    team: u8,
    hp: u32,
    max_hp: u32,
    /// Scrap the body is worth whole: a unit's cost, or a building's cost
    /// summed over the tiers it has reached.
    value: u64,
    tile: TilePos,
    vision: i32,
    class: Class,
    unit: Option<UnitKind>,
    building: Option<(BuildingKind, u8, bool)>,
    /// The transport a rider travels in; riders count toward worth and die
    /// with their carrier.
    carrier: Option<Key>,
    /// A building's construction or upgrade progress, the build ticks its
    /// tier takes, and the health salvage has drained from it.
    progress: u32,
    build_ticks: u32,
    salvage: u32,
}

impl Body {
    fn tier(&self) -> Option<u8> {
        self.building.map(|(_, tier, _)| tier)
    }

    /// Health construction or an upgrade added by `post`, which emits no
    /// event: the simulation's ramp from a fifth of full health by build
    /// progress. `None` on the tick work starts, which sets the site up.
    fn built_gain(&self, post: &Body) -> Option<u32> {
        if !matches!(self.building, Some((_, _, false))) || self.build_ticks == 0 {
            return Some(0);
        }
        if self.progress == 0 && post.progress > 0 {
            return None;
        }
        let ramp = self.max_hp - self.max_hp / 5;
        let at = |progress: u32| ramp * progress.min(self.build_ticks) / self.build_ticks;
        Some(at(post.progress).saturating_sub(at(self.progress)))
    }
}

/// Who dealt a loss, as they were when they fired: a shell can land after
/// its gun is gone.
#[derive(Debug, Clone, Copy)]
struct Source {
    key: Key,
    owner: u8,
    team: u8,
    vision: i32,
    /// Where it fired from, for spotting credit.
    from: Option<TilePos>,
}

/// One body's tally over its life.
#[derive(Debug, Clone, Default)]
struct Record {
    owner: u8,
    unit: Option<UnitKind>,
    building: Option<BuildingKind>,
    top_tier: u8,
    starting: bool,
    born: u64,
    gone: Option<u64>,
    paid: u64,
    dealt: Dealt,
    taken: u64,
    enabled: u64,
    repaired: u64,
    harvested: u64,
    received: u64,
    passive: u64,
    produced: u64,
}

/// Scrap value of damage dealt, by what was hit and where.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dealt {
    /// To armed units.
    pub army: u64,
    /// To workers.
    pub workers: u64,
    /// To other units: transports, Tenders, scouts.
    pub other: u64,
    /// To buildings.
    pub buildings: u64,
    /// Near the dealer's own buildings.
    pub home: u64,
    /// Near the victim's buildings.
    pub away: u64,
    /// Near neither.
    pub field: u64,
}

impl Dealt {
    /// Everything dealt.
    pub fn total(&self) -> u64 {
        self.army + self.workers + self.other + self.buildings
    }

    fn add(&mut self, other: &Dealt) {
        self.army += other.army;
        self.workers += other.workers;
        self.other += other.other;
        self.buildings += other.buildings;
        self.home += other.home;
        self.away += other.away;
        self.field += other.field;
    }
}

/// One unit kind's, or one building kind's at its highest tier, totals for
/// a seat.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KindLedger {
    /// Bodies the seat built or trained during the match.
    pub built: u64,
    /// Bodies the seat started with.
    pub starting: u64,
    /// Scrap paid, counting starting bodies at their worth.
    pub paid: u64,
    /// Damage dealt, in scrap.
    pub dealt: Dealt,
    /// Damage taken, in scrap.
    pub taken: u64,
    /// Damage friendly shooters dealt to targets only this kind could see,
    /// and hidden charges destroyed inside an Array's detection.
    pub enabled: u64,
    /// Repair supplied to others, in scrap.
    pub repaired: u64,
    /// Scrap delivered by workers.
    pub harvested: u64,
    /// Scrap delivered to this kind of building.
    pub received: u64,
    /// Scrap credited without deliveries: Reclaimers, Extractors and the
    /// Foundry drip.
    pub passive: u64,
    /// Scrap of units this kind of building trained.
    pub produced: u64,
    /// Bodies destroyed.
    pub deaths: u64,
    /// Ticks the destroyed ones lived, summed.
    pub lifetime: u64,
}

impl KindLedger {
    /// Adds `other` to these totals.
    pub fn add(&mut self, other: &KindLedger) {
        self.built += other.built;
        self.starting += other.starting;
        self.paid += other.paid;
        self.dealt.add(&other.dealt);
        self.taken += other.taken;
        self.enabled += other.enabled;
        self.repaired += other.repaired;
        self.harvested += other.harvested;
        self.received += other.received;
        self.passive += other.passive;
        self.produced += other.produced;
        self.deaths += other.deaths;
        self.lifetime += other.lifetime;
    }
}

/// What a seat's units and buildings did over a match.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatLedger {
    /// Totals by unit kind.
    pub units: BTreeMap<String, KindLedger>,
    /// Totals by building kind at the highest tier each building reached.
    pub buildings: BTreeMap<String, KindLedger>,
    /// Damage this seat took that no hit accounts for, in scrap.
    pub unattributed: u64,
    /// Scrap spent by `<category>:<kind>`, in the phases starting at minutes
    /// 0, 5, 10 and 20. Categories: `army` and `worker` units, `economy`,
    /// `defense` and `tech` buildings, and `upgrade` to the named tier. Units
    /// count when trained, not when queued and paid, so one still queued
    /// when the match ends is not counted.
    pub spend: BTreeMap<String, [u64; 4]>,
    /// Scrap refunded by cancelled and salvaged buildings.
    pub refunds: u64,
    /// Net worth every [`WORTH_PERIOD`] ticks after `start`: units, riders
    /// included, and buildings at their worth times their health fraction,
    /// plus the bank.
    pub worth: Vec<u64>,
    /// Tick the ledger began: zero for a match, later for a recording that
    /// starts from a saved world.
    #[serde(default)]
    pub start: u64,
    /// Tick the ledger ended; zero in rows recorded before it was kept.
    #[serde(default)]
    pub end: u64,
    /// Net worth at `end`.
    #[serde(default)]
    pub final_worth: u64,
}

/// A shell in flight, waiting to land.
#[derive(Debug, Clone, Copy)]
struct Shell {
    lands: u64,
    owner: u8,
    to: TilePos,
    source: Source,
}

/// Damage sources reported in one tick.
#[derive(Default)]
struct Sources {
    aimed: BTreeMap<Key, Vec<Source>>,
    blind: Vec<(TilePos, Source)>,
    splash: Vec<(TilePos, i32, Source)>,
    repaired: BTreeMap<Key, u32>,
    /// Charges that fired this tick, spending themselves.
    detonated: Vec<Key>,
    /// Units trained this tick, with their kind and owner.
    born: Vec<(Key, UnitKind, u8)>,
    /// Where each unit that died this tick died.
    died: BTreeMap<Key, TilePos>,
}

/// The ledger.
pub struct ImpactLedger {
    start: u64,
    teams: Vec<u8>,
    bodies: Vec<Body>,
    records: BTreeMap<Key, Record>,
    unattributed: Vec<u64>,
    spend: Vec<BTreeMap<String, [u64; 4]>>,
    refunds: Vec<u64>,
    worth: Vec<Vec<u64>>,
    shells: Vec<Shell>,
    /// Crucibles that have a wreck in reach as a smelting tick begins.
    smelting: Vec<Key>,
}

fn building_value(kind: BuildingKind, tier: u8) -> u64 {
    kind.tiers()
        .iter()
        .take(usize::from(tier) + 1)
        .filter_map(|stats| stats.construction.as_ref())
        .map(|construction| u64::from(construction.cost))
        .sum()
}

fn snapshot(state: &State) -> Vec<Body> {
    let team = |player: oxide_sim::PlayerId| state.players()[usize::from(player.0)].team;
    let mut bodies: Vec<Body> = state
        .units()
        .iter()
        .map(|unit| {
            let stats = unit.kind.stats();
            Body {
                key: Key::Unit(unit.id),
                owner: unit.player.0,
                team: team(unit.player),
                hp: unit.hp,
                max_hp: stats.max_hp,
                value: u64::from(stats.cost),
                tile: unit.tile(),
                vision: stats.vision,
                class: class(unit.kind),
                unit: Some(unit.kind),
                building: None,
                carrier: None,
                progress: 0,
                build_ticks: 0,
                salvage: 0,
            }
        })
        .collect();
    let riders: Vec<Body> = state
        .units()
        .iter()
        .flat_map(|carrier| {
            carrier.cargo.iter().map(|rider| {
                let stats = rider.kind.stats();
                Body {
                    key: Key::Unit(rider.id),
                    owner: rider.player.0,
                    team: team(rider.player),
                    hp: rider.hp,
                    max_hp: stats.max_hp,
                    value: u64::from(stats.cost),
                    tile: carrier.tile(),
                    vision: 0,
                    class: class(rider.kind),
                    unit: Some(rider.kind),
                    building: None,
                    carrier: Some(Key::Unit(carrier.id)),
                    progress: 0,
                    build_ticks: 0,
                    salvage: 0,
                }
            })
        })
        .collect();
    if !riders.is_empty() {
        bodies.extend(riders);
        bodies.sort_unstable_by_key(|body| body.key);
    }
    bodies.extend(state.buildings().iter().map(|building| {
        let stats = building.kind.tier_stats(building.tier);
        Body {
            key: Key::Building(building.id),
            owner: building.player.0,
            team: team(building.player),
            hp: building.hp,
            max_hp: stats.max_hp,
            value: building_value(building.kind, building.tier),
            tile: TilePos::containing(building.center()),
            vision: stats.vision,
            class: Class::Building,
            unit: None,
            building: Some((building.kind, building.tier, building.built)),
            carrier: None,
            progress: building.progress,
            build_ticks: stats
                .construction
                .as_ref()
                .map_or(0, |construction| construction.build_ticks),
            salvage: building.salvage_drained,
        }
    }));
    bodies
}

fn class(kind: UnitKind) -> Class {
    let stats = kind.stats();
    if stats.harvest.is_some() {
        Class::Worker
    } else if stats.can_fight() {
        Class::Army
    } else {
        Class::Other
    }
}

/// A seat's net worth: its share of `bodies` plus its bank in `state`.
fn worth(state: &State, bodies: &[Body], seat: usize) -> u64 {
    let owned: u64 = bodies
        .iter()
        .filter(|body| usize::from(body.owner) == seat)
        .map(|body| body.value * u64::from(body.hp) / u64::from(body.max_hp.max(1)))
        .sum();
    owned + u64::from(state.players()[seat].scrap)
}

/// Crucibles in `state` that will smelt a wreck, by the simulation's reach
/// rule and allocation: in id order, each takes one unit of salvage from the
/// nearest wreck, the richer on a tie, so two sharing a last unit do not
/// both smelt it.
fn smelters(state: &State) -> Vec<Key> {
    let reach = CRUCIBLE_SMELT_RADIUS.to_num::<i32>() + 1;
    let radius = CRUCIBLE_SMELT_RADIUS * CRUCIBLE_SMELT_RADIUS;
    let mut taken: BTreeMap<(i32, i32), u32> = BTreeMap::new();
    let mut smelting = Vec::new();
    for building in state.buildings() {
        if !building.built || building.hp == 0 || building.kind != BuildingKind::Crucible {
            continue;
        }
        let (w, h) = building.kind.base_stats().size;
        let anchor = building.anchor;
        let mut fuel = None;
        for y in (anchor.y - reach)..(anchor.y + h + reach) {
            for x in (anchor.x - reach)..(anchor.x + w + reach) {
                let tile = TilePos::new(x, y);
                let left = state
                    .map()
                    .wreck_at(tile)
                    .saturating_sub(taken.get(&(x, y)).copied().unwrap_or(0));
                let center = tile.center();
                let distance = building.closest_point_to(center).dist_sq(center);
                if left == 0 || distance > radius {
                    continue;
                }
                let key = (distance, std::cmp::Reverse(left), (y, x));
                if fuel.is_none_or(|best| key < best) {
                    fuel = Some(key);
                }
            }
        }
        if let Some((_, _, (y, x))) = fuel {
            *taken.entry((x, y)).or_default() += 1;
            smelting.push(Key::Building(building.id));
        }
    }
    smelting
}

fn within(a: TilePos, b: TilePos, radius: i32) -> bool {
    let (dx, dy) = (a.x - b.x, a.y - b.y);
    dx * dx + dy * dy <= radius * radius
}

fn tiles(radius: chassis::fx::Fx) -> i32 {
    radius.to_num::<i32>() + 1
}

fn phase(tick: u64) -> usize {
    let minute = tick / MINUTE;
    PHASES.iter().filter(|start| minute >= **start).count()
}

impl ImpactLedger {
    /// A ledger starting from `state`, whose bodies count as the seats'
    /// starting stock.
    pub fn new(state: &State) -> Self {
        let start = state.current_tick();
        let bodies = snapshot(state);
        let mut records = BTreeMap::new();
        for body in &bodies {
            let mut record = record(body, true);
            record.born = start;
            records.insert(body.key, record);
        }
        let seats = state.players().len();
        Self {
            start,
            teams: state.players().iter().map(|player| player.team).collect(),
            bodies,
            records,
            unattributed: vec![0; seats],
            spend: vec![BTreeMap::new(); seats],
            refunds: vec![0; seats],
            worth: vec![Vec::new(); seats],
            shells: Vec::new(),
            smelting: if start.is_multiple_of(CRUCIBLE_SMELT_PERIOD) {
                smelters(state)
            } else {
                Vec::new()
            },
        }
    }

    /// Value of the damage unit `id` has dealt and taken so far, in
    /// thousandths of a scrap.
    pub fn totals(&self, id: UnitId) -> (u64, u64) {
        self.records
            .get(&Key::Unit(id))
            .map_or((0, 0), |record| (record.dealt.total(), record.taken))
    }

    /// Accounts for the tick `report` describes, which left `state`.
    pub fn observe(&mut self, state: &State, report: &TickReport) {
        let now = report.tick;
        let after = snapshot(state);
        let sources = self.events(report);
        self.spending(&after, now);
        self.damage(&after, &sources, now);
        self.passive(state, now);
        if (now + 1 - self.start).is_multiple_of(WORTH_PERIOD) {
            for (seat, samples) in self.worth.iter_mut().enumerate() {
                samples.push(worth(state, &after, seat));
            }
        }
        self.smelting = if (now + 1).is_multiple_of(CRUCIBLE_SMELT_PERIOD) {
            smelters(state)
        } else {
            Vec::new()
        };
        self.bodies = after;
    }

    /// `key` as a damage source, as it is now; a unit trained this tick,
    /// absent from the snapshot, as its record has it.
    fn source(&self, key: Key, from: Option<TilePos>) -> Option<Source> {
        if let Some(body) = self.body(key) {
            return Some(Source {
                key,
                owner: body.owner,
                team: body.team,
                vision: body.vision,
                from,
            });
        }
        let record = self.records.get(&key)?;
        Some(Source {
            key,
            owner: record.owner,
            team: self.teams[usize::from(record.owner)],
            vision: record.unit?.stats().vision,
            from,
        })
    }

    fn body(&self, key: Key) -> Option<&Body> {
        self.bodies
            .binary_search_by_key(&key, |body| body.key)
            .ok()
            .map(|index| &self.bodies[index])
    }

    /// Records births, deaths, deliveries and repairs, and gathers this
    /// tick's damage sources.
    fn events(&mut self, report: &TickReport) -> Sources {
        let now = report.tick;
        let mut sources = Sources::default();
        for event in &report.events {
            match event {
                Event::UnitTrained {
                    building,
                    unit,
                    kind,
                    player,
                } => {
                    let cost = u64::from(kind.stats().cost);
                    sources.born.push((Key::Unit(*unit), *kind, player.0));
                    self.records.insert(
                        Key::Unit(*unit),
                        Record {
                            owner: player.0,
                            unit: Some(*kind),
                            born: now,
                            paid: cost,
                            ..Record::default()
                        },
                    );
                    if let Some(producer) = self.records.get_mut(&Key::Building(*building)) {
                        producer.produced += cost;
                    }
                    let category = if kind.stats().harvest.is_some() {
                        "worker"
                    } else {
                        "army"
                    };
                    self.spent(player.0, format!("{category}:{}", kind.name()), now, cost);
                }
                Event::UnitDied { unit, pos, .. } => {
                    sources
                        .died
                        .insert(Key::Unit(*unit), TilePos::containing(*pos));
                    self.gone(Key::Unit(*unit), now);
                }
                Event::BuildingDestroyed { building, .. } => {
                    self.gone(Key::Building(*building), now);
                }
                Event::BuildCancelled { player, refund, .. }
                | Event::BuildingSalvaged { player, refund, .. } => {
                    self.refunds[usize::from(player.0)] += u64::from(*refund);
                }
                Event::ScrapDeposited {
                    unit,
                    foundry,
                    amount,
                    ..
                } => {
                    if let Some(record) = self.records.get_mut(&Key::Unit(*unit)) {
                        record.harvested += u64::from(*amount);
                    }
                    if let Some(record) = self.records.get_mut(&Key::Building(*foundry)) {
                        record.received += u64::from(*amount);
                    }
                }
                Event::UnitRepaired {
                    unit,
                    source,
                    amount,
                    ..
                } => {
                    let welder = match source {
                        UnitRepairSource::FieldWelder { unit } => Key::Unit(*unit),
                        UnitRepairSource::RepairBay { building } => Key::Building(*building),
                    };
                    self.repair(Key::Unit(*unit), welder, *amount, &mut sources);
                }
                Event::BuildingRepaired {
                    building,
                    repair_bay,
                    amount,
                    ..
                } => self.repair(
                    Key::Building(*building),
                    Key::Building(*repair_bay),
                    *amount,
                    &mut sources,
                ),
                Event::AttackHit {
                    attacker,
                    attacker_kind,
                    target,
                    attacker_pos,
                    target_pos,
                    ..
                } => {
                    let Some(source) = self.source(
                        Key::Unit(*attacker),
                        Some(TilePos::containing(*attacker_pos)),
                    ) else {
                        continue;
                    };
                    let at = TilePos::containing(*target_pos);
                    if *attacker_kind == UnitKind::Sapper {
                        sources
                            .splash
                            .push((at, tiles(SAPPER_BLAST_RADIUS), source));
                    }
                    aim(&mut sources, *target, at, source);
                }
                Event::TurretFired {
                    turret,
                    target,
                    turret_pos,
                    target_pos,
                    ..
                } => {
                    if let Some(source) = self.source(
                        Key::Building(*turret),
                        Some(TilePos::containing(*turret_pos)),
                    ) {
                        aim(
                            &mut sources,
                            *target,
                            TilePos::containing(*target_pos),
                            source,
                        );
                    }
                }
                Event::ShellLaunched {
                    shooter,
                    player,
                    from,
                    to,
                    flight,
                    ..
                } => {
                    let key = match shooter {
                        Target::Unit(id) => Key::Unit(*id),
                        Target::Building(id) => Key::Building(*id),
                    };
                    if let Some(source) = self.source(key, Some(TilePos::containing(*from))) {
                        self.shells.push(Shell {
                            lands: now + flight,
                            owner: player.0,
                            to: TilePos::containing(*to),
                            source,
                        });
                    }
                }
                Event::ShellLanded {
                    player, at, splash, ..
                } => {
                    let at = TilePos::containing(*at);
                    let landed = |slack: u64| {
                        self.shells.iter().position(|shell| {
                            shell.owner == player.0
                                && shell.to == at
                                && shell.lands.abs_diff(now) <= slack
                        })
                    };
                    if let Some(index) = landed(0).or_else(|| landed(SHELL_SLACK)) {
                        let shell = self.shells.remove(index);
                        let radius = splash.map_or(1, tiles);
                        sources.splash.push((at, radius, shell.source));
                    }
                }
                Event::AircraftImpacted { crash } => {
                    if let Some(profile) = crash.kind.crash_profile() {
                        sources.splash.push((
                            TilePos::containing(crash.impact),
                            tiles(profile.radius),
                            Source {
                                key: Key::Unit(crash.unit),
                                owner: crash.player.0,
                                team: self.teams[usize::from(crash.player.0)],
                                vision: 0,
                                from: None,
                            },
                        ));
                    }
                }
                Event::ChargeDetonated { building, at, .. } => {
                    sources.detonated.push(Key::Building(*building));
                    if let Some(source) = self.source(Key::Building(*building), None) {
                        sources.splash.push((
                            TilePos::containing(*at),
                            tiles(CHARGE_BLAST_RADIUS),
                            source,
                        ));
                    }
                }
                _ => {}
            }
        }
        self.shells.retain(|shell| shell.lands + SHELL_SLACK >= now);
        sources
    }

    fn repair(&mut self, patient: Key, welder: Key, amount: u32, sources: &mut Sources) {
        *sources.repaired.entry(patient).or_default() += amount;
        let Some(body) = self.body(patient).copied() else {
            return;
        };
        let value = u64::from(amount) * body.value * MILLI / u64::from(body.max_hp.max(1));
        if let Some(record) = self.records.get_mut(&welder) {
            record.repaired += value;
        }
    }

    fn gone(&mut self, key: Key, now: u64) {
        if let Some(record) = self.records.get_mut(&key) {
            record.gone = Some(now);
        }
    }

    fn spent(&mut self, seat: u8, what: String, now: u64, scrap: u64) {
        self.spend[usize::from(seat)].entry(what).or_default()[phase(now)] += scrap;
    }

    /// Buildings placed and upgraded this tick, and bodies that appeared
    /// without being trained.
    fn spending(&mut self, after: &[Body], now: u64) {
        for body in after {
            let before = self.body(body.key).copied();
            match (before, body.building) {
                (None, _) if !self.records.contains_key(&body.key) => {
                    self.records.insert(body.key, record(body, false));
                    if let Some((kind, tier, _)) = body.building {
                        let record = self.records.get_mut(&body.key).expect("just inserted");
                        record.born = now;
                        record.paid = body.value;
                        record.top_tier = tier;
                        self.spent(
                            body.owner,
                            format!("{}:{}", category(kind), kind.name()),
                            now,
                            body.value,
                        );
                    }
                }
                (Some(before), Some((kind, tier, _))) => {
                    let Some((_, old, _)) = before.building else {
                        continue;
                    };
                    if tier > old {
                        let cost = body.value.saturating_sub(before.value);
                        if let Some(record) = self.records.get_mut(&body.key) {
                            record.paid += cost;
                            record.top_tier = record.top_tier.max(tier);
                        }
                        self.spent(
                            body.owner,
                            format!("upgrade:{}", kind.tier_name(tier)),
                            now,
                            cost,
                        );
                    }
                }
                _ => {}
            }
        }
    }

    /// Splits each body's health lost this tick among the sources that hit
    /// it, including units trained and killed within the tick.
    fn damage(&mut self, after: &[Body], sources: &Sources, now: u64) {
        let mut cursor = 0;
        let before = std::mem::take(&mut self.bodies);
        for pre in &before {
            while cursor < after.len() && after[cursor].key < pre.key {
                cursor += 1;
            }
            // A charge spending itself in its own blast is no one's damage.
            if sources.detonated.contains(&pre.key) {
                continue;
            }
            let gained = sources.repaired.get(&pre.key).copied().unwrap_or(0);
            let lost = match after.get(cursor) {
                // An upgrade rebuilds the building; what it loses to that is
                // no one's damage.
                Some(post) if post.key == pre.key && post.tier() != pre.tier() => continue,
                Some(post) if post.key == pre.key => {
                    // Construction and salvage move health without events.
                    let Some(built) = pre.built_gain(post) else {
                        continue;
                    };
                    let drained = post.salvage.saturating_sub(pre.salvage);
                    (pre.hp + gained + built).saturating_sub(post.hp + drained)
                }
                _ if self
                    .records
                    .get(&pre.key)
                    .is_some_and(|record| record.gone == Some(now)) =>
                {
                    pre.hp + gained
                }
                _ => continue,
            };
            let destroyed = !matches!(after.get(cursor), Some(post) if post.key == pre.key);
            self.attribute(&before, pre, lost, destroyed, sources);
        }
        // A unit trained this tick starts at full health, whether it lived
        // through the tick or died in it.
        for &(key, kind, owner) in &sources.born {
            if find(&before, key).is_some() {
                continue;
            }
            let stats = kind.stats();
            let (tile, hp, destroyed) = match (find(after, key), sources.died.get(&key)) {
                (Some(post), _) => (post.tile, post.hp, false),
                (None, Some(&tile)) => (tile, 0, true),
                (None, None) => continue,
            };
            let body = Body {
                key,
                owner,
                team: self.teams[usize::from(owner)],
                hp: stats.max_hp,
                max_hp: stats.max_hp,
                value: u64::from(stats.cost),
                tile,
                vision: stats.vision,
                class: class(kind),
                unit: Some(kind),
                building: None,
                carrier: None,
                progress: 0,
                build_ticks: 0,
                salvage: 0,
            };
            self.attribute(&before, &body, stats.max_hp - hp, destroyed, sources);
        }
        self.bodies = before;
    }

    /// Credits `lost` health of `pre`, which ended the tick destroyed when
    /// `destroyed`, to the sources that hit it this tick.
    fn attribute(
        &mut self,
        before: &[Body],
        pre: &Body,
        lost: u32,
        destroyed: bool,
        sources: &Sources,
    ) {
        if lost == 0 {
            return;
        }
        let value = u64::from(lost) * pre.value * MILLI / u64::from(pre.max_hp.max(1));
        let mut from: Vec<Source> = sources.aimed.get(&pre.key).cloned().unwrap_or_default();
        // A rider dies to whatever brought its carrier down.
        if let Some(carrier) = pre.carrier {
            from.extend(sources.aimed.get(&carrier).into_iter().flatten().copied());
        }
        from.extend(
            sources
                .splash
                .iter()
                .filter(|(at, radius, _)| within(*at, pre.tile, *radius))
                .map(|(_, _, source)| *source),
        );
        from.extend(
            sources
                .blind
                .iter()
                .filter(|(at, _)| at.chebyshev(pre.tile) <= 1)
                .map(|(_, source)| *source),
        );
        from.retain(|source| source.team != pre.team);
        // A source counts once per victim per tick: a Sapper's blast reports
        // its target both as aimed and inside its splash.
        from.sort_by_key(|source| source.key);
        from.dedup_by_key(|source| source.key);
        if from.is_empty() {
            // An unfinished site that loses health with no one firing is
            // decaying, not under attack.
            if !matches!(pre.building, Some((_, _, false))) {
                self.unattributed[usize::from(pre.owner)] += value;
                if let Some(record) = self.records.get_mut(&pre.key) {
                    record.taken += value;
                }
            }
            return;
        }
        if let Some(record) = self.records.get_mut(&pre.key) {
            record.taken += value;
        }
        let share = value / from.len() as u64;
        for source in &from {
            let place = place(before, source.owner, pre.owner, pre.tile);
            if let Some(record) = self.records.get_mut(&source.key) {
                let dealt = &mut record.dealt;
                *match pre.class {
                    Class::Army => &mut dealt.army,
                    Class::Worker => &mut dealt.workers,
                    Class::Other => &mut dealt.other,
                    Class::Building => &mut dealt.buildings,
                } += share;
                *match place {
                    Place::Home => &mut dealt.home,
                    Place::Away => &mut dealt.away,
                    Place::Field => &mut dealt.field,
                } += share;
            }
            if let Some(origin) = source.from
                && !within(origin, pre.tile, source.vision)
                && let Some(spotter) = before
                    .iter()
                    .filter(|body| {
                        body.team == source.team
                            && body.key != source.key
                            && within(body.tile, pre.tile, body.vision)
                    })
                    .min_by_key(|body| {
                        let (dx, dy) = (body.tile.x - pre.tile.x, body.tile.y - pre.tile.y);
                        (dx * dx + dy * dy, body.key)
                    })
                && let Some(record) = self.records.get_mut(&spotter.key)
            {
                record.enabled += share;
            }
        }
        let charge = matches!(pre.building, Some((BuildingKind::ScuttleCharge, _, _)));
        if charge && destroyed {
            let team = from[0].team;
            let array = before
                .iter()
                .filter(|body| body.team == team)
                .filter(|body| match body.building {
                    Some((BuildingKind::Array, tier, true)) => within(
                        body.tile,
                        pre.tile,
                        if tier >= 1 {
                            CHARGE_ARRAY_DETECT_RADIUS
                        } else {
                            CHARGE_BASE_ARRAY_DETECT_RADIUS
                        },
                    ),
                    _ => false,
                })
                .min_by_key(|body| body.key);
            if let Some(array) = array
                && let Some(record) = self.records.get_mut(&array.key)
            {
                record.enabled += pre.value * MILLI;
            }
        }
    }

    /// Credits the income the simulation pays without an event, by the same
    /// rules and cadence: Reclaimers and the Foundry drip as buildings stood
    /// when the tick began, before any combat in it; Extractors by the
    /// support the state reports; and Crucibles that had a wreck in reach.
    fn passive(&mut self, state: &State, now: u64) {
        let completed = now + 1;
        let mut credits: Vec<(Key, u64)> = Vec::new();
        for body in &self.bodies {
            let Some((kind, tier, true)) = body.building else {
                continue;
            };
            if body.hp == 0 {
                continue;
            }
            let resigned = state.players()[usize::from(body.owner)].resigned;
            let scrap = match kind {
                BuildingKind::Reclaimer if tier == 0 => {
                    u64::from(now.is_multiple_of(RECLAIMER_PERIOD))
                }
                BuildingKind::Reclaimer if tier == 1 => {
                    u64::from(now.is_multiple_of(REFINERY_PERIOD))
                }
                BuildingKind::Foundry
                    if !resigned
                        && completed >= FOUNDRY_DRIP_START_TICK
                        && completed.is_multiple_of(FOUNDRY_DRIP_PERIOD) =>
                {
                    1
                }
                _ => 0,
            };
            if scrap > 0 {
                credits.push((body.key, scrap));
            }
        }
        for building in state.buildings() {
            if building.kind != BuildingKind::Extractor
                || !building.built
                || building.hp == 0
                || state.player(building.player).resigned
            {
                continue;
            }
            let (amount, period) = match state.extractor_income(building.id) {
                Some(ExtractorIncome::Supported) => EXTRACTOR_SUPPORTED_YIELD,
                Some(ExtractorIncome::Remote) => EXTRACTOR_REMOTE_YIELD,
                None => continue,
            };
            if completed.is_multiple_of(period) {
                credits.push((Key::Building(building.id), u64::from(amount)));
            }
        }
        if now.is_multiple_of(CRUCIBLE_SMELT_PERIOD) {
            credits.extend(self.smelting.iter().map(|key| (*key, 1)));
        }
        for (key, scrap) in credits {
            if let Some(record) = self.records.get_mut(&key) {
                record.passive += scrap;
            }
        }
    }

    /// Each seat's ledger, ending with `state`, the state the last observed
    /// tick left.
    pub fn finish(self, state: &State) -> Vec<SeatLedger> {
        let end = state.current_tick();
        let mut seats: Vec<SeatLedger> = (0..self.spend.len())
            .map(|seat| SeatLedger {
                unattributed: scrap(self.unattributed[seat]),
                spend: self.spend[seat].clone(),
                refunds: self.refunds[seat],
                worth: self.worth[seat].clone(),
                start: self.start,
                end,
                final_worth: worth(state, &self.bodies, seat),
                ..SeatLedger::default()
            })
            .collect();
        for record in self.records.values() {
            let Some(seat) = seats.get_mut(usize::from(record.owner)) else {
                continue;
            };
            let (table, name) = match (record.unit, record.building) {
                (Some(kind), _) => (&mut seat.units, kind.name()),
                (None, Some(kind)) => (&mut seat.buildings, kind.tier_name(record.top_tier)),
                (None, None) => continue,
            };
            let entry = table.entry(name.to_owned()).or_default();
            if record.starting {
                entry.starting += 1;
            } else {
                entry.built += 1;
            }
            entry.paid += record.paid;
            entry.dealt.add(&record.dealt);
            entry.taken += record.taken;
            entry.enabled += record.enabled;
            entry.repaired += record.repaired;
            entry.harvested += record.harvested;
            entry.received += record.received;
            entry.passive += record.passive;
            entry.produced += record.produced;
            if let Some(gone) = record.gone {
                entry.deaths += 1;
                entry.lifetime += gone.min(end).saturating_sub(record.born);
            }
        }
        for seat in &mut seats {
            for totals in seat.units.values_mut().chain(seat.buildings.values_mut()) {
                let dealt = &mut totals.dealt;
                for value in [
                    &mut dealt.army,
                    &mut dealt.workers,
                    &mut dealt.other,
                    &mut dealt.buildings,
                    &mut dealt.home,
                    &mut dealt.away,
                    &mut dealt.field,
                    &mut totals.taken,
                    &mut totals.enabled,
                    &mut totals.repaired,
                ] {
                    *value = scrap(*value);
                }
            }
        }
        seats
    }
}

/// Thousandths of a scrap rounded to whole scrap.
pub fn scrap(milli: u64) -> u64 {
    (milli + MILLI / 2) / MILLI
}

#[derive(Clone, Copy)]
enum Place {
    Home,
    Away,
    Field,
}

/// Where a loss to `victim`'s body at `tile` happened, from `dealer`'s view.
fn place(bodies: &[Body], dealer: u8, victim: u8, tile: TilePos) -> Place {
    let near = |seat: u8| {
        bodies
            .iter()
            .filter(|body| {
                body.owner == seat
                    && !matches!(
                        body.building,
                        None | Some((BuildingKind::ScuttleCharge | BuildingKind::Barricade, _, _))
                    )
            })
            .map(|body| body.tile.chebyshev(tile))
            .min()
            .unwrap_or(i32::MAX)
    };
    let (own, theirs) = (near(dealer), near(victim));
    if own <= PLACE_TILES && own <= theirs {
        Place::Home
    } else if theirs <= PLACE_TILES {
        Place::Away
    } else {
        Place::Field
    }
}

/// The spending category of a building kind.
fn category(kind: BuildingKind) -> &'static str {
    match kind {
        BuildingKind::Foundry | BuildingKind::Extractor | BuildingKind::Reclaimer => "economy",
        BuildingKind::Turret
        | BuildingKind::FlakTurret
        | BuildingKind::Bastion
        | BuildingKind::Barricade
        | BuildingKind::ScuttleCharge => "defense",
        _ => "tech",
    }
}

fn find(bodies: &[Body], key: Key) -> Option<&Body> {
    bodies
        .binary_search_by_key(&key, |body| body.key)
        .ok()
        .map(|index| &bodies[index])
}

fn aim(sources: &mut Sources, target: Option<Target>, at: TilePos, source: Source) {
    match target {
        Some(Target::Unit(id)) => sources.aimed.entry(Key::Unit(id)).or_default().push(source),
        Some(Target::Building(id)) => sources
            .aimed
            .entry(Key::Building(id))
            .or_default()
            .push(source),
        None => sources.blind.push((at, source)),
    }
}

fn record(body: &Body, starting: bool) -> Record {
    Record {
        owner: body.owner,
        unit: body.unit,
        building: body.building.map(|(kind, _, _)| kind),
        top_tier: body.building.map_or(0, |(_, tier, _)| tier),
        starting,
        paid: if starting { body.value } else { 0 },
        ..Record::default()
    }
}

/// Phase names for spending, by the minute each begins.
const PHASE_NAMES: [&str; 4] = ["0-5", "5-10", "10-20", "20+"];

/// Ledgers of several seats, pooled.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LedgerPool {
    /// Seat ledgers pooled.
    pub seats: u64,
    /// Totals by unit kind.
    pub units: BTreeMap<String, KindLedger>,
    /// Totals by building kind at its highest tier.
    pub buildings: BTreeMap<String, KindLedger>,
    /// Damage the seats took that no hit accounts for, in scrap.
    pub unattributed: u64,
    /// Scrap spent by `<category>:<kind>` and phase.
    pub spend: BTreeMap<String, [u64; 4]>,
}

impl LedgerPool {
    /// Adds one seat's ledger.
    pub fn add(&mut self, ledger: &SeatLedger) {
        self.seats += 1;
        for (kind, totals) in &ledger.units {
            self.units.entry(kind.clone()).or_default().add(totals);
        }
        for (kind, totals) in &ledger.buildings {
            self.buildings.entry(kind.clone()).or_default().add(totals);
        }
        self.unattributed += ledger.unattributed;
        for (what, phases) in &ledger.spend {
            let entry = self.spend.entry(what.clone()).or_default();
            for (sum, scrap) in entry.iter_mut().zip(phases) {
                *sum += scrap;
            }
        }
    }

    /// Scrap earned by source: worker deliveries, then each building kind's
    /// passive credits, most first.
    pub fn income(&self) -> Vec<(String, u64)> {
        let harvest: u64 = self.units.values().map(|totals| totals.harvested).sum();
        let mut income: Vec<(String, u64)> = vec![("harvest".to_owned(), harvest)];
        income.extend(
            self.buildings
                .iter()
                .filter(|(_, totals)| totals.passive > 0)
                .map(|(kind, totals)| (kind.clone(), totals.passive)),
        );
        income.sort_by_key(|(name, scrap)| (std::cmp::Reverse(*scrap), name.clone()));
        income
    }

    /// Renders unit and building tables, the first `limit` kinds of each by
    /// scrap paid, then income sources, spending by phase and the
    /// unattributed share, each line led by `indent`.
    pub fn render(&self, out: &mut String, indent: &str, limit: Option<usize>) {
        let limit = limit.unwrap_or(usize::MAX);
        let unit_spend: u64 = self.units.values().map(|totals| totals.paid).sum();
        let _ = writeln!(
            out,
            "{indent}{:<12} {:>6} {:>6} {:>6} | {:>5} {:>5} {:>5} {:>5} | {:>5} {:>5} {:>5} | {:>7} {:>8} {:>5} {:>6}",
            "unit",
            "built",
            "spend",
            "dealt",
            "army",
            "wrkr",
            "other",
            "bldg",
            "home",
            "field",
            "away",
            "enabled",
            "repaired",
            "died",
            "life s"
        );
        let mut units: Vec<(&String, &KindLedger)> = self.units.iter().collect();
        units.sort_by_key(|(kind, totals)| (std::cmp::Reverse(totals.paid), (*kind).clone()));
        for (kind, totals) in units.into_iter().take(limit) {
            let dealt = totals.dealt.total();
            let _ = writeln!(
                out,
                "{indent}{:<12} {:>6} {:>6} {:>6} | {:>5} {:>5} {:>5} {:>5} | {:>5} {:>5} {:>5} | {:>7} {:>8} {:>5} {:>6}",
                kind,
                totals.built + totals.starting,
                share(totals.paid, unit_spend),
                ratio(dealt, totals.paid),
                share(totals.dealt.army, dealt),
                share(totals.dealt.workers, dealt),
                share(totals.dealt.other, dealt),
                share(totals.dealt.buildings, dealt),
                share(totals.dealt.home, dealt),
                share(totals.dealt.field, dealt),
                share(totals.dealt.away, dealt),
                ratio(totals.enabled, totals.paid),
                ratio(totals.repaired, totals.paid),
                share(totals.deaths, totals.built + totals.starting),
                life(totals),
            );
        }
        let _ = writeln!(
            out,
            "{indent}{:<14} {:>6} {:>8} {:>6} {:>7} {:>8} {:>8} {:>8} {:>9}",
            "building",
            "built",
            "paid",
            "dealt",
            "income",
            "produced",
            "enabled",
            "repaired",
            "destroyed"
        );
        let mut buildings: Vec<(&String, &KindLedger)> = self.buildings.iter().collect();
        buildings.sort_by_key(|(kind, totals)| (std::cmp::Reverse(totals.paid), (*kind).clone()));
        for (kind, totals) in buildings.into_iter().take(limit) {
            let _ = writeln!(
                out,
                "{indent}{:<14} {:>6} {:>8} {:>6} {:>7} {:>8} {:>8} {:>8} {:>9}",
                kind,
                totals.built + totals.starting,
                totals.paid,
                ratio(totals.dealt.total(), totals.paid),
                ratio(totals.passive + totals.received, totals.paid),
                ratio(totals.produced, totals.paid),
                ratio(totals.enabled, totals.paid),
                ratio(totals.repaired, totals.paid),
                share(totals.deaths, totals.built + totals.starting),
            );
        }
        let income = self.income();
        let earned: u64 = income.iter().map(|(_, scrap)| scrap).sum();
        let _ = writeln!(
            out,
            "{indent}income {:.0} per seat-game: {}",
            earned as f64 / self.seats.max(1) as f64,
            income
                .iter()
                .filter(|(_, scrap)| *scrap > 0)
                .map(|(name, scrap)| format!("{name} {}", share(*scrap, earned)))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let mut categories: BTreeMap<&str, [u64; 4]> = BTreeMap::new();
        for (what, phases) in &self.spend {
            let category = what.split(':').next().unwrap_or(what);
            let entry = categories.entry(category).or_default();
            for (sum, scrap) in entry.iter_mut().zip(phases) {
                *sum += scrap;
            }
        }
        for (index, phase) in PHASE_NAMES.iter().enumerate() {
            let total: u64 = categories.values().map(|phases| phases[index]).sum();
            if total == 0 {
                continue;
            }
            let mut parts: Vec<(&str, u64)> = categories
                .iter()
                .map(|(category, phases)| (*category, phases[index]))
                .filter(|(_, scrap)| *scrap > 0)
                .collect();
            parts.sort_by_key(|(category, scrap)| (std::cmp::Reverse(*scrap), *category));
            let _ = writeln!(
                out,
                "{indent}spend, minutes {phase:<5}: {}",
                parts
                    .iter()
                    .map(|(category, scrap)| format!("{category} {}", share(*scrap, total)))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        let taken: u64 = self
            .units
            .values()
            .chain(self.buildings.values())
            .map(|totals| totals.taken)
            .sum();
        let _ = writeln!(
            out,
            "{indent}damage taken with no shooter found: {}",
            share(self.unattributed, taken)
        );
    }
}

/// A seat's net worth at `tick`: its final worth once the ledger has ended,
/// else the last sample at or before it; `None` before the first. Rows that
/// predate the final worth carry their last sample forward instead.
pub fn worth_at(ledger: &SeatLedger, tick: u64) -> Option<u64> {
    if ledger.end > 0 && tick >= ledger.end {
        return Some(ledger.final_worth);
    }
    let samples = usize::try_from(tick.checked_sub(ledger.start)? / WORTH_PERIOD).ok()?;
    if samples == 0 {
        return None;
    }
    let worth = &ledger.worth;
    worth.get(samples - 1).or_else(|| worth.last()).copied()
}

fn ratio(numerator: u64, denominator: u64) -> String {
    if denominator == 0 {
        "-".to_owned()
    } else {
        format!("{:.2}", numerator as f64 / denominator as f64)
    }
}

/// `part` as a whole percentage of `whole`, or `-` when `whole` is zero.
pub fn share(part: u64, whole: u64) -> String {
    if whole == 0 {
        "-".to_owned()
    } else {
        format!("{:.0}%", part as f64 * 100.0 / whole as f64)
    }
}

fn life(totals: &KindLedger) -> String {
    if totals.deaths == 0 {
        "-".to_owned()
    } else {
        format!(
            "{:.0}",
            totals.lifetime as f64 / totals.deaths as f64 / f64::from(oxide_sim::TICKS_PER_SECOND)
        )
    }
}

/// Ticks at which reports compare seats' net worth.
pub const WORTH_TICKS: [u64; 4] = [6_000, 12_000, 18_000, 24_000];

/// Per mille of all net worth that `team` held at each of [`WORTH_TICKS`],
/// from each seat's ledger and team; `None` unless every seat has a ledger.
pub fn team_shares(
    ledgers: &[Option<&SeatLedger>],
    teams: &[u8],
    team: u8,
) -> Option<[Option<u32>; 4]> {
    let ledgers: Vec<&SeatLedger> = ledgers.iter().copied().collect::<Option<_>>()?;
    Some(WORTH_TICKS.map(|tick| {
        let (mut own, mut all) = (0_u64, 0_u64);
        for (ledger, seat_team) in ledgers.iter().zip(teams) {
            let worth = worth_at(ledger, tick)?;
            all += worth;
            if *seat_team == team {
                own += worth;
            }
        }
        (all > 0).then(|| u32::try_from(own * 1_000 / all).unwrap_or(1_000))
    }))
}

/// A side's mean share of net worth at one tick over seat-swapped pairs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorthShare {
    /// Tick compared.
    pub tick: u64,
    /// Pairs with a share at that tick.
    pub pairs: u32,
    /// Mean share, 0 to 1.
    pub mean: f64,
    /// 95% interval of the mean over pairs, absent with fewer than two.
    pub interval: Option<[f64; 2]>,
}

/// Each pair's legs' shares at [`WORTH_TICKS`], from [`team_shares`], keyed
/// by pair.
pub type PairShares = BTreeMap<String, Vec<[Option<u32>; 4]>>;

/// Mean shares at each of [`WORTH_TICKS`] over pairs: each pair's legs are
/// averaged first, so a pair counts once.
pub fn worth_shares(pairs: &PairShares) -> Vec<WorthShare> {
    WORTH_TICKS
        .iter()
        .enumerate()
        .filter_map(|(index, tick)| {
            let means: Vec<f64> = pairs
                .values()
                .filter_map(|legs| {
                    let shares: Vec<f64> = legs
                        .iter()
                        .filter_map(|leg| leg[index])
                        .map(|share| f64::from(share) / 1_000.0)
                        .collect();
                    (!shares.is_empty()).then(|| shares.iter().sum::<f64>() / shares.len() as f64)
                })
                .collect();
            if means.is_empty() {
                return None;
            }
            let n = means.len() as f64;
            let mean = means.iter().sum::<f64>() / n;
            let interval = (means.len() >= 2).then(|| {
                let variance = means
                    .iter()
                    .map(|value| (value - mean).powi(2))
                    .sum::<f64>()
                    / (n - 1.0);
                let half = 1.96 * (variance / n).sqrt();
                [mean - half, mean + half]
            });
            Some(WorthShare {
                tick: *tick,
                pairs: means.len() as u32,
                mean,
                interval,
            })
        })
        .collect()
}

/// The shares as one line: `6k 52% [48-56%]`, and so on.
pub fn render_shares(shares: &[WorthShare]) -> String {
    shares
        .iter()
        .map(|share| {
            let interval = share.interval.map_or_else(String::new, |[low, high]| {
                format!(" [{:.0}-{:.0}%]", low * 100.0, high * 100.0)
            });
            format!(
                "{}k {:.0}%{interval}",
                share.tick / 1_000,
                share.mean * 100.0
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_sim::scenario::{BuildingSpec, PlayerSpec, ScenarioMode, UnitSpec};
    use oxide_sim::{AttackTarget, Command, Faction, PlayerCommand, PlayerId, Scenario};

    /// A 32-wide, 12-deep field: West's start at (2, 2) and East's at
    /// (28, 2), each seat with `scrap`, plus the given units and buildings.
    fn field(
        scrap: u32,
        units: &[(u8, UnitKind, i32, i32)],
        buildings: &[(u8, BuildingKind, i32, i32)],
    ) -> Scenario {
        let ground = ".".repeat(32);
        let mut map = vec![ground.clone(); 12];
        let mut top: Vec<char> = ground.chars().collect();
        top[2] = '1';
        top[28] = '2';
        map[2] = top.into_iter().collect();
        Scenario {
            mode: ScenarioMode::Match,
            name: "ledger".into(),
            seed: 5,
            map,
            players: [Faction::Ferrous, Faction::Cupric]
                .into_iter()
                .enumerate()
                .map(|(seat, faction)| PlayerSpec {
                    name: format!("seat {seat}"),
                    faction,
                    team: None,
                    scrap,
                    bot: false,
                    bot_config: None,
                })
                .collect(),
            units: units
                .iter()
                .map(|&(player, kind, x, y)| UnitSpec { player, kind, x, y })
                .collect(),
            buildings: buildings
                .iter()
                .map(|&(player, kind, x, y)| BuildingSpec { player, kind, x, y })
                .collect(),
            meta: None,
        }
    }

    /// Plays `state` for `ticks`, issuing `orders` on the first tick, and
    /// returns the ledgers.
    fn play(mut state: State, ticks: u64, orders: Vec<PlayerCommand>) -> (State, Vec<SeatLedger>) {
        let mut ledger = ImpactLedger::new(&state);
        let mut orders = Some(orders);
        for _ in 0..ticks {
            let report = state.tick(&orders.take().unwrap_or_default());
            ledger.observe(&state, &report);
        }
        let ledgers = ledger.finish(&state);
        (state, ledgers)
    }

    fn unit_at(state: &State, x: i32, y: i32) -> UnitId {
        state
            .units()
            .iter()
            .find(|unit| unit.tile() == TilePos::new(x, y))
            .unwrap()
            .id
    }

    fn building(state: &State, player: u8, kind: BuildingKind) -> BuildingId {
        state
            .buildings()
            .iter()
            .find(|building| building.player == PlayerId(player) && building.kind == kind)
            .unwrap()
            .id
    }

    fn order(player: u8, command: Command) -> PlayerCommand {
        PlayerCommand {
            player: PlayerId(player),
            command,
        }
    }

    fn unit(ledgers: &[SeatLedger], seat: usize, kind: &str) -> KindLedger {
        ledgers[seat].units.get(kind).cloned().unwrap_or_default()
    }

    fn built(ledgers: &[SeatLedger], seat: usize, kind: &str) -> KindLedger {
        ledgers[seat]
            .buildings
            .get(kind)
            .cloned()
            .unwrap_or_default()
    }

    #[test]
    fn a_kill_is_credited_to_its_shooter_at_the_victims_worth() {
        let scenario = field(
            0,
            &[
                (0, UnitKind::Sentinel, 14, 6),
                (1, UnitKind::Harvester, 16, 6),
            ],
            &[],
        );
        let (_, ledgers) = play(scenario.build().unwrap(), 600, Vec::new());
        let sentinel = unit(&ledgers, 0, "sentinel");
        let harvester = unit(&ledgers, 1, "harvester");
        let cost = u64::from(UnitKind::Harvester.stats().cost);
        assert_eq!(harvester.taken, cost, "{harvester:?}");
        assert_eq!(harvester.deaths, 1);
        assert_eq!(sentinel.dealt.workers, cost, "{sentinel:?}");
        assert_eq!(sentinel.dealt.total(), cost);
        assert_eq!(
            sentinel.dealt.field + sentinel.dealt.home + sentinel.dealt.away,
            cost
        );
        assert_eq!(ledgers[1].unattributed, 0);
    }

    #[test]
    fn shooters_that_hit_one_victim_split_its_loss() {
        let scenario = field(
            0,
            &[
                (0, UnitKind::Sentinel, 14, 5),
                (0, UnitKind::Warden, 14, 7),
                (1, UnitKind::Sentinel, 16, 6),
            ],
            &[],
        );
        let (_, ledgers) = play(scenario.build().unwrap(), 600, Vec::new());
        let (sentinel, warden) = (unit(&ledgers, 0, "sentinel"), unit(&ledgers, 0, "warden"));
        let victim = unit(&ledgers, 1, "sentinel");
        assert_eq!(victim.deaths, 1);
        assert!(sentinel.dealt.army > 0 && warden.dealt.army > 0);
        assert!(
            (sentinel.dealt.army + warden.dealt.army).abs_diff(victim.taken) <= 1,
            "{sentinel:?} {warden:?} {victim:?}"
        );
    }

    #[test]
    fn a_shell_is_credited_to_its_gun_and_to_the_spotter_that_saw_its_target() {
        let scenario = field(
            0,
            &[(0, UnitKind::Bombard, 8, 6), (0, UnitKind::Kestrel, 15, 6)],
            &[(1, BuildingKind::Barricade, 16, 6)],
        );
        let mut state = scenario.build().unwrap();
        state.tick(&[]);
        let bombard = unit_at(&state, 8, 6);
        let barricade = building(&state, 1, BuildingKind::Barricade);
        assert!(
            UnitKind::Bombard.stats().vision < 8,
            "premise: beyond its sight"
        );
        let (_, ledgers) = play(
            state,
            1_200,
            vec![order(
                0,
                Command::Attack {
                    units: vec![bombard],
                    target: AttackTarget::Building(barricade),
                    queue: false,
                },
            )],
        );
        let gun = unit(&ledgers, 0, "bombard");
        assert!(gun.dealt.buildings > 0, "{gun:?}");
        assert!(unit(&ledgers, 0, "kestrel").enabled > 0);
        assert_eq!(gun.enabled, 0);
    }

    #[test]
    fn a_charge_is_credited_with_what_its_blast_does() {
        let scenario = field(
            0,
            &[(0, UnitKind::Sentinel, 12, 6)],
            &[(1, BuildingKind::ScuttleCharge, 16, 6)],
        );
        let mut state = scenario.build().unwrap();
        state.tick(&[]);
        let sentinel = unit_at(&state, 12, 6);
        let (_, ledgers) = play(
            state,
            400,
            vec![order(
                0,
                Command::Run {
                    units: vec![sentinel],
                    goal: TilePos::new(20, 6),
                    queue: false,
                },
            )],
        );
        let charge = built(&ledgers, 1, "scuttle charge");
        assert!(charge.dealt.army > 0, "{charge:?}");
        assert_eq!(charge.dealt.army, unit(&ledgers, 0, "sentinel").taken);
        assert_eq!(
            (charge.taken, ledgers[1].unattributed),
            (0, 0),
            "a charge spending itself is no one's damage"
        );
    }

    #[test]
    fn a_sapper_is_credited_with_its_blast() {
        let scenario = field(
            0,
            &[(0, UnitKind::Sapper, 14, 6)],
            &[(1, BuildingKind::Barricade, 16, 6)],
        );
        let mut state = scenario.build().unwrap();
        state.tick(&[]);
        let sapper = unit_at(&state, 14, 6);
        let barricade = building(&state, 1, BuildingKind::Barricade);
        let (_, ledgers) = play(
            state,
            400,
            vec![order(
                0,
                Command::Attack {
                    units: vec![sapper],
                    target: AttackTarget::Building(barricade),
                    queue: false,
                },
            )],
        );
        let sapper = unit(&ledgers, 0, "sapper");
        assert!(sapper.dealt.buildings > 0, "{sapper:?}");
        assert_eq!(sapper.deaths, 1, "it blows itself up");
    }

    #[test]
    fn repair_is_credited_to_whoever_supplied_it() {
        let scenario = field(
            500,
            &[
                (0, UnitKind::Sentinel, 7, 6),
                (0, UnitKind::Sentinel, 14, 9),
                (0, UnitKind::Harvester, 15, 9),
            ],
            &[(0, BuildingKind::RepairBay, 5, 6)],
        );
        let mut value = serde_json::to_value(scenario.build().unwrap()).unwrap();
        for unit in value["units"].as_array_mut().unwrap() {
            if unit["kind"] == "sentinel" {
                unit["hp"] = serde_json::json!(20);
            }
        }
        let mut state: State = serde_json::from_value(value).unwrap();
        state.tick(&[]);
        let (welder, patient) = (unit_at(&state, 15, 9), unit_at(&state, 14, 9));
        let (_, ledgers) = play(
            state,
            1_200,
            vec![order(
                0,
                Command::RepairUnit {
                    units: vec![welder],
                    target: patient,
                    queue: false,
                },
            )],
        );
        assert!(built(&ledgers, 0, "repair bay").repaired > 0);
        assert!(unit(&ledgers, 0, "harvester").repaired > 0);
    }

    #[test]
    fn an_array_is_credited_with_a_hidden_charge_it_revealed() {
        let scenario = field(
            0,
            &[(0, UnitKind::Lancer, 12, 6)],
            &[
                (0, BuildingKind::Array, 12, 8),
                (1, BuildingKind::ScuttleCharge, 16, 6),
            ],
        );
        let mut state = scenario.build().unwrap();
        state.tick(&[]);
        let lancer = unit_at(&state, 12, 6);
        let charge = building(&state, 1, BuildingKind::ScuttleCharge);
        let (_, ledgers) = play(
            state,
            600,
            vec![order(
                0,
                Command::Attack {
                    units: vec![lancer],
                    target: AttackTarget::Building(charge),
                    queue: false,
                },
            )],
        );
        assert_eq!(built(&ledgers, 1, "scuttle charge").deaths, 1);
        assert_eq!(
            built(&ledgers, 0, "array").enabled,
            building_value(BuildingKind::ScuttleCharge, 0)
        );
    }

    #[test]
    fn passive_income_matches_the_bank() {
        // A harvester keeps the seat off the recovery trickle, which the
        // ledger cannot credit.
        let scenario = field(
            0,
            &[(0, UnitKind::Harvester, 4, 8)],
            &[
                (0, BuildingKind::Reclaimer, 6, 6),
                (0, BuildingKind::Reclaimer, 8, 6),
            ],
        );
        let mut value = serde_json::to_value(scenario.build().unwrap()).unwrap();
        let reclaimer = value["buildings"]
            .as_array()
            .unwrap()
            .iter()
            .position(|building| building["kind"] == "reclaimer")
            .unwrap();
        value["buildings"][reclaimer]["tier"] = serde_json::json!(1);
        let state: State = serde_json::from_value(value).unwrap();
        let before = state.players()[0].scrap;
        let (state, ledgers) = play(state, 3_000, Vec::new());
        let passive: u64 = ledgers[0]
            .buildings
            .values()
            .map(|totals| totals.passive)
            .sum();
        assert_eq!(passive, u64::from(state.players()[0].scrap - before));
        assert!(built(&ledgers, 0, "refinery").passive > built(&ledgers, 0, "reclaimer").passive);
        assert!(built(&ledgers, 0, "foundry").passive > 0, "the drip");
    }

    #[test]
    fn spending_is_recorded_by_category_and_phase() {
        let scenario = field(
            1_000,
            &[(0, UnitKind::Harvester, 6, 8)],
            &[
                (0, BuildingKind::Fabricator, 8, 4),
                (0, BuildingKind::Reclaimer, 12, 4),
            ],
        );
        let mut state = scenario.build().unwrap();
        state.tick(&[]);
        let foundry = building(&state, 0, BuildingKind::Foundry);
        let reclaimer = building(&state, 0, BuildingKind::Reclaimer);
        let harvester = unit_at(&state, 6, 8);
        let (_, ledgers) = play(
            state,
            1_200,
            vec![
                order(
                    0,
                    Command::Train {
                        building: foundry,
                        kind: UnitKind::Sentinel,
                    },
                ),
                order(
                    0,
                    Command::Build {
                        units: vec![harvester],
                        kind: BuildingKind::Turret,
                        anchor: TilePos::new(6, 10),
                        queue: false,
                        defer: false,
                    },
                ),
                order(
                    0,
                    Command::UpgradeBuilding {
                        building: reclaimer,
                    },
                ),
            ],
        );
        let spend = &ledgers[0].spend;
        assert_eq!(
            spend["army:sentinel"][0],
            u64::from(UnitKind::Sentinel.stats().cost)
        );
        assert_eq!(
            spend["defense:turret"][0],
            building_value(BuildingKind::Turret, 0)
        );
        assert_eq!(
            spend["upgrade:refinery"][0],
            building_value(BuildingKind::Reclaimer, 1) - building_value(BuildingKind::Reclaimer, 0)
        );
        let refinery = built(&ledgers, 0, "refinery");
        assert_eq!(refinery.built + refinery.starting, 1);
        assert_eq!(
            (refinery.taken, ledgers[0].unattributed),
            (0, 0),
            "an upgrade rebuilding the building is no one's damage"
        );
        assert!(built(&ledgers, 0, "foundry").produced > 0);
    }

    #[test]
    fn net_worth_is_sampled_from_bodies_and_the_bank() {
        let scenario = field(250, &[(0, UnitKind::Sentinel, 10, 6)], &[]);
        let (state, ledgers) = play(scenario.build().unwrap(), 2 * WORTH_PERIOD, Vec::new());
        assert_eq!(ledgers[0].worth.len(), 2);
        let expected: u64 = snapshot(&state)
            .iter()
            .filter(|body| body.owner == 0)
            .map(|body| body.value * u64::from(body.hp) / u64::from(body.max_hp.max(1)))
            .sum::<u64>()
            + u64::from(state.players()[0].scrap);
        assert_eq!(ledgers[0].worth[1], expected);
        assert_eq!(worth_at(&ledgers[0], 1_500), Some(ledgers[0].worth[0]));
        assert_eq!(worth_at(&ledgers[0], 9_000), Some(ledgers[0].worth[1]));
        assert_eq!(worth_at(&ledgers[0], 500), None);
    }

    #[test]
    fn worth_shares_average_each_pair_before_pooling() {
        let mut pairs = PairShares::new();
        pairs.insert(
            "a".into(),
            vec![[Some(600), None, None, None], [Some(400), None, None, None]],
        );
        pairs.insert("b".into(), vec![[Some(700), None, None, None]]);
        let shares = worth_shares(&pairs);
        assert_eq!(shares.len(), 1, "only ticks some pair reached");
        assert_eq!(shares[0].pairs, 2);
        assert!((shares[0].mean - 0.6).abs() < 1e-9);
        assert!(shares[0].interval.is_some());
        let ledgers = [
            SeatLedger {
                worth: vec![1, 1, 1, 1, 1, 300],
                ..SeatLedger::default()
            },
            SeatLedger {
                worth: vec![1, 1, 1, 1, 1, 100],
                ..SeatLedger::default()
            },
        ];
        let refs: Vec<Option<&SeatLedger>> = ledgers.iter().map(Some).collect();
        assert_eq!(team_shares(&refs, &[0, 1], 0).unwrap()[0], Some(750));
        assert_eq!(team_shares(&[Some(&ledgers[0]), None], &[0, 1], 0), None);
    }

    /// `scenario` built, with `edit` applied to its JSON form.
    fn staged(scenario: &Scenario, edit: impl FnOnce(&mut serde_json::Value)) -> State {
        let mut value = serde_json::to_value(scenario.build().unwrap()).unwrap();
        edit(&mut value);
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_shell_that_lands_after_its_gun_is_gone_is_still_its_guns() {
        let scenario = field(
            0,
            &[(0, UnitKind::Bombard, 8, 6), (0, UnitKind::Kestrel, 15, 6)],
            &[(1, BuildingKind::Barricade, 16, 6)],
        );
        let mut state = scenario.build().unwrap();
        state.tick(&[]);
        let bombard = unit_at(&state, 8, 6);
        let barricade = building(&state, 1, BuildingKind::Barricade);
        let mut ledger = ImpactLedger::new(&state);
        let mut orders = vec![order(
            0,
            Command::Attack {
                units: vec![bombard],
                target: AttackTarget::Building(barricade),
                queue: false,
            },
        )];
        let mut fired = false;
        for _ in 0..600 {
            let report = state.tick(&std::mem::take(&mut orders));
            ledger.observe(&state, &report);
            if report
                .events
                .iter()
                .any(|event| matches!(event, Event::ShellLaunched { .. }))
            {
                fired = true;
                break;
            }
        }
        assert!(fired, "premise: the gun fires");
        let mut value = serde_json::to_value(&state).unwrap();
        value["units"]
            .as_array_mut()
            .unwrap()
            .retain(|unit| unit["kind"] != "bombard");
        let mut state: State = serde_json::from_value(value).unwrap();
        for _ in 0..200 {
            let report = state.tick(&[]);
            ledger.observe(&state, &report);
        }
        let ledgers = ledger.finish(&state);
        assert!(unit(&ledgers, 0, "bombard").dealt.buildings > 0);
        assert_eq!(ledgers[1].unattributed, 0);
    }

    #[test]
    fn riders_count_toward_worth_and_die_to_whoever_downs_their_carrier() {
        let scenario = field(
            0,
            &[
                (0, UnitKind::Skyhook, 14, 6),
                (0, UnitKind::Sentinel, 4, 10),
            ],
            &[(1, BuildingKind::FlakTurret, 16, 6)],
        );
        let state = staged(&scenario, |value| {
            let units = value["units"].as_array_mut().unwrap();
            let rider = units
                .iter()
                .position(|unit| unit["kind"] == "sentinel")
                .unwrap();
            let rider = units.remove(rider);
            let skyhook = units
                .iter_mut()
                .find(|unit| unit["kind"] == "skyhook")
                .unwrap();
            skyhook["cargo"] = serde_json::json!([rider]);
        });
        let rider_value = u64::from(UnitKind::Sentinel.stats().cost);
        let ledger = ImpactLedger::new(&state);
        assert!(
            worth(&state, &ledger.bodies, 0)
                >= rider_value + u64::from(UnitKind::Skyhook.stats().cost),
            "the rider counts while aboard"
        );
        let (_, ledgers) = play(state, 1_200, Vec::new());
        let rider = unit(&ledgers, 0, "sentinel");
        assert_eq!((rider.deaths, rider.taken), (1, rider_value), "{rider:?}");
        assert_eq!(ledgers[0].unattributed, 0);
        assert!(built(&ledgers, 1, "flak turret").dealt.army >= rider_value);
    }

    #[test]
    fn a_crucible_is_credited_with_the_wrecks_it_smelts() {
        let scenario = field(
            0,
            &[
                (0, UnitKind::Harvester, 4, 8),
                (0, UnitKind::Lancer, 10, 6),
                (0, UnitKind::Lancer, 10, 7),
                (1, UnitKind::Sentinel, 13, 6),
            ],
            &[(0, BuildingKind::Crucible, 9, 9)],
        );
        let state = scenario.build().unwrap();
        let before = state.players()[0].scrap;
        let (state, ledgers) = play(state, 1_200, Vec::new());
        let crucible = built(&ledgers, 0, "crucible");
        assert!(crucible.passive > 0, "{crucible:?}");
        let passive: u64 = ledgers[0]
            .buildings
            .values()
            .map(|totals| totals.passive)
            .sum();
        assert_eq!(passive, u64::from(state.players()[0].scrap - before));
    }

    #[test]
    fn worth_runs_from_the_start_of_the_recording_to_its_end() {
        let scenario = field(250, &[(0, UnitKind::Sentinel, 10, 6)], &[]);
        let mut state = scenario.build().unwrap();
        for _ in 0..500 {
            state.tick(&[]);
        }
        let mut ledger = ImpactLedger::new(&state);
        for _ in 0..1_500 {
            let report = state.tick(&[]);
            ledger.observe(&state, &report);
        }
        let ledgers = ledger.finish(&state);
        let seat = &ledgers[0];
        assert_eq!((seat.start, seat.end, seat.worth.len()), (500, 2_000, 1));
        assert_eq!(worth_at(seat, 1_499), None, "before the first sample");
        assert_eq!(worth_at(seat, 1_500), Some(seat.worth[0]));
        let expected: u64 = snapshot(&state)
            .iter()
            .filter(|body| body.owner == 0)
            .map(|body| body.value * u64::from(body.hp) / u64::from(body.max_hp.max(1)))
            .sum::<u64>()
            + u64::from(state.players()[0].scrap);
        assert_eq!(seat.final_worth, expected);
        assert_eq!(
            worth_at(seat, 24_000),
            Some(expected),
            "a finished match keeps its end"
        );
        let sentinel = unit(&ledgers, 0, "sentinel");
        assert_eq!(sentinel.starting, 1);
    }

    #[test]
    fn construction_does_not_hide_fire_on_a_site() {
        let scenario = field(
            500,
            &[
                (0, UnitKind::Harvester, 10, 8),
                (1, UnitKind::Sentinel, 16, 6),
            ],
            &[],
        );
        let mut state = scenario.build().unwrap();
        state.tick(&[]);
        let harvester = unit_at(&state, 10, 8);
        let sentinel = unit_at(&state, 16, 6);
        let mut ledger = ImpactLedger::new(&state);
        let report = state.tick(&[order(
            0,
            Command::Build {
                units: vec![harvester],
                kind: BuildingKind::Turret,
                anchor: TilePos::new(13, 6),
                queue: false,
                defer: false,
            },
        )]);
        ledger.observe(&state, &report);
        let site = building(&state, 0, BuildingKind::Turret);
        let mut orders = vec![order(
            1,
            Command::Attack {
                units: vec![sentinel],
                target: AttackTarget::Building(site),
                queue: false,
            },
        )];
        let mut hits = 0_u64;
        for _ in 0..400 {
            let report = state.tick(&std::mem::take(&mut orders));
            hits += report
                .events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        Event::AttackHit { target: Some(Target::Building(id)), .. } if *id == site
                    )
                })
                .count() as u64;
            ledger.observe(&state, &report);
        }
        let ledgers = ledger.finish(&state);
        let turret = BuildingKind::Turret.tier_stats(0);
        let damage = u64::from(UnitKind::Sentinel.stats().weapons[0].damage);
        let expected =
            hits * damage * building_value(BuildingKind::Turret, 0) / u64::from(turret.max_hp);
        let dealt = unit(&ledgers, 1, "sentinel").dealt.buildings;
        assert!(hits > 0, "premise: the site is under fire");
        assert!(
            dealt.abs_diff(expected) <= 1,
            "{dealt} dealt, {expected} expected from {hits} hits"
        );
    }

    #[test]
    fn salvage_is_no_ones_damage() {
        let scenario = field(
            0,
            &[(0, UnitKind::Harvester, 9, 6)],
            &[(0, BuildingKind::Turret, 10, 6)],
        );
        let mut state = scenario.build().unwrap();
        state.tick(&[]);
        let harvester = unit_at(&state, 9, 6);
        let turret = building(&state, 0, BuildingKind::Turret);
        let (_, ledgers) = play(
            state,
            1_200,
            vec![order(
                0,
                Command::Salvage {
                    units: vec![harvester],
                    building: turret,
                    queue: false,
                },
            )],
        );
        assert!(ledgers[0].refunds > 0, "premise: it was salvaged");
        assert_eq!(built(&ledgers, 0, "turret").taken, 0);
        assert_eq!(ledgers[0].unattributed, 0);
    }

    #[test]
    fn a_crash_is_credited_to_the_aircraft_that_fell() {
        let mut units = vec![(0, UnitKind::Condor, 16, 6)];
        for y in 4..9 {
            for x in 14..19 {
                if (x, y) != (16, 6) {
                    units.push((1, UnitKind::Sentinel, x, y));
                }
            }
        }
        let scenario = field(0, &units, &[(1, BuildingKind::FlakTurret, 20, 6)]);
        let (_, ledgers) = play(scenario.build().unwrap(), 1_200, Vec::new());
        let condor = unit(&ledgers, 0, "condor");
        assert_eq!(condor.deaths, 1, "premise: it is shot down");
        assert!(condor.dealt.army > 0, "{condor:?}");
        assert_eq!(ledgers[1].unattributed, 0);
    }

    #[test]
    fn a_source_counts_once_per_victim_per_tick() {
        let scenario = field(
            0,
            &[(0, UnitKind::Sapper, 14, 6), (0, UnitKind::Sentinel, 14, 7)],
            &[(1, BuildingKind::Barricade, 16, 6)],
        );
        let state = scenario.build().unwrap();
        let mut ledger = ImpactLedger::new(&state);
        let (sapper, sentinel) = (unit_at(&state, 14, 6), unit_at(&state, 14, 7));
        let barricade = Key::Building(building(&state, 1, BuildingKind::Barricade));
        let source = |id| ledger.source(Key::Unit(id), None).unwrap();
        let mut sources = Sources::default();
        sources
            .aimed
            .insert(barricade, vec![source(sapper), source(sentinel)]);
        let at = find(&ledger.bodies, barricade).unwrap().tile;
        sources.splash.push((at, 2, source(sapper)));
        let before = ledger.bodies.clone();
        let target = *find(&before, barricade).unwrap();
        ledger.attribute(&before, &target, 100, false, &sources);
        let dealt = |id| ledger.records[&Key::Unit(id)].dealt.buildings;
        assert_eq!(
            dealt(sapper),
            dealt(sentinel),
            "the Sapper's blast is one hit"
        );
    }

    #[test]
    fn crucibles_sharing_a_last_unit_of_wreck_do_not_both_smelt_it() {
        let scenario = field(
            0,
            &[],
            &[
                (0, BuildingKind::Crucible, 10, 6),
                (0, BuildingKind::Crucible, 14, 6),
            ],
        );
        let state = staged(&scenario, |value| {
            let width = value["map"]["grid"]["width"].as_i64().unwrap();
            let cell = (8 * width + 13) as usize;
            value["map"]["grid"]["cells"][cell]["wreck"] = serde_json::json!(1);
        });
        assert_eq!(state.map().wreck_at(TilePos::new(13, 8)), 1);
        let smelting = smelters(&state);
        assert_eq!(smelting.len(), 1, "{smelting:?}");
    }

    #[test]
    fn construction_gain_follows_the_simulations_ramp() {
        let scenario = field(0, &[], &[(0, BuildingKind::Turret, 10, 6)]);
        let state = scenario.build().unwrap();
        let mut site = *snapshot(&state)
            .iter()
            .find(|body| matches!(body.building, Some((BuildingKind::Turret, ..))))
            .unwrap();
        site.building = Some((BuildingKind::Turret, 0, false));
        let ramp = site.max_hp - site.max_hp / 5;
        let at = |progress: u32| Body { progress, ..site };
        assert_eq!(
            at(0).built_gain(&at(3)),
            None,
            "the first work sets the site up"
        );
        assert_eq!(
            at(10).built_gain(&at(12)),
            Some(ramp * 12 / site.build_ticks - ramp * 10 / site.build_ticks)
        );
        let built = Body {
            building: Some((BuildingKind::Turret, 0, true)),
            ..site
        };
        assert_eq!(built.built_gain(&built), Some(0));
    }
}
