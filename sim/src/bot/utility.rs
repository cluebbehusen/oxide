//! The utility policy: decision channels over an [`Observation`].
//!
//! The policy is a set
//! of independent **channels** — economy, production, construction,
//! scouting, army command — each contributing its best intents per think
//! under one shared scrap budget. Channels don't compete for a single
//! winning action (a commander harvests, trains, builds, and fights in
//! the same breath); the budget is what keeps them honest with each
//! other.
//!
//! The army channel is the anti-trickle core: fighters are drafted into
//! a staging army every think and the army is only ever committed as a
//! body, once it reaches the size the dials demand. Everything after the
//! push — contact, the withdraw call, pullbacks — belongs to the
//! [`super::Executive`].
//!
//! Deterministic given (dials, observation, executive): every selection
//! orders by an explicit key ending in an id or (y, x).

use super::executive::{Army, ArmyState, Intent};
use super::observation::{Observation, UnitObs};
use crate::ids::UnitId;
use crate::stats::{BuildingKind, Domain, UnitKind};
use chassis::grid::TilePos;

mod combat;
mod construction;
mod economy;
mod terrain;

/// How far from home an enemy unit counts as an intruder (Chebyshev).
const DEFENSE_RADIUS: i32 = 8;
/// Ticks between scout refreshes toward a known enemy base, and the
/// window inside which that intel still counts as fresh.
const SCOUT_REFRESH: u64 = 1800;
/// Most turrets the policy will pay for in answer to raids.
const TURRET_CAP: usize = 2;
/// Scrap kept banked past a Fabricator's price before teching — the
/// fighting reserve that keeps the sentinel drip alive.
const TECH_RESERVE: u32 = 70;
/// Most flak turrets the policy will pay for against an air threat.
const FLAK_CAP: usize = 2;
/// Most Reclaimers the policy will run at once.
const RECLAIMER_CAP: usize = 3;
/// Ground-attack wings gathered before an air raid launches.
const AIR_WING: usize = 3;
/// How far around home the policy counts remaining salvage (Chebyshev)
/// when judging whether the patches are running dry.
const HOME_SALVAGE_RADIUS: i32 = 14;
/// Below this much known salvage near home, Reclaimers earn their keep.
const SALVAGE_LOW: u32 = 250;
/// Known anti-air within this range of a raid target scrubs the raid.
const RAID_AA_RADIUS: i32 = 6;
/// A salvage field farther than this (Chebyshev) from every own
/// Foundry counts as an unserved frontier worth an expansion.
const EXPANSION_RADIUS: i32 = 12;
/// Idle ground fighters gathered before the ferry loads a lift.
const FERRY_SQUAD: usize = 3;
/// Most Scuttle Charges the lane-mining arm keeps in the ground.
const MINE_CAP: usize = 3;
/// How far out from home (per axis) the mining arm centers its field
/// along the approach.
const MINE_LEAN: i32 = 5;
/// Static assets the bot may liquidate when its economy is exhausted,
/// ordered from least to most strategically costly.
const SALVAGE_PRIORITY: [BuildingKind; 6] = [
    BuildingKind::Turret,
    BuildingKind::FlakTurret,
    BuildingKind::Array,
    BuildingKind::Bastion,
    BuildingKind::Reclaimer,
    BuildingKind::RepairBay,
];

