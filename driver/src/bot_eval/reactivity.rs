//! Situations a seat meets in a match, and whether it answered them.
//!
//! Like the failure detectors, these read omniscient state and events after
//! each tick, and nothing here reaches a controller. A case opens when a
//! situation first holds for a subject and closes once: answered when the
//! seat's response shows in time, moot when the situation ends first, missed
//! at its deadline. Situations a seat must see to answer count only what it
//! can see.

use super::failures::{FailureIncident, MAX_FAILURE_EXAMPLES, gap};
use crate::bot_pressure::owns_anti_air;
use chassis::grid::TilePos;
use oxide_opponent::{MissionKind, MissionStatus, Phase};
use oxide_sim::ids::Target;
use oxide_sim::stats::Domain;
use oxide_sim::{Building, BuildingKind, Event, PlayerId, State, Unit, UnitKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Ticks a seat has, once it first sees an armed enemy aircraft, to own
/// anti-air.
pub const ANTI_AIR_TICKS: u64 = 3_600;

/// Tiles from a Foundry's footprint within which an armed enemy presses it.
pub const PRESS_TILES: i32 = 8;

/// Ticks a seat has to hit an armed enemy pressing one of its Foundries.
pub const DEFENSE_TICKS: u64 = 600;

/// Ticks a seat has to hit an armed enemy pressing an ally's Foundry, which
/// it may have to walk to.
pub const RELIEF_TICKS: u64 = 1_200;

/// Ticks a seat has to hit or kill a gun shelling it.
pub const ARTILLERY_TICKS: u64 = 1_200;

/// Ticks a hostile start may go unseen before it is stale.
pub const STALE_TICKS: u64 = 3_600;

/// Ticks a seat has to see a stale hostile start again.
pub const SCOUT_TICKS: u64 = 3_600;

/// Tiles from an own Foundry within which a worker is at home, where the
/// seat's defense answers rather than the worker running.
pub const HOME_TILES: i32 = 8;

/// Ticks a worker in a seen enemy's reach has to get out of it.
pub const EVACUATE_TICKS: u64 = 240;

/// Tiles a worker must cover to count as having run.
pub const RUN_TILES: i32 = 2;

/// Health, per mille, under which a building wants repair.
pub const DAMAGED: u32 = 750;

/// Tiles around a damaged building a seen armed enemy must keep clear of
/// for it to want repair.
pub const CLEAR_TILES: i32 = 10;

/// Ticks a seat has to start repairing a damaged building with no enemy
/// near.
pub const REPAIR_TICKS: u64 = 1_200;

/// Ticks a seat has to put a new Extractor where one was destroyed.
pub const RESTORE_TICKS: u64 = 3_600;

/// How a seat answered one kind of situation. Every case that arose was
/// answered, missed or moot; cases still open when the leg ended are moot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reactions {
    /// Cases that opened.
    pub arose: u64,
    /// Cases the seat answered in time.
    pub answered: u64,
    /// Cases that reached their deadline unanswered.
    pub missed: u64,
    /// Cases whose situation ended first.
    pub moot: u64,
    /// Ticks from each answered case's start to its answer, summed.
    pub answer_ticks: u64,
    /// The first [`MAX_FAILURE_EXAMPLES`] missed cases, at the tick each
    /// opened.
    pub examples: Vec<FailureIncident>,
}

impl Reactions {
    fn open(&mut self) {
        self.arose = self.arose.saturating_add(1);
    }

    fn answer(&mut self, case: &Case, now: u64) {
        self.answered = self.answered.saturating_add(1);
        self.answer_ticks = self.answer_ticks.saturating_add(now - case.opened);
    }

    fn miss(&mut self, case: &Case) {
        self.missed = self.missed.saturating_add(1);
        if self.examples.len() < MAX_FAILURE_EXAMPLES {
            self.examples.push(FailureIncident {
                tick: case.opened,
                subject: case.subject,
                detail: case.detail.clone(),
            });
        }
    }

    fn lapse(&mut self) {
        self.moot = self.moot.saturating_add(1);
    }

    /// Adds `other`'s cases, keeping the first examples.
    pub fn merge(&mut self, other: &Reactions) {
        self.arose += other.arose;
        self.answered += other.answered;
        self.missed += other.missed;
        self.moot += other.moot;
        self.answer_ticks += other.answer_ticks;
        let room = MAX_FAILURE_EXAMPLES.saturating_sub(self.examples.len());
        self.examples
            .extend(other.examples.iter().take(room).cloned());
    }
}

/// The situations one seat met and how it answered them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatReactivity {
    /// It first saw an armed enemy aircraft; answered once it owns a
    /// dedicated anti-air unit or a built Flak Turret, within
    /// [`ANTI_AIR_TICKS`].
    pub anti_air: Reactions,
    /// It saw an enemy Airworks before any armed enemy aircraft; answered
    /// once it owns anti-air, missed if the first such aircraft comes first.
    pub airworks: Reactions,
    /// A seen armed enemy ground unit pressed one of its Foundries; answered
    /// when its units or turrets hit a presser, or its guns fire at one,
    /// within [`DEFENSE_TICKS`].
    pub ground_defense: Reactions,
    /// The same for armed enemy aircraft.
    pub air_defense: Reactions,
    /// An enemy gun fired a shell at its units or buildings that lands within
    /// [`PRESS_TILES`] of one of its buildings; answered when it hits the gun,
    /// or its own guns fire at it, within [`ARTILLERY_TICKS`] of the shell's
    /// launch, moot if the gun dies to something else.
    pub artillery: Reactions,
    /// A standing hostile start went unseen for [`STALE_TICKS`]; answered when
    /// seen again within [`SCOUT_TICKS`].
    pub scouting: Reactions,
    /// A worker away from home stood in a seen armed enemy's reach; answered
    /// when it ran out of reach within [`EVACUATE_TICKS`], missed if it died
    /// or stayed, moot if the enemy left.
    pub evacuation: Reactions,
    /// A built building other than a Barricade or Scuttle Charge fell under
    /// [`DAMAGED`] with no seen armed enemy within [`CLEAR_TILES`]; answered
    /// when its health rises within [`REPAIR_TICKS`].
    pub repair: Reactions,
    /// One of its Extractors was destroyed; answered when another stands, or
    /// is placed, on that site within [`RESTORE_TICKS`], moot if an armed
    /// enemy still stands near it then.
    pub restoration: Reactions,
    /// A seen armed enemy ground unit pressed an ally's Foundry; answered when
    /// the seat hits a presser, or fires at one, within [`RELIEF_TICKS`].
    pub relief: Reactions,
    /// `oxide-opponent` only: an attack, strike or raid entered its fight;
    /// answered when it withdrew, missed when it vanished mid-fight having
    /// lost at least half its units, moot otherwise.
    #[serde(default)]
    pub withdrawal: Option<Reactions>,
    /// `oxide-opponent` only, diagnostic: how often a newly formed attack,
    /// strike or lift went after a different player than the one before it.
    /// Raids, which go after whichever harvest line is least guarded, are
    /// left out.
    #[serde(default)]
    pub target_switches: Option<u64>,
}

