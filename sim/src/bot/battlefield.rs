//! Spatially grouped observations and consequential uncertainty, never hidden tracks.

use super::difficulty::DifficultyTuning;
use super::executive::{Army, ground_strength, weapon_burst_dps100};
use super::observation::{BuildingObs, Observation, UnitObs};
use crate::ids::{BuildingId, PlayerId, UnitId};
use crate::stats::Domain;
use chassis::Tick;
use chassis::grid::TilePos;
use serde::Serialize;
use std::collections::BTreeMap;

const CELL_SIZE: i32 = 8;
const APPROACH_RADIUS: i32 = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Motion {
    id: UnitId,
    tile: TilePos,
    previous: Option<(TilePos, Tick)>,
    last_seen: Tick,
}

/// Current observed force in one spatial cell and physical domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Concentration {
    /// Observed owner; anonymous radar never contributes here.
    pub player: PlayerId,
    /// Canonical cell anchor, not a forecast destination.
    pub anchor: TilePos,
    /// Whether these bodies currently occupy the air domain.
    pub air: bool,
    /// Exact currently observed members.
    pub members: Vec<UnitId>,
    /// HP-weighted observed capability against ground.
    pub ground_strength: u64,
    /// HP-weighted observed capability against aircraft.
    pub air_strength: u64,
}

/// Current pressure attributed to one specific friendly asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AssetPressure {
    /// Exact own or allied building.
    pub asset: BuildingId,
    /// Asset owner, not authority to issue its commands.
    pub player: PlayerId,
    /// Actual observed footprint anchor.
    pub anchor: TilePos,
    /// Current attacker positions remain separate from the defended anchor.
    pub attackers: Vec<UnitId>,
    /// Observed ground-body threat strength.
    pub ground: u64,
    /// Observed air-body threat strength.
    pub air: u64,
    /// Ordinary replacement cost, with Foundry survival priority.
    pub value: u32,
    /// Earliest continuous observation of these attackers.
    pub evidence_at: Tick,
}

/// A bounded region whose current contents could change an asset decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BattlefieldQuestion {
    /// Consumer asset.
    pub asset: BuildingId,
    /// Region anchor, not a claimed enemy destination.
    pub anchor: TilePos,
    /// Complete region whose visibility is needed for an answer.
    pub size: (i32, i32),
    /// Last direct evidence or current anonymous radar timestamp.
    pub evidence_at: Tick,
    /// True only for an unidentified current radar contact.
    pub anonymous: bool,
}

/// Marginal radar service for a consequential observed approach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoverageDemand {
    /// Asset that benefits from advance warning.
    pub asset: BuildingId,
    /// Bounded approach rectangle, not an inferred hostile position.
    pub anchor: TilePos,
    /// Complete demand rectangle.
    pub size: (i32, i32),
    /// Relative consequence, independent of hidden composition.
    pub weight: u32,
    /// Last useful service tick for this assessment.
    pub deadline: Tick,
}

/// Exact Executive ownership visible to mission and investment preparation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArmyCommitment {
    /// Stable Executive body identity.
    pub army: u32,
    /// Members are not a second allocator claim.
    pub members: Vec<UnitId>,
    /// Accepted tactical destination or current muster.
    pub goal: TilePos,
    /// Current ground-target capability of the available body.
    pub ground_strength: u64,
}

/// Current pressure remaining after reachable local service, credited once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DefensiveDemand {
    /// Exact defended asset.
    pub asset: BuildingId,
    /// Still-uncovered pressure from ground bodies.
    pub ground: u64,
    /// Still-uncovered pressure from aircraft.
    pub air: u64,
    /// Exact own providers credited to this region, regardless of planner owner.
    pub providers: Vec<UnitId>,
}

