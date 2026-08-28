//! The bot executive: intent in, commands out, armies in between.
//!
//! The utility policy never touches [`crate::PlayerCommand`] directly. It
//! emits [`Intent`]s against an [`super::Observation`], and
//! the executive owns everything an intent leaves unsaid: which harvester
//! builds, which fighters join an army, when a pushing army counts as
//! engaged, and when an engagement has gone wrong enough to withdraw.
//! The army lifecycle (Staging → Pushing → Engaging → Withdrawing) is the
//! anti-trickle machinery: units gather before they march, march together,
//! and leave together when the fight turns.
//!
//! Everything here is deterministic given the observation: member choice,
//! targeting, and state transitions order by explicit keys ending in ids.

use super::observation::{Observation, UnitObs};
use crate::command::{Command, PlayerCommand};
use crate::ids::{BuildingId, PlayerId, Target, UnitId};
use crate::stats::{BuildingKind, UnitKind};
use chassis::grid::TilePos;

mod armies;
mod lowering;

pub(super) use armies::{catastrophically_outmatched_near, locally_overmatches_near};

/// Stable handle for an army within one bot's executive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ArmyId(pub u32);

/// Where an army is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmyState {
    /// Gathering at a rally, or holding a live objective after arrival.
    Staging,
    /// Marching on a target as one body.
    Pushing,
    /// In contact: the executive is scoring the fight every think.
    Engaging,
    /// The fight turned: falling back to the staging point.
    Withdrawing,
}

/// A body of fighters managed as one thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Army {
    /// Handle.
    pub id: ArmyId,
    /// Members, id order. Pruned of the dead every think.
    pub members: Vec<UnitId>,
    /// Lifecycle state.
    pub state: ArmyState,
    /// Where this army gathers and falls back to.
    pub staging: TilePos,
    /// Current objective, or the visible fight area that caused a withdrawal.
    pub target: Option<TilePos>,
    /// The unit the whole army is concentrating on while engaged.
    /// Spread fire kills nothing; focus deletes one gun at a time.
    pub focus: Option<UnitId>,
    /// Best distance yet toward the current march goal and the tick it
    /// was set. A march that beats it resets the wedge clock; one that
    /// stalls past the patience re-stages in place.
    pub progress: Option<(i32, u64)>,
    /// The tick and vanguard tile of the last march order, so a later
    /// think can tell a march that never started (every member idle
    /// where it stood — the sim refused the order) from one under way.
    pub issued: Option<(u64, TilePos)>,
    /// Consecutive march orders that bounced at issue. Two in a row is
    /// route testimony: the target is unreachable from here today.
    pub bounces: u8,
}