impl SeatReactivity {
    /// Adds `other`'s cases.
    pub fn merge(&mut self, other: &SeatReactivity) {
        for (mine, theirs) in [
            (&mut self.anti_air, &other.anti_air),
            (&mut self.airworks, &other.airworks),
            (&mut self.ground_defense, &other.ground_defense),
            (&mut self.air_defense, &other.air_defense),
            (&mut self.artillery, &other.artillery),
            (&mut self.scouting, &other.scouting),
            (&mut self.evacuation, &other.evacuation),
            (&mut self.repair, &other.repair),
            (&mut self.restoration, &other.restoration),
            (&mut self.relief, &other.relief),
        ] {
            mine.merge(theirs);
        }
        if let Some(theirs) = &other.withdrawal {
            self.withdrawal
                .get_or_insert_with(Reactions::default)
                .merge(theirs);
        }
        if let Some(theirs) = other.target_switches {
            *self.target_switches.get_or_insert(0) += theirs;
        }
    }

    /// Each kind of situation by its report name.
    pub fn items(&self) -> [(&'static str, Option<&Reactions>); 11] {
        [
            ("anti-air", Some(&self.anti_air)),
            ("airworks", Some(&self.airworks)),
            ("ground defense", Some(&self.ground_defense)),
            ("air defense", Some(&self.air_defense)),
            ("artillery", Some(&self.artillery)),
            ("scouting", Some(&self.scouting)),
            ("evacuation", Some(&self.evacuation)),
            ("repair", Some(&self.repair)),
            ("restoration", Some(&self.restoration)),
            ("relief", Some(&self.relief)),
            ("withdrawal", self.withdrawal.as_ref()),
        ]
    }
}

/// An open case.
struct Case {
    opened: u64,
    subject: u32,
    detail: String,
}

impl Case {
    fn new(opened: u64, subject: u32, detail: impl Into<String>) -> Self {
        Self {
            opened,
            subject,
            detail: detail.into(),
        }
    }
}

/// A Foundry under pressure: the open case, and every presser seen since
/// it opened. A closed episode stays until the pressure lifts, so one
/// pressing lasts one case.
enum Press {
    Open { case: Case, pressers: BTreeSet<u32> },
    Closed,
}

/// The first armed aircraft and the first Airworks a seat saw.
#[derive(Default)]
enum Sighting {
    #[default]
    Unseen,
    Open(Case),
    Done,
}

/// A worker in reach, and where it stood then.
struct Flight {
    case: Case,
    from: TilePos,
}

/// A damaged building, and its health then.
struct Patient {
    case: Case,
    hp: u32,
}

/// An offensive mission in its fight.
struct Fight {
    case: Case,
    engaged: u32,
    last: u32,
}

#[derive(Default)]
struct SeatWatch {
    found: SeatReactivity,
    air: Sighting,
    airworks: Sighting,
    ground: BTreeMap<u32, Press>,
    aircraft: BTreeMap<u32, Press>,
    relief: BTreeMap<u32, Press>,
    shooters: BTreeMap<Target, Case>,
    last_seen: BTreeMap<u8, u64>,
    stale: BTreeMap<u8, Case>,
    workers: BTreeMap<u32, Flight>,
    /// Workers whose case was missed while they stay in reach.
    stayed: BTreeSet<u32>,
    patients: BTreeMap<u32, Patient>,
    /// Buildings whose case closed while they stay damaged.
    tended: BTreeSet<u32>,
    extractors: BTreeMap<u32, TilePos>,
    restores: BTreeMap<TilePos, Case>,
    fights: Option<BTreeMap<u64, Fight>>,
    formed: BTreeSet<u64>,
    last_target: Option<PlayerId>,
    switches: u64,
}

/// Reactivity cases for one evaluation leg. Seats without a controller are
/// not watched.
pub(super) struct ReactivityDetectors {
    seats: Vec<Option<SeatWatch>>,
    /// Each seat's first Foundry, as its start.
    starts: Option<Vec<Option<Start>>>,
}

/// A seat's start: its first Foundry's footprint.
#[derive(Clone, Copy)]
struct Start {
    anchor: TilePos,
    size: (i32, i32),
}

impl ReactivityDetectors {
    pub(super) fn new(watched: impl IntoIterator<Item = bool>) -> Self {
        Self {
            seats: watched
                .into_iter()
                .map(|watched| watched.then(SeatWatch::default))
                .collect(),
            starts: None,
        }
    }

    /// Answers cases from the seats' strikes, and opens and settles cases on
    /// shelling, deaths and destroyed Extractors.
    pub(super) fn observe_events(&mut self, state: &State, events: &[Event], now: u64) {
        for (player, target) in strikes(state, events) {
            // An eliminated seat's remnants no longer answer for it.
            let Some(Some(watch)) = self
                .seats
                .get_mut(usize::from(player.0))
                .filter(|_| state.accepts_commands(player))
            else {
                continue;
            };
            if let Target::Unit(unit) = target {
                for (presses, found) in [
                    (&mut watch.ground, &mut watch.found.ground_defense),
                    (&mut watch.aircraft, &mut watch.found.air_defense),
                    (&mut watch.relief, &mut watch.found.relief),
                ] {
                    for press in presses.values_mut() {
                        if let Press::Open { case, pressers } = press
                            && pressers.contains(&unit.0)
                        {
                            found.answer(case, now);
                            *press = Press::Closed;
                        }
                    }
                }
            }
            if let Some(case) = watch.shooters.remove(&target) {
                watch.found.artillery.answer(&case, now);
            }
        }
        for event in events {
            match *event {
                Event::ShellLaunched {
                    shooter,
                    target: Some(target),
                    player,
                    to,
                    ..
                } => {
                    let Some(victim) = owner(state, target) else {
                        continue;
                    };
                    let impact = TilePos::containing(to);
                    let at_home = state.buildings().iter().any(|building| {
                        building.player == victim
                            && building.hp > 0
                            && gap(impact, building) <= PRESS_TILES
                    });
                    if !state.hostile(player, victim) || !at_home || !state.accepts_commands(victim)
                    {
                        continue;
                    }
                    if let Some(Some(watch)) = self.seats.get_mut(usize::from(victim.0))
                        && !watch.shooters.contains_key(&shooter)
                    {
                        watch.found.artillery.open();
                        let (subject, detail) = match shooter {
                            Target::Unit(unit) => (
                                unit.0,
                                state.unit(unit).map_or("gun", |gun| gun.kind.name()),
                            ),
                            Target::Building(building) => (
                                building.0,
                                state
                                    .building(building)
                                    .map_or("gun", |gun| gun.kind.name()),
                            ),
                        };
                        watch
                            .shooters
                            .insert(shooter, Case::new(now, subject, detail));
                    }
                }
                Event::UnitDied { unit, player, .. } => {
                    for watch in self.seats.iter_mut().flatten() {
                        if let Some(_case) = watch.shooters.remove(&Target::Unit(unit)) {
                            watch.found.artillery.lapse();
                        }
                    }
                    if let Some(Some(watch)) = self.seats.get_mut(usize::from(player.0))
                        && let Some(flight) = watch.workers.remove(&unit.0)
                    {
                        watch.found.evacuation.miss(&flight.case);
                    }
                }
                Event::BuildingDestroyed {
                    building, player, ..
                } => {
                    for watch in self.seats.iter_mut().flatten() {
                        if watch.shooters.remove(&Target::Building(building)).is_some() {
                            watch.found.artillery.lapse();
                        }
                    }
                    let Some(Some(watch)) = self.seats.get_mut(usize::from(player.0)) else {
                        continue;
                    };
                    if watch.patients.remove(&building.0).is_some() {
                        watch.found.repair.lapse();
                    }
                    watch.tended.remove(&building.0);
                    if let Some(anchor) = watch.extractors.remove(&building.0)
                        && !watch.restores.contains_key(&anchor)
                        && state.accepts_commands(player)
                    {
                        watch.found.restoration.open();
                        watch.restores.insert(
                            anchor,
                            Case::new(now, building.0, format!("({}, {})", anchor.x, anchor.y)),
                        );
                    }
                }
                // An Extractor finished and destroyed between two checks
                // still opens a restoration case.
                Event::BuildingCompleted {
                    building,
                    player,
                    kind: BuildingKind::Extractor,
                } => {
                    if let Some(Some(watch)) = self.seats.get_mut(usize::from(player.0))
                        && let Some(extractor) = state.building(building)
                    {
                        watch.extractors.insert(building.0, extractor.anchor);
                    }
                }
                _ => {}
            }
        }
    }