/// The policy's tunable considerations. The fairness rule is that
/// dials change *thinking* — never income, vision, or combat math.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dials {
    /// Think every N ticks.
    pub cadence: u64,
    /// Harvesters wanted alive or queued.
    pub harvester_target: u32,
    /// Fighters gathered before an army is committed.
    pub army_size: u32,
    /// Build a Fabricator and use the advanced roster.
    pub tech: bool,
    /// Answer harvester raids with turrets.
    pub turret_response: bool,
    /// Keep a scout sweeping the map (pointless without fog-honesty).
    pub scouting: bool,
    /// Observe through own vision instead of omnisciently.
    pub fog_honest: bool,
    /// Answer air threats: anti-air crawlers and flak turrets.
    pub aa_response: bool,
    /// Raise an Array once teched — the eyes for blips and long guns.
    pub radar: bool,
    /// Build Reclaimers when the patches near home run dry.
    pub reclaimers: bool,
    /// Weld wounded buildings instead of watching them rust.
    pub repair: bool,
    /// Fly ground-attack wings at the enemy economy.
    pub air_harass: bool,
    /// Liquidate static defense when the war outlives the economy.
    pub salvage: bool,
    /// Climb the full tree: Airworks after the Fabricator, Crucible
    /// after that, and tier-three metal once the Crucible stands.
    pub deep_tech: bool,
    /// Restore derelict Extractor frames when known and affordable.
    pub extractors: bool,
    /// Lift Reclaimers and Turrets one rung when the bank runs rich.
    pub upgrades: bool,
    /// Raise expansion Foundries toward unserved salvage frontiers.
    pub expansion: bool,
    /// Run a Skyhook shuttle at a known enemy base no ground route
    /// reaches: buy the lifter, load a squad, drop it on their shore.
    pub ferry: bool,
    /// Bury Scuttle Charges along the ground approach once raided or
    /// once the enemy's road home is known.
    pub mines: bool,
}

impl Dials {
    /// The player-facing rules-based opponent. Keep this as its own
    /// literal so later balance work can tune the opponent without
    /// changing the Overseer QA anchor.
    pub fn balanced() -> Self {
        Self {
            cadence: 8,
            harvester_target: 5,
            army_size: 5,
            tech: true,
            turret_response: true,
            scouting: true,
            fog_honest: true,
            aa_response: true,
            radar: true,
            reclaimers: true,
            repair: true,
            air_harass: true,
            salvage: true,
            deep_tech: true,
            extractors: true,
            upgrades: true,
            expansion: true,
            ferry: true,
            mines: true,
        }
    }

    /// The core channel set used by focused policy tests. Later strategic
    /// channels stay off so each test can enable them deliberately.
    pub fn full() -> Self {
        Self {
            cadence: 8,
            harvester_target: 4,
            army_size: 5,
            tech: true,
            turret_response: true,
            scouting: true,
            fog_honest: true,
            aa_response: true,
            radar: true,
            reclaimers: true,
            repair: true,
            air_harass: true,
            salvage: true,
            deep_tech: false,
            extractors: false,
            upgrades: false,
            expansion: false,
            ferry: false,
            mines: false,
        }
    }

    /// The stable QA controller's full strategic surface: deep tech,
    /// Extractors, upgrades, expansions, transports, and mines.
    pub fn overseer() -> Self {
        Self {
            deep_tech: true,
            extractors: true,
            upgrades: true,
            expansion: true,
            ferry: true,
            mines: true,
            harvester_target: 5,
            ..Self::full()
        }
    }
}