/// What a policy may ask for. Intents mutate executive bookkeeping or
/// request commands; they are not commands themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// Queue a unit at an own factory.
    TrainAt {
        /// The producing building.
        building: crate::ids::BuildingId,
        /// What to build.
        kind: UnitKind,
    },
    /// Start a construction site (the executive picks the builder).
    Build {
        /// What to construct.
        kind: BuildingKind,
        /// Footprint anchor.
        anchor: TilePos,
    },
    /// Start a construction site with one exact policy-selected builder.
    ///
    /// Player-facing policy binds an implicit [`Self::Build`] only after it
    /// has checked the worker's fog-honest command route. The profile-free
    /// Overseer keeps the historical implicit-builder path.
    BuildWith {
        /// Exact Harvester whose route was checked.
        builder: UnitId,
        /// What to construct.
        kind: BuildingKind,
        /// Footprint anchor.
        anchor: TilePos,
    },
    /// Draft idle, un-enlisted fighters (nearest first, up to `size`)
    /// into the army staged at this rally point — reinforcing it if one
    /// is already staging there, creating it otherwise. Repeating the
    /// intent every think is how a policy feeds a gathering army without
    /// trickling units into battle.
    FormArmy {
        /// Gather point.
        staging: TilePos,
        /// How many fighters to draft.
        size: u32,
    },
    /// Commit a staged (or withdrawn) army against a target.
    PushArmy {
        /// Which army.
        army: ArmyId,
        /// Where to attack toward.
        target: TilePos,
    },
    /// Move one exact strategic group without engaging. The executive filters
    /// stale, dead, duplicate, and non-owned ids before lowering.
    MoveUnits {
        /// Exact operation members.
        units: Vec<UnitId>,
        /// Destination tile.
        goal: TilePos,
    },
    /// March one exact strategic group toward a tile while engaging.
    AttackMoveUnits {
        /// Exact operation members.
        units: Vec<UnitId>,
        /// Destination tile.
        goal: TilePos,
    },
    /// Commit one exact strategic group against a visible target.
    AttackUnits {
        /// Exact operation members.
        units: Vec<UnitId>,
        /// Enemy unit or building.
        target: Target,
    },
    /// Send exact mobile welders to repair one own ground unit.
    RepairUnits {
        /// Exact welders, normally Tenders.
        welders: Vec<UnitId>,
        /// Wounded own unit.
        target: UnitId,
    },
    /// Cancel voluntary paid repair programs on exact own units.
    StopUnits {
        /// Repairers whose active order and queue must be cleared.
        units: Vec<UnitId>,
    },
    /// Put a harvester on a node.
    AssignHarvest {
        /// The harvester.
        unit: UnitId,
        /// The node tile.
        node: TilePos,
    },
    /// Send a unit to look at a tile (a plain move — scouts are
    /// expendable, not brave).
    Scout {
        /// The scout.
        unit: UnitId,
        /// Where to look.
        to: TilePos,
    },
    /// Weld a damaged own building (the executive picks the welder).
    Repair {
        /// The patient.
        building: crate::ids::BuildingId,
    },
    /// Strip an own built building for scrap (the executive picks the
    /// crew; the sim refuses Foundries).
    Salvage {
        /// The building coming down.
        building: crate::ids::BuildingId,
    },
    /// Throw every idle ground-attack flyer at a target — a strike, not
    /// an army: no lifecycle, no withdraw call, just wings and a place.
    RaidAir {
        /// Where the strike flies.
        target: TilePos,
    },
    /// Commit a built own building to its automatic next-tier rebuild.
    Upgrade {
        /// The works to lift.
        building: crate::ids::BuildingId,
    },
    /// Send riders to climb aboard an own transport.
    Load {
        /// The carrier.
        transport: UnitId,
        /// The machines to carry.
        riders: Vec<UnitId>,
    },
    /// Fly a transport to a tile and set its riders down.
    Unload {
        /// The carrier.
        transport: UnitId,
        /// The drop point.
        at: TilePos,
    },
}

/// One fighter waiting behind the line and the tick its retreat began.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RearUnit {
    id: UnitId,
    since: u64,
}

/// Player-facing tactical state that survives between decision ticks.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlayerFacingTactics {
    frame: armies::CentroidFrame,
    defense_focus: Option<(Target, Vec<BuildingId>)>,
}

/// The layer between policies and the sim. One per bot; carries across
/// ticks because armies are controller memory rather than simulation state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Executive {
    armies: Vec<Army>,
    next_army: u32,
    /// Rear-line members temporarily kept out of drafts.
    rear: Vec<RearUnit>,
    /// Wounded veterans whose rear-line wait expired without a repair. They
    /// may fight again and are not pulled out a second time until genuinely
    /// repaired.
    exhausted_rear: Vec<UnitId>,
    /// Stable owner-facing tactical state. Player-facing brains latch it on
    /// their first maintenance pass; the profile-free Overseer leaves it
    /// absent and preserves its historical world-space centroids.
    player_frame: Option<PlayerFacingTactics>,
}

impl Executive {
    /// Fresh, armyless executive.
    pub fn new() -> Self {
        Self::default()
    }

    /// Read access for policies and tests.
    pub fn armies(&self) -> &[Army] {
        &self.armies
    }

    /// Ids of units already spoken for (army members and the rear line) —
    /// the pool FormArmy and harvest assignment must not double-book.
    pub fn enlisted(&self) -> impl Iterator<Item = UnitId> + '_ {
        self.armies
            .iter()
            .flat_map(|a| a.members.iter().copied())
            .chain(self.rear.iter().map(|unit| unit.id))
    }
}

