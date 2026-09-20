//! Receding-horizon quotes for restoring frames around one support site.

use super::construction_checks::ConstructionChecks;
use super::economic_investment::{FundingCalendar, economic_case};
use super::*;
use crate::bot::navigation::travel::travel_ticks;
use crate::bot::query_work::QueryPurpose;
use std::collections::BTreeMap;

impl UtilityPolicy {
    pub(super) fn value_extractor_developments<'a>(
        &'a self,
        context: EconomicInvestmentContext<'a>,
        geometry: &mut Option<ConstructionChecks<'a>>,
        funding: &FundingCalendar<'_>,
        proposals: &mut Vec<EconomicInvestment>,
    ) {
        let obs = context.obs;
        let frames = proposals
            .iter()
            .enumerate()
            .filter_map(|(index, proposal)| match proposal.key {
                EconomicInvestmentKey::Build {
                    kind: BuildingKind::Extractor,
                    anchor,
                } => Some((index, anchor)),
                _ => None,
            })
            .collect::<Vec<_>>();
        if frames.is_empty() {
            return;
        }
        let Some(worker) = frames.iter().find_map(|(index, _)| {
            obs.my_units
                .iter()
                .find(|worker| Some(worker.id) == proposals[*index].builder)
        }) else {
            return;
        };
        let built = obs
            .my_buildings
            .iter()
            .filter(|building| {
                building.built
                    && building.kind == BuildingKind::Extractor
                    && !Self::frame_has_foundry_support(obs, building.anchor)
            })
            .map(|building| building.anchor)
            .collect::<Vec<_>>();
        if frames.len() + built.len() < 2 {
            return;
        }
        let foundry = BuildingKind::Foundry.base_stats();
        let construction = foundry.construction.unwrap();
        let can_build_support = construction.requires.iter().all(|kind| {
            obs.my_buildings
                .iter()
                .any(|building| building.built && building.kind == *kind)
        }) && Self::projected_foundries(obs).1 == 0
            && self.state.foundry_saving.is_none();
        let mut sites = BTreeMap::<Vec<usize>, Vec<TilePos>>::new();
        for y in 0..obs.map_height - foundry.size.1 + 1 {
            for x in 0..obs.map_width - foundry.size.0 + 1 {
                let anchor = TilePos::new(x, y);
                let members = frames
                    .iter()
                    .filter(|(_, frame)| Self::foundry_supports_extractor(anchor, *frame))
                    .map(|(index, _)| *index)
                    .collect::<Vec<_>>();
                let existing = obs.my_buildings.iter().any(|building| {
                    building.built
                        && building.kind == BuildingKind::Foundry
                        && building.anchor == anchor
                });
                let owned = built
                    .iter()
                    .filter(|frame| Self::foundry_supports_extractor(anchor, **frame))
                    .count();
                if !members.is_empty()
                    && members.len() + owned >= 2
                    && (existing || can_build_support)
                    && compact_group(
                        members
                            .iter()
                            .filter_map(|index| {
                                proposals[*index].build().map(|(_, frame, _)| frame)
                            })
                            .chain(
                                built.iter().copied().filter(|frame| {
                                    Self::foundry_supports_extractor(anchor, *frame)
                                }),
                            ),
                    )
                {
                    sites.entry(members).or_default().push(anchor);
                }
            }
        }
        if sites.is_empty() {
            return;
        }
        let geometry = geometry.get_or_insert_with(|| {
            ConstructionChecks::new(
                crate::bot::query_work::QueryPurpose::ExtractorCluster,
                self,
                obs,
                context.briefing,
                context.unit_contacts,
                context.building_contacts,
                context.orientation,
            )
        });
        let base_routes = RouteProjection::with_public_terrain(
            QueryPurpose::ExtractorCluster,
            obs,
            Domain::Ground,
            context.briefing,
        );
        let open_origin = routing::ground_open(QueryPurpose::ExtractorCluster, obs, worker.tile);
        let dials = Dials::scripted(
            context.profile,
            DifficultyTuning::for_level(context.profile.difficulty),
        );
        let economy = expansion_economy(
            &dials,
            obs,
            obs.scrap,
            Reserve::Exact(context.protected_scrap),
        );
        let hostile_starts = self.uncleared_hostile_starts(context.briefing, obs.me);
        let security = expansion::ExpansionAssessmentContext {
            obs,
            public_map: context.briefing,
            unit_contacts: context.unit_contacts,
            uncleared_hostile_starts: &hostile_starts,
            combat_core_exclusions: context.unavailable,
            same_think_intents: &[],
            minimum_core_equivalents: dials.minimum_core_equivalents,
            own_strength_scale: dials.own_strength_scale,
            economy,
        };
        // Freeze single-frame prices before comparing overlapping development alternatives.
        let original = proposals.to_vec();
        let placement = super::terrain::PlacementGeometry::new(obs);
        for (members, mut anchors) in sites {
            anchors.sort_by_key(|anchor| {
                (
                    !obs.my_buildings.iter().any(|building| {
                        building.built
                            && building.kind == BuildingKind::Foundry
                            && building.anchor == *anchor
                    }),
                    (anchor.x - worker.tile.x).unsigned_abs()
                        + (anchor.y - worker.tile.y).unsigned_abs(),
                    anchor.y,
                    anchor.x,
                )
            });
            let Some((anchor, existing)) = anchors.into_iter().find_map(|anchor| {
                let existing = obs.my_buildings.iter().any(|building| {
                    building.built
                        && building.kind == BuildingKind::Foundry
                        && building.anchor == anchor
                });
                if existing
                    || (placement.valid(self, BuildingKind::Foundry, anchor)
                        && (!open_origin || base_routes.reaches(worker.tile, anchor))
                        && geometry.resource_access_survives(BuildingKind::Foundry, anchor)
                        && geometry
                            .future_ground_producer_egress_survives(BuildingKind::Foundry, anchor)
                        && geometry
                            .safe_implicit_builder(BuildingKind::Foundry, anchor, &[worker])
                            .is_some())
                {
                    Some((anchor, existing))
                } else {
                    None
                }
            }) else {
                continue;
            };
            let mut layout = members
                .iter()
                .filter_map(|index| original[*index].build())
                .map(|(kind, anchor, _)| (kind, anchor, worker.id))
                .collect::<Vec<_>>();
            if !existing {
                layout.push((BuildingKind::Foundry, anchor, worker.id));
            }
            let mut remaining = members;
            let mut cursor = worker.clone();
            let mut elapsed = 0;
            let mut cost = 0u32;
            let mut benefit = 0u64;
            let mut first = None;
            let deadline = original[remaining[0]].deadline;
            let horizon = deadline.saturating_sub(obs.tick);
            let mut restored = 0u64;
            while !remaining.is_empty() {
                let next = remaining
                    .iter()
                    .filter_map(|index| {
                        let (_, frame, _) = original[*index].build()?;
                        geometry.safe_implicit_builder(
                            BuildingKind::Extractor,
                            frame,
                            &[&cursor],
                        )?;
                        geometry
                            .builder_travel_cost(&cursor, BuildingKind::Extractor, frame)
                            .map(|distance| (distance, frame.y, frame.x, *index, frame))
                    })
                    .min();
                let Some((distance, _, _, index, frame)) = next else {
                    break;
                };
                remaining.retain(|candidate| *candidate != index);
                cost = cost.saturating_add(original[index].cost);
                elapsed = elapsed
                    .max(funding.delay(cost, deadline))
                    .saturating_add(travel_ticks(cursor.kind, distance))
                    .saturating_add(
                        u64::from(
                            BuildingKind::Extractor
                                .base_stats()
                                .construction
                                .unwrap()
                                .build_ticks,
                        )
                        .div_ceil(u64::from(cursor.kind.stats().build_rate.max(1))),
                    );
                first.get_or_insert((index, elapsed));
                let rate = if existing || Self::frame_has_foundry_support(obs, frame) {
                    crate::stats::EXTRACTOR_SUPPORTED_INCOME_PER_MINUTE
                } else {
                    crate::stats::EXTRACTOR_REMOTE_INCOME_PER_MINUTE
                };
                benefit = benefit.saturating_add(
                    horizon.saturating_sub(elapsed) * u64::from(rate)
                        / (u64::from(crate::TICKS_PER_SECOND) * 60),
                );
                restored += u64::from(!Self::frame_has_foundry_support(obs, frame));
                cursor.tile = frame;
            }
            if !remaining.is_empty() {
                continue;
            }
            let Some((first, first_delay)) = first else {
                continue;
            };
            if !existing {
                cost = cost.saturating_add(construction.cost);
                let Some(distance) =
                    geometry.builder_travel_cost(&cursor, BuildingKind::Foundry, anchor)
                else {
                    continue;
                };
                elapsed = elapsed
                    .max(funding.delay(cost, deadline))
                    .saturating_add(travel_ticks(cursor.kind, distance))
                    .saturating_add(
                        u64::from(construction.build_ticks)
                            .div_ceil(u64::from(cursor.kind.stats().build_rate.max(1))),
                    );
                let owned = built
                    .iter()
                    .filter(|frame| Self::foundry_supports_extractor(anchor, **frame))
                    .count() as u64;
                let income = (restored + owned)
                    * u64::from(
                        crate::stats::EXTRACTOR_SUPPORTED_INCOME_PER_MINUTE
                            - crate::stats::EXTRACTOR_REMOTE_INCOME_PER_MINUTE,
                    );
                let support_return = horizon.saturating_sub(elapsed) * income
                    / (u64::from(crate::TICKS_PER_SECOND) * 60)
                    + horizon.saturating_sub(
                        elapsed.max(crate::stats::FOUNDRY_DRIP_START_TICK.saturating_sub(obs.tick)),
                    ) / crate::stats::FOUNDRY_DRIP_PERIOD;
                if support_return < u64::from(construction.cost) {
                    continue;
                }
                benefit = benefit.saturating_add(support_return);
                let opportunity =
                    expansion::FoundryOpportunity::capacity_only(anchor, benefit, economy);
                let assessment = expansion::assess_retained_foundry(
                    opportunity,
                    worker.id,
                    &security,
                    &mut self.queries.expansion_routing_cache.borrow_mut(),
                );
                if assessment.disposition != expansion::ExpansionDisposition::Build {
                    continue;
                }
            }
            if elapsed >= horizon || benefit < u64::from(cost) {
                continue;
            }
            let proposal = &mut proposals[first];
            if benefit.saturating_sub(u64::from(cost))
                > proposal
                    .benefit
                    .saturating_sub(u64::from(proposal.valuation_cost))
            {
                if !self.combined_build_layout_with_builders_is_safe(
                    obs,
                    context.briefing,
                    context.unit_contacts,
                    context.building_contacts,
                    context.orientation,
                    &layout,
                ) {
                    continue;
                }
                proposal.benefit = benefit;
                proposal.valuation_cost = cost;
                proposal.builder = Some(worker.id);
                proposal.ready_at = obs.tick.saturating_add(first_delay);
                proposal.case = economic_case(benefit, cost, elapsed);
            }
        }
        let best = frames
            .iter()
            .map(|(index, _)| &proposals[*index])
            .max_by_key(|proposal| {
                (
                    proposal
                        .benefit
                        .saturating_sub(u64::from(proposal.valuation_cost)),
                    std::cmp::Reverse(proposal.key),
                )
            })
            .map(|proposal| proposal.key);
        proposals.retain(|proposal| {
            !matches!(
                proposal.key,
                EconomicInvestmentKey::Build {
                    kind: BuildingKind::Extractor,
                    ..
                }
            ) || Some(proposal.key) == best
        });
    }
}