/// Immutable evidence shared by domain proposal preparation in one decision.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct BattlefieldAssessment {
    /// Whether a real observation has initialized this assessment.
    pub observed: bool,
    /// Observation tick.
    pub tick: Tick,
    /// Canonically grouped current sight.
    pub concentrations: Vec<Concentration>,
    /// Current credible asset pressure, most consequential first.
    pub pressure: Vec<AssetPressure>,
    /// Consequential unanswered questions.
    pub questions: Vec<BattlefieldQuestion>,
    /// Weighted early-warning demand. Radar does not answer composition questions.
    pub coverage: Vec<CoverageDemand>,
    /// Exact already-enlisted members; these are not free operational supply.
    pub enlisted: Vec<UnitId>,
    /// Coherent groups already owned by the Executive.
    pub commitments: Vec<ArmyCommitment>,
    /// Useful additional service, never a desired roster quota.
    pub uncovered: Vec<DefensiveDemand>,
    /// Observed movement: identity, previous tile/tick, latest tile/tick.
    pub motion: Vec<(UnitId, TilePos, Tick, TilePos, Tick)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Battlefield {
    map: (i32, i32),
    tracks: BTreeMap<UnitId, Motion>,
    assessment: BattlefieldAssessment,
    observed_at: Option<Tick>,
}

