//! Scouting: one scout at a time looks at the most valuable place the seat
//! has not seen for a while, hostile starts first. With no scout it asks
//! production for one.

use super::Scratch;
use super::air::{self, Hazard};
use super::{MISSION_CAP, Mission, Missions, Task, approach, mine, run};
use crate::decision::Ledger;
use crate::frame::{HomeFrame, doubled};
use crate::map::MapModel;
use crate::memory::Memory;
use chassis::grid::TilePos;
use oxide_sim::observation::{ObservationData, UnitObs};
use oxide_sim::stats::{Domain, Role};
use oxide_sim::{BuildingKind, Command, PlayerId, UnitKind};

/// Ticks a point may go unseen before it is worth a look.
const STALE_TICKS: u64 = 1_800;

/// Unseen ticks counted at most, so a point's value outweighs its age.
const AGE_CAP: u64 = 7_200;

/// Ticks a scout may take to see its point.
pub(super) const TRAVEL_TICKS: u64 = 1_800;

/// A place worth seeing, as a Foundry footprint.
pub(crate) struct Point {
    anchor: TilePos,
    value: u64,
}

/// The seat's scouting points: hostile starts, then expansion sites by
/// their best Foundry anchor. Starts are worth most, then sites nearer an
/// enemy than home.
pub(crate) fn points(map: &MapModel, me: PlayerId) -> Vec<Point> {
    let footprint =
        |anchor: TilePos| (0..2).flat_map(move |dy| (0..2).map(move |dx| anchor.offset(dx, dy)));
    let starts = map
        .hostiles(me)
        .filter_map(|owner| map.start(owner))
        .map(|anchor| Point { anchor, value: 3 });
    let sites = map
        .sites()
        .iter()
        .filter_map(|site| site.anchors.first().copied())
        .map(|anchor| {
            let hostile = footprint(anchor)
                .map(|tile| map.hostile_distance(me, tile))
                .min();
            let own = footprint(anchor).map(|tile| map.distance(me, tile)).min();
            Point {
                anchor,
                value: if hostile < own { 2 } else { 1 },
            }
        });
    starts.chain(sites).collect()
}

impl Missions {
    /// Keeps one scout looking at stale places. Returns whether the seat
    /// wants a scout it does not have, which production then trains.
    pub(crate) fn scout(
        &mut self,
        observation: &ObservationData,
        map: &MapModel,
        frame: HomeFrame,
        memory: &mut Memory,
        scratch: &Scratch,
        ledger: &mut Ledger,
    ) -> bool {
        let now = observation.tick;
        let points = points(map, observation.me);
        let hazards = &scratch.air;
        let scouted = memory.scouted(points.len());
        for (point, seen) in points.iter().zip(scouted.iter_mut()) {
            let (width, height) = BuildingKind::Foundry.base_stats().size;
            let visible = (0..height)
                .any(|dy| (0..width).any(|dx| observation.visible(point.anchor.offset(dx, dy))));
            if visible {
                *seen = now;
            }
        }

        let index = self
            .list
            .iter()
            .position(|mission| matches!(mission.task, Task::Scout { .. }));
        if let Some(index) = index {
            let mission = &self.list[index];
            let Task::Scout { point } = mission.task else {
                unreachable!("the scout mission's kind");
            };
            let point = usize::from(point);
            let arrived = scouted[point] == now;
            let late = now >= mission.since + TRAVEL_TICKS;
            if !(arrived || late) {
                return false;
            }
            scouted[point] = now;
            let Some(scout) = mine(observation, mission.units[0]) else {
                return false;
            };
            match best(observation, map, frame, &points, scouted, scout) {
                Some((next, goal)) => {
                    if send(observation, frame, hazards, scout, goal, ledger) {
                        let mission = &mut self.list[index];
                        mission.task = Task::Scout { point: next };
                        mission.since = now;
                        mission.goal = goal;
                    }
                }
                None => {
                    self.list.remove(index);
                }
            }
            return false;
        }

        let stale = scouted.iter().any(|seen| now - seen >= STALE_TICKS);
        if !stale || self.list.len() >= MISSION_CAP {
            return false;
        }
        let home = map
            .start(observation.me)
            .and_then(|start| map.component(start));
        let mut scouts: Vec<&UnitObs> = self
            .available(observation, false)
            .into_iter()
            .filter_map(|id| mine(observation, id))
            .filter(|unit| {
                unit.kind.role() == Role::Scout
                    || (unit.kind == UnitKind::Scuttler && map.component(unit.tile) == home)
            })
            .collect();
        scouts.sort_by_key(|unit| {
            (
                unit.kind.role() != Role::Scout,
                frame.rank(frame.home, doubled(unit.tile)),
                unit.id,
            )
        });
        let chosen = scouts.into_iter().find_map(|scout| {
            best(observation, map, frame, &points, scouted, scout).map(|best| (scout, best))
        });
        if let Some((scout, (point, goal))) = chosen {
            if send(observation, frame, hazards, scout, goal, ledger) {
                self.list.push(Mission {
                    id: self.next,
                    since: now,
                    units: vec![scout.id],
                    goal,
                    task: Task::Scout { point },
                });
                self.next += 1;
            }
            return false;
        }
        true
    }
}

/// Sends `scout` to `goal`, an aircraft around known anti-air when the
/// straight line crosses it. Returns whether the orders fit this decision.
fn send(
    observation: &ObservationData,
    frame: HomeFrame,
    hazards: &[Hazard],
    scout: &UnitObs,
    goal: TilePos,
    ledger: &mut Ledger,
) -> bool {
    let flies = scout.kind.stats().domain == Domain::Air;
    let Some(via) = flies
        .then(|| air::route(observation, frame, hazards, scout.tile, goal))
        .flatten()
    else {
        return ledger.order(run(vec![scout.id], goal));
    };
    if ledger.room() < 2 {
        return false;
    }
    ledger.order(run(vec![scout.id], via));
    ledger.order(Command::Run {
        units: vec![scout.id],
        goal,
        queue: true,
    })
}

/// The stale point `scout` should look at next and where to send it, with
/// the point's index.
fn best(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    points: &[Point],
    scouted: &[u64],
    scout: &UnitObs,
) -> Option<(u16, TilePos)> {
    let now = observation.tick;
    let flies = scout.kind.stats().domain == Domain::Air;
    points
        .iter()
        .zip(scouted)
        .enumerate()
        .filter(|(_, (_, seen))| now - **seen >= STALE_TICKS)
        .filter_map(|(index, (point, seen))| {
            let goal = if flies {
                let (width, height) = BuildingKind::Foundry.base_stats().size;
                (0..height)
                    .flat_map(|dy| (0..width).map(move |dx| point.anchor.offset(dx, dy)))
                    .min_by_key(|tile| frame.rank(frame.home, doubled(*tile)))
            } else {
                approach(
                    map,
                    observation.me,
                    frame,
                    BuildingKind::Foundry,
                    point.anchor,
                )
            }?;
            let score = (now - seen).min(AGE_CAP) * point.value;
            Some((index, goal, score))
        })
        .max_by_key(|(_, goal, score)| {
            (
                *score,
                std::cmp::Reverse(frame.rank(frame.home, doubled(*goal))),
            )
        })
        .and_then(|(index, goal, _)| Some((u16::try_from(index).ok()?, goal)))
}
