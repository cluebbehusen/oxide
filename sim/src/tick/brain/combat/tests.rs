use super::super::super::spatial::UnitIndex;
use super::*;
use crate::command::{Command, PlayerCommand};
use crate::scenario::{PlayerSpec, Scenario, ScenarioMode, UnitSpec};
use crate::state::Faction;
use crate::stats::UnitKind;

/// The reference acquisition: a full scan of the unit list. The indexed
/// window prunes candidates and must never change the pick.
fn linear_acquire(state: &State, id: UnitId) -> Option<Target> {
    let unit = state.unit(id).expect("caller checked");
    let stats = unit.kind.stats();
    if !stats.can_fight() {
        return None;
    }
    let (pos, me) = (unit.pos, unit.player);
    let acquisition_range = stats.aggro_range;
    let needs_shared_sight = acquisition_range > Fx::from_num(stats.vision);
    let aggro_sq = acquisition_range * acquisition_range;
    let unit_target = state
        .units
        .iter()
        .filter(|u| {
            state.hostile(me, u.player)
                && u.hp > 0
                && stats.can_target(u.domain())
                && (!needs_shared_sight || state.can_see(me, u.tile()))
        })
        .map(|u| (pos.dist_sq(u.pos), u.id))
        .filter(|(d, uid)| {
            *d <= aggro_sq
                && stats.weapons.iter().any(|weapon| {
                    weapon.targets.covers(
                        state
                            .unit(*uid)
                            .expect("candidate exists")
                            .kind
                            .stats()
                            .domain,
                    ) && *d >= weapon.minimum_range * weapon.minimum_range
                })
        })
        .min();
    if let Some((_, uid)) = unit_target {
        return Some(Target::Unit(uid));
    }
    if !stats.can_target(Domain::Ground) {
        return None;
    }
    state
        .buildings
        .iter()
        .filter(|b| state.hostile(me, b.player) && b.hp > 0)
        .filter(|b| !needs_shared_sight || b.tiles().any(|tile| state.can_see(me, tile)))
        .map(|b| (pos.dist_sq(b.closest_point_to(pos)), b.id))
        .filter(|(d, _)| {
            *d <= aggro_sq
                && stats.weapons.iter().any(|weapon| {
                    weapon.targets.covers(Domain::Ground)
                        && *d >= weapon.minimum_range * weapon.minimum_range
                })
        })
        .min()
        .map(|(_, bid)| Target::Building(bid))
}

fn seat(name: &str, faction: Faction) -> PlayerSpec {
    PlayerSpec {
        name: name.into(),
        faction,
        team: None,
        scrap: 0,
        bot: false,
        bot_config: None,
    }
}

fn boundary_duel() -> State {
    Scenario {
        mode: ScenarioMode::Match,
        name: "boundary-duel".into(),
        seed: 1,
        map: vec![
            "............".into(),
            "............".into(),
            "............".into(),
            "1.........2.".into(),
            "............".into(),
            "............".into(),
            "............".into(),
            "............".into(),
        ],
        players: vec![
            seat("North", Faction::Ferrous),
            seat("South", Faction::Cupric),
        ],
        units: vec![
            UnitSpec {
                player: 0,
                kind: UnitKind::Sentinel,
                x: 5,
                y: 1,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Sentinel,
                x: 6,
                y: 1,
            },
        ],
        buildings: Vec::new(),
        meta: None,
    }
    .build()
    .expect("boundary duel builds")
}

#[test]
fn indexed_acquisition_keeps_targets_beyond_north_and_south_rows() {
    let mut state = boundary_duel();
    let height = state.map.height();
    let attacker = state.units[0].id;
    let victim = state.units[1].id;
    let mut index = UnitIndex::new();

    for (inside, outside) in [
        (TilePos::new(5, 0), TilePos::new(5, -1)),
        (TilePos::new(5, height - 1), TilePos::new(5, height)),
    ] {
        state.units[0].pos = inside.center();
        state.units[1].pos = outside.center();
        state.refresh_vision();
        state
            .validate_invariants()
            .expect("the accepted coordinate envelope includes border rows");
        index.rebuild(&state.units);
        index.survey(&state);

        let indexed = acquire_target(&state, &index, attacker);
        assert_eq!(indexed, Some(Target::Unit(victim)));
        assert_eq!(indexed, linear_acquire(&state, attacker));
    }
}

