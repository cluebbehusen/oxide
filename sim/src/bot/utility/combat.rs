//! Air raids, scouting, and ground-army strategy.

use super::*;

impl UtilityPolicy {
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
            self.scout_dispatch = None;
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
                    !enlisted.contains(&u.id) && (u.kind.stats().harvest.is_some() || u.idle)
                })
                .filter(|u| !self.air_scout_needed || u.kind.role() == crate::stats::Role::Scout)
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
        let from = obs
            .my_units
            .iter()
            .find(|unit| unit.id == scout)
            .map(|unit| unit.tile)
            .expect("the selected scout came from this observation");
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
                    .map(crate::bot::executive::unit_strength)
                    .sum()
            })
            .unwrap_or(0);
        let enemy_strength: u64 = obs
            .enemy_units
            .iter()
            .map(crate::bot::executive::unit_strength)
            .sum::<u64>()
            + obs
                .enemy_buildings
                .iter()
                .map(crate::bot::executive::building_strength)
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
    fn rally_point(
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
}

/// Convenience for tests and policies: whether a unit observation
/// can fight.
fn is_fighter(u: &UnitObs) -> bool {
    u.kind.stats().can_fight()
}
