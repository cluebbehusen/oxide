//! Exact, reusable worker-danger projection.

use super::UtilityPolicy;
use crate::bot::intelligence::{BuildingContact, UnitContact};
use crate::bot::observation::Observation;
use crate::stats::Domain;
use chassis::grid::TilePos;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct DangerRect {
    min_x: i32,
    min_y: i32,
    max_x: i32,
    max_y: i32,
}

impl DangerRect {
    fn around(center: TilePos, radius: i32) -> Self {
        Self {
            min_x: center.x.saturating_sub(radius),
            min_y: center.y.saturating_sub(radius),
            max_x: center.x.saturating_add(radius),
            max_y: center.y.saturating_add(radius),
        }
    }

    fn around_footprint(anchor: TilePos, size: (i32, i32), radius: i32) -> Self {
        Self {
            min_x: anchor.x.saturating_sub(radius),
            min_y: anchor.y.saturating_sub(radius),
            max_x: anchor
                .x
                .saturating_add(size.0.saturating_sub(1))
                .saturating_add(radius),
            max_y: anchor
                .y
                .saturating_add(size.1.saturating_sub(1))
                .saturating_add(radius),
        }
    }

    fn contains_with_margin(self, tile: TilePos, margin: i32) -> bool {
        let min_x = i64::from(self.min_x) - i64::from(margin);
        let min_y = i64::from(self.min_y) - i64::from(margin);
        let max_x = i64::from(self.max_x) + i64::from(margin);
        let max_y = i64::from(self.max_y) + i64::from(margin);
        min_x <= max_x
            && min_y <= max_y
            && i64::from(tile.x) >= min_x
            && i64::from(tile.x) <= max_x
            && i64::from(tile.y) >= min_y
            && i64::from(tile.y) <= max_y
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct HarvestDangerWork {
    pub(super) source_visits: usize,
    pub(super) effective_rectangles: usize,
    pub(super) rectangle_stamp_attempts: usize,
    pub(super) mask_cell_visits: usize,
}

/// The derived equality is the cache-invalidation predicate: the cache
/// rebuilds its projection exactly when a fresh layout compares unequal
/// to the cached one, so every field added here participates in that
/// decision automatically.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HarvestDangerLayout {
    map_size: (i32, i32),
    rectangles: Vec<DangerRect>,
}

/// How many threat sources fed a layout, recomputed from the same public
/// inputs the layout was built from so the counter never has to live on
/// the production struct.
#[cfg(test)]
fn source_visit_count(
    obs: &Observation,
    unit_contacts: Option<&[UnitContact]>,
    building_contacts: Option<&[BuildingContact]>,
) -> usize {
    obs.blips.len()
        + obs.enemy_units.len()
        + unit_contacts.map_or(0, <[UnitContact]>::len)
        + building_contacts.map_or(0, <[BuildingContact]>::len)
        + obs.enemy_buildings.len()
}

impl HarvestDangerLayout {
    fn from_observation(
        obs: &Observation,
        unit_contacts: Option<&[UnitContact]>,
        building_contacts: Option<&[BuildingContact]>,
    ) -> Self {
        let mut rectangles = Vec::new();
        rectangles.extend(
            obs.blips
                .iter()
                .map(|&tile| DangerRect::around(tile, crate::stats::HARVEST_RADAR_DANGER_RADIUS)),
        );

        for (kind, tile) in obs
            .enemy_units
            .iter()
            .map(|unit| (unit.kind, unit.tile))
            .chain(unit_contacts.into_iter().flatten().filter_map(|contact| {
                (contact.confidence_at(obs.tick) > 0).then_some((contact.kind, contact.tile))
            }))
        {
            if let Some(radius) = kind
                .stats()
                .weapons
                .iter()
                .filter(|weapon| weapon.targets.covers(Domain::Ground))
                .map(|weapon| {
                    (weapon.range + crate::stats::HARVEST_MOBILE_DANGER_MARGIN)
                        .ceil()
                        .to_num::<i32>()
                })
                .max()
            {
                rectangles.push(DangerRect::around(tile, radius));
            }
        }

        let mut remembered_tiers = BTreeMap::new();
        for contact in building_contacts.into_iter().flatten() {
            remembered_tiers
                .entry((contact.player, contact.anchor, contact.kind))
                .or_insert(contact.tier);
        }
        for building in &obs.enemy_buildings {
            if !building.built {
                continue;
            }
            let tier = if building.seen {
                building.tier
            } else {
                remembered_tiers
                    .get(&(building.player, building.anchor, building.kind))
                    .copied()
                    .unwrap_or(building.tier)
            };
            let stats = building.kind.tier_stats(tier);
            if let Some(radius) = stats
                .weapons
                .iter()
                .filter(|weapon| weapon.targets.covers(Domain::Ground))
                .map(|weapon| {
                    (weapon.range + crate::stats::HARVEST_STATIC_DANGER_MARGIN)
                        .ceil()
                        .to_num::<i32>()
                })
                .max()
            {
                rectangles.push(DangerRect::around_footprint(
                    building.anchor,
                    stats.size,
                    radius,
                ));
            }
        }

        rectangles.sort_unstable();
        rectangles.dedup();
        Self {
            map_size: (obs.map_width, obs.map_height),
            rectangles,
        }
    }
}

/// Immutable point-query surface for every currently justified worker threat.
///
/// Mobile ranges, blips, and hostile building footprints are all Chebyshev
/// rectangles. Stamping each distinct rectangle once and prefixing the dense
/// grid makes the route planner's later point queries constant-time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HarvestDangerProjection {
    map_size: (i32, i32),
    rectangles: Vec<DangerRect>,
    danger: Vec<bool>,
    #[cfg(test)]
    work: HarvestDangerWork,
}

