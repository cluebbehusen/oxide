//! Action-driven sprite animation state derived from simulation facts.
//!
//! This module owns no gameplay state. Its clocks advance in completed
//! simulation ticks, while [`AnimationController`] remembers only recent
//! output events and mechanism transitions that are not recoverable from the current world snapshot.
//! Clearing the controller is therefore always safe when seeking or bulk
//! advancing a replay.

use std::collections::HashMap;

use chassis::fx::Vec2Fx;
use oxide_sim::stats::{Domain, MAX_WEAPONS};
use oxide_sim::{
    Building, BuildingId, BuildingKind, Event, Order, State, Unit, UnitId, UnitKind,
    UnitRepairSource,
};

const GROUND_MOVE_PERIOD: u64 = 6;
const HARVEST_PERIOD: u64 = 20;
const EXCAVATOR_ROLLER_PERIOD: u64 = 20;
const WELD_ARM_FOLD_TICKS: f32 = 12.0;
const CONSTRUCTION_PERIOD: u64 = 8;
const FOUNDRY_PRODUCTION_PERIOD: u64 = 24;
const FABRICATOR_PRODUCTION_PERIOD: u64 = 12;
const CRUCIBLE_PRODUCTION_PERIOD: u64 = 16;
const AIRWORKS_PRODUCTION_PERIOD: u64 = 12;
const ARRAY_SWEEP_PERIOD: u64 = 32;
const EXTRACTOR_PERIOD: u64 = 16;
const RECLAIMER_PERIOD: u64 = 24;
const BUZZARD_ROTOR_PERIOD: u64 = 6;
const WISP_ROTOR_PERIOD: u64 = 6;
const SKYHOOK_ROTOR_PERIOD: u64 = 9;
const SKYHOOK_ACTION_TICKS: f32 = 8.0;
const REPAIR_PULSE_TICKS: f32 = 6.0;
const AIRWORKS_LAUNCH_TICKS: u64 = 16;
const AIRWORKS_OPEN_TICKS: u32 = 12;
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

/// Identity of the visible surface touched by a worker's tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkTarget {
    Scrap(chassis::grid::TilePos),
    Wreck(chassis::grid::TilePos),
    Building(BuildingId),
    Unit(UnitId),
}

/// Work performed by a non-combat unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum UnitWorkState {
    /// No mechanism is presently doing work.
    Idle,
    /// A scrap worker is physically extracting an adjacent node or the wreck
    /// under its chassis.
    Harvesting { target: Vec2Fx, cycle: f32 },
    /// A scrap worker is adjacent to and actively raising this paid site.
    Constructing {
        site: BuildingId,
        target: Vec2Fx,
        cycle: f32,
    },
    /// A field welder is actively repairing a wounded unit or building.
    Repairing { target: Vec2Fx, cycle: f32 },
    /// A scrap worker is actively stripping a friendly structure for scrap.
    Salvaging { target: Vec2Fx, cycle: f32 },
    /// Cargo is still aboard during this authoritative release cycle.
    Unloading { target: Vec2Fx, progress: f32 },
}

/// The Excavator mechanism that performs a unit's current work.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ExcavatorTool {
    /// Both mechanisms rest against the chassis.
    Stowed,
    /// The front milling drum grinds scrap, a wreck, or a structure being
    /// salvaged.
    Drum { cycle: f32 },
    /// The side welding arm raises a site or repairs a patient.
    WeldingArm { cycle: f32 },
}

impl UnitWorkState {
    /// The Excavator mechanism that performs this work.
    pub(crate) fn excavator_tool(self) -> ExcavatorTool {
        match self {
            Self::Harvesting { cycle, .. } | Self::Salvaging { cycle, .. } => {
                ExcavatorTool::Drum { cycle }
            }
            Self::Constructing { cycle, .. } | Self::Repairing { cycle, .. } => {
                ExcavatorTool::WeldingArm { cycle }
            }
            Self::Idle | Self::Unloading { .. } => ExcavatorTool::Stowed,
        }
    }
}

