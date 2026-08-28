//! Construction, repair, upgrade, and salvage decisions.

use super::*;

impl UtilityPolicy {
    fn frame_contains(frame: TilePos, tile: TilePos) -> bool {
        let (width, height) = BuildingKind::Extractor.base_stats().size;
        tile.x >= frame.x
            && tile.x < frame.x + width
            && tile.y >= frame.y
            && tile.y < frame.y + height
    }

    fn player_can_plan_frame_restoration(&self, obs: &Observation, frame: TilePos) -> bool {
        let (width, height) = BuildingKind::Extractor.base_stats().size;
        let footprint_explored =
            (0..height).all(|dy| (0..width).all(|dx| obs.explored(frame.offset(dx, dy))));
        let visibly_occupied = obs.enemy_units.iter().any(|unit| {
            unit.hp > 0
                && unit.kind.stats().domain == Domain::Ground
                && obs.visible(unit.tile)
                && Self::frame_contains(frame, unit.tile)
        });
        footprint_explored
            && !visibly_occupied
            && !Self::source_in_salvage_incident(obs, frame)
            && !self.harvest_location_contested(frame)
    }

    pub(super) fn foundry_supports_extractor(foundry: TilePos, extractor: TilePos) -> bool {
        fn axis_distance(a: i32, a_len: i32, b: i32, b_len: i32) -> i32 {
            let a_far = a + a_len - 1;
            let b_far = b + b_len - 1;
            (a - b_far).max(b - a_far).max(0)
        }

        let foundry_size = BuildingKind::Foundry.base_stats().size;
        let extractor_size = BuildingKind::Extractor.base_stats().size;
        axis_distance(foundry.x, foundry_size.0, extractor.x, extractor_size.0).max(axis_distance(
            foundry.y,
            foundry_size.1,
            extractor.y,
            extractor_size.1,
        )) <= crate::stats::EXTRACTOR_SUPPORT_RADIUS
    }

    fn frame_has_foundry_support(obs: &Observation, frame: TilePos) -> bool {
        obs.my_buildings.iter().any(|building| {
            building.kind == BuildingKind::Foundry
                && building.built
                && Self::foundry_supports_extractor(building.anchor, frame)
        })
    }

    pub(super) fn supported_frame_restoration_needed(
        &self,
        obs: &Observation,
        home: TilePos,
    ) -> bool {
        let deferred = Self::deferred_claims(obs);
        obs.known_frames.iter().any(|frame| {
            self.player_can_plan_frame_restoration(obs, *frame)
                && Self::frame_has_foundry_support(obs, *frame)
                && Self::ground_route_known(obs, home, *frame)
                && !obs
                    .my_buildings
                    .iter()
                    .any(|building| building.anchor == *frame)
                && !obs
                    .enemy_buildings
                    .iter()
                    .any(|building| building.anchor == *frame)
                && !deferred
                    .iter()
                    .any(|(kind, anchor)| *kind == BuildingKind::Extractor && *anchor == *frame)
        })
    }

