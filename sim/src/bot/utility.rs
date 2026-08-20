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
use serde::{Deserialize, Serialize};

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

/// The policy's tunable considerations. The fairness rule is that
/// dials change *thinking* — never income, vision, or combat math.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Climb the 0.15 tree: Airworks after the Fabricator, Crucible
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
    /// Everything the 0.14-era commander had on. The 0.15 channels
    /// stay off here: gym parity fixtures were measured against this
    /// exact commander, and their behavior must not drift underneath
    /// them.
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

    /// The Overseer: the full commander with the 0.15 tree switched
    /// on — deep tech, extractor restoration, and tier upgrades.
    /// Training bootstrap and QA yardstick only, never player-facing.
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
/// Caller-supplied search aids for the scouting routine: units
/// explicitly released for scout duty (the gym offers its staged army)
/// and pre-oriented start anchors — public map knowledge of where
/// enemy bases began. The scripted Brain passes both empty and keeps
/// its historical behavior byte for byte.
#[derive(Clone, Copy)]
pub struct ScoutAids<'a> {
    /// Enlisted units released for scout duty.
    pub extra: &'a [UnitId],
    /// Pre-oriented enemy start anchors.
    pub anchors: &'a [TilePos],
    /// Whether ground machines may join the search: false when no
    /// birthplace is reachable on known terrain — a sealed map's
    /// search belongs to the air while the ground waits for the ferry.
    pub ground_may_search: bool,
    /// Whether ground searchers step to the exploration frontier
    /// instead of taking cross-dark targets raw (the gym path; the
    /// scripted Brain keeps its historical dispatch byte for byte).
    pub frontier_step: bool,
}

impl Default for ScoutAids<'_> {
    fn default() -> Self {
        Self {
            frontier_step: false,
            extra: &[],
            anchors: &[],
            ground_may_search: true,
        }
    }
}