/// The visible fill of a scrap worker's internal cargo bay.
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

/// A transport mechanism settling after an authoritative boarding event.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum TransportActionState {
    /// A rider entered this transport.
    Boarding { progress: f32 },
    /// A rider left this transport.
    Unloading { progress: f32 },
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

/// A chassis-mounted tool unfolding toward its work surface or returning to stow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WeldingArmState {
    pub(crate) target: WorkTarget,
    pub(crate) deployment: f32,
    pub(crate) active: bool,
}

/// All independent animation channels for one unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct UnitAnimationState {
    /// Position-driven tread, wheel, leg, or internal propulsion phase.
    pub(crate) locomotion: LocomotionState,
    /// Economy mechanism state.
    pub(crate) work: UnitWorkState,
    pub(crate) work_target: Option<WorkTarget>,
    pub(crate) welding_arm: Option<WeldingArmState>,
    /// Harvest cargo, present for Harvesters and Excavators.
    pub(crate) cargo: Option<CargoState>,
    /// Event-driven attack report and recovery.
    pub(crate) attack: Option<AttackPhase>,
    /// Cooldown-driven preparation, one entry per simulation weapon slot.
    pub(crate) weapons: [WeaponCycle; MAX_WEAPONS],
    /// Mechanisms that must run independently of locomotion.
    pub(crate) propulsion: PropulsionState,
    /// Continuously powered scout scanner, independent of flight or movement.
    pub(crate) scanner: Option<f32>,
    /// Event-driven cargo-door and clamp movement.
    pub(crate) transport: Option<TransportActionState>,
    /// A Sapper has physically reached contact and will detonate next tick.
    pub(crate) demolition_preparation: Option<f32>,
}

/// Facts about a unit that can be captured without mutating the simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UnitAnimationFacts {
    id: UnitId,
    kind: UnitKind,
    moved: bool,
    work: UnitWorkFact,
    work_target: Option<WorkTarget>,
    carrying: u32,
    demolition_contact: bool,
    cooldowns: [u32; MAX_WEAPONS],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnitWorkFact {
    Idle,
    Harvesting(Vec2Fx, u32),
    Constructing(BuildingId, Vec2Fx),
    Repairing(Vec2Fx),
    Salvaging(Vec2Fx),
    Unloading(Vec2Fx, u8),
}

impl UnitAnimationFacts {
    /// Reads the unit's visible mechanisms from the post-tick world.
    pub(crate) fn capture(state: &State, unit: &Unit, moved: bool) -> Self {
        let unloading = unit.unloading.and_then(|release| {
            state
                .building(release.foundry)
                .filter(|b| unit.work_stopped() && state.in_building_work_reach(unit, b.id))
                .map(|b| (b.center(), release.elapsed))
        });
        let work = if let Some((target, elapsed)) = unloading {
            UnitWorkFact::Unloading(target, elapsed)
        } else if let Some(target) = active_harvesting(state, unit) {
            UnitWorkFact::Harvesting(target, unit.progress)
        } else if let Some((site, target)) = active_unit_construction(state, unit) {
            UnitWorkFact::Constructing(site, target)
        } else if let Some(target) = active_unit_repair(state, unit) {
            UnitWorkFact::Repairing(target)
        } else if let Some(target) = active_unit_salvage(state, unit) {
            UnitWorkFact::Salvaging(target)
        } else {
            UnitWorkFact::Idle
        };
        let work_target = if work == UnitWorkFact::Idle {
            None
        } else if let Some(release) = unit.unloading {
            Some(WorkTarget::Building(release.foundry))
        } else {
            match unit.order {
                Order::Harvest { node, .. } => Some(if state.map().scrap_at(node) > 0 {
                    WorkTarget::Scrap(node)
                } else {
                    WorkTarget::Wreck(node)
                }),
                Order::Build { site } => Some(WorkTarget::Building(site)),
                Order::Repair { building } | Order::Salvage { building } => {
                    Some(WorkTarget::Building(building))
                }
                Order::RepairUnit { unit } => Some(WorkTarget::Unit(unit)),
                _ => None,
            }
        };
        Self {
            work_target,
            id: unit.id,
            kind: unit.kind,
            moved,
            work,
            carrying: unit.carrying,
            demolition_contact: sapper_at_contact(state, unit),
            cooldowns: unit.cooldowns,
        }
    }