/// Channel-based scripted policy. Its memory is bot-local and legitimate
/// (a bot is a command source, not sim state): harvest blacklists, raid
/// memory, and the scout rotation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UtilityPolicy {
    /// Harvest assignments from the last think: worker, node, and where
    /// the worker stood when sent. A unit idle again right after being
    /// sent AND still standing where it started bounced off an
    /// unreachable node; an idle unit that moved (or was re-tasked by
    /// the scout press mid-walk) proves nothing about the node.
    last_sent: Vec<(UnitId, TilePos, TilePos)>,
    /// Nodes that bounced a harvester back.
    dead_nodes: Vec<TilePos>,
    /// Harvester count at the last think; a drop means raiders.
    harvesters_seen: usize,
    /// Bank reading at the last think and the last tick it grew — the
    /// starvation clock behind the desperation endgame. A bank that has
    /// not grown in eighty seconds is a dead economy whatever its
    /// level: rich seats freeze too, hoarding a reserve no income will
    /// ever top up.
    bank_seen: u32,
    bank_grew_at: u64,
    desperate: bool,
    /// Under desperation, two different route questions about home's
    /// mirror — the blind guess at the enemy base a symmetric quarry
    /// offers. Marching may trust optimism (`desperate_march`, the
    /// flood where unexplored counts open): a blind march explores,
    /// and on a connected map it finds the war. Liquidating the
    /// capital fund must not (`desperate_road`, walked tiles only):
    /// optimism survives any unexplored gulf forever, and a seat that
    /// releases its savings on that hope buys infantry against a
    /// strait until the map dies. No known road means island war —
    /// protect the fund and climb to the sky.
    desperate_march: bool,
    desperate_road: bool,
    /// Set when a harvester died on this watch; cleared when a turret
    /// stands (not when the command is emitted — commands can bounce).
    raided: bool,
    /// Turret count at the last think.
    turrets_seen: usize,
    /// Build sites requested last think, by anchor — one that never
    /// appeared was rejected by ground truth the observation lacks
    /// (an unseen unit in the footprint, say); blacklist the anchor.
    pending_sites: Vec<TilePos>,
    /// Anchors the sim refused.
    dead_anchors: Vec<TilePos>,
    /// The designated scout, held only mid-sweep (released between
    /// sweeps so the draft can have it back).
    scout: Option<UnitId>,
    /// Which leg of the search sweep the scout is on.
    scout_leg: u32,
    /// Tick of the last intel refresh toward a known enemy base.
    scouted_at: u64,
    /// Whether enemy air has ever been sighted — the sky stays suspect
    /// afterward.
    seen_air: bool,
    /// Riders sent to board the ferry on its last Load — still walking
    /// until they vanish into the sling (aboard), die, or go idle again
    /// (bounced). The lift waits for this ledger to drain.
    ferry_boarding: Vec<UnitId>,
}

impl UtilityPolicy {
    /// Fresh policy, no memory.
    pub fn new() -> Self {
        Self::default()
    }

    /// One think: intents for this observation, in lowering order
    /// (economy, production, construction, scouting, army — scouts are
    /// claimed before the draft can grab them). `armies` and `enlisted`
    /// are the executive's bookkeeping, pre-oriented by the caller when
    /// the policy thinks in flipped space.
    pub fn think(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        armies: &[Army],
        enlisted: &[UnitId],
    ) -> Vec<Intent> {
        let mut intents = Vec::new();
        if obs.scrap > self.bank_seen || obs.tick == 0 {
            self.bank_grew_at = obs.tick;
        }
        self.bank_seen = obs.scrap;
        // The clock must undercut the liveness gate's stall patience
        // (roughly two thousand ticks): desperation is the designed
        // answer to an economic freeze, so it has to fire before the
        // freeze detector calls the game dead between pushes.
        self.desperate = obs.tick.saturating_sub(self.bank_grew_at) > 1_600;
        let mut budget = obs.scrap;

        let Some(home) = obs
            .my_buildings
            .iter()
            .filter(|b| b.kind == BuildingKind::Foundry && b.built)
            .min_by_key(|b| b.id)
        else {
            return intents; // eliminated: nothing left to decide
        };
        let home_tile = home.anchor;
        let mirror_site = TilePos::new(
            obs.map_width - 1 - home_tile.x,
            obs.map_height - 1 - home_tile.y,
        );
        if self.desperate {
            self.desperate_march = Self::ground_reaches(obs, home_tile, mirror_site);
            self.desperate_road = Self::ground_route_known(obs, home_tile, mirror_site);
        }

        self.audit_harvests(obs);
        self.audit_sites(obs);
        self.audit_raids(obs);
        if obs
            .enemy_units
            .iter()
            .any(|u| u.kind.stats().domain == crate::stats::Domain::Air)
        {
            self.seen_air = true;
        }

        self.economy(obs, home_tile, &mut intents);
        self.production(dials, obs, home_tile, &mut budget, &mut intents);
        self.construction(dials, obs, home_tile, &mut budget, &mut intents);
        self.repairs(dials, obs, &mut budget, &mut intents);
        self.salvage(dials, obs, &mut intents);
        // No scouting while the economy is short-handed: pulling one of
        // three starting harvesters off the line buys intel with the
        // opening — the most expensive scrap there is.
        let harvesters = obs
            .my_units
            .iter()
            .filter(|u| u.kind.stats().harvest.is_some())
            .count();
        if dials.scouting && harvesters >= dials.harvester_target as usize {
            self.scouting(obs, home_tile, enlisted, &mut intents);
        }
        // The ferry gathers before the army channel so its Load claims
        // riders ahead of the draft (intents lower in order).
        self.ferry(dials, obs, armies, home_tile, enlisted, &mut intents);
        self.army(dials, obs, armies, home_tile, &mut intents);
        self.air_raid(dials, obs, home_tile, enlisted, &mut intents);
        intents
    }

