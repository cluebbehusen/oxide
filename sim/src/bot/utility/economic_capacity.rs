//! Shared deadline-bound Airworks time for concurrent operational demand.

use super::AirCapacityDemand;
use std::collections::VecDeque;

pub(super) struct AirCapacityLane {
    pub(super) ready_after: u64,
    pub(super) serves: Vec<bool>,
}

pub(super) fn additional_air_capacity_value(
    demands: &[AirCapacityDemand],
    now: u64,
    existing: &[AirCapacityLane],
    candidate: &AirCapacityLane,
) -> u64 {
    if demands.is_empty() || !candidate.serves.iter().any(|serves| *serves) {
        return 0;
    }
    let deadlines = demands
        .iter()
        .map(|demand| demand.deadline.saturating_sub(now))
        .collect::<Vec<_>>();
    let mut time = AirworkTime::new(demands.len() + 2);
    let claims = demands
        .iter()
        .enumerate()
        .map(|(index, demand)| time.connect(index + 2, 1, demand.work_ticks))
        .collect::<Vec<_>>();
    for lane in existing {
        time.add_lane(lane, &deadlines);
    }
    time.fill();
    let unserved = claims
        .iter()
        .enumerate()
        .map(|(index, edge)| time.edges[index + 2][*edge].remaining)
        .collect::<Vec<_>>();
    time.add_lane(candidate, &deadlines);
    time.fill();
    demands
        .iter()
        .enumerate()
        .map(|(index, demand)| {
            let gained =
                unserved[index].saturating_sub(time.edges[index + 2][claims[index]].remaining);
            (gained / u64::from(demand.kind.stats().train_ticks.max(1)))
                .saturating_mul(u64::from(demand.kind.stats().cost))
        })
        .fold(0, u64::saturating_add)
}

struct TimeEdge {
    to: usize,
    reverse: usize,
    remaining: u64,
}

struct AirworkTime {
    edges: Vec<Vec<TimeEdge>>,
}

impl AirworkTime {
    fn new(nodes: usize) -> Self {
        Self {
            edges: (0..nodes).map(|_| Vec::new()).collect(),
        }
    }

    fn connect(&mut self, from: usize, to: usize, capacity: u64) -> usize {
        let edge = self.edges[from].len();
        let reverse = self.edges[to].len();
        self.edges[from].push(TimeEdge {
            to,
            reverse,
            remaining: capacity,
        });
        self.edges[to].push(TimeEdge {
            to: from,
            reverse: edge,
            remaining: 0,
        });
        edge
    }

    fn add_lane(&mut self, lane: &AirCapacityLane, deadlines: &[u64]) {
        let mut ends = deadlines
            .iter()
            .zip(&lane.serves)
            .filter_map(|(deadline, serves)| {
                (*serves && *deadline > lane.ready_after).then_some(*deadline)
            })
            .collect::<Vec<_>>();
        ends.sort_unstable();
        ends.dedup();
        let mut start = lane.ready_after;
        for end in ends {
            let interval = self.edges.len();
            self.edges.push(Vec::new());
            self.connect(0, interval, end - start);
            for (index, (deadline, serves)) in deadlines.iter().zip(&lane.serves).enumerate() {
                if *serves && *deadline >= end {
                    self.connect(interval, index + 2, end - start);
                }
            }
            start = end;
        }
    }

