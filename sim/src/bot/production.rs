//! Immediate queue capacity after already-staged commands.

use super::{Intent, Observation, resources::ProducerLaneReservations};
#[cfg(test)]
use crate::bot::observation::ObservationData;
use crate::{
    ids::BuildingId,
    stats::{BuildingKind, QUEUE_CAP, UnitKind},
};
use std::collections::BTreeMap;

pub(super) struct ImmediateProduction<'a> {
    observation: &'a Observation,
    reservations: &'a ProducerLaneReservations,
    staged: BTreeMap<BuildingId, Vec<UnitKind>>,
}

pub(super) struct AvailableProducer {
    kind: UnitKind,
    pub(super) id: BuildingId,
    pub(super) depth: usize,
}

impl<'a> ImmediateProduction<'a> {
    /// `prior` includes the accepted current-tick prefix against the raw observation.
    pub(super) fn new(
        observation: &'a Observation,
        reservations: &'a ProducerLaneReservations,
        prior: &[Intent],
    ) -> Self {
        let mut staged = BTreeMap::<_, Vec<_>>::new();
        for intent in prior {
            if let Intent::TrainAt { building, kind } = intent {
                staged.entry(*building).or_default().push(*kind);
            }
        }
        Self {
            observation,
            reservations,
            staged,
        }
    }

    pub(super) fn available(
        &self,
        kind: UnitKind,
        depth_limit: impl Fn(BuildingKind) -> usize,
    ) -> impl Iterator<Item = AvailableProducer> {
        self.observation
            .my_buildings
            .iter()
            .enumerate()
            .filter_map(move |(index, building)| {
                let staged = self.staged.get(&building.id).map_or(&[][..], Vec::as_slice);
                let depth = self
                    .observation
                    .my_queues
                    .get(index)?
                    .len()
                    .saturating_add(staged.len());
                (building.built
                    && building.kind.base_stats().produces.contains(&kind)
                    && depth < depth_limit(building.kind).min(QUEUE_CAP)
                    && self
                        .reservations
                        .allows_raw_immediate_append(building.id, staged, kind))
                .then_some(AvailableProducer {
                    kind,
                    id: building.id,
                    depth,
                })
            })
    }

    pub(super) fn lowest_id(
        &self,
        kind: UnitKind,
        depth_limit: usize,
    ) -> Option<AvailableProducer> {
        self.available(kind, |_| depth_limit)
            .min_by_key(|producer| producer.id)
    }