/// Two mixed armies crossing an open arena, compared pick-for-pick
/// against the linear scan every tick — range edges, weapon-mask
/// misses (flak vs ground, ground-only vs flyers), dying candidates,
/// and the building fallback near the far Foundry all churn as the
/// fronts meet and melt.
#[test]
fn indexed_acquisition_matches_the_linear_scan() {
    let width = 31usize;
    let height = 19usize;
    let mut rows = vec![vec!['#'; width]; height];
    for row in rows.iter_mut().take(height - 1).skip(1) {
        for cell in row.iter_mut().take(width - 1).skip(1) {
            *cell = '.';
        }
    }
    rows[1][1] = '1';
    rows[height - 3][width - 3] = '2';
    let west: &[UnitKind] = &[
        UnitKind::Sentinel,
        UnitKind::Sentinel,
        UnitKind::Scuttler,
        UnitKind::Lancer,
        UnitKind::Flakhound,
        UnitKind::Buzzard,
        UnitKind::Talon,
        UnitKind::Harvester,
    ];
    let east: &[UnitKind] = &[
        UnitKind::Sentinel,
        UnitKind::Sentinel,
        UnitKind::Scuttler,
        UnitKind::Lancer,
        UnitKind::Stinger,
        UnitKind::Darter,
        UnitKind::Wisp,
        UnitKind::Harvester,
    ];
    let mut units = Vec::new();
    for (&kind, i) in west.iter().zip(0_i32..) {
        let (dx, dy) = (i % 4, i / 4);
        units.push(UnitSpec {
            player: 0,
            kind,
            x: 3 + dx * 2,
            y: 4 + dy * 2,
        });
    }
    for (&kind, i) in east.iter().zip(0_i32..) {
        let (dx, dy) = (i % 4, i / 4);
        units.push(UnitSpec {
            player: 1,
            kind,
            x: 27 - dx * 2,
            y: 14 - dy * 2,
        });
    }
    let scenario = Scenario {
        mode: ScenarioMode::Match,
        name: "acquisition-differential".into(),
        seed: 7,
        map: rows.into_iter().map(|r| r.into_iter().collect()).collect(),
        players: vec![
            seat("West", Faction::Ferrous),
            seat("East", Faction::Cupric),
        ],
        units,
        buildings: Vec::new(),
        meta: None,
    };
    let mut state = scenario.build().expect("arena builds");
    let march = |state: &State, player: u8, goal: TilePos| -> PlayerCommand {
        PlayerCommand {
            player: PlayerId(player),
            command: Command::Hunt {
                units: state
                    .units
                    .iter()
                    .filter(|u| u.player == PlayerId(player))
                    .map(|u| u.id)
                    .collect(),
                goal,
                queue: false,
            },
        }
    };
    let opening = [
        march(&state, 0, TilePos::new(27, 15)),
        march(&state, 1, TilePos::new(3, 3)),
    ];
    state.tick(&opening);

    let mut index = UnitIndex::new();
    let mut picks = 0usize;
    for _ in 0..400 {
        state.tick(&[]);
        index.rebuild(&state.units);
        index.survey(&state);
        for unit in &state.units {
            if unit.hp == 0 {
                continue;
            }
            let indexed = acquire_target(&state, &index, unit.id);
            assert_eq!(
                indexed,
                linear_acquire(&state, unit.id),
                "unit {:?} at tick {}",
                unit.id,
                state.tick
            );
            picks += usize::from(indexed.is_some());
        }
    }
    assert!(picks > 100, "the armies never met ({picks} picks)");
}