    /// Follows an `oxide-opponent` seat's offensive missions at `now`.
    pub(super) fn check_missions(&mut self, seat: u8, now: u64, missions: &[MissionStatus]) {
        let Some(Some(watch)) = self.seats.get_mut(usize::from(seat)) else {
            return;
        };
        let fights = watch.fights.get_or_insert_with(BTreeMap::new);
        let withdrawal = watch
            .found
            .withdrawal
            .get_or_insert_with(Reactions::default);
        for mission in missions {
            let target = match mission.kind {
                MissionKind::Attack { owner, .. }
                | MissionKind::Strike { owner, .. }
                | MissionKind::Raid { owner, .. }
                | MissionKind::Lift { owner, .. } => Some(owner),
                MissionKind::Defend { .. } | MissionKind::Scout { .. } => None,
            };
            let Some(target) = target else {
                continue;
            };
            let raid = matches!(mission.kind, MissionKind::Raid { .. });
            if !raid && watch.formed.insert(mission.id) {
                if watch.last_target.is_some_and(|last| last != target) {
                    watch.switches = watch.switches.saturating_add(1);
                }
                watch.last_target = Some(target);
            }
            if matches!(mission.kind, MissionKind::Lift { .. }) {
                continue;
            }
            match (fights.get_mut(&mission.id), mission.phase) {
                (None, Phase::Engage) => {
                    withdrawal.open();
                    fights.insert(
                        mission.id,
                        Fight {
                            case: Case::new(now, mission.id as u32, kind_name(mission.kind)),
                            engaged: mission.units,
                            last: mission.units,
                        },
                    );
                }
                (Some(fight), Phase::Engage) => fight.last = mission.units,
                (Some(_), Phase::Withdraw) => {
                    if let Some(fight) = fights.remove(&mission.id) {
                        withdrawal.answer(&fight.case, now);
                    }
                }
                (Some(_), _) => {
                    fights.remove(&mission.id);
                    withdrawal.lapse();
                }
                (None, _) => {}
            }
        }
        let standing: BTreeSet<u64> = missions.iter().map(|mission| mission.id).collect();
        let ended: Vec<u64> = fights
            .keys()
            .copied()
            .filter(|id| !standing.contains(id))
            .collect();
        for id in ended {
            let Some(fight) = fights.remove(&id) else {
                continue;
            };
            if fight.last * 2 <= fight.engaged {
                withdrawal.miss(&fight.case);
            } else {
                withdrawal.lapse();
            }
        }
    }

    /// Opens, answers and closes the cases state shows at `now`.
    pub(super) fn check(&mut self, state: &State, now: u64) {
        let starts = self.starts.get_or_insert_with(|| first_foundries(state));
        for (index, watch) in self.seats.iter_mut().enumerate() {
            let Some(watch) = watch else {
                continue;
            };
            let player = PlayerId(index as u8);
            if !state.accepts_commands(player) {
                settle(watch);
                continue;
            }
            let seen: Vec<&Unit> = state
                .units()
                .iter()
                .filter(|unit| {
                    unit.hp > 0
                        && state.hostile(player, unit.player)
                        && attacker(unit.kind)
                        && state.can_see(player, unit.tile())
                })
                .collect();
            check_air(watch, state, player, &seen, now);
            let own: Vec<&Building> = state
                .buildings()
                .iter()
                .filter(|building| building.player == player && building.hp > 0)
                .collect();
            let foundries: Vec<&Building> = own
                .iter()
                .copied()
                .filter(|building| building.kind == BuildingKind::Foundry && building.built)
                .collect();
            for (domain, presses, found) in [
                (
                    Domain::Ground,
                    &mut watch.ground,
                    &mut watch.found.ground_defense,
                ),
                (
                    Domain::Air,
                    &mut watch.aircraft,
                    &mut watch.found.air_defense,
                ),
            ] {
                press(
                    presses,
                    found,
                    &foundries,
                    &seen,
                    domain,
                    DEFENSE_TICKS,
                    now,
                );
            }
            let allied: Vec<&Building> = state
                .buildings()
                .iter()
                .filter(|building| {
                    building.player != player
                        && !state.hostile(player, building.player)
                        && state.accepts_commands(building.player)
                        && building.kind == BuildingKind::Foundry
                        && building.built
                        && building.hp > 0
                })
                .collect();
            press(
                &mut watch.relief,
                &mut watch.found.relief,
                &allied,
                &seen,
                Domain::Ground,
                RELIEF_TICKS,
                now,
            );
            let shooters: Vec<Target> = watch
                .shooters
                .iter()
                .filter(|(_, case)| now - case.opened >= ARTILLERY_TICKS)
                .map(|(shooter, _)| *shooter)
                .collect();
            for shooter in shooters {
                if let Some(case) = watch.shooters.remove(&shooter) {
                    watch.found.artillery.miss(&case);
                }
            }
            scout(watch, state, player, starts, now);
            evacuate(watch, state, player, &foundries, &seen, now);
            repair(watch, &own, &seen, now);
            restore(watch, state, player, &own, now);
        }
    }