impl Battlefield {
    pub(crate) fn observe(
        &mut self,
        obs: &Observation,
        armies: &[Army],
        tuning: DifficultyTuning,
        public_map: Option<&super::briefing::PublicMapBriefing>,
    ) {
        if self.map != (obs.map_width, obs.map_height) {
            *self = Self::default();
            self.map = (obs.map_width, obs.map_height);
        }
        if self.observed_at.is_some_and(|tick| tick >= obs.tick) {
            return;
        }
        self.observed_at = Some(obs.tick);
        self.tracks
            .retain(|_, track| obs.tick.saturating_sub(track.last_seen) <= tuning.tactical_memory);
        let mut bins: BTreeMap<(PlayerId, i32, i32, bool), Concentration> = BTreeMap::new();
        let mut current: Vec<_> = obs
            .enemy_units
            .iter()
            .filter(|unit| unit.hp > 0 && obs.visible(unit.tile))
            .collect();
        current.sort_unstable_by_key(|unit| unit.id);
        for unit in &current {
            let track = self.tracks.entry(unit.id).or_insert(Motion {
                id: unit.id,
                tile: unit.tile,
                previous: None,
                last_seen: obs.tick,
            });
            if track.last_seen < obs.tick {
                if track.tile != unit.tile {
                    track.previous = Some((track.tile, track.last_seen));
                }
                track.tile = unit.tile;
                track.last_seen = obs.tick;
            }
            if !unit.kind.stats().can_fight() {
                continue;
            }
            let key = (
                unit.player,
                unit.tile.y / CELL_SIZE,
                unit.tile.x / CELL_SIZE,
                unit.body_domain() == Domain::Air,
            );
            let bin = bins.entry(key).or_insert_with(|| Concentration {
                player: key.0,
                anchor: TilePos::new(key.2 * CELL_SIZE, key.1 * CELL_SIZE),
                air: key.3,
                members: Vec::new(),
                ground_strength: 0,
                air_strength: 0,
            });
            bin.members.push(unit.id);
            bin.ground_strength = bin
                .ground_strength
                .saturating_add(ground_strength(unit.kind, unit.hp));
            bin.air_strength = bin
                .air_strength
                .saturating_add(strength_against(unit, Domain::Air));
        }
        let assets: Vec<_> = obs
            .my_buildings
            .iter()
            .chain(&obs.ally_buildings)
            .filter(|building| building.built && building.hp > 0)
            .collect();
        let mut pressure: BTreeMap<BuildingId, AssetPressure> = BTreeMap::new();
        for unit in &current {
            let strength = ground_strength(unit.kind, unit.hp);
            if strength == 0 {
                continue;
            }
            let Some(asset) = assets
                .iter()
                .copied()
                .filter(|building| {
                    distance_to_building(unit.tile, building) <= CELL_SIZE
                        || super::utility::ground_weapon_reaches_footprint(
                            unit,
                            building.anchor,
                            building.kind.base_stats().size,
                        )
                })
                .min_by_key(|building| (distance_to_building(unit.tile, building), building.id))
            else {
                continue;
            };
            let entry = pressure.entry(asset.id).or_insert_with(|| AssetPressure {
                asset: asset.id,
                player: asset.player,
                anchor: asset.anchor,
                attackers: Vec::new(),
                ground: 0,
                air: 0,
                value: asset_value(asset),
                evidence_at: self
                    .assessment
                    .pressure
                    .iter()
                    .find(|pressure| {
                        pressure.asset == asset.id
                            && obs.tick.saturating_sub(self.assessment.tick) <= tuning.cadence
                    })
                    .map_or(obs.tick, |pressure| pressure.evidence_at),
            });
            entry.attackers.push(unit.id);
            if unit.body_domain() == Domain::Air {
                entry.air = entry.air.saturating_add(strength);
            } else {
                entry.ground = entry.ground.saturating_add(strength);
            }
        }
        let mut pressure: Vec<_> = pressure.into_values().collect();
        pressure.sort_unstable_by_key(|entry| {
            (
                std::cmp::Reverse(entry.value),
                std::cmp::Reverse(entry.ground.saturating_add(entry.air)),
                entry.anchor.y,
                entry.anchor.x,
                entry.asset,
            )
        });
        let mut questions = Vec::new();
        for track in self
            .tracks
            .values()
            .filter(|track| track.last_seen < obs.tick)
        {
            if let Some(asset) = nearest_asset(&assets, track.tile) {
                let anchor = TilePos::new((track.tile.x - 4).max(0), (track.tile.y - 4).max(0));
                let size = (
                    (obs.map_width - anchor.x).min(9),
                    (obs.map_height - anchor.y).min(9),
                );
                if !(0..size.1).all(|dy| (0..size.0).all(|dx| obs.visible(anchor.offset(dx, dy)))) {
                    questions.push(BattlefieldQuestion {
                        asset: asset.id,
                        anchor,
                        size,
                        evidence_at: track.last_seen,
                        anonymous: false,
                    });
                }
            }
        }
        for &tile in &obs.blips {
            if let Some(asset) = nearest_asset(&assets, tile) {
                questions.push(BattlefieldQuestion {
                    asset: asset.id,
                    anchor: tile,
                    size: (1, 1),
                    evidence_at: obs.tick,
                    anonymous: true,
                });
            }
        }
        questions.sort_unstable_by_key(|question| {
            (
                question.asset,
                question.anchor.y,
                question.anchor.x,
                question.anonymous,
                question.evidence_at,
            )
        });
        questions.dedup_by(|a, b| {
            a.asset == b.asset && a.anchor == b.anchor && a.anonymous == b.anonymous
        });
        let mut coverage: Vec<_> = questions
            .iter()
            .map(|question| CoverageDemand {
                asset: question.asset,
                anchor: question.anchor,
                size: question.size,
                weight: 1,
                deadline: question.evidence_at.saturating_add(1800),
            })
            .collect();
        for threatened in &pressure {
            for id in &threatened.attackers {
                let track = &self.tracks[id];
                let anchor = TilePos::new((track.tile.x - 4).max(0), (track.tile.y - 4).max(0));
                coverage.push(CoverageDemand {
                    asset: threatened.asset,
                    anchor,
                    size: (
                        (obs.map_width - anchor.x).min(9),
                        (obs.map_height - anchor.y).min(9),
                    ),
                    weight: 1 + threatened.value.min(1000) / 250,
                    deadline: obs.tick.saturating_add(1800),
                });
            }
        }
        coverage.sort_unstable_by_key(|demand| {
            (
                demand.asset,
                demand.anchor.y,
                demand.anchor.x,
                demand.deadline,
            )
        });
        let mut enlisted: Vec<_> = armies
            .iter()
            .flat_map(|army| army.members.iter().copied())
            .collect();
        enlisted.sort_unstable();
        enlisted.dedup();
        let mut commitments: Vec<_> = armies
            .iter()
            .map(|army| ArmyCommitment {
                army: army.id.0,
                members: army.members.clone(),
                goal: army.target.unwrap_or(army.staging),
                ground_strength: super::executive::marching_strength(army, obs),
            })
            .collect();
        commitments.sort_unstable_by_key(|commitment| commitment.army);
        let mut credited = std::collections::BTreeSet::new();
        let mut routes = None;
        let mut air_routes = None;
        let mut uncovered = Vec::new();
        for front in &pressure {
            let mut demand = DefensiveDemand {
                asset: front.asset,
                ground: front.ground,
                air: front.air,
                providers: Vec::new(),
            };
            let mut providers: Vec<_> = obs
                .my_units
                .iter()
                .filter(|unit| {
                    unit.hp > 0
                        && unit.kind.stats().can_fight()
                        && unit.tile.chebyshev(front.anchor) <= 8
                        && !credited.contains(&unit.id)
                })
                .collect();
            providers.sort_unstable_by_key(|unit| unit.id);
            for unit in providers {
                let ground = ground_strength(unit.kind, unit.hp);
                let air = strength_against(unit, Domain::Air);
                if (demand.ground == 0 || ground == 0) && (demand.air == 0 || air == 0) {
                    continue;
                }
                let domain = unit.body_domain();
                let projection = if domain == Domain::Ground {
                    &mut routes
                } else {
                    &mut air_routes
                };
                if !projection
                    .get_or_insert_with(|| {
                        public_map.map_or_else(
                            || super::routing::RouteProjection::new(obs, domain),
                            |map| {
                                super::routing::RouteProjection::with_public_terrain(
                                    obs, domain, map,
                                )
                            },
                        )
                    })
                    .group_reaches_command_goal(&[unit.id], front.anchor)
                {
                    continue;
                }
                if demand.ground > 0 && ground > 0 {
                    demand.ground = demand.ground.saturating_sub(ground);
                } else {
                    demand.air = demand.air.saturating_sub(air);
                }
                demand.providers.push(unit.id);
                credited.insert(unit.id);
            }
            uncovered.push(demand);
        }
        let motion = self
            .tracks
            .values()
            .filter_map(|track| {
                track
                    .previous
                    .map(|(previous, tick)| (track.id, previous, tick, track.tile, track.last_seen))
            })
            .collect();
        self.assessment = BattlefieldAssessment {
            observed: true,
            tick: obs.tick,
            concentrations: bins.into_values().collect(),
            pressure,
            questions,
            coverage,
            enlisted,
            commitments,
            uncovered,
            motion,
        };
    }