/// A plain linear scan: the reference the differential below compares
/// `sidearm_victim`'s windowed search against.
fn linear_sidearm_victim(
    state: &State,
    shooter_pos: Vec2Fx,
    me: PlayerId,
    shooter_domain: Domain,
    weapon: &WeaponStats,
) -> Option<(Fx, UnitId, Vec2Fx, Domain)> {
    let shot_open = |t: TilePos, full: bool| {
        state.map.tile(t).is_some_and(|tile| {
            !tile.terrain.blocks_all_fire() && (!full || !tile.terrain.blocks_direct_fire())
        })
    };
    state
        .units
        .iter()
        .filter(|u| state.hostile(me, u.player) && u.hp > 0 && weapon.targets.covers(u.domain()))
        .filter(|u| state.can_see(me, u.tile()))
        .map(|u| (shooter_pos.dist_sq(u.pos), u.id, u.pos, u.domain()))
        .filter(|(d, ..)| within_weapon_reach(weapon, *d))
        .filter(|(_, _, upos, dom)| {
            let full = traces_terrain(weapon, shooter_domain, *dom);
            !chassis::path::line_blocked(shooter_pos, *upos, |t| shot_open(t, full))
        })
        .min_by_key(|&(d, uid, _, _)| (d, uid))
}

/// The same mixed-armies churn as the acquisition differential, but
/// comparing every multi-weapon unit's sidearm pick — window edges,
/// fog, air-vs-ground weapon masks, and dying candidates included.
#[test]
fn windowed_sidearm_victim_matches_the_linear_scan() {
    let width = 31usize;
    let height = 19usize;
    let mut rows = vec![vec!['#'; width]; height];
    for row in rows.iter_mut().take(height - 1).skip(1) {
        for cell in row.iter_mut().take(width - 1).skip(1) {
            *cell = '.';
        }
    }
    rows[1][1] = '1';
    rows[height - 3][width - 3] = '2';
    let west: &[UnitKind] = &[
        UnitKind::Sentinel,
        UnitKind::Sentinel,
        UnitKind::Sentinel,
        UnitKind::Buzzard,
        UnitKind::Talon,
        UnitKind::Flakhound,
    ];
    let east: &[UnitKind] = &[
        UnitKind::Sentinel,
        UnitKind::Sentinel,
        UnitKind::Wisp,
        UnitKind::Wisp,
        UnitKind::Darter,
        UnitKind::Scuttler,
    ];
    let mut units = Vec::new();
    for (&kind, i) in west.iter().zip(0_i32..) {
        let (dx, dy) = (i % 3, i / 3);
        units.push(UnitSpec {
            player: 0,
            kind,
            x: 3 + dx * 2,
            y: 4 + dy * 2,
        });
    }
    for (&kind, i) in east.iter().zip(0_i32..) {
        let (dx, dy) = (i % 3, i / 3);
        units.push(UnitSpec {
            player: 1,
            kind,
            x: 27 - dx * 2,
            y: 14 - dy * 2,
        });
    }
    let scenario = Scenario {
        mode: ScenarioMode::Match,
        name: "sidearm-differential".into(),
        seed: 11,
        map: rows.into_iter().map(|r| r.into_iter().collect()).collect(),
        players: vec![
            seat("West", Faction::Ferrous),
            seat("East", Faction::Cupric),
        ],
        units,
        buildings: Vec::new(),
        meta: None,
    };
    let mut state = scenario.build().expect("arena builds");
    let march = |state: &State, player: u8, goal: TilePos| -> PlayerCommand {
        PlayerCommand {
            player: PlayerId(player),
            command: Command::Hunt {
                units: state
                    .units
                    .iter()
                    .filter(|u| u.player == PlayerId(player))
                    .map(|u| u.id)
                    .collect(),
                goal,
                queue: false,
            },
        }
    };
    let opening = [
        march(&state, 0, TilePos::new(27, 15)),
        march(&state, 1, TilePos::new(3, 3)),
    ];
    state.tick(&opening);

    let mut index = UnitIndex::new();
    let mut picks = 0usize;
    for _ in 0..400 {
        state.tick(&[]);
        index.rebuild(&state.units);
        for unit in &state.units {
            let stats = unit.kind.stats();
            if unit.hp == 0 || stats.weapons.len() < 2 {
                continue;
            }
            for weapon in stats.weapons {
                let windowed =
                    sidearm_victim(&state, &index, unit.pos, unit.player, stats.domain, weapon);
                assert_eq!(
                    windowed,
                    linear_sidearm_victim(&state, unit.pos, unit.player, stats.domain, weapon),
                    "unit {:?} at tick {}",
                    unit.id,
                    state.tick
                );
                picks += usize::from(windowed.is_some());
            }
        }
    }
    assert!(picks > 50, "no sidearm ever found a victim ({picks} picks)");
}

