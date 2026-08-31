//! Action-driven sprite animation state derived from simulation facts.
//!
//! This module owns no gameplay state. Its clocks advance in completed
//! simulation ticks, while [`AnimationController`] remembers only recent
//! output events that are not recoverable from the current world snapshot.
//! Clearing the controller is therefore always safe when seeking or bulk
//! advancing a replay.

use std::collections::HashMap;

use chassis::fx::Vec2Fx;
use oxide_sim::stats::MAX_WEAPONS;
use oxide_sim::{
    Building, BuildingId, BuildingKind, Event, Order, State, TickReport, Unit, UnitId, UnitKind,
    UnitRepairSource,
};

const GROUND_MOVE_PERIOD: u64 = 6;
const HARVEST_PERIOD: u64 = 20;
const CONSTRUCTION_PERIOD: u64 = 8;
const FOUNDRY_PRODUCTION_PERIOD: u64 = 40;
const FABRICATOR_PRODUCTION_PERIOD: u64 = 12;
const ARRAY_SWEEP_PERIOD: u64 = 32;
const RECLAIMER_PERIOD: u64 = 12;
const BUZZARD_ROTOR_PERIOD: u64 = 6;
const WISP_ROTOR_PERIOD: u64 = 4;
const REPAIR_PULSE_TICKS: f32 = 6.0;
pub(crate) const FLAKHOUND_REPORT_TICKS: f32 = 2.0;
pub(crate) const FLAK_TURRET_REPORT_TICKS: f32 = 3.0;

/// A render instant on the simulation timeline.
///
/// `completed_ticks` is [`State::current_tick`]. `tick_fraction` is the
/// shell accumulator's stable interpolation fraction. Repeating an instant
/// repeats every pose, which makes pausing exact; playback speed merely
/// changes how quickly callers move through these instants.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AnimationClock {
    completed_ticks: u64,
    tick_fraction: f32,
}

impl AnimationClock {
    /// Captures the current simulation instant.
    pub(crate) fn from_state(state: &State, tick_fraction: f32) -> Self {
        Self::new(state.current_tick(), tick_fraction)
    }

    /// Builds a clock from explicit timeline coordinates.
    pub(crate) fn new(completed_ticks: u64, tick_fraction: f32) -> Self {
        let tick_fraction = if tick_fraction.is_finite() {
            tick_fraction.clamp(0.0, 1.0)
        } else {
            0.0
        };
        Self {
            completed_ticks,
            tick_fraction,
        }
    }

    fn elapsed_since(self, completed_tick: u64) -> Option<f32> {
        let whole = self.completed_ticks.checked_sub(completed_tick)?;
        Some(whole as f32 + self.tick_fraction)
    }

    fn cycle(self, id: u32, period: u64, reduced_motion: bool) -> f32 {
        if reduced_motion || period == 0 {
            return 0.0;
        }
        let offset = u64::from(id).wrapping_mul(7) % period;
        let whole = (self.completed_ticks % period + offset) % period;
        ((whole as f32 + self.tick_fraction) / period as f32).fract()
    }
}

/// Presentation accessibility choices that affect authored motion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AnimationOptions {
    /// Hold repeating machinery on its representative powered frame.
    pub(crate) reduced_motion: bool,
}

/// A unit's actual locomotion state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum LocomotionState {
    /// No position change occurred across the last simulation tick.
    Rest,
    /// The unit changed position. `cycle` is a normalized authored-frame
    /// phase, stable for the same entity and simulation instant.
    Moving { cycle: f32 },
}

/// Work performed by a non-combat unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum UnitWorkState {
    /// No mechanism is presently doing work.
    Idle,
    /// A Harvester is physically extracting an adjacent node or the wreck
    /// under its chassis.
    Harvesting { target: Vec2Fx, cycle: f32 },
    /// A Harvester is adjacent to and actively raising this paid site.
    Constructing {
        site: BuildingId,
        target: Vec2Fx,
        cycle: f32,
    },
    /// A field welder is actively repairing a wounded unit or building.
    Repairing { target: Vec2Fx, cycle: f32 },
    /// A Harvester is actively stripping a friendly structure for scrap.
    Salvaging { target: Vec2Fx, cycle: f32 },
}

/// The visible fill of a Harvester's internal cargo bay.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CargoState {
    /// Scrap currently aboard.
    pub(crate) amount: u32,
    /// Maximum scrap the bay can hold.
    pub(crate) capacity: u32,
    /// `amount / capacity`, clamped to `0..=1` for direct frame selection.
    pub(crate) fill: f32,
}

/// The mechanism that must remain powered while a unit is stationary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum PropulsionState {
    /// No continuously animated propulsion mechanism.
    None,
    /// Visible lift rotors. These remain powered while an aircraft is idle;
    /// reduced motion holds one representative rotor frame.
    LiftRotors { cycle: f32 },
}

/// A weapon's readiness after the simulation has resolved a tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum WeaponCycle {
    /// This sprite has no weapon in the corresponding slot.
    Unavailable,
    /// The weapon can fire immediately. This is also the initial state, so
    /// the first shot never receives an invented presentation wind-up.
    Ready,
    /// The previous shot put the weapon on cooldown. `progress` moves from
    /// empty to prepared and may drive charging cells, shell loading, or a
    /// mechanical reset according to the sprite's own mechanism.
    Preparing { progress: f32 },
}

/// A recent logical attack report.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum AttackPhase {
    /// The one decisive frame that corresponds to a damage or launch event.
    Report { weapon: usize, progress: f32 },
    /// The mechanism settling after the report.
    Recover { weapon: usize, progress: f32 },
}