    pub(crate) fn assessment(&self) -> &BattlefieldAssessment {
        &self.assessment
    }

    pub(crate) fn review_approaches(
        &mut self,
        obs: &Observation,
        experience: &super::experience::Experience,
    ) {
        use super::experience::{Doctrine, Outcome, OutcomeReason};
        for episode in experience.episodes().iter().filter(|episode| {
            matches!(
                episode.context.doctrine,
                Doctrine::Pressure | Doctrine::Air | Doctrine::Siege
            ) && matches!(episode.outcome, Outcome::Aborted | Outcome::Ineffective)
                && matches!(
                    episode.reason,
                    OutcomeReason::UnsafeApproach | OutcomeReason::ObservedCounter
                )
                && experience.contextual_score(episode.context) < 0
        }) {
            let tile = TilePos::new(episode.context.x, episode.context.y);
            if !obs.enemy_buildings.iter().any(|building| {
                u64::from(building.id.0) == episode.context.subject && building.hp > 0
            }) {
                continue;
            }
            let Some(asset) = obs
                .my_buildings
                .iter()
                .filter(|building| building.built && building.hp > 0)
                .min_by_key(|building| (distance_to_building(tile, building), building.id))
            else {
                continue;
            };
            let anchor = TilePos::new((tile.x - 4).max(0), (tile.y - 4).max(0));
            let size = (
                (obs.map_width - anchor.x).min(9),
                (obs.map_height - anchor.y).min(9),
            );
            if (0..size.1).all(|dy| (0..size.0).all(|dx| obs.visible(anchor.offset(dx, dy)))) {
                continue;
            }
            self.assessment.questions.push(BattlefieldQuestion {
                asset: asset.id,
                anchor,
                size,
                evidence_at: episode.finished_at,
                anonymous: false,
            });
        }
        self.assessment.questions.sort_unstable_by_key(|question| {
            (
                question.asset,
                question.anchor.y,
                question.anchor.x,
                question.anonymous,
                question.evidence_at,
            )
        });
        self.assessment.questions.dedup_by(|left, right| {
            left.asset == right.asset
                && left.anchor == right.anchor
                && left.anonymous == right.anonymous
        });
    }
}