    /// Resolves the visible work at a render instant.
    fn work_state(self, clock: AnimationClock, options: AnimationOptions) -> UnitWorkState {
        match self.work {
            UnitWorkFact::Idle => UnitWorkState::Idle,
            UnitWorkFact::Unloading(target, elapsed) => UnitWorkState::Unloading {
                target,
                progress: if options.reduced_motion {
                    0.5
                } else {
                    (f32::from(elapsed) + clock.tick_fraction)
                        / f32::from(oxide_sim::stats::UNLOAD_TICKS)
                },
            },
            UnitWorkFact::Harvesting(target, progress) => UnitWorkState::Harvesting {
                target,
                cycle: if self.kind == UnitKind::Excavator {
                    clock.cycle(self.id.0, EXCAVATOR_ROLLER_PERIOD, options.reduced_motion)
                } else if options.reduced_motion {
                    0.5
                } else {
                    (progress as f32 + clock.tick_fraction)
                        / self.kind.stats().harvest.map_or(1, |h| h.ticks_per_scrap) as f32
                },
            },
            UnitWorkFact::Constructing(site, target) => UnitWorkState::Constructing {
                site,
                target,
                cycle: clock.cycle(self.id.0, CONSTRUCTION_PERIOD, options.reduced_motion),
            },
            UnitWorkFact::Repairing(target) => UnitWorkState::Repairing {
                target,
                cycle: clock.cycle(self.id.0, CONSTRUCTION_PERIOD, options.reduced_motion),
            },
            UnitWorkFact::Salvaging(target) => UnitWorkState::Salvaging {
                target,
                cycle: clock.cycle(self.id.0, HARVEST_PERIOD, options.reduced_motion),
            },
        }
    }
}

/// A site under construction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ConstructionState {
    /// Normalized build completion.
    pub(crate) progress: f32,
    /// Whether an assigned scrap worker is adjacent and advancing the site.
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
    /// An Airworks has opened its bay to release a completed aircraft.
    AirworksLaunch { progress: f32 },
    /// A completed Array's continuous full-bearing scan.
    ArraySweep { cycle: f32 },
    /// A completed Extractor's continuous radial auger cycle.
    Extracting { cycle: f32 },
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
        // new tier's labor budget, and a base denominator would show the
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

#[derive(Debug, Clone, Copy)]
enum TransportActionKind {
    Boarding,
    Unloading,
}

#[derive(Debug, Clone, Copy)]
struct TransportActionStamp {
    completed_tick: u64,
    kind: TransportActionKind,
}

#[derive(Debug, Clone, Copy)]
struct WeldingArmTransition {
    target: WorkTarget,
    from: f32,
    started: u64,
    extending: bool,
}

impl WeldingArmTransition {
    fn deployment(self, clock: AnimationClock) -> f32 {
        let elapsed = clock.elapsed_since(self.started).unwrap_or(0.) / WELD_ARM_FOLD_TICKS;
        (self.from + if self.extending { elapsed } else { -elapsed }).clamp(0., 1.)
    }
}

/// Presentation-only memory for transient reports and mechanism transitions.
#[derive(Debug, Default)]
pub(crate) struct AnimationController {
    unit_attacks: HashMap<UnitId, [Option<u64>; MAX_WEAPONS]>,
    building_attacks: HashMap<BuildingId, AttackStamp>,
    repair_pulses: HashMap<BuildingId, u64>,
    transport_actions: HashMap<UnitId, TransportActionStamp>,
    airworks_launches: HashMap<BuildingId, u64>,
    welding_arms: HashMap<UnitId, WeldingArmTransition>,
}