    /// Each watched seat's cases, with those still open counted moot.
    pub(super) fn finish(self) -> Vec<Option<SeatReactivity>> {
        self.seats
            .into_iter()
            .map(|watch| {
                watch.map(|mut watch| {
                    settle(&mut watch);
                    let mut found = watch.found;
                    if watch.fights.is_some() {
                        found.target_switches = Some(watch.switches);
                    }
                    found
                })
            })
            .collect()
    }
}

/// Closes every open case as moot: the seat is out of the match, or the leg
/// ended.
fn settle(watch: &mut SeatWatch) {
    let found = &mut watch.found;
    for (sighting, reactions) in [
        (&mut watch.air, &mut found.anti_air),
        (&mut watch.airworks, &mut found.airworks),
    ] {
        if matches!(sighting, Sighting::Open(_)) {
            reactions.lapse();
            *sighting = Sighting::Done;
        }
    }
    for (presses, reactions) in [
        (&mut watch.ground, &mut found.ground_defense),
        (&mut watch.aircraft, &mut found.air_defense),
        (&mut watch.relief, &mut found.relief),
    ] {
        for press in std::mem::take(presses).into_values() {
            if matches!(press, Press::Open { .. }) {
                reactions.lapse();
            }
        }
    }
    for (count, reactions) in [
        (
            std::mem::take(&mut watch.shooters).len(),
            &mut found.artillery,
        ),
        (std::mem::take(&mut watch.stale).len(), &mut found.scouting),
        (
            std::mem::take(&mut watch.workers).len(),
            &mut found.evacuation,
        ),
        (std::mem::take(&mut watch.patients).len(), &mut found.repair),
        (
            std::mem::take(&mut watch.restores).len(),
            &mut found.restoration,
        ),
    ] {
        reactions.moot += count as u64;
    }
    if let Some(fights) = &mut watch.fights
        && let Some(withdrawal) = &mut found.withdrawal
    {
        withdrawal.moot += std::mem::take(fights).len() as u64;
    }
}

/// The player each strike in `events` came from, and what it went at: unit
/// hits, turret shots and shells. A shooter destroyed on the same tick is
/// known by the event of its death.
fn strikes(state: &State, events: &[Event]) -> Vec<(PlayerId, Target)> {
    let died = |shooter: Target| {
        events.iter().find_map(|event| match (event, shooter) {
            (Event::UnitDied { unit, player, .. }, Target::Unit(dead)) if *unit == dead => {
                Some(*player)
            }
            (
                Event::BuildingDestroyed {
                    building, player, ..
                },
                Target::Building(dead),
            ) if *building == dead => Some(*player),
            _ => None,
        })
    };
    events
        .iter()
        .filter_map(|event| match *event {
            Event::AttackHit {
                attacker,
                target: Some(target),
                ..
            } => Some((
                owner(state, Target::Unit(attacker)).or_else(|| died(Target::Unit(attacker)))?,
                target,
            )),
            Event::TurretFired {
                turret,
                target: Some(target),
                ..
            } => Some((
                owner(state, Target::Building(turret))
                    .or_else(|| died(Target::Building(turret)))?,
                target,
            )),
            Event::ShellLaunched {
                player,
                target: Some(target),
                ..
            } => Some((player, target)),
            _ => None,
        })
        .collect()
}

/// Who owns `target`, while it stands.
fn owner(state: &State, target: Target) -> Option<PlayerId> {
    match target {
        Target::Unit(unit) => state.unit(unit).map(|unit| unit.player),
        Target::Building(building) => state.building(building).map(|building| building.player),
    }
}

/// Whether `kind` fights or carries others.
fn attacker(kind: UnitKind) -> bool {
    let stats = kind.stats();
    !stats.weapons.is_empty() || stats.transport_capacity > 0
}

fn armed_aircraft(unit: &Unit) -> bool {
    let stats = unit.kind.stats();
    stats.domain == Domain::Air && !stats.weapons.is_empty()
}

fn kind_name(kind: MissionKind) -> &'static str {
    match kind {
        MissionKind::Attack { .. } => "attack",
        MissionKind::Strike { .. } => "strike",
        MissionKind::Raid { .. } => "raid",
        MissionKind::Lift { .. } => "lift",
        MissionKind::Defend { .. } => "defend",
        MissionKind::Scout { .. } => "scout",
    }
}

/// Each seat's first Foundry, taken as its start.
fn first_foundries(state: &State) -> Vec<Option<Start>> {
    (0..state.players().len())
        .map(|seat| {
            state
                .buildings()
                .iter()
                .filter(|building| {
                    building.player == PlayerId(seat as u8)
                        && building.kind == BuildingKind::Foundry
                })
                .min_by_key(|building| building.id)
                .map(|building| Start {
                    anchor: building.anchor,
                    size: building.stats().size,
                })
        })
        .collect()
}

fn check_air(watch: &mut SeatWatch, state: &State, player: PlayerId, seen: &[&Unit], now: u64) {
    let defended = owns_anti_air(state, player);
    let aircraft = seen.iter().any(|unit| armed_aircraft(unit));
    if matches!(watch.airworks, Sighting::Unseen) && !matches!(watch.air, Sighting::Unseen) {
        watch.airworks = Sighting::Done;
    }
    if matches!(watch.airworks, Sighting::Unseen) && !aircraft {
        let airworks = state.buildings().iter().find(|building| {
            building.kind == BuildingKind::Airworks
                && building.hp > 0
                && state.hostile(player, building.player)
                && building.tiles().any(|tile| state.can_see(player, tile))
        });
        if let Some(airworks) = airworks {
            watch.found.airworks.open();
            watch.airworks = Sighting::Open(Case::new(now, airworks.id.0, "airworks"));
        }
    }
    if let Sighting::Open(case) = &watch.airworks {
        if defended {
            watch.found.airworks.answer(case, now);
            watch.airworks = Sighting::Done;
        } else if aircraft {
            watch.found.airworks.miss(case);
            watch.airworks = Sighting::Done;
        }
    }
    if matches!(watch.air, Sighting::Unseen)
        && let Some(first) = seen.iter().find(|unit| armed_aircraft(unit))
    {
        watch.found.anti_air.open();
        watch.air = Sighting::Open(Case::new(now, first.id.0, first.kind.name()));
    }
    if let Sighting::Open(case) = &watch.air {
        if defended {
            watch.found.anti_air.answer(case, now);
            watch.air = Sighting::Done;
        } else if now - case.opened >= ANTI_AIR_TICKS {
            watch.found.anti_air.miss(case);
            watch.air = Sighting::Done;
        }
    }
}

/// Opens a case on each of `foundries` that seen armed enemies of `domain`
/// press, and closes cases whose pressure lifted, whose Foundry fell or
/// whose deadline passed.
fn press(
    presses: &mut BTreeMap<u32, Press>,
    found: &mut Reactions,
    foundries: &[&Building],
    seen: &[&Unit],
    domain: Domain,
    window: u64,
    now: u64,
) {
    let standing: BTreeSet<u32> = foundries.iter().map(|foundry| foundry.id.0).collect();
    presses.retain(|id, press| {
        let standing = standing.contains(id);
        if !standing && let Press::Open { case, .. } = press {
            found.miss(case);
        }
        standing
    });
    for foundry in foundries {
        let id = foundry.id.0;
        let pressers: BTreeSet<u32> = seen
            .iter()
            .filter(|unit| {
                unit.kind.stats().domain == domain
                    && !unit.kind.stats().weapons.is_empty()
                    && gap(unit.tile(), foundry) <= PRESS_TILES
            })
            .map(|unit| unit.id.0)
            .collect();
        let next = match presses.remove(&id) {
            None if pressers.is_empty() => None,
            None => {
                found.open();
                Some(Press::Open {
                    case: Case::new(now, id, foundry.kind.name()),
                    pressers,
                })
            }
            Some(Press::Closed) if pressers.is_empty() => None,
            Some(Press::Closed) => Some(Press::Closed),
            Some(Press::Open { .. }) if pressers.is_empty() => {
                found.lapse();
                None
            }
            Some(Press::Open { case, .. }) if now - case.opened >= window => {
                found.miss(&case);
                Some(Press::Closed)
            }
            Some(Press::Open {
                case,
                pressers: mut all,
            }) => {
                all.extend(pressers);
                Some(Press::Open {
                    case,
                    pressers: all,
                })
            }
        };
        if let Some(next) = next {
            presses.insert(id, next);
        }
    }
}