fn nearest_asset<'a>(assets: &[&'a BuildingObs], tile: TilePos) -> Option<&'a BuildingObs> {
    assets
        .iter()
        .copied()
        .filter(|building| distance_to_building(tile, building) <= APPROACH_RADIUS)
        .min_by_key(|building| (distance_to_building(tile, building), building.id))
}

fn asset_value(asset: &BuildingObs) -> u32 {
    asset
        .kind
        .tier_stats(asset.tier)
        .construction
        .map_or(0, |cost| cost.cost)
        .saturating_add(if asset.kind == crate::stats::BuildingKind::Foundry {
            1000
        } else {
            0
        })
}

fn distance_to_building(tile: TilePos, building: &BuildingObs) -> i32 {
    let (width, height) = building.kind.base_stats().size;
    let closest = TilePos::new(
        tile.x
            .clamp(building.anchor.x, building.anchor.x + width - 1),
        tile.y
            .clamp(building.anchor.y, building.anchor.y + height - 1),
    );
    tile.chebyshev(closest)
}

fn strength_against(unit: &UnitObs, domain: Domain) -> u64 {
    u64::from(unit.hp)
        * unit
            .kind
            .stats()
            .weapons
            .iter()
            .filter(|weapon| weapon.targets.covers(domain))
            .map(weapon_burst_dps100)
            .sum::<u64>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::{BotDifficulty, Scenario};
    use crate::stats::{BuildingKind, UnitKind};

    fn fixture() -> Observation {
        let state = Scenario::skirmish().build().unwrap();
        let mut obs = Observation::omniscient(&state, PlayerId(0));
        obs.enemy_units.clear();
        let mut hostile = obs.my_units[0].clone();
        hostile.id = UnitId(100);
        hostile.player = PlayerId(1);
        hostile.kind = UnitKind::Sentinel;
        hostile.hp = UnitKind::Sentinel.stats().max_hp;
        hostile.tile = obs.my_buildings[0].anchor.offset(7, 0);
        obs.enemy_units.push(hostile);
        obs
    }

    fn tuning() -> DifficultyTuning {
        DifficultyTuning::for_level(BotDifficulty::Prime)
    }

    #[test]
    fn lost_motion_never_extrapolates_and_empty_last_tile_does_not_answer_region() {
        let mut obs = fixture();
        let mut battlefield = Battlefield::default();
        battlefield.observe(&obs, &[], tuning(), None);
        obs.tick = 12;
        obs.enemy_units[0].tile.x += 1;
        let last = obs.enemy_units[0].tile;
        battlefield.observe(&obs, &[], tuning(), None);
        obs.tick = 24;
        obs.enemy_units.clear();
        obs.visible.fill(false);
        obs.visible[(last.y * obs.map_width + last.x) as usize] = true;
        battlefield.observe(&obs, &[], tuning(), None);
        assert_eq!(battlefield.assessment.motion[0].3, last);
        assert_eq!(battlefield.assessment.motion[0].4, 12);
        assert!(battlefield.assessment.pressure.is_empty());
        assert_eq!(battlefield.assessment.questions.len(), 1);
        assert_eq!(battlefield.assessment.questions[0].size, (9, 9));
        obs.tick = 36;
        obs.visible.fill(true);
        battlefield.observe(&obs, &[], tuning(), None);
        assert!(battlefield.assessment.questions.is_empty());
    }

    #[test]
    fn visible_movement_to_a_new_asset_starts_a_new_pressure_reaction_window() {
        let mut obs = fixture();
        let first = obs.my_buildings[0].id;
        let mut expansion = obs.my_buildings[0].clone();
        expansion.id = BuildingId(900);
        expansion.anchor = obs.my_buildings[0].anchor.offset(24, 0);
        let next = expansion.anchor.offset(4, 0);
        obs.my_buildings.push(expansion);
        let mut battlefield = Battlefield::default();
        battlefield.observe(&obs, &[], tuning(), None);
        assert_eq!(battlefield.assessment.pressure[0].asset, first);
        obs.tick += tuning().cadence;
        battlefield.observe(&obs, &[], tuning(), None);
        assert_eq!(battlefield.assessment.pressure[0].evidence_at, 0);
        obs.tick += tuning().cadence;
        obs.enemy_units[0].tile = next;
        battlefield.observe(&obs, &[], tuning(), None);
        assert_eq!(battlefield.assessment.pressure[0].asset, BuildingId(900));
        assert_eq!(battlefield.assessment.pressure[0].evidence_at, obs.tick);
        let arrival = obs.tick;
        obs.tick += tuning().cadence;
        battlefield.observe(&obs, &[], tuning(), None);
        assert_eq!(battlefield.assessment.pressure[0].evidence_at, arrival);
    }

    #[test]
    fn anonymous_radar_does_not_supply_strength_or_domain() {
        let mut obs = fixture();
        obs.blips = vec![obs.enemy_units[0].tile];
        obs.enemy_units.clear();
        let mut battlefield = Battlefield::default();
        battlefield.observe(&obs, &[], tuning(), None);
        assert!(battlefield.assessment.concentrations.is_empty());
        assert!(battlefield.assessment.pressure.is_empty());
        assert!(battlefield.assessment.questions[0].anonymous);
    }

    #[test]
    fn one_attacker_is_not_counted_again_for_neighbouring_assets() {
        let mut obs = fixture();
        let mut asset = obs.my_buildings[0].clone();
        asset.id = BuildingId(900);
        asset.kind = BuildingKind::Fabricator;
        asset.anchor.x += 5;
        obs.my_buildings.push(asset);
        let mut battlefield = Battlefield::default();
        battlefield.observe(&obs, &[], tuning(), None);
        assert_eq!(
            battlefield
                .assessment
                .pressure
                .iter()
                .map(|p| p.attackers.len())
                .sum::<usize>(),
            1
        );
        let expected = battlefield.assessment.clone();
        obs.my_buildings.reverse();
        obs.enemy_units.reverse();
        let mut permuted = Battlefield::default();
        permuted.observe(&obs, &[], tuning(), None);
        assert_eq!(expected, permuted.assessment);
    }

    #[test]
    fn repeated_observation_is_inert_and_memory_expires() {
        let mut obs = fixture();
        let mut battlefield = Battlefield::default();
        battlefield.observe(&obs, &[], tuning(), None);
        let expected = battlefield.clone();
        battlefield.observe(&obs, &[], tuning(), None);
        assert_eq!(battlefield, expected);
        obs.tick = tuning().tactical_memory + 1;
        obs.enemy_units.clear();
        obs.visible.fill(false);
        battlefield.observe(&obs, &[], tuning(), None);
        assert!(battlefield.tracks.is_empty());
        assert!(battlefield.assessment.questions.is_empty());
    }
}
