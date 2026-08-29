//! Air raids, scouting, and ground-army strategy.

use super::*;
use crate::bot::intelligence::MAX_CONFIDENCE;
use crate::bot::observation::BuildingObs;

const DEMONSTRATED_FORCE_RESERVE_PERCENT: u64 = 15;
/// A remembered force may shape longer-lived planning, but voluntary attack
/// timing uses one shared horizon. Otherwise retaining the same fact longer
/// can make a higher difficulty wait for an opening a lower difficulty takes.
const VOLUNTARY_FORCE_RISK_HORIZON: u64 = 1_800;

fn coherent_attack_size(dials: &Dials, player_facing: bool) -> usize {
    (dials.army_size as usize).saturating_add(usize::from(player_facing))
}

fn ground_capable_members(army: &Army, obs: &Observation) -> usize {
    obs.my_units
        .iter()
        .filter(|unit| army.members.contains(&unit.id))
        .filter(|unit| crate::bot::executive::unit_strength(unit) > 0)
        .count()
}

fn demonstrated_ground_strength(unit: &UnitObs) -> u64 {
    let stats = unit.kind.stats();
    let damage_per_100_ticks = stats
        .weapons
        .iter()
        .filter(|weapon| weapon.targets.covers(Domain::Ground))
        .map(|weapon| u64::from(weapon.damage) * 100 / u64::from(weapon.cooldown_ticks))
        .sum::<u64>();
    u64::from(stats.max_hp) * damage_per_100_ticks
}

fn utility_scout_preference(unit: &UnitObs, contested: bool) -> Option<(u8, u32)> {
    match unit.kind {
        UnitKind::Kestrel | UnitKind::Gnat => Some((0, 0)),
        UnitKind::Harvester if !contested => Some((1, unit.carrying)),
        UnitKind::Scuttler => Some((2, 0)),
        UnitKind::Sentinel => Some((3, 0)),
        _ => None,
    }
}

fn contested_region_visible(obs: &Observation, center: TilePos) -> bool {
    (-CONTESTED_RECON_RADIUS..=CONTESTED_RECON_RADIUS).all(|dy| {
        (-CONTESTED_RECON_RADIUS..=CONTESTED_RECON_RADIUS).all(|dx| {
            let tile = center.offset(dx, dy);
            tile.x < 0
                || tile.y < 0
                || tile.x >= obs.map_width
                || tile.y >= obs.map_height
                || obs.visible(tile)
        })
    })
}

impl UtilityPolicy {
    fn opponent_force_risk(&mut self, dials: &Dials, obs: &Observation) -> u64 {
        if self
            .opponent_force_peak
            .is_some_and(|(_, seen)| obs.tick.saturating_sub(seen) > dials.opponent_force_memory)
        {
            self.opponent_force_peak = None;
        }

        let observed = obs
            .enemy_units
            .iter()
            .map(demonstrated_ground_strength)
            .sum::<u64>();
        if observed > 0
            && self
                .opponent_force_peak
                .is_none_or(|(peak, _)| observed >= peak)
        {
            self.opponent_force_peak = Some((observed, obs.tick));
        }

        self.opponent_force_peak.map_or(0, |(strength, _)| {
            strength.saturating_mul(100 + DEMONSTRATED_FORCE_RESERVE_PERCENT) / 100
        })
    }

    fn voluntary_attack_force_risk(&mut self, dials: &Dials, obs: &Observation) -> u64 {
        let remembered = self.opponent_force_risk(dials, obs);
        let attack_horizon = dials
            .opponent_force_memory
            .min(VOLUNTARY_FORCE_RISK_HORIZON);
        if self
            .opponent_force_peak
            .is_some_and(|(_, seen)| obs.tick.saturating_sub(seen) <= attack_horizon)
        {
            remembered
        } else {
            0
        }
    }

