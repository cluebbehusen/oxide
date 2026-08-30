//! Fog-honest placement for persistent information coverage.

use super::*;
use crate::bot::intelligence::ContactEvidence;
use crate::stats::RADAR_DETECT_RADIUS;
use std::cmp::Reverse;

use super::defense::ResourceAccessGuard;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ArraySiteCandidate {
    anchor: TilePos,
    usable_radar: u32,
    novel_radar: u32,
    threat_distance: i32,
    unexplored_sight: u32,
    builder_distance: i32,
    home_distance: i32,
}

impl ArraySiteCandidate {
    fn key(self) -> impl Ord {
        (
            self.novel_radar,
            self.usable_radar,
            Reverse(self.threat_distance),
            self.unexplored_sight,
            Reverse(self.builder_distance),
            Reverse(self.home_distance),
            Reverse(self.anchor.y),
            Reverse(self.anchor.x),
        )
    }
}

impl UtilityPolicy {
    pub(super) fn strategic_array_site(
        &self,
        obs: &Observation,
        briefing: &PublicMapBriefing,
        home: TilePos,
        unit_contacts: &[UnitContact],
        building_contacts: &[BuildingContact],
        builders: &[&UnitObs],
    ) -> Option<(TilePos, UnitId)> {
        if builders.is_empty() {
            return None;
        }

        let kind = BuildingKind::Array;
        let foundry_size = BuildingKind::Foundry.base_stats().size;
        let home_center = home.offset(foundry_size.0 / 2, foundry_size.1 / 2);
        let minimum_radius = foundry_size.0.max(foundry_size.1);
        let maximum_radius = RADAR_DETECT_RADIUS;
        let existing_arrays: Vec<_> = obs
            .my_buildings
            .iter()
            .chain(obs.ally_buildings.iter())
            .filter(|building| building.hp > 0 && building.built && building.kind == kind)
            .map(|building| building.anchor)
            .collect();
        let threats = self.array_threat_origins(obs, briefing, unit_contacts, building_contacts);
        let resource_access = ResourceAccessGuard::new(self, obs, briefing);

        let mut candidates = Vec::new();
        for radius in minimum_radius..=maximum_radius {
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    if dx.abs().max(dy.abs()) != radius {
                        continue;
                    }
                    let anchor = home_center.offset(dx, dy);
                    if self.first_valid_placement(obs, kind, [anchor]) != Some(anchor) {
                        continue;
                    }
                    let (usable_radar, novel_radar, unexplored_sight) =
                        array_coverage(obs, briefing, anchor, &existing_arrays);
                    let threat_distance = threats
                        .iter()
                        .map(|threat| threat.manhattan(anchor))
                        .min()
                        .unwrap_or(i32::MAX);
                    let builder_distance = builders
                        .iter()
                        .map(|builder| builder.tile.manhattan(anchor))
                        .min()
                        .unwrap_or(i32::MAX);
                    candidates.push(ArraySiteCandidate {
                        anchor,
                        usable_radar,
                        novel_radar,
                        threat_distance,
                        unexplored_sight,
                        builder_distance,
                        home_distance: home_center.chebyshev(anchor),
                    });
                }
            }
        }

        candidates.sort_unstable_by_key(|candidate| candidate.key());
        let danger =
            self.harvest_danger_projection(obs, Some(unit_contacts), Some(building_contacts));
        candidates.into_iter().rev().find_map(|candidate| {
            if !resource_access.survives(kind, candidate.anchor) {
                return None;
            }
            let mut safe_builders = builders.to_vec();
            self.safe_implicit_builder(
                obs,
                kind,
                candidate.anchor,
                &mut safe_builders,
                &danger,
                Some(briefing),
            )
            .map(|builder| (candidate.anchor, builder))
        })
    }

    fn array_threat_origins(
        &self,
        obs: &Observation,
        briefing: &PublicMapBriefing,
        unit_contacts: &[UnitContact],
        building_contacts: &[BuildingContact],
    ) -> Vec<TilePos> {
        let mut current: Vec<_> = obs
            .enemy_units
            .iter()
            .filter(|unit| unit.hp > 0)
            .map(|unit| unit.tile)
            .chain(
                obs.enemy_buildings
                    .iter()
                    .filter(|building| building.hp > 0 && building.seen)
                    .map(|building| building.anchor),
            )
            .collect();
        current.sort_unstable_by_key(|tile| (tile.y, tile.x));
        current.dedup();
        if !current.is_empty() {
            return current;
        }

        let mut remembered: Vec<_> = unit_contacts
            .iter()
            .filter(|contact| {
                contact.evidence == ContactEvidence::Remembered
                    && contact.hp > 0
                    && contact.confidence_at(obs.tick) > 0
            })
            .map(|contact| contact.tile)
            .chain(
                building_contacts
                    .iter()
                    .filter(|contact| {
                        contact.evidence == ContactEvidence::Remembered
                            && contact.hp > 0
                            && contact.confidence_at(obs.tick) > 0
                    })
                    .map(|contact| contact.anchor),
            )
            .collect();
        remembered.sort_unstable_by_key(|tile| (tile.y, tile.x));
        remembered.dedup();
        if !remembered.is_empty() {
            return remembered;
        }

        self.uncleared_hostile_starts(briefing, obs.me)
            .into_iter()
            .map(|start| start.anchor)
            .collect()
    }
}

fn array_coverage(
    obs: &Observation,
    briefing: &PublicMapBriefing,
    anchor: TilePos,
    existing_arrays: &[TilePos],
) -> (u32, u32, u32) {
    let sight_radius = BuildingKind::Array.base_stats().vision;
    let mut usable_radar = 0_u32;
    let mut novel_radar = 0_u32;
    let mut unexplored_sight = 0_u32;
    for dy in -RADAR_DETECT_RADIUS..=RADAR_DETECT_RADIUS {
        for dx in -RADAR_DETECT_RADIUS..=RADAR_DETECT_RADIUS {
            let distance_squared = dx * dx + dy * dy;
            if distance_squared > RADAR_DETECT_RADIUS * RADAR_DETECT_RADIUS {
                continue;
            }
            let tile = anchor.offset(dx, dy);
            if briefing.terrain_at(tile).is_none() {
                continue;
            }
            usable_radar += 1;
            if existing_arrays.iter().all(|existing| {
                let delta_x = tile.x - existing.x;
                let delta_y = tile.y - existing.y;
                delta_x * delta_x + delta_y * delta_y > RADAR_DETECT_RADIUS * RADAR_DETECT_RADIUS
            }) {
                novel_radar += 1;
            }
            if distance_squared <= sight_radius * sight_radius && !obs.explored(tile) {
                unexplored_sight += 1;
            }
        }
    }
    (usable_radar, novel_radar, unexplored_sight)
}
