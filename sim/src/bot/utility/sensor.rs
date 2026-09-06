//! Fog-honest placement for persistent information coverage.

use super::*;
use crate::bot::intelligence::ContactEvidence;
use crate::stats::RADAR_DETECT_RADIUS;
use std::cmp::Reverse;

#[cfg(test)]
use super::defense::ResourceAccessGuard;
use super::defense::{DefenseOpportunityEvidence, DefenseThinkContext};
use crate::map::Terrain;

/// Exact Array-site quote retained by the voluntary defense allocator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct StrategicArrayQuote {
    pub(super) anchor: TilePos,
    pub(super) builder: UnitId,
    pub(super) usable_radar: u32,
    pub(super) novel_radar: u32,
    pub(super) strategic_radar: u32,
    pub(super) builder_travel_cost: u32,
    pub(super) evidence: DefenseOpportunityEvidence,
    pub(super) evidence_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ArraySiteCandidate {
    anchor: TilePos,
    usable_radar: u32,
    novel_radar: u32,
    strategic_radar: u32,
    threat_distance: i32,
    unexplored_sight: u32,
    builder_distance: i32,
    home_distance: i32,
}

impl ArraySiteCandidate {
    fn key(self) -> impl Ord {
        (
            self.strategic_radar,
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
    #[cfg(test)]
    pub(super) fn strategic_array_site(
        &self,
        obs: &Observation,
        briefing: &PublicMapBriefing,
        home: TilePos,
        unit_contacts: &[UnitContact],
        building_contacts: &[BuildingContact],
        builders: &[&UnitObs],
    ) -> Option<(TilePos, UnitId)> {
        self.strategic_array_quote(
            obs,
            briefing,
            home,
            unit_contacts,
            building_contacts,
            builders,
        )
        .map(|quote| (quote.anchor, quote.builder))
    }

    #[cfg(test)]
    pub(super) fn strategic_array_quote(
        &self,
        obs: &Observation,
        briefing: &PublicMapBriefing,
        home: TilePos,
        unit_contacts: &[UnitContact],
        building_contacts: &[BuildingContact],
        builders: &[&UnitObs],
    ) -> Option<StrategicArrayQuote> {
        if builders.is_empty() {
            return None;
        }

        let kind = BuildingKind::Array;
        let resource_access = ResourceAccessGuard::new(self, obs, briefing);
        let danger =
            self.harvest_danger_projection(obs, Some(unit_contacts), Some(building_contacts));
        self.strategic_array_quote_with_candidate(
            obs,
            briefing,
            home,
            unit_contacts,
            building_contacts,
            builders,
            |anchor| {
                if !resource_access.survives(kind, anchor) {
                    return None;
                }
                let mut safe_builders = builders.to_vec();
                let builder = self.safe_implicit_builder(
                    obs,
                    kind,
                    anchor,
                    &mut safe_builders,
                    &danger,
                    Some(briefing),
                )?;
                let builder = builders.iter().find(|unit| unit.id == builder)?;
                let travel = resource_access.builder_travel_cost(builder, kind, anchor)?;
                Some((builder.id, travel))
            },
        )
    }

    pub(super) fn strategic_array_quote_in_context(
        &self,
        home: TilePos,
        builders: &[&UnitObs],
        context: &mut DefenseThinkContext<'_>,
    ) -> Option<StrategicArrayQuote> {
        if builders.is_empty() {
            return None;
        }

        let kind = BuildingKind::Array;
        self.strategic_array_quote_with_candidate(
            context.observation(),
            context.briefing(),
            home,
            context.unit_contacts(),
            context.building_contacts(),
            builders,
            |anchor| {
                if !context.future_ground_producer_egress_survives(kind, anchor)
                    || !context.resource_access_survives(kind, anchor)
                {
                    return None;
                }
                let builder = context.safe_implicit_builder(self, kind, anchor, builders)?;
                let builder = builders.iter().find(|unit| unit.id == builder)?;
                let travel = context.builder_travel_cost(builder, kind, anchor)?;
                Some((builder.id, travel))
            },
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one frozen Array-scoring boundary"
    )]
    fn strategic_array_quote_with_candidate(
        &self,
        obs: &Observation,
        briefing: &PublicMapBriefing,
        home: TilePos,
        unit_contacts: &[UnitContact],
        building_contacts: &[BuildingContact],
        builders: &[&UnitObs],
        mut quote_candidate: impl FnMut(TilePos) -> Option<(UnitId, u32)>,
    ) -> Option<StrategicArrayQuote> {
        let kind = BuildingKind::Array;
        let foundry_size = BuildingKind::Foundry.base_stats().size;
        let home_center = home.offset(foundry_size.0 / 2, foundry_size.1 / 2);
        let minimum_radius = foundry_size.0.max(foundry_size.1);
        let maximum_radius = RADAR_DETECT_RADIUS;
        let existing_arrays: Vec<_> = obs
            .my_buildings
            .iter()
            .chain(obs.ally_buildings.iter())
            .filter(|building| building.hp > 0 && building.kind == kind)
            .map(|building| building.anchor)
            .chain(obs.my_units.iter().filter_map(|unit| {
                unit.founding
                    .filter(|(kind, _)| *kind == BuildingKind::Array)
                    .map(|(_, anchor)| anchor)
            }))
            .collect();
        let threats = self.array_threat_origins(obs, briefing, unit_contacts, building_contacts);
        // The ring scan validates up to ~1.7k anchors. Prepare the egress
        // cache once and answer each exact circular coverage query from row
        // prefixes, so the equally large radar disc is not rescanned for
        // every survivor.
        self.prepare_ground_producer_egress(obs);
        let earliest_ready = builders
            .iter()
            .map(|worker| array_ready_at(obs.tick, worker, 0))
            .min()
            .expect("nonempty builders");
        let coverage = ArrayCoverageIndex::new(obs, briefing, &existing_arrays)
            .with_demand(&self.battlefield.coverage, earliest_ready);
        let first_deadline = self
            .battlefield
            .coverage
            .iter()
            .filter(|demand| demand.deadline > earliest_ready)
            .map(|demand| demand.deadline)
            .min()
            .unwrap_or(u64::MAX);

        let mut candidates = Vec::new();
        let mut centers = std::collections::BTreeSet::from([(home_center.y, home_center.x)]);
        for question in &self.battlefield.coverage {
            if let Some(asset) = obs
                .my_buildings
                .iter()
                .find(|asset| asset.id == question.asset)
            {
                let size = asset.kind.base_stats().size;
                centers.insert((asset.anchor.y + size.1 / 2, asset.anchor.x + size.0 / 2));
            }
        }
        let mut considered = std::collections::BTreeSet::new();
        for (center_y, center_x) in centers {
            let center = TilePos::new(center_x, center_y);
            for radius in minimum_radius..=maximum_radius {
                for dy in -radius..=radius {
                    for dx in -radius..=radius {
                        if dx.abs().max(dy.abs()) != radius {
                            continue;
                        }
                        let anchor = center.offset(dx, dy);
                        if !considered.insert((anchor.y, anchor.x))
                            || !self.placement_valid_prepared(obs, kind, anchor)
                        {
                            continue;
                        }
                        let (usable_radar, novel_radar, unexplored_sight) =
                            coverage.coverage(anchor);
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
                        let optimistic_ready = builders
                            .iter()
                            .map(|worker| {
                                let size = kind.base_stats().size;
                                let distance =
                                    (worker.tile.chebyshev(anchor) - size.0.max(size.1) - 1).max(0)
                                        as u32;
                                array_ready_at(obs.tick, worker, distance.saturating_mul(10))
                            })
                            .min()
                            .expect("nonempty builders");
                        let strategic_radar = if optimistic_ready >= first_deadline {
                            coverage.ready_demand(
                                anchor,
                                &self.battlefield.coverage,
                                optimistic_ready,
                            )
                        } else {
                            coverage.circle_sum(
                                &coverage.strategic,
                                anchor,
                                &coverage.radar_half_widths,
                            )
                        };
                        candidates.push(ArraySiteCandidate {
                            anchor,
                            usable_radar,
                            novel_radar,
                            strategic_radar,
                            threat_distance,
                            unexplored_sight,
                            builder_distance,
                            home_distance: home_center.chebyshev(anchor),
                        });
                    }
                }
            }
        }
        candidates.sort_unstable_by_key(|candidate| candidate.key());
        let mut best: Option<(ArraySiteCandidate, StrategicArrayQuote)> = None;
        for mut candidate in candidates.into_iter().rev() {
            if best
                .as_ref()
                .is_some_and(|(chosen, _)| candidate.key() <= chosen.key())
            {
                break;
            }
            if candidate.novel_radar == 0 {
                continue;
            }
            if let Some((builder, builder_travel_cost)) = quote_candidate(candidate.anchor) {
                let ready_at = builders
                    .iter()
                    .find(|unit| unit.id == builder)
                    .map_or(u64::MAX, |worker| {
                        array_ready_at(obs.tick, worker, builder_travel_cost)
                    });
                let strategic_radar =
                    coverage.ready_demand(candidate.anchor, &self.battlefield.coverage, ready_at);
                candidate.strategic_radar = strategic_radar;
                if best
                    .as_ref()
                    .is_some_and(|(chosen, _)| candidate.key() <= chosen.key())
                {
                    continue;
                }
                let (evidence, evidence_count) = array_opportunity_evidence(
                    obs,
                    unit_contacts,
                    building_contacts,
                    candidate.anchor,
                    &existing_arrays,
                );
                let quote = StrategicArrayQuote {
                    anchor: candidate.anchor,
                    builder,
                    usable_radar: candidate.usable_radar,
                    novel_radar: candidate.novel_radar,
                    strategic_radar,
                    builder_travel_cost,
                    evidence,
                    evidence_count,
                };
                best = Some((candidate, quote));
            }
        }
        best.map(|(_, quote)| quote)
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

fn array_ready_at(now: chassis::Tick, worker: &UnitObs, travel: u32) -> chassis::Tick {
    now.saturating_add(super::defense::travel_ticks(
        travel,
        worker.kind.stats().speed,
    ))
    .saturating_add(u64::from(
        BuildingKind::Array
            .base_stats()
            .construction
            .expect("Array construction")
            .build_ticks
            .div_ceil(worker.kind.stats().build_rate.max(1)),
    ))
}

fn array_opportunity_evidence(
    obs: &Observation,
    unit_contacts: &[UnitContact],
    building_contacts: &[BuildingContact],
    anchor: TilePos,
    existing_arrays: &[TilePos],
) -> (DefenseOpportunityEvidence, usize) {
    let in_novel_coverage = |tile: TilePos| {
        within_radar(anchor, tile)
            && existing_arrays
                .iter()
                .all(|existing| !within_radar(*existing, tile))
    };
    let current = obs
        .enemy_units
        .iter()
        .filter(|unit| unit.hp > 0 && in_novel_coverage(unit.tile))
        .count()
        + obs
            .enemy_buildings
            .iter()
            .filter(|building| {
                building.hp > 0 && building.seen && in_novel_coverage(building.anchor)
            })
            .count();
    if current > 0 {
        return (DefenseOpportunityEvidence::CurrentFoothold, current);
    }
    let remembered = unit_contacts
        .iter()
        .filter(|contact| {
            contact.evidence == ContactEvidence::Remembered
                && contact.hp > 0
                && contact.confidence_at(obs.tick) > 0
                && in_novel_coverage(contact.tile)
        })
        .count()
        + building_contacts
            .iter()
            .filter(|contact| {
                contact.evidence == ContactEvidence::Remembered
                    && contact.hp > 0
                    && contact.confidence_at(obs.tick) > 0
                    && in_novel_coverage(contact.anchor)
            })
            .count();
    if remembered > 0 {
        return (DefenseOpportunityEvidence::Remembered, remembered);
    }
    (DefenseOpportunityEvidence::PublicPrior, 1)
}

fn within_radar(anchor: TilePos, tile: TilePos) -> bool {
    let dx = anchor.x - tile.x;
    let dy = anchor.y - tile.y;
    dx * dx + dy * dy <= RADAR_DETECT_RADIUS * RADAR_DETECT_RADIUS
}

/// Exact map-row prefix sums for the three Array coverage populations.
///
/// A circular query still visits every integer row in the radius and uses the
/// same `dx * dx + dy * dy <= radius * radius` boundary as the direct scan.
/// The only shortcut is summing each row's inclusive integer span in O(1).
struct ArrayCoverageIndex {
    width: i32,
    height: i32,
    row_stride: usize,
    usable: Vec<u32>,
    novel: Vec<u32>,
    unexplored: Vec<u32>,
    strategic: Vec<u32>,
    radar_half_widths: Vec<i32>,
    sight_half_widths: Vec<i32>,
}

impl ArrayCoverageIndex {
    fn new(obs: &Observation, briefing: &PublicMapBriefing, existing_arrays: &[TilePos]) -> Self {
        let width = briefing.map_width();
        let height = briefing.map_height();
        let row_stride = usize::try_from(width)
            .expect("validated map width is nonnegative")
            .saturating_add(1);
        let rows = usize::try_from(height).expect("validated map height is nonnegative");
        let slots = row_stride.saturating_mul(rows);
        let mut usable = vec![0_u32; slots];
        let mut novel = vec![0_u32; slots];
        let mut unexplored = vec![0_u32; slots];

        for y in 0..height {
            let row = usize::try_from(y).expect("map row is nonnegative") * row_stride;
            for x in 0..width {
                let tile = TilePos::new(x, y);
                let offset = usize::try_from(x).expect("map column is nonnegative");
                let next = row + offset + 1;
                usable[next] = usable[next - 1];
                novel[next] = novel[next - 1];
                unexplored[next] = unexplored[next - 1];

                let radar_usable = matches!(
                    briefing.terrain_at(tile),
                    Some(Terrain::Ground | Terrain::Rock | Terrain::Pit)
                );
                if !radar_usable {
                    continue;
                }
                usable[next] += 1;
                if existing_arrays
                    .iter()
                    .all(|existing| !within_radar(*existing, tile))
                {
                    novel[next] += 1;
                }
                if !obs.explored(tile) {
                    unexplored[next] += 1;
                }
            }
        }

        let sight_radius = BuildingKind::Array
            .base_stats()
            .vision
            .min(RADAR_DETECT_RADIUS);
        Self {
            width,
            height,
            row_stride,
            usable,
            novel,
            unexplored,
            strategic: vec![0; slots],
            radar_half_widths: circle_half_widths(RADAR_DETECT_RADIUS),
            sight_half_widths: circle_half_widths(sight_radius),
        }
    }

    fn coverage(&self, anchor: TilePos) -> (u32, u32, u32) {
        (
            self.circle_sum(&self.usable, anchor, &self.radar_half_widths),
            self.circle_sum(&self.novel, anchor, &self.radar_half_widths),
            self.circle_sum(&self.unexplored, anchor, &self.sight_half_widths),
        )
    }

    fn with_demand(
        mut self,
        questions: &[crate::bot::battlefield::CoverageDemand],
        now: chassis::Tick,
    ) -> Self {
        let mut tiles = std::collections::BTreeMap::new();
        for question in questions {
            if question.deadline <= now {
                continue;
            }
            for dy in 0..question.size.1 {
                for dx in 0..question.size.0 {
                    let tile = question.anchor.offset(dx, dy);
                    tiles
                        .entry((tile.y, tile.x))
                        .and_modify(|weight: &mut u32| *weight = (*weight).max(question.weight))
                        .or_insert(question.weight);
                }
            }
        }
        for ((y, x), weight) in tiles {
            if x >= 0 && y >= 0 && x < self.width && y < self.height {
                let offset = y as usize * self.row_stride + x as usize + 1;
                if self.novel[offset] > self.novel[offset - 1] {
                    self.strategic[offset] = weight;
                }
            }
        }
        for row in self.strategic.chunks_exact_mut(self.row_stride) {
            for index in 1..row.len() {
                row[index] += row[index - 1];
            }
        }
        self
    }

    fn ready_demand(
        &self,
        anchor: TilePos,
        demands: &[crate::bot::battlefield::CoverageDemand],
        ready_at: chassis::Tick,
    ) -> u32 {
        let mut tiles = std::collections::BTreeMap::new();
        for demand in demands.iter().filter(|demand| ready_at < demand.deadline) {
            for dy in 0..demand.size.1 {
                for dx in 0..demand.size.0 {
                    let tile = demand.anchor.offset(dx, dy);
                    let row = tile.y.abs_diff(anchor.y) as usize;
                    if tile.x < 0
                        || tile.y < 0
                        || tile.x >= self.width
                        || tile.y >= self.height
                        || self
                            .radar_half_widths
                            .get(row)
                            .is_none_or(|half| tile.x.abs_diff(anchor.x) > *half as u32)
                    {
                        continue;
                    }
                    let index = tile.y as usize * self.row_stride + tile.x as usize + 1;
                    if self.novel[index] > self.novel[index - 1] {
                        tiles
                            .entry((tile.y, tile.x))
                            .and_modify(|weight: &mut u32| *weight = (*weight).max(demand.weight))
                            .or_insert(demand.weight);
                    }
                }
            }
        }
        tiles.values().sum()
    }

    fn circle_sum(&self, rows: &[u32], anchor: TilePos, half_widths: &[i32]) -> u32 {
        let radius = i32::try_from(half_widths.len().saturating_sub(1))
            .expect("Array coverage radius fits i32");
        let mut total = 0_u32;
        for dy in -radius..=radius {
            let y = anchor.y + dy;
            if y < 0 || y >= self.height {
                continue;
            }
            let half_width = half_widths[dy.unsigned_abs() as usize];
            let first = (anchor.x - half_width).max(0);
            let after_last = (anchor.x + half_width + 1).min(self.width);
            if first >= after_last {
                continue;
            }
            let row = usize::try_from(y).expect("in-bounds row is nonnegative") * self.row_stride;
            let first = usize::try_from(first).expect("clamped column is nonnegative");
            let after_last = usize::try_from(after_last).expect("clamped column is nonnegative");
            total += rows[row + after_last] - rows[row + first];
        }
        total
    }
}

fn circle_half_widths(radius: i32) -> Vec<i32> {
    let radius_squared = radius * radius;
    (0..=radius)
        .map(|dy| {
            (0..=radius)
                .rev()
                .find(|dx| dx * dx + dy * dy <= radius_squared)
                .expect("the vertical circle boundary always admits x = 0")
        })
        .collect()
}

#[cfg(test)]
fn array_coverage_naive(
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
            if tile.x < 0
                || tile.y < 0
                || tile.x >= briefing.map_width()
                || tile.y >= briefing.map_height()
                || matches!(briefing.terrain_at(tile), Some(Terrain::Peak) | None)
            {
                continue;
            }
            usable_radar += 1;
            if existing_arrays.iter().all(|existing| {
                let dx = i64::from(tile.x) - i64::from(existing.x);
                let dy = i64::from(tile.y) - i64::from(existing.y);
                let radius = i64::from(RADAR_DETECT_RADIUS);
                dx * dx + dy * dy > radius * radius
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::{PlayerSpec, Scenario};
    use crate::state::Faction;

    fn briefing_with(center: char) -> PublicMapBriefing {
        let mut map = vec![".......".to_owned(); 7];
        map[0].replace_range(0..1, "1");
        map[3].replace_range(3..4, &center.to_string());
        PublicMapBriefing::from_scenario(&Scenario {
            name: "Array terrain coverage".into(),
            seed: 0,
            map,
            players: vec![PlayerSpec {
                name: "player".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            }],
            units: Vec::new(),
            buildings: Vec::new(),
            meta: None,
        })
        .expect("coverage fixture is a valid public map")
    }

    #[test]
    fn array_selection_ranks_demand_at_the_exact_builder_ready_time() {
        use crate::bot::battlefield::{BattlefieldAssessment, CoverageDemand};
        let mut scenario = Scenario::skirmish();
        scenario.map = vec![".".repeat(90); 60];
        scenario.map[30].replace_range(30..31, "1");
        scenario.players.truncate(1);
        scenario.units.clear();
        let state = scenario.build().unwrap();
        let mut obs = Observation::omniscient(&state, PlayerId(0));
        let map = PublicMapBriefing::from_scenario(&scenario).unwrap();
        let home = obs.my_buildings[0].anchor;
        let original = Scenario::skirmish().build().unwrap();
        let mut worker = Observation::omniscient(&original, PlayerId(0)).my_units[0].clone();
        worker.tile = home.offset(-3, 0);
        obs.my_units = vec![worker];
        let builders: Vec<_> = obs
            .my_units
            .iter()
            .filter(|unit| unit.kind == UnitKind::Harvester)
            .collect();
        let distant = TilePos::new(12, 26);
        let timely = TilePos::new(50, 26);
        let mut policy = UtilityPolicy::new();
        policy.battlefield = std::sync::Arc::new(BattlefieldAssessment {
            coverage: vec![
                CoverageDemand {
                    asset: obs.my_buildings[0].id,
                    anchor: TilePos::new(0, 26),
                    size: (9, 9),
                    weight: 100,
                    deadline: 700,
                },
                CoverageDemand {
                    asset: obs.my_buildings[0].id,
                    anchor: TilePos::new(70, 26),
                    size: (1, 1),
                    weight: 5,
                    deadline: 10_000,
                },
            ],
            ..Default::default()
        });
        let quote = policy
            .strategic_array_quote_with_candidate(&obs, &map, home, &[], &[], &builders, |anchor| {
                if anchor == distant {
                    Some((builders[0].id, 1000))
                } else if anchor == timely {
                    Some((builders[0].id, 250))
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(quote.anchor, timely);
        assert_eq!(quote.strategic_radar, 5);
        assert_eq!(quote.builder_travel_cost, 250);

        policy.battlefield = std::sync::Arc::new(Default::default());
        let mut quoted = 0;
        assert!(
            policy
                .strategic_array_quote_with_candidate(&obs, &map, home, &[], &[], &builders, |_| {
                    quoted += 1;
                    Some((builders[0].id, 250))
                })
                .is_some()
        );
        assert_eq!(
            quoted, 1,
            "neutral demand needs only the best feasible quote"
        );
    }

    #[test]
    fn array_coverage_excludes_peaks_that_no_unit_can_occupy() {
        let obs = Observation {
            map_width: 7,
            map_height: 7,
            visible: vec![false; 49],
            explored: vec![false; 49],
            ..Observation::default()
        };
        let anchor = TilePos::new(3, 3);
        let ground_briefing = briefing_with('.');
        let peak_briefing = briefing_with('^');
        let ground = ArrayCoverageIndex::new(&obs, &ground_briefing, &[]).coverage(anchor);
        let peak = ArrayCoverageIndex::new(&obs, &peak_briefing, &[]).coverage(anchor);

        assert_eq!(ground.0, peak.0 + 1);
        assert_eq!(ground.1, peak.1 + 1);
        assert_eq!(ground.2, peak.2 + 1);
    }

    #[test]
    fn an_existing_array_leaves_no_novel_coverage_at_the_same_site() {
        let obs = Observation {
            map_width: 7,
            map_height: 7,
            visible: vec![false; 49],
            explored: vec![false; 49],
            ..Observation::default()
        };
        let anchor = TilePos::new(3, 3);
        let map = briefing_with('.');
        let (_, novel, _) = ArrayCoverageIndex::new(&obs, &map, &[anchor]).coverage(anchor);

        assert_eq!(novel, 0);
    }

    #[test]
    fn strategic_coverage_is_marginal_deadline_bound_and_overlap_deduplicated() {
        use crate::bot::battlefield::CoverageDemand;
        let obs = Observation {
            map_width: 7,
            map_height: 7,
            visible: vec![false; 49],
            explored: vec![false; 49],
            ..Observation::default()
        };
        let anchor = TilePos::new(3, 3);
        let demand = CoverageDemand {
            asset: BuildingId(1),
            anchor,
            size: (1, 1),
            weight: 5,
            deadline: 200,
        };
        let mut duplicate = demand.clone();
        duplicate.asset = BuildingId(2);
        duplicate.weight = 3;
        let demands = [demand, duplicate];
        let map = briefing_with('.');
        let index = ArrayCoverageIndex::new(&obs, &map, &[]).with_demand(&demands, 100);
        assert_eq!(index.ready_demand(anchor, &demands, 199), 5);
        assert_eq!(index.ready_demand(anchor, &demands, 200), 0);
        assert_eq!(
            index.circle_sum(&index.strategic, anchor, &index.radar_half_widths),
            5
        );
        let overlap = ArrayCoverageIndex::new(&obs, &map, &[anchor]).with_demand(&demands, 100);
        assert_eq!(overlap.ready_demand(anchor, &demands, 100), 0);
        let peaks = briefing_with('^');
        let blocked = ArrayCoverageIndex::new(&obs, &peaks, &[]).with_demand(&demands, 100);
        assert_eq!(blocked.ready_demand(anchor, &demands, 100), 0);
    }

    #[test]
    fn row_prefix_coverage_matches_the_naive_disc_scan() {
        let width = 19_i32;
        let height = 13_i32;
        let mut map = (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| match (x * 7 + y * 11) % 17 {
                        0 | 1 => '^',
                        2 | 3 => '#',
                        4 => '~',
                        _ => '.',
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        map[0].replace_range(0..1, "1");
        let briefing = PublicMapBriefing::from_scenario(&Scenario {
            name: "Array row-prefix differential".into(),
            seed: 0,
            map,
            players: vec![PlayerSpec {
                name: "player".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            }],
            units: Vec::new(),
            buildings: Vec::new(),
            meta: None,
        })
        .expect("differential fixture is a valid public map");
        let mut explored = vec![false; (width * height) as usize];
        for y in 0..height {
            for x in 0..width {
                explored[(y * width + x) as usize] = (x * 13 + y * 5) % 7 <= 2;
            }
        }
        let obs = Observation {
            map_width: width,
            map_height: height,
            visible: vec![false; (width * height) as usize],
            explored,
            ..Observation::default()
        };

        for existing_arrays in [
            Vec::new(),
            vec![TilePos::new(0, 0)],
            vec![
                TilePos::new(0, 0),
                TilePos::new(width / 2, height / 2),
                TilePos::new(width - 1, height - 1),
            ],
        ] {
            let index = ArrayCoverageIndex::new(&obs, &briefing, &existing_arrays);
            for y in -3..height + 3 {
                for x in -3..width + 3 {
                    let anchor = TilePos::new(x, y);
                    assert_eq!(
                        index.coverage(anchor),
                        array_coverage_naive(&obs, &briefing, anchor, &existing_arrays),
                        "coverage changed at {anchor:?} with {existing_arrays:?}"
                    );
                }
            }
            for anchor in [
                TilePos::new(-RADAR_DETECT_RADIUS, -RADAR_DETECT_RADIUS),
                TilePos::new(width + RADAR_DETECT_RADIUS, -RADAR_DETECT_RADIUS),
                TilePos::new(-RADAR_DETECT_RADIUS, height + RADAR_DETECT_RADIUS),
                TilePos::new(width + RADAR_DETECT_RADIUS, height + RADAR_DETECT_RADIUS),
            ] {
                assert_eq!(
                    index.coverage(anchor),
                    array_coverage_naive(&obs, &briefing, anchor, &existing_arrays),
                    "off-map coverage changed at {anchor:?} with {existing_arrays:?}"
                );
            }
        }
    }
}
