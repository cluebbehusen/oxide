//! Focus fire at Veteran and Prime: an engaged mission's members that can
//! all already reach one enemy shoot the weakest such enemy together. Nobody
//! chases, so focusing never pulls a member out of position.

use super::{Missions, Phase, hunt, mine};
use crate::decision::Ledger;
use crate::frame::{HomeFrame, doubled};
use oxide_sim::observation::{ObservationData, UnitObs};
use oxide_sim::scenario::BotDifficulty;
use oxide_sim::{AttackTarget, Command, UnitId};

/// Tiles from a member inside which an enemy can be focused.
const FOCUS_TILES: i32 = 6;

impl Missions {
    /// Points every engaged mission at its weakest reachable enemy, keeping a
    /// focus while it lasts and ordering only when it changes.
    pub(crate) fn focus(
        &mut self,
        observation: &ObservationData,
        frame: HomeFrame,
        difficulty: BotDifficulty,
        ledger: &mut Ledger,
    ) {
        for mission in &mut self.list {
            if mission.phase != Phase::Engage {
                mission.focus = None;
            }
        }
        if !matches!(difficulty, BotDifficulty::Veteran | BotDifficulty::Prime) {
            return;
        }
        for mission in self
            .list
            .iter_mut()
            .filter(|mission| mission.phase == Phase::Engage)
        {
            let members: Vec<&UnitObs> = mission
                .units
                .iter()
                .filter_map(|id| mine(observation, *id))
                .collect();
            let legal = |enemy: &UnitObs| !shooters(&members, enemy).is_empty();
            let kept = mission.focus.and_then(|id| {
                observation
                    .enemy_units
                    .iter()
                    .find(|enemy| enemy.id == id)
                    .filter(|enemy| legal(enemy))
            });
            if kept.is_some() {
                continue;
            }
            let goal = doubled(mission.goal);
            let next = observation
                .enemy_units
                .iter()
                .filter(|enemy| threatens(enemy, &members) && legal(enemy))
                .min_by_key(|enemy| (enemy.hp, frame.rank(goal, doubled(enemy.tile)), enemy.id));
            match next {
                Some(enemy) => {
                    let attack = Command::Attack {
                        units: shooters(&members, enemy),
                        target: AttackTarget::Unit(enemy.id),
                        queue: false,
                    };
                    if ledger.order(attack) {
                        mission.focus = Some(enemy.id);
                    }
                }
                None if mission.focus.is_some()
                    && ledger.order(hunt(mission.units.clone(), mission.goal)) =>
                {
                    mission.focus = None;
                }
                None => {}
            }
        }
    }
}

/// Whether `enemy` stands near a member and can hit one.
fn threatens(enemy: &UnitObs, members: &[&UnitObs]) -> bool {
    members.iter().any(|unit| {
        unit.tile.chebyshev(enemy.tile) <= FOCUS_TILES
            && enemy
                .kind
                .stats()
                .weapons
                .iter()
                .any(|weapon| weapon.targets.covers(unit.body_domain()))
    })
}

/// The members that can hit `enemy`, by id, if every one of them already
/// reaches it; otherwise none.
fn shooters(members: &[&UnitObs], enemy: &UnitObs) -> Vec<UnitId> {
    let domain = enemy.body_domain();
    let mut shooters = Vec::new();
    for unit in members {
        let reach = unit
            .kind
            .stats()
            .weapons
            .iter()
            .filter(|weapon| weapon.targets.covers(domain))
            .map(|weapon| weapon.range.ceil().to_num::<i32>())
            .max();
        let Some(reach) = reach else {
            continue;
        };
        if unit.tile.chebyshev(enemy.tile) > reach {
            return Vec::new();
        }
        shooters.push(unit.id);
    }
    shooters
}