/// Channel-based scripted policy. Its memory is bot-local and legitimate
/// (a bot is a command source, not sim state): harvest blacklists, raid
/// memory, and the scout rotation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The starvation prospector (neural chore only): one harvester
    /// walking the sweep legs because nothing harvestable is known.
    /// Released the moment the economy channel can feed the line.
    prospector: Option<UnitId>,
    /// Which leg of the prospecting sweep is next.
    prospect_leg: u32,
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

        self.economy(obs, home_tile, false, &mut intents);
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
            self.scouting(
                obs,
                home_tile,
                enlisted,
                ScoutAids::default(),
                false,
                &mut intents,
            );
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
    pub(super) fn audit_harvests(&mut self, obs: &Observation) {
        for (id, node, sent_from) in std::mem::take(&mut self.last_sent) {
            // Within one tile of the send point: collision separation
            // nudges a routeless worker off its exact tile, and an
            // equality test let the same unreachable node be retried
            // forever (measured as 98 then 227 NoRoute stalls in the
            // first two windows of one seat's game).
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

    /// Where a GROUND searcher extends the light: the explored tile
    /// bordering unexplored ground (the exploration frontier) closest
    /// to the searcher's assigned target.
    /// Arriving there lights what lies beyond, the ring recedes, and the
    /// next press plans from the new edge — a sweep that needs no route
    /// through the dark at all. Raw cross-dark targets were measured as
    /// ~480 UnreachableGoal rejections per digest window on archipelago
    /// maps, and a walk-the-lit-prefix variant parked every searcher on
    /// one coastal tile forever (exploration frozen at 9%). `None` once
    /// nothing known borders darkness — the lit world is swept, and the
    /// caller keeps its original target.
    /// The set of tiles walkable from `from` over KNOWN terrain
    /// (unexplored reads open, known rock closed) — one breadth-first
    /// flood, indexed `y * width + x`. The route truth every dispatcher
    /// that names a ground destination must consult: manhattan-nearest
    /// picks across explored walls stall forever.
    pub(super) fn reachable_component(&self, obs: &Observation, from: TilePos) -> Vec<bool> {
        let (w, h) = (obs.map_width, obs.map_height);
        let index = |t: TilePos| (t.y * w + t.x) as usize;
        let mut reachable = vec![false; (w * h).max(0) as usize];
        if from.x >= 0 && from.y >= 0 && from.x < w && from.y < h {
            let mut queue = std::collections::VecDeque::new();
            reachable[index(from)] = true;
            queue.push_back(from);
            while let Some(tile) = queue.pop_front() {
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let n = tile.offset(dx, dy);
                    if n.x >= 0
                        && n.y >= 0
                        && n.x < w
                        && n.y < h
                        && !reachable[index(n)]
                        && !self.rock_at(obs, n)
                    {
                        reachable[index(n)] = true;
                        queue.push_back(n);
                    }
                }
            }
        }
        reachable
    }

    fn ground_frontier_toward(
        &self,
        obs: &Observation,
        from: TilePos,
        toward: TilePos,
    ) -> Option<TilePos> {
        let (w, h) = (obs.map_width, obs.map_height);
        let index = |t: TilePos| (t.y * w + t.x) as usize;
        let explored = |t: TilePos| {
            t.x >= 0
                && t.y >= 0
                && t.x < w
                && t.y < h
                && obs.explored.get(index(t)).copied().unwrap_or(false)
        };
        // The searcher's reachable component over KNOWN terrain: a ring
        // tile across an explored bench wall is manhattan-near and
        // walk-impossible — the same missing-route-check disease this
        // sweep was built to cure.
        let reachable = self.reachable_component(obs, from);
        let mut best: Option<(i32, i32, i32, i32)> = None;
        for y in 0..h {
            for x in 0..w {
                let tile = TilePos::new(x, y);
                if !explored(tile) || self.rock_at(obs, tile) || !reachable[index(tile)] {
                    continue;
                }
                let borders_dark = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| {
                    let n = tile.offset(*dx, *dy);
                    n.x >= 0 && n.y >= 0 && n.x < w && n.y < h && !explored(n)
                });
                if !borders_dark {
                    continue;
                }
                // Standing on the edge already; this tile cannot extend
                // the light for THIS searcher.
                if tile == from {
                    continue;
                }
                // Directed sweep: the ring tile closest to the ASSIGNED
                // target wins, the searcher's own distance breaks ties.
                // A nearest-to-self pick swept the home region forever
                // and never carried the search toward the enemy
                // quadrant (measured as two broad-front seats fighting
                // fifty minutes without ever seeing a foundry).
                let key = (tile.manhattan(toward), tile.manhattan(from), tile.y, tile.x);
                if best.is_none_or(|current| key < current) {
                    best = Some(key);
                }
            }
        }
        best.map(|(_, _, y, x)| TilePos::new(x, y))
    }

    /// Retains only the harvest assignments the executive actually
    /// emitted this think: an assignment the lowering silently refused
    /// (worker enlisted or claimed elsewhere) must never reach the
    /// bounce audit — the worker reads idle next think and a live node
    /// would join the permanent blacklist for a refusal that had
    /// nothing to do with the node.
    pub(super) fn confirm_harvest_dispatches(&mut self, commands: &[crate::PlayerCommand]) {
        self.last_sent.retain(|(id, ..)| {
            commands.iter().any(|command| match &command.command {
                crate::Command::Harvest { units, .. } => units.contains(id),
                _ => false,
            })
        });
    }

    /// A site requested last think that never appeared was refused for a
    /// reason the observation can't see; stop asking for that anchor.
    /// A pending deferred found is a site on its way, not a refusal:
    /// the founder pays on arrival, so while one is still walking the
    /// anchor stays pending for a later audit to judge (blacklisting
    /// it would poison ground the claim is about to prove). The
    /// scripted `Brain` never defers — `founding` is always `None` on
    /// its path.
    pub(super) fn audit_sites(&mut self, obs: &Observation) {
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
    pub(super) fn audit_raids(&mut self, obs: &Observation) {
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

    /// Economy channel: idle harvesters back to work on the nearest
    /// known node that hasn't bounced anyone. A node only qualifies if
    /// it sits no deeper in their half than ours — a returning scout
    /// must not be "efficiently" assigned to mine at the enemy's
    /// doorstep.
    /// `route_check` (the gym path only; the scripted Brain keeps its
    /// historical dispatch byte for byte) filters candidate nodes to the
    /// component walkable from home — a manhattan-nearest node across a
    /// bench wall stalls the walk, and collision nudges defeated the
    /// bounce audit for 98 then 227 stalls in one seat's opening.
    pub(super) fn economy(
        &mut self,
        obs: &Observation,
        home: TilePos,
        route_check: bool,
        intents: &mut Vec<Intent>,
    ) {
        let reach = route_check.then(|| self.reachable_component(obs, home));
        let (w, _h) = (obs.map_width, obs.map_height);
        let enemy_base = obs
            .enemy_buildings
            .iter()
            .map(|b| (b.anchor.manhattan(home), b.anchor.y, b.anchor.x))
            .min()
            .map(|(_, y, x)| TilePos::new(x, y));
        for u in obs
            .my_units
            .iter()
            .filter(|u| u.kind.stats().harvest.is_some() && u.idle && Some(u.id) != self.scout)
        {
            let node = obs
                .known_scrap
                .iter()
                .chain(obs.known_wrecks.iter())
                .filter(|(pos, amount)| {
                    *amount > 0
                        && !self.dead_nodes.contains(pos)
                        && reach.as_ref().is_none_or(|r| {
                            r.get((pos.y * w + pos.x) as usize)
                                .copied()
                                .unwrap_or(false)
                        })
                        && enemy_base.is_none_or(|eb| pos.manhattan(home) <= pos.manhattan(eb))
                })
                .map(|(pos, _)| (pos.manhattan(u.tile), pos.y, pos.x))
                .min()
                .map(|(_, y, x)| TilePos::new(x, y));
            if let Some(node) = node {
                intents.push(Intent::AssignHarvest { unit: u.id, node });
                self.last_sent.push((u.id, node, u.tile));
            }
        }
    }

    /// Starvation ladder — the neural bot's chore only; the scripted
    /// `Brain` never climbs it. Runs after [`Self::economy`]: any
    /// harvester still idle found no qualifying node. Rung 1 re-tries
    /// with the enemy-side rule dropped — a dry home half makes
    /// contested nodes acceptable. Rung 2, nothing known at all: one
    /// machine (lowest id) walks the sweep legs so far scrap can enter
    /// `known_scrap`. Deliberately disjoint from the scout machinery:
    /// prospecting never stamps `scouted_at` (that feeds the trained
    /// `intel_age` feature) and never claims `self.scout`.
    pub(super) fn prospect(
        &mut self,
        obs: &Observation,
        spoken_for: &[UnitId],
        intents: &mut Vec<Intent>,
    ) {
        // Economy's assignments this think are still in the ledger
        // (the audits drained it at the think's start).
        let assigned: Vec<UnitId> = self.last_sent.iter().map(|(id, ..)| *id).collect();
        let starved: Vec<(UnitId, TilePos)> = obs
            .my_units
            .iter()
            .filter(|u| {
                u.kind.stats().harvest.is_some()
                    && u.idle
                    && Some(u.id) != self.scout
                    && !assigned.contains(&u.id)
                    && !spoken_for.contains(&u.id)
            })
            .map(|u| (u.id, u.tile))
            .collect();
        if starved.is_empty() {
            // The line is fed; the claim dissolves. A still-walking
            // ex-prospector goes idle at its leg's end and the economy
            // channel hires it back like any other harvester.
            self.prospector = None;
            return;
        }

        // Rung 1: any known node, enemy-side rule dropped.
        let mut still_starved: Vec<UnitId> = Vec::new();
        for &(id, tile) in &starved {
            // Route truth for the starved line too: manhattan-nearest
            // across a known wall re-dispatches and re-stalls the same
            // worker at the same node every think. Nodes outside the
            // worker's own walkable component fall through to rung 2.
            let reach = self.reachable_component(obs, tile);
            let index = |pos: TilePos| (pos.y * obs.map_width + pos.x) as usize;
            let node = obs
                .known_scrap
                .iter()
                .chain(obs.known_wrecks.iter())
                .filter(|(pos, amount)| {
                    *amount > 0 && !self.dead_nodes.contains(pos) && reach[index(*pos)]
                })
                .map(|(pos, _)| (pos.manhattan(tile), pos.y, pos.x))
                .min()
                .map(|(_, y, x)| TilePos::new(x, y));
            match node {
                Some(node) => {
                    intents.push(Intent::AssignHarvest { unit: id, node });
                    if let Some(at) = obs.my_units.iter().find(|u| u.id == id).map(|u| u.tile) {
                        self.last_sent.push((id, node, at));
                    }
                }
                None => still_starved.push(id),
            }
        }

        // Rung 2: nothing known anywhere. One prospector is enough;
        // the rest of the line waits for its news.
        if let Some(id) = self.prospector
            && !obs.my_units.iter().any(|u| u.id == id && u.hp > 0)
        {
            self.prospector = None; // died prospecting
        }
        if let Some(id) = self.prospector {
            if !still_starved.contains(&id) {
                return; // mid-leg, still walking
            }
        } else {
            self.prospector = still_starved.iter().min().copied();
        }
        let Some(unit) = self.prospector else {
            return;
        };
        // The sweep hunts scrap, not intel: the centre first, then the
        // corners — never a deliberate walk into the enemy base.
        let (w, h) = (obs.map_width, obs.map_height);
        let legs = [
            TilePos::new(w / 2, h / 2),
            TilePos::new(3, 3),
            TilePos::new(w - 4, 3),
            TilePos::new(3, h - 4),
            TilePos::new(w - 4, h - 4),
        ];
        let to = legs[self.prospect_leg as usize % legs.len()];
        self.prospect_leg += 1;
        let to = self.passable_near(obs, to);
        // A ground prospector on a partitioned map cannot take raw legs:
        // it steps to the frontier toward the leg exactly like the
        // search party, or the dispatch bounces forever. (This chore is
        // the neural bot's only — the scripted Brain never climbs the
        // ladder — so no scripted-path gate is needed.)
        let to = match obs.my_units.iter().find(|u| u.id == unit) {
            Some(u) if u.kind.stats().domain != Domain::Air => {
                // No reachable frontier left: the lit component is fully
                // swept, and the raw corner leg is exactly the unroutable
                // target this machinery exists to avoid — one prospector
                // was measured cycling five corner legs 1,722 times over
                // 41 minutes without moving. Stand down instead.
                match self.ground_frontier_toward(obs, u.tile, to) {
                    Some(frontier) => frontier,
                    None => {
                        self.prospector = None;
                        return;
                    }
                }
            }
            _ => to,
        };
        intents.push(Intent::Scout { unit, to });
    }

    /// Production channel: harvesters to target, then a sentinel drip
    /// from the Foundry; counters and lancers from the Fabricator. The
    /// unbounded drip arms respect [`Self::capital_reserve`] so the
    /// tech fund
    /// can actually accumulate — without that discipline the drip eats
    /// every think's income and the brain never techs at all (the
    /// contested-play starvation the all-Overseer sweeps exposed).
    fn production(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        home: TilePos,
        budget: &mut u32,
        intents: &mut Vec<Intent>,
    ) {
        let queued = |kind: UnitKind| -> usize {
            obs.my_queues
                .iter()
                .flat_map(|q| q.iter())
                .filter(|k| **k == kind)
                .count()
        };
        let alive =
            |kind: UnitKind| -> usize { obs.my_units.iter().filter(|u| u.kind == kind).count() };
        let harvesters = alive(UnitKind::Harvester) + queued(UnitKind::Harvester);
        // Survival outranks saving: with the home screen thin, the drip
        // spends freely — a banked Fabricator is worthless underneath a
        // Sentinel rush.
        let screen = obs
            .my_units
            .iter()
            .filter(|u| {
                let stats = u.kind.stats();
                stats.domain == Domain::Ground && stats.can_fight()
            })
            .count();
        // A desperate economy with a road to march releases the capital
        // fund: saving for the next tech rung is saving for a purchase
        // no income will ever complete, while the freed bank buys the
        // bodies that end the game now. Island desperation keeps the
        // fund — with no ground road, the tech chain to the sky is the
        // only road left, and spending its savings on infantry is how
        // forty-seven fighters end up staring at a gulf forever.
        let capital = if screen < 3 || (self.desperate && self.desperate_road) {
            0
        } else {
            Self::capital_reserve(dials, obs)
        };

        // The ferry fund: with a built Airworks, a known island target,
        // a squad worth lifting, and no lifter, the Skyhook's price is
        // banked ahead of every other military purchase — the wing and
        // AA arms otherwise skim the bank at their own smaller reserves
        // forever and the lifter never arrives (the Severance probe's
        // exact stall). Bought the moment the Airworks has room; the
        // hold ends with the purchase. The squad gate keeps the order
        // right and the seat alive: a lifter without riders is dead
        // capital, so while the last squad lies dead on the far shore
        // the fund stands down and the drip rebuilds fighters first.
        if dials.ferry
            && screen >= FERRY_SQUAD
            && alive(UnitKind::Skyhook) + queued(UnitKind::Skyhook) < 1
        {
            let airworks = obs
                .my_buildings
                .iter()
                .enumerate()
                .filter(|(_, b)| b.kind == BuildingKind::Airworks && b.built)
                .min_by_key(|(_, b)| b.id);
            if let Some((qi, airworks)) = airworks
                && (Self::island_target(obs, home).is_some()
                    || (self.desperate && !self.desperate_road))
            {
                let price = UnitKind::Skyhook.stats().cost + TECH_RESERVE;
                if *budget >= price && obs.my_queues[qi].len() < 2 {
                    *budget -= UnitKind::Skyhook.stats().cost;
                    intents.push(Intent::TrainAt {
                        building: airworks.id,
                        kind: UnitKind::Skyhook,
                    });
                } else {
                    *budget = budget.saturating_sub(price);
                }
            }
        }

        // The Overseer's heavy metal: one Warden per think from the
        // Fabricator once it stands, and one Breaker whenever the
        // Crucible is idle and the bank can take it. Deliberately ahead
        // of the legacy drip so tier-two walks the field before another
        // sentinel does.
        if dials.deep_tech {
            let crucible = obs
                .my_buildings
                .iter()
                .enumerate()
                .filter(|(_, b)| b.kind == BuildingKind::Crucible && b.built)
                .min_by_key(|(_, b)| b.id);
            if let Some((qi, crucible)) = crucible
                && obs.my_queues[qi].is_empty()
                && alive(UnitKind::Breaker) + queued(UnitKind::Breaker) < 2
                && *budget >= UnitKind::Breaker.stats().cost + TECH_RESERVE
            {
                *budget -= UnitKind::Breaker.stats().cost;
                intents.push(Intent::TrainAt {
                    building: crucible.id,
                    kind: UnitKind::Breaker,
                });
            }
            let fabricator = obs
                .my_buildings
                .iter()
                .enumerate()
                .filter(|(_, b)| b.kind == BuildingKind::Fabricator && b.built)
                .min_by_key(|(_, b)| b.id);
            if let Some((qi, fabricator)) = fabricator
                && obs.my_queues[qi].len() < 2
                && alive(UnitKind::Warden) + queued(UnitKind::Warden) < 4
                && *budget >= UnitKind::Warden.stats().cost + UnitKind::Harvester.stats().cost
            {
                *budget -= UnitKind::Warden.stats().cost;
                intents.push(Intent::TrainAt {
                    building: fabricator.id,
                    kind: UnitKind::Warden,
                });
            }
            // Once the whole tree stands, a small bomber wing: the
            // payload that decides sieges — and island wars, where no
            // crawler ever crosses.
            {
                use crate::stats::Role;
                let bomber_kind = Role::Bomber.unit_for(obs.faction);
                let airworks = obs
                    .my_buildings
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| b.kind == BuildingKind::Airworks && b.built)
                    .min_by_key(|(_, b)| b.id);
                let crucible_stands = obs
                    .my_buildings
                    .iter()
                    .any(|b| b.kind == BuildingKind::Crucible && b.built);
                if let Some((qi, airworks)) = airworks
                    && crucible_stands
                    && obs.my_queues[qi].len() < 2
                    && alive(bomber_kind) + queued(bomber_kind) < 2
                    && *budget >= bomber_kind.stats().cost + TECH_RESERVE
                {
                    *budget -= bomber_kind.stats().cost;
                    intents.push(Intent::TrainAt {
                        building: airworks.id,
                        kind: bomber_kind,
                    });
                }
            }
        }

        let foundry = obs
            .my_buildings
            .iter()
            .enumerate()
            .filter(|(_, b)| b.kind == BuildingKind::Foundry && b.built)
            .min_by_key(|(_, b)| b.id);
        if let Some((qi, foundry)) = foundry
            && obs.my_queues[qi].len() < 2
        {
            if harvesters < dials.harvester_target as usize
                && *budget >= UnitKind::Harvester.stats().cost
            {
                *budget -= UnitKind::Harvester.stats().cost;
                intents.push(Intent::TrainAt {
                    building: foundry.id,
                    kind: UnitKind::Harvester,
                });
            } else if *budget >= UnitKind::Sentinel.stats().cost + capital {
                *budget -= UnitKind::Sentinel.stats().cost;
                intents.push(Intent::TrainAt {
                    building: foundry.id,
                    kind: UnitKind::Sentinel,
                });
            }
        }

        if !dials.tech {
            return;
        }
        let fabricator = obs
            .my_buildings
            .iter()
            .enumerate()
            .filter(|(_, b)| b.kind == BuildingKind::Fabricator && b.built)
            .min_by_key(|(_, b)| b.id);
        let enemy_turrets = obs
            .enemy_buildings
            .iter()
            .filter(|b| b.kind == BuildingKind::Turret && b.built)
            .count();
        let enemy_harvesters = obs
            .enemy_units
            .iter()
            .filter(|u| u.kind.stats().harvest.is_some())
            .count();
        if let Some((qi, fab)) = fabricator {
            use crate::stats::{Domain, Role};
            let fab_open = obs.my_queues[qi].len() < 2;
            let foundry_open = foundry.filter(|(fqi, _)| obs.my_queues[*fqi].len() < 2);
            let airworks_open = obs
                .my_buildings
                .iter()
                .enumerate()
                .filter(|(_, b)| b.kind == BuildingKind::Airworks && b.built)
                .min_by_key(|(_, b)| b.id)
                .filter(|(aqi, _)| obs.my_queues[*aqi].len() < 2);
            let aa_kind = Role::AntiAir.unit_for(obs.faction);
            let wing_kind = Role::AirGround.unit_for(obs.faction);
            let lancer = UnitKind::Lancer.stats().cost;
            let scuttler = UnitKind::Scuttler.stats().cost;
            let reserve = UnitKind::Sentinel.stats().cost;
            // The sky answers first: enemy air on the field (or ever
            // sighted) wants a dedicated gun per two known wings, before
            // any ground purchase.
            let enemy_air = obs
                .enemy_units
                .iter()
                .filter(|u| u.kind.stats().domain == Domain::Air)
                .count();
            let want_aa = if enemy_air > 0 {
                enemy_air.div_ceil(2) + 1
            } else {
                usize::from(self.seen_air)
            };
            if dials.aa_response
                && fab_open
                && alive(aa_kind) + queued(aa_kind) < want_aa
                && *budget >= aa_kind.stats().cost
            {
                *budget -= aa_kind.stats().cost;
                intents.push(Intent::TrainAt {
                    building: fab.id,
                    kind: aa_kind,
                });
            } else if fab_open
                && enemy_turrets > alive(UnitKind::Lancer) + queued(UnitKind::Lancer)
                && *budget >= lancer
            {
                *budget -= lancer;
                intents.push(Intent::TrainAt {
                    building: fab.id,
                    kind: UnitKind::Lancer,
                });
            } else if alive(UnitKind::Scuttler) < 4
                && enemy_harvesters >= 2
                && *budget >= scuttler + reserve
                && let Some((_, raid_bay)) = foundry_open
            {
                // The Scuttler homes at the Foundry on the closed tree.
                *budget -= scuttler;
                intents.push(Intent::TrainAt {
                    building: raid_bay.id,
                    kind: UnitKind::Scuttler,
                });
            } else if dials.air_harass
                && alive(wing_kind) + queued(wing_kind) < AIR_WING
                && (enemy_harvesters >= 2 || !obs.enemy_buildings.is_empty())
                && *budget >= wing_kind.stats().cost + reserve
                && let Some((_, airworks)) = airworks_open
            {
                // A wing for the harvest line — bought once raiding has
                // something to eat OR the enemy base is known at all
                // (on an island map the wing IS the reach), and only
                // from a standing Airworks.
                *budget -= wing_kind.stats().cost;
                intents.push(Intent::TrainAt {
                    building: airworks.id,
                    kind: wing_kind,
                });
            } else if fab_open && *budget >= lancer + reserve + capital {
                *budget -= lancer;
                intents.push(Intent::TrainAt {
                    building: fab.id,
                    kind: UnitKind::Lancer,
                });
            }
        }
    }

    /// Whether known ground connects `home` to any tile of the 2x2
    /// footprint anchored at `anchor`. BFS over tiles not known
    /// impassable (rock, mesa, pit — `known_rock` carries all three);
    /// unexplored tiles count open, the same optimism every founding
    /// walk uses. Runs only when a frame claim is otherwise ready, so
    /// the flood's cost is paid a handful of times per match.
    fn ground_reaches(obs: &Observation, home: TilePos, anchor: TilePos) -> bool {
        Self::ground_flood(obs, home, anchor, |t| !obs.known_rock_at(t))
    }

    /// Whether a ground road from `home` to `anchor` is actually KNOWN:
    /// the same flood, but unexplored tiles count blocked. This is the
    /// ferry's and the mining arm's route question — a base only ever
    /// seen from the sky is an island war until a walked road proves
    /// otherwise, and the optimistic flood above can wander through any
    /// unexplored gulf forever without ever proving severance.
    fn ground_route_known(obs: &Observation, home: TilePos, anchor: TilePos) -> bool {
        Self::ground_flood(obs, home, anchor, |t| {
            obs.explored(t) && !obs.known_rock_at(t)
        })
    }

    /// The shared reachability flood: BFS from `home` through tiles
    /// `enter` admits, looking for the 2x2 footprint at `anchor`.
    fn ground_flood(
        obs: &Observation,
        home: TilePos,
        anchor: TilePos,
        enter: impl Fn(TilePos) -> bool,
    ) -> bool {
        let (w, h) = (obs.map_width, obs.map_height);
        if w <= 0 || h <= 0 {
            return false;
        }
        let idx = |t: TilePos| (t.y * w + t.x) as usize;
        let target = |t: TilePos| {
            (anchor.x..anchor.x + 2).contains(&t.x) && (anchor.y..anchor.y + 2).contains(&t.y)
        };
        let in_bounds = |t: TilePos| t.x >= 0 && t.y >= 0 && t.x < w && t.y < h;
        if !in_bounds(home) {
            return false;
        }
        let mut seen = vec![false; (w * h) as usize];
        let mut open = std::collections::VecDeque::new();
        seen[idx(home)] = true;
        open.push_back(home);
        while let Some(t) = open.pop_front() {
            if target(t) {
                return true;
            }
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let n = t.offset(dx, dy);
                if in_bounds(n) && !seen[idx(n)] && enter(n) {
                    seen[idx(n)] = true;
                    open.push_back(n);
                }
            }
        }
        false
    }

    /// The nearest known enemy building no KNOWN ground road reaches —
    /// the island war's objective — or `None` while every known site
    /// has a walked road. Candidates are tried nearest-first by
    /// (manhattan, y, x). One flood of home's known-road component
    /// answers every candidate: per-site reachability from a fixed
    /// origin is component membership, and the per-site BFS this
    /// replaces re-walked the same component once per known enemy
    /// building on any connected map.
    fn island_target(obs: &Observation, home: TilePos) -> Option<TilePos> {
        let mut sites: Vec<(i32, i32, i32)> = obs
            .enemy_buildings
            .iter()
            .map(|b| (b.anchor.manhattan(home), b.anchor.y, b.anchor.x))
            .collect();
        sites.sort_unstable();
        if sites.is_empty() {
            return None;
        }
        let (w, h) = (obs.map_width, obs.map_height);
        let component =
            Self::ground_component(obs, home, |t| obs.explored(t) && !obs.known_rock_at(t));
        let footprint_reached = |anchor: TilePos| {
            component.as_ref().is_some_and(|seen| {
                (anchor.y..anchor.y + 2).any(|y| {
                    (anchor.x..anchor.x + 2).any(|x| {
                        (0..w).contains(&x) && (0..h).contains(&y) && seen[(y * w + x) as usize]
                    })
                })
            })
        };
        sites
            .into_iter()
            .map(|(_, y, x)| TilePos::new(x, y))
            .find(|anchor| !footprint_reached(*anchor))
    }

    /// Home's full walkable component under `enter`, as a seen-tile
    /// grid — the membership form of [`Self::ground_flood`], flooded to
    /// exhaustion. `None` when the map is degenerate or `home` is out
    /// of bounds, where the per-target flood reports nothing reachable.
    fn ground_component(
        obs: &Observation,
        home: TilePos,
        enter: impl Fn(TilePos) -> bool,
    ) -> Option<Vec<bool>> {
        let (w, h) = (obs.map_width, obs.map_height);
        if w <= 0 || h <= 0 || home.x < 0 || home.y < 0 || home.x >= w || home.y >= h {
            return None;
        }
        let idx = |t: TilePos| (t.y * w + t.x) as usize;
        let in_bounds = |t: TilePos| t.x >= 0 && t.y >= 0 && t.x < w && t.y < h;
        let mut seen = vec![false; (w * h) as usize];
        let mut open = std::collections::VecDeque::new();
        seen[idx(home)] = true;
        open.push_back(home);
        while let Some(t) = open.pop_front() {
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let n = t.offset(dx, dy);
                if in_bounds(n) && !seen[idx(n)] && enter(n) {
                    seen[idx(n)] = true;
                    open.push_back(n);
                }
            }
        }
        Some(seen)
    }

    /// A drop point beside the enemy base, from the target side's own
    /// known ground: the first ring-scanned tile ((r, y, x) order) that
    /// is not known rock, scrap, or a known building footprint —
    /// unexplored tiles count open, like every founding walk. The sim's
    /// unload scan handles exact placement around it; everything nearby
    /// known-blocked falls back to the anchor itself.
    fn unload_site(&self, obs: &Observation, target: TilePos) -> TilePos {
        for r in 2i32..=6 {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dy.abs()) != r {
                        continue;
                    }
                    let t = target.offset(dx, dy);
                    let in_bounds =
                        t.x >= 0 && t.y >= 0 && t.x < obs.map_width && t.y < obs.map_height;
                    if in_bounds && self.tile_open(obs, t) {
                        return t;
                    }
                }
            }
        }
        target
    }

    /// Ferry channel (dial-gated, Overseer only): when the known enemy
    /// base sits across ground no crawler can walk, run the Skyhook as
    /// a shuttle — gather a squad of idle ground fighters aboard, fly
    /// them to walkable ground beside the enemy base, and set them
    /// down. Landed machines are ordinary units again: the army channel
    /// drafts them where they stand and their own aggro carries the
    /// fight. The staging army's members are fair riders — on a gulf
    /// map the rally body IS the assault, and the Load lowering strikes
    /// riders from army bookkeeping — but the rear line and mid-march
    /// bodies are not.
    fn ferry(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        armies: &[Army],
        home: TilePos,
        enlisted: &[UnitId],
        intents: &mut Vec<Intent>,
    ) {
        if !dials.ferry {
            return;
        }
        let Some(sky) = obs
            .my_units
            .iter()
            .filter(|u| u.kind.stats().transport_capacity > 0)
            .min_by_key(|u| u.id)
        else {
            self.ferry_boarding.clear();
            return;
        };
        let Some(target) = Self::island_target(obs, home).or_else(|| {
            // Blind island desperation presumes the enemy at home's
            // mirror: the ferry flies at the one guess a symmetric
            // quarry offers, and contact does the rest.
            (self.desperate && !self.desperate_road)
                .then(|| TilePos::new(obs.map_width - 1 - home.x, obs.map_height - 1 - home.y))
        }) else {
            return;
        };
        // Riders gone from the field are aboard or dead; riders idle
        // again bounced off the sling. Either way, no longer pending.
        self.ferry_boarding
            .retain(|id| obs.my_units.iter().any(|u| u.id == *id && !u.idle));
        if sky.cargo > 0 {
            // Loaded, settled, and nobody still walking out: fly the
            // drop. A partial squad flies rather than waiting forever.
            if sky.idle && self.ferry_boarding.is_empty() {
                intents.push(Intent::Unload {
                    transport: sky.id,
                    at: self.unload_site(obs, target),
                });
            }
            return;
        }
        if !sky.idle {
            return; // outbound or returning
        }
        let staging: Vec<UnitId> = armies
            .iter()
            .filter(|a| a.state == ArmyState::Staging)
            .flat_map(|a| a.members.iter().copied())
            .collect();
        let pool: Vec<&UnitObs> = obs
            .my_units
            .iter()
            .filter(|u| {
                let stats = u.kind.stats();
                stats.domain == Domain::Ground
                    && stats.can_fight()
                    && stats.transport_size > 0
                    && u.idle
                    && (!enlisted.contains(&u.id) || staging.contains(&u.id))
            })
            .collect();
        if pool.len() < FERRY_SQUAD {
            return;
        }
        // Nearest to the sling first, ties to the lowest id; take what
        // fits the rack (a machine too big for the remaining room is
        // passed over for a smaller one behind it).
        let mut ranked: Vec<(i32, UnitId, u8)> = pool
            .iter()
            .map(|u| {
                (
                    u.tile.chebyshev(sky.tile),
                    u.id,
                    u.kind.stats().transport_size,
                )
            })
            .collect();
        ranked.sort_unstable();
        let mut room = sky.kind.stats().transport_capacity;
        let mut riders = Vec::new();
        for (_, id, size) in ranked {
            if size > 0 && size <= room {
                room -= size;
                riders.push(id);
            }
        }
        if riders.is_empty() {
            return;
        }
        self.ferry_boarding = riders.clone();
        intents.push(Intent::Load {
            transport: sky.id,
            riders,
        });
    }

    /// The next owed tech rung's price plus the fighting reserve — the
    /// fund the unbounded military drip must leave untouched so the
    /// construction channel can ever afford to climb. Zero once the
    /// dials' tree is fully raised (a standing site counts: its cost is
    /// already spent).
    fn capital_reserve(dials: &Dials, obs: &Observation) -> u32 {
        if !dials.tech {
            return 0;
        }
        let have = |kind: BuildingKind| obs.my_buildings.iter().any(|b| b.kind == kind);
        let price =
            |kind: BuildingKind| kind.base_stats().construction.map(|c| c.cost).unwrap_or(0);
        let mut rungs = vec![BuildingKind::Fabricator];
        if dials.deep_tech {
            rungs.push(BuildingKind::Airworks);
        }
        // The expansion Foundry is a capital rung too: without holding
        // its fund, the wartime drip pins the bank far below it and the
        // core 0.15 economy move never happens. Cheap frontier proxy
        // here (any known salvage beyond the radius); the construction
        // arm still proves reachability before claiming.
        if dials.expansion && (!dials.deep_tech || have(BuildingKind::Airworks)) {
            let foundries: Vec<TilePos> = obs
                .my_buildings
                .iter()
                .filter(|b| b.kind == BuildingKind::Foundry)
                .map(|b| b.anchor)
                .collect();
            let frontier = obs
                .known_scrap
                .iter()
                .filter(|(_, amount)| *amount > 0)
                .map(|(tile, _)| *tile)
                .chain(obs.known_frames.iter().copied())
                .any(|tile| {
                    foundries
                        .iter()
                        .all(|f| f.chebyshev(tile) > EXPANSION_RADIUS)
                });
            if frontier && foundries.len() < 2 && have(BuildingKind::Foundry) {
                return price(BuildingKind::Foundry) + TECH_RESERVE;
            }
        }
        if dials.deep_tech {
            rungs.push(BuildingKind::Crucible);
        }
        rungs
            .into_iter()
            .find(|kind| !have(*kind))
            .map(|kind| price(kind) + TECH_RESERVE)
            .unwrap_or(0)
    }

    /// The Overseer's 0.15 ladder: restore a known frame, then raise the
    /// Airworks, then the Crucible, then lift a Reclaimer or Turret one
    /// rung — one act per think, each gated on a healthy bank so the
    /// army channels never starve. Returns true when it spent the think.
    fn overseer_construction(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        home: TilePos,
        budget: &mut u32,
        intents: &mut Vec<Intent>,
    ) -> bool {
        let have = |kind: BuildingKind| obs.my_buildings.iter().any(|b| b.kind == kind);
        let have_built =
            |kind: BuildingKind| obs.my_buildings.iter().any(|b| b.kind == kind && b.built);
        // Restoring a frame is the cheapest, highest-yield act on the
        // board: take the nearest known unclaimed frame.
        if dials.extractors {
            let cost = BuildingKind::Extractor
                .base_stats()
                .construction
                .map(|c| c.cost)
                .unwrap_or(0);
            if *budget >= cost + TECH_RESERVE {
                // A frame's anchor is FIXED, so it must never enter the
                // pending/dead blacklists: one think whose builders were
                // all claimed elsewhere would poison the only anchor the
                // Extractor can ever have. The intent simply re-issues
                // until a standing site claims the frame.
                let claimed = |anchor: TilePos| {
                    obs.my_buildings.iter().any(|b| b.anchor == anchor)
                        || obs.enemy_buildings.iter().any(|b| b.anchor == anchor)
                };
                let frame = obs
                    .known_frames
                    .iter()
                    .filter(|f| !claimed(**f))
                    // A frame no builder can walk to must not be
                    // claimed: the intent would re-issue forever and
                    // starve every deeper construction rung (the
                    // island-map deadlock). The road must be KNOWN —
                    // the optimistic flood survives any unexplored
                    // gulf, and a cross-strait frame it admits eats
                    // every construction think until the map dies.
                    .filter(|f| Self::ground_route_known(obs, home, **f))
                    .min_by_key(|f| (f.chebyshev(home), f.y, f.x))
                    .copied();
                if let Some(anchor) = frame {
                    *budget -= cost;
                    intents.push(Intent::Build {
                        kind: BuildingKind::Extractor,
                        anchor,
                    });
                    return true;
                }
            }
        }
        // Expansion: once the tree stands, a second Foundry toward the
        // nearest salvage frontier no Foundry serves — forward
        // production, a drop-off that shortens the haul, and one more
        // victory token the enemy must come dig out. The core 0.15
        // economy move, demonstrated where training can see it.
        if dials.expansion
            && have_built(BuildingKind::Foundry)
            && (!dials.deep_tech || have(BuildingKind::Airworks))
        {
            let cost = BuildingKind::Foundry
                .base_stats()
                .construction
                .map(|c| c.cost)
                .unwrap_or(0);
            let foundries: Vec<TilePos> = obs
                .my_buildings
                .iter()
                .filter(|b| b.kind == BuildingKind::Foundry)
                .map(|b| b.anchor)
                .collect();
            if foundries.len() < 3 && *budget >= cost + TECH_RESERVE {
                let frontier = obs
                    .known_scrap
                    .iter()
                    .filter(|(_, amount)| *amount > 0)
                    .map(|(tile, _)| *tile)
                    .chain(obs.known_frames.iter().copied())
                    .filter(|tile| {
                        foundries
                            .iter()
                            .all(|f| f.chebyshev(*tile) > EXPANSION_RADIUS)
                            && Self::ground_route_known(obs, home, *tile)
                    })
                    .min_by_key(|tile| {
                        let frontier = foundries
                            .iter()
                            .map(|f| f.chebyshev(*tile))
                            .min()
                            .unwrap_or(0);
                        (frontier, tile.y, tile.x)
                    });
                if let Some(focus) = frontier
                    && let Some(anchor) = self.placement_near(obs, BuildingKind::Foundry, focus)
                {
                    *budget -= cost;
                    self.pending_sites.push(anchor);
                    intents.push(Intent::Build {
                        kind: BuildingKind::Foundry,
                        anchor,
                    });
                    return true;
                }
            }
        }
        if dials.deep_tech && have_built(BuildingKind::Fabricator) {
            for kind in [BuildingKind::Airworks, BuildingKind::Crucible] {
                if have(kind) {
                    continue;
                }
                let cost = kind.base_stats().construction.map(|c| c.cost).unwrap_or(0);
                if *budget >= cost + TECH_RESERVE
                    && let Some(anchor) = self.placement_near(obs, kind, home)
                {
                    *budget -= cost;
                    self.pending_sites.push(anchor);
                    intents.push(Intent::Build { kind, anchor });
                    return true;
                }
                // The next rung waits until this one is affordable.
                return false;
            }
        }
        if dials.upgrades {
            for (kind, tier) in [(BuildingKind::Reclaimer, 0), (BuildingKind::Turret, 0)] {
                let Some(upgrade) = kind.upgrade_from(tier) else {
                    continue;
                };
                if upgrade.requires.iter().any(|req| !have_built(*req)) {
                    continue;
                }
                if *budget < upgrade.cost + TECH_RESERVE {
                    continue;
                }
                let target = obs
                    .my_buildings
                    .iter()
                    .filter(|b| b.kind == kind && b.built && b.tier == tier)
                    .min_by_key(|b| (b.anchor.y, b.anchor.x));
                if let Some(b) = target {
                    *budget -= upgrade.cost;
                    intents.push(Intent::Upgrade { building: b.id });
                    return true;
                }
            }
        }
        false
    }

    /// Construction channel: orphaned sites first (paid-for progress
    /// must not strand), then the Fabricator, then a turret answer to
    /// raids. One build per think.
    fn construction(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        home: TilePos,
        budget: &mut u32,
        intents: &mut Vec<Intent>,
    ) {
        // Orphan relief is free (resuming an own site charges nothing).
        let orphan = obs
            .my_buildings
            .iter()
            .filter(|b| !b.built && !obs.my_units.iter().any(|u| u.site == Some(b.id)))
            .min_by_key(|b| (b.anchor.y, b.anchor.x));
        if let Some(site) = orphan {
            intents.push(Intent::Build {
                kind: site.kind,
                anchor: site.anchor,
            });
            return;
        }

        // The 0.15 climb: one rung per think, cheapest gate first.
        if (dials.deep_tech || dials.extractors || dials.upgrades || dials.expansion)
            && self.overseer_construction(dials, obs, home, budget, intents)
        {
            return;
        }

        let harvesters = obs
            .my_units
            .iter()
            .filter(|u| u.kind.stats().harvest.is_some())
            .count();
        if dials.tech {
            let fab_cost = BuildingKind::Fabricator
                .base_stats()
                .construction
                .map(|c| c.cost);
            let have_fab = obs
                .my_buildings
                .iter()
                .any(|b| b.kind == BuildingKind::Fabricator);
            if let Some(cost) = fab_cost
                && !have_fab
                && harvesters >= dials.harvester_target.min(3) as usize
                && *budget >= cost + TECH_RESERVE
                && let Some(anchor) = self.placement_near(obs, BuildingKind::Fabricator, home)
            {
                *budget -= cost;
                self.pending_sites.push(anchor);
                intents.push(Intent::Build {
                    kind: BuildingKind::Fabricator,
                    anchor,
                });
                return;
            }
        }

        if dials.turret_response && self.raided {
            let turret_cost = BuildingKind::Turret
                .base_stats()
                .construction
                .map(|c| c.cost);
            let turrets = obs
                .my_buildings
                .iter()
                .filter(|b| b.kind == BuildingKind::Turret)
                .count();
            if let Some(cost) = turret_cost
                && turrets < TURRET_CAP
                && *budget >= cost + UnitKind::Harvester.stats().cost
                && let Some(node) = self.nearest_scrap(obs, home)
                && let Some(anchor) = self.placement_near(obs, BuildingKind::Turret, node)
            {
                *budget -= cost;
                self.pending_sites.push(anchor);
                intents.push(Intent::Build {
                    kind: BuildingKind::Turret,
                    anchor,
                });
                return;
            }
        }

        // Lane mines (dial-gated, Overseer only): with the harvest line
        // at strength and either a raid felt or the enemy's ground road
        // known, bury a few cheap Scuttle Charges a few tiles out from
        // home along the approach. Defense the enemy pays to discover,
        // never the economy's opening.
        if dials.mines {
            let have_fab = obs
                .my_buildings
                .iter()
                .any(|b| b.kind == BuildingKind::Fabricator && b.built);
            let charges = obs
                .my_buildings
                .iter()
                .filter(|b| b.kind == BuildingKind::ScuttleCharge)
                .count();
            let charge_cost = BuildingKind::ScuttleCharge
                .base_stats()
                .construction
                .map(|c| c.cost);
            if harvesters >= dials.harvester_target as usize
                && have_fab
                && charges < MINE_CAP
                && let Some(cost) = charge_cost
                && *budget >= cost + TECH_RESERVE
            {
                let site = Self::enemy_site(obs, home);
                let route_known = site.is_some_and(|s| Self::ground_route_known(obs, home, s));
                if self.raided || route_known {
                    // Raided blind (no site known), the field centers on
                    // the map's middle — the only approach there is.
                    let toward =
                        site.unwrap_or(TilePos::new(obs.map_width / 2, obs.map_height / 2));
                    let lean = |from: i32, to: i32| from + (to - from).clamp(-MINE_LEAN, MINE_LEAN);
                    let focus = TilePos::new(lean(home.x, toward.x), lean(home.y, toward.y));
                    if let Some(anchor) =
                        self.placement_near(obs, BuildingKind::ScuttleCharge, focus)
                    {
                        *budget -= cost;
                        self.pending_sites.push(anchor);
                        intents.push(Intent::Build {
                            kind: BuildingKind::ScuttleCharge,
                            anchor,
                        });
                        return;
                    }
                }
            }
        }

        // The sky over the economy: enemy air sighted (or blips inbound)
        // raises flak over the harvest line.
        if dials.aa_response && (self.seen_air || !obs.blips.is_empty()) {
            let flak_cost = BuildingKind::FlakTurret
                .base_stats()
                .construction
                .map(|c| c.cost);
            let flak = obs
                .my_buildings
                .iter()
                .filter(|b| b.kind == BuildingKind::FlakTurret)
                .count();
            if let Some(cost) = flak_cost
                && flak < FLAK_CAP
                && *budget >= cost + UnitKind::Harvester.stats().cost
                && let Some(node) = self.nearest_scrap(obs, home)
                && let Some(anchor) = self.placement_near(obs, BuildingKind::FlakTurret, node)
            {
                *budget -= cost;
                self.pending_sites.push(anchor);
                intents.push(Intent::Build {
                    kind: BuildingKind::FlakTurret,
                    anchor,
                });
                return;
            }
        }

        // One Array once teched: the early-warning ring and the eyes
        // long guns fire on.
        if dials.radar {
            let have_fab = obs
                .my_buildings
                .iter()
                .any(|b| b.kind == BuildingKind::Fabricator && b.built);
            let have_array = obs
                .my_buildings
                .iter()
                .any(|b| b.kind == BuildingKind::Array);
            let array_cost = BuildingKind::Array
                .base_stats()
                .construction
                .map(|c| c.cost);
            if have_fab
                && !have_array
                && let Some(cost) = array_cost
                && *budget >= cost + TECH_RESERVE
                && let Some(anchor) = self.placement_near(obs, BuildingKind::Array, home)
            {
                *budget -= cost;
                self.pending_sites.push(anchor);
                intents.push(Intent::Build {
                    kind: BuildingKind::Array,
                    anchor,
                });
                return;
            }
        }

        // Reclaimers once the patches near home run dry: the economy's
        // retirement plan, never its opening.
        if dials.reclaimers {
            let near_home: u32 = obs
                .known_scrap
                .iter()
                .chain(obs.known_wrecks.iter())
                .filter(|(pos, _)| pos.chebyshev(home) <= HOME_SALVAGE_RADIUS)
                .map(|(_, amount)| amount)
                .sum();
            let reclaimers = obs
                .my_buildings
                .iter()
                .filter(|b| b.kind == BuildingKind::Reclaimer)
                .count();
            let rec_cost = BuildingKind::Reclaimer
                .base_stats()
                .construction
                .map(|c| c.cost);
            if near_home < SALVAGE_LOW
                && reclaimers < RECLAIMER_CAP
                && let Some(cost) = rec_cost
                && *budget >= cost + TECH_RESERVE
                && let Some(anchor) = self.placement_near(obs, BuildingKind::Reclaimer, home)
            {
                *budget -= cost;
                self.pending_sites.push(anchor);
                intents.push(Intent::Build {
                    kind: BuildingKind::Reclaimer,
                    anchor,
                });
            }
        }
    }

    /// Repair channel: one weld order per think for the most wounded
    /// standing building, funded only past a fighting reserve — welding
    /// is upkeep, never the main line's budget.
    fn repairs(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        budget: &mut u32,
        intents: &mut Vec<Intent>,
    ) {
        if !dials.repair {
            return;
        }
        // Reserve: a sentinel's price stays banked, and the trickle
        // itself is cheap — gate on the reserve, not the full damage.
        let reserve = UnitKind::Sentinel.stats().cost;
        if *budget < reserve {
            return;
        }
        let patient = obs
            .my_buildings
            .iter()
            .filter(|b| b.built && b.hp * 10 < b.kind.tier_stats(b.tier).max_hp * 8)
            // A building an own crew is stripping is being LIQUIDATED
            // on purpose — repair and salvage evict each other in the
            // sim, so a repair intent here would re-crew the teardown
            // and reverse it (the gym's lowering applies this same
            // filter).
            .filter(|b| !obs.my_units.iter().any(|u| u.salvaging == Some(b.id)))
            .map(|b| {
                let deficit = b.kind.tier_stats(b.tier).max_hp - b.hp;
                (std::cmp::Reverse(deficit), b.anchor.y, b.anchor.x, b.id)
            })
            .min()
            .map(|(.., id)| id);
        if let Some(building) = patient {
            intents.push(Intent::Repair { building });
        }
    }

    /// Salvage channel: when the war has outlived the economy — bank
    /// starved, nothing known left to mine or strip off the ground —
    /// liquidate static defense cheapest-first and spend the ground on
    /// one more wave. Deliberately narrow: a commander that sells its
    /// walls mid-siege teaches the learner the wrong lesson; one that
    /// converts dead weight into a late push teaches the right one,
    /// from the receiving side.
    fn salvage(&mut self, dials: &Dials, obs: &Observation, intents: &mut Vec<Intent>) {
        if !dials.salvage {
            return;
        }
        if obs.scrap >= UnitKind::Harvester.stats().cost {
            return;
        }
        let sources_left = obs.known_scrap.iter().any(|(_, amount)| *amount > 0)
            || obs.known_wrecks.iter().any(|(_, amount)| *amount > 0);
        if sources_left {
            return;
        }
        let target = obs
            .my_buildings
            .iter()
            .filter(|b| b.built)
            .filter_map(|b| {
                super::gym::SALVAGE_PRIORITY
                    .iter()
                    .position(|k| *k == b.kind)
                    .map(|rank| (rank, b.anchor.y, b.anchor.x, b.id))
            })
            .min()
            .map(|(.., id)| id);
        if let Some(building) = target {
            intents.push(Intent::Salvage { building });
        }
    }

    /// Air-raid channel: once a wing of idle ground-attack flyers has
    /// gathered, throw it at the enemy's harvest line — unless known
    /// anti-air stands over the target. Wings are spent, not managed:
    /// the raid is an attack-move and whatever comes back rejoins the
    /// idle pool.
    fn air_raid(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        home: TilePos,
        enlisted: &[UnitId],
        intents: &mut Vec<Intent>,
    ) {
        if !dials.air_harass {
            return;
        }
        let wings = obs
            .my_units
            .iter()
            .filter(|u| {
                let stats = u.kind.stats();
                stats.domain == crate::stats::Domain::Air
                    && stats.can_target(crate::stats::Domain::Ground)
                    && u.idle
                    && !enlisted.contains(&u.id)
            })
            .count();
        if wings < AIR_WING {
            return;
        }
        // The juiciest known target: an enemy harvester, else any
        // enemy building — the raid flies at work, not at armies.
        let target = obs
            .enemy_units
            .iter()
            .filter(|u| u.kind.stats().harvest.is_some())
            .map(|u| (u.tile.manhattan(home), u.tile.y, u.tile.x))
            .min()
            .map(|(_, y, x)| TilePos::new(x, y))
            .or_else(|| Self::enemy_site(obs, home));
        let Some(target) = target else { return };
        // Known anti-air over the target scrubs the raid: flak turrets
        // and AA crawlers in sight or memory near the objective.
        let aa_guard = obs
            .enemy_buildings
            .iter()
            .filter(|b| b.kind == BuildingKind::FlakTurret)
            .map(|b| b.anchor)
            .chain(
                obs.enemy_units
                    .iter()
                    .filter(|u| u.kind.stats().can_target(crate::stats::Domain::Air))
                    .map(|u| u.tile),
            )
            .any(|t| t.chebyshev(target) <= RAID_AA_RADIUS);
        if !aa_guard {
            intents.push(Intent::RaidAir { target });
        }
    }

    /// Scouting channel: keep the fog-honest observation fresh without
    /// running a death conveyor. While the enemy is unlocated, sweep
    /// standoff points (the mirror of home first — symmetric maps put
    /// the enemy there). Once a base is known, peek at it from vision
    /// range every so often; between refreshes the scout is released
    /// back to the draft pool. A scout looks — it never parks in the
    /// enemy's aggro.
    pub(super) fn scouting(
        &mut self,
        obs: &Observation,
        home: TilePos,
        enlisted: &[UnitId],
        aids: ScoutAids<'_>,
        force: bool,
        intents: &mut Vec<Intent>,
    ) {
        let ScoutAids {
            extra,
            anchors,
            ground_may_search,
            frontier_step,
        } = aids;
        /// How far short of the objective a scout stops — inside a
        /// harvester's vision (6), and close enough to aggro (5) that
        /// the peek must rely on the scout's legs, not its armor.
        const STANDOFF: i32 = 5;
        /// Fighters a forced, target-less scout press may fan out at
        /// once beyond the primary peeker.
        const SEARCH_PARTY_SIZE: usize = 6;

        let known_base = obs
            .enemy_buildings
            .iter()
            .map(|b| (b.anchor.manhattan(home), b.anchor.y, b.anchor.x))
            .min()
            .map(|(_, y, x)| TilePos::new(x, y));
        let due = force
            || known_base.is_none()
            || obs.tick.saturating_sub(self.scouted_at) >= SCOUT_REFRESH;

        if let Some(id) = self.scout
            && !obs.my_units.iter().any(|u| u.id == id)
        {
            self.scout = None; // died on duty
        }
        // On a proven-sealed map a ground scout cannot reach any leg: a
        // designated crawler is released back to the pool (a harvester
        // returns to the economy) instead of being re-dispatched at the
        // coast forever. Air designates keep the job.
        if let Some(id) = self.scout
            && !ground_may_search
            && obs
                .my_units
                .iter()
                .any(|u| u.id == id && u.kind.stats().domain != Domain::Air)
        {
            self.scout = None;
        }
        if !due {
            // Between sweeps the scout goes back in the pool.
            if let Some(id) = self.scout
                && obs.my_units.iter().any(|u| u.id == id && u.idle)
            {
                self.scout = None;
            }
            return;
        }
        let picked_now = self.scout.is_none();
        if self.scout.is_none() {
            // A scout-role flyer is the scout of choice: unarmed, wide
            // eyes, and it crosses pits and gulfs no crawler can — on
            // an island map it is the only machine that can look at
            // all. A harvester is next (it outruns every chaser, so
            // the peek costs a stretch of income instead of a body).
            // Fighters are the fallback.
            self.scout = obs
                .my_units
                .iter()
                // A walking founder (`founding`) is spoken for like a
                // builder on site: a scout order would replace the
                // deferred claim's whole program.
                .filter(|u| u.site.is_none() && u.founding.is_none())
                .filter(|u| {
                    extra.contains(&u.id)
                        || (!enlisted.contains(&u.id)
                            && (u.kind.stats().harvest.is_some() || u.idle))
                })
                .filter(|u| ground_may_search || u.kind.stats().domain == Domain::Air)
                .min_by_key(|u| {
                    let preference = match u.kind {
                        UnitKind::Kestrel | UnitKind::Gnat => (0, 0),
                        UnitKind::Harvester => (1, u.carrying),
                        UnitKind::Scuttler => (2, 0),
                        UnitKind::Sentinel => (3, 0),
                        _ => (4, 0),
                    };
                    (preference, u.id)
                })
                .map(|u| u.id);
        }
        let Some(scout) = self.scout else { return };
        // A fresh pick is dispatched immediately (a working harvester is
        // not idle); an existing scout gets its next leg only once the
        // current one completes.
        if !picked_now && !obs.my_units.iter().any(|u| u.id == scout && u.idle) {
            return;
        }

        let standoff = |from: TilePos, objective: TilePos| -> TilePos {
            let (dx, dy) = (objective.x - from.x, objective.y - from.y);
            let d = dx.abs().max(dy.abs());
            if d <= STANDOFF {
                return objective;
            }
            TilePos::new(
                objective.x - dx * STANDOFF / d,
                objective.y - dy * STANDOFF / d,
            )
        };
        let to = if let Some(base) = known_base {
            self.scouted_at = obs.tick;
            standoff(home, base)
        } else {
            let (w, h) = (obs.map_width, obs.map_height);
            let legs = [
                standoff(home, TilePos::new(w - 1 - home.x, h - 1 - home.y)),
                TilePos::new(w / 2, h / 2),
                TilePos::new(3, 3),
                TilePos::new(w - 4, 3),
                TilePos::new(3, h - 4),
                TilePos::new(w - 4, h - 4),
            ];
            let leg = legs[self.scout_leg as usize % legs.len()];
            self.scout_leg += 1;
            leg
        };
        let to = self.passable_near(obs, to);
        let to = match obs.my_units.iter().find(|u| u.id == scout) {
            Some(u) if frontier_step && u.kind.stats().domain != Domain::Air => {
                match self.ground_frontier_toward(obs, u.tile, to) {
                    Some(frontier) => frontier,
                    // The lit component is swept: a raw cross-dark leg
                    // would stall every think. Release the designate.
                    None => {
                        self.scout = None;
                        return;
                    }
                }
            }
            _ => to,
        };
        intents.push(Intent::Scout { unit: scout, to });

        // The search party: with every enemy site lost, one peeker
        // cannot sweep a big map before the game rots — measured as
        // 250-unit seats idling for forty thousand ticks over a hiding
        // remnant. A forced scout (the gym path; the scripted Brain
        // never forces) fans idle fighters out over the sweep legs,
        // exactly as a player would fan a search. Walking scouts are
        // not idle, so each press tops the party back up to strength
        // without churning anyone already on a leg.
        if force && known_base.is_none() {
            let (w, h) = (obs.map_width, obs.map_height);
            // Check where the enemy STARTED before hunting darkness: a
            // player knows every base's birthplace from the map screen
            // and looks there first. Only unexplored anchors qualify —
            // an explored, empty birthplace teaches nothing twice.
            let unexplored_anchors: Vec<TilePos> = anchors
                .iter()
                .copied()
                .filter(|anchor| {
                    let index = (anchor.y * w + anchor.x) as usize;
                    anchor.x >= 0
                        && anchor.y >= 0
                        && anchor.x < w
                        && anchor.y < h
                        && !obs.explored.get(index).copied().unwrap_or(true)
                })
                .collect();
            // Hunt the darkness second: score a coarse grid by
            // unexplored tiles (the minimap's black, the same read a
            // player makes) and send each searcher at the darkest
            // cell. A remnant that rebuilt inside old, unwatched
            // exploration falls back to the fixed legs.
            const GRID: i32 = 8;
            let cell_w = (w / GRID).max(1);
            let cell_h = (h / GRID).max(1);
            let mut cells: Vec<(usize, i32, i32)> = Vec::new();
            for cy in 0..GRID.min(h) {
                for cx in 0..GRID.min(w) {
                    let mut unexplored = 0usize;
                    for y in (cy * cell_h)..((cy + 1) * cell_h).min(h) {
                        for x in (cx * cell_w)..((cx + 1) * cell_w).min(w) {
                            let index = (y * w + x) as usize;
                            if !obs.explored.get(index).copied().unwrap_or(false) {
                                unexplored += 1;
                            }
                        }
                    }
                    if unexplored > 0 {
                        cells.push((unexplored, cy, cx));
                    }
                }
            }
            cells.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
            let legs = [
                standoff(home, TilePos::new(w - 1 - home.x, h - 1 - home.y)),
                TilePos::new(w / 2, h / 2),
                TilePos::new(3, 3),
                TilePos::new(w - 4, 3),
                TilePos::new(3, h - 4),
                TilePos::new(w - 4, h - 4),
            ];
            let party: Vec<UnitId> = obs
                .my_units
                .iter()
                .filter(|u| {
                    u.id != scout
                        && u.idle
                        && u.site.is_none()
                        && u.founding.is_none()
                        && u.kind.stats().can_fight()
                        && (ground_may_search || u.kind.stats().domain == Domain::Air)
                        && (!enlisted.contains(&u.id) || extra.contains(&u.id))
                })
                .map(|u| u.id)
                .take(SEARCH_PARTY_SIZE)
                .collect();
            for (index, unit) in party.into_iter().enumerate() {
                let to = if let Some(anchor) = unexplored_anchors.get(index) {
                    *anchor
                } else {
                    let past_anchors = index - unexplored_anchors.len().min(index);
                    match cells.get(past_anchors % cells.len().max(1)) {
                        Some((_, cy, cx)) if !cells.is_empty() => {
                            TilePos::new(cx * cell_w + cell_w / 2, cy * cell_h + cell_h / 2)
                        }
                        _ => legs[(unit.0 as usize) % legs.len()],
                    }
                };
                let to = self.passable_near(obs, to);
                let to = match obs.my_units.iter().find(|u| u.id == unit) {
                    Some(u) if frontier_step && u.kind.stats().domain != Domain::Air => {
                        match self.ground_frontier_toward(obs, u.tile, to) {
                            Some(frontier) => frontier,
                            // Swept component: this searcher has nowhere
                            // useful to walk — skip it rather than stall.
                            None => continue,
                        }
                    }
                    _ => to,
                };
                intents.push(Intent::Scout { unit, to });
            }
        }
    }

    /// Army channel: an intruder near home turns every army on it;
    /// otherwise keep feeding the staging army and commit it when it
    /// reaches size.
    fn army(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        armies: &[Army],
        home: TilePos,
        intents: &mut Vec<Intent>,
    ) {
        let enemy_site = Self::enemy_site(obs, home);
        let staging_army = armies
            .iter()
            .filter(|a| a.state == ArmyState::Staging)
            .min_by_key(|a| a.id);
        let rally = self.rally_point(obs, staging_army, enemy_site, home);

        // Defense: an intruder near home — or near an ally's foundry;
        // a teammate's base is ground worth marching for — turns every
        // army on it. Fresh fighters still muster at the rally — a body
        // forms there and joins whole; sending each spawn at the threat
        // is the trickle.
        let bases: Vec<TilePos> = std::iter::once(home)
            .chain(
                obs.ally_buildings
                    .iter()
                    .filter(|b| b.kind == BuildingKind::Foundry)
                    .map(|b| b.anchor),
            )
            .collect();
        let intruder = obs
            .enemy_units
            .iter()
            .filter(|u| is_fighter(u))
            .filter_map(|u| {
                bases
                    .iter()
                    .map(|b| u.tile.chebyshev(*b))
                    .min()
                    .filter(|d| *d <= DEFENSE_RADIUS)
                    .map(|d| (d, u.tile.y, u.tile.x, u.id))
            })
            .min()
            .map(|(_, y, x, _)| TilePos::new(x, y));
        if let Some(threat) = intruder {
            for army in armies {
                // Re-target only when the threat has really moved: churning
                // fresh attack-moves every think as it shifts a tile keeps
                // interrupting members mid-swing — auto-acquire handles
                // the last few tiles better than micromanagement does.
                if army.target.is_none_or(|t| t.chebyshev(threat) > 4) {
                    intents.push(Intent::PushArmy {
                        army: army.id,
                        target: threat,
                    });
                }
            }
            intents.push(Intent::FormArmy {
                staging: rally,
                size: dials.army_size,
            });
            return;
        }

        // The push gate: an offensive march is the hardest fight there is
        // — the whole approach is time the defender spends producing, and
        // the fight lands on their turf with their turrets. Commit only
        // at twice everything the enemy is known to field; until then,
        // keep raising the draft target so the army outgrows the
        // threshold instead of trickling into a fair fight.
        let army_strength: u64 = staging_army
            .map(|a| {
                obs.my_units
                    .iter()
                    .filter(|u| a.members.contains(&u.id))
                    .map(super::executive::unit_strength)
                    .sum()
            })
            .unwrap_or(0);
        let enemy_strength: u64 = obs
            .enemy_units
            .iter()
            .map(super::executive::unit_strength)
            .sum::<u64>()
            + obs
                .enemy_buildings
                .iter()
                .map(super::executive::building_strength)
                .sum::<u64>();
        // Seeing no enemy strength is not the same as the enemy having
        // none — fog hides armies. Floor the estimate by how fresh the
        // intel is: a recent peek at their base earns trust in the count,
        // blindness demands mass. Omniscience is permanently fresh.
        let intel_fresh = !dials.fog_honest
            || (self.scouted_at > 0
                && obs.tick.saturating_sub(self.scouted_at) < 2 * SCOUT_REFRESH);
        let sentinel = UnitKind::Sentinel.stats();
        let atk = sentinel.weapons.first().expect("sentinels fight");
        let sentinel_worth = u64::from(sentinel.max_hp)
            * (u64::from(atk.damage) * 100 / u64::from(atk.cooldown_ticks));
        let floor = if intel_fresh { 2 } else { 5 } * sentinel_worth;
        // Patience decays the demanded margin from 2.0× down to 1.0×
        // over the match: two flawless defenders would otherwise wait
        // forever for an edge neither can get, and a fair fight taken
        // late beats a stalemate never resolved.
        let patience = (obs.tick / 4000).min(4);
        // 0.15.3 desperation: with the Foundry drip gone, a starved
        // economy can freeze against a frozen army gate — harvesters
        // hide from a contested midfield, the bank never grows, and
        // the push waits for an edge no income will ever buy. "Nothing
        // has come in for a long time and the bank is empty" is the
        // honest starvation signal: fog memory makes "no income
        // possible" unknowable, because a turtled seat remembers
        // salvage it will never dare work. The margin drops to an even
        // fight and the blind-mass floor to one sentinel's worth, so
        // scarcity ends games instead of freezing them.
        let desperate = self.desperate;
        let (margin_num, margin_den) = if desperate {
            (4, 4u64)
        } else {
            (8 - patience, 4u64)
        };
        let commit_floor = if desperate { sentinel_worth } else { floor };
        let gate_open = army_strength * margin_den >= enemy_strength.max(commit_floor) * margin_num;

        let members = staging_army.map(|a| a.members.len()).unwrap_or(0);
        let target_size = if gate_open {
            dials.army_size.max(members as u32)
        } else {
            dials.army_size.max(members as u32 + 2)
        };
        intents.push(Intent::FormArmy {
            staging: rally,
            size: target_size,
        });

        // Commit: numbers met and the fight is expected to be unfair —
        // in our favor. A desperate seat commits whatever stands: the
        // draft will never grow, and when the enemy was never even
        // found (a broke seat cannot afford scouts), the march heads
        // for home's mirror — the one guess a symmetric quarry always
        // offers — and lets contact do the rest.
        if let Some(army) = staging_army
            && (army.members.len() >= dials.army_size as usize
                || (desperate && !army.members.is_empty()))
            && gate_open
            && let Some(target) = enemy_site.or_else(|| {
                (desperate && self.desperate_march).then(|| {
                    self.passable_near(
                        obs,
                        TilePos::new(obs.map_width - 1 - home.x, obs.map_height - 1 - home.y),
                    )
                })
            })
        {
            intents.push(Intent::PushArmy {
                army: army.id,
                target,
            });
        }
    }

    /// The nearest known enemy presence — buildings (ghosts included)
    /// before units — or None while the enemy is entirely unlocated.
    /// The unit fallback skips machines hovering over known rock: a site
    /// is a place ground forces could go, and a flyer parked on a crag
    /// once declared a fully land-connected map "sealed" because the
    /// route flood was asked to reach an unstandable goal.
    pub(super) fn enemy_site(obs: &Observation, home: TilePos) -> Option<TilePos> {
        obs.enemy_buildings
            .iter()
            .map(|b| (b.anchor.manhattan(home), b.anchor.y, b.anchor.x))
            .min()
            .map(|(_, y, x)| TilePos::new(x, y))
            .or_else(|| {
                obs.enemy_units
                    .iter()
                    .filter(|u| !obs.known_rock_at(u.tile))
                    .map(|u| (u.tile.manhattan(home), u.tile.y, u.tile.x))
                    .min()
                    .map(|(_, y, x)| TilePos::new(x, y))
            })
    }

    /// Where armies gather: the staging army's rally if one exists, else
    /// a fresh point leaning toward the enemy but within reach of home —
    /// a mid-map rally sits on the enemy's march path and gets
    /// reinforcements killed piecemeal.
    pub(super) fn rally_point(
        &self,
        obs: &Observation,
        staging_army: Option<&Army>,
        enemy_site: Option<TilePos>,
        home: TilePos,
    ) -> TilePos {
        staging_army.map(|a| a.staging).unwrap_or_else(|| {
            let toward = enemy_site.unwrap_or(TilePos::new(obs.map_width / 2, obs.map_height / 2));
            let lean = |from: i32, to: i32| from + ((to - from) / 3).clamp(-3, 3);
            self.passable_near(
                obs,
                TilePos::new(lean(home.x, toward.x), lean(home.y, toward.y)),
            )
        })
    }

    /// Ticks since the last scout order toward a known enemy base
    /// (u64::MAX before the first one) — the gym's intel-age feature.
    pub(super) fn intel_age(&self, tick: u64) -> u64 {
        if self.scouted_at == 0 {
            u64::MAX
        } else {
            tick.saturating_sub(self.scouted_at)
        }
    }

    /// Records a Build anchor requested this think, so the next audit
    /// can blacklist it if the sim refuses the site.
    pub(super) fn note_pending_site(&mut self, anchor: TilePos) {
        self.pending_sites.push(anchor);
    }

    /// Nearest known scrap by (manhattan, y, x), skipping bounced nodes.
    pub(super) fn nearest_scrap(&self, obs: &Observation, from: TilePos) -> Option<TilePos> {
        obs.known_scrap
            .iter()
            .filter(|(pos, amount)| *amount > 0 && !self.dead_nodes.contains(pos))
            .map(|(pos, _)| (pos.manhattan(from), pos.y, pos.x))
            .min()
            .map(|(_, y, x)| TilePos::new(x, y))
    }

    /// First anchor for `kind` ring-scanned outward from `near` whose
    /// footprint and doorstep ring are clear of everything the
    /// observation knows about — the sim's `can_place` still has the
    /// final word, and refusals land in [`Self::dead_anchors`].
    pub(super) fn placement_near(
        &self,
        obs: &Observation,
        kind: BuildingKind,
        near: TilePos,
    ) -> Option<TilePos> {
        let (w, h) = kind.base_stats().size;
        for r in 3i32..=7 {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dy.abs()) != r {
                        continue;
                    }
                    let anchor = near.offset(dx, dy);
                    if self.placement_valid(obs, anchor, w, h) {
                        return Some(anchor);
                    }
                }
            }
        }
        None
    }

    /// Every valid anchor in the same bounded field as [`Self::placement_near`].
    /// Defense placement uses the full set; ordinary scripted construction keeps
    /// the historical first-anchor behavior above.
    pub(super) fn placements_near(
        &self,
        obs: &Observation,
        kind: BuildingKind,
        near: TilePos,
    ) -> Vec<TilePos> {
        let (w, h) = kind.base_stats().size;
        let mut anchors = Vec::new();
        for r in 3i32..=7 {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dy.abs()) != r {
                        continue;
                    }
                    let anchor = near.offset(dx, dy);
                    if self.placement_valid(obs, anchor, w, h) {
                        anchors.push(anchor);
                    }
                }
            }
        }
        anchors
    }

    fn placement_valid(&self, obs: &Observation, anchor: TilePos, width: i32, height: i32) -> bool {
        if self.dead_anchors.contains(&anchor) {
            return false;
        }
        let in_bounds = |tile: TilePos| {
            tile.x >= 0 && tile.y >= 0 && tile.x < obs.map_width && tile.y < obs.map_height
        };
        let footprint_ok = (0..width).all(|dx| {
            (0..height).all(|dy| {
                let tile = anchor.offset(dx, dy);
                in_bounds(tile) && obs.explored(tile) && self.placement_tile_open(obs, tile)
            })
        });
        if !footprint_ok {
            return false;
        }
        (-1..=width).any(|dx| {
            (-1..=height).any(|dy| {
                let core = (0..width).contains(&dx) && (0..height).contains(&dy);
                let tile = anchor.offset(dx, dy);
                !core && in_bounds(tile) && obs.explored(tile) && self.tile_open(obs, tile)
            })
        })
    }

    fn placement_tile_open(&self, obs: &Observation, tile: TilePos) -> bool {
        if !self.tile_open(obs, tile) {
            return false;
        }
        // Nothing may pave over a derelict Extractor frame: the sim
        // refuses the whole footprint as FrameBlocked, and an anchor the
        // scorer keeps proposing anyway feeds the dead-anchor ledger for
        // a refusal the bot could have predicted. (Frames are map data;
        // this check lives here rather than in `tile_open` because that
        // predicate also serves rally spots, where standing on a frame
        // is fine.)
        if obs.known_frames.iter().any(|frame| {
            tile.x >= frame.x && tile.x < frame.x + 2 && tile.y >= frame.y && tile.y < frame.y + 2
        }) {
            return false;
        }
        let claimed = obs.my_units.iter().any(|unit| {
            unit.founding.is_some_and(|(kind, anchor)| {
                let (width, height) = kind.base_stats().size;
                tile.x >= anchor.x
                    && tile.x < anchor.x + width
                    && tile.y >= anchor.y
                    && tile.y < anchor.y + height
            })
        });
        !claimed
            && !obs
                .enemy_units
                .iter()
                .any(|unit| unit.kind.stats().domain == Domain::Ground && unit.tile == tile)
    }

    /// Known-buildable: not rock, not scrap, not under any known
    /// building footprint.
    fn tile_open(&self, obs: &Observation, t: TilePos) -> bool {
        if self.rock_at(obs, t) || obs.known_scrap_at(t) {
            return false;
        }
        let covered = |b: &super::observation::BuildingObs| {
            let (w, h) = b.kind.base_stats().size;
            t.x >= b.anchor.x && t.x < b.anchor.x + w && t.y >= b.anchor.y && t.y < b.anchor.y + h
        };
        !obs.my_buildings.iter().any(covered)
            && !obs.ally_buildings.iter().any(covered)
            && !obs.enemy_buildings.iter().any(covered)
    }

    fn rock_at(&self, obs: &Observation, t: TilePos) -> bool {
        obs.known_rock_at(t)
    }

    /// The nearest known-open tile to `want` (spiral out to 3), for
    /// rally points that shouldn't sit inside a rock formation.
    pub(super) fn passable_near(&self, obs: &Observation, want: TilePos) -> TilePos {
        for r in 0i32..=3 {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dy.abs()) != r {
                        continue;
                    }
                    let t = want.offset(dx, dy);
                    if t.x >= 0
                        && t.y >= 0
                        && t.x < obs.map_width
                        && t.y < obs.map_height
                        && self.tile_open(obs, t)
                    {
                        return t;
                    }
                }
            }
        }
        want
    }
}

/// Convenience for tests and policies: whether a unit observation
/// can fight.
pub fn is_fighter(u: &UnitObs) -> bool {
    u.kind.stats().can_fight()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::observation::{OBSERVATION_VERSION, Observation, UnitObs};
    use crate::ids::{PlayerId, UnitId};

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
}