fn scout(
    watch: &mut SeatWatch,
    state: &State,
    player: PlayerId,
    starts: &[Option<Start>],
    now: u64,
) {
    for (seat, start) in starts.iter().enumerate() {
        let owner = PlayerId(seat as u8);
        let Some(Start {
            anchor,
            size: (width, height),
        }) = *start
        else {
            continue;
        };
        if owner == player || !state.hostile(player, owner) {
            continue;
        }
        if !state.accepts_commands(owner) {
            if watch.stale.remove(&owner.0).is_some() {
                watch.found.scouting.lapse();
            }
            continue;
        }
        let visible =
            (0..height).any(|dy| (0..width).any(|dx| state.can_see(player, anchor.offset(dx, dy))));
        let last = watch.last_seen.entry(owner.0).or_insert(0);
        if visible {
            *last = now;
            if let Some(case) = watch.stale.remove(&owner.0) {
                watch.found.scouting.answer(&case, now);
            }
            continue;
        }
        match watch.stale.get(&owner.0) {
            Some(case) if now - case.opened >= SCOUT_TICKS => {
                watch.found.scouting.miss(case);
                watch.stale.remove(&owner.0);
                *last = now;
            }
            Some(_) => {}
            None if now - *last >= STALE_TICKS => {
                watch.found.scouting.open();
                watch.stale.insert(
                    owner.0,
                    Case::new(
                        now,
                        u32::from(owner.0),
                        format!("start of seat {}", owner.0),
                    ),
                );
            }
            None => {}
        }
    }
}

fn evacuate(
    watch: &mut SeatWatch,
    state: &State,
    player: PlayerId,
    foundries: &[&Building],
    seen: &[&Unit],
    now: u64,
) {
    let home = |tile: TilePos| {
        foundries
            .iter()
            .any(|foundry| gap(tile, foundry) <= HOME_TILES)
    };
    let reached = |tile: TilePos| {
        seen.iter().any(|enemy| {
            let reach = enemy
                .kind
                .stats()
                .weapons
                .iter()
                .filter(|weapon| weapon.targets.ground)
                .map(|weapon| weapon.range.ceil().to_num::<i32>())
                .max();
            reach.is_some_and(|reach| enemy.tile().chebyshev(tile) <= reach + 1)
        })
    };
    let workers: BTreeMap<u32, &Unit> = state
        .units()
        .iter()
        .filter(|unit| unit.player == player && unit.hp > 0 && unit.kind.stats().harvest.is_some())
        .map(|unit| (unit.id.0, unit))
        .collect();
    let open: Vec<u32> = watch.workers.keys().copied().collect();
    for id in open {
        let Some(flight) = watch.workers.get(&id) else {
            continue;
        };
        let Some(worker) = workers.get(&id) else {
            watch.workers.remove(&id);
            watch.found.evacuation.lapse();
            continue;
        };
        let tile = worker.tile();
        let ran = flight.from.chebyshev(tile) >= RUN_TILES;
        if home(tile) || !reached(tile) {
            if ran {
                watch.found.evacuation.answer(&flight.case, now);
            } else {
                watch.found.evacuation.lapse();
            }
            watch.workers.remove(&id);
        } else if now - flight.case.opened >= EVACUATE_TICKS {
            watch.found.evacuation.miss(&flight.case);
            watch.workers.remove(&id);
            watch.stayed.insert(id);
        }
    }
    watch.stayed.retain(|id| {
        workers
            .get(id)
            .is_some_and(|worker| !home(worker.tile()) && reached(worker.tile()))
    });
    for (id, worker) in workers {
        let tile = worker.tile();
        if watch.workers.contains_key(&id)
            || watch.stayed.contains(&id)
            || home(tile)
            || !reached(tile)
        {
            continue;
        }
        watch.found.evacuation.open();
        watch.workers.insert(
            id,
            Flight {
                case: Case::new(now, id, worker.kind.name()),
                from: tile,
            },
        );
    }
}

fn repair(watch: &mut SeatWatch, own: &[&Building], seen: &[&Unit], now: u64) {
    let clear = |building: &Building| {
        !seen.iter().any(|enemy| {
            !enemy.kind.stats().weapons.is_empty() && gap(enemy.tile(), building) <= CLEAR_TILES
        })
    };
    for building in own {
        if matches!(
            building.kind,
            BuildingKind::Barricade | BuildingKind::ScuttleCharge
        ) {
            continue;
        }
        let id = building.id.0;
        let max = building.stats().max_hp.max(1);
        let damaged = building.built && building.hp * 1_000 < max * DAMAGED;
        // One damage episode is one case: a building whose case closed opens
        // another only once it has been back above the threshold.
        if !damaged {
            watch.tended.remove(&id);
        }
        match watch.patients.get(&id) {
            Some(patient) => {
                if building.hp > patient.hp {
                    watch.found.repair.answer(&patient.case, now);
                } else if !clear(building) || !building.built {
                    watch.found.repair.lapse();
                } else if now - patient.case.opened >= REPAIR_TICKS {
                    watch.found.repair.miss(&patient.case);
                } else {
                    continue;
                }
                watch.patients.remove(&id);
                watch.tended.insert(id);
            }
            None if damaged && clear(building) && !watch.tended.contains(&id) => {
                watch.found.repair.open();
                watch.patients.insert(
                    id,
                    Patient {
                        case: Case::new(now, id, building.kind.name()),
                        hp: building.hp,
                    },
                );
            }
            None => {}
        }
    }
}

