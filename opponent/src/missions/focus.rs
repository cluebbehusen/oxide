//! Focus fire: an engaged mission's members that can all already reach one
//! enemy shoot one such enemy together, the one each rung judges weakest.
//! Nobody chases, so focusing never pulls a member out of position or sends
//! it after an enemy it cannot walk to. A defense sent at artillery keeps
//! after it.

use super::{Missions, Task, hunt, mine, walking_gun};
use crate::decision::Ledger;
use crate::frame::{HomeFrame, doubled};
use crate::map::MapModel;
use chassis::fx::Fx;
use chassis::path::line_blocked;
use oxide_sim::observation::{ObservationData, UnitObs};
use oxide_sim::scenario::BotDifficulty;
use oxide_sim::stats::Domain;
use oxide_sim::{AttackTarget, Command, UnitId};

/// Tiles from a member inside which an enemy can be focused.
const FOCUS_TILES: i32 = 6;

impl Missions {
    /// Points every engaged mission at the reachable enemy it judges weakest,
    /// keeping a focus while it lasts and ordering only when it changes.
    pub(crate) fn focus(
        &mut self,
        observation: &ObservationData,
        map: &MapModel,
        frame: HomeFrame,
        difficulty: BotDifficulty,
        ledger: &mut Ledger,
    ) {
        for mission in &mut self.list {
            let members: Vec<&UnitObs> = mission
                .units
                .iter()
                .filter_map(|id| mine(observation, *id))
                .collect();
            let defending = matches!(mission.task, Task::Defend { .. });
            let Some(focus) = mission.task.focus() else {
                continue;
            };
            let legal = |enemy: &UnitObs| !shooters(map, &members, enemy).is_empty();
            // A defense sent at artillery closes on it before it can shoot.
            let kept = focus.and_then(|id| {
                observation
                    .enemy_units
                    .iter()
                    .find(|enemy| enemy.id == id)
                    .filter(|enemy| legal(enemy) || (defending && walking_gun(enemy)))
            });
            if kept.is_some() {
                continue;
            }
            let goal = doubled(mission.goal);
            let next = observation
                .enemy_units
                .iter()
                .filter(|enemy| threatens(enemy, &members) && legal(enemy))
                .min_by_key(|enemy| {
                    (
                        health(difficulty, enemy),
                        frame.rank(goal, doubled(enemy.tile)),
                        enemy.id,
                    )
                });
            match next {
                Some(enemy) => {
                    let attack = Command::Attack {
                        units: shooters(map, &members, enemy),
                        target: AttackTarget::Unit(enemy.id),
                        queue: false,
                    };
                    if ledger.order(attack) {
                        *focus = Some(enemy.id);
                    }
                }
                None if focus.is_some()
                    && ledger.order(hunt(mission.units.clone(), mission.goal)) =>
                {
                    *focus = None;
                }
                None => {}
            }
        }
    }
}

/// How wounded `enemy` looks when choosing a focus, lowest first: a
/// difficulty limit on judging targets. The upper rungs read its exact
/// health, Standard only the quarter of its health bar it shows, and
/// Scrapheap ignores wounds and takes the enemy nearest the mission's goal.
fn health(difficulty: BotDifficulty, enemy: &UnitObs) -> u32 {
    match difficulty {
        BotDifficulty::Scrapheap => 0,
        BotDifficulty::Standard => (4 * enemy.hp).div_ceil(enemy.kind.stats().max_hp.max(1)),
        BotDifficulty::Veteran | BotDifficulty::Prime => enemy.hp,
    }
}

/// Whether `enemy` stands near a member and can hit one.
fn threatens(enemy: &UnitObs, members: &[&UnitObs]) -> bool {
    let stats = enemy.kind.stats();
    members.iter().any(|unit| {
        let domain = unit.body_domain();
        unit.tile.chebyshev(enemy.tile) <= FOCUS_TILES
            && (stats
                .weapons
                .iter()
                .any(|weapon| weapon.targets.covers(domain))
                || (stats.demolition.is_some() && domain == Domain::Ground))
    })
}

/// The members that can hit `enemy`, by id, if every one of them armed
/// against it already reaches it in a straight line between tile centres
/// that terrain does not stop, as the simulation measures a shot, and every
/// ground member stands on the enemy's ground, so one a little short steps
/// closer rather than seeking a way round; otherwise none.
fn shooters(map: &MapModel, members: &[&UnitObs], enemy: &UnitObs) -> Vec<UnitId> {
    let domain = enemy.body_domain();
    let mut shooters = Vec::new();
    for unit in members {
        let walks = unit.kind.stats().domain == Domain::Ground;
        let ground = map.component(unit.tile);
        if walks && (ground.is_none() || map.component(enemy.tile) != ground) {
            return Vec::new();
        }
        let dx = Fx::from_num(unit.tile.x - enemy.tile.x);
        let dy = Fx::from_num(unit.tile.y - enemy.tile.y);
        let distance_sq = dx * dx + dy * dy;
        let mut weapons = unit
            .kind
            .stats()
            .weapons
            .iter()
            .filter(|weapon| weapon.targets.covers(domain))
            .peekable();
        if weapons.peek().is_none() {
            continue;
        }
        let reaches = weapons.any(|weapon| {
            let direct = !weapon.indirect && walks && domain == Domain::Ground;
            distance_sq <= weapon.range * weapon.range
                && distance_sq >= weapon.minimum_range * weapon.minimum_range
                && !line_blocked(unit.tile.center(), enemy.tile.center(), |tile| {
                    map.shot_crosses(tile, direct)
                })
        });
        if !reaches {
            return Vec::new();
        }
        shooters.push(unit.id);
    }
    shooters
}

#[cfg(test)]
mod tests;