#[test]
fn motion_snapshot_tracks_motor_speed_through_retargeting_and_coasting() {
    let mut state = boundary_duel();
    let target = &mut state.units[1];
    target.kind = UnitKind::Scuttler;
    target.heading = 0;
    target.drive_speed = target.kind.stats().speed / 6;
    target.path = Some(PathFollow {
        final_point: None,
        goal: TilePos::new(3, 1),
        waypoints: vec![TilePos::new(5, 1), TilePos::new(3, 1)],
        next: 0,
    });
    let (id, pos, speed) = (target.id, target.pos, target.drive_speed);
    let expected = pos + Vec2Fx::new(speed, Fx::ZERO);
    assert_eq!(
        MotionSnapshot::capture(&state).position_after(id, pos, 1),
        Some(expected)
    );

    state.units[1].path = None;
    assert_eq!(
        MotionSnapshot::capture(&state).position_after(id, pos, 1),
        Some(expected)
    );
    state.units[1].drive_speed = Fx::ZERO;
    state.units[1].path = Some(PathFollow {
        final_point: None,
        goal: TilePos::new(8, 1),
        waypoints: vec![TilePos::new(8, 1)],
        next: 0,
    });
    assert_eq!(
        MotionSnapshot::capture(&state).position_after(id, pos, 1),
        None
    );
}

#[test]
fn motion_snapshot_ignores_later_route_turns() {
    let aim_with_later_turn = |turn: TilePos| {
        let mut state = boundary_duel();
        state.units[0].kind = UnitKind::Bombard;
        state.units[0].pos = TilePos::new(2, 1).center();
        state.units[1].kind = UnitKind::Scuttler;
        state.units[1].pos = TilePos::new(7, 1).center();
        state.units[1].heading = 0;
        state.units[1].drive_speed = UnitKind::Scuttler.stats().speed;
        let target = state.units[1].id;
        state.units[1].path = Some(PathFollow {
            final_point: None,
            goal: turn,
            waypoints: vec![TilePos::new(8, 1), turn],
            next: 0,
        });
        let motion = MotionSnapshot::capture(&state);
        let shooter = state.units[0].pos;
        let current = state.units[1].pos;
        projectile_aim(
            &state,
            &motion,
            ProjectileShooter {
                owner: PlayerId(0),
                domain: Domain::Ground,
            },
            shooter,
            Target::Unit(target),
            current,
            &UnitKind::Bombard.stats().weapons[0],
        )
    };

    assert_eq!(
        aim_with_later_turn(TilePos::new(8, 6)),
        aim_with_later_turn(TilePos::new(8, -4)),
        "future A* turns are private intent, not observable velocity"
    );
}

#[test]
fn predictive_splash_aim_uses_the_near_edge_of_the_footprint() {
    let current = Vec2Fx::new(Fx::from_num(4), Fx::from_num(3));
    let predicted = Vec2Fx::new(Fx::from_num(9), Fx::from_num(3));
    let radius = Fx::lit("1.4");

    let aim = aim_at_near_splash_edge(current, predicted, radius);

    let expected = Vec2Fx::new(Fx::lit("7.6"), Fx::from_num(3));
    let tolerance = Fx::DELTA * Fx::from_num(8);
    assert!(
        aim.dist(expected) <= tolerance,
        "fixed-point direction scaling stays within eight ulps of the near edge"
    );
    assert!(
        aim.dist(predicted) <= radius && aim.dist(predicted) >= radius - tolerance,
        "a straight mover sits exactly on the predicted blast edge: distance={}, radius={}, tolerance={}",
        aim.dist(predicted),
        radius,
        tolerance
    );
}