/// hp-weighted dps a unit can bring against the given movement domain —
/// the shared coin every fight estimate is priced in. Damage per 100
/// ticks keeps it in integers (cooldowns divide 100 unevenly — close
/// enough for margin calls that carry hysteresis).
fn strength_vs(u: &UnitObs, domain: crate::stats::Domain) -> u64 {
    let stats = u.kind.stats();
    let dps100: u64 = stats
        .weapons
        .iter()
        .filter(|w| w.targets.covers(domain))
        .map(|w| u64::from(w.damage) * 100 / u64::from(w.cooldown_ticks))
        .sum();
    u64::from(u.hp) * dps100
}

/// Ground-battle strength. Weapons that can only look up contribute
/// nothing, so an anti-air escort never inflates a push estimate.
pub(super) fn unit_strength(u: &UnitObs) -> u64 {
    strength_vs(u, crate::stats::Domain::Ground)
}

/// Ground strength that the next march order will actually deploy. Long guns
/// without their escort quorum remain at staging and cannot justify a push.
pub(super) fn marching_strength(army: &Army, obs: &Observation) -> u64 {
    let artillery_moves = armies::artillery_has_escort_quorum(army, obs);
    obs.my_units
        .iter()
        .filter(|unit| army.members.contains(&unit.id))
        .filter(|unit| artillery_moves || !armies::is_artillery(unit))
        .map(unit_strength)
        .sum()
}

/// Whether these two would have anything to say to each other in a
/// fight: the enemy is armed and coverable, or it can cover us. Unarmed
/// enemies never constitute a fight (chasing a fleeing harvester is not
/// an engagement), and a flak platform staring at ground infantry is
/// scenery to both sides.
fn mutually_relevant(member: &UnitObs, enemy: &UnitObs) -> bool {
    let m = member.kind.stats();
    let e = enemy.kind.stats();
    (e.can_fight() && m.can_target(e.domain)) || e.can_target(m.domain)
}