fn compact_group(frames: impl Iterator<Item = TilePos>) -> bool {
    let mut bounds = None::<(i32, i32, i32, i32)>;
    for frame in frames {
        bounds = Some(bounds.map_or(
            (frame.x, frame.x, frame.y, frame.y),
            |(left, right, top, bottom)| {
                (
                    left.min(frame.x),
                    right.max(frame.x),
                    top.min(frame.y),
                    bottom.max(frame.y),
                )
            },
        ));
    }
    bounds.is_some_and(|(left, right, top, bottom)| {
        right - left <= crate::stats::EXTRACTOR_SUPPORT_RADIUS
            && bottom - top <= crate::stats::EXTRACTOR_SUPPORT_RADIUS
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_support_range_alone_does_not_make_a_tight_defense_area() {
        let spread = [TilePos::new(5, 10), TilePos::new(18, 10)];
        assert!(
            spread
                .iter()
                .all(|frame| UtilityPolicy::foundry_supports_extractor(
                    TilePos::new(10, 10),
                    *frame
                ))
        );
        assert!(!compact_group(spread.into_iter()));
        assert!(compact_group(
            [
                TilePos::new(20, 10),
                TilePos::new(22, 10),
                TilePos::new(24, 10)
            ]
            .into_iter()
        ));
    }
}