    // Residual edges can move existing work to another compatible interval,
    // but never revoke delivered demand when the additional lane is introduced.
    fn fill(&mut self) {
        let mut parents = vec![None; self.edges.len()];
        let mut queue = VecDeque::new();
        loop {
            parents.fill(None);
            parents[0] = Some((0, 0));
            queue.clear();
            queue.push_back(0);
            while let Some(from) = queue.pop_front() {
                for (index, edge) in self.edges[from].iter().enumerate() {
                    if edge.remaining > 0 && parents[edge.to].is_none() {
                        parents[edge.to] = Some((from, index));
                        queue.push_back(edge.to);
                    }
                }
                if parents[1].is_some() {
                    break;
                }
            }
            if parents[1].is_none() {
                return;
            }
            let mut amount = u64::MAX;
            let mut to = 1;
            while to != 0 {
                let (from, edge) = parents[to].expect("the sink has a complete time path");
                amount = amount.min(self.edges[from][edge].remaining);
                to = from;
            }
            to = 1;
            while to != 0 {
                let (from, edge) = parents[to].expect("the sink has a complete time path");
                let reverse = self.edges[from][edge].reverse;
                self.edges[from][edge].remaining -= amount;
                self.edges[to][reverse].remaining += amount;
                to = from;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::allocation::StandingForceServiceKey;
    use crate::stats::UnitKind;
    use chassis::grid::TilePos;

    fn demand(work_ticks: u64, deadline: u64) -> AirCapacityDemand {
        AirCapacityDemand {
            work_ticks,
            deadline,
            kind: UnitKind::Skyhook,
            service: StandingForceServiceKey::point(TilePos::new(0, 0)),
        }
    }

    fn lane(ready_after: u64, serves: &[bool]) -> AirCapacityLane {
        AirCapacityLane {
            ready_after,
            serves: serves.to_vec(),
        }
    }

    fn value(ticks: u64) -> u64 {
        ticks / u64::from(UnitKind::Skyhook.stats().train_ticks)
            * u64::from(UnitKind::Skyhook.stats().cost)
    }

    #[test]
    fn simultaneous_work_cannot_reuse_one_lane_twice() {
        let work = [demand(3_000, 3_000), demand(3_000, 3_000)];
        assert_eq!(
            additional_air_capacity_value(
                &work,
                0,
                &[lane(0, &[true, true])],
                &lane(500, &[true, true])
            ),
            value(2_500),
        );
        assert_eq!(
            additional_air_capacity_value(
                &work,
                0,
                &[lane(0, &[true, false]), lane(0, &[false, true])],
                &lane(500, &[true, true])
            ),
            0,
        );
    }

    #[test]
    fn rerouting_shared_time_preserves_restricted_customers() {
        let work = [demand(2_000, 2_000), demand(2_000, 2_000)];
        assert_eq!(
            additional_air_capacity_value(
                &work,
                0,
                &[lane(0, &[true, true]), lane(0, &[true, false])],
                &lane(0, &[true, true])
            ),
            0,
            "the baseline must move its first customer off the shared lane",
        );
        assert_eq!(
            additional_air_capacity_value(
                &work,
                0,
                &[lane(0, &[true, true])],
                &lane(0, &[true, false])
            ),
            value(2_000),
            "a restricted new lane can free the old lane for the other customer",
        );
        assert_eq!(
            additional_air_capacity_value(
                &work,
                0,
                &[lane(0, &[true, false])],
                &lane(0, &[true, false])
            ),
            0,
            "extra time cannot satisfy an unreachable customer",
        );
    }

    #[test]
    fn deadlines_and_readiness_bound_each_shared_interval() {
        let work = [demand(1_000, 1_100), demand(3_000, 3_100)];
        assert_eq!(
            additional_air_capacity_value(
                &work,
                100,
                &[lane(0, &[true, true])],
                &lane(1_000, &[true, false])
            ),
            0,
            "a lane ready on the early deadline cannot serve that customer",
        );
        assert_eq!(
            additional_air_capacity_value(
                &work,
                100,
                &[lane(0, &[true, true])],
                &lane(1_000, &[false, true])
            ),
            value(1_000),
        );
        assert_eq!(
            additional_air_capacity_value(
                &[demand(2_000, 3_000)],
                0,
                &[lane(2_000, &[true])],
                &lane(2_500, &[true])
            ),
            value(500),
            "paid or deferred lanes contribute only after readiness",
        );
        assert_eq!(
            additional_air_capacity_value(&work, 3_100, &[], &lane(0, &[true, true])),
            0,
        );
        assert_eq!(additional_air_capacity_value(&[], 0, &[], &lane(0, &[])), 0);
    }

    #[test]
    fn partial_work_never_promises_a_whole_additional_unit() {
        let ticks = u64::from(UnitKind::Skyhook.stats().train_ticks);
        assert_eq!(
            additional_air_capacity_value(&[demand(ticks - 1, ticks)], 0, &[], &lane(0, &[true])),
            0,
        );
    }
}
