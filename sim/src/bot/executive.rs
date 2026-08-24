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
use crate::ids::{PlayerId, UnitId};
use crate::stats::{BuildingKind, UnitKind};
use chassis::grid::TilePos;

mod armies;
mod lowering;

/// Stable handle for an army within one bot's executive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ArmyId(pub u32);

/// Where an army is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmyState {
    /// Gathering at the staging point until the policy commits it.
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
    /// Where it was last sent.
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

/// Fraction of max hp below which a member is rotated out of its army.
/// The layer between policies and the sim. One per bot; carries across
/// ticks (armies are memory, legitimately — a bot is a command source,
/// not sim state).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Executive {
    armies: Vec<Army>,
    next_army: u32,
    /// Rear-line members kept out of drafts for the rest of their lives.
    rear: Vec<UnitId>,
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
            .chain(self.rear.iter().copied())
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