/// All independent animation channels for one unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct UnitAnimationState {
    /// Position-driven tread, wheel, leg, or internal propulsion phase.
    pub(crate) locomotion: LocomotionState,
    /// Economy mechanism state.
    pub(crate) work: UnitWorkState,
    /// Harvester cargo, absent for every other kind.
    pub(crate) cargo: Option<CargoState>,
    /// Event-driven attack report and recovery.
    pub(crate) attack: Option<AttackPhase>,
    /// Cooldown-driven preparation, one entry per simulation weapon slot.
    pub(crate) weapons: [WeaponCycle; MAX_WEAPONS],
    /// Mechanisms that must run independently of locomotion.
    pub(crate) propulsion: PropulsionState,
}

/// Facts about a unit that can be captured without mutating the simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UnitAnimationFacts {
    id: UnitId,
    kind: UnitKind,
    moved: bool,
    work: UnitWorkFact,
    carrying: u32,
    cooldowns: [u32; MAX_WEAPONS],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnitWorkFact {
    Idle,
    Harvesting(Vec2Fx),
    Constructing(BuildingId, Vec2Fx),
    Repairing(Vec2Fx),
    Salvaging(Vec2Fx),
}

impl UnitAnimationFacts {
    /// Reads the unit's visible mechanisms from the post-tick world.
    pub(crate) fn capture(state: &State, unit: &Unit, moved: bool) -> Self {
        let work = if let Some(target) = active_harvesting(state, unit) {
            UnitWorkFact::Harvesting(target)
        } else if let Some((site, target)) = active_unit_construction(state, unit) {
            UnitWorkFact::Constructing(site, target)
        } else if let Some(target) = active_unit_repair(state, unit) {
            UnitWorkFact::Repairing(target)
        } else if let Some(target) = active_unit_salvage(state, unit) {
            UnitWorkFact::Salvaging(target)
        } else {
            UnitWorkFact::Idle
        };
        Self {
            id: unit.id,
            kind: unit.kind,
            moved,
            work,
            carrying: unit.carrying,
            cooldowns: unit.cooldowns,
        }
    }
}

/// A site under construction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ConstructionState {
    /// Normalized build completion.
    pub(crate) progress: f32,
    /// Whether an assigned Harvester is adjacent and advancing the site.
    pub(crate) active: bool,
    /// Authored machinery phase. Inactive and reduced-motion sites hold it.
    pub(crate) machinery_cycle: f32,
}

/// The mutually exclusive primary activity of a completed building.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum BuildingActivity {
    /// No production or work is occurring.
    Idle,
    /// A Foundry or Fabricator is advancing the front of its queue.
    Production {
        /// The unit currently under construction.
        unit: UnitKind,
        /// Normalized progress through that unit's build time.
        progress: f32,
        /// Transfer, gantry, or fabrication machinery phase.
        cycle: f32,
    },
    /// A completed Array's continuous full-bearing scan.
    ArraySweep { cycle: f32 },
    /// A completed Reclaimer's continuous grind.
    Reclaiming { cycle: f32 },
    /// A Repair Bay actually delivered at least one accepted repair pulse.
    RepairPulse { progress: f32 },
}

/// All independent animation channels for one building.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BuildingAnimationState {
    /// Site progress while incomplete; absent once built.
    pub(crate) construction: Option<ConstructionState>,
    /// Production, continuous machinery, or accepted repair work.
    pub(crate) activity: BuildingActivity,
    /// Event-driven firing report and recovery for defenses.
    pub(crate) attack: Option<AttackPhase>,
    /// Primary defense readiness, absent for unarmed buildings.
    pub(crate) weapon: Option<WeaponCycle>,
}

/// Facts about a building that can be captured without mutating the sim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BuildingAnimationFacts {
    id: BuildingId,
    kind: BuildingKind,
    tier: u8,
    built: bool,
    progress: u32,
    construction_total: Option<u32>,
    construction_active: bool,
    production: Option<(UnitKind, u32, u32)>,
    cooldown: u32,
}