fn restore(watch: &mut SeatWatch, state: &State, player: PlayerId, own: &[&Building], now: u64) {
    watch.extractors = own
        .iter()
        .filter(|building| building.kind == BuildingKind::Extractor && building.built)
        .map(|building| (building.id.0, building.anchor))
        .collect();
    let sites: Vec<TilePos> = watch.restores.keys().copied().collect();
    for site in sites {
        let rebuilt = own
            .iter()
            .any(|building| building.kind == BuildingKind::Extractor && building.anchor == site);
        let Some(case) = watch.restores.get(&site) else {
            continue;
        };
        if rebuilt {
            watch.found.restoration.answer(case, now);
            watch.restores.remove(&site);
        } else if now - case.opened >= RESTORE_TICKS {
            let contested = state.units().iter().any(|unit| {
                unit.hp > 0
                    && state.hostile(player, unit.player)
                    && !unit.kind.stats().weapons.is_empty()
                    && unit.tile().chebyshev(site) <= PRESS_TILES
            });
            if contested {
                watch.found.restoration.lapse();
            } else {
                watch.found.restoration.miss(case);
            }
            watch.restores.remove(&site);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chassis::fx::{Fx, Vec2Fx};
    use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
    use oxide_sim::{BuildingId, Faction, Scenario, UnitId};

    /// A 24-wide field: West's start at (2, 2) and East's at (20, 2), 12 rows
    /// deep, or 16 with a third start at (2, 13), allied to West, when `trio`
    /// holds.
    fn field(
        units: &[(u8, UnitKind, i32, i32)],
        buildings: &[(u8, BuildingKind, i32, i32)],
        trio: bool,
    ) -> Scenario {
        let ground = ".".repeat(24);
        let mut map = vec![ground.clone(); if trio { 16 } else { 12 }];
        let mut top: Vec<char> = ground.chars().collect();
        top[2] = '1';
        top[20] = '2';
        map[2] = top.into_iter().collect();
        let mut factions = vec![Faction::Ferrous, Faction::Cupric];
        if trio {
            let mut low: Vec<char> = ground.chars().collect();
            low[2] = '3';
            map[13] = low.into_iter().collect();
            factions.push(Faction::Ferrous);
        }
        Scenario {
            mode: Default::default(),
            name: "reactivity".into(),
            seed: 3,
            map,
            players: factions
                .into_iter()
                .enumerate()
                .map(|(seat, faction)| PlayerSpec {
                    name: format!("seat {seat}"),
                    faction,
                    team: trio.then_some(if seat == 1 { 1 } else { 0 }),
                    scrap: 0,
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

    /// `scenario` built, with vision refreshed by one tick.
    fn built(scenario: &Scenario) -> State {
        let mut state = scenario.build().unwrap();
        state.tick(&[]);
        state
    }

    fn unit_at(state: &State, x: i32, y: i32) -> UnitId {
        state
            .units()
            .iter()
            .find(|unit| unit.tile() == TilePos::new(x, y))
            .unwrap()
            .id
    }

    fn building_of(state: &State, player: u8, kind: BuildingKind) -> BuildingId {
        state
            .buildings()
            .iter()
            .find(|building| building.player == PlayerId(player) && building.kind == kind)
            .unwrap()
            .id
    }

    fn hit(attacker: UnitId, target: Target) -> Event {
        Event::AttackHit {
            attacker,
            attacker_kind: UnitKind::Sentinel,
            weapon: 0,
            target: Some(target),
            attacker_pos: Vec2Fx::ZERO,
            target_pos: Vec2Fx::ZERO,
        }
    }

    fn found(detectors: ReactivityDetectors, seat: usize) -> SeatReactivity {
        detectors.finish()[seat].clone().unwrap()
    }

    fn counts(reactions: &Reactions) -> (u64, u64, u64, u64) {
        (
            reactions.arose,
            reactions.answered,
            reactions.missed,
            reactions.moot,
        )
    }

    fn run(detectors: &mut ReactivityDetectors, state: &State, ticks: std::ops::Range<u64>) {
        for now in ticks.step_by(12) {
            detectors.check(state, now);
        }
    }

    #[test]
    fn first_enemy_air_is_answered_by_anti_air_in_time_or_missed() {
        let stinger = (1, UnitKind::Buzzard, 4, 4);
        let bare = built(&field(&[stinger], &[], false));
        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &bare, 0..ANTI_AIR_TICKS + 12);
        let missed = found(detectors, 0);
        assert_eq!(counts(&missed.anti_air), (1, 0, 1, 0));

        let defended = built(&field(
            &[stinger, (0, UnitKind::Flakhound, 5, 6)],
            &[],
            false,
        ));
        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &bare, 0..24);
        run(&mut detectors, &defended, 24..36);
        let answered = found(detectors, 0);
        assert_eq!(counts(&answered.anti_air), (1, 1, 0, 0));
        assert_eq!(answered.anti_air.answer_ticks, 24);
    }

    #[test]
    fn an_airworks_seen_first_wants_anti_air_before_the_first_aircraft() {
        let airworks = (1, BuildingKind::Airworks, 6, 4);
        let seen = built(&field(&[], &[airworks], false));
        let defended = built(&field(
            &[(0, UnitKind::Flakhound, 5, 7)],
            &[airworks],
            false,
        ));
        let raided = built(&field(&[(1, UnitKind::Buzzard, 4, 4)], &[airworks], false));

        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &seen, 0..24);
        run(&mut detectors, &defended, 24..36);
        assert_eq!(counts(&found(detectors, 0).airworks), (1, 1, 0, 0));

        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &seen, 0..24);
        run(&mut detectors, &raided, 24..36);
        let missed = found(detectors, 0);
        assert_eq!(counts(&missed.airworks), (1, 0, 1, 0));
        assert_eq!(missed.anti_air.arose, 1, "the aircraft opens its own case");
    }

    #[test]
    fn a_pressed_foundry_is_answered_by_a_hit_lapses_when_the_enemy_leaves_or_is_missed() {
        let pressing = field(
            &[(1, UnitKind::Sentinel, 6, 3), (0, UnitKind::Sentinel, 2, 6)],
            &[],
            false,
        );
        let state = built(&pressing);
        let presser = unit_at(&state, 6, 3);
        let defender = unit_at(&state, 2, 6);

        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &state, 0..24);
        detectors.observe_events(&state, &[hit(defender, Target::Unit(presser))], 30);
        run(&mut detectors, &state, 36..48);
        let answered = found(detectors, 0);
        assert_eq!(counts(&answered.ground_defense), (1, 1, 0, 0));
        assert_eq!(answered.ground_defense.answer_ticks, 30);

        let quiet = built(&field(&[(0, UnitKind::Sentinel, 2, 6)], &[], false));
        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &state, 0..24);
        run(&mut detectors, &quiet, 24..36);
        assert_eq!(counts(&found(detectors, 0).ground_defense), (1, 0, 0, 1));

        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &state, 0..DEFENSE_TICKS + 24);
        assert_eq!(
            counts(&found(detectors, 0).ground_defense),
            (1, 0, 1, 0),
            "one pressing is one case"
        );
    }

    #[test]
    fn enemy_aircraft_over_a_foundry_open_an_air_defense_case() {
        let state = built(&field(
            &[(1, UnitKind::Buzzard, 6, 3), (0, UnitKind::Flakhound, 2, 6)],
            &[],
            false,
        ));
        let raider = unit_at(&state, 6, 3);
        let flak = unit_at(&state, 2, 6);
        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &state, 0..12);
        detectors.observe_events(&state, &[hit(flak, Target::Unit(raider))], 12);
        let answered = found(detectors, 0);
        assert_eq!(counts(&answered.air_defense), (1, 1, 0, 0));
        assert_eq!(answered.ground_defense.arose, 0);
    }

    #[test]
    fn shelling_is_answered_by_hitting_the_gun_and_lapses_if_it_dies_to_another() {
        let state = built(&field(
            &[(1, UnitKind::Bombard, 9, 3), (0, UnitKind::Sentinel, 6, 6)],
            &[],
            false,
        ));
        let gun = unit_at(&state, 9, 3);
        let sentinel = unit_at(&state, 6, 6);
        let shell = Event::ShellLaunched {
            shooter: Target::Unit(gun),
            unit_pose: None,
            target: Some(Target::Building(building_of(
                &state,
                0,
                BuildingKind::Foundry,
            ))),
            player: PlayerId(1),
            from: Vec2Fx::ZERO,
            to: Vec2Fx::ZERO,
            flight: 20,
        };

        let mut detectors = ReactivityDetectors::new([true, false]);
        detectors.observe_events(&state, std::slice::from_ref(&shell), 12);
        detectors.observe_events(&state, std::slice::from_ref(&shell), 24);
        detectors.observe_events(&state, &[hit(sentinel, Target::Unit(gun))], 100);
        assert_eq!(counts(&found(detectors, 0).artillery), (1, 1, 0, 0));

        let mut detectors = ReactivityDetectors::new([true, false]);
        detectors.observe_events(&state, std::slice::from_ref(&shell), 12);
        detectors.observe_events(
            &state,
            &[Event::UnitDied {
                unit: gun,
                kind: UnitKind::Bombard,
                player: PlayerId(1),
                pos: Vec2Fx::ZERO,
                grounded: true,
            }],
            60,
        );
        assert_eq!(counts(&found(detectors, 0).artillery), (1, 0, 0, 1));

        let mut detectors = ReactivityDetectors::new([true, false]);
        detectors.observe_events(&state, std::slice::from_ref(&shell), 0);
        run(&mut detectors, &state, 0..ARTILLERY_TICKS + 12);
        let missed = found(detectors, 0);
        assert_eq!(counts(&missed.artillery), (1, 0, 1, 0));
        assert_eq!(missed.artillery.examples[0].detail, "bombard");

        let afield = Event::ShellLaunched {
            shooter: Target::Unit(gun),
            unit_pose: None,
            target: Some(Target::Unit(sentinel)),
            player: PlayerId(1),
            from: Vec2Fx::ZERO,
            to: Vec2Fx::new(Fx::from_num(20), Fx::from_num(10)),
            flight: 20,
        };
        let mut detectors = ReactivityDetectors::new([true, false]);
        detectors.observe_events(&state, &[afield], 12);
        assert_eq!(
            found(detectors, 0).artillery.arose,
            0,
            "a shell at an army far from home is no shelling of assets"
        );
    }

    #[test]
    fn a_stale_hostile_start_is_answered_when_seen_again_or_missed() {
        let hidden = built(&field(&[], &[], false));
        let looking = built(&field(&[(0, UnitKind::Sentinel, 19, 5)], &[], false));
        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &hidden, 0..STALE_TICKS + 12);
        run(&mut detectors, &looking, STALE_TICKS + 12..STALE_TICKS + 24);
        assert_eq!(counts(&found(detectors, 0).scouting), (1, 1, 0, 0));

        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &hidden, 0..STALE_TICKS + SCOUT_TICKS + 12);
        assert_eq!(counts(&found(detectors, 0).scouting), (1, 0, 1, 0));
    }

    #[test]
    fn a_worker_in_reach_must_run_and_one_that_dies_or_stays_is_missed() {
        let enemy = (1, UnitKind::Sentinel, 14, 8);
        let reached = built(&field(
            &[(0, UnitKind::Harvester, 12, 8), enemy],
            &[],
            false,
        ));
        let ran = built(&field(&[(0, UnitKind::Harvester, 8, 8), enemy], &[], false));
        let left = built(&field(&[(0, UnitKind::Harvester, 12, 8)], &[], false));
        let worker = unit_at(&reached, 12, 8);

        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &reached, 0..12);
        run(&mut detectors, &ran, 12..24);
        assert_eq!(counts(&found(detectors, 0).evacuation), (1, 1, 0, 0));

        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &reached, 0..12);
        run(&mut detectors, &left, 12..24);
        assert_eq!(
            counts(&found(detectors, 0).evacuation),
            (1, 0, 0, 1),
            "the enemy left before the worker moved"
        );

        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &reached, 0..12);
        detectors.observe_events(
            &reached,
            &[Event::UnitDied {
                unit: worker,
                kind: UnitKind::Harvester,
                player: PlayerId(0),
                pos: Vec2Fx::ZERO,
                grounded: true,
            }],
            20,
        );
        assert_eq!(counts(&found(detectors, 0).evacuation), (1, 0, 1, 0));

        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &reached, 0..EVACUATE_TICKS + 12);
        assert_eq!(counts(&found(detectors, 0).evacuation), (1, 0, 1, 0));
    }

    /// `state` with every building of `kind` at `hp`.
    fn damaged(state: &State, kind: &str, hp: u32) -> State {
        let mut value = serde_json::to_value(state).unwrap();
        for building in value["buildings"].as_array_mut().unwrap() {
            if building["kind"] == kind {
                building["hp"] = hp.into();
            }
        }
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_damaged_building_with_no_enemy_near_wants_repair() {
        let fabricator = (0, BuildingKind::Fabricator, 5, 6);
        let state = built(&field(&[], &[fabricator], false));
        let hurt = damaged(&state, "fabricator", 100);
        let mending = damaged(&state, "fabricator", 140);

        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &hurt, 0..24);
        run(&mut detectors, &mending, 24..36);
        assert_eq!(counts(&found(detectors, 0).repair), (1, 1, 0, 0));

        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &hurt, 0..REPAIR_TICKS + 12);
        assert_eq!(counts(&found(detectors, 0).repair), (1, 0, 1, 0));

        let contested = damaged(
            &built(&field(
                &[(1, UnitKind::Sentinel, 8, 7)],
                &[fabricator],
                false,
            )),
            "fabricator",
            100,
        );
        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &contested, 0..REPAIR_TICKS + 12);
        assert_eq!(found(detectors, 0).repair.arose, 0, "an enemy stands near");

        let barricade = (0, BuildingKind::Barricade, 6, 6);
        let worn = damaged(&built(&field(&[], &[barricade], false)), "barricade", 10);
        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &worn, 0..REPAIR_TICKS + 12);
        assert_eq!(
            found(detectors, 0).repair.arose,
            0,
            "obstacles are not patients"
        );
    }

    #[test]
    fn a_destroyed_extractor_wants_another_on_its_site() {
        let extractor = (0, BuildingKind::Extractor, 6, 6);
        let standing = built(&field(&[], &[extractor], false));
        let gone = built(&field(&[], &[], false));
        let lost = Event::BuildingDestroyed {
            building: building_of(&standing, 0, BuildingKind::Extractor),
            player: PlayerId(0),
            pos: Vec2Fx::ZERO,
        };

        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &standing, 0..12);
        detectors.observe_events(&standing, std::slice::from_ref(&lost), 12);
        run(&mut detectors, &gone, 12..48);
        run(&mut detectors, &standing, 48..60);
        assert_eq!(counts(&found(detectors, 0).restoration), (1, 1, 0, 0));

        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &standing, 0..12);
        detectors.observe_events(&standing, std::slice::from_ref(&lost), 12);
        run(&mut detectors, &gone, 12..RESTORE_TICKS + 24);
        assert_eq!(counts(&found(detectors, 0).restoration), (1, 0, 1, 0));
    }

    #[test]
    fn an_ally_s_pressed_foundry_is_answered_by_the_seat_s_hit() {
        let state = built(&field(
            &[
                (1, UnitKind::Sentinel, 6, 13),
                (0, UnitKind::Sentinel, 5, 11),
            ],
            &[],
            true,
        ));
        let presser = unit_at(&state, 6, 13);
        let helper = unit_at(&state, 5, 11);
        let mut detectors = ReactivityDetectors::new([true, false, false]);
        run(&mut detectors, &state, 0..12);
        detectors.observe_events(&state, &[hit(helper, Target::Unit(presser))], 20);
        let answered = found(detectors, 0);
        assert_eq!(counts(&answered.relief), (1, 1, 0, 0));
        assert_eq!(
            answered.ground_defense.arose, 0,
            "West's own Foundry is clear"
        );
    }

    fn mission(id: u64, owner: u8, phase: Phase, units: u32) -> MissionStatus {
        MissionStatus {
            id,
            kind: MissionKind::Attack {
                owner: PlayerId(owner),
                building: BuildingKind::Foundry,
                anchor: TilePos::new(20, 2),
            },
            phase,
            since: 0,
            timeout: 600,
            units,
            goal: TilePos::new(18, 2),
        }
    }

    #[test]
    fn a_losing_fight_should_withdraw_and_new_targets_count_switches() {
        let mut detectors = ReactivityDetectors::new([true, false, false]);
        detectors.check_missions(0, 12, &[mission(0, 1, Phase::Travel, 8)]);
        detectors.check_missions(0, 24, &[mission(0, 1, Phase::Engage, 8)]);
        detectors.check_missions(0, 36, &[mission(0, 1, Phase::Withdraw, 5)]);
        detectors.check_missions(0, 48, &[mission(1, 2, Phase::Engage, 8)]);
        detectors.check_missions(0, 60, &[mission(1, 2, Phase::Engage, 3)]);
        detectors.check_missions(0, 72, &[mission(2, 2, Phase::Engage, 6)]);
        detectors.check_missions(0, 84, &[mission(2, 2, Phase::Recover, 6)]);
        let raid = MissionStatus {
            kind: MissionKind::Raid {
                owner: PlayerId(1),
                building: BuildingKind::Extractor,
                anchor: TilePos::new(14, 8),
            },
            ..mission(3, 1, Phase::Travel, 2)
        };
        detectors.check_missions(0, 96, &[raid]);
        let found = found(detectors, 0);
        let withdrawal = found.withdrawal.unwrap();
        assert_eq!(counts(&withdrawal), (3, 1, 1, 1));
        assert_eq!(withdrawal.examples[0].detail, "attack");
        assert_eq!(found.target_switches, Some(1));
    }

    #[test]
    fn seats_without_missions_report_no_withdrawal() {
        let detectors = ReactivityDetectors::new([true, false]);
        let found = found(detectors, 0);
        assert_eq!(found.withdrawal, None);
        assert_eq!(found.target_switches, None);
    }

    #[test]
    fn cases_open_when_a_seat_falls_or_the_leg_ends_are_moot() {
        let state = built(&field(&[(1, UnitKind::Sentinel, 6, 3)], &[], false));
        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &state, 0..12);
        assert_eq!(counts(&found(detectors, 0).ground_defense), (1, 0, 0, 1));
    }

    /// `state` without `player`'s buildings of `kind`.
    fn without(state: &State, player: u8, kind: &str) -> State {
        let mut value = serde_json::to_value(state).unwrap();
        value["buildings"]
            .as_array_mut()
            .unwrap()
            .retain(|building| !(building["player"] == player && building["kind"] == kind));
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_hit_from_a_shooter_killed_that_tick_still_answers() {
        // The defender comes last, so the field without it keeps every other
        // unit's id.
        let state = built(&field(
            &[(1, UnitKind::Sentinel, 6, 3), (0, UnitKind::Sentinel, 2, 6)],
            &[],
            false,
        ));
        let presser = unit_at(&state, 6, 3);
        let defender = unit_at(&state, 2, 6);
        let after = built(&field(&[(1, UnitKind::Sentinel, 6, 3)], &[], false));
        let died = Event::UnitDied {
            unit: defender,
            kind: UnitKind::Sentinel,
            player: PlayerId(0),
            pos: Vec2Fx::ZERO,
            grounded: true,
        };
        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &state, 0..24);
        detectors.observe_events(&after, &[hit(defender, Target::Unit(presser)), died], 30);
        assert_eq!(counts(&found(detectors, 0).ground_defense), (1, 1, 0, 0));
    }

    #[test]
    fn an_eliminated_seat_opens_and_answers_no_cases() {
        let state = built(&field(
            &[(1, UnitKind::Bombard, 9, 3), (0, UnitKind::Sentinel, 6, 6)],
            &[(0, BuildingKind::Fabricator, 3, 8)],
            false,
        ));
        let gun = unit_at(&state, 9, 3);
        let sentinel = unit_at(&state, 6, 6);
        let shell = Event::ShellLaunched {
            shooter: Target::Unit(gun),
            unit_pose: None,
            target: Some(Target::Building(building_of(
                &state,
                0,
                BuildingKind::Fabricator,
            ))),
            player: PlayerId(1),
            from: Vec2Fx::ZERO,
            to: TilePos::new(4, 8).center(),
            flight: 20,
        };
        let fallen = without(&state, 0, "foundry");
        assert!(!fallen.accepts_commands(PlayerId(0)), "premise");

        let mut detectors = ReactivityDetectors::new([true, false]);
        detectors.observe_events(&fallen, std::slice::from_ref(&shell), 12);
        assert_eq!(
            found(detectors, 0).artillery.arose,
            0,
            "shelling a fallen seat is no case"
        );

        let mut detectors = ReactivityDetectors::new([true, false]);
        detectors.observe_events(&state, std::slice::from_ref(&shell), 12);
        detectors.observe_events(&fallen, &[hit(sentinel, Target::Unit(gun))], 24);
        assert_eq!(
            counts(&found(detectors, 0).artillery),
            (1, 0, 0, 1),
            "its remnants answer nothing"
        );
    }

    #[test]
    fn a_building_still_damaged_after_its_case_closes_opens_no_second_case() {
        let fabricator = (0, BuildingKind::Fabricator, 5, 6);
        let state = built(&field(&[], &[fabricator], false));
        let hurt = damaged(&state, "fabricator", 100);
        let mending = damaged(&state, "fabricator", 140);
        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &hurt, 0..24);
        run(&mut detectors, &mending, 24..240);
        assert_eq!(counts(&found(detectors, 0).repair), (1, 1, 0, 0));

        let mut detectors = ReactivityDetectors::new([true, false]);
        run(&mut detectors, &hurt, 0..24);
        run(&mut detectors, &mending, 24..36);
        run(&mut detectors, &state, 36..48);
        run(&mut detectors, &hurt, 48..60);
        assert_eq!(
            found(detectors, 0).repair.arose,
            2,
            "damaged again once repaired"
        );
    }

    #[test]
    fn merged_reactions_sum_and_keep_the_first_examples() {
        let mut total = SeatReactivity::default();
        let one = SeatReactivity {
            repair: Reactions {
                arose: 2,
                missed: 2,
                examples: (0..MAX_FAILURE_EXAMPLES as u64)
                    .map(|tick| FailureIncident {
                        tick,
                        subject: 1,
                        detail: "fabricator".into(),
                    })
                    .collect(),
                ..Reactions::default()
            },
            withdrawal: Some(Reactions {
                arose: 1,
                answered: 1,
                answer_ticks: 50,
                ..Reactions::default()
            }),
            target_switches: Some(2),
            ..SeatReactivity::default()
        };
        total.merge(&one);
        total.merge(&one);
        assert_eq!(counts(&total.repair), (4, 0, 4, 0));
        assert_eq!(total.repair.examples.len(), MAX_FAILURE_EXAMPLES);
        assert_eq!(total.withdrawal.unwrap().answer_ticks, 100);
        assert_eq!(total.target_switches, Some(4));
    }
}
