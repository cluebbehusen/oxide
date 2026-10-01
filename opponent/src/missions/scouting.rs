//! Scouting: a scout looks at each place the seat has not seen for a while,
//! the most valuable first, hostile starts first. For places no scout can
//! take it asks production for more.

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
use std::cell::OnceCell;

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
    /// Keeps a scout looking at each stale place, no two at one place.
    /// Returns how many stale places no scout holds or could take, which
    /// production trains scouts for.
    pub(crate) fn scout(
        &mut self,
        observation: &ObservationData,
        map: &MapModel,
        frame: HomeFrame,
        memory: &mut Memory,
        scratch: &Scratch,
        ledger: &mut Ledger,
    ) -> usize {
        let now = observation.tick;
        let points = points(map, observation.me);
        let goals = Goals {
            observation,
            map,
            frame,
            points: &points,
            ground: OnceCell::new(),
            air: OnceCell::new(),
        };
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

        let scouting: fn(&Task) -> bool = |task| matches!(task, Task::Scout { .. });
        let held = |missions: &Self, except: Option<u64>| -> Vec<u16> {
            missions
                .list
                .iter()
                .filter(|mission| Some(mission.id) != except)
                .filter_map(|mission| match mission.task {
                    Task::Scout { point } => Some(point),
                    _ => None,
                })
                .collect()
        };
        for id in self.ids(scouting) {
            let Some(index) = self.index_of(id) else {
                continue;
            };
            let mission = &self.list[index];
            let Task::Scout { point } = mission.task else {
                unreachable!("the scout mission's kind");
            };
            let point = usize::from(point);
            let arrived = scouted[point] == now;
            let late = now >= mission.since + TRAVEL_TICKS;
            if !(arrived || late) {
                continue;
            }
            scouted[point] = now;
            let Some(scout) = mine(observation, mission.units[0]) else {
                continue;
            };
            let others = held(self, Some(id));
            match best(now, frame, &points, scouted, &others, goals.of(scout)) {
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
        while self.list.len() < MISSION_CAP {
            let taken = held(self, None);
            let chosen = scouts.iter().enumerate().find_map(|(index, scout)| {
                best(now, frame, &points, scouted, &taken, goals.of(scout))
                    .map(|best| (index, best))
            });
            let Some((index, (point, goal))) = chosen else {
                break;
            };
            let scout = scouts.remove(index);
            if !send(observation, frame, hazards, scout, goal, ledger) {
                break;
            }
            self.list.push(Mission {
                id: self.next,
                since: now,
                units: vec![scout.id],
                goal,
                task: Task::Scout { point },
            });
            self.next += 1;
        }
        let taken = held(self, None);
        scouted
            .iter()
            .enumerate()
            .filter(|(index, seen)| {
                now - **seen >= STALE_TICKS
                    && u16::try_from(*index).is_ok_and(|point| !taken.contains(&point))
            })
            .count()
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

/// Where a scout goes to see each point, on foot or in the air. A goal does
/// not depend on which scout goes, so each is worked out once per decision,
/// when first wanted.
struct Goals<'a> {
    observation: &'a ObservationData,
    map: &'a MapModel,
    frame: HomeFrame,
    points: &'a [Point],
    ground: OnceCell<Vec<Option<TilePos>>>,
    air: OnceCell<Vec<Option<TilePos>>>,
}

impl Goals<'_> {
    /// Each point's goal for `scout`, `None` where it cannot go.
    fn of(&self, scout: &UnitObs) -> &[Option<TilePos>] {
        let frame = self.frame;
        if scout.kind.stats().domain == Domain::Air {
            self.air.get_or_init(|| {
                let (width, height) = BuildingKind::Foundry.base_stats().size;
                self.points
                    .iter()
                    .map(|point| {
                        (0..height)
                            .flat_map(|dy| (0..width).map(move |dx| point.anchor.offset(dx, dy)))
                            .min_by_key(|tile| frame.rank(frame.home, doubled(*tile)))
                    })
                    .collect()
            })
        } else {
            self.ground.get_or_init(|| {
                self.points
                    .iter()
                    .map(|point| {
                        approach(
                            self.map,
                            self.observation.me,
                            frame,
                            BuildingKind::Foundry,
                            point.anchor,
                        )
                    })
                    .collect()
            })
        }
    }
}

/// The stale point no other scout holds that a scout with `goals` should look
/// at next and where to send it, with the point's index.
fn best(
    now: u64,
    frame: HomeFrame,
    points: &[Point],
    scouted: &[u64],
    held: &[u16],
    goals: &[Option<TilePos>],
) -> Option<(u16, TilePos)> {
    points
        .iter()
        .zip(scouted)
        .zip(goals)
        .enumerate()
        .filter(|(_, ((_, seen), _))| now - **seen >= STALE_TICKS)
        .filter(|(index, _)| u16::try_from(*index).is_ok_and(|point| !held.contains(&point)))
        .filter_map(|(index, ((point, seen), goal))| {
            let score = (now - seen).min(AGE_CAP) * point.value;
            Some((index, (*goal)?, score))
        })
        .max_by_key(|(_, goal, score)| {
            (
                *score,
                std::cmp::Reverse(frame.rank(frame.home, doubled(*goal))),
            )
        })
        .and_then(|(index, goal, _)| Some((u16::try_from(index).ok()?, goal)))
}