    pub(super) fn append(&mut self, producer: AvailableProducer) -> Intent {
        self.staged
            .entry(producer.id)
            .or_default()
            .push(producer.kind);
        Intent::TrainAt {
            building: producer.id,
            kind: producer.kind,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::{
        BuildingObs,
        resources::{ReservedProducerJob, ResourceSnapshot},
    };
    use chassis::grid::TilePos;

    fn observation() -> Observation {
        let mut obs = Observation::from_data(ObservationData {
            map_width: 48,
            map_height: 24,
            visible: vec![true; 48 * 24],
            explored: vec![true; 48 * 24],
            ..Default::default()
        });
        obs.my_buildings = [7, 3]
            .into_iter()
            .map(|id| BuildingObs {
                id: BuildingId(id),
                player: obs.me,
                kind: BuildingKind::Foundry,
                anchor: TilePos::new(id as i32 * 4, 4),
                hp: BuildingKind::Foundry.base_stats().max_hp,
                built: true,
                seen: true,
                provisional: false,
                tier: 0,
            })
            .collect();
        obs.my_queues = vec![vec![], vec![]];
        obs.my_queue_progress = vec![0; 2];
        obs
    }

    #[test]
    fn callers_choose_order_while_paid_and_staged_occurrences_share_capacity() {
        let mut obs = observation();
        obs.my_queues[1] = vec![UnitKind::Sentinel];
        let mut production = ImmediateProduction::new(&obs, ProducerLaneReservations::empty(), &[]);
        assert_eq!(
            production.lowest_id(UnitKind::Sentinel, 2).unwrap().id,
            BuildingId(3)
        );
        let shallowest = production
            .available(UnitKind::Sentinel, |_| 2)
            .min_by_key(|p| (p.depth, p.id))
            .unwrap();
        assert_eq!(shallowest.id, BuildingId(7));
        let first = production.append(shallowest);
        let low = production.lowest_id(UnitKind::Sentinel, 2).unwrap();
        let second = production.append(low);
        assert_eq!(
            second,
            Intent::TrainAt {
                building: BuildingId(3),
                kind: UnitKind::Sentinel
            }
        );
        let last = production.lowest_id(UnitKind::Sentinel, 2).unwrap();
        assert_eq!(last.id, BuildingId(7));
        let third = production.append(last);
        assert_eq!(
            first, third,
            "identical commands occupy distinct queue positions"
        );
        assert!(production.lowest_id(UnitKind::Sentinel, 2).is_none());
        let prior = [first, second, third, Intent::StopUnits { units: vec![] }];
        let rebuilt = ImmediateProduction::new(&obs, ProducerLaneReservations::empty(), &prior);
        assert!(rebuilt.lowest_id(UnitKind::Sentinel, 2).is_none());
        assert_eq!(
            obs.my_queues,
            [vec![], vec![UnitKind::Sentinel]],
            "staging must not manufacture paid inventory"
        );
    }

    #[test]
    fn unavailable_queues_and_wrong_producer_kinds_cannot_supply_capacity() {
        let mut obs = observation();
        obs.my_queues[0] = vec![UnitKind::Sentinel; QUEUE_CAP];
        obs.my_queues.pop();
        let available = |obs: &Observation| {
            ImmediateProduction::new(obs, ProducerLaneReservations::empty(), &[])
                .lowest_id(UnitKind::Sentinel, usize::MAX)
                .map(|p| p.id)
        };
        assert_eq!(available(&obs), None);
        obs.my_queues.push(vec![]);
        obs.my_buildings[1].built = false;
        assert_eq!(available(&obs), None);
        obs.my_buildings[1].built = true;
        obs.my_buildings[1].kind = BuildingKind::Fabricator;
        assert_eq!(available(&obs), None);
        obs.my_buildings[1].kind = BuildingKind::Foundry;
        assert_eq!(available(&obs), Some(BuildingId(3)));
    }

    #[test]
    fn accepted_prefix_is_required_once_and_residual_appends_preserve_future_timing() {
        let obs = observation();
        let ticks = u64::from(UnitKind::Sentinel.stats().train_ticks);
        let resources = ResourceSnapshot::from_observation(&obs)
            .planning_projection(5 * ticks, 1)
            .unwrap();
        let job = |enqueue, start| ReservedProducerJob {
            producer: BuildingId(3),
            kind: UnitKind::Sentinel,
            enqueued_at: enqueue,
            starts_at: start,
            ready_at: start + ticks - 1,
            ready_before: start + ticks,
        };
        let reservations = ProducerLaneReservations::from_jobs(
            &resources,
            [job(0, 0), job(0, ticks), job(3 * ticks, 3 * ticks)],
        )
        .unwrap();
        let train = Intent::TrainAt {
            building: BuildingId(3),
            kind: UnitKind::Sentinel,
        };
        for prefix in [vec![], vec![train.clone()]] {
            let production = ImmediateProduction::new(&obs, &reservations, &prefix);
            assert_eq!(
                production
                    .lowest_id(UnitKind::Sentinel, QUEUE_CAP)
                    .unwrap()
                    .id,
                BuildingId(7),
                "an incomplete accepted prefix must not enter the reserved lane"
            );
        }
        let mut production =
            ImmediateProduction::new(&obs, &reservations, &[train.clone(), train.clone()]);
        let spare = production.lowest_id(UnitKind::Sentinel, QUEUE_CAP).unwrap();
        assert_eq!((spare.id, spare.depth), (BuildingId(3), 2));
        assert_eq!(production.append(spare), train);
        assert_eq!(
            production
                .lowest_id(UnitKind::Sentinel, QUEUE_CAP)
                .unwrap()
                .id,
            BuildingId(7),
            "a second residual append would delay the accepted future job"
        );
    }
}