impl AnimationController {
    /// Records transient action events at the completed tick.
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
                Event::BuildingRepaired { repair_bay, .. } => {
                    self.repair_pulses.insert(*repair_bay, completed_tick);
                }
                Event::UnitBoarded { transport, .. } => {
                    self.transport_actions.insert(
                        *transport,
                        TransportActionStamp {
                            completed_tick,
                            kind: TransportActionKind::Boarding,
                        },
                    );
                }
                Event::UnitUnloaded { transport, .. } => {
                    self.transport_actions.insert(
                        *transport,
                        TransportActionStamp {
                            completed_tick,
                            kind: TransportActionKind::Unloading,
                        },
                    );
                }
                Event::UnitTrained { building, kind, .. } if kind.stats().domain == Domain::Air => {
                    self.airworks_launches.insert(*building, completed_tick);
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
        self.transport_actions.clear();
        self.airworks_launches.clear();
        self.welding_arms.clear();
    }

    /// Forgets ids no longer present in the current world.
    pub(crate) fn retain_live(&mut self, state: &State) {
        self.unit_attacks.retain(|id, _| state.unit(*id).is_some());
        self.building_attacks
            .retain(|id, _| state.building(*id).is_some());
        self.repair_pulses
            .retain(|id, _| state.building(*id).is_some());
        self.transport_actions
            .retain(|id, _| state.unit(*id).is_some());
        self.airworks_launches.retain(|building, completed_tick| {
            state.building(*building).is_some()
                && state.current_tick().saturating_sub(*completed_tick) < AIRWORKS_LAUNCH_TICKS
        });
    }

    /// Captures tool transitions once per completed simulation tick.
    pub(crate) fn observe_workers(&mut self, state: &State) {
        let tick = state.current_tick();
        for unit in state
            .units()
            .iter()
            .filter(|unit| unit.kind == UnitKind::Excavator)
        {
            self.observe_worker(UnitAnimationFacts::capture(state, unit, false), tick);
        }
        self.welding_arms.retain(|id, arm| {
            state.unit(*id).is_some()
                && (arm.extending || arm.deployment(AnimationClock::new(tick, 0.)) > 0.)
        });
    }

    /// Rebuilds settled tools at a seek destination without replaying deployment.
    pub(crate) fn reset_workers(&mut self, state: &State) {
        self.welding_arms.clear();
        self.observe_workers(state);
        for arm in self.welding_arms.values_mut() {
            arm.from = 1.;
        }
    }

    fn observe_worker(&mut self, facts: UnitAnimationFacts, tick: u64) {
        let clock = AnimationClock::new(tick, 0.);
        let welding = matches!(
            facts
                .work_state(clock, AnimationOptions::default())
                .excavator_tool(),
            ExcavatorTool::WeldingArm { .. }
        );
        if let Some(target) = welding.then_some(facts.work_target).flatten() {
            let arm = self
                .welding_arms
                .entry(facts.id)
                .or_insert(WeldingArmTransition {
                    target,
                    from: 0.,
                    started: tick,
                    extending: true,
                });
            if !arm.extending {
                arm.from = arm.deployment(clock);
                arm.started = tick;
                arm.extending = true;
            }
            arm.target = target;
        } else if let Some(arm) = self.welding_arms.get_mut(&facts.id)
            && arm.extending
        {
            arm.from = arm.deployment(clock);
            arm.started = tick;
            arm.extending = false;
        }
    }

