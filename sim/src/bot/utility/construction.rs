//! Construction, repair, upgrade, and salvage decisions.

use super::*;

impl UtilityPolicy {
    /// Restore a known frame, raise advanced production, expand, and
    /// upgrade one structure per think without starving the army budget.
    /// Returns true when this channel spent the think.
    fn advanced_construction(
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
        // victory token the enemy must come dig out.
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
    pub(super) fn construction(
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
            .filter(|b| {
                !b.built && b.tier == 0 && !obs.my_units.iter().any(|u| u.site == Some(b.id))
            })
            .min_by_key(|b| (b.anchor.y, b.anchor.x));
        if let Some(site) = orphan {
            intents.push(Intent::Build {
                kind: site.kind,
                anchor: site.anchor,
            });
            return;
        }

        // One advanced construction rung per think, cheapest gate first.
        if (dials.deep_tech || dials.extractors || dials.upgrades || dials.expansion)
            && self.advanced_construction(dials, obs, home, budget, intents)
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

        // With the harvest line at strength and either a raid felt or
        // the enemy's ground road
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
    pub(super) fn repairs(
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
            // on purpose. Repair and salvage evict each other, so a repair
            // intent here would reverse the teardown.
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
    /// one more wave. Deliberately narrow so the bot does not sell its
    /// defenses during an otherwise sustainable siege.
    pub(super) fn salvage(&mut self, dials: &Dials, obs: &Observation, intents: &mut Vec<Intent>) {
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
                SALVAGE_PRIORITY
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
}