    fn unsupported_extractors(
        obs: &Observation,
        home: TilePos,
        projected_foundries: &[TilePos],
    ) -> Vec<TilePos> {
        let mut extractors: Vec<_> = obs
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Extractor && building.built)
            .filter(|extractor| {
                projected_foundries
                    .iter()
                    .all(|foundry| !Self::foundry_supports_extractor(*foundry, extractor.anchor))
            })
            .filter(|extractor| Self::ground_route_known(obs, home, extractor.anchor))
            .map(|extractor| {
                (
                    extractor.anchor.chebyshev(home),
                    extractor.anchor.y,
                    extractor.anchor.x,
                    extractor.id,
                    extractor.anchor,
                )
            })
            .collect();
        extractors.sort_unstable();
        extractors
            .into_iter()
            .map(|(_, _, _, _, anchor)| anchor)
            .collect()
    }

    pub(super) fn construction_builders<'a>(
        &self,
        obs: &'a Observation,
        enlisted: &[UnitId],
        reserved: &[UnitId],
    ) -> Vec<&'a UnitObs> {
        obs.my_units
            .iter()
            .filter(|unit| {
                unit.kind.stats().harvest.is_some()
                    && unit.site.is_none()
                    && unit.founding.is_none()
                    && !enlisted.contains(&unit.id)
                    && !reserved.contains(&unit.id)
                    && self.scout != Some(unit.id)
            })
            .collect()
    }

    fn supporting_foundry_site(
        &self,
        obs: &Observation,
        extractor: TilePos,
        builders: &[&UnitObs],
        danger: &danger::HarvestDangerProjection,
    ) -> Option<(TilePos, UnitId)> {
        let kind = BuildingKind::Foundry;
        let size = kind.base_stats().size;
        let extractor_size = BuildingKind::Extractor.base_stats().size;
        // Two 2x2 footprints can have an eight-tile edge gap while their
        // anchors differ by nine. Scan the complete support geometry rather
        // than the smaller generic construction ring.
        let farthest_anchor = crate::stats::EXTRACTOR_SUPPORT_RADIUS
            + size
                .0
                .max(size.1)
                .max(extractor_size.0)
                .max(extractor_size.1)
            - 1;
        let mut candidates = Vec::new();
        for radius in 2..=farthest_anchor {
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    if dx.abs().max(dy.abs()) != radius {
                        continue;
                    }
                    let anchor = extractor.offset(dx, dy);
                    if Self::foundry_supports_extractor(anchor, extractor) {
                        candidates.push(anchor);
                    }
                }
            }
        }
        let mut selected_builder = None;
        let anchor = self.first_valid_placement_where(obs, kind, candidates, |anchor| {
            selected_builder = self.safe_foundry_builder(obs, anchor, builders, danger);
            selected_builder.is_some()
        })?;
        Some((
            anchor,
            selected_builder.expect("an accepted Foundry site selected one builder"),
        ))
    }

    fn safe_foundry_builder(
        &self,
        obs: &Observation,
        anchor: TilePos,
        builders: &[&UnitObs],
        danger: &danger::HarvestDangerProjection,
    ) -> Option<UnitId> {
        let size = BuildingKind::Foundry.base_stats().size;
        let (width, height) = size;
        let site_is_safe = (-1..=height).all(|dy| {
            (-1..=width).all(|dx| {
                let tile = anchor.offset(dx, dy);
                !self.harvest_location_contested(tile) && !danger.contains(tile)
            })
        });
        if !site_is_safe {
            return None;
        }
        let defer = (0..height).any(|dy| (0..width).any(|dx| !obs.visible(anchor.offset(dx, dy))));
        let mut candidates = builders.to_vec();
        candidates.sort_unstable_by_key(|builder| (builder.tile.manhattan(anchor), builder.id));
        candidates
            .into_iter()
            .find(|builder| {
                crate::bot::routing::build_command_path_avoids(
                    obs,
                    builder,
                    anchor,
                    size,
                    defer,
                    |tile| {
                        !obs.explored(tile)
                            || self.harvest_location_contested(tile)
                            || danger.contains(tile)
                    },
                )
            })
            .map(|builder| builder.id)
    }

    fn generic_foundry_site(
        &self,
        obs: &Observation,
        focus: TilePos,
        builders: &[&UnitObs],
        danger: &danger::HarvestDangerProjection,
    ) -> Option<(TilePos, UnitId)> {
        let candidates = (3i32..=7).flat_map(|radius| {
            (-radius..=radius).flat_map(move |dy| {
                (-radius..=radius)
                    .filter(move |dx| dx.abs().max(dy.abs()) == radius)
                    .map(move |dx| focus.offset(dx, dy))
            })
        });
        let mut selected_builder = None;
        let anchor =
            self.first_valid_placement_where(obs, BuildingKind::Foundry, candidates, |anchor| {
                selected_builder = self.safe_foundry_builder(obs, anchor, builders, danger);
                selected_builder.is_some()
            })?;
        Some((
            anchor,
            selected_builder.expect("an accepted Foundry site selected one builder"),
        ))
    }

    fn enemy_controls_frontier(
        obs: &Observation,
        projected_foundries: &[TilePos],
        frontier: TilePos,
    ) -> bool {
        let own_distance = projected_foundries
            .iter()
            .map(|foundry| foundry.chebyshev(frontier))
            .min();
        obs.enemy_buildings
            .iter()
            .filter(|building| building.built && building.kind == BuildingKind::Foundry)
            .map(|building| building.anchor.chebyshev(frontier))
            .min()
            .zip(own_distance)
            .is_some_and(|(enemy, own)| enemy < own)
    }

    pub(super) fn supporting_foundry_claim(
        &self,
        obs: &Observation,
        home: TilePos,
        projected_foundries: &[TilePos],
        builders: &[&UnitObs],
    ) -> Option<(TilePos, TilePos, UnitId)> {
        let danger = self.harvest_danger_projection(obs, None, None);
        Self::unsupported_extractors(obs, home, projected_foundries)
            .into_iter()
            .filter(|extractor| !self.harvest_location_contested(*extractor))
            .find_map(|extractor| {
                self.supporting_foundry_site(obs, extractor, builders, &danger)
                    .map(|(anchor, builder)| (extractor, anchor, builder))
            })
    }

    pub(super) fn player_facing_foundry_claim(
        &self,
        obs: &Observation,
        home: TilePos,
        projected_foundries: &[TilePos],
        builders: &[&UnitObs],
        support_extractors: bool,
        ordinary_frontiers: bool,
    ) -> Option<(TilePos, TilePos, UnitId)> {
        if support_extractors
            && let Some(claim) =
                self.supporting_foundry_claim(obs, home, projected_foundries, builders)
        {
            return Some(claim);
        }
        if !ordinary_frontiers {
            return None;
        }

        let danger = self.harvest_danger_projection(obs, None, None);
        let mut frontiers: Vec<_> = obs
            .known_scrap
            .iter()
            .filter(|(_, amount)| *amount > 0)
            .map(|(tile, _)| *tile)
            .chain(obs.known_frames.iter().copied().filter(|frame| {
                !obs.my_buildings
                    .iter()
                    .chain(obs.enemy_buildings.iter())
                    .any(|building| building.anchor == *frame)
            }))
            .filter(|frontier| {
                projected_foundries
                    .iter()
                    .all(|foundry| foundry.chebyshev(*frontier) > EXPANSION_RADIUS)
                    && Self::ground_route_known(obs, home, *frontier)
                    && !self.harvest_location_contested(*frontier)
                    && !Self::enemy_controls_frontier(obs, projected_foundries, *frontier)
            })
            .map(|frontier| {
                let distance = projected_foundries
                    .iter()
                    .map(|foundry| foundry.chebyshev(frontier))
                    .min()
                    .unwrap_or(0);
                (distance, frontier.y, frontier.x, frontier)
            })
            .collect();
        frontiers.sort_unstable();
        frontiers.into_iter().find_map(|(_, _, _, frontier)| {
            self.generic_foundry_site(obs, frontier, builders, &danger)
                .map(|(anchor, builder)| (frontier, anchor, builder))
        })
    }

    fn recurring_income_per_minute(obs: &Observation) -> u32 {
        // This is construction planning, not a current-cash-flow report. Paid
        // sites and deferred claims count at their eventual rate so the bot
        // does not start duplicate passive-income projects while crews work.
        let cadence_income = |period: u64| {
            u32::try_from(u64::from(crate::TICKS_PER_SECOND) * 60 / period)
                .expect("one-minute income fits u32")
        };
        let standing = obs
            .my_buildings
            .iter()
            .map(|building| match building.kind {
                BuildingKind::Reclaimer if building.tier == 0 => {
                    cadence_income(crate::stats::RECLAIMER_PERIOD)
                }
                BuildingKind::Reclaimer => cadence_income(crate::stats::REFINERY_PERIOD),
                _ if !building.built => 0,
                BuildingKind::Foundry if obs.tick >= crate::stats::FOUNDRY_DRIP_START_TICK => {
                    cadence_income(crate::stats::FOUNDRY_DRIP_PERIOD)
                }
                BuildingKind::Extractor
                    if Self::frame_has_foundry_support(obs, building.anchor) =>
                {
                    crate::stats::EXTRACTOR_SUPPORTED_INCOME_PER_MINUTE
                }
                BuildingKind::Extractor => crate::stats::EXTRACTOR_REMOTE_INCOME_PER_MINUTE,
                _ => 0,
            })
            .fold(0, u32::saturating_add);
        let deferred = Self::deferred_claims(obs)
            .iter()
            .filter(|(kind, _)| *kind == BuildingKind::Reclaimer)
            .map(|_| cadence_income(crate::stats::RECLAIMER_PERIOD))
            .fold(0, u32::saturating_add);
        standing.saturating_add(deferred)
    }

    fn recurring_income_target(obs: &Observation) -> u32 {
        let producers = obs
            .my_buildings
            .iter()
            .filter(|building| {
                building.built && !building.kind.tier_stats(building.tier).produces.is_empty()
            })
            .count();
        u32::try_from(producers)
            .unwrap_or(u32::MAX)
            .saturating_mul(PASSIVE_INCOME_PER_PRODUCER)
    }

    /// Restore a known frame, raise advanced production, expand, and
    /// upgrade one structure per think without starving the army budget.
    /// Returns true when this channel spent the think.
    fn advanced_construction(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        context: AdvancedConstructionContext<'_>,
        budget: &mut u32,
        intents: &mut Vec<Intent>,
    ) -> bool {
        let AdvancedConstructionContext {
            home,
            player_facing,
            builders,
            reserved,
        } = context;
        let have = |kind: BuildingKind| Self::projected_count(obs, kind, player_facing) > 0;
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
            // A frame's anchor is FIXED, so it must never enter the
            // pending/dead blacklists: one think whose builders were
            // all claimed elsewhere would poison the only anchor the
            // Extractor can ever have. The intent simply re-issues
            // until a standing site claims the frame.
            let claimed = |anchor: TilePos| {
                obs.my_buildings.iter().any(|b| b.anchor == anchor)
                    || obs.enemy_buildings.iter().any(|b| b.anchor == anchor)
                    || (player_facing
                        && Self::deferred_claims(obs).iter().any(|(kind, pending)| {
                            *kind == BuildingKind::Extractor && *pending == anchor
                        }))
            };
            let frame = obs
                .known_frames
                .iter()
                .filter(|f| !claimed(**f))
                .filter(|f| !player_facing || self.player_can_plan_frame_restoration(obs, **f))
                // A frame no builder can walk to must not be
                // claimed: the intent would re-issue forever and
                // starve every deeper construction rung (the
                // island-map deadlock). The road must be KNOWN —
                // the optimistic flood survives any unexplored
                // gulf, and a cross-strait frame it admits eats
                // every construction think until the map dies.
                .filter(|f| Self::ground_route_known(obs, home, **f))
                .filter(|f| {
                    *budget >= cost + TECH_RESERVE
                        || (player_facing
                            && *budget >= cost
                            && Self::frame_has_foundry_support(obs, **f))
                })
                .min_by_key(|f| {
                    (
                        u8::from(player_facing && !Self::frame_has_foundry_support(obs, **f)),
                        f.chebyshev(home),
                        f.y,
                        f.x,
                    )
                })
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
        // Expansion: once the tree stands, a second Foundry toward the
        // nearest salvage frontier no Foundry serves — forward
        // production, a drop-off that shortens the haul, and one more
        // victory token the enemy must come dig out.
        if dials.expansion && have_built(BuildingKind::Foundry) {
            let cost = BuildingKind::Foundry
                .base_stats()
                .construction
                .map(|c| c.cost)
                .unwrap_or(0);
            let (foundries, pending_foundries) = if player_facing {
                Self::projected_foundries(obs)
            } else {
                (
                    obs.my_buildings
                        .iter()
                        .filter(|building| building.kind == BuildingKind::Foundry)
                        .map(|building| building.anchor)
                        .collect(),
                    0,
                )
            };
            if pending_foundries == 0
                && foundries.len() < dials.foundry_cap
                && (!player_facing
                    || super::production::extra_foundry_core_ready(obs, reserved, foundries.len()))
                && *budget >= cost + TECH_RESERVE
            {
                let player_claim = player_facing
                    .then(|| {
                        self.player_facing_foundry_claim(
                            obs,
                            home,
                            &foundries,
                            builders,
                            have_built(BuildingKind::Fabricator),
                            !dials.deep_tech || have(BuildingKind::Airworks),
                        )
                    })
                    .flatten();
                if let Some((_objective, anchor, builder)) = player_claim {
                    *budget -= cost;
                    let before_harvest = intents
                        .iter()
                        .position(|intent| matches!(intent, Intent::AssignHarvest { .. }))
                        .unwrap_or(intents.len());
                    intents.insert(
                        before_harvest,
                        Intent::BuildWith {
                            builder,
                            kind: BuildingKind::Foundry,
                            anchor,
                        },
                    );
                    return true;
                }

                if !player_facing && (!dials.deep_tech || have(BuildingKind::Airworks)) {
                    let frontier = obs
                        .known_scrap
                        .iter()
                        .filter(|(_, amount)| *amount > 0)
                        .map(|(tile, _)| *tile)
                        .chain(obs.known_frames.iter().copied().filter(|frame| {
                            !obs.my_buildings
                                .iter()
                                .chain(obs.enemy_buildings.iter())
                                .any(|building| building.anchor == *frame)
                        }))
                        .filter(|tile| {
                            foundries
                                .iter()
                                .all(|f| f.chebyshev(*tile) > EXPANSION_RADIUS)
                                && Self::ground_route_known(obs, home, *tile)
                                && (!player_facing || !self.harvest_location_contested(*tile))
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
                        intents.push(Intent::Build {
                            kind: BuildingKind::Foundry,
                            anchor,
                        });
                        return true;
                    }
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
        claims: ConstructionClaims<'_>,
        budget: &mut u32,
        intents: &mut Vec<Intent>,
    ) {
        let ConstructionClaims {
            player_facing,
            enlisted,
            reserved,
        } = claims;
        // Orphan relief is free (resuming an own site charges nothing).
        let builders = self.construction_builders(obs, enlisted, reserved);
        let mut routes = crate::bot::routing::RouteProjection::new(obs, Domain::Ground);
        let orphan = obs
            .my_buildings
            .iter()
            .filter(|b| {
                !b.built && b.tier == 0 && !obs.my_units.iter().any(|u| u.site == Some(b.id))
            })
            .filter(|site| {
                !player_facing
                    || (!self.harvest_location_contested(site.anchor)
                        && builders.iter().any(|builder| {
                            routes.group_reaches_command_goal(&[builder.id], site.anchor)
                        }))
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
            && self.advanced_construction(
                dials,
                obs,
                AdvancedConstructionContext {
                    home,
                    player_facing,
                    builders: &builders,
                    reserved,
                },
                budget,
                intents,
            )
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
            let have_fab = Self::projected_count(obs, BuildingKind::Fabricator, player_facing) > 0;
            if let Some(cost) = fab_cost
                && !have_fab
                && harvesters >= dials.harvester_target.min(3) as usize
                && *budget >= cost + TECH_RESERVE
                && let Some(anchor) = self.placement_near(obs, BuildingKind::Fabricator, home)
            {
                *budget -= cost;
                intents.push(Intent::Build {
                    kind: BuildingKind::Fabricator,
                    anchor,
                });
                return;
            }
        }

        let proactive_turret_cap = usize::from(
            dials.adaptive_composition
                && dials.turret_cap > 0
                && Self::enemy_site(obs, home).is_some(),
        );
        let turret_limit = if self.raided {
            dials.turret_cap
        } else {
            proactive_turret_cap
        };
        if dials.turret_response && turret_limit > 0 {
            let turret_cost = BuildingKind::Turret
                .base_stats()
                .construction
                .map(|c| c.cost);
            let turrets = Self::projected_count(obs, BuildingKind::Turret, player_facing);
            if let Some(cost) = turret_cost
                && turrets < turret_limit
                && *budget >= cost + UnitKind::Harvester.stats().cost
                && let Some(node) = self.nearest_scrap(obs, home)
                && let Some(anchor) = self.placement_near(obs, BuildingKind::Turret, node)
            {
                *budget -= cost;
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
            let charges = Self::projected_count(obs, BuildingKind::ScuttleCharge, player_facing);
            let charge_cost = BuildingKind::ScuttleCharge
                .base_stats()
                .construction
                .map(|c| c.cost);
            if harvesters >= dials.harvester_target as usize
                && have_fab
                && charges < dials.mine_cap
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
                        intents.push(Intent::Build {
                            kind: BuildingKind::ScuttleCharge,
                            anchor,
                        });
                        return;
                    }
                }
            }
        }

        // The sky over the economy: confirmed enemy air raises flak over the
        // harvest line. An anonymous radar blip cannot justify specialized
        // spending for the player-facing controller; the profile-free QA
        // controller retains its historical blip response.
        let air_evidence = self.seen_air || (!player_facing && !obs.blips.is_empty());
        if dials.aa_response && air_evidence {
            let flak_cost = BuildingKind::FlakTurret
                .base_stats()
                .construction
                .map(|c| c.cost);
            let flak = Self::projected_count(obs, BuildingKind::FlakTurret, player_facing);
            if let Some(cost) = flak_cost
                && flak < dials.flak_cap
                && *budget >= cost + UnitKind::Harvester.stats().cost
                && let Some(node) = self.nearest_scrap(obs, home)
                && let Some(anchor) = self.placement_near(obs, BuildingKind::FlakTurret, node)
            {
                *budget -= cost;
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
            let have_array = Self::projected_count(obs, BuildingKind::Array, player_facing) > 0;
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
                intents.push(Intent::Build {
                    kind: BuildingKind::Array,
                    anchor,
                });
                return;
            }
        }

        // High-support identities establish one repair point once the first
        // tech rung stands. It is sustain for a developed army, not opening
        // infrastructure.
        if dials.adaptive_composition && (dials.support_target >= 3 || obs.tick >= 6_000) {
            let have_fabricator = obs
                .my_buildings
                .iter()
                .any(|building| building.kind == BuildingKind::Fabricator && building.built);
            let have_repair_bay =
                Self::projected_count(obs, BuildingKind::RepairBay, player_facing) > 0;
            let cost = BuildingKind::RepairBay
                .base_stats()
                .construction
                .map(|construction| construction.cost);
            if have_fabricator
                && !have_repair_bay
                && let Some(cost) = cost
                && *budget >= cost + TECH_RESERVE
                && let Some(anchor) = self.placement_near(obs, BuildingKind::RepairBay, home)
            {
                *budget -= cost;
                intents.push(Intent::Build {
                    kind: BuildingKind::RepairBay,
                    anchor,
                });
                return;
            }
        }

        // Siege-heavy identities anchor one long gun after locating the
        // enemy. Mobile artillery remains the primary pressure tool; the
        // Bastion makes a developed defensive line harder to rush through.
        if dials.adaptive_composition && dials.siege_target >= 3 {
            let have_fabricator = obs
                .my_buildings
                .iter()
                .any(|building| building.kind == BuildingKind::Fabricator && building.built);
            let have_bastion = Self::projected_count(obs, BuildingKind::Bastion, player_facing) > 0;
            let cost = BuildingKind::Bastion
                .base_stats()
                .construction
                .map(|construction| construction.cost);
            if have_fabricator
                && !have_bastion
                && Self::enemy_site(obs, home).is_some()
                && let Some(cost) = cost
                && *budget >= cost + TECH_RESERVE
                && let Some(anchor) = self.placement_near(obs, BuildingKind::Bastion, home)
            {
                *budget -= cost;
                intents.push(Intent::Build {
                    kind: BuildingKind::Bastion,
                    anchor,
                });
                return;
            }
        }

        // Reclaimers once the patches near home run dry. Player-facing play
        // adds passive capacity only while completed producers remain
        // underfunded after already-paid and promised income comes online;
        // the frozen Overseer retains its historical count cap.
        if dials.reclaimers {
            let near_home: u32 = obs
                .known_scrap
                .iter()
                .chain(obs.known_wrecks.iter())
                .filter(|(pos, _)| pos.chebyshev(home) <= HOME_SALVAGE_RADIUS)
                .map(|(_, amount)| amount)
                .sum();
            let reclaimers = Self::projected_count(obs, BuildingKind::Reclaimer, player_facing);
            let rec_cost = BuildingKind::Reclaimer
                .base_stats()
                .construction
                .map(|c| c.cost);
            let needs_income = if player_facing {
                Self::recurring_income_per_minute(obs) < Self::recurring_income_target(obs)
            } else {
                reclaimers < dials.reclaimer_cap
            };
            if near_home < SALVAGE_LOW
                && needs_income
                && let Some(cost) = rec_cost
                && *budget >= cost + TECH_RESERVE
                && let Some(anchor) = self.placement_near(obs, BuildingKind::Reclaimer, home)
            {
                *budget -= cost;
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
        mode: PolicyMode<'_>,
        budget: &mut u32,
        intents: &mut Vec<Intent>,
    ) {
        if !dials.repair {
            return;
        }
        // Repair is a persistent unit program. Keep the active welder on its
        // patient instead of emitting the same replacement command every
        // think (and eventually drafting every other Harvester onto it).
        if obs
            .my_units
            .iter()
            .any(|unit| unit.kind.stats().harvest.is_some() && unit.repairing)
        {
            return;
        }
        // Reserve: a sentinel's price stays banked, and the trickle
        // itself is cheap — gate on the reserve, not the full damage.
        let reserve = UnitKind::Sentinel.stats().cost;
        if *budget < reserve {
            return;
        }
        let has_wounded_patient = obs.my_buildings.iter().any(|building| {
            building.built && building.hp * 10 < building.kind.tier_stats(building.tier).max_hp * 8
        });
        let danger = (mode.player_facing && has_wounded_patient).then(|| {
            self.harvest_danger_projection(obs, mode.unit_contacts, mode.building_contacts)
        });
        let patient = obs
            .my_buildings
            .iter()
            .filter(|b| b.built && b.hp * 10 < b.kind.tier_stats(b.tier).max_hp * 8)
            .filter(|b| {
                !mode.player_facing
                    || !self.repair_patient_unsafe(
                        b,
                        danger
                            .as_deref()
                            .expect("player-facing repair prepared worker danger"),
                    )
            })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::Executive;
    use crate::bot::observation::{BuildingObs, OBSERVATION_VERSION, UnitObs};
    use crate::event::Event;
    use crate::ids::{BuildingId, PlayerId, UnitId};
    use crate::scenario::{BotConfig, BotDifficulty, BotStance};
    use crate::state::{Faction, Order};
    use crate::{Command, PlayerCommand, Scenario};

    const HOME: TilePos = TilePos::new(4, 10);

    fn observation() -> Observation {
        Observation {
            version: OBSERVATION_VERSION,
            tick: 0,
            me: PlayerId(0),
            scrap: 10_000,
            map_width: 40,
            map_height: 24,
            my_units: vec![harvester(1, TilePos::new(8, 11), None)],
            my_buildings: vec![building(0, PlayerId(0), BuildingKind::Foundry, HOME)],
            my_queues: vec![Vec::new()],
            ally_units: Vec::new(),
            ally_buildings: Vec::new(),
            enemy_units: Vec::new(),
            enemy_buildings: Vec::new(),
            visible: vec![true; 40 * 24],
            explored: vec![true; 40 * 24],
            known_scrap: vec![(TilePos::new(10, 10), 200)],
            known_rock: Vec::new(),
            known_frames: Vec::new(),
            known_peaks: Vec::new(),
            known_wrecks: Vec::new(),
            salvage_incidents: Vec::new(),
            blips: Vec::new(),
            faction: Faction::Ferrous,
            my_shells: 0,
            incoming_shells: Vec::new(),
        }
    }

    fn harvester(id: u32, tile: TilePos, founding: Option<(BuildingKind, TilePos)>) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(0),
            kind: UnitKind::Harvester,
            tile,
            hp: UnitKind::Harvester.stats().max_hp,
            idle: founding.is_none(),
            carrying: 0,
            cargo: 0,
            site: None,
            salvaging: None,
            founding,
            repairing: false,
        }
    }

    fn sentinel(id: u32, tile: TilePos) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(0),
            kind: UnitKind::Sentinel,
            tile,
            hp: UnitKind::Sentinel.stats().max_hp,
            idle: true,
            carrying: 0,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
        }
    }

    fn building(id: u32, player: PlayerId, kind: BuildingKind, anchor: TilePos) -> BuildingObs {
        BuildingObs {
            id: BuildingId(id),
            player,
            kind,
            anchor,
            hp: kind.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        }
    }

    fn focused_dials() -> Dials {
        let mut dials = Dials::full();
        dials.tech = false;
        dials.turret_response = false;
        dials.aa_response = false;
        dials.radar = false;
        dials.reclaimers = false;
        dials.deep_tech = false;
        dials.extractors = false;
        dials.upgrades = false;
        dials.expansion = false;
        dials.mines = false;
        dials.adaptive_composition = false;
        dials
    }

    fn construction_intents(
        policy: &mut UtilityPolicy,
        dials: &Dials,
        obs: &Observation,
    ) -> Vec<Intent> {
        construction_intents_for(policy, dials, obs, true)
    }

    fn construction_intents_for(
        policy: &mut UtilityPolicy,
        dials: &Dials,
        obs: &Observation,
        player_facing: bool,
    ) -> Vec<Intent> {
        let mut budget = obs.scrap;
        let mut intents = Vec::new();
        policy.construction(
            dials,
            obs,
            HOME,
            ConstructionClaims {
                player_facing,
                enlisted: &[],
                reserved: &[],
            },
            &mut budget,
            &mut intents,
        );
        intents
    }

    fn assert_build_kind(intents: &[Intent], expected: BuildingKind) -> TilePos {
        let [intent] = intents else {
            panic!("expected one {expected:?} build, got {intents:?}");
        };
        let (kind, anchor) = match intent {
            Intent::Build { kind, anchor } | Intent::BuildWith { kind, anchor, .. } => {
                (kind, anchor)
            }
            _ => panic!("expected one {expected:?} build, got {intents:?}"),
        };
        assert_eq!(*kind, expected);
        *anchor
    }

    fn developed_expansion_observation() -> Observation {
        let mut obs = observation();
        obs.map_width = 72;
        obs.map_height = 30;
        obs.visible = vec![true; 72 * 30];
        obs.explored = vec![true; 72 * 30];
        for (id, kind, anchor) in [
            (1, BuildingKind::Fabricator, TilePos::new(4, 3)),
            (2, BuildingKind::Airworks, TilePos::new(9, 3)),
            (3, BuildingKind::Crucible, TilePos::new(14, 3)),
        ] {
            obs.my_buildings
                .push(building(id, PlayerId(0), kind, anchor));
            obs.my_queues.push(Vec::new());
        }
        obs.known_scrap = vec![(TilePos::new(32, 12), 800), (TilePos::new(62, 12), 800)];
        obs
    }

    fn expansion_dials() -> Dials {
        let mut dials = focused_dials();
        dials.tech = true;
        dials.deep_tech = true;
        dials.expansion = true;
        dials.foundry_cap = 4;
        dials
    }

    fn generic_expansion_claim(
        policy: &UtilityPolicy,
        obs: &Observation,
    ) -> Option<(TilePos, TilePos, UnitId)> {
        let (foundries, _) = UtilityPolicy::projected_foundries(obs);
        let builders = policy.construction_builders(obs, &[], &[]);
        policy.player_facing_foundry_claim(obs, HOME, &foundries, &builders, false, true)
    }

    #[test]
    fn first_expansion_remains_available_without_an_ordinary_screen() {
        let obs = developed_expansion_observation();
        let intents = construction_intents(&mut UtilityPolicy::new(), &expansion_dials(), &obs);
        let anchor = assert_build_kind(&intents, BuildingKind::Foundry);
        assert!(
            anchor.chebyshev(TilePos::new(32, 12)) < anchor.chebyshev(TilePos::new(62, 12)),
            "the common second Foundry should claim the nearer unserved frontier"
        );
    }

    #[test]
    fn generic_expansion_requires_one_safe_command_feasible_claim() {
        let frontier = TilePos::new(32, 12);
        let mut ready = developed_expansion_observation();
        ready.known_scrap = vec![(frontier, 800)];
        let expected = generic_expansion_claim(&UtilityPolicy::new(), &ready)
            .expect("the open frontier has one safe expansion claim");
        assert_eq!(expected.0, frontier);
        assert_eq!(expected.2, UnitId(1));

        let mut no_builder = ready.clone();
        no_builder.my_units.clear();

        let mut unreachable = ready.clone();
        unreachable.known_rock = (0..unreachable.map_height)
            .map(|y| TilePos::new(20, y))
            .collect();

        let mut blocked_policy = UtilityPolicy::new();
        blocked_policy.dead_anchors = (3i32..=7)
            .flat_map(|radius| {
                (-radius..=radius).flat_map(move |dy| {
                    (-radius..=radius)
                        .filter(move |dx| dx.abs().max(dy.abs()) == radius)
                        .map(move |dx| frontier.offset(dx, dy))
                })
            })
            .collect();

        let mut contested_policy = UtilityPolicy::new();
        contested_policy
            .contested_harvest_regions
            .push(ContestedHarvestRegion {
                center: frontier,
                last_evidence: ready.tick,
                clear_since: None,
            });

        let mut enemy_side = ready.clone();
        enemy_side.enemy_buildings.push(building(
            90,
            PlayerId(1),
            BuildingKind::Foundry,
            frontier.offset(2, 0),
        ));

        let mut hostile = ready.clone();
        hostile.enemy_units.push(UnitObs {
            id: UnitId(90),
            player: PlayerId(1),
            kind: UnitKind::Avalanche,
            tile: frontier,
            hp: UnitKind::Avalanche.stats().max_hp,
            idle: false,
            carrying: 0,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
        });

        let cases = [
            ("no builder", UtilityPolicy::new(), &no_builder),
            ("unreachable", UtilityPolicy::new(), &unreachable),
            ("blocked", blocked_policy, &ready),
            ("contested", contested_policy, &ready),
            ("enemy side", UtilityPolicy::new(), &enemy_side),
            ("hostile footprint", UtilityPolicy::new(), &hostile),
        ];
        for (label, policy, obs) in cases {
            assert!(
                generic_expansion_claim(&policy, obs).is_none(),
                "{label} frontier unexpectedly retained a claim"
            );
        }

        let intents = construction_intents(&mut UtilityPolicy::new(), &expansion_dials(), &ready);
        assert!(matches!(
            intents.as_slice(),
            [Intent::BuildWith {
                builder: UnitId(1),
                kind: BuildingKind::Foundry,
                anchor,
            }] if *anchor == expected.1
        ));

        let mut policy = UtilityPolicy::new();
        let mut ordered = vec![Intent::AssignHarvest {
            unit: UnitId(1),
            node: frontier,
        }];
        let mut budget = ready.scrap;
        policy.construction(
            &expansion_dials(),
            &ready,
            HOME,
            ConstructionClaims {
                player_facing: true,
                enlisted: &[],
                reserved: &[],
            },
            &mut budget,
            &mut ordered,
        );
        assert!(matches!(
            ordered.as_slice(),
            [
                Intent::BuildWith {
                    builder: UnitId(1),
                    kind: BuildingKind::Foundry,
                    anchor,
                },
                Intent::AssignHarvest {
                    unit: UnitId(1),
                    ..
                },
            ] if *anchor == expected.1
        ));
        policy.bind_player_facing_builders(&ready, &[], &[], &[], &[], &mut ordered);
        let commands = Executive::new().apply_with_reservations(PlayerId(0), &ready, &ordered, &[]);
        assert!(matches!(
            commands.as_slice(),
            [PlayerCommand {
                command: Command::Build {
                    units,
                    kind: BuildingKind::Foundry,
                    anchor,
                    ..
                },
                ..
            }] if units == &vec![UnitId(1)] && *anchor == expected.1
        ));
    }

    #[test]
    fn greed_can_support_a_safe_third_foundry_at_owned_renewable_income() {
        let profile = BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, 1_616_304)
            .resolve_profile();
        let dials = Dials::scripted(
            &profile,
            DifficultyTuning::for_level(BotDifficulty::Standard),
        );
        assert_eq!((profile.traits.greed, dials.foundry_cap), (64, 3));

        let mut obs = developed_expansion_observation();
        obs.known_scrap.clear();
        let extractor = TilePos::new(34, 15);
        obs.my_buildings.extend([
            building(4, PlayerId(0), BuildingKind::Foundry, TilePos::new(15, 10)),
            building(5, PlayerId(0), BuildingKind::Extractor, extractor),
        ]);
        obs.my_queues.extend([Vec::new(), Vec::new()]);
        obs.my_units.extend((0..6).map(|index| {
            sentinel(
                100 + index,
                HOME.offset(
                    i32::try_from(index).expect("small fixture index fits i32"),
                    6,
                ),
            )
        }));

        let intents = construction_intents(&mut UtilityPolicy::new(), &dials, &obs);
        let [
            Intent::BuildWith {
                builder,
                kind: BuildingKind::Foundry,
                anchor,
            },
        ] = intents.as_slice()
        else {
            panic!(
                "the protected renewable frontier should admit the Greed expansion: {intents:?}"
            );
        };
        assert_eq!(*builder, UnitId(1));
        assert!(UtilityPolicy::foundry_supports_extractor(
            *anchor, extractor
        ));
    }

    #[test]
    fn later_foundry_admission_unlocks_at_six_then_twelve_sentinel_equivalents() {
        let mut obs = developed_expansion_observation();
        obs.my_buildings.push(building(
            4,
            PlayerId(0),
            BuildingKind::Foundry,
            TilePos::new(14, 10),
        ));
        obs.my_queues.push(Vec::new());
        obs.my_units
            .extend((100..105).map(|id| sentinel(id, TilePos::new(8 + (id - 100) as i32, 16))));
        let dials = expansion_dials();

        let five = construction_intents(&mut UtilityPolicy::new(), &dials, &obs);
        assert!(
            five.is_empty(),
            "five ordinary fighters must not admit the third Foundry: {five:?}"
        );

        let second_foundry = obs
            .my_buildings
            .iter()
            .position(|building| building.id == BuildingId(4))
            .expect("the fixture has a second Foundry");
        obs.my_queues[second_foundry].push(UnitKind::Sentinel);
        let six = construction_intents(&mut UtilityPolicy::new(), &dials, &obs);
        let third_anchor = assert_build_kind(&six, BuildingKind::Foundry);
        assert!(
            third_anchor.chebyshev(TilePos::new(32, 12))
                < third_anchor.chebyshev(TilePos::new(62, 12))
        );

        obs.my_buildings.push(building(
            5,
            PlayerId(0),
            BuildingKind::Foundry,
            third_anchor,
        ));
        obs.my_queues.push(Vec::new());
        obs.my_units
            .extend((105..110).map(|id| sentinel(id, TilePos::new(20 + (id - 105) as i32, 16))));
        let eleven = construction_intents(&mut UtilityPolicy::new(), &dials, &obs);
        assert!(
            eleven.is_empty(),
            "the fourth Foundry must scale past eleven Sentinel-equivalents: {eleven:?}"
        );

        obs.my_units.push(sentinel(110, TilePos::new(25, 16)));
        let twelve = construction_intents(&mut UtilityPolicy::new(), &dials, &obs);
        let fourth_anchor = assert_build_kind(&twelve, BuildingKind::Foundry);
        assert!(
            fourth_anchor.chebyshev(TilePos::new(62, 12))
                < fourth_anchor.chebyshev(TilePos::new(32, 12)),
            "the fourth Foundry should claim the remaining farther frontier"
        );
    }

    #[test]
    fn restoration_reserve_requires_a_safe_reachable_unclaimed_supported_frame() {
        let frame = HOME.offset(8, 0);
        let mut ready = observation();
        ready.known_frames.push(frame);
        let policy = UtilityPolicy::new();
        assert!(policy.supported_frame_restoration_needed(&ready, HOME));

        let mut own_claim = ready.clone();
        own_claim
            .my_buildings
            .push(building(2, PlayerId(0), BuildingKind::Extractor, frame));
        assert!(!policy.supported_frame_restoration_needed(&own_claim, HOME));

        let mut enemy_claim = ready.clone();
        enemy_claim
            .enemy_buildings
            .push(building(3, PlayerId(1), BuildingKind::Extractor, frame));
        assert!(!policy.supported_frame_restoration_needed(&enemy_claim, HOME));

        let mut deferred_claim = ready.clone();
        deferred_claim.my_units[0].founding = Some((BuildingKind::Extractor, frame));
        assert!(!policy.supported_frame_restoration_needed(&deferred_claim, HOME));

        let mut partially_unknown = ready.clone();
        let unknown = frame.offset(1, 1);
        partially_unknown.explored
            [(unknown.y * partially_unknown.map_width + unknown.x) as usize] = false;
        assert!(!policy.supported_frame_restoration_needed(&partially_unknown, HOME));

        let mut occupied = ready.clone();
        occupied.enemy_units.push(UnitObs {
            id: UnitId(20),
            player: PlayerId(1),
            kind: UnitKind::Sentinel,
            tile: frame.offset(1, 0),
            hp: UnitKind::Sentinel.stats().max_hp,
            idle: false,
            carrying: 0,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
        });
        assert!(!policy.supported_frame_restoration_needed(&occupied, HOME));

        let mut recent_battle = ready.clone();
        recent_battle.salvage_incidents.push(frame.offset(2, 0));
        assert!(!policy.supported_frame_restoration_needed(&recent_battle, HOME));

        let mut unsupported = ready;
        unsupported.my_buildings[0].anchor = TilePos::new(0, 0);
        assert!(!policy.supported_frame_restoration_needed(&unsupported, HOME));
    }

    #[test]
    fn salvage_waits_until_every_known_ground_source_is_exhausted() {
        let mut exhausted = observation();
        exhausted.scrap = 0;
        exhausted.known_scrap = vec![(TilePos::new(10, 10), 0)];
        exhausted.known_wrecks = vec![(TilePos::new(11, 10), 0)];
        exhausted.my_buildings.push(building(
            1,
            PlayerId(0),
            BuildingKind::Turret,
            HOME.offset(5, 0),
        ));

        let salvage = |obs: &Observation| {
            let mut intents = Vec::new();
            UtilityPolicy::new().salvage(&Dials::full(), obs, &mut intents);
            intents
        };
        assert_eq!(
            salvage(&exhausted),
            vec![Intent::Salvage {
                building: BuildingId(1)
            }],
            "zero-valued memories are exhausted; the cheapest static defense may fund one more wave"
        );

        let mut scrap_left = exhausted.clone();
        scrap_left.known_scrap[0].1 = 1;
        assert!(
            salvage(&scrap_left).is_empty(),
            "one known scrap must prevent premature liquidation"
        );

        let mut wreck_left = exhausted;
        wreck_left.known_wrecks[0].1 = 1;
        assert!(
            salvage(&wreck_left).is_empty(),
            "one known wreck must prevent premature liquidation"
        );
    }

    #[test]
    fn repair_bay_requires_completed_tech_and_is_not_promised_twice() {
        let mut dials = focused_dials();
        dials.adaptive_composition = true;
        dials.support_target = 3;

        let mut unfinished = observation();
        let mut fabricator = building(1, PlayerId(0), BuildingKind::Fabricator, HOME.offset(5, -5));
        fabricator.built = false;
        unfinished.my_buildings.push(fabricator.clone());
        assert_build_kind(
            &construction_intents(&mut UtilityPolicy::new(), &dials, &unfinished),
            BuildingKind::Fabricator,
        );

        let mut developed = unfinished;
        developed.my_buildings.last_mut().unwrap().built = true;
        let anchor = assert_build_kind(
            &construction_intents(&mut UtilityPolicy::new(), &dials, &developed),
            BuildingKind::RepairBay,
        );

        developed.my_units.push(harvester(
            2,
            HOME.offset(1, 3),
            Some((BuildingKind::RepairBay, anchor)),
        ));
        assert!(
            construction_intents(&mut UtilityPolicy::new(), &dials, &developed).is_empty(),
            "a deferred Repair Bay already satisfies the one-bay plan"
        );
    }

    #[test]
    fn bastion_requires_both_completed_tech_and_a_located_enemy() {
        let mut dials = focused_dials();
        dials.adaptive_composition = true;
        dials.siege_target = 3;

        let mut obs = observation();
        obs.my_buildings.push(building(
            1,
            PlayerId(0),
            BuildingKind::Fabricator,
            HOME.offset(5, -5),
        ));
        assert!(
            construction_intents(&mut UtilityPolicy::new(), &dials, &obs).is_empty(),
            "siege identity should not guess where an unseen opponent is"
        );

        obs.enemy_buildings.push(building(
            20,
            PlayerId(1),
            BuildingKind::Foundry,
            TilePos::new(32, 10),
        ));
        let anchor = assert_build_kind(
            &construction_intents(&mut UtilityPolicy::new(), &dials, &obs),
            BuildingKind::Bastion,
        );

        obs.my_units.push(harvester(
            2,
            HOME.offset(1, 3),
            Some((BuildingKind::Bastion, anchor)),
        ));
        assert!(
            construction_intents(&mut UtilityPolicy::new(), &dials, &obs).is_empty(),
            "a promised Bastion must count against the one-emplacement plan"
        );
    }

    #[test]
    fn anti_air_response_counts_only_own_or_promised_flak_toward_its_cap() {
        let mut dials = focused_dials();
        dials.aa_response = true;
        dials.flak_cap = 2;

        let mut no_contact = UtilityPolicy::new();
        assert!(construction_intents(&mut no_contact, &dials, &observation()).is_empty());

        let mut threatened = observation();
        threatened.enemy_buildings.push(building(
            20,
            PlayerId(1),
            BuildingKind::FlakTurret,
            TilePos::new(32, 10),
        ));
        let mut policy = UtilityPolicy::new();
        policy.seen_air = true;
        let first = assert_build_kind(
            &construction_intents(&mut policy, &dials, &threatened),
            BuildingKind::FlakTurret,
        );

        threatened.my_buildings.push(building(
            2,
            PlayerId(0),
            BuildingKind::FlakTurret,
            HOME.offset(4, 4),
        ));
        threatened.my_units.push(harvester(
            2,
            HOME.offset(1, 3),
            Some((BuildingKind::FlakTurret, first)),
        ));
        assert!(
            construction_intents(&mut policy, &dials, &threatened).is_empty(),
            "one standing and one promised own Flak exhaust the cap; enemy Flak does not"
        );
    }

    #[test]
    fn player_facing_blip_waits_for_confirmed_air_before_funding_flak() {
        let mut dials = focused_dials();
        dials.aa_response = true;
        dials.flak_cap = 3;

        let mut obs = observation();
        obs.enemy_units.push(UnitObs {
            id: UnitId(20),
            player: PlayerId(1),
            kind: UnitKind::Sentinel,
            tile: TilePos::new(20, 10),
            hp: UnitKind::Sentinel.stats().max_hp,
            idle: false,
            carrying: 0,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
        });
        let mut policy = UtilityPolicy::new();
        assert!(
            construction_intents(&mut policy, &dials, &obs).is_empty(),
            "a currently visible ground force is not evidence of hostile air"
        );

        obs.blips.push(TilePos::new(20, 10));
        assert!(
            construction_intents(&mut policy, &dials, &obs).is_empty(),
            "an unidentified radar contact cannot justify specialized AA spending"
        );

        policy.seen_air = true;
        for id in 2..=4 {
            let anchor = assert_build_kind(
                &construction_intents(&mut policy, &dials, &obs),
                BuildingKind::FlakTurret,
            );
            obs.my_buildings
                .push(building(id, PlayerId(0), BuildingKind::FlakTurret, anchor));
        }
        assert!(
            construction_intents(&mut policy, &dials, &obs).is_empty(),
            "confirmed hostile air unlocks exactly the configured AA cap"
        );
    }

    #[test]
    fn profile_free_overseer_keeps_its_full_blip_flak_cap() {
        let mut dials = focused_dials();
        dials.aa_response = true;
        dials.flak_cap = 3;

        let mut obs = observation();
        obs.blips.push(TilePos::new(20, 10));
        let mut policy = UtilityPolicy::new();
        for id in 2..=4 {
            let anchor = assert_build_kind(
                &construction_intents_for(&mut policy, &dials, &obs, false),
                BuildingKind::FlakTurret,
            );
            obs.my_buildings
                .push(building(id, PlayerId(0), BuildingKind::FlakTurret, anchor));
        }
        assert!(
            construction_intents_for(&mut policy, &dials, &obs, false).is_empty(),
            "the stable profile-free QA controller retains its full response to radar blips"
        );
    }

    #[test]
    fn every_player_facing_identity_gets_one_bounded_proactive_turret() {
        for cap in 1..=4 {
            let mut dials = focused_dials();
            dials.turret_response = true;
            dials.adaptive_composition = true;
            dials.turret_cap = cap;

            let mut obs = observation();
            obs.enemy_buildings.push(building(
                20,
                PlayerId(1),
                BuildingKind::Foundry,
                TilePos::new(32, 10),
            ));
            let mut policy = UtilityPolicy::new();
            let anchor = assert_build_kind(
                &construction_intents(&mut policy, &dials, &obs),
                BuildingKind::Turret,
            );
            obs.my_buildings
                .push(building(2, PlayerId(0), BuildingKind::Turret, anchor));
            assert!(
                construction_intents(&mut policy, &dials, &obs).is_empty(),
                "cap {cap} must allow exactly one proactive turret before a raid"
            );
        }
    }

    #[test]
    fn a_real_raid_unlocks_each_configured_turret_cap() {
        for cap in [2, 3, 4] {
            let mut dials = focused_dials();
            dials.turret_response = true;
            dials.adaptive_composition = true;
            dials.turret_cap = cap;

            let mut obs = observation();
            let mut policy = UtilityPolicy::new();
            policy.raided = true;
            for index in 0..cap {
                let anchor = assert_build_kind(
                    &construction_intents(&mut policy, &dials, &obs),
                    BuildingKind::Turret,
                );
                obs.my_buildings.push(building(
                    2 + u32::try_from(index).expect("small configured cap fits u32"),
                    PlayerId(0),
                    BuildingKind::Turret,
                    anchor,
                ));
            }
            assert!(
                construction_intents(&mut policy, &dials, &obs).is_empty(),
                "a recorded raid unlocks all {cap} configured turrets, but never another"
            );
        }
    }

    #[test]
    fn exact_bound_construction_command_is_accepted_by_state() {
        let mut state = Scenario::skirmish().build().expect("skirmish builds");
        let mut obs = Observation::omniscient(&state, PlayerId(0));
        assert!(
            !obs.known_scrap.is_empty(),
            "premise: placement has an economy focus"
        );

        let mut dials = focused_dials();
        dials.aa_response = true;
        dials.flak_cap = 1;
        let mut policy = UtilityPolicy::new();
        policy.seen_air = true;
        let mut intents = construction_intents(&mut policy, &dials, &obs);
        let anchor = assert_build_kind(&intents, BuildingKind::FlakTurret);

        policy.bind_player_facing_builders(&obs, &[], &[], &[], &[], &mut intents);
        let [
            Intent::BuildWith {
                builder,
                kind,
                anchor: bound_anchor,
            },
        ] = intents.as_slice()
        else {
            panic!("the player-facing build should bind one exact worker: {intents:?}");
        };
        assert_eq!(*kind, BuildingKind::FlakTurret);
        assert_eq!(*bound_anchor, anchor);

        let commands = Executive::new().apply_with_reservations(PlayerId(0), &obs, &intents, &[]);
        let [command] = commands.as_slice() else {
            panic!("the exact build should lower to one command: {commands:?}");
        };
        assert!(matches!(
            &command.command,
            Command::Build {
                units,
                kind: BuildingKind::FlakTurret,
                anchor: command_anchor,
                queue: false,
                defer: false,
            } if units == &vec![*builder] && command_anchor == &anchor
        ));

        let scrap_before = state.player(PlayerId(0)).scrap;
        let report = state.tick(&commands);
        assert!(
            report
                .events
                .iter()
                .all(|event| !matches!(event, Event::CommandRejected { .. })),
            "the policy-selected command must be legal in the authoritative sim: {report:?}"
        );
        let site = state
            .buildings()
            .iter()
            .find(|building| {
                building.player == PlayerId(0)
                    && building.kind == BuildingKind::FlakTurret
                    && building.anchor == anchor
            })
            .expect("the exact command places the intended site");
        assert!(!site.built);
        assert!(matches!(
            state.unit(*builder).expect("builder survives").order,
            Order::Build { site: building } if building == site.id
        ));
        assert_eq!(
            state.player(PlayerId(0)).scrap,
            scrap_before
                - BuildingKind::FlakTurret
                    .base_stats()
                    .construction
                    .unwrap()
                    .cost
        );
        obs = Observation::omniscient(&state, PlayerId(0));
        assert!(obs.my_units.iter().any(|unit| {
            unit.id == *builder && unit.site == Some(site.id) && unit.founding.is_none()
        }));
    }

    #[test]
    fn reclaimer_projection_counts_paid_sites_at_their_eventual_rate() {
        let mut obs = observation();
        obs.my_buildings.clear();

        let mut reclaimer = building(1, PlayerId(0), BuildingKind::Reclaimer, TilePos::new(4, 4));
        obs.my_buildings = vec![reclaimer.clone()];
        let base_income = UtilityPolicy::recurring_income_per_minute(&obs);
        assert!(base_income > 0);

        reclaimer.tier = 1;
        obs.my_buildings = vec![reclaimer.clone()];
        let refinery_income = UtilityPolicy::recurring_income_per_minute(&obs);
        assert!(
            refinery_income > base_income,
            "the paid Refinery upgrade must project its faster cadence"
        );

        reclaimer.built = false;
        reclaimer.tier = 0;
        obs.my_buildings = vec![reclaimer.clone()];
        assert_eq!(
            UtilityPolicy::recurring_income_per_minute(&obs),
            base_income,
            "a paid Reclaimer site prevents a duplicate income project"
        );

        reclaimer.tier = 1;
        obs.my_buildings = vec![reclaimer];
        assert_eq!(
            UtilityPolicy::recurring_income_per_minute(&obs),
            refinery_income,
            "an automatic Refinery upgrade retains its eventual income promise while offline"
        );
    }

    #[test]
    fn foundry_projection_observes_both_warmup_and_completion() {
        let mut obs = observation();
        obs.my_buildings.truncate(1);
        obs.tick = crate::stats::FOUNDRY_DRIP_START_TICK - 1;
        assert_eq!(UtilityPolicy::recurring_income_per_minute(&obs), 0);

        obs.tick = crate::stats::FOUNDRY_DRIP_START_TICK;
        let completed_income = UtilityPolicy::recurring_income_per_minute(&obs);
        assert!(completed_income > 0);

        obs.my_buildings[0].built = false;
        assert_eq!(
            UtilityPolicy::recurring_income_per_minute(&obs),
            0,
            "the global warmup ending cannot make an unfinished Foundry pay"
        );
    }

    #[test]
    fn extractor_projection_is_remote_until_a_completed_foundry_supports_it() {
        let extractor_anchor = TilePos::new(18, 10);
        let mut obs = observation();
        obs.my_buildings = vec![building(
            1,
            PlayerId(0),
            BuildingKind::Extractor,
            extractor_anchor,
        )];
        assert_eq!(
            UtilityPolicy::recurring_income_per_minute(&obs),
            crate::stats::EXTRACTOR_REMOTE_INCOME_PER_MINUTE
        );

        let mut supporting_foundry = building(
            2,
            PlayerId(0),
            BuildingKind::Foundry,
            extractor_anchor.offset(-9, 0),
        );
        supporting_foundry.built = false;
        obs.my_buildings.push(supporting_foundry.clone());
        assert_eq!(
            UtilityPolicy::recurring_income_per_minute(&obs),
            crate::stats::EXTRACTOR_REMOTE_INCOME_PER_MINUTE,
            "an unfinished expansion does not develop the Extractor"
        );

        obs.my_buildings.last_mut().unwrap().built = true;
        assert_eq!(
            UtilityPolicy::recurring_income_per_minute(&obs),
            crate::stats::EXTRACTOR_SUPPORTED_INCOME_PER_MINUTE
        );

        obs.my_buildings[0].built = false;
        assert_eq!(
            UtilityPolicy::recurring_income_per_minute(&obs),
            0,
            "support cannot make an unfinished Extractor productive"
        );
    }

    #[test]
    fn duplicate_deferred_reclaimer_crews_count_as_one_income_promise() {
        let anchor = TilePos::new(12, 8);
        let second_anchor = TilePos::new(18, 8);
        let mut obs = observation();
        obs.my_buildings.clear();
        obs.my_units = vec![
            harvester(
                1,
                TilePos::new(5, 5),
                Some((BuildingKind::Reclaimer, anchor)),
            ),
            harvester(
                2,
                TilePos::new(6, 5),
                Some((BuildingKind::Reclaimer, anchor)),
            ),
        ];
        let one_promise = UtilityPolicy::recurring_income_per_minute(&obs);
        assert!(one_promise > 0);

        obs.my_units.push(harvester(
            3,
            TilePos::new(7, 5),
            Some((BuildingKind::Reclaimer, second_anchor)),
        ));
        assert_eq!(
            UtilityPolicy::recurring_income_per_minute(&obs),
            one_promise * 2,
            "distinct sites are two promises, but duplicate crews at one site are not"
        );

        obs.my_buildings
            .push(building(4, PlayerId(0), BuildingKind::Reclaimer, anchor));
        assert_eq!(
            UtilityPolicy::recurring_income_per_minute(&obs),
            one_promise * 2,
            "a paid site replaces, rather than stacks with, its deferred promise"
        );
    }
}