    /// A harvester sent last think and idle again now bounced off an
    /// unreachable node — never ask twice. Only a node still reporting
    /// value earns the blacklist: a source the harvester honestly
    /// drained reads as empty and needs no entry (the amount filter
    /// already refuses it), and blacklisting it would poison the tile
    /// against every future deposit landing there.
    fn audit_harvests(&mut self, obs: &Observation) {
        for (id, node, sent_from) in std::mem::take(&mut self.last_sent) {
            // Collision separation can nudge a routeless worker one tile
            // from its send point, so exact equality misses a bounce.
            let bounced = obs
                .my_units
                .iter()
                .any(|u| u.id == id && u.idle && u.hp > 0 && u.tile.chebyshev(sent_from) <= 1);
            let still_reports = obs
                .known_scrap
                .iter()
                .chain(obs.known_wrecks.iter())
                .any(|(pos, amount)| *pos == node && *amount > 0);
            if bounced && still_reports && !self.dead_nodes.contains(&node) {
                self.dead_nodes.push(node);
            }
        }
    }

    /// A site requested last think that never appeared was refused for a
    /// reason the observation can't see; stop asking for that anchor.
    /// A pending deferred found is a site on its way, not a refusal:
    /// the founder pays on arrival, so while one is still walking the
    /// anchor stays pending for a later audit to judge (blacklisting
    /// it would poison ground the claim is about to prove). The
    /// scripted `Brain` never defers — `founding` is always `None` on
    /// its path.
    fn audit_sites(&mut self, obs: &Observation) {
        for anchor in std::mem::take(&mut self.pending_sites) {
            let appeared = obs.my_buildings.iter().any(|b| b.anchor == anchor);
            if appeared {
                continue;
            }
            let walking = obs
                .my_units
                .iter()
                .any(|u| u.founding.is_some_and(|(_, a)| a == anchor));
            if walking {
                self.pending_sites.push(anchor);
            } else if !self.dead_anchors.contains(&anchor) {
                self.dead_anchors.push(anchor);
            }
        }
    }

