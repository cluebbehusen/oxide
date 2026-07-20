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

use super::executive::{ArmyState, Executive, Intent};
use super::observation::{Observation, UnitObs};
use crate::ids::UnitId;
use crate::stats::{BuildingKind, UnitKind};
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
    /// Tick of the last intel refresh toward a known enemy base.
    scouted_at: u64,
}

impl UtilityPolicy {
    /// Fresh policy, no memory.
    pub fn new() -> Self {
        Self::default()
    }

    /// One think: intents for this observation, in lowering order
    /// (economy, production, construction, scouting, army — scouts are
    /// claimed before the draft can grab them).
    pub fn think(&mut self, dials: &Dials, obs: &Observation, exec: &Executive) -> Vec<Intent> {
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
        let enlisted: Vec<UnitId> = exec.enlisted().collect();

        self.economy(obs, home_tile, &mut intents);
        self.production(dials, obs, &mut budget, &mut intents);
        self.construction(dials, obs, home_tile, &mut budget, &mut intents);
        // No scouting while the economy is short-handed: pulling one of
        // three starting harvesters off the line buys intel with the
        // opening — the most expensive scrap there is.
        let harvesters = obs
            .my_units
            .iter()
            .filter(|u| u.kind == UnitKind::Harvester)
            .count();
        if dials.scouting && harvesters >= dials.harvester_target as usize {
            self.scouting(obs, home_tile, &enlisted, &mut intents);
        }
        self.army(dials, obs, exec, home_tile, &enlisted, &mut intents);
        intents
    }

    /// A harvester sent last think and idle again now bounced off an
    /// unreachable node — never ask twice.
    fn audit_harvests(&mut self, obs: &Observation) {
        for (id, node) in std::mem::take(&mut self.last_sent) {
            let bounced = obs
                .my_units
                .iter()
                .any(|u| u.id == id && u.idle && u.hp > 0);
            if bounced && !self.dead_nodes.contains(&node) {
                self.dead_nodes.push(node);
            }
        }
    }

    /// A site requested last think that never appeared was refused for a
    /// reason the observation can't see; stop asking for that anchor.
    fn audit_sites(&mut self, obs: &Observation) {
        for anchor in std::mem::take(&mut self.pending_sites) {
            let appeared = obs.my_buildings.iter().any(|b| b.anchor == anchor);
            if !appeared && !self.dead_anchors.contains(&anchor) {
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
    fn economy(&mut self, obs: &Observation, home: TilePos, intents: &mut Vec<Intent>) {
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
            let lancer = UnitKind::Lancer.stats().cost;
            let scuttler = UnitKind::Scuttler.stats().cost;
            let reserve = UnitKind::Sentinel.stats().cost;
            if enemy_turrets > alive(UnitKind::Lancer) + queued(UnitKind::Lancer)
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
            }
        }
    }

    /// Scouting channel: keep the fog-honest observation fresh without
    /// running a death conveyor. While the enemy is unlocated, sweep
    /// standoff points (the mirror of home first — symmetric maps put
    /// the enemy there). Once a base is known, peek at it from vision
    /// range every so often; between refreshes the scout is released
    /// back to the draft pool. A scout looks — it never parks in the
    /// enemy's aggro.
    fn scouting(
        &mut self,
        obs: &Observation,
        home: TilePos,
        enlisted: &[UnitId],
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
        let due = known_base.is_none() || obs.tick.saturating_sub(self.scouted_at) >= SCOUT_REFRESH;

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
                .filter(|u| !enlisted.contains(&u.id) && u.site.is_none())
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
        exec: &Executive,
        home: TilePos,
        _enlisted: &[UnitId],
        intents: &mut Vec<Intent>,
    ) {
        // Rally: reinforce the army already staging (there is at most one
        // in practice — FormArmy merges on the same tile); a fresh rally
        // leans toward the enemy but stays within reach of home, where
        // the defender's advantage lives — a mid-map rally sits on the
        // enemy's march path and gets reinforcements killed piecemeal.
        let enemy_site = obs
            .enemy_buildings
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
            });
        let staging_army = exec
            .armies()
            .iter()
            .filter(|a| a.state == ArmyState::Staging)
            .min_by_key(|a| a.id);
        let rally = staging_army.map(|a| a.staging).unwrap_or_else(|| {
            let toward = enemy_site.unwrap_or(TilePos::new(obs.map_width / 2, obs.map_height / 2));
            let lean = |from: i32, to: i32| from + ((to - from) / 3).clamp(-3, 3);
            self.passable_near(
                obs,
                TilePos::new(lean(home.x, toward.x), lean(home.y, toward.y)),
            )
        });

        // Defense: an intruder near home turns every army on it. Fresh
        // fighters still muster at the rally — a body forms there and
        // joins whole; sending each spawn at the threat is the trickle.
        let intruder = obs
            .enemy_units
            .iter()
            .filter(|u| is_fighter(u))
            .map(|u| (u.tile.chebyshev(home), u.tile.y, u.tile.x, u.id))
            .filter(|(d, ..)| *d <= DEFENSE_RADIUS)
            .min()
            .map(|(_, y, x, _)| TilePos::new(x, y));
        if let Some(threat) = intruder {
            for army in exec.armies() {
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
        let atk = sentinel.attack.expect("sentinels fight");
        let sentinel_worth = u64::from(sentinel.max_hp)
            * (u64::from(atk.damage) * 100 / u64::from(atk.cooldown_ticks));
        let floor = if intel_fresh { 2 } else { 5 } * sentinel_worth;
        let gate_open = army_strength >= enemy_strength.max(floor) * 2;

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

    /// Nearest known scrap by (manhattan, y, x), skipping bounced nodes.
    fn nearest_scrap(&self, obs: &Observation, from: TilePos) -> Option<TilePos> {
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
    fn placement_near(
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
                    if self.dead_anchors.contains(&anchor) {
                        continue;
                    }
                    let in_bounds = |t: TilePos| {
                        t.x >= 0 && t.y >= 0 && t.x < obs.map_width && t.y < obs.map_height
                    };
                    let footprint_ok = (0..w).all(|fx| {
                        (0..h).all(|fy| {
                            let t = anchor.offset(fx, fy);
                            in_bounds(t) && self.tile_open(obs, t)
                        })
                    });
                    // A builder needs somewhere to stand.
                    let doorstep = (-1..=w).any(|fx| {
                        (-1..=h).any(|fy| {
                            let core = (0..w).contains(&fx) && (0..h).contains(&fy);
                            let t = anchor.offset(fx, fy);
                            !core && in_bounds(t) && self.tile_open(obs, t)
                        })
                    });
                    if footprint_ok && doorstep {
                        return Some(anchor);
                    }
                }
            }
        }
        None
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
        !obs.my_buildings.iter().any(covered) && !obs.enemy_buildings.iter().any(covered)
    }

    fn rock_at(&self, obs: &Observation, t: TilePos) -> bool {
        obs.known_rock.contains(&t)
    }

    /// The nearest known-open tile to `want` (spiral out to 3), for
    /// rally points that shouldn't sit inside a rock formation.
    fn passable_near(&self, obs: &Observation, want: TilePos) -> TilePos {
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
    u.kind.stats().attack.is_some()
}