impl BuildingAnimationFacts {
    /// Reads construction, queue, and cooldown facts from the post-tick
    /// world. Render visibility remains the caller's responsibility.
    pub(crate) fn capture(state: &State, building: &Building) -> Self {
        let production = building
            .built
            .then_some(())
            .and_then(|()| building.queue.front().copied())
            .filter(|kind| building.progress < kind.stats().train_ticks)
            .map(|kind| (kind, building.progress, kind.stats().train_ticks));
        // The active tier's clock: a committed upgrade rebuilds on the
        // NEW tier's labor budget, and a base denominator would show the
        // scaffold complete early.
        let construction_total = building.stats().construction.map(|stats| stats.build_ticks);
        Self {
            id: building.id,
            kind: building.kind,
            tier: building.tier,
            built: building.built,
            progress: building.progress,
            construction_total,
            construction_active: !building.built && active_site_construction(state, building),
            production,
            cooldown: building.cooldown,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct AttackStamp {
    completed_tick: u64,
    weapon: usize,
}

/// Presentation-only memory for output events that have no standing state.
#[derive(Debug, Default)]
pub(crate) struct AnimationController {
    unit_attacks: HashMap<UnitId, [Option<u64>; MAX_WEAPONS]>,
    building_attacks: HashMap<BuildingId, AttackStamp>,
    repair_pulses: HashMap<BuildingId, u64>,
}

impl AnimationController {
    /// Records the transient action events produced by one completed tick.
    pub(crate) fn observe(&mut self, report: &TickReport) {
        let completed_tick = report.tick.saturating_add(1);
        self.observe_events(completed_tick, &report.events);
    }

    /// Records events when playback already supplies the post-tick cursor.
    pub(crate) fn observe_events(&mut self, completed_tick: u64, events: &[Event]) {
        for event in events {
            match event {
                Event::AttackHit {
                    attacker, weapon, ..
                } => self.note_unit_attack(*attacker, *weapon, completed_tick),
                Event::ShellLaunched { shooter, .. } => match shooter {
                    oxide_sim::Target::Unit(unit) => {
                        self.note_unit_attack(*unit, 0, completed_tick);
                    }
                    oxide_sim::Target::Building(building) => {
                        self.building_attacks.insert(
                            *building,
                            AttackStamp {
                                completed_tick,
                                weapon: 0,
                            },
                        );
                    }
                },
                Event::TurretFired { turret, .. } => {
                    self.building_attacks.insert(
                        *turret,
                        AttackStamp {
                            completed_tick,
                            weapon: 0,
                        },
                    );
                }
                Event::UnitRepaired {
                    source: UnitRepairSource::RepairBay { building },
                    ..
                } => {
                    self.repair_pulses.insert(*building, completed_tick);
                }
                _ => {}
            }
        }
    }

    /// Drops timeline-local reports after a seek or bulk jump.
    pub(crate) fn reset_transients(&mut self) {
        self.unit_attacks.clear();
        self.building_attacks.clear();
        self.repair_pulses.clear();
    }

    /// Forgets ids no longer present in the current world.
    pub(crate) fn retain_live(&mut self, state: &State) {
        self.unit_attacks.retain(|id, _| state.unit(*id).is_some());
        self.building_attacks
            .retain(|id, _| state.building(*id).is_some());
        self.repair_pulses
            .retain(|id, _| state.building(*id).is_some());
    }

    /// Resolves authored animation channels for one unit.
    pub(crate) fn unit_state(
        &self,
        facts: UnitAnimationFacts,
        clock: AnimationClock,
        options: AnimationOptions,
    ) -> UnitAnimationState {
        let locomotion = if facts.moved {
            LocomotionState::Moving {
                cycle: clock.cycle(
                    facts.id.0,
                    unit_move_period(facts.kind),
                    options.reduced_motion,
                ),
            }
        } else {
            LocomotionState::Rest
        };
        let work = match facts.work {
            UnitWorkFact::Idle => UnitWorkState::Idle,
            UnitWorkFact::Harvesting(target) => UnitWorkState::Harvesting {
                target,
                cycle: clock.cycle(facts.id.0, HARVEST_PERIOD, options.reduced_motion),
            },
            UnitWorkFact::Constructing(site, target) => UnitWorkState::Constructing {
                site,
                target,
                cycle: clock.cycle(facts.id.0, CONSTRUCTION_PERIOD, options.reduced_motion),
            },
            UnitWorkFact::Repairing(target) => UnitWorkState::Repairing {
                target,
                cycle: clock.cycle(facts.id.0, CONSTRUCTION_PERIOD, options.reduced_motion),
            },
            UnitWorkFact::Salvaging(target) => UnitWorkState::Salvaging {
                target,
                cycle: clock.cycle(facts.id.0, HARVEST_PERIOD, options.reduced_motion),
            },
        };
        let cargo = facts.kind.stats().harvest.map(|harvest| CargoState {
            amount: facts.carrying,
            capacity: harvest.capacity,
            fill: ratio(facts.carrying, harvest.capacity),
        });
        let weapons = std::array::from_fn(|index| {
            facts
                .kind
                .stats()
                .weapons
                .get(index)
                .map_or(WeaponCycle::Unavailable, |weapon| {
                    weapon_cycle(
                        facts.cooldowns[index],
                        weapon.cooldown_ticks,
                        clock.tick_fraction,
                    )
                })
        });
        let propulsion = match facts.kind {
            UnitKind::Buzzard => PropulsionState::LiftRotors {
                cycle: clock.cycle(facts.id.0, BUZZARD_ROTOR_PERIOD, options.reduced_motion),
            },
            UnitKind::Wisp => PropulsionState::LiftRotors {
                cycle: clock.cycle(facts.id.0, WISP_ROTOR_PERIOD, options.reduced_motion),
            },
            _ => PropulsionState::None,
        };
        UnitAnimationState {
            locomotion,
            work,
            cargo,
            attack: self.unit_attack(facts.id, facts.kind, clock),
            weapons,
            propulsion,
        }
    }

    /// Resolves authored animation channels for one building.
    pub(crate) fn building_state(
        &self,
        facts: BuildingAnimationFacts,
        clock: AnimationClock,
        options: AnimationOptions,
    ) -> BuildingAnimationState {
        let construction = (!facts.built).then(|| {
            let total = facts.construction_total.unwrap_or(1);
            ConstructionState {
                progress: ratio(facts.progress, total),
                active: facts.construction_active,
                machinery_cycle: clock.cycle(
                    facts.id.0,
                    CONSTRUCTION_PERIOD,
                    options.reduced_motion || !facts.construction_active,
                ),
            }
        });
        let activity = if !facts.built {
            BuildingActivity::Idle
        } else {
            match facts.kind {
                BuildingKind::Foundry | BuildingKind::Fabricator => {
                    let period = if facts.kind == BuildingKind::Foundry {
                        FOUNDRY_PRODUCTION_PERIOD
                    } else {
                        FABRICATOR_PRODUCTION_PERIOD
                    };
                    facts
                        .production
                        .map_or(BuildingActivity::Idle, |(unit, progress, total)| {
                            BuildingActivity::Production {
                                unit,
                                progress: ratio(progress, total),
                                cycle: clock.cycle(facts.id.0, period, options.reduced_motion),
                            }
                        })
                }
                BuildingKind::Array => BuildingActivity::ArraySweep {
                    cycle: clock.cycle(facts.id.0, ARRAY_SWEEP_PERIOD, options.reduced_motion),
                },
                BuildingKind::Reclaimer => BuildingActivity::Reclaiming {
                    cycle: clock.cycle(facts.id.0, RECLAIMER_PERIOD, options.reduced_motion),
                },
                BuildingKind::RepairBay => self.repair_activity(facts.id, clock),
                _ => BuildingActivity::Idle,
            }
        };
        let weapon = facts
            .kind
            .tier_stats(facts.tier)
            .weapons
            .first()
            .map(|weapon| weapon_cycle(facts.cooldown, weapon.cooldown_ticks, clock.tick_fraction));
        BuildingAnimationState {
            construction,
            activity,
            attack: self.building_attack(facts.id, facts.kind, clock),
            weapon,
        }
    }

    fn note_unit_attack(&mut self, unit: UnitId, weapon: usize, completed_tick: u64) {
        if weapon >= MAX_WEAPONS {
            return;
        }
        self.unit_attacks.entry(unit).or_insert([None; MAX_WEAPONS])[weapon] = Some(completed_tick);
    }

    fn unit_attack(
        &self,
        unit: UnitId,
        kind: UnitKind,
        clock: AnimationClock,
    ) -> Option<AttackPhase> {
        let stamps = self.unit_attacks.get(&unit)?;
        stamps
            .iter()
            .enumerate()
            .filter_map(|(weapon, stamp)| {
                let elapsed = clock.elapsed_since((*stamp)?)?;
                attack_phase(elapsed, weapon, unit_attack_timing(kind))
                    .map(|phase| (elapsed, weapon, phase))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)))
            .map(|(_, _, phase)| phase)
    }

    fn building_attack(
        &self,
        building: BuildingId,
        kind: BuildingKind,
        clock: AnimationClock,
    ) -> Option<AttackPhase> {
        let stamp = self.building_attacks.get(&building)?;
        let elapsed = clock.elapsed_since(stamp.completed_tick)?;
        attack_phase(elapsed, stamp.weapon, building_attack_timing(kind))
    }

    fn repair_activity(&self, building: BuildingId, clock: AnimationClock) -> BuildingActivity {
        self.repair_pulses
            .get(&building)
            .and_then(|stamp| clock.elapsed_since(*stamp))
            .filter(|elapsed| *elapsed < REPAIR_PULSE_TICKS)
            .map_or(BuildingActivity::Idle, |elapsed| {
                BuildingActivity::RepairPulse {
                    progress: (elapsed / REPAIR_PULSE_TICKS).clamp(0.0, 1.0),
                }
            })
    }
}

#[derive(Debug, Clone, Copy)]
struct AttackTiming {
    report_ticks: f32,
    recover_ticks: f32,
}

fn unit_attack_timing(kind: UnitKind) -> AttackTiming {
    match kind {
        UnitKind::Scuttler => AttackTiming {
            report_ticks: 1.0,
            recover_ticks: 2.0,
        },
        UnitKind::Lancer => AttackTiming {
            report_ticks: 2.0,
            recover_ticks: 4.0,
        },
        UnitKind::Bombard => AttackTiming {
            report_ticks: 3.0,
            recover_ticks: 5.0,
        },
        UnitKind::Flakhound | UnitKind::Stinger => AttackTiming {
            report_ticks: FLAKHOUND_REPORT_TICKS,
            recover_ticks: 3.0,
        },
        UnitKind::Buzzard => AttackTiming {
            report_ticks: 2.0,
            recover_ticks: 4.0,
        },
        UnitKind::Warden => AttackTiming {
            report_ticks: 2.0,
            recover_ticks: 4.0,
        },
        UnitKind::Shrike | UnitKind::Sylph => AttackTiming {
            report_ticks: 2.0,
            recover_ticks: 3.0,
        },
        UnitKind::Condor | UnitKind::Moth => AttackTiming {
            report_ticks: 3.0,
            recover_ticks: 5.0,
        },
        UnitKind::Breaker | UnitKind::Avalanche => AttackTiming {
            report_ticks: 3.0,
            recover_ticks: 6.0,
        },
        UnitKind::Tender
        | UnitKind::Excavator
        | UnitKind::Kestrel
        | UnitKind::Gnat
        | UnitKind::Skyhook
        | UnitKind::Sapper => AttackTiming {
            report_ticks: 1.0,
            recover_ticks: 1.0,
        },
        UnitKind::Darter | UnitKind::Talon | UnitKind::Wisp => AttackTiming {
            report_ticks: 2.0,
            recover_ticks: 3.0,
        },
        UnitKind::Sentinel => AttackTiming {
            report_ticks: 2.0,
            recover_ticks: 3.0,
        },
        UnitKind::Harvester => AttackTiming {
            report_ticks: 1.0,
            recover_ticks: 1.0,
        },
    }
}

fn building_attack_timing(kind: BuildingKind) -> AttackTiming {
    match kind {
        BuildingKind::FlakTurret => AttackTiming {
            report_ticks: FLAK_TURRET_REPORT_TICKS,
            recover_ticks: 3.0,
        },
        BuildingKind::Bastion => AttackTiming {
            report_ticks: 1.0,
            recover_ticks: 3.0,
        },
        _ => AttackTiming {
            report_ticks: 2.0,
            recover_ticks: 3.0,
        },
    }
}

fn attack_phase(elapsed: f32, weapon: usize, timing: AttackTiming) -> Option<AttackPhase> {
    if elapsed < timing.report_ticks {
        return Some(AttackPhase::Report {
            weapon,
            progress: (elapsed / timing.report_ticks).clamp(0.0, 1.0),
        });
    }
    let recovery = elapsed - timing.report_ticks;
    (recovery < timing.recover_ticks).then(|| AttackPhase::Recover {
        weapon,
        progress: (recovery / timing.recover_ticks).clamp(0.0, 1.0),
    })
}

fn weapon_cycle(remaining: u32, total: u32, tick_fraction: f32) -> WeaponCycle {
    if remaining == 0 || total == 0 {
        return WeaponCycle::Ready;
    }
    let elapsed = total.saturating_sub(remaining) as f32 + tick_fraction;
    WeaponCycle::Preparing {
        progress: (elapsed / total as f32).clamp(0.0, 1.0),
    }
}

fn unit_move_period(kind: UnitKind) -> u64 {
    match kind {
        UnitKind::Buzzard => BUZZARD_ROTOR_PERIOD,
        UnitKind::Wisp => WISP_ROTOR_PERIOD,
        _ => GROUND_MOVE_PERIOD,
    }
}

fn ratio(value: u32, total: u32) -> f32 {
    if total == 0 {
        0.0
    } else {
        (value as f32 / total as f32).clamp(0.0, 1.0)
    }
}

fn active_harvesting(state: &State, unit: &Unit) -> Option<Vec2Fx> {
    let harvest = unit.kind.stats().harvest?;
    let Order::Harvest {
        node,
        retiring: false,
        ..
    } = unit.order
    else {
        return None;
    };
    if unit.carrying >= harvest.capacity {
        return None;
    }
    let tile = unit.tile();
    let active = if state.map().scrap_at(node) > 0 {
        tile != node && tile.chebyshev(node) <= 1
    } else {
        state.map().wreck_at(node) > 0 && tile == node
    };
    active.then_some(node.center())
}

fn active_unit_construction(state: &State, unit: &Unit) -> Option<(BuildingId, Vec2Fx)> {
    let Order::Build { site } = unit.order else {
        return None;
    };
    state.building(site).and_then(|building| {
        (!building.built
            && building.progress > 0
            && building.player == unit.player
            && unit.kind == UnitKind::Harvester
            && tile_adjacent_to_building(unit.tile(), building))
        .then_some((site, building.center()))
    })
}

fn active_unit_repair(state: &State, unit: &Unit) -> Option<Vec2Fx> {
    if !unit.kind.stats().welder || unit.progress == 0 || unit.path.is_some() {
        return None;
    }
    match unit.order {
        Order::Repair { building } => state.building(building).and_then(|patient| {
            (patient.player == unit.player
                && patient.built
                && patient.hp > 0
                && patient.hp < patient.stats().max_hp
                && tile_adjacent_to_building(unit.tile(), patient))
            .then_some(patient.center())
        }),
        Order::RepairUnit { unit: patient } => state.unit(patient).and_then(|patient| {
            (patient.id != unit.id
                && patient.player == unit.player
                && patient.hp > 0
                && patient.hp < patient.kind.stats().max_hp
                && patient.path.is_none()
                && !matches!(patient.order, Order::Found { .. })
                && unit.pos.dist_sq(patient.pos)
                    <= oxide_sim::stats::REPAIR_REACH * oxide_sim::stats::REPAIR_REACH)
                .then_some(patient.pos)
        }),
        _ => None,
    }
}

fn active_unit_salvage(state: &State, unit: &Unit) -> Option<Vec2Fx> {
    if unit.kind != UnitKind::Harvester || unit.progress == 0 || unit.path.is_some() {
        return None;
    }
    let Order::Salvage { building } = unit.order else {
        return None;
    };
    state.building(building).and_then(|target| {
        (target.player == unit.player
            && target.built
            && target.hp > 0
            && target.kind != BuildingKind::Foundry
            && tile_adjacent_to_building(unit.tile(), target))
        .then_some(target.center())
    })
}

fn active_site_construction(state: &State, building: &Building) -> bool {
    if building.built {
        return false;
    }
    if building.tier > 0 {
        return true;
    }
    building.progress > 0
        && state.units().iter().any(|unit| {
            unit.player == building.player
                && unit.kind == UnitKind::Harvester
                && matches!(unit.order, Order::Build { site } if site == building.id)
                && tile_adjacent_to_building(unit.tile(), building)
        })
}

fn tile_adjacent_to_building(tile: chassis::grid::TilePos, building: &Building) -> bool {
    let (width, height) = building.stats().size;
    let anchor = building.anchor;
    let inside = tile.x >= anchor.x
        && tile.y >= anchor.y
        && tile.x < anchor.x + width
        && tile.y < anchor.y + height;
    !inside
        && tile.x >= anchor.x - 1
        && tile.y >= anchor.y - 1
        && tile.x <= anchor.x + width
        && tile.y <= anchor.y + height
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use chassis::fx::Vec2Fx;
    use chassis::grid::TilePos;
    use oxide_sim::{PlayerId, Scenario, Target};

    use super::*;

    fn unit_facts(kind: UnitKind) -> UnitAnimationFacts {
        UnitAnimationFacts {
            id: UnitId(7),
            kind,
            moved: false,
            work: UnitWorkFact::Idle,
            carrying: 0,
            cooldowns: [0; MAX_WEAPONS],
        }
    }

    fn building_facts(kind: BuildingKind) -> BuildingAnimationFacts {
        BuildingAnimationFacts {
            id: BuildingId(9),
            kind,
            tier: 0,
            built: true,
            progress: 0,
            construction_total: kind
                .base_stats()
                .construction
                .map(|stats| stats.build_ticks),
            construction_active: false,
            production: None,
            cooldown: 0,
        }
    }

    fn point() -> Vec2Fx {
        TilePos::new(4, 4).center()
    }

    fn unit_attack_report(tick: u64, attacker: UnitId, weapon: usize) -> TickReport {
        TickReport {
            tick,
            events: vec![Event::AttackHit {
                attacker,
                attacker_kind: UnitKind::Sentinel,
                weapon,
                target: Target::Unit(UnitId(99)),
                attacker_pos: point(),
                target_pos: point(),
            }],
        }
    }

    #[test]
    fn first_shot_is_ready_and_cooldown_prepares_only_the_next_shot() {
        let controller = AnimationController::default();
        let ready = controller.unit_state(
            unit_facts(UnitKind::Lancer),
            AnimationClock::new(0, 0.0),
            AnimationOptions::default(),
        );
        assert_eq!(ready.weapons[0], WeaponCycle::Ready);
        assert!(ready.attack.is_none());

        let mut cooling = unit_facts(UnitKind::Lancer);
        let total = UnitKind::Lancer.stats().weapons[0].cooldown_ticks;
        cooling.cooldowns[0] = total;
        let just_fired = controller.unit_state(
            cooling,
            AnimationClock::new(1, 0.0),
            AnimationOptions::default(),
        );
        assert_eq!(
            just_fired.weapons[0],
            WeaponCycle::Preparing { progress: 0.0 }
        );

        cooling.cooldowns[0] = total / 2;
        let halfway = controller.unit_state(
            cooling,
            AnimationClock::new(30, 0.0),
            AnimationOptions::default(),
        );
        assert!(matches!(
            halfway.weapons[0],
            WeaponCycle::Preparing { progress } if (progress - 0.5).abs() < 0.001
        ));
    }

    #[test]
    fn damage_event_drives_one_report_then_recovery() {
        let mut controller = AnimationController::default();
        controller.observe(&unit_attack_report(40, UnitId(7), 0));
        let facts = unit_facts(UnitKind::Sentinel);

        let report = controller.unit_state(
            facts,
            AnimationClock::new(41, 0.5),
            AnimationOptions::default(),
        );
        assert!(matches!(
            report.attack,
            Some(AttackPhase::Report { weapon: 0, .. })
        ));

        let recovery = controller.unit_state(
            facts,
            AnimationClock::new(43, 0.5),
            AnimationOptions::default(),
        );
        assert!(matches!(
            recovery.attack,
            Some(AttackPhase::Recover { weapon: 0, .. })
        ));

        let settled = controller.unit_state(
            facts,
            AnimationClock::new(46, 0.0),
            AnimationOptions::default(),
        );
        assert!(settled.attack.is_none());
    }

    #[test]
    fn projectile_launch_drives_bombard_and_bastion_reports() {
        let mut controller = AnimationController::default();
        controller.observe(&TickReport {
            tick: 8,
            events: vec![
                Event::ShellLaunched {
                    shooter: Target::Unit(UnitId(7)),
                    target: Target::Unit(UnitId(8)),
                    player: PlayerId(0),
                    from: point(),
                    to: point(),
                    flight: 10,
                },
                Event::ShellLaunched {
                    shooter: Target::Building(BuildingId(9)),
                    target: Target::Unit(UnitId(8)),
                    player: PlayerId(0),
                    from: point(),
                    to: point(),
                    flight: 10,
                },
            ],
        });
        let clock = AnimationClock::new(9, 0.0);
        assert!(matches!(
            controller
                .unit_state(
                    unit_facts(UnitKind::Bombard),
                    clock,
                    AnimationOptions::default()
                )
                .attack,
            Some(AttackPhase::Report { .. })
        ));
        assert!(matches!(
            controller
                .building_state(
                    building_facts(BuildingKind::Bastion),
                    clock,
                    AnimationOptions::default()
                )
                .attack,
            Some(AttackPhase::Report { .. })
        ));
    }

    #[test]
    fn bastion_report_is_a_single_hard_recoil_then_a_short_settle() {
        let timing = building_attack_timing(BuildingKind::Bastion);
        assert_eq!(timing.report_ticks, 1.0);
        assert_eq!(timing.recover_ticks, 3.0);
        assert!(matches!(
            attack_phase(0.99, 0, timing),
            Some(AttackPhase::Report { .. })
        ));
        assert!(matches!(
            attack_phase(1.0, 0, timing),
            Some(AttackPhase::Recover { progress: 0.0, .. })
        ));
        assert_eq!(attack_phase(4.0, 0, timing), None);
    }

    #[test]
    fn paused_clock_holds_motion_and_lift_rotors_run_while_idle() {
        let controller = AnimationController::default();
        let clock = AnimationClock::new(77, 0.35);
        let options = AnimationOptions::default();
        for kind in [UnitKind::Buzzard, UnitKind::Wisp] {
            let a = controller.unit_state(unit_facts(kind), clock, options);
            let b = controller.unit_state(unit_facts(kind), clock, options);
            assert_eq!(a, b);
            assert_eq!(a.locomotion, LocomotionState::Rest);
            assert!(matches!(a.propulsion, PropulsionState::LiftRotors { .. }));

            let held = controller.unit_state(
                unit_facts(kind),
                AnimationClock::new(200, 0.9),
                AnimationOptions {
                    reduced_motion: true,
                },
            );
            assert_eq!(held.propulsion, PropulsionState::LiftRotors { cycle: 0.0 });
        }
    }

    #[test]
    fn lift_rotor_cadence_does_not_change_when_an_aircraft_starts_moving() {
        assert_eq!(unit_move_period(UnitKind::Buzzard), BUZZARD_ROTOR_PERIOD);
        assert_eq!(unit_move_period(UnitKind::Wisp), WISP_ROTOR_PERIOD);
    }

    #[test]
    fn harvesting_requires_real_work_and_cargo_is_a_continuous_fill() {
        let state = Scenario::skirmish().build().expect("skirmish builds");
        let base = state
            .units()
            .iter()
            .find(|unit| unit.kind == UnitKind::Harvester)
            .expect("skirmish starts a harvester")
            .clone();
        let node = (0..state.map().height())
            .flat_map(|y| (0..state.map().width()).map(move |x| TilePos::new(x, y)))
            .find(|tile| state.map().scrap_at(*tile) > 0)
            .expect("skirmish carries scrap");
        let mut unit = base;
        unit.order = Order::Harvest {
            node,
            anchor: Some(node),
            retiring: false,
        };
        unit.pos = node.offset(-1, 0).center();
        unit.carrying = 5;
        unit.progress = 1;
        let facts = UnitAnimationFacts::capture(&state, &unit, false);
        assert_eq!(facts.work, UnitWorkFact::Harvesting(node.center()));
        let animation = AnimationController::default().unit_state(
            facts,
            AnimationClock::new(5, 0.0),
            AnimationOptions::default(),
        );
        assert!(matches!(animation.work, UnitWorkState::Harvesting { .. }));
        assert!(matches!(
            animation.cargo,
            Some(CargoState { fill, .. }) if (fill - 0.5).abs() < 0.001
        ));

        unit.progress = 0;
        assert_eq!(
            UnitAnimationFacts::capture(&state, &unit, false).work,
            UnitWorkFact::Harvesting(node.center()),
            "a scoop boundary retains the physical work target and facing"
        );
        unit.progress = 1;

        unit.order = Order::Harvest {
            node,
            anchor: Some(node),
            retiring: true,
        };
        assert_eq!(
            UnitAnimationFacts::capture(&state, &unit, false).work,
            UnitWorkFact::Idle
        );
    }

    #[test]
    fn construction_requires_the_assigned_harvester_at_the_site() {
        let site = Building {
            id: BuildingId(12),
            player: PlayerId(0),
            kind: BuildingKind::Fabricator,
            anchor: TilePos::new(10, 10),
            hp: 100,
            queue: VecDeque::new(),
            progress: 20,
            rally: None,
            focus: None,
            built: false,
            tier: 0,
            cooldown: 0,
            salvage_drained: 0,
            salvage_credited: 0,
            salvaged: false,
        };
        let mut builder = Unit {
            id: UnitId(2),
            player: PlayerId(0),
            kind: UnitKind::Harvester,
            pos: TilePos::new(9, 10).center(),
            hp: UnitKind::Harvester.stats().max_hp,
            carrying: 0,
            cooldowns: [0; MAX_WEAPONS],
            progress: 0,
            order: Order::Build { site: site.id },
            queue: VecDeque::new(),
            looping: false,
            path: None,
            leash: None,
            settled: 0,
            heading: 0,
            cargo: Vec::new(),
            landed: false,
        };
        assert!(tile_adjacent_to_building(builder.tile(), &site));
        assert!(matches!(builder.order, Order::Build { site: id } if id == site.id));
        builder.pos = TilePos::new(1, 1).center();
        assert!(!tile_adjacent_to_building(builder.tile(), &site));
    }

    #[test]
    fn factories_animate_only_while_queue_progress_can_advance() {
        let kind = UnitKind::Sentinel;
        let mut facts = building_facts(BuildingKind::Foundry);
        facts.production = Some((kind, 25, kind.stats().train_ticks));
        let controller = AnimationController::default();
        let working = controller.building_state(
            facts,
            AnimationClock::new(10, 0.0),
            AnimationOptions::default(),
        );
        assert!(matches!(
            working.activity,
            BuildingActivity::Production { unit, .. } if unit == kind
        ));

        facts.production = None;
        let idle = controller.building_state(
            facts,
            AnimationClock::new(10, 0.0),
            AnimationOptions::default(),
        );
        assert_eq!(idle.activity, BuildingActivity::Idle);
    }

    #[test]
    fn foundry_eye_pulses_more_slowly_than_fabricator_machinery() {
        let kind = UnitKind::Sentinel;
        let controller = AnimationController::default();
        let production_cycle = |building_kind, tick| {
            let mut facts = building_facts(building_kind);
            facts.production = Some((kind, 25, kind.stats().train_ticks));
            let state = controller.building_state(
                facts,
                AnimationClock::new(tick, 0.0),
                AnimationOptions::default(),
            );
            let BuildingActivity::Production { cycle, .. } = state.activity else {
                panic!("factory with a queue must expose production motion");
            };
            cycle
        };

        assert_eq!(
            production_cycle(BuildingKind::Foundry, 0),
            production_cycle(BuildingKind::Foundry, FOUNDRY_PRODUCTION_PERIOD)
        );
        assert_eq!(
            production_cycle(BuildingKind::Fabricator, 0),
            production_cycle(BuildingKind::Fabricator, FABRICATOR_PRODUCTION_PERIOD)
        );
        assert_ne!(
            production_cycle(BuildingKind::Foundry, FABRICATOR_PRODUCTION_PERIOD),
            production_cycle(BuildingKind::Foundry, 0)
        );
    }

    #[test]
    fn repair_bay_moves_only_after_an_accepted_repair_event() {
        let mut controller = AnimationController::default();
        let facts = building_facts(BuildingKind::RepairBay);
        let before = controller.building_state(
            facts,
            AnimationClock::new(20, 0.0),
            AnimationOptions::default(),
        );
        assert_eq!(before.activity, BuildingActivity::Idle);

        controller.observe(&TickReport {
            tick: 20,
            events: vec![Event::UnitRepaired {
                unit: UnitId(4),
                player: PlayerId(0),
                source: UnitRepairSource::RepairBay { building: facts.id },
                amount: 2,
            }],
        });
        let pulse = controller.building_state(
            facts,
            AnimationClock::new(21, 0.0),
            AnimationOptions::default(),
        );
        assert!(matches!(
            pulse.activity,
            BuildingActivity::RepairPulse { progress: 0.0 }
        ));
        let done = controller.building_state(
            facts,
            AnimationClock::new(27, 0.0),
            AnimationOptions::default(),
        );
        assert_eq!(done.activity, BuildingActivity::Idle);
    }

    #[test]
    fn array_and_reclaimer_are_the_only_continuous_building_loops() {
        let controller = AnimationController::default();
        let clock = AnimationClock::new(10, 0.5);
        let options = AnimationOptions::default();
        assert!(matches!(
            controller
                .building_state(building_facts(BuildingKind::Array), clock, options)
                .activity,
            BuildingActivity::ArraySweep { .. }
        ));
        assert!(matches!(
            controller
                .building_state(building_facts(BuildingKind::Reclaimer), clock, options)
                .activity,
            BuildingActivity::Reclaiming { .. }
        ));
        for kind in [
            BuildingKind::Foundry,
            BuildingKind::Fabricator,
            BuildingKind::RepairBay,
            BuildingKind::Turret,
            BuildingKind::FlakTurret,
            BuildingKind::Bastion,
        ] {
            assert_eq!(
                controller
                    .building_state(building_facts(kind), clock, options)
                    .activity,
                BuildingActivity::Idle,
                "{kind:?} must not receive a decorative idle loop"
            );
        }
    }

    #[test]
    fn reset_for_seek_drops_reports_but_cooldowns_still_reconstruct() {
        let mut controller = AnimationController::default();
        controller.observe(&unit_attack_report(4, UnitId(7), 0));
        let mut facts = unit_facts(UnitKind::Lancer);
        facts.cooldowns[0] = 20;
        controller.reset_transients();
        let state = controller.unit_state(
            facts,
            AnimationClock::new(5, 0.0),
            AnimationOptions::default(),
        );
        assert!(state.attack.is_none());
        assert!(matches!(state.weapons[0], WeaponCycle::Preparing { .. }));
    }

    #[test]
    fn construction_progress_freezes_machinery_when_inactive_or_reduced() {
        let mut facts = building_facts(BuildingKind::Fabricator);
        facts.built = false;
        facts.progress = 140;
        facts.construction_active = false;
        let controller = AnimationController::default();
        let inactive = controller.building_state(
            facts,
            AnimationClock::new(19, 0.5),
            AnimationOptions::default(),
        );
        assert!(matches!(
            inactive.construction,
            Some(ConstructionState {
                active: false,
                machinery_cycle: 0.0,
                ..
            })
        ));

        facts.construction_active = true;
        let reduced = controller.building_state(
            facts,
            AnimationClock::new(19, 0.5),
            AnimationOptions {
                reduced_motion: true,
            },
        );
        assert!(matches!(
            reduced.construction,
            Some(ConstructionState {
                active: true,
                machinery_cycle: 0.0,
                ..
            })
        ));
    }

    #[test]
    fn an_automatic_upgrade_animates_without_a_builder() {
        let mut scenario = Scenario::skirmish();
        scenario.players[0].scrap = 500;
        scenario.units.clear();
        scenario.buildings.extend([
            oxide_sim::scenario::BuildingSpec {
                player: 0,
                kind: BuildingKind::Fabricator,
                x: 9,
                y: 3,
            },
            oxide_sim::scenario::BuildingSpec {
                player: 0,
                kind: BuildingKind::Turret,
                x: 12,
                y: 3,
            },
        ]);
        let mut state = scenario.build().expect("upgrade fixture builds");
        let turret = state
            .buildings()
            .iter()
            .find(|building| building.kind == BuildingKind::Turret)
            .expect("fixture has a turret")
            .id;
        state.tick(&[oxide_sim::PlayerCommand {
            player: PlayerId(0),
            command: oxide_sim::Command::UpgradeBuilding { building: turret },
        }]);

        let building = state.building(turret).expect("upgrade lives");
        let facts = BuildingAnimationFacts::capture(&state, building);
        assert!(facts.construction_active);
        assert!(matches!(
            AnimationController::default()
                .building_state(
                    facts,
                    AnimationClock::new(state.current_tick(), 0.5),
                    AnimationOptions::default(),
                )
                .construction,
            Some(ConstructionState { active: true, .. })
        ));
    }

    #[test]
    fn capture_and_retention_entrypoints_follow_the_current_world() {
        let state = Scenario::skirmish().build().expect("skirmish builds");
        let clock = AnimationClock::from_state(&state, 0.25);
        assert_eq!(clock, AnimationClock::new(state.current_tick(), 0.25));

        let building = state.buildings().first().expect("skirmish has a Foundry");
        let facts = BuildingAnimationFacts::capture(&state, building);
        assert_eq!(facts.id, building.id);

        let mut controller = AnimationController::default();
        controller.observe_events(
            state.current_tick(),
            &[Event::TurretFired {
                turret: BuildingId(u32::MAX),
                kind: BuildingKind::Turret,
                target: Target::Unit(UnitId(0)),
                turret_pos: point(),
                target_pos: point(),
            }],
        );
        assert_eq!(controller.building_attacks.len(), 1);
        controller.retain_live(&state);
        assert!(controller.building_attacks.is_empty());
    }
}