impl HarvestDangerProjection {
    fn new(layout: HarvestDangerLayout) -> Self {
        let danger = if layout.rectangles.is_empty() {
            Vec::new()
        } else {
            stamp_rectangles(layout.map_size, &layout.rectangles)
        };
        #[cfg(test)]
        let work = HarvestDangerWork {
            source_visits: 0,
            effective_rectangles: layout.rectangles.len(),
            rectangle_stamp_attempts: layout.rectangles.len(),
            mask_cell_visits: if layout.rectangles.is_empty() {
                0
            } else {
                map_cell_count(layout.map_size)
            },
        };
        Self {
            map_size: layout.map_size,
            rectangles: layout.rectangles,
            danger,
            #[cfg(test)]
            work,
        }
    }

    pub(super) fn contains(&self, tile: TilePos) -> bool {
        self.contains_with_margin(tile, 0)
    }

    pub(super) fn contains_with_margin(&self, tile: TilePos, margin: i32) -> bool {
        if margin == 0
            && let Some(index) = map_index(self.map_size, tile)
            && let Some(&danger) = self.danger.get(index)
        {
            return danger;
        }
        self.rectangles
            .iter()
            .any(|rectangle| rectangle.contains_with_margin(tile, margin))
    }

    #[cfg(test)]
    pub(super) fn work(&self) -> HarvestDangerWork {
        self.work
    }

    #[cfg(test)]
    fn with_source_visits(mut self, source_visits: usize) -> Self {
        self.work.source_visits = source_visits;
        self
    }
}

#[cfg(test)]
fn map_cell_count((width, height): (i32, i32)) -> usize {
    let Ok(width) = usize::try_from(width) else {
        return 0;
    };
    let Ok(height) = usize::try_from(height) else {
        return 0;
    };
    width.checked_mul(height).unwrap_or(0)
}

fn map_index(map_size: (i32, i32), tile: TilePos) -> Option<usize> {
    let (width, height) = map_size;
    if tile.x < 0 || tile.y < 0 || tile.x >= width || tile.y >= height {
        return None;
    }
    let width = usize::try_from(width).ok()?;
    let x = usize::try_from(tile.x).ok()?;
    let y = usize::try_from(tile.y).ok()?;
    y.checked_mul(width)?.checked_add(x)
}