    /// Air-raid channel: once a wing of idle ground-attack flyers has
    /// gathered, throw it at the enemy's harvest line — unless known
    /// anti-air stands over the target. Wings are spent, not managed:
    /// the raid is an attack-move and whatever comes back rejoins the
    /// idle pool.
    pub(super) fn air_raid(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        home: TilePos,
        enlisted: &[UnitId],
        reserved: &[UnitId],
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
                    && !reserved.contains(&u.id)
            })
            .count();
        if wings < dials.air_wing {
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
        contested_recon: Option<TilePos>,
        enlisted: &[UnitId],
        intents: &mut Vec<Intent>,
    ) {
        /// How far short of the objective a scout stops — inside a
        /// harvester's vision (6), and close enough to aggro (5) that
        /// the peek must rely on the scout's legs, not its armor.
        const STANDOFF: i32 = 5;

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
        let rear_side = |from: TilePos, objective: TilePos| -> TilePos {
            let (dx, dy) = (objective.x - from.x, objective.y - from.y);
            let distance = dx.abs().max(dy.abs());
            if distance == 0 {
                objective
            } else {
                TilePos::new(
                    objective.x + dx * STANDOFF / distance,
                    objective.y + dy * STANDOFF / distance,
                )
            }
        };

        let nearest = |foundries_only: bool| {
            obs.enemy_buildings
                .iter()
                .filter(|building| !foundries_only || building.kind == BuildingKind::Foundry)
                .map(|building| {
                    (
                        building.anchor.manhattan(home),
                        building.anchor.y,
                        building.anchor.x,
                    )
                })
                .min()
                .map(|(_, y, x)| TilePos::new(x, y))
        };
        let known_foundry = nearest(true);
        let known_base = known_foundry.or_else(|| nearest(false));
        let rear_recon_goal =
            known_foundry.map(|foundry| self.passable_near(obs, rear_side(home, foundry)));
        let foundry_current = known_foundry.is_some_and(|foundry| {
            obs.enemy_buildings.iter().any(|building| {
                building.kind == BuildingKind::Foundry
                    && building.anchor == foundry
                    && building.seen
            })
        });
        if foundry_current && rear_recon_goal.is_some_and(|goal| obs.visible(goal)) {
            self.scouted_at = obs.tick;
        }
        let due = contested_recon.is_some()
            || known_base.is_none()
            || obs.tick.saturating_sub(self.scout_sent_at) >= SCOUT_REFRESH;

        if let Some(id) = self.scout
            && !obs.my_units.iter().any(|u| u.id == id)
        {
            let dispatched_air_scout_lost =
                self.air_scout_needed && self.scout_dispatch.is_some_and(|(sent, _, _)| sent == id);
            self.scout = None;
            self.scout_dispatch = None;
            if dispatched_air_scout_lost {
                self.solo_air_scout_suspended = true;
                self.solo_air_scout_dark_since = None;
                self.solo_air_scout_retry_at = obs.tick.saturating_add(SOLO_SCOUT_RETRY_TICKS);
            }
        }
        if self.solo_air_scout_suspended {
            let actionable_enemy_sight = obs
                .enemy_units
                .iter()
                .any(|unit| unit.kind.role() != crate::stats::Role::Scout)
                || obs.enemy_buildings.iter().any(|building| building.seen);
            if actionable_enemy_sight {
                if self.solo_air_scout_dark_since.is_some() {
                    self.solo_air_scout_suspended = false;
                    self.solo_air_scout_dark_since = None;
                }
            } else {
                let dark_since = *self.solo_air_scout_dark_since.get_or_insert(obs.tick);
                if obs.tick >= self.solo_air_scout_retry_at
                    && obs.tick.saturating_sub(dark_since) >= SOLO_SCOUT_QUIET_TICKS
                {
                    self.solo_air_scout_suspended = false;
                    self.solo_air_scout_dark_since = None;
                }
            }
        }
        if self.solo_air_scout_suspended {
            return;
        }
        if let Some(id) = self.scout
            && let Some(unit) = obs.my_units.iter().find(|unit| unit.id == id)
            && utility_scout_preference(unit, contested_recon.is_some()).is_none()
        {
            let completed = contested_recon.is_some_and(|region| {
                let recon_goal = self.passable_near(obs, region);
                unit.idle
                    && self
                        .scout_dispatch
                        .is_some_and(|(sent, _, prior)| sent == id && prior == recon_goal)
                    && contested_region_visible(obs, region)
            });
            self.scout = None;
            self.scout_dispatch = None;
            if completed {
                return;
            }
        }
        if let Some(id) = self.scout
            && let Some(unit) = obs.my_units.iter().find(|unit| unit.id == id)
            && unit.idle
            && unit.kind.stats().domain == Domain::Ground
            && let Some((sent, from, to)) = self.scout_dispatch
            && sent == id
            && from.chebyshev(to) > 1
            && unit.tile.chebyshev(from) <= 1
        {
            // A ground Move with no route goes idle where it started.
            // Stop cycling the same island shoreline and ask production
            // for the faction's dedicated scout flyer.
            self.scout = None;
            self.scout_dispatch = None;
            self.air_scout_needed = true;
        }
        if !due {
            // Between sweeps the scout goes back in the pool.
            if let Some(id) = self.scout
                && obs.my_units.iter().any(|u| u.id == id && u.idle)
            {
                self.scout = None;
                self.scout_dispatch = None;
            }
            return;
        }
        let picked_now = self.scout.is_none();
        if self.scout.is_none() {
            // A scout-role flyer is the scout of choice: unarmed, wide
            // eyes, and able to cross pits and gulfs. An ordinary sweep may
            // borrow a Harvester; contested work never does. Only the two
            // cheap ground skirmishers are generic combat fallbacks, so this
            // channel cannot consume a strategic or support specialist.
            self.scout = obs
                .my_units
                .iter()
                // A walking founder (`founding`) is spoken for like a
                // builder on site: a scout order would replace the
                // deferred claim's whole program.
                .filter(|u| u.site.is_none() && u.founding.is_none())
                .filter(|u| !enlisted.contains(&u.id))
                .filter(|u| u.kind.stats().harvest.is_some() || u.idle)
                .filter_map(|u| {
                    let preference = utility_scout_preference(u, contested_recon.is_some())?;
                    (!self.air_scout_needed || u.kind.role() == crate::stats::Role::Scout)
                        .then_some((preference, u.id))
                })
                .min()
                .map(|(_, id)| id);
            if self.scout.is_none() && contested_recon.is_some() {
                self.air_scout_needed = true;
            }
        }
        let Some(scout) = self.scout else { return };
        // A fresh pick is dispatched immediately (a working harvester is
        // not idle); an existing scout gets its next leg only once the
        // current one completes.
        if !picked_now && !obs.my_units.iter().any(|u| u.id == scout && u.idle) {
            return;
        }

        let member = obs
            .my_units
            .iter()
            .find(|unit| unit.id == scout)
            .expect("the selected scout came from this observation");
        let to = if let Some(region) = contested_recon {
            // Unlike an ordinary base peek, a quarantined salvage region
            // needs full current sight before work can resume. Never spend a
            // Harvester on this probe: an armed or dedicated scout supplies
            // the negative evidence without recreating the worker conveyor.
            region
        } else if let Some(base) = known_base {
            self.scout_sent_at = obs.tick;
            if member.kind.role() == crate::stats::Role::Scout
                && member.kind.stats().domain == Domain::Air
            {
                rear_recon_goal.unwrap_or_else(|| rear_side(home, base))
            } else {
                standoff(home, base)
            }
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
        if !picked_now
            && member.idle
            && self
                .scout_dispatch
                .is_some_and(|(sent, _, prior)| sent == scout && prior == to)
            && contested_recon.map_or_else(
                || obs.visible(to),
                |region| contested_region_visible(obs, region),
            )
        {
            return;
        }
        let from = member.tile;
        self.scout_dispatch = Some((scout, from, to));
        intents.push(Intent::Scout { unit: scout, to });
    }

    /// Army channel: an intruder near home turns every army on it;
    /// otherwise keep feeding the staging army and commit it when it
    /// reaches size.
    pub(super) fn army(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        armies: &[Army],
        home: TilePos,
        mode: PolicyMode<'_>,
        intents: &mut Vec<Intent>,
    ) {
        let player_facing = mode.player_facing;
        let opponent_force_risk = if player_facing {
            self.voluntary_attack_force_risk(dials, obs)
        } else {
            0
        };
        let enemy_site = Self::enemy_site(obs, home);
        let staging_army = armies
            .iter()
            .filter(|army| {
                // A player-facing staged body with a target still owns a live
                // offensive order. New production must gather separately.
                army.state == ArmyState::Staging && (!player_facing || army.target.is_none())
            })
            .min_by_key(|a| a.id);
        let rally = self.rally_point(obs, staging_army, enemy_site, home, player_facing);

        // Defense: an intruder near any own or allied Foundry turns every
        // army on it. The profile-free Overseer retains its historical
        // home-and-allies base set. Fresh fighters still muster at the rally
        // rather than trickling into the threat one spawn at a time.
        let bases: Vec<TilePos> = std::iter::once(home)
            .chain(
                obs.my_buildings
                    .iter()
                    .filter(|b| {
                        player_facing
                            && b.kind == BuildingKind::Foundry
                            && b.built
                            && b.hp > 0
                            && b.anchor != home
                    })
                    .map(|b| b.anchor),
            )
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
            .filter(|u| !player_facing || obs.visible(u.tile))
            .filter_map(|u| {
                let (distance, base_y, base_x) = bases
                    .iter()
                    .map(|base| (u.tile.chebyshev(*base), base.y, base.x))
                    .min()?;
                (distance <= DEFENSE_RADIUS)
                    .then_some((distance, u.tile.y, u.tile.x, u.id, base_y, base_x))
            })
            .min()
            .map(|(_, y, x, _, base_y, base_x)| (TilePos::new(x, y), TilePos::new(base_x, base_y)));
        if let Some((threat, threatened_base)) = intruder {
            let coherent_size = coherent_attack_size(dials, player_facing);
            for army in armies {
                // Maintenance has already committed a withdrawing body to its
                // retreat this think. Re-pushing it here would emit a second
                // queue-replacing order for the same members and erase the
                // retreat before it can begin. The frozen Overseer keeps its
                // historical retargeting behavior.
                if player_facing && army.state == ArmyState::Withdrawing {
                    continue;
                }
                if player_facing
                    && army.state == ArmyState::Staging
                    && army.staging.chebyshev(threat) <= 2
                {
                    continue;
                }
                let local_counterattack = player_facing
                    && army.state == ArmyState::Staging
                    && crate::bot::executive::locally_overmatches_near(
                        obs,
                        &army.members,
                        threat,
                        DEFENSE_RADIUS,
                    );
                // Fresh production must not chase a remote intruder one
                // spawn at a time. A staged body already in contact holds
                // its ground through auto-acquire above; crossing the map to
                // defend another base requires the same coherent muster as
                // an offensive march.
                if player_facing
                    && army.state == ArmyState::Staging
                    && ground_capable_members(army, obs) < coherent_size
                    && !local_counterattack
                {
                    continue;
                }
                if player_facing
                    && army.state == ArmyState::Staging
                    && crate::bot::executive::catastrophically_outmatched_near(
                        obs,
                        &army.members,
                        threat,
                        DEFENSE_RADIUS,
                    )
                {
                    continue;
                }
                // Re-target only when the threat has really moved: churning
                // fresh attack-moves every think as it shifts a tile keeps
                // interrupting members mid-swing — auto-acquire handles
                // the last few tiles better than micromanagement does.
                if should_march(player_facing, army, threat)
                    && (!player_facing || self.army_reaches(obs, army, threat))
                {
                    intents.push(Intent::PushArmy {
                        army: army.id,
                        target: threat,
                    });
                }
            }
            let defensive_rally = if player_facing {
                self.durable_rally_near(obs, threatened_base)
            } else {
                rally
            };
            intents.push(Intent::FormArmy {
                staging: defensive_rally,
                size: u32::try_from(coherent_size).unwrap_or(u32::MAX),
            });
            return;
        }

        if !mode.admit_voluntary_macro {
            return;
        }

        // The push gate: an offensive march is the hardest fight there is
        // — the whole approach is time the defender spends producing, and
        // the fight lands on their turf with their turrets. Compare the
        // army with the force defending this objective, not every separated
        // hostile cluster on the map. Until the gate opens, keep raising the
        // draft target so the army outgrows the threshold instead of
        // trickling into a fair fight.
        let army_strength: u64 = staging_army
            .map(|army| crate::bot::executive::marching_strength(army, obs))
            .unwrap_or(0)
            .saturating_mul(u64::from(dials.own_strength_scale))
            / 10_000;
        let enemy_strength = enemy_site
            .map(|target| {
                obs.enemy_units
                    .iter()
                    .filter(|unit| unit.tile.chebyshev(target) <= DEFENSE_RADIUS)
                    .map(crate::bot::executive::unit_strength)
                    .sum::<u64>()
                    + obs
                        .enemy_buildings
                        .iter()
                        .filter(|building| building.anchor.chebyshev(target) <= DEFENSE_RADIUS)
                        .map(|building| objective_building_strength(building, mode, obs.tick))
                        .sum::<u64>()
            })
            .unwrap_or(0)
            .max(opponent_force_risk)
            .saturating_mul(u64::from(dials.enemy_strength_scale))
            / 10_000;
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
        let floor = (if intel_fresh { 3 } else { 6 }) * sentinel_worth;
        // Patience decays the demanded margin from 2.0× down to 1.0×
        // over the match: two flawless defenders would otherwise wait
        // forever for an edge neither can get, and a fair fight taken
        // late beats a stalemate never resolved.
        let patience = (obs.tick / 4000).min(4);
        // A stalled economy cannot wait for an advantage it has no income
        // to buy. Desperation lowers the margin to an even fight and the
        // blind-mass floor to one Sentinel so scarcity ends the match.
        let desperate = self.desperate;
        let (margin_num, margin_den) = if desperate {
            (4, 4u64)
        } else {
            (8 - patience, 4u64)
        };
        let commit_floor = if desperate { sentinel_worth } else { floor };
        let gate_open = army_strength * margin_den >= enemy_strength.max(commit_floor) * margin_num;

        let members = staging_army.map(|a| a.members.len()).unwrap_or(0);
        let coherent_size = coherent_attack_size(dials, player_facing);
        let target_size = if gate_open && members >= coherent_size {
            dials.army_size.max(members as u32)
        } else {
            dials
                .army_size
                .max(u32::try_from(coherent_size).unwrap_or(u32::MAX))
                .max(u32::try_from(members).unwrap_or(u32::MAX).saturating_add(2))
        };
        intents.push(Intent::FormArmy {
            staging: rally,
            size: target_size,
        });

        // Commit: numbers met and the fight is expected to be unfair —
        // in our favor. Desperation may relax the odds, but a player-facing
        // body still musters one reserve machine beyond its configured line;
        // otherwise a faster thinker launches the instant the bare minimum
        // appears while a slower thinker naturally gathers the next spawn.
        // The profile-free controller retains its historical last-machine
        // liveness push. When the enemy was never found, home's mirror is the
        // one guess a symmetric quarry offers; the player-facing route gate
        // still requires an explored approach.
        if let Some(army) = staging_army
            && (army.members.len() >= coherent_size
                || (!player_facing && desperate && !army.members.is_empty()))
            && gate_open
            && let Some(target) = enemy_site.or_else(|| {
                (desperate && self.desperate_march).then(|| {
                    self.passable_near(
                        obs,
                        TilePos::new(obs.map_width - 1 - home.x, obs.map_height - 1 - home.y),
                    )
                })
            })
            && should_march(player_facing, army, target)
            && (!player_facing || self.army_reaches(obs, army, target))
        {
            intents.push(Intent::PushArmy {
                army: army.id,
                target,
            });
        }
    }

    /// An offensive ground order is meaningful only when every surviving
    /// member can reach the objective along explored ground. A ground unit
    /// that walked to its current component leaves an explored corridor; one
    /// ferried onto an island leaves no imaginary road across the intervening
    /// fog. This suppresses island-crossing command storms while still
    /// allowing a landed squad to push locally.
    fn army_reaches(&self, obs: &Observation, army: &Army, target: TilePos) -> bool {
        let mut members: Vec<_> = obs
            .my_units
            .iter()
            .filter(|unit| army.members.contains(&unit.id))
            .collect();
        members.sort_unstable_by_key(|unit| unit.id);
        let Some(goals) = self.ground_attack_goals(obs, target, members.len()) else {
            return false;
        };
        let mut routes = crate::bot::routing::RouteProjection::known_ground(obs);
        !members.is_empty()
            && members
                .iter()
                .zip(goals)
                .all(|(unit, goal)| routes.unit_reaches(unit, goal))
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
    /// a fresh point screening the nearest reachable forward Foundry. Without
    /// that screen, a body which correctly refuses a risky attack can still
    /// leave its expansion undefended until the attacker is already on the
    /// footprint. An island expansion is not a ground rally: if no forward
    /// Foundry has a known route from home, gather near home instead. A
    /// mid-map rally sits on the enemy's march path and gets reinforcements
    /// killed piecemeal.
    fn rally_point(
        &self,
        obs: &Observation,
        staging_army: Option<&Army>,
        enemy_site: Option<TilePos>,
        home: TilePos,
        player_facing: bool,
    ) -> TilePos {
        let desired = staging_army.map(|army| army.staging).unwrap_or_else(|| {
            let toward = enemy_site.unwrap_or(TilePos::new(obs.map_width / 2, obs.map_height / 2));
            if player_facing
                && let Some(frontline) = enemy_site.and_then(|enemy| {
                    let mut routes = RouteProjection::known_ground(obs);
                    obs.my_buildings
                        .iter()
                        .filter(|building| {
                            building.kind == BuildingKind::Foundry
                                && building.built
                                && building.hp > 0
                                && building.anchor != home
                        })
                        .map(|building| {
                            let anchor = building.anchor;
                            let behind = TilePos::new(
                                anchor.x + (home.x - anchor.x).signum() * 2,
                                anchor.y + (home.y - anchor.y).signum() * 2,
                            );
                            let rally = self.durable_rally_near(obs, behind);
                            (anchor.chebyshev(enemy), anchor.y, anchor.x, rally)
                        })
                        .filter(|(_, _, _, rally)| routes.reaches(home, *rally))
                        .min_by_key(|(distance, y, x, _)| (*distance, *y, *x))
                        .map(|(_, _, _, rally)| rally)
                })
            {
                return frontline;
            }
            let lean = |from: i32, to: i32| from + ((to - from) / 3).clamp(-3, 3);
            TilePos::new(lean(home.x, toward.x), lean(home.y, toward.y))
        });
        if player_facing {
            self.durable_rally_near(obs, desired)
        } else if staging_army.is_some() {
            desired
        } else {
            self.passable_near(obs, desired)
        }
    }
}

fn should_march(player_facing: bool, army: &Army, target: TilePos) -> bool {
    if !player_facing {
        return army
            .target
            .is_none_or(|current| current.chebyshev(target) > 4);
    }
    match army.state {
        ArmyState::Staging => army.target != Some(target),
        ArmyState::Pushing => army
            .target
            .is_none_or(|current| current.chebyshev(target) > 4),
        ArmyState::Engaging | ArmyState::Withdrawing => false,
    }
}

fn objective_building_strength(building: &BuildingObs, mode: PolicyMode<'_>, now: u64) -> u64 {
    if !mode.player_facing {
        return crate::bot::executive::building_strength(building);
    }

    let contact = (!building.seen)
        .then(|| {
            mode.building_contacts.and_then(|contacts| {
                contacts.iter().find(|contact| {
                    contact.player == building.player
                        && contact.anchor == building.anchor
                        && contact.kind == building.kind
                })
            })
        })
        .flatten();
    let tier = contact.map_or(building.tier, |contact| contact.tier);
    let stats = building.kind.tier_stats(tier);
    let strength = if building.built {
        let damage_per_100: u64 = stats
            .weapons
            .iter()
            .filter(|weapon| weapon.targets.ground)
            .map(|weapon| u64::from(weapon.damage) * 100 / u64::from(weapon.cooldown_ticks))
            .sum();
        u64::from(building.hp) * damage_per_100
    } else {
        0
    };
    if building.seen {
        return strength;
    }

    // A ghost first encountered between this brain's think ticks has no
    // timestamp. Its age is unknown, not ancient, so retain full strength
    // until the controller has a real sighting from which confidence can age.
    let confidence = contact.map_or(MAX_CONFIDENCE, |contact| {
        contact
            .last_seen
            .map_or(MAX_CONFIDENCE, |_| contact.confidence_at(now))
    });
    strength.saturating_mul(u64::from(confidence)) / u64::from(MAX_CONFIDENCE)
}

/// Convenience for tests and policies: whether a unit observation
/// can fight.
fn is_fighter(u: &UnitObs) -> bool {
    u.kind.stats().can_fight()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::executive::ArmyId;
    use crate::bot::intelligence::StrategicIntelligence;
    use crate::bot::observation::{BuildingObs, OBSERVATION_VERSION};
    use crate::ids::{BuildingId, PlayerId};
    use crate::scenario::{BotConfig, BotDifficulty, BotStance};
    use crate::state::Faction;

    fn fighter(id: u32, tile: TilePos) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(0),
            kind: UnitKind::Lancer,
            tile,
            hp: UnitKind::Lancer.stats().max_hp,
            idle: true,
            carrying: 0,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        }
    }

    fn bombard(id: u32, tile: TilePos) -> UnitObs {
        UnitObs {
            kind: UnitKind::Bombard,
            hp: UnitKind::Bombard.stats().max_hp,
            ..fighter(id, tile)
        }
    }

    fn sentinel(id: u32, tile: TilePos) -> UnitObs {
        UnitObs {
            kind: UnitKind::Sentinel,
            hp: UnitKind::Sentinel.stats().max_hp,
            ..fighter(id, tile)
        }
    }

    fn hostile(id: u32, kind: UnitKind, tile: TilePos) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(1),
            kind,
            tile,
            hp: kind.stats().max_hp,
            idle: true,
            carrying: 0,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        }
    }

    fn own(id: u32, kind: UnitKind, tile: TilePos) -> UnitObs {
        UnitObs {
            player: PlayerId(0),
            kind,
            hp: kind.stats().max_hp,
            ..fighter(id, tile)
        }
    }

    fn set_region_visible(obs: &mut Observation, center: TilePos, visible: bool) {
        for dy in -CONTESTED_RECON_RADIUS..=CONTESTED_RECON_RADIUS {
            for dx in -CONTESTED_RECON_RADIUS..=CONTESTED_RECON_RADIUS {
                let tile = center.offset(dx, dy);
                if tile.x >= 0 && tile.y >= 0 && tile.x < obs.map_width && tile.y < obs.map_height {
                    let index = usize::try_from(tile.y * obs.map_width + tile.x).unwrap();
                    obs.visible[index] = visible;
                }
            }
        }
    }

    fn own_foundry(id: u32, anchor: TilePos) -> BuildingObs {
        BuildingObs {
            id: BuildingId(id),
            player: PlayerId(0),
            kind: BuildingKind::Foundry,
            anchor,
            hp: BuildingKind::Foundry.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        }
    }

    fn defense(id: u32, kind: BuildingKind, anchor: TilePos) -> BuildingObs {
        BuildingObs {
            id: BuildingId(id),
            player: PlayerId(1),
            kind,
            anchor,
            hp: kind.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        }
    }

    fn offensive_position(remote_defense: TilePos) -> (Observation, Army) {
        let units = vec![
            fighter(1, TilePos::new(5, 6)),
            fighter(2, TilePos::new(5, 7)),
            fighter(3, TilePos::new(6, 6)),
            fighter(4, TilePos::new(6, 7)),
        ];
        let army = Army {
            id: ArmyId(7),
            members: units.iter().map(|unit| unit.id).collect(),
            state: ArmyState::Staging,
            staging: TilePos::new(6, 6),
            target: None,
            focus: None,
            progress: None,
            issued: None,
            bounces: 0,
        };
        let obs = Observation {
            version: OBSERVATION_VERSION,
            tick: 20_000,
            me: PlayerId(0),
            scrap: 0,
            map_width: 40,
            map_height: 24,
            my_units: units,
            my_buildings: Vec::new(),
            my_queues: Vec::new(),
            ally_units: Vec::new(),
            ally_buildings: Vec::new(),
            enemy_units: Vec::new(),
            enemy_buildings: vec![
                defense(20, BuildingKind::Turret, TilePos::new(12, 6)),
                defense(21, BuildingKind::Bastion, remote_defense),
            ],
            visible: vec![true; 40 * 24],
            explored: vec![true; 40 * 24],
            known_scrap: Vec::new(),
            known_rock: Vec::new(),
            known_frames: Vec::new(),
            known_peaks: Vec::new(),
            known_wrecks: Vec::new(),
            salvage_incidents: Vec::new(),
            blips: Vec::new(),
            faction: Faction::Ferrous,
            my_shells: 0,
            incoming_shells: Vec::new(),
        };
        (obs, army)
    }

    fn push_target(intents: &[Intent]) -> Option<TilePos> {
        intents.iter().find_map(|intent| match intent {
            Intent::PushArmy { target, .. } => Some(*target),
            _ => None,
        })
    }

    fn hide_enemy_buildings(obs: &mut Observation) {
        obs.visible.fill(false);
        for building in &mut obs.enemy_buildings {
            building.id = BuildingId(u32::MAX);
            building.seen = false;
        }
    }

    fn player_mode(building_contacts: Option<&[BuildingContact]>) -> PolicyMode<'_> {
        PolicyMode {
            player_facing: true,
            admit_voluntary_macro: true,
            unit_contacts: None,
            building_contacts,
        }
    }

    fn profile_free_mode() -> PolicyMode<'static> {
        PolicyMode {
            player_facing: false,
            admit_voluntary_macro: true,
            unit_contacts: None,
            building_contacts: None,
        }
    }

    #[test]
    fn base_recon_prefers_foundries_and_only_confirmed_sight_refreshes_intel() {
        let home = TilePos::new(4, 12);
        let extractor = TilePos::new(16, 12);
        let foundry = TilePos::new(28, 12);
        let (mut obs, _) = offensive_position(foundry);
        obs.tick = 2_000;
        obs.visible.fill(false);
        obs.enemy_buildings = vec![
            defense(20, BuildingKind::Extractor, extractor),
            defense(21, BuildingKind::Foundry, foundry),
        ];
        for building in &mut obs.enemy_buildings {
            building.seen = false;
        }
        obs.my_units = vec![UnitObs {
            kind: UnitKind::Kestrel,
            hp: UnitKind::Kestrel.stats().max_hp,
            ..fighter(1, home)
        }];
        let mut air_policy = UtilityPolicy::new();
        let mut air_intents = Vec::new();

        air_policy.scouting(&obs, home, None, &[], &mut air_intents);

        assert_eq!(
            air_intents,
            vec![Intent::Scout {
                unit: UnitId(1),
                to: TilePos::new(33, 12),
            }],
            "a dedicated flyer should look through the Foundry to its defended rear, not stop at the nearer Extractor"
        );
        assert_eq!(air_policy.scout_sent_at, obs.tick);
        assert_eq!(
            air_policy.scouted_at, 0,
            "issuing a recon order is not proof that the base was observed"
        );

        let mut ground_obs = obs.clone();
        ground_obs.my_units = vec![UnitObs {
            kind: UnitKind::Harvester,
            hp: UnitKind::Harvester.stats().max_hp,
            ..fighter(2, home)
        }];
        let mut ground_policy = UtilityPolicy::new();
        let mut ground_intents = Vec::new();
        ground_policy.scouting(&ground_obs, home, None, &[], &mut ground_intents);
        assert_eq!(
            ground_intents,
            vec![Intent::Scout {
                unit: UnitId(2),
                to: TilePos::new(23, 12),
            }],
            "a ground fallback scout should keep the safe near-side standoff"
        );

        obs.tick += 50;
        obs.my_units[0].tile = TilePos::new(33, 12);
        obs.enemy_buildings[0].seen = true;
        air_intents.clear();
        air_policy.scouting(&obs, home, None, &[], &mut air_intents);
        assert_eq!(
            air_policy.scouted_at, 0,
            "seeing only the economic outpost must not certify the enemy base"
        );

        obs.tick += 1;
        obs.enemy_buildings[1].seen = true;
        air_policy.scouting(&obs, home, None, &[], &mut air_intents);
        assert_eq!(
            air_policy.scouted_at, 0,
            "seeing the Foundry face while its rear defensive sample stays dark is incomplete intelligence"
        );

        obs.tick += 1;
        let rear_sample = TilePos::new(33, 12);
        let rear_index = usize::try_from(rear_sample.y * obs.map_width + rear_sample.x).unwrap();
        obs.visible[rear_index] = true;
        air_policy.scouting(&obs, home, None, &[], &mut air_intents);
        assert_eq!(air_policy.scouted_at, obs.tick);
    }

    #[test]
    fn contested_region_recon_uses_a_non_worker_and_looks_at_the_whole_region() {
        let home = TilePos::new(3, 12);
        let region = TilePos::new(24, 12);
        let (mut obs, _) = offensive_position(TilePos::new(32, 18));
        obs.my_units = vec![
            UnitObs {
                kind: UnitKind::Harvester,
                hp: UnitKind::Harvester.stats().max_hp,
                ..fighter(1, TilePos::new(4, 12))
            },
            UnitObs {
                kind: UnitKind::Kestrel,
                hp: UnitKind::Kestrel.stats().max_hp,
                ..fighter(2, TilePos::new(5, 12))
            },
        ];
        let mut policy = UtilityPolicy::new();
        let mut intents = Vec::new();

        policy.scouting(&obs, home, Some(region), &[], &mut intents);

        assert_eq!(
            intents,
            vec![Intent::Scout {
                unit: UnitId(2),
                to: region,
            }],
            "a quarantined region needs direct current sight, without sacrificing another Harvester"
        );

        obs.my_units[1].tile = region;
        intents.clear();
        policy.scouting(&obs, home, Some(region), &[], &mut intents);
        assert!(
            intents.is_empty(),
            "an idle scout already watching the quarantined region must hold instead of receiving the same Move every cadence"
        );
    }

    #[test]
    fn contested_region_recon_releases_an_already_latched_worker_scout() {
        let home = TilePos::new(3, 12);
        let region = TilePos::new(24, 12);
        let (mut obs, _) = offensive_position(TilePos::new(32, 18));
        obs.my_units = vec![
            UnitObs {
                kind: UnitKind::Harvester,
                hp: UnitKind::Harvester.stats().max_hp,
                ..fighter(1, TilePos::new(4, 12))
            },
            UnitObs {
                kind: UnitKind::Kestrel,
                hp: UnitKind::Kestrel.stats().max_hp,
                ..fighter(2, TilePos::new(5, 12))
            },
        ];
        let mut policy = UtilityPolicy::new();
        policy.scout = Some(UnitId(1));
        policy.scout_dispatch = Some((UnitId(1), home, TilePos::new(20, 8)));
        let mut intents = Vec::new();

        policy.scouting(&obs, home, Some(region), &[], &mut intents);

        assert_eq!(policy.scout, Some(UnitId(2)));
        assert_eq!(
            intents,
            vec![Intent::Scout {
                unit: UnitId(2),
                to: region,
            }],
            "hazard reconnaissance must not inherit an ordinary Harvester scout"
        );
    }

    #[test]
    fn scout_fallbacks_are_an_explicit_role_allowlist() {
        for kind in UnitKind::ALL {
            let unit = own(1, kind, TilePos::new(4, 4));
            let ordinary_allowed = matches!(
                kind,
                UnitKind::Kestrel
                    | UnitKind::Gnat
                    | UnitKind::Harvester
                    | UnitKind::Scuttler
                    | UnitKind::Sentinel
            );
            let contested_allowed = matches!(
                kind,
                UnitKind::Kestrel | UnitKind::Gnat | UnitKind::Scuttler | UnitKind::Sentinel
            );

            assert_eq!(
                utility_scout_preference(&unit, false).is_some(),
                ordinary_allowed,
                "ordinary reconnaissance eligibility drifted for {kind:?}"
            );
            assert_eq!(
                utility_scout_preference(&unit, true).is_some(),
                contested_allowed,
                "contested reconnaissance eligibility drifted for {kind:?}"
            );
        }
    }

    #[test]
    fn accepted_turn_limited_bomber_terminal_releases_without_repeating_the_move() {
        let home = TilePos::new(3, 12);
        let region = TilePos::new(24, 12);
        let terminal = region.offset(-3, 0);
        let (mut obs, _) = offensive_position(TilePos::new(32, 18));
        obs.visible.fill(false);
        set_region_visible(&mut obs, region, true);
        obs.my_units = vec![
            own(1, UnitKind::Moth, terminal),
            own(2, UnitKind::Sentinel, home.offset(1, 0)),
        ];
        let mut policy = UtilityPolicy::new();
        policy.scout = Some(UnitId(1));
        policy.scout_dispatch = Some((UnitId(1), home, region));
        let mut intents = Vec::new();

        policy.scouting(&obs, home, Some(region), &[], &mut intents);

        assert!(
            intents.is_empty(),
            "the accepted terminal position already supplies the requested sight: {intents:?}"
        );
        assert_eq!(
            policy.scout, None,
            "the bomber must return to its real role"
        );
        assert_eq!(policy.scout_dispatch, None);
        assert!(
            !policy.air_scout_needed,
            "completed reconnaissance must not manufacture another scout demand"
        );
    }

    #[test]
    fn contested_recon_chooses_a_cheap_skirmisher_over_a_strategic_bomber() {
        let home = TilePos::new(3, 12);
        let region = TilePos::new(24, 12);
        let (mut obs, _) = offensive_position(TilePos::new(32, 18));
        obs.visible.fill(false);
        obs.my_units = vec![
            own(1, UnitKind::Moth, home.offset(1, 0)),
            own(2, UnitKind::Sentinel, home.offset(0, 1)),
        ];
        let mut policy = UtilityPolicy::new();
        let mut intents = Vec::new();

        policy.scouting(&obs, home, Some(region), &[], &mut intents);

        assert_eq!(policy.scout, Some(UnitId(2)));
        assert_eq!(
            intents,
            vec![Intent::Scout {
                unit: UnitId(2),
                to: region,
            }],
            "strategic airframes must remain available for their planned operation"
        );
    }

    #[test]
    fn contested_recon_without_an_allowed_body_waits_and_funds_a_dedicated_scout() {
        let home = TilePos::new(3, 12);
        let region = TilePos::new(24, 12);
        let (mut obs, _) = offensive_position(TilePos::new(32, 18));
        obs.visible.fill(false);
        obs.my_units = [
            UnitKind::Harvester,
            UnitKind::Moth,
            UnitKind::Skyhook,
            UnitKind::Bombard,
            UnitKind::Tender,
            UnitKind::Stinger,
            UnitKind::Avalanche,
        ]
        .into_iter()
        .enumerate()
        .map(|(index, kind)| own(u32::try_from(index + 1).unwrap(), kind, home.offset(1, 0)))
        .collect();
        let mut policy = UtilityPolicy::new();
        let mut intents = Vec::new();

        policy.scouting(&obs, home, Some(region), &[], &mut intents);

        assert!(
            intents.is_empty(),
            "no specialist should be drafted: {intents:?}"
        );
        assert_eq!(policy.scout, None);
        assert!(
            policy.air_scout_needed,
            "the existing production path needs this signal to fund the faction scout"
        );
    }

    #[test]
    fn contested_ground_scout_still_reports_a_no_route_terminal() {
        let home = TilePos::new(3, 12);
        let region = TilePos::new(24, 12);
        let start = home.offset(1, 0);
        let (mut obs, _) = offensive_position(TilePos::new(32, 18));
        obs.visible.fill(false);
        obs.my_units = vec![own(1, UnitKind::Scuttler, start)];
        let mut policy = UtilityPolicy::new();
        let mut intents = Vec::new();

        policy.scouting(&obs, home, Some(region), &[], &mut intents);
        assert_eq!(
            intents,
            vec![Intent::Scout {
                unit: UnitId(1),
                to: region,
            }]
        );

        intents.clear();
        policy.scouting(&obs, home, Some(region), &[], &mut intents);

        assert!(intents.is_empty());
        assert_eq!(policy.scout, None);
        assert_eq!(policy.scout_dispatch, None);
        assert!(
            policy.air_scout_needed,
            "an idle ground scout still at its dispatch origin proves this look must fly"
        );
    }

    #[test]
    fn player_facing_rallies_do_not_park_on_known_extractor_frames() {
        let frame = TilePos::new(6, 6);
        let (mut obs, mut army) = offensive_position(TilePos::new(30, 18));
        obs.known_frames = vec![frame];
        army.staging = frame;
        let policy = UtilityPolicy::new();

        let player_rally = policy.rally_point(&obs, Some(&army), None, frame, true);
        assert!(
            player_rally.x < frame.x
                || player_rally.x >= frame.x + 2
                || player_rally.y < frame.y
                || player_rally.y >= frame.y + 2,
            "a durable army rally must not be consumed by later restoration: {player_rally:?}"
        );
        assert_eq!(
            policy.rally_point(&obs, Some(&army), None, frame, false),
            frame,
            "the frozen profile-free controller retains its exact staging behavior"
        );
    }

    #[test]
    fn fresh_armies_screen_a_reachable_forward_foundry_but_not_an_island_expansion() {
        let home = TilePos::new(4, 4);
        let expansion = TilePos::new(18, 10);
        let enemy = TilePos::new(34, 18);
        let (mut connected, _) = offensive_position(enemy);
        connected.my_buildings = vec![own_foundry(0, home), own_foundry(1, expansion)];
        let policy = UtilityPolicy::new();

        assert_eq!(
            policy.rally_point(&connected, None, Some(enemy), home, true),
            TilePos::new(16, 8),
            "a new army should assemble on the homeward side of the forward base"
        );
        assert_eq!(
            policy.rally_point(&connected, None, Some(enemy), home, false),
            TilePos::new(7, 7),
            "the frozen profile-free controller keeps its home rally"
        );

        let mut island = connected;
        island.known_rock = (0..island.map_height)
            .map(|y| TilePos::new(12, y))
            .collect();
        assert_eq!(
            policy.rally_point(&island, None, Some(enemy), home, true),
            TilePos::new(7, 7),
            "a ground army must not be assigned to screen an unreachable island Foundry"
        );
    }

    #[test]
    fn a_target_holding_staged_army_does_not_block_the_next_muster() {
        let target = TilePos::new(18, 8);
        let home = TilePos::new(4, 4);
        let (mut obs, mut army) = offensive_position(TilePos::new(30, 18));
        obs.my_units = (1..=6)
            .map(|id| fighter(id, target))
            .chain((100..=104).map(|id| fighter(id, home)))
            .collect();
        obs.enemy_buildings = vec![defense(20, BuildingKind::Foundry, target)];
        army.members = (1..=6).map(UnitId).collect();
        army.staging = target;
        army.target = Some(target);

        let mut dials = Dials::full();
        dials.army_size = 5;
        let mut policy = UtilityPolicy::new();
        policy.scouted_at = obs.tick;
        let mut intents = Vec::new();

        policy.army(&dials, &obs, &[army], home, player_mode(None), &mut intents);

        assert_eq!(
            intents,
            vec![Intent::FormArmy {
                staging: TilePos::new(7, 5),
                size: 6,
            }]
        );
    }

    #[test]
    fn visible_pressure_at_an_owned_expansion_waits_for_a_coherent_home_army() {
        let home = TilePos::new(3, 6);
        let expansion = TilePos::new(28, 14);
        let members = vec![
            UnitObs {
                player: PlayerId(0),
                ..hostile(1, UnitKind::Sentinel, TilePos::new(5, 6))
            },
            UnitObs {
                player: PlayerId(0),
                ..hostile(2, UnitKind::Sentinel, TilePos::new(5, 7))
            },
            UnitObs {
                player: PlayerId(0),
                ..hostile(3, UnitKind::Sentinel, TilePos::new(6, 6))
            },
        ];
        let army = Army {
            id: ArmyId(7),
            members: members.iter().map(|unit| unit.id).collect(),
            state: ArmyState::Staging,
            staging: TilePos::new(6, 6),
            target: None,
            focus: None,
            progress: None,
            issued: None,
            bounces: 0,
        };
        let mut obs = Observation {
            version: OBSERVATION_VERSION,
            tick: 4_200,
            me: PlayerId(0),
            scrap: 0,
            map_width: 40,
            map_height: 24,
            my_units: members,
            my_buildings: vec![own_foundry(0, home), own_foundry(1, expansion)],
            my_queues: vec![Vec::new(), Vec::new()],
            ally_units: Vec::new(),
            ally_buildings: Vec::new(),
            enemy_units: vec![
                hostile(20, UnitKind::Scuttler, TilePos::new(29, 12)),
                hostile(21, UnitKind::Scuttler, TilePos::new(30, 13)),
                hostile(22, UnitKind::Scuttler, TilePos::new(31, 14)),
                hostile(23, UnitKind::Scuttler, TilePos::new(30, 15)),
                hostile(24, UnitKind::Scuttler, TilePos::new(29, 16)),
            ],
            enemy_buildings: Vec::new(),
            visible: vec![true; 40 * 24],
            explored: vec![true; 40 * 24],
            known_scrap: Vec::new(),
            known_rock: Vec::new(),
            known_frames: Vec::new(),
            known_peaks: Vec::new(),
            known_wrecks: Vec::new(),
            salvage_incidents: Vec::new(),
            blips: Vec::new(),
            faction: Faction::Ferrous,
            my_shells: 0,
            incoming_shells: Vec::new(),
        };
        let mut dials = Dials::full();
        dials.army_size = 5;

        let defend = |observation: &Observation, body: &Army| {
            let mut intents = Vec::new();
            UtilityPolicy::new().army(
                &dials,
                observation,
                std::slice::from_ref(body),
                home,
                player_mode(None),
                &mut intents,
            );
            intents
        };

        let intents = defend(&obs, &army);
        assert!(
            intents.iter().all(|intent| !matches!(
                intent,
                Intent::PushArmy {
                    army: ArmyId(7),
                    ..
                }
            )),
            "three fresh machines must not cross the map into five visible raiders: {intents:?}"
        );
        assert!(
            intents
                .iter()
                .any(|intent| matches!(intent, Intent::FormArmy { size: 6, .. })),
            "remote defense must keep mustering a coherent replacement: {intents:?}"
        );
        assert!(
            intents.iter().any(|intent| matches!(
                intent,
                Intent::FormArmy { staging, .. }
                    if staging.chebyshev(expansion) < staging.chebyshev(home)
            )),
            "fresh production should gather beside the threatened expansion instead of being recalled to the home body: {intents:?}"
        );

        let reinforcements = (4..=6).map(|id| fighter(id, TilePos::new(6, 7)));
        obs.my_units.extend(reinforcements.clone());
        let mut coherent = army.clone();
        coherent.members.extend(reinforcements.map(|unit| unit.id));
        let coherent_intents = {
            let mut intents = Vec::new();
            UtilityPolicy::new().army(
                &dials,
                &obs,
                std::slice::from_ref(&coherent),
                home,
                player_mode(None),
                &mut intents,
            );
            intents
        };
        assert!(
            coherent_intents.iter().any(|intent| matches!(
                intent,
                Intent::PushArmy {
                    army: ArmyId(7),
                    target
                } if *target == TilePos::new(29, 12)
            )),
            "six machines should answer the remote expansion threat: {coherent_intents:?}"
        );

        let mut mixed_obs = obs.clone();
        mixed_obs.my_units = (1_u32..=5)
            .map(|id| sentinel(id, TilePos::new(5 + i32::try_from(id % 3).unwrap(), 6)))
            .chain((6_u32..=7).map(|id| UnitObs {
                kind: UnitKind::Stinger,
                hp: UnitKind::Stinger.stats().max_hp,
                ..fighter(id, TilePos::new(6 + i32::try_from(id % 2).unwrap(), 7))
            }))
            .collect();
        let mut mixed = coherent.clone();
        mixed.members = mixed_obs.my_units.iter().map(|unit| unit.id).collect();
        assert_eq!(
            push_target(&defend(&mixed_obs, &mixed)),
            None,
            "ground-incapable AA bodies must not complete a remote-defense quorum"
        );

        let sixth_ground = sentinel(8, TilePos::new(7, 6));
        mixed.members.push(sixth_ground.id);
        mixed_obs.my_units.push(sixth_ground);
        assert_eq!(
            push_target(&defend(&mixed_obs, &mixed)),
            Some(TilePos::new(29, 12)),
            "a sixth ground-capable member should release the same defense"
        );

        let mut overseer = Vec::new();
        UtilityPolicy::new().army(
            &dials,
            &obs,
            std::slice::from_ref(&army),
            home,
            profile_free_mode(),
            &mut overseer,
        );
        assert!(
            overseer.iter().all(|intent| !matches!(
                intent,
                Intent::PushArmy {
                    army: ArmyId(7),
                    ..
                }
            )),
            "the profile-free Overseer keeps its frozen base set: {overseer:?}"
        );

        obs.enemy_units.reverse();
        assert_eq!(
            push_target(&defend(&obs, &coherent)),
            Some(TilePos::new(29, 12)),
            "threat selection must not depend on observation order"
        );

        for unit in &obs.enemy_units {
            obs.visible[(unit.tile.y * obs.map_width + unit.tile.x) as usize] = false;
        }
        let hidden_intents = defend(&obs, &coherent);
        assert!(
            hidden_intents.iter().all(|intent| !matches!(
                intent,
                Intent::PushArmy {
                    army: ArmyId(7),
                    ..
                }
            )),
            "an unseen mobile contact cannot trigger expansion defense: {hidden_intents:?}"
        );
    }

    #[test]
    fn local_overmatch_answers_one_intruder_at_every_difficulty_without_remote_trickling() {
        let home = TilePos::new(4, 6);
        let expansion = TilePos::new(30, 14);
        let threat = TilePos::new(12, 7);
        let (mut obs, mut army) = offensive_position(TilePos::new(35, 18));
        obs.tick = 6_000;
        obs.my_buildings = vec![own_foundry(0, home), own_foundry(1, expansion)];
        obs.my_queues = vec![Vec::new(), Vec::new()];
        obs.my_units = (1..=4)
            .map(|id| sentinel(id, TilePos::new(5 + i32::try_from(id % 2).unwrap(), 6)))
            .collect();
        army.members = obs.my_units.iter().map(|unit| unit.id).collect();
        army.staging = TilePos::new(6, 6);
        obs.enemy_units = vec![hostile(20, UnitKind::Sentinel, threat)];
        obs.enemy_buildings.clear();

        for difficulty in BotDifficulty::ALL {
            let profile =
                BotConfig::scripted(difficulty, BotStance::Balanced, 1_616_200).resolve_profile();
            let dials = Dials::scripted(&profile, DifficultyTuning::for_level(difficulty));
            let mut intents = Vec::new();
            UtilityPolicy::new().army(
                &dials,
                &obs,
                std::slice::from_ref(&army),
                home,
                player_mode(None),
                &mut intents,
            );
            assert_eq!(
                push_target(&intents),
                Some(threat),
                "{difficulty:?} left a locally overwhelming four-unit screen idle against one home intruder: {intents:?}"
            );
        }

        let remote_threat = expansion.offset(1, 0);
        obs.enemy_units[0].tile = remote_threat;
        obs.my_units.truncate(1);
        army.members = vec![obs.my_units[0].id];
        for difficulty in BotDifficulty::ALL {
            let profile =
                BotConfig::scripted(difficulty, BotStance::Balanced, 1_616_200).resolve_profile();
            let dials = Dials::scripted(&profile, DifficultyTuning::for_level(difficulty));
            let mut intents = Vec::new();
            UtilityPolicy::new().army(
                &dials,
                &obs,
                std::slice::from_ref(&army),
                home,
                player_mode(None),
                &mut intents,
            );
            assert_eq!(
                push_target(&intents),
                None,
                "{difficulty:?} sent one fresh home defender across the map as a trickle: {intents:?}"
            );
            assert!(intents.iter().any(|intent| matches!(
                intent,
                Intent::FormArmy { staging, .. }
                    if staging.chebyshev(expansion) < staging.chebyshev(home)
            )));
        }
    }

    #[test]
    fn local_and_catastrophically_outmatched_defenders_hold_while_the_muster_continues() {
        let home = TilePos::new(3, 6);
        let expansion = TilePos::new(28, 14);
        let make_body = |staging: TilePos| {
            let units: Vec<_> = (1..=6)
                .map(|id| sentinel(id, staging.offset(i32::try_from(id % 2).unwrap(), 0)))
                .collect();
            let army = Army {
                id: ArmyId(7),
                members: units.iter().map(|unit| unit.id).collect(),
                state: ArmyState::Staging,
                staging,
                target: None,
                focus: None,
                progress: None,
                issued: None,
                bounces: 0,
            };
            (units, army)
        };
        let observation = |units: Vec<UnitObs>, enemy_units: Vec<UnitObs>| Observation {
            version: OBSERVATION_VERSION,
            tick: 4_200,
            me: PlayerId(0),
            scrap: 0,
            map_width: 40,
            map_height: 24,
            my_units: units,
            my_buildings: vec![own_foundry(0, home), own_foundry(1, expansion)],
            my_queues: vec![Vec::new(), Vec::new()],
            ally_units: Vec::new(),
            ally_buildings: Vec::new(),
            enemy_units,
            enemy_buildings: Vec::new(),
            visible: vec![true; 40 * 24],
            explored: vec![true; 40 * 24],
            known_scrap: Vec::new(),
            known_rock: Vec::new(),
            known_frames: Vec::new(),
            known_peaks: Vec::new(),
            known_wrecks: Vec::new(),
            salvage_incidents: Vec::new(),
            blips: Vec::new(),
            faction: Faction::Ferrous,
            my_shells: 0,
            incoming_shells: Vec::new(),
        };
        let mut dials = Dials::full();
        dials.army_size = 5;
        let decide = |obs: &Observation, army: &Army| {
            let mut intents = Vec::new();
            UtilityPolicy::new().army(
                &dials,
                obs,
                std::slice::from_ref(army),
                home,
                player_mode(None),
                &mut intents,
            );
            intents
        };

        let (local_units, local_army) = make_body(expansion);
        let local = observation(
            local_units,
            vec![hostile(20, UnitKind::Scuttler, expansion.offset(1, 0))],
        );
        let local_intents = decide(&local, &local_army);
        assert!(
            local_intents.iter().all(|intent| !matches!(
                intent,
                Intent::PushArmy {
                    army: ArmyId(7),
                    ..
                }
            )),
            "a staged body already in contact should fight through auto-acquire: {local_intents:?}"
        );
        assert!(
            local_intents
                .iter()
                .any(|intent| matches!(intent, Intent::FormArmy { size: 6, .. }))
        );

        let (remote_units, remote_army) = make_body(home.offset(2, 0));
        let overwhelming: Vec<_> = (20..=27)
            .map(|id| {
                hostile(
                    id,
                    UnitKind::Breaker,
                    expansion.offset(i32::try_from(id % 3).unwrap(), 0),
                )
            })
            .collect();
        let remote = observation(remote_units, overwhelming);
        let remote_intents = decide(&remote, &remote_army);
        assert!(
            remote_intents.iter().all(|intent| !matches!(
                intent,
                Intent::PushArmy {
                    army: ArmyId(7),
                    ..
                }
            )),
            "a coherent body must not cross the map into catastrophic odds: {remote_intents:?}"
        );
        assert!(
            remote_intents
                .iter()
                .any(|intent| matches!(intent, Intent::FormArmy { size: 6, .. }))
        );
    }

    #[test]
    fn an_isolated_objective_is_not_guarded_by_a_remote_defense_cluster() {
        let (mut obs, mut army) = offensive_position(TilePos::new(31, 18));
        let reserve = fighter(5, TilePos::new(7, 6));
        army.members.push(reserve.id);
        obs.my_units.push(reserve);
        let mut dials = Dials::full();
        dials.army_size = 4;
        let mut policy = UtilityPolicy::new();
        policy.scouted_at = obs.tick;
        let mut intents = Vec::new();

        policy.army(
            &dials,
            &obs,
            &[army],
            TilePos::new(2, 6),
            player_mode(None),
            &mut intents,
        );

        assert_eq!(push_target(&intents), Some(TilePos::new(12, 6)));
    }

    #[test]
    fn defenses_clustered_around_the_objective_keep_the_push_gate_closed() {
        let (obs, army) = offensive_position(TilePos::new(14, 7));
        let mut dials = Dials::full();
        dials.army_size = 4;
        let mut policy = UtilityPolicy::new();
        policy.scouted_at = obs.tick;
        let mut intents = Vec::new();

        policy.army(
            &dials,
            &obs,
            &[army],
            TilePos::new(2, 6),
            player_mode(None),
            &mut intents,
        );

        assert_eq!(push_target(&intents), None);
        assert!(matches!(
            intents.as_slice(),
            [Intent::FormArmy { size: 6, .. }]
        ));
    }

    #[test]
    fn harder_rungs_never_wait_for_a_push_that_an_easier_rung_takes() {
        let (mut baseline, mut army) = offensive_position(TilePos::new(31, 18));
        let fifth = fighter(5, TilePos::new(7, 6));
        army.members.push(fifth.id);
        baseline.my_units.push(fifth);
        let sixth = fighter(6, TilePos::new(7, 7));
        army.members.push(sixth.id);
        baseline.my_units.push(sixth);
        baseline.enemy_buildings[0].hp = 280;
        let home = TilePos::new(2, 6);

        for seed in 0..256 {
            let dials: Vec<_> = BotDifficulty::ALL
                .into_iter()
                .map(|difficulty| {
                    let profile = BotConfig::scripted(difficulty, BotStance::Balanced, seed)
                        .resolve_profile();
                    Dials::scripted(&profile, DifficultyTuning::for_level(difficulty))
                })
                .collect();
            assert!(dials.iter().all(|dial| dial.fog_honest));
            assert!(
                dials
                    .iter()
                    .all(|dial| dial.army_size == dials[0].army_size)
            );

            for hp in (20..=50).step_by(2) {
                let mut obs = baseline.clone();
                for unit in &mut obs.my_units {
                    unit.hp = hp;
                }
                let pushes: Vec<_> = dials
                    .iter()
                    .map(|dial| {
                        let mut policy = UtilityPolicy::new();
                        policy.scouted_at = obs.tick;
                        let mut intents = Vec::new();
                        policy.army(
                            dial,
                            &obs,
                            std::slice::from_ref(&army),
                            home,
                            player_mode(None),
                            &mut intents,
                        );
                        push_target(&intents).is_some()
                    })
                    .collect();

                for (index, pair) in pushes.windows(2).enumerate() {
                    assert!(
                        !pair[0] || pair[1],
                        "seed {seed}, hp {hp}: {:?} pushed while harder {:?} held ({pushes:?})",
                        BotDifficulty::ALL[index],
                        BotDifficulty::ALL[index + 1]
                    );
                }
            }
        }
    }

    #[test]
    fn remembered_six_unit_force_does_not_give_veteran_initiative_over_prime() {
        let target = TilePos::new(31, 18);
        let (mut obs, mut army) = offensive_position(target);
        obs.tick = 8_184;
        obs.enemy_buildings.clear();
        obs.enemy_units = vec![hostile(90, UnitKind::Harvester, target)];
        obs.my_units = (1..=10)
            .map(|id| sentinel(id, TilePos::new(6 + i32::try_from(id % 2).unwrap(), 6)))
            .collect();
        army.members = obs.my_units.iter().map(|unit| unit.id).collect();
        let six_sentinel_peak = 6 * demonstrated_ground_strength(&obs.my_units[0]);
        let home = TilePos::new(2, 6);

        let decide = |difficulty, tick| {
            let profile =
                BotConfig::scripted(difficulty, BotStance::Balanced, 1_616_100).resolve_profile();
            let dials = Dials::scripted(&profile, DifficultyTuning::for_level(difficulty));
            let mut current = obs.clone();
            current.tick = tick;
            let mut policy = UtilityPolicy::new();
            policy.scouted_at = tick;
            policy.opponent_force_peak = Some((six_sentinel_peak, tick));
            let mut intents = Vec::new();
            policy.army(
                &dials,
                &current,
                std::slice::from_ref(&army),
                home,
                player_mode(None),
                &mut intents,
            );
            push_target(&intents)
        };

        assert_eq!(decide(BotDifficulty::Veteran, 8_184), None);
        assert_eq!(decide(BotDifficulty::Prime, 8_184), None);
        assert_eq!(decide(BotDifficulty::Veteran, 12_000), Some(target));
        assert_eq!(decide(BotDifficulty::Prime, 12_000), Some(target));
    }

    #[test]
    fn stale_fog_requires_a_real_surplus_before_a_mixed_army_pushes() {
        let home = TilePos::new(2, 6);
        let objective = TilePos::new(31, 18);
        let mut obs = offensive_position(objective).0;
        obs.tick = 12_624;
        obs.visible.fill(false);
        obs.enemy_units.clear();
        obs.enemy_buildings = vec![BuildingObs {
            id: BuildingId(u32::MAX),
            player: PlayerId(1),
            kind: BuildingKind::Foundry,
            anchor: objective,
            hp: BuildingKind::Foundry.base_stats().max_hp,
            built: false,
            seen: false,
            tier: 0,
        }];
        obs.my_units = (0..5)
            .map(|index| {
                sentinel(
                    u32::try_from(index + 1).unwrap(),
                    TilePos::new(5 + index % 3, 6),
                )
            })
            .chain((0..2).map(|index| UnitObs {
                kind: UnitKind::Scuttler,
                hp: UnitKind::Scuttler.stats().max_hp,
                ..fighter(
                    u32::try_from(index + 6).unwrap(),
                    TilePos::new(6 + index, 7),
                )
            }))
            .collect();
        let mut army = Army {
            id: ArmyId(7),
            members: obs.my_units.iter().map(|unit| unit.id).collect(),
            state: ArmyState::Staging,
            staging: TilePos::new(6, 6),
            target: None,
            focus: None,
            progress: None,
            issued: None,
            bounces: 0,
        };

        let dials = [BotDifficulty::Veteran, BotDifficulty::Prime].map(|difficulty| {
            let profile =
                BotConfig::scripted(difficulty, BotStance::Balanced, 1_616_201).resolve_profile();
            Dials::scripted(&profile, DifficultyTuning::for_level(difficulty))
        });
        let pushes = |observation: &Observation, body: &Army| {
            dials.each_ref().map(|dials| {
                let mut policy = UtilityPolicy::new();
                let mut intents = Vec::new();
                policy.army(
                    dials,
                    observation,
                    std::slice::from_ref(body),
                    home,
                    player_mode(None),
                    &mut intents,
                );
                push_target(&intents)
            })
        };

        assert_eq!(
            pushes(&obs, &army),
            [None, None],
            "an accurate estimate must not make Prime treat stale fog as an undefended fair fight"
        );

        for id in 8..=9 {
            let reinforcement = sentinel(id, TilePos::new(7, 5 + i32::try_from(id % 2).unwrap()));
            army.members.push(reinforcement.id);
            obs.my_units.push(reinforcement);
        }
        assert_eq!(
            pushes(&obs, &army),
            [Some(objective), Some(objective)],
            "a genuine two-Sentinel surplus should open the same attack for both competent rungs"
        );
    }

    #[test]
    fn opponent_force_memory_keeps_the_peak_through_each_exact_rung_boundary() {
        let one_unit_strength =
            demonstrated_ground_strength(&hostile(100, UnitKind::Sentinel, TilePos::new(30, 18)));
        let initial_tick = 10_000;

        for difficulty in BotDifficulty::ALL {
            let profile =
                BotConfig::scripted(difficulty, BotStance::Balanced, 1_616_200).resolve_profile();
            let dials = Dials::scripted(&profile, DifficultyTuning::for_level(difficulty));
            let mut policy = UtilityPolicy::new();
            let mut obs = offensive_position(TilePos::new(31, 18)).0;
            obs.tick = initial_tick;
            obs.enemy_units = vec![
                hostile(100, UnitKind::Sentinel, TilePos::new(30, 18)),
                hostile(101, UnitKind::Sentinel, TilePos::new(31, 18)),
            ];
            let initial_strength = one_unit_strength * 2;
            let initial_risk = initial_strength * (100 + DEMONSTRATED_FORCE_RESERVE_PERCENT) / 100;

            assert_eq!(policy.opponent_force_risk(&dials, &obs), initial_risk);
            assert_eq!(
                policy.opponent_force_peak,
                Some((initial_strength, initial_tick)),
                "{difficulty:?} did not record the observed force"
            );

            obs.tick += 1;
            obs.enemy_units.truncate(1);
            assert_eq!(policy.opponent_force_risk(&dials, &obs), initial_risk);
            assert_eq!(
                policy.opponent_force_peak,
                Some((initial_strength, initial_tick)),
                "{difficulty:?} let a weaker sight replace or refresh the peak"
            );

            obs.tick = initial_tick + dials.opponent_force_memory;
            obs.enemy_units.clear();
            assert_eq!(policy.opponent_force_risk(&dials, &obs), initial_risk);
            assert_eq!(
                policy.opponent_force_peak,
                Some((initial_strength, initial_tick)),
                "{difficulty:?} forgot the force at the exact memory boundary"
            );

            obs.tick += 1;
            assert_eq!(policy.opponent_force_risk(&dials, &obs), 0);
            assert_eq!(
                policy.opponent_force_peak, None,
                "{difficulty:?} retained the force beyond its memory boundary"
            );

            let replacement_tick = obs.tick + 100;
            obs.tick = replacement_tick;
            obs.enemy_units = vec![
                hostile(200, UnitKind::Sentinel, TilePos::new(30, 18)),
                hostile(201, UnitKind::Sentinel, TilePos::new(31, 18)),
            ];
            assert_eq!(policy.opponent_force_risk(&dials, &obs), initial_risk);
            assert_eq!(
                policy.opponent_force_peak,
                Some((initial_strength, replacement_tick))
            );

            obs.tick += 1;
            let equal_tick = obs.tick;
            assert_eq!(policy.opponent_force_risk(&dials, &obs), initial_risk);
            assert_eq!(
                policy.opponent_force_peak,
                Some((initial_strength, equal_tick)),
                "{difficulty:?} did not refresh an equally strong sight"
            );

            obs.tick += 1;
            let stronger_tick = obs.tick;
            obs.enemy_units
                .push(hostile(202, UnitKind::Sentinel, TilePos::new(32, 18)));
            let stronger_strength = one_unit_strength * 3;
            let stronger_risk =
                stronger_strength * (100 + DEMONSTRATED_FORCE_RESERVE_PERCENT) / 100;
            assert_eq!(policy.opponent_force_risk(&dials, &obs), stronger_risk);
            assert_eq!(
                policy.opponent_force_peak,
                Some((stronger_strength, stronger_tick)),
                "{difficulty:?} did not replace the peak with a stronger sight"
            );
        }
    }

    #[test]
    fn remembered_force_never_makes_a_higher_rung_refuse_a_lower_rung_push() {
        let home = TilePos::new(2, 6);
        let objective = TilePos::new(31, 18);
        let mut observed = offensive_position(objective).0;
        observed.tick = 10_000;
        observed.my_units = (1_u32..=10)
            .map(|id| sentinel(id, TilePos::new(5 + i32::try_from(id % 4).unwrap(), 6)))
            .collect();
        observed.enemy_units = (100_u32..108)
            .map(|id| {
                hostile(
                    id,
                    UnitKind::Sentinel,
                    objective.offset(i32::try_from(id % 3).unwrap(), 0),
                )
            })
            .collect();
        observed.enemy_buildings = vec![defense(20, BuildingKind::Foundry, objective)];
        let army = Army {
            id: ArmyId(7),
            members: observed.my_units.iter().map(|unit| unit.id).collect(),
            state: ArmyState::Staging,
            staging: TilePos::new(6, 6),
            target: None,
            focus: None,
            progress: None,
            issued: None,
            bounces: 0,
        };
        let dials = BotDifficulty::ALL.map(|difficulty| {
            let profile =
                BotConfig::scripted(difficulty, BotStance::Balanced, 1_616_200).resolve_profile();
            Dials::scripted(&profile, DifficultyTuning::for_level(difficulty))
        });
        let mut policies = std::array::from_fn::<_, 4, _>(|_| UtilityPolicy::new());

        for (policy, dials) in policies.iter_mut().zip(&dials) {
            let mut intents = Vec::new();
            policy.army(
                dials,
                &observed,
                std::slice::from_ref(&army),
                home,
                player_mode(None),
                &mut intents,
            );
            assert!(policy.opponent_force_peak.is_some());
        }

        let mut hidden = observed.clone();
        hidden.visible.fill(false);
        hidden.enemy_units.clear();
        hidden.enemy_buildings = vec![BuildingObs {
            id: BuildingId(u32::MAX),
            player: PlayerId(1),
            kind: BuildingKind::Foundry,
            anchor: objective,
            hp: BuildingKind::Foundry.base_stats().max_hp,
            built: false,
            seen: false,
            tier: 0,
        }];

        let probe_ages = [
            0,
            VOLUNTARY_FORCE_RISK_HORIZON - 1,
            VOLUNTARY_FORCE_RISK_HORIZON,
            VOLUNTARY_FORCE_RISK_HORIZON + 1,
            dials[1].opponent_force_memory,
            dials[1].opponent_force_memory + 1,
            dials[2].opponent_force_memory,
            dials[2].opponent_force_memory + 1,
            dials[3].opponent_force_memory,
            dials[3].opponent_force_memory + 1,
        ];
        for age in probe_ages {
            hidden.tick = observed.tick + age;
            let mut intents: [Vec<Intent>; 4] = std::array::from_fn(|_| Vec::new());
            for ((policy, dials), intents) in policies.iter_mut().zip(&dials).zip(&mut intents) {
                policy.army(
                    dials,
                    &hidden,
                    std::slice::from_ref(&army),
                    home,
                    player_mode(None),
                    intents,
                );
            }
            let pushes = intents
                .each_ref()
                .map(|intents| push_target(intents).is_some());
            for (pair, difficulties) in pushes.windows(2).zip(BotDifficulty::ALL.windows(2)) {
                assert!(
                    !pair[0] || pair[1],
                    "{:?} pushed while {:?} refused the same remembered-force history at age {age}: {pushes:?}",
                    difficulties[0],
                    difficulties[1]
                );
            }
            if age <= VOLUNTARY_FORCE_RISK_HORIZON {
                assert_eq!(pushes, [false; 4], "risk expired early at age {age}");
            } else {
                assert_eq!(pushes, [true; 4], "risk blocked initiative at age {age}");
            }

            if age == VOLUNTARY_FORCE_RISK_HORIZON + 1 {
                assert_eq!(policies[0].opponent_force_peak, None);
                assert!(
                    policies[1..]
                        .iter()
                        .all(|policy| policy.opponent_force_peak.is_some()),
                    "competent rungs discarded longer-lived intelligence with voluntary risk"
                );
            }
        }
        assert!(
            policies
                .iter()
                .all(|policy| policy.opponent_force_peak.is_none()),
            "configured long-term memories did not expire at their own boundaries"
        );
    }

    #[test]
    fn fresh_intelligence_still_requires_a_cohesive_attack_body() {
        let (mut baseline, mut army) = offensive_position(TilePos::new(31, 18));
        baseline.tick = 4_656;
        baseline.enemy_buildings = vec![defense(20, BuildingKind::Foundry, TilePos::new(31, 18))];
        baseline.my_units = vec![
            sentinel(1, TilePos::new(5, 6)),
            sentinel(2, TilePos::new(5, 7)),
            sentinel(3, TilePos::new(6, 6)),
            sentinel(4, TilePos::new(6, 7)),
        ];
        let fifth = sentinel(5, TilePos::new(7, 6));
        army.members.push(fifth.id);
        baseline.my_units.push(fifth);
        let home = TilePos::new(2, 6);

        for seed in 0..256 {
            let dials: Vec<_> = BotDifficulty::ALL
                .into_iter()
                .map(|difficulty| {
                    let profile = BotConfig::scripted(difficulty, BotStance::Balanced, seed)
                        .resolve_profile();
                    Dials::scripted(&profile, DifficultyTuning::for_level(difficulty))
                })
                .collect();

            for (difficulty, dial) in BotDifficulty::ALL.into_iter().zip(&dials) {
                let mut policy = UtilityPolicy::new();
                policy.scouted_at = baseline.tick;
                let mut intents = Vec::new();
                policy.army(
                    dial,
                    &baseline,
                    std::slice::from_ref(&army),
                    home,
                    player_mode(None),
                    &mut intents,
                );

                assert_eq!(
                    push_target(&intents),
                    None,
                    "{difficulty:?} seed {seed} launched the brittle five-unit body: {intents:?}"
                );
                assert!(
                    intents
                        .iter()
                        .any(|intent| matches!(intent, Intent::FormArmy { size: 7, .. })),
                    "{difficulty:?} seed {seed} must wait for two reinforcements: {intents:?}"
                );
            }

            let sixth = sentinel(6, TilePos::new(7, 7));
            let mut reinforced = baseline.clone();
            reinforced.my_units.push(sixth.clone());
            let mut reinforced_army = army.clone();
            reinforced_army.members.push(sixth.id);
            let pushes: Vec<_> = dials
                .iter()
                .map(|dial| {
                    let mut policy = UtilityPolicy::new();
                    policy.scouted_at = reinforced.tick;
                    let mut intents = Vec::new();
                    policy.army(
                        dial,
                        &reinforced,
                        std::slice::from_ref(&reinforced_army),
                        home,
                        player_mode(None),
                        &mut intents,
                    );
                    push_target(&intents).is_some()
                })
                .collect();
            assert_eq!(
                pushes,
                [true, true, true, true],
                "the sixth machine should release the cohesive body at every rung for personality seed {seed}"
            );
        }
    }

    #[test]
    fn desperation_never_turns_a_player_facing_remuster_into_a_trickle() {
        let home = TilePos::new(2, 6);
        for stance in BotStance::ALL {
            for seed in 0..256 {
                for difficulty in BotDifficulty::ALL {
                    let profile = BotConfig::scripted(difficulty, stance, seed).resolve_profile();
                    let dials = Dials::scripted(&profile, DifficultyTuning::for_level(difficulty));
                    let configured = dials.army_size as usize;
                    let mut obs = offensive_position(TilePos::new(31, 18)).0;
                    obs.tick = 5_136;
                    obs.enemy_buildings =
                        vec![defense(20, BuildingKind::Foundry, TilePos::new(31, 18))];
                    obs.my_units = (0..configured)
                        .map(|index| {
                            sentinel(
                                u32::try_from(index + 1).unwrap(),
                                TilePos::new(5 + i32::try_from(index % 3).unwrap(), 6),
                            )
                        })
                        .collect();
                    let mut army = Army {
                        id: ArmyId(7),
                        members: obs.my_units.iter().map(|unit| unit.id).collect(),
                        state: ArmyState::Staging,
                        staging: TilePos::new(6, 6),
                        target: None,
                        focus: None,
                        progress: None,
                        issued: None,
                        bounces: 0,
                    };
                    let mut policy = UtilityPolicy::new();
                    policy.desperate = true;
                    policy.scouted_at = obs.tick;
                    let mut intents = Vec::new();

                    policy.army(
                        &dials,
                        &obs,
                        std::slice::from_ref(&army),
                        home,
                        player_mode(None),
                        &mut intents,
                    );

                    assert_eq!(
                        push_target(&intents),
                        None,
                        "{difficulty:?} {stance:?} seed {seed} launched only {configured} machines: {intents:?}"
                    );
                    assert!(intents.iter().any(|intent| matches!(
                        intent,
                        Intent::FormArmy { size, .. }
                            if *size as usize >= coherent_attack_size(&dials, true)
                    )));

                    let reserve =
                        sentinel(u32::try_from(configured + 1).unwrap(), TilePos::new(7, 7));
                    army.members.push(reserve.id);
                    obs.my_units.push(reserve);
                    intents.clear();
                    policy.army(
                        &dials,
                        &obs,
                        std::slice::from_ref(&army),
                        home,
                        player_mode(None),
                        &mut intents,
                    );
                    assert_eq!(
                        push_target(&intents),
                        Some(TilePos::new(31, 18)),
                        "{difficulty:?} {stance:?} seed {seed} failed to launch its coherent remuster: {intents:?}"
                    );
                }
            }
        }

        let (mut obs, mut army) = offensive_position(TilePos::new(31, 18));
        obs.enemy_buildings = vec![defense(20, BuildingKind::Foundry, TilePos::new(31, 18))];
        army.members.truncate(1);
        let mut policy = UtilityPolicy::new();
        policy.desperate = true;
        policy.desperate_march = true;
        let mut intents = Vec::new();
        policy.army(
            &Dials::full(),
            &obs,
            &[army],
            home,
            profile_free_mode(),
            &mut intents,
        );
        assert!(
            push_target(&intents).is_some(),
            "the profile-free controller must retain its historical last-machine push"
        );
    }

    #[test]
    fn desperate_player_facing_army_probes_only_a_known_route_to_the_mirror_objective() {
        let home = TilePos::new(2, 6);
        let profile = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 20_045)
            .resolve_profile();
        let dials = Dials::scripted(&profile, DifficultyTuning::for_level(BotDifficulty::Prime));
        let body_size = coherent_attack_size(&dials, true);
        let (mut open, _) = offensive_position(TilePos::new(31, 18));
        open.tick = 8_000;
        open.enemy_units.clear();
        open.enemy_buildings.clear();
        open.my_units = (0..body_size)
            .map(|index| {
                sentinel(
                    u32::try_from(index + 1).unwrap(),
                    TilePos::new(5 + i32::try_from(index % 3).unwrap(), 6),
                )
            })
            .collect();
        let army = Army {
            id: ArmyId(7),
            members: open.my_units.iter().map(|unit| unit.id).collect(),
            state: ArmyState::Staging,
            staging: TilePos::new(6, 6),
            target: None,
            focus: None,
            progress: None,
            issued: None,
            bounces: 0,
        };
        let mirror = TilePos::new(open.map_width - 1 - home.x, open.map_height - 1 - home.y);
        let decide = |obs: &Observation| {
            let mut policy = UtilityPolicy::new();
            policy.desperate = true;
            policy.desperate_march = true;
            policy.scouted_at = obs.tick;
            let mut intents = Vec::new();
            policy.army(
                &dials,
                obs,
                std::slice::from_ref(&army),
                home,
                player_mode(None),
                &mut intents,
            );
            intents
        };

        let open_intents = decide(&open);
        assert_eq!(
            push_target(&open_intents),
            Some(mirror),
            "a stalled economy with a coherent army needs one honest liveness probe: {open_intents:?}"
        );

        let mut unknown = open.clone();
        unknown.explored.fill(false);
        assert_eq!(
            push_target(&decide(&unknown)),
            None,
            "desperation cannot invent an explored road through fog"
        );

        let mut severed = open;
        severed.known_rock = (0..severed.map_height)
            .map(|y| TilePos::new(severed.map_width / 2, y))
            .collect();
        assert_eq!(
            push_target(&decide(&severed)),
            None,
            "a mapped wall must still veto the mirror probe"
        );
    }

    #[test]
    fn current_and_fresh_remembered_defenses_keep_the_push_gate_closed() {
        let (seen, army) = offensive_position(TilePos::new(14, 7));
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&seen);
        let mut dials = Dials::full();
        dials.army_size = 4;
        let mut policy = UtilityPolicy::new();
        policy.scouted_at = seen.tick;
        let mut intents = Vec::new();

        policy.army(
            &dials,
            &seen,
            std::slice::from_ref(&army),
            TilePos::new(2, 6),
            player_mode(Some(intelligence.buildings())),
            &mut intents,
        );
        assert_eq!(push_target(&intents), None);

        let mut hidden = seen;
        hidden.tick += 1;
        hide_enemy_buildings(&mut hidden);
        intelligence.update(&hidden);
        assert!(
            intelligence
                .buildings()
                .iter()
                .all(|contact| contact.confidence_at(hidden.tick) > 0)
        );
        policy.scouted_at = hidden.tick;
        intents.clear();
        policy.army(
            &dials,
            &hidden,
            &[army],
            TilePos::new(2, 6),
            player_mode(Some(intelligence.buildings())),
            &mut intents,
        );

        assert_eq!(push_target(&intents), None);
    }

    #[test]
    fn an_unknown_age_ghost_defense_remains_conservative() {
        let (mut hidden, army) = offensive_position(TilePos::new(14, 7));
        hide_enemy_buildings(&mut hidden);
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&hidden);
        assert!(
            intelligence
                .buildings()
                .iter()
                .all(|contact| contact.last_seen.is_none())
        );
        assert!(
            intelligence
                .buildings()
                .iter()
                .all(|contact| contact.confidence_at(hidden.tick) == 0)
        );

        let mut dials = Dials::full();
        dials.army_size = 4;
        let mut policy = UtilityPolicy::new();
        policy.scouted_at = hidden.tick;
        let mut intents = Vec::new();
        policy.army(
            &dials,
            &hidden,
            &[army],
            TilePos::new(2, 6),
            player_mode(Some(intelligence.buildings())),
            &mut intents,
        );

        assert_eq!(push_target(&intents), None);
    }

    #[test]
    fn visible_and_remembered_upgraded_defense_use_the_observed_tier() {
        let (mut seen, army) = offensive_position(TilePos::new(31, 18));
        let turret = &mut seen.enemy_buildings[0];
        turret.tier = 1;
        // At this damaged HP, the base-tier estimate is weak enough to permit
        // the push while the observed Heavy Turret is strong enough to veto it.
        turret.hp = BuildingKind::Turret.base_stats().max_hp;

        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&seen);
        let mut dials = Dials::full();
        dials.army_size = 4;
        let mut policy = UtilityPolicy::new();
        policy.scouted_at = seen.tick;
        let mut intents = Vec::new();
        policy.army(
            &dials,
            &seen,
            std::slice::from_ref(&army),
            TilePos::new(2, 6),
            player_mode(Some(intelligence.buildings())),
            &mut intents,
        );
        assert_eq!(push_target(&intents), None);

        let mut hidden = seen;
        hidden.tick += 1;
        hide_enemy_buildings(&mut hidden);
        hidden.enemy_buildings[0].tier = 0;
        intelligence.update(&hidden);
        let contact = intelligence
            .buildings()
            .iter()
            .find(|contact| contact.kind == BuildingKind::Turret)
            .expect("the observed turret remains in memory");
        assert_eq!(contact.tier, 1);

        policy.scouted_at = hidden.tick;
        intents.clear();
        policy.army(
            &dials,
            &hidden,
            &[army],
            TilePos::new(2, 6),
            player_mode(Some(intelligence.buildings())),
            &mut intents,
        );
        assert_eq!(push_target(&intents), None);
    }

    #[test]
    fn profile_free_strength_keeps_the_legacy_base_tier_rule() {
        let mut turret = defense(20, BuildingKind::Turret, TilePos::new(12, 6));
        turret.tier = 2;
        turret.hp = 700;

        assert_eq!(
            objective_building_strength(&turret, profile_free_mode(), 20_000),
            crate::bot::executive::building_strength(&turret)
        );
    }

    #[test]
    fn expired_remembered_defenses_cannot_veto_a_player_facing_probe() {
        let (mut seen, mut army) = offensive_position(TilePos::new(14, 7));
        let reserve = fighter(5, TilePos::new(7, 6));
        army.members.push(reserve.id);
        seen.my_units.push(reserve);
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&seen);
        let mut hidden = seen;
        hidden.tick += 10_000;
        hide_enemy_buildings(&mut hidden);
        intelligence.update(&hidden);
        assert!(
            intelligence
                .buildings()
                .iter()
                .all(|contact| contact.confidence_at(hidden.tick) == 0)
        );
        let mut dials = Dials::full();
        dials.army_size = 4;

        let mut player_facing = UtilityPolicy::new();
        player_facing.scouted_at = hidden.tick;
        let mut intents = Vec::new();
        player_facing.army(
            &dials,
            &hidden,
            std::slice::from_ref(&army),
            TilePos::new(2, 6),
            player_mode(Some(intelligence.buildings())),
            &mut intents,
        );
        assert_eq!(push_target(&intents), Some(TilePos::new(12, 6)));

        let mut profile_free = UtilityPolicy::new();
        profile_free.scouted_at = hidden.tick;
        intents.clear();
        profile_free.army(
            &dials,
            &hidden,
            &[army],
            TilePos::new(2, 6),
            profile_free_mode(),
            &mut intents,
        );
        assert_eq!(push_target(&intents), None);
    }

    #[test]
    fn parked_artillery_cannot_supply_the_strength_that_opens_a_push() {
        let (mut obs, mut army) = offensive_position(TilePos::new(14, 7));
        obs.my_units.clear();
        for id in 1..=6 {
            obs.my_units.push(fighter(
                id,
                TilePos::new(
                    4 + i32::try_from(id % 3).unwrap(),
                    5 + i32::try_from(id / 3).unwrap(),
                ),
            ));
        }
        for id in 7..=23 {
            obs.my_units.push(bombard(id, TilePos::new(6, 8)));
        }
        army.members = obs.my_units.iter().map(|unit| unit.id).collect();
        let escort_strength: u64 = obs
            .my_units
            .iter()
            .filter(|unit| unit.kind == UnitKind::Lancer)
            .map(crate::bot::executive::unit_strength)
            .sum();
        assert_eq!(
            crate::bot::executive::marching_strength(&army, &obs),
            escort_strength,
            "the parked guns contribute no deployable strength"
        );
        let mut dials = Dials::full();
        dials.army_size = 23;
        let mut policy = UtilityPolicy::new();
        policy.scouted_at = obs.tick;
        let mut intents = Vec::new();

        policy.army(
            &dials,
            &obs,
            &[army],
            TilePos::new(2, 6),
            player_mode(None),
            &mut intents,
        );

        assert_eq!(push_target(&intents), None);
        assert!(matches!(
            intents.as_slice(),
            [Intent::FormArmy { size: 25, .. }]
        ));
    }

    #[test]
    fn artillery_with_its_escort_quorum_does_contribute_to_a_push() {
        let (mut obs, mut army) = offensive_position(TilePos::new(31, 18));
        obs.my_units = vec![
            fighter(1, TilePos::new(5, 6)),
            fighter(2, TilePos::new(5, 7)),
            bombard(3, TilePos::new(6, 5)),
            bombard(4, TilePos::new(6, 6)),
            bombard(5, TilePos::new(6, 7)),
            bombard(6, TilePos::new(6, 8)),
            fighter(7, TilePos::new(7, 6)),
        ];
        army.members = obs.my_units.iter().map(|unit| unit.id).collect();
        let escort_strength: u64 = obs
            .my_units
            .iter()
            .filter(|unit| unit.kind == UnitKind::Lancer)
            .map(crate::bot::executive::unit_strength)
            .sum();
        assert!(crate::bot::executive::marching_strength(&army, &obs) > escort_strength);
        let mut dials = Dials::full();
        dials.army_size = 6;
        let mut policy = UtilityPolicy::new();
        policy.scouted_at = obs.tick;
        let mut intents = Vec::new();

        policy.army(
            &dials,
            &obs,
            &[army],
            TilePos::new(2, 6),
            player_mode(None),
            &mut intents,
        );

        assert_eq!(push_target(&intents), Some(TilePos::new(12, 6)));
    }
}
