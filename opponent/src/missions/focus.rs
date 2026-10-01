//! Focus fire at Veteran and Prime: an engaged mission's members that can
//! all already reach one enemy shoot the weakest such enemy together. Nobody
//! chases, so focusing never pulls a member out of position or sends it after
//! an enemy it cannot walk to.

use super::{Missions, hunt, mine};
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
    /// Points every engaged mission at its weakest reachable enemy, keeping a
    /// focus while it lasts and ordering only when it changes.
    pub(crate) fn focus(
        &mut self,
        observation: &ObservationData,
        map: &MapModel,
        frame: HomeFrame,
        difficulty: BotDifficulty,
        ledger: &mut Ledger,
    ) {
        if !matches!(difficulty, BotDifficulty::Veteran | BotDifficulty::Prime) {
            return;
        }
        for mission in &mut self.list {
            let members: Vec<&UnitObs> = mission
                .units
                .iter()
                .filter_map(|id| mine(observation, *id))
                .collect();
            let Some(focus) = mission.task.focus() else {
                continue;
            };
            let legal = |enemy: &UnitObs| !shooters(map, &members, enemy).is_empty();
            let kept = focus.and_then(|id| {
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
/// reaches it in a straight line between tile centres that terrain does not
/// stop, as the simulation measures a shot, and every ground member stands on
/// the enemy's ground, so one a little short steps closer rather than seeking
/// a way round; otherwise none.
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
mod tests {
    use super::*;
    use oxide_sim::scenario::{PlayerSpec, UnitSpec};
    use oxide_sim::{Faction, PlayerId, Scenario, UnitKind};

    /// A 20 by 9 field, starts at (2, 1) and (17, 1), with a one-tile chasm
    /// down column 8 when `chasm` holds; West's `members` and one East
    /// `enemy`, each as kind and tile.
    fn field(
        chasm: bool,
        members: &[(UnitKind, i32, i32)],
        enemy: (UnitKind, i32, i32),
    ) -> Scenario {
        let map: Vec<String> = (0..9)
            .map(|row| {
                let mut tiles: Vec<char> = ".".repeat(20).chars().collect();
                if chasm {
                    tiles[8] = '~';
                }
                if row == 1 {
                    tiles[2] = '1';
                    tiles[17] = '2';
                }
                tiles.into_iter().collect()
            })
            .collect();
        let units = members
            .iter()
            .map(|&(kind, x, y)| UnitSpec {
                player: 0,
                kind,
                x,
                y,
            })
            .chain(std::iter::once(UnitSpec {
                player: 1,
                kind: enemy.0,
                x: enemy.1,
                y: enemy.2,
            }))
            .collect();
        Scenario {
            mode: Default::default(),
            name: "focus".into(),
            seed: 1,
            map,
            players: [Faction::Ferrous, Faction::Cupric]
                .into_iter()
                .map(|faction| PlayerSpec {
                    name: format!("{faction:?}"),
                    faction,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                })
                .collect(),
            units,
            buildings: Vec::new(),
            meta: None,
        }
    }

    /// How many of West's units `shooters` would focus on East's unit.
    fn focused(scenario: &Scenario) -> usize {
        let state = scenario.build().unwrap();
        let map = MapModel::from_scenario(scenario).unwrap();
        let observation = ObservationData::fog_honest(&state, PlayerId(0));
        let members: Vec<&UnitObs> = observation.my_units.iter().collect();
        let east = state
            .units()
            .iter()
            .find(|unit| unit.player == PlayerId(1))
            .unwrap();
        let enemy = UnitObs {
            id: east.id,
            player: east.player,
            kind: east.kind,
            tile: east.tile(),
            ..*members[0]
        };
        shooters(&map, &members, &enemy).len()
    }

    #[test]
    fn a_diagonal_enemy_inside_the_old_square_but_out_of_straight_reach_is_not_focused() {
        let sentinel = |x, y| (UnitKind::Sentinel, x, y);
        let near = field(false, &[sentinel(4, 4)], sentinel(6, 5));
        assert_eq!(focused(&near), 1, "about 2.2 tiles away");
        let diagonal = field(false, &[sentinel(4, 4)], sentinel(7, 7));
        assert_eq!(focused(&diagonal), 0, "3 tiles each way is 4.2 in a line");
    }

    #[test]
    fn ground_members_never_focus_an_enemy_across_ground_they_cannot_walk() {
        let sentinel = |x, y| (UnitKind::Sentinel, x, y);
        assert_eq!(focused(&field(false, &[sentinel(7, 4)], sentinel(9, 4))), 1);
        assert_eq!(
            focused(&field(true, &[sentinel(7, 4)], sentinel(9, 4))),
            0,
            "in range, but across the chasm"
        );
    }

    #[test]
    fn nothing_focuses_an_enemy_behind_terrain_that_stops_its_shot() {
        let behind = |terrain: &str, member: (UnitKind, i32, i32), enemy| {
            let mut scenario = field(false, &[member], enemy);
            scenario.map[4].replace_range(10..11, terrain);
            focused(&scenario)
        };
        let sentinel = (UnitKind::Sentinel, 9, 4);
        let target = (UnitKind::Sentinel, 11, 4);
        assert_eq!(behind(".", sentinel, target), 1, "premise: in reach");
        assert_eq!(behind("#", sentinel, target), 0, "rock, ground round it");
        let gun = (UnitKind::Avalanche, 6, 4);
        let far = (UnitKind::Sentinel, 14, 4);
        assert_eq!(behind("#", gun, far), 1, "shells arc over rock");
        assert_eq!(behind("^", gun, far), 0, "but not over a peak");
    }

    #[test]
    fn a_gun_never_focuses_an_enemy_inside_its_minimum_range() {
        let enemy = (UnitKind::Sentinel, 14, 4);
        let close = field(false, &[(UnitKind::Avalanche, 12, 4)], enemy);
        assert_eq!(focused(&close), 0);
        let clear = field(false, &[(UnitKind::Avalanche, 6, 4)], enemy);
        assert_eq!(focused(&clear), 1);
    }
}
