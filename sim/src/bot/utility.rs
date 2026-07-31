//! The utility policy: decision channels over an [`Observation`].
//!
//! Where the classic bot is one long rule cascade, this policy is a set
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

/// The policy's tunable considerations. Phase C's difficulty tiers are
/// presets over these dials (plus the executive's); the fairness rule is
/// that dials change *thinking* — never income, vision, or combat math.
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
}

impl Dials {
    /// Everything on: the full-strength scripted commander.
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
        }
    }

    /// Full strength with the classic cheating view — the strongest
    /// purely scripted opponent (Phase C tiers pick between these).
    pub fn full_omniscient() -> Self {
        Self {
            scouting: false,
            fog_honest: false,
            ..Self::full()
        }
    }
}

/// Channel-based scripted policy. Its memory is bot-local and legitimate
/// (a bot is a command source, not sim state): harvest blacklists, raid
/// memory, and the scout rotation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UtilityPolicy {
    /// Harvest assignments from the last think — a unit idle again right
    /// after being sent bounced off an unreachable node.
    last_sent: Vec<(UnitId, TilePos)>,
    /// Nodes that bounced a harvester back.
    dead_nodes: Vec<TilePos>,
    /// Harvester count at the last think; a drop means raiders.
    harvesters_seen: usize,
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
        self.production(dials, obs, &mut budget, &mut intents);
        self.construction(dials, obs, home_tile, &mut budget, &mut intents);
        self.repairs(dials, obs, &mut budget, &mut intents);
        self.salvage(dials, obs, &mut intents);
        // No scouting while the economy is short-handed: pulling one of
        // three starting harvesters off the line buys intel with the
        // opening — the most expensive scrap there is.
        let harvesters = obs
            .my_units
            .iter()
            .filter(|u| u.kind == UnitKind::Harvester)
            .count();
        if dials.scouting && harvesters >= dials.harvester_target as usize {
            self.scouting(obs, home_tile, enlisted, false, &mut intents);
        }
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
        for (id, node) in std::mem::take(&mut self.last_sent) {
            let bounced = obs
                .my_units
                .iter()
                .any(|u| u.id == id && u.idle && u.hp > 0);
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
    /// scripted tiers never defer — `founding` is always `None` on
    /// their path.
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
            .filter(|u| u.kind == UnitKind::Harvester)
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
    pub(super) fn economy(&mut self, obs: &Observation, home: TilePos, intents: &mut Vec<Intent>) {
        let enemy_base = obs
            .enemy_buildings
            .iter()
            .map(|b| (b.anchor.manhattan(home), b.anchor.y, b.anchor.x))
            .min()
            .map(|(_, y, x)| TilePos::new(x, y));
        for u in obs
            .my_units
            .iter()
            .filter(|u| u.kind == UnitKind::Harvester && u.idle && Some(u.id) != self.scout)
        {
            let node = obs
                .known_scrap
                .iter()
                .chain(obs.known_wrecks.iter())
                .filter(|(pos, amount)| {
                    *amount > 0
                        && !self.dead_nodes.contains(pos)
                        && enemy_base.is_none_or(|eb| pos.manhattan(home) <= pos.manhattan(eb))
                })
                .map(|(pos, _)| (pos.manhattan(u.tile), pos.y, pos.x))
                .min()
                .map(|(_, y, x)| TilePos::new(x, y));
            if let Some(node) = node {
                intents.push(Intent::AssignHarvest { unit: u.id, node });
                self.last_sent.push((u.id, node));
            }
        }
    }

    /// Starvation ladder — the neural bot's chore only; the scripted
    /// tiers never climb it (they are the ladder's fixed yardstick and
    /// must stay byte-identical). Runs after [`Self::economy`]: any
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
        let assigned: Vec<UnitId> = self.last_sent.iter().map(|(id, _)| *id).collect();
        let starved: Vec<(UnitId, TilePos)> = obs
            .my_units
            .iter()
            .filter(|u| {
                u.kind == UnitKind::Harvester
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
            let node = obs
                .known_scrap
                .iter()
                .chain(obs.known_wrecks.iter())
                .filter(|(pos, amount)| *amount > 0 && !self.dead_nodes.contains(pos))
                .map(|(pos, _)| (pos.manhattan(tile), pos.y, pos.x))
                .min()
                .map(|(_, y, x)| TilePos::new(x, y));
            match node {
                Some(node) => {
                    intents.push(Intent::AssignHarvest { unit: id, node });
                    self.last_sent.push((id, node));
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
        intents.push(Intent::Scout {
            unit,
            to: self.passable_near(obs, to),
        });
    }

    /// Production channel: harvesters to target, then a sentinel drip
    /// from the Foundry; counters and lancers from the Fabricator.
    fn production(
        &mut self,
        dials: &Dials,
        obs: &Observation,
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
            } else if *budget >= UnitKind::Sentinel.stats().cost {
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
            .filter(|u| u.kind == UnitKind::Harvester)
            .count();
        if let Some((qi, fab)) = fabricator
            && obs.my_queues[qi].len() < 2
        {
            use crate::stats::{Domain, Role};
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
                && alive(aa_kind) + queued(aa_kind) < want_aa
                && *budget >= aa_kind.stats().cost
            {
                *budget -= aa_kind.stats().cost;
                intents.push(Intent::TrainAt {
                    building: fab.id,
                    kind: aa_kind,
                });
            } else if enemy_turrets > alive(UnitKind::Lancer) + queued(UnitKind::Lancer)
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
            {
                *budget -= scuttler;
                intents.push(Intent::TrainAt {
                    building: fab.id,
                    kind: UnitKind::Scuttler,
                });
            } else if dials.air_harass
                && alive(wing_kind) + queued(wing_kind) < AIR_WING
                && enemy_harvesters >= 2
                && *budget >= wing_kind.stats().cost + reserve
            {
                // A wing for the harvest line — bought only once raiding
                // has something to eat.
                *budget -= wing_kind.stats().cost;
                intents.push(Intent::TrainAt {
                    building: fab.id,
                    kind: wing_kind,
                });
            } else if *budget >= lancer + reserve {
                *budget -= lancer;
                intents.push(Intent::TrainAt {
                    building: fab.id,
                    kind: UnitKind::Lancer,
                });
            }
        }
    }

    /// Construction channel: orphaned sites first (paid-for progress must
    /// not strand), then the Fabricator, then a turret answer to raids.
    /// One build per think.
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

        let harvesters = obs
            .my_units
            .iter()
            .filter(|u| u.kind == UnitKind::Harvester)
            .count();
        if dials.tech {
            let fab_cost = BuildingKind::Fabricator
                .stats()
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
            let turret_cost = BuildingKind::Turret.stats().construction.map(|c| c.cost);
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

        // The sky over the economy: enemy air sighted (or blips inbound)
        // raises flak over the harvest line.
        if dials.aa_response && (self.seen_air || !obs.blips.is_empty()) {
            let flak_cost = BuildingKind::FlakTurret
                .stats()
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
            let array_cost = BuildingKind::Array.stats().construction.map(|c| c.cost);
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
            let rec_cost = BuildingKind::Reclaimer.stats().construction.map(|c| c.cost);
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
            .filter(|b| b.built && b.hp * 10 < b.kind.stats().max_hp * 8)
            // A building an own crew is stripping is being LIQUIDATED
            // on purpose — repair and salvage evict each other in the
            // sim, so a repair intent here would re-crew the teardown
            // and reverse it (the gym's lowering applies this same
            // filter).
            .filter(|b| !obs.my_units.iter().any(|u| u.salvaging == Some(b.id)))
            .map(|b| {
                let deficit = b.kind.stats().max_hp - b.hp;
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
    /// one more wave. Deliberately narrow: a scripted tier that sells
    /// its walls mid-siege teaches the learner the wrong lesson; one
    /// that converts dead weight into a late push teaches the right
    /// one, from the receiving side.
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
        force: bool,
        intents: &mut Vec<Intent>,
    ) {
        /// How far short of the objective a scout stops — inside a
        /// harvester's vision (6), and close enough to aggro (5) that
        /// the peek must rely on the scout's legs, not its armor.
        const STANDOFF: i32 = 5;

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
            // A harvester is the scout of choice: it outruns every
            // chaser that could kill it, so the peek costs a stretch of
            // income instead of a body. Pulling one off a node is fine —
            // the economy channel re-hires it after. Fighters are the
            // fallback, fastest first.
            self.scout = obs
                .my_units
                .iter()
                // A walking founder (`founding`) is spoken for like a
                // builder on site: a scout order would replace the
                // deferred claim's whole program.
                .filter(|u| !enlisted.contains(&u.id) && u.site.is_none() && u.founding.is_none())
                .filter(|u| u.kind == UnitKind::Harvester || u.idle)
                .min_by_key(|u| {
                    let preference = match u.kind {
                        UnitKind::Harvester => (0, u.carrying),
                        UnitKind::Scuttler => (1, 0),
                        UnitKind::Sentinel => (2, 0),
                        _ => (3, 0),
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
        intents.push(Intent::Scout {
            unit: scout,
            to: self.passable_near(obs, to),
        });
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
        let (margin_num, margin_den) = (8 - patience, 4u64);
        let gate_open = army_strength * margin_den >= enemy_strength.max(floor) * margin_num;

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
        // in our favor.
        if let Some(army) = staging_army
            && army.members.len() >= dials.army_size as usize
            && gate_open
            && let Some(target) = enemy_site
        {
            intents.push(Intent::PushArmy {
                army: army.id,
                target,
            });
        }
    }

    /// The nearest known enemy presence — buildings (ghosts included)
    /// before units — or None while the enemy is entirely unlocated.
    pub(super) fn enemy_site(obs: &Observation, home: TilePos) -> Option<TilePos> {
        obs.enemy_buildings
            .iter()
            .map(|b| (b.anchor.manhattan(home), b.anchor.y, b.anchor.x))
            .min()
            .map(|(_, y, x)| TilePos::new(x, y))
            .or_else(|| {
                obs.enemy_units
                    .iter()
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
        let (w, h) = kind.stats().size;
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
        let (w, h) = kind.stats().size;
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
        let claimed = obs.my_units.iter().any(|unit| {
            unit.founding.is_some_and(|(kind, anchor)| {
                let (width, height) = kind.stats().size;
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
        if self.rock_at(obs, t) || obs.known_scrap.iter().any(|(p, _)| *p == t) {
            return false;
        }
        let covered = |b: &super::observation::BuildingObs| {
            let (w, h) = b.kind.stats().size;
            t.x >= b.anchor.x && t.x < b.anchor.x + w && t.y >= b.anchor.y && t.y < b.anchor.y + h
        };
        !obs.my_buildings.iter().any(covered)
            && !obs.ally_buildings.iter().any(covered)
            && !obs.enemy_buildings.iter().any(covered)
    }

    fn rock_at(&self, obs: &Observation, t: TilePos) -> bool {
        obs.known_rock.contains(&t)
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

/// Convenience for tests and future tiers: whether a unit observation
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