    fn welding_arm(
        &self,
        facts: UnitAnimationFacts,
        work: UnitWorkState,
        clock: AnimationClock,
        options: AnimationOptions,
    ) -> Option<WeldingArmState> {
        if facts.kind != UnitKind::Excavator {
            return None;
        }
        let active = matches!(work.excavator_tool(), ExcavatorTool::WeldingArm { .. });
        if let Some(arm) = self.welding_arms.get(&facts.id) {
            let deployment = if options.reduced_motion {
                f32::from(active)
            } else {
                arm.deployment(clock)
            };
            (active || deployment > 0.).then_some(WeldingArmState {
                target: arm.target,
                deployment,
                active,
            })
        } else {
            active
                .then_some(facts.work_target)
                .flatten()
                .map(|target| WeldingArmState {
                    target,
                    deployment: 1.,
                    active: true,
                })
        }
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
        let work = facts.work_state(clock, options);
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
            UnitKind::Skyhook => PropulsionState::LiftRotors {
                cycle: clock.cycle(facts.id.0, SKYHOOK_ROTOR_PERIOD, options.reduced_motion),
            },
            _ => PropulsionState::None,
        };
        UnitAnimationState {
            work_target: facts.work_target,
            welding_arm: self.welding_arm(facts, work, clock, options),
            locomotion,
            work,
            cargo,
            attack: self.unit_attack(facts.id, facts.kind, clock),
            weapons,
            propulsion,
            scanner: matches!(facts.kind, UnitKind::Kestrel | UnitKind::Gnat)
                .then(|| clock.cycle(facts.id.0, 96, options.reduced_motion)),
            transport: self.transport_action(facts.id, clock),
            demolition_preparation: facts
                .demolition_contact
                .then_some(if options.reduced_motion {
                    0.75
                } else {
                    clock.tick_fraction
                }),
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
        let activity = if facts.built {
            match facts.kind {
                BuildingKind::Foundry | BuildingKind::Fabricator | BuildingKind::Crucible => {
                    let period = match facts.kind {
                        BuildingKind::Foundry => FOUNDRY_PRODUCTION_PERIOD,
                        BuildingKind::Fabricator => FABRICATOR_PRODUCTION_PERIOD,
                        BuildingKind::Crucible => CRUCIBLE_PRODUCTION_PERIOD,
                        _ => unreachable!("production match narrowed the building kind"),
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
                BuildingKind::Airworks => self.airworks_activity(facts, clock, options),
                BuildingKind::Array => BuildingActivity::ArraySweep {
                    cycle: clock.cycle(facts.id.0, ARRAY_SWEEP_PERIOD, options.reduced_motion),
                },
                BuildingKind::Extractor => BuildingActivity::Extracting {
                    cycle: clock.cycle(facts.id.0, EXTRACTOR_PERIOD, options.reduced_motion),
                },
                BuildingKind::Reclaimer => BuildingActivity::Reclaiming {
                    cycle: clock.cycle(facts.id.0, RECLAIMER_PERIOD, options.reduced_motion),
                },
                BuildingKind::RepairBay => self.repair_activity(facts.id, clock),
                _ => BuildingActivity::Idle,
            }
        } else {
            BuildingActivity::Idle
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

    fn transport_action(
        &self,
        transport: UnitId,
        clock: AnimationClock,
    ) -> Option<TransportActionState> {
        let stamp = self.transport_actions.get(&transport)?;
        let elapsed = clock.elapsed_since(stamp.completed_tick)?;
        if elapsed >= SKYHOOK_ACTION_TICKS {
            return None;
        }
        let progress = (elapsed / SKYHOOK_ACTION_TICKS).clamp(0.0, 1.0);
        Some(match stamp.kind {
            TransportActionKind::Boarding => TransportActionState::Boarding { progress },
            TransportActionKind::Unloading => TransportActionState::Unloading { progress },
        })
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

    fn airworks_activity(
        &self,
        facts: BuildingAnimationFacts,
        clock: AnimationClock,
        options: AnimationOptions,
    ) -> BuildingActivity {
        if let Some(progress) = self
            .airworks_launches
            .get(&facts.id)
            .and_then(|completed_tick| clock.elapsed_since(*completed_tick))
            .filter(|elapsed| *elapsed < AIRWORKS_LAUNCH_TICKS as f32)
            .map(|elapsed| (2.0 - 2.0 * elapsed / AIRWORKS_LAUNCH_TICKS as f32).clamp(0.0, 1.0))
        {
            return BuildingActivity::AirworksLaunch { progress };
        }
        if let Some((_, progress, total)) = facts.production {
            let remaining = total.saturating_sub(progress);
            if remaining <= AIRWORKS_OPEN_TICKS {
                return BuildingActivity::AirworksLaunch {
                    progress: 1.0 - remaining as f32 / AIRWORKS_OPEN_TICKS as f32,
                };
            }
        }
        facts
            .production
            .map_or(BuildingActivity::Idle, |(unit, progress, total)| {
                BuildingActivity::Production {
                    unit,
                    progress: ratio(progress, total),
                    cycle: clock.cycle(
                        facts.id.0,
                        AIRWORKS_PRODUCTION_PERIOD,
                        options.reduced_motion,
                    ),
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
        UnitKind::Skyhook => SKYHOOK_ROTOR_PERIOD,
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

fn sapper_at_contact(state: &State, unit: &Unit) -> bool {
    let Some(demolition) = unit.kind.stats().demolition else {
        return false;
    };
    let Order::Attack { target, .. } = unit.order else {
        return false;
    };
    let target_pos = state
        .attack_view(unit.player, target)
        .map(|view| view.aim_from(unit.pos));
    target_pos.is_some_and(|target| {
        let reach = demolition.contact_range;
        unit.pos.dist_sq(target) <= reach * reach
    })
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
    if unit.carrying >= harvest.capacity || unit.unloading.is_some() || !unit.work_stopped() {
        return None;
    }
    let active = (state.map().scrap_at(node) > 0 || state.map().wreck_at(node) > 0)
        && unit.in_work_reach(node, (1, 1));
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
            && unit.kind.stats().harvest.is_some()
            && unit.work_stopped()
            && state.in_building_work_reach(unit, building.id))
        .then_some((site, building.center()))
    })
}

fn active_unit_repair(state: &State, unit: &Unit) -> Option<Vec2Fx> {
    if !unit.kind.stats().welder || unit.progress == 0 || !unit.work_stopped() {
        return None;
    }
    match unit.order {
        Order::Repair { building } => state.building(building).and_then(|patient| {
            (patient.player == unit.player
                && patient.built
                && patient.hp > 0
                && patient.hp < patient.stats().max_hp
                && state.in_building_work_reach(unit, patient.id))
            .then_some(patient.center())
        }),
        Order::RepairUnit { unit: patient } => state.unit(patient).and_then(|patient| {
            (patient.id != unit.id
                && patient.player == unit.player
                && patient.hp > 0
                && patient.hp < patient.kind.stats().max_hp
                && patient.path.is_none()
                && !matches!(patient.order, Order::Found { .. })
                && patient.drive_speed == chassis::fx::Fx::ZERO
                && unit.in_repair_reach(patient))
            .then_some(patient.pos)
        }),
        _ => None,
    }
}

fn active_unit_salvage(state: &State, unit: &Unit) -> Option<Vec2Fx> {
    if unit.kind.stats().harvest.is_none() || unit.progress == 0 || !unit.work_stopped() {
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
            && state.in_building_work_reach(unit, target.id))
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
                && unit.kind.stats().harvest.is_some()
                && matches!(unit.order, Order::Build { site } if site == building.id)
                && unit.work_stopped()
                && state.in_building_work_reach(unit, building.id)
        })
}

#[cfg(test)]
// Explicit because `shell/tests/presentation_animation.rs` also includes
// this file by path, and a path-included file resolves children beside it.
#[path = "presentation_animation/tests.rs"]
mod tests;