    /// A shrinking harvest line means raiders; remember until a turret
    /// actually stands.
    fn audit_raids(&mut self, obs: &Observation) {
        let harvesters = obs
            .my_units
            .iter()
            .filter(|u| u.kind.stats().harvest.is_some())
            .count();
        if harvesters < self.harvesters_seen {
            self.raided = true;
        }
        self.harvesters_seen = harvesters;
        let turrets = obs
            .my_buildings
            .iter()
            .filter(|b| b.kind == BuildingKind::Turret)
            .count();
        if turrets > self.turrets_seen {
            self.raided = false;
        }
        self.turrets_seen = turrets;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::observation::{BuildingObs, OBSERVATION_VERSION, Observation, UnitObs};
    use crate::ids::{BuildingId, PlayerId, UnitId};

    fn obs_with(units: Vec<UnitObs>) -> Observation {
        Observation {
            version: OBSERVATION_VERSION,
            tick: 0,
            me: PlayerId(0),
            scrap: 0,
            map_width: 32,
            map_height: 20,
            my_units: units,
            my_buildings: Vec::new(),
            my_queues: Vec::new(),
            ally_units: Vec::new(),
            ally_buildings: Vec::new(),
            enemy_units: Vec::new(),
            enemy_buildings: Vec::new(),
            explored: vec![true; 32 * 20],
            known_scrap: Vec::new(),
            known_rock: Vec::new(),
            known_frames: Vec::new(),
            known_peaks: Vec::new(),
            known_wrecks: Vec::new(),
            blips: Vec::new(),
            faction: crate::state::Faction::Ferrous,
            my_shells: 0,
            incoming_shells: Vec::new(),
        }
    }

    fn harvester(id: u32, founding: Option<(BuildingKind, TilePos)>) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(0),
            kind: UnitKind::Harvester,
            tile: TilePos::new(5, 5),
            hp: UnitKind::Harvester.stats().max_hp,
            idle: founding.is_none(),
            carrying: 0,
            cargo: 0,
            site: None,
            salvaging: None,
            founding,
        }
    }

    /// The site audit's deferred-found contract (fog placement Part
    /// B): an anchor whose founder is still walking is a site on its
    /// way — kept pending, never blacklisted — while the same anchor
    /// with no founder and no building is a refusal and earns the
    /// blacklist as it always did.
    #[test]
    fn a_walking_founder_defers_the_site_audits_verdict() {
        let anchor = TilePos::new(9, 4);

        let mut policy = UtilityPolicy::new();
        policy.pending_sites.push(anchor);
        let walking = obs_with(vec![harvester(0, Some((BuildingKind::Turret, anchor)))]);
        policy.audit_sites(&walking);
        assert!(
            policy.dead_anchors.is_empty(),
            "the audit blacklisted an anchor whose founder is still walking"
        );
        assert_eq!(
            policy.pending_sites,
            vec![anchor],
            "a walking claim's anchor must stay pending for a later audit"
        );

        let mut policy = UtilityPolicy::new();
        policy.pending_sites.push(anchor);
        let refused = obs_with(vec![harvester(0, None)]);
        policy.audit_sites(&refused);
        assert_eq!(
            policy.dead_anchors,
            vec![anchor],
            "with no founder and no building, the anchor was refused and \
             must be blacklisted exactly as before"
        );
        assert!(policy.pending_sites.is_empty());
    }

    /// A founder walking toward one anchor must not shield a different
    /// pending anchor from the audit.
    #[test]
    fn the_founder_shields_only_its_own_anchor() {
        let claimed = TilePos::new(9, 4);
        let refused = TilePos::new(15, 8);
        let mut policy = UtilityPolicy::new();
        policy.pending_sites.push(claimed);
        policy.pending_sites.push(refused);
        let obs = obs_with(vec![harvester(0, Some((BuildingKind::Turret, claimed)))]);
        policy.audit_sites(&obs);
        assert_eq!(policy.pending_sites, vec![claimed]);
        assert_eq!(policy.dead_anchors, vec![refused]);
    }

    #[test]
    fn an_automatic_upgrade_is_not_an_orphaned_site() {
        let mut obs = obs_with(Vec::new());
        let anchor = TilePos::new(9, 4);
        obs.my_buildings.push(BuildingObs {
            id: BuildingId(7),
            player: PlayerId(0),
            kind: BuildingKind::Turret,
            anchor,
            hp: 100,
            built: false,
            seen: true,
            tier: 1,
        });
        obs.my_queues.push(Vec::new());
        let mut policy = UtilityPolicy::new();
        let mut budget = 0;
        let mut intents = Vec::new();

        policy.construction(
            &Dials::full(),
            &obs,
            TilePos::new(2, 2),
            &mut budget,
            &mut intents,
        );

        assert!(
            intents.iter().all(
                |intent| !matches!(intent, Intent::Build { anchor: site, .. } if *site == anchor)
            ),
            "a self-timed upgrade must not draft an orphan-relief worker"
        );
    }
}
