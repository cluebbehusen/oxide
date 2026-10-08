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
                MissionKind::Defend { .. }
                | MissionKind::Scout { .. }
                | MissionKind::Clear { .. } => None,
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
                            case: Case::new(
                                now,
                                u32::try_from(mission.id).expect("mission ids fit in u32"),
                                kind_name(mission.kind),
                            ),
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
            let player = PlayerId::from_index(index);
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
        MissionKind::Clear { .. } => "clear",
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
                    building.player == PlayerId::from_index(seat)
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
        let owner = PlayerId::from_index(seat);
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
mod tests;