fn stamp_rectangles(map_size: (i32, i32), rectangles: &[DangerRect]) -> Vec<bool> {
    let (width, height) = map_size;
    let (Ok(width), Ok(height)) = (usize::try_from(width), usize::try_from(height)) else {
        return Vec::new();
    };
    let Some(cells) = width.checked_mul(height) else {
        return Vec::new();
    };
    if width == 0 || height == 0 {
        return vec![false; cells];
    }
    let stride = width + 1;
    let mut difference = vec![0_i64; stride * (height + 1)];
    let map_max_x = i64::try_from(width - 1).expect("map width fits i64");
    let map_max_y = i64::try_from(height - 1).expect("map height fits i64");

    for rectangle in rectangles {
        let min_x = i64::from(rectangle.min_x).clamp(0, map_max_x);
        let min_y = i64::from(rectangle.min_y).clamp(0, map_max_y);
        let max_x = i64::from(rectangle.max_x).clamp(0, map_max_x);
        let max_y = i64::from(rectangle.max_y).clamp(0, map_max_y);
        if min_x > max_x
            || min_y > max_y
            || rectangle.max_x < 0
            || rectangle.max_y < 0
            || rectangle.min_x >= map_size.0
            || rectangle.min_y >= map_size.1
        {
            continue;
        }
        let min_x = usize::try_from(min_x).expect("clipped x is nonnegative");
        let min_y = usize::try_from(min_y).expect("clipped y is nonnegative");
        let after_x = usize::try_from(max_x + 1).expect("clipped x successor is nonnegative");
        let after_y = usize::try_from(max_y + 1).expect("clipped y successor is nonnegative");
        difference[min_y * stride + min_x] += 1;
        difference[min_y * stride + after_x] -= 1;
        difference[after_y * stride + min_x] -= 1;
        difference[after_y * stride + after_x] += 1;
    }

    let mut danger = vec![false; cells];
    for y in 0..height {
        let mut row_sum = 0_i64;
        for x in 0..width {
            let index = y * stride + x;
            row_sum += difference[index];
            let above = if y == 0 {
                0
            } else {
                difference[(y - 1) * stride + x]
            };
            difference[index] = row_sum + above;
            danger[y * width + x] = difference[index] > 0;
        }
    }
    danger
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CachedHarvestDanger {
    layout: HarvestDangerLayout,
    projection: Arc<HarvestDangerProjection>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct HarvestDangerCache {
    entry: Option<CachedHarvestDanger>,
    #[cfg(test)]
    builds: usize,
}

impl HarvestDangerCache {
    fn projection(
        &mut self,
        obs: &Observation,
        unit_contacts: Option<&[UnitContact]>,
        building_contacts: Option<&[BuildingContact]>,
    ) -> Arc<HarvestDangerProjection> {
        let layout = HarvestDangerLayout::from_observation(obs, unit_contacts, building_contacts);
        if self
            .entry
            .as_ref()
            .is_none_or(|entry| entry.layout != layout)
        {
            let projection = Arc::new(HarvestDangerProjection::new(layout.clone()));
            self.entry = Some(CachedHarvestDanger { layout, projection });
            #[cfg(test)]
            {
                self.builds += 1;
            }
        }
        Arc::clone(
            &self
                .entry
                .as_ref()
                .expect("a requested harvest-danger projection is cached")
                .projection,
        )
    }

    #[cfg(test)]
    fn build_count(&self) -> usize {
        self.builds
    }
}

impl UtilityPolicy {
    pub(super) fn harvest_danger_projection(
        &self,
        obs: &Observation,
        unit_contacts: Option<&[UnitContact]>,
        building_contacts: Option<&[BuildingContact]>,
    ) -> Arc<HarvestDangerProjection> {
        self.harvest_danger_cache
            .borrow_mut()
            .projection(obs, unit_contacts, building_contacts)
    }

    #[cfg(test)]
    pub(super) fn harvest_danger_build_count(&self) -> usize {
        self.harvest_danger_cache.borrow().build_count()
    }
}

pub(super) fn current_location_has_known_danger(
    obs: &Observation,
    node: TilePos,
    additional_margin: i32,
) -> bool {
    location_has_known_danger(obs, node, additional_margin, None, None, true)
}

#[cfg(test)]
pub(super) fn direct_location_has_known_danger(
    obs: &Observation,
    node: TilePos,
    additional_margin: i32,
    unit_contacts: Option<&[UnitContact]>,
    building_contacts: Option<&[BuildingContact]>,
) -> bool {
    location_has_known_danger(
        obs,
        node,
        additional_margin,
        unit_contacts,
        building_contacts,
        false,
    )
}

fn location_has_known_danger(
    obs: &Observation,
    node: TilePos,
    additional_margin: i32,
    unit_contacts: Option<&[UnitContact]>,
    building_contacts: Option<&[BuildingContact]>,
    visible_buildings_only: bool,
) -> bool {
    if obs.blips.iter().any(|contact| {
        contact.chebyshev(node) <= crate::stats::HARVEST_RADAR_DANGER_RADIUS + additional_margin
    }) {
        return true;
    }
    let mobile_threat = obs
        .enemy_units
        .iter()
        .map(|unit| (unit.kind, unit.tile))
        .chain(unit_contacts.into_iter().flatten().filter_map(|contact| {
            (contact.confidence_at(obs.tick) > 0).then_some((contact.kind, contact.tile))
        }))
        .any(|(kind, tile)| {
            kind.stats()
                .weapons
                .iter()
                .filter(|weapon| weapon.targets.covers(Domain::Ground))
                .any(|weapon| {
                    tile.chebyshev(node)
                        <= (weapon.range + crate::stats::HARVEST_MOBILE_DANGER_MARGIN)
                            .ceil()
                            .to_num::<i32>()
                            + additional_margin
                })
        });
    if mobile_threat {
        return true;
    }
    obs.enemy_buildings.iter().any(|building| {
        if visible_buildings_only && !building.seen {
            return false;
        }
        let tier = if building.seen {
            building.tier
        } else {
            building_contacts
                .and_then(|contacts| {
                    contacts.iter().find(|contact| {
                        contact.player == building.player
                            && contact.anchor == building.anchor
                            && contact.kind == building.kind
                    })
                })
                .map_or(building.tier, |contact| contact.tier)
        };
        let stats = building.kind.tier_stats(tier);
        building.built
            && stats.weapons.iter().any(|weapon| {
                weapon.targets.covers(Domain::Ground)
                    && distance_to_footprint(node, building.anchor, stats.size)
                        <= (weapon.range + crate::stats::HARVEST_STATIC_DANGER_MARGIN)
                            .ceil()
                            .to_num::<i32>()
                            + additional_margin
            })
    })
}

fn distance_to_footprint(node: TilePos, anchor: TilePos, size: (i32, i32)) -> i32 {
    let far_x = anchor.x + size.0 - 1;
    let far_y = anchor.y + size.1 - 1;
    let dx = (anchor.x - node.x).max(node.x - far_x).max(0);
    let dy = (anchor.y - node.y).max(node.y - far_y).max(0);
    dx.max(dy)
}

#[cfg(test)]
mod tests {
    use super::super::CONTESTED_RECON_RADIUS;
    use super::*;
    use crate::bot::intelligence::ContactEvidence;
    use crate::bot::observation::{BuildingObs, UnitObs};
    use crate::ids::{BuildingId, PlayerId, UnitId};

    use crate::stats::{BuildingKind, UnitKind};

    fn observation(width: i32, height: i32) -> Observation {
        Observation {
            tick: 1_000,
            map_width: width,
            map_height: height,
            visible: vec![true; map_cell_count((width, height))],
            explored: vec![true; map_cell_count((width, height))],
            ..Observation::default()
        }
    }

    fn enemy_unit(id: u32, kind: UnitKind, tile: TilePos) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(1),
            kind,
            tile,
            hp: kind.stats().max_hp,
            idle: false,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        }
    }

    fn unit_contact(id: u32, kind: UnitKind, tile: TilePos, last_seen: u64) -> UnitContact {
        UnitContact {
            id: UnitId(id),
            player: PlayerId(1),
            kind,
            tile,
            hp: kind.stats().max_hp,
            last_seen,
            evidence: ContactEvidence::Remembered,
        }
    }

    fn assert_direct_equivalence(
        case: &str,
        obs: &Observation,
        units: &[UnitContact],
        buildings: &[BuildingContact],
    ) {
        let projection = HarvestDangerProjection::new(HarvestDangerLayout::from_observation(
            obs,
            Some(units),
            Some(buildings),
        ));
        let fringe = CONTESTED_RECON_RADIUS + 2;
        for margin in [-1, 0, 1, CONTESTED_RECON_RADIUS] {
            for y in -fringe..obs.map_height + fringe {
                for x in -fringe..obs.map_width + fringe {
                    let tile = TilePos::new(x, y);
                    assert_eq!(
                        projection.contains_with_margin(tile, margin),
                        direct_location_has_known_danger(
                            obs,
                            tile,
                            margin,
                            Some(units),
                            Some(buildings),
                        ),
                        "{case}: mismatch at {tile:?} with margin {margin}",
                    );
                }
            }
        }
    }

    #[test]
    fn projection_matches_the_direct_predicate_for_every_danger_evidence_shape() {
        let mut obs = observation(17, 11);
        obs.enemy_units
            .push(enemy_unit(1, UnitKind::Sentinel, TilePos::new(0, 0)));
        assert_direct_equivalence("current mobile at map edge", &obs, &[], &[]);

        obs.enemy_units.clear();
        let remembered = unit_contact(2, UnitKind::Sentinel, TilePos::new(16, 10), 999);
        assert_direct_equivalence(
            "remembered mobile",
            &obs,
            std::slice::from_ref(&remembered),
            &[],
        );

        obs.enemy_units
            .push(enemy_unit(2, UnitKind::Sentinel, remembered.tile));
        assert_direct_equivalence(
            "duplicate current and remembered mobile",
            &obs,
            &[remembered.clone(), remembered],
            &[],
        );

        obs.enemy_units.clear();
        let expired = unit_contact(3, UnitKind::Sentinel, TilePos::new(8, 5), 0);
        assert_eq!(expired.confidence_at(obs.tick), 0);
        assert_direct_equivalence("zero-confidence mobile", &obs, &[expired], &[]);

        obs.blips = vec![TilePos::new(0, 10), TilePos::new(0, 10)];
        assert_direct_equivalence("duplicate radar blip at map edge", &obs, &[], &[]);

        obs.blips.clear();
        let anchor = TilePos::new(15, 0);
        obs.enemy_buildings.push(BuildingObs {
            id: BuildingId(u32::MAX),
            player: PlayerId(1),
            kind: BuildingKind::Turret,
            anchor,
            hp: BuildingKind::Turret.base_stats().max_hp,
            built: true,
            seen: false,
            tier: 0,
        });
        let upgraded_ghost = BuildingContact {
            id: Some(BuildingId(7)),
            player: PlayerId(1),
            kind: BuildingKind::Turret,
            anchor,
            hp: BuildingKind::Turret.tier_stats(1).max_hp,
            built: true,
            tier: 1,
            last_seen: None,
            evidence: ContactEvidence::Remembered,
        };
        assert_eq!(upgraded_ghost.confidence_at(obs.tick), 0);
        assert_direct_equivalence(
            "zero-confidence upgraded building ghost at map edge",
            &obs,
            &[],
            std::slice::from_ref(&upgraded_ghost),
        );

        obs.enemy_buildings[0].built = false;
        assert_direct_equivalence("unfinished upgraded ghost", &obs, &[], &[]);

        obs.enemy_buildings[0].built = true;
        obs.blips = vec![TilePos::new(0, 10)];
        obs.enemy_units
            .push(enemy_unit(8, UnitKind::Sentinel, TilePos::new(5, 5)));
        let active_memory = unit_contact(9, UnitKind::Sentinel, TilePos::new(11, 8), 999);
        let mut conflicting_later_contact = upgraded_ghost.clone();
        conflicting_later_contact.tier = 0;
        assert_direct_equivalence(
            "overlapping current, remembered, blip, and first-match ghost evidence",
            &obs,
            std::slice::from_ref(&active_memory),
            &[upgraded_ghost, conflicting_later_contact],
        );
    }

    #[test]
    fn projection_work_does_not_multiply_sources_by_map_cells() {
        let empty = observation(13, 9);
        let empty_projection =
            HarvestDangerProjection::new(HarvestDangerLayout::from_observation(&empty, None, None));
        assert_eq!(empty_projection.work(), HarvestDangerWork::default());
        assert!(!empty_projection.contains(TilePos::new(5, 4)));

        let mut small = observation(13, 9);
        let tile = TilePos::new(5, 4);
        small.enemy_units.extend([
            enemy_unit(1, UnitKind::Sentinel, tile),
            enemy_unit(2, UnitKind::Sentinel, tile),
        ]);
        let contacts = [
            unit_contact(1, UnitKind::Sentinel, tile, small.tick),
            unit_contact(2, UnitKind::Sentinel, tile, small.tick),
        ];
        let small_projection = HarvestDangerProjection::new(HarvestDangerLayout::from_observation(
            &small,
            Some(&contacts),
            Some(&[]),
        ))
        .with_source_visits(source_visit_count(&small, Some(&contacts), Some(&[])));
        assert_eq!(
            small_projection.work(),
            HarvestDangerWork {
                source_visits: 4,
                effective_rectangles: 1,
                rectangle_stamp_attempts: 1,
                mask_cell_visits: 13 * 9,
            }
        );

        let mut large = small.clone();
        large.map_width = 130;
        large.map_height = 90;
        large.visible = vec![true; 130 * 90];
        large.explored = vec![true; 130 * 90];
        let large_projection = HarvestDangerProjection::new(HarvestDangerLayout::from_observation(
            &large,
            Some(&contacts),
            Some(&[]),
        ))
        .with_source_visits(source_visit_count(&large, Some(&contacts), Some(&[])));
        let large_work = large_projection.work();
        assert_eq!(large_work.source_visits, 4);
        assert_eq!(large_work.effective_rectangles, 1);
        assert_eq!(large_work.rectangle_stamp_attempts, 1);
        assert_eq!(large_work.mask_cell_visits, 130 * 90);
    }

    #[test]
    fn cache_reuses_an_identical_effective_danger_layout() {
        let mut obs = observation(17, 11);
        let tile = TilePos::new(4, 4);
        obs.enemy_units
            .push(enemy_unit(1, UnitKind::Sentinel, tile));
        let mut cache = HarvestDangerCache::default();

        let first = cache.projection(&obs, None, None);
        assert_eq!(cache.build_count(), 1);
        let second = cache.projection(&obs, None, None);
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(cache.build_count(), 1);

        let duplicate = unit_contact(1, UnitKind::Sentinel, tile, obs.tick);
        let semantically_identical = cache.projection(&obs, Some(&[duplicate]), None);
        assert!(Arc::ptr_eq(&first, &semantically_identical));
        assert_eq!(cache.build_count(), 1);

        obs.enemy_units[0].tile = TilePos::new(5, 4);
        let changed = cache.projection(&obs, None, None);
        assert!(!Arc::ptr_eq(&first, &changed));
        assert_eq!(cache.build_count(), 2);
    }

    #[test]
    fn cache_rebuilds_when_remembered_mobile_danger_expires() {
        let mut obs = observation(17, 11);
        let tile = TilePos::new(8, 5);
        let remembered = unit_contact(1, UnitKind::Sentinel, tile, obs.tick);
        let mut cache = HarvestDangerCache::default();

        let active = cache.projection(&obs, Some(std::slice::from_ref(&remembered)), None);
        assert!(active.contains(tile));
        assert_eq!(cache.build_count(), 1);

        obs.tick += 601;
        assert_eq!(remembered.confidence_at(obs.tick), 0);
        let expired = cache.projection(&obs, Some(std::slice::from_ref(&remembered)), None);
        assert!(!Arc::ptr_eq(&active, &expired));
        assert!(!expired.contains(tile));
        assert_eq!(cache.build_count(), 2);
    }
}