/// Same coin for a standing building (turrets; zero for the unarmed).
pub(super) fn building_strength(b: &super::observation::BuildingObs) -> u64 {
    if !b.built {
        return 0;
    }
    let dps100: u64 = b
        .kind
        .base_stats()
        .weapons
        .iter()
        .filter(|w| w.targets.ground)
        .map(|w| u64::from(w.damage) * 100 / u64::from(w.cooldown_ticks))
        .sum();
    u64::from(b.hp) * dps100
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wedge_clock_arms_tracks_and_expires() {
        let mut progress = None;
        // First sighting arms the clock without reporting a wedge.
        assert!(!armies::wedged(&mut progress, 40, 100));
        assert_eq!(progress, Some((40, 100)));
        // Any strictly better distance resets the clock.
        assert!(!armies::wedged(
            &mut progress,
            39,
            100 + armies::ARMY_PROGRESS_PATIENCE_TICKS
        ));
        assert_eq!(
            progress,
            Some((39, 100 + armies::ARMY_PROGRESS_PATIENCE_TICKS))
        );
        // Matching the best is not progress; short of patience holds.
        assert!(!armies::wedged(
            &mut progress,
            39,
            99 + 2 * armies::ARMY_PROGRESS_PATIENCE_TICKS
        ));
        // A full patience window with no better distance is a wedge,
        // even if the reported distance oscillates upward meanwhile.
        assert!(armies::wedged(
            &mut progress,
            55,
            100 + 2 * armies::ARMY_PROGRESS_PATIENCE_TICKS
        ));
    }

    #[test]
    fn an_upgrade_intent_needs_no_harvester() {
        let state = crate::Scenario::skirmish().build().unwrap();
        let mut obs = Observation::omniscient(&state, PlayerId(0));
        obs.my_units.clear();
        let building = obs.my_buildings[0].id;

        let commands = Executive::new().apply(PlayerId(0), &obs, &[Intent::Upgrade { building }]);

        assert_eq!(
            commands,
            vec![PlayerCommand {
                player: PlayerId(0),
                command: Command::UpgradeBuilding { building },
            }]
        );
    }

    #[test]
    fn player_facing_repair_chooses_only_a_route_capable_welder() {
        let me = PlayerId(0);
        let state = crate::Scenario::skirmish().build().unwrap();
        let mut obs = Observation::omniscient(&state, me);
        let wall_x = obs.map_width / 2;
        let y = obs.map_height / 2;
        let mut target = obs
            .my_buildings
            .iter()
            .find(|building| building.kind == BuildingKind::Foundry)
            .cloned()
            .expect("the skirmish has a player-zero Foundry");
        target.anchor = TilePos::new(wall_x + 4, y - 1);
        let building = target.id;
        obs.my_buildings = vec![target];
        obs.my_queues = vec![Vec::new()];
        obs.known_scrap.clear();
        obs.known_wrecks.clear();
        obs.known_rock = (0..obs.map_height)
            .map(|row| TilePos::new(wall_x, row))
            .collect();

        let mut blocked = obs
            .my_units
            .iter()
            .find(|unit| unit.kind == UnitKind::Harvester)
            .cloned()
            .expect("the skirmish has a player-zero Harvester");
        blocked.id = UnitId(100);
        blocked.tile = TilePos::new(wall_x - 1, y);
        let mut reachable = blocked.clone();
        reachable.id = UnitId(101);
        reachable.tile = TilePos::new(wall_x + 10, y);
        obs.my_units = vec![blocked.clone(), reachable.clone()];

        let intent = [Intent::Repair { building }];
        let commands = Executive::new().apply_with_reservations(me, &obs, &intent, &[]);
        assert!(matches!(
            commands.as_slice(),
            [PlayerCommand {
                command: Command::Repair { units, building: target, queue: false },
                ..
            }] if units == &[reachable.id] && *target == building
        ));

        obs.my_units = vec![blocked.clone()];
        assert!(
            Executive::new()
                .apply_with_reservations(me, &obs, &intent, &[])
                .is_empty(),
            "the player-facing controller must not emit a repair that cannot route"
        );
        assert!(matches!(
            Executive::new().apply(me, &obs, &intent).as_slice(),
            [PlayerCommand {
                command: Command::Repair { units, building: target, queue: false },
                ..
            }] if units == &[blocked.id] && *target == building
        ));
    }

    #[test]
    fn a_stranded_economy_reserves_its_bank_for_exactly_one_harvester() {
        let state = crate::Scenario::skirmish().build().unwrap();
        let mut obs = Observation::omniscient(&state, PlayerId(0));
        obs.my_units
            .retain(|unit| unit.kind.stats().harvest.is_none());
        let foundry_index = obs
            .my_buildings
            .iter()
            .position(|building| building.kind == BuildingKind::Foundry)
            .expect("the skirmish has a home Foundry");
        let foundry = obs.my_buildings[foundry_index].id;
        let price = UnitKind::Harvester.stats().cost;
        let exec = Executive::new();

        obs.scrap = price - 1;
        assert_eq!(
            exec.harvester_recovery(PlayerId(0), &obs),
            Some(Vec::new()),
            "ordinary policy spending must pause while the replacement fund is short"
        );

        obs.scrap = price;
        assert_eq!(
            exec.harvester_recovery(PlayerId(0), &obs),
            Some(vec![PlayerCommand {
                player: PlayerId(0),
                command: Command::Train {
                    building: foundry,
                    kind: UnitKind::Harvester,
                },
            }]),
            "the complete reserve buys the recovery unit and nothing else"
        );

        obs.my_queues[foundry_index].push(UnitKind::Harvester);
        assert_eq!(
            exec.harvester_recovery(PlayerId(0), &obs),
            None,
            "a prepaid replacement returns the seat to ordinary policy"
        );

        obs.my_queues[foundry_index] = vec![UnitKind::Sentinel; crate::stats::QUEUE_CAP];
        assert_eq!(
            exec.harvester_recovery(PlayerId(0), &obs),
            Some(Vec::new()),
            "a full queue must keep the reserve intact until a slot opens"
        );

        obs.my_buildings[foundry_index].built = false;
        assert_eq!(
            exec.harvester_recovery(PlayerId(0), &obs),
            None,
            "without a standing Foundry there is no legal recovery purchase"
        );
    }
}
