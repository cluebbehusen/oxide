//! Static-defense role contracts: what each emplacement stops, what
//! bypasses it, and which siege rung answers it.

mod common;

use chassis::grid::TilePos;
use oxide_sim::scenario::BuildingSpec;
use oxide_sim::{BuildingKind, Command, Event, PlayerId, Scenario, Target, UnitId, UnitKind};

use common::{cmd, open_arena, unit};

const WIDTH: i32 = 48;
const HEIGHT: i32 = 20;

fn mirrored_tile(tile: TilePos) -> TilePos {
    TilePos::new(WIDTH - 1 - tile.x, HEIGHT - 1 - tile.y)
}

fn mirrored_anchor(anchor: TilePos, kind: BuildingKind) -> TilePos {
    let (width, height) = kind.stats().size;
    TilePos::new(WIDTH - width - anchor.x, HEIGHT - height - anchor.y)
}

fn role_scenario(
    defender: u8,
    buildings: &[(BuildingKind, TilePos)],
    attackers: &[(UnitKind, TilePos)],
    spotters: &[(u8, UnitKind, TilePos)],
) -> Scenario {
    let attacker = 1 - defender;
    let buildings = buildings
        .iter()
        .map(|(kind, anchor)| {
            let anchor = if defender == 0 {
                *anchor
            } else {
                mirrored_anchor(*anchor, *kind)
            };
            BuildingSpec {
                player: defender,
                kind: *kind,
                x: anchor.x,
                y: anchor.y,
            }
        })
        .collect();
    let mut units: Vec<_> = attackers
        .iter()
        .map(|(kind, tile)| {
            let tile = if defender == 0 {
                *tile
            } else {
                mirrored_tile(*tile)
            };
            unit(attacker, *kind, tile.x, tile.y)
        })
        .collect();
    units.extend(spotters.iter().map(|(owner, kind, tile)| {
        let tile = if defender == 0 {
            *tile
        } else {
            mirrored_tile(*tile)
        };
        unit(*owner, *kind, tile.x, tile.y)
    }));
    let mut scenario = open_arena(WIDTH as usize, HEIGHT as usize, units);
    scenario.buildings = buildings;
    scenario
}

fn attack_building(
    state: &mut oxide_sim::State,
    attacker: u8,
    units: &[UnitId],
    building: oxide_sim::BuildingId,
) -> Vec<Event> {
    let mut events = state
        .tick(&[cmd(
            attacker,
            Command::Attack {
                units: units.to_vec(),
                target: Target::Building(building),
                queue: false,
            },
        )])
        .events;
    for _ in 0..3_000u32 {
        if state.building(building).is_none() {
            break;
        }
        events.extend(state.tick(&[]).events);
    }
    events
}

#[test]
fn defense_stats_name_three_distinct_jobs() {
    let turret = BuildingKind::Turret.stats();
    assert_eq!(turret.max_hp, 350);
    assert_eq!(turret.vision, 6);
    assert_eq!(turret.weapons[0].damage, 12);
    assert_eq!(turret.weapons[0].range, chassis::fx::Fx::lit("5"));
    assert_eq!(turret.weapons[0].minimum_range, chassis::fx::Fx::ZERO);
    assert_eq!(turret.weapons[0].cooldown_ticks, 25);
    assert_eq!(turret.construction.unwrap().cost, 100);
    assert_eq!(turret.construction.unwrap().build_ticks, 300);

    let flak = BuildingKind::FlakTurret.stats();
    assert_eq!(flak.max_hp, 300);
    assert_eq!(flak.vision, 7);
    assert_eq!(flak.weapons[0].damage, 7);
    assert_eq!(flak.weapons[0].range, chassis::fx::Fx::lit("5.5"));
    assert_eq!(flak.weapons[0].minimum_range, chassis::fx::Fx::ZERO);
    assert_eq!(flak.weapons[0].cooldown_ticks, 12);
    assert!(flak.weapons[0].targets.air);
    assert!(!flak.weapons[0].targets.ground);
    assert_eq!(flak.weapons[0].splash, Some(chassis::fx::Fx::lit("1.2")));
    assert_eq!(flak.construction.unwrap().cost, 90);
    assert_eq!(flak.construction.unwrap().build_ticks, 250);

    let bastion = BuildingKind::Bastion.stats();
    assert_eq!(bastion.max_hp, 500);
    assert_eq!(bastion.vision, 6);
    assert_eq!(bastion.weapons[0].damage, 40);
    assert_eq!(bastion.weapons[0].range, chassis::fx::Fx::lit("9.5"));
    assert_eq!(
        bastion.weapons[0].minimum_range,
        chassis::fx::Fx::lit("2.5")
    );
    assert_eq!(bastion.weapons[0].cooldown_ticks, 90);
    assert_eq!(bastion.weapons[0].splash, Some(chassis::fx::Fx::lit("1.3")));
    assert_eq!(bastion.construction.unwrap().cost, 250);
    assert_eq!(bastion.construction.unwrap().build_ticks, 500);
}

#[test]
fn lancer_sieges_a_turret_from_outside_return_range() {
    for defender in [0, 1] {
        let attacker = 1 - defender;
        let mut state = role_scenario(
            defender,
            &[(BuildingKind::Turret, TilePos::new(20, 9))],
            &[(UnitKind::Lancer, TilePos::new(14, 9))],
            &[],
        )
        .build()
        .unwrap();
        let turret = state
            .buildings()
            .iter()
            .find(|building| building.kind == BuildingKind::Turret)
            .unwrap()
            .id;
        let lancer = state
            .units()
            .iter()
            .find(|unit| unit.player == PlayerId(attacker))
            .unwrap()
            .id;
        let events = attack_building(&mut state, attacker, &[lancer], turret);

        assert!(state.building(turret).is_none());
        assert_eq!(
            state.unit(lancer).unwrap().hp,
            UnitKind::Lancer.stats().max_hp
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::TurretFired { turret: fired, .. } if *fired == turret)),
            "the five-tile point defense must not answer the 5.5-tile rail"
        );
    }
}

#[test]
fn a_spotted_bombard_sieges_a_bastion_at_nominal_range_parity() {
    assert_eq!(
        UnitKind::Bombard.stats().weapons[0].range,
        BuildingKind::Bastion.stats().weapons[0].range
    );
    for defender in [0, 1] {
        let attacker = 1 - defender;
        let mut state = role_scenario(
            defender,
            &[(BuildingKind::Bastion, TilePos::new(20, 8))],
            &[(UnitKind::Bombard, TilePos::new(10, 9))],
            &[
                (attacker, UnitKind::Wisp, TilePos::new(16, 9)),
                (defender, UnitKind::Harvester, TilePos::new(14, 9)),
            ],
        )
        .build()
        .unwrap();
        let bastion = state
            .buildings()
            .iter()
            .find(|building| building.kind == BuildingKind::Bastion)
            .unwrap()
            .id;
        let bombard = state
            .units()
            .iter()
            .find(|unit| unit.player == PlayerId(attacker) && unit.kind == UnitKind::Bombard)
            .unwrap()
            .id;
        let events = attack_building(&mut state, attacker, &[bombard], bastion);

        assert!(state.building(bastion).is_none());
        assert_eq!(
            state.unit(bombard).unwrap().hp,
            UnitKind::Bombard.stats().max_hp
        );
        assert!(
            events.iter().any(|event| matches!(
                event,
                Event::ShellLaunched {
                    shooter: Target::Unit(unit),
                    ..
                } if *unit == bombard
            )),
            "the spotter must let the 9.5-tile gun speak"
        );
        assert!(
            !events.iter().any(|event| matches!(
                event,
                Event::ShellLaunched {
                    shooter: Target::Building(building),
                    ..
                } if *building == bastion
            )),
            "the mobile gun reaches the footprint edge before the Bastion's centered gun reaches it"
        );
    }
}

#[test]
fn close_pressure_breaches_an_isolated_bastion_dead_zone() {
    for defender in [0, 1] {
        let attacker = 1 - defender;
        let mut state = role_scenario(
            defender,
            &[(BuildingKind::Bastion, TilePos::new(20, 8))],
            &[(UnitKind::Scuttler, TilePos::new(14, 9))],
            &[],
        )
        .build()
        .unwrap();
        let bastion = state
            .buildings()
            .iter()
            .find(|building| building.kind == BuildingKind::Bastion)
            .unwrap()
            .id;
        let scuttler = state
            .units()
            .iter()
            .find(|unit| unit.player == PlayerId(attacker))
            .unwrap()
            .id;
        let events = attack_building(&mut state, attacker, &[scuttler], bastion);

        assert!(
            state.building(bastion).is_none(),
            "a fast close-assault unit must breach an isolated Bastion from seat {defender}"
        );
        assert_eq!(
            state.unit(scuttler).unwrap().hp,
            UnitKind::Scuttler.stats().max_hp,
            "the unguided opening shell should miss before the attacker enters the dead zone"
        );
        let shots: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                Event::ShellLaunched {
                    shooter: Target::Building(building),
                    from,
                    to,
                    ..
                } if *building == bastion => Some((*from, *to)),
                _ => None,
            })
            .collect();
        assert!(
            !shots.is_empty(),
            "the breach must cross the outer firing envelope before reaching safety"
        );
        let minimum = BuildingKind::Bastion.stats().weapons[0].minimum_range;
        assert!(
            shots
                .iter()
                .all(|(from, to)| from.dist_sq(*to) >= minimum * minimum),
            "the Bastion must stop firing after pressure reaches its dead zone"
        );
    }
}

#[test]
fn ground_attack_aircraft_bypass_ground_only_defenses() {
    for defender in [0, 1] {
        let attacker = 1 - defender;
        for kind in [BuildingKind::Turret, BuildingKind::Bastion] {
            let anchor = if kind == BuildingKind::Turret {
                TilePos::new(20, 9)
            } else {
                TilePos::new(20, 8)
            };
            let mut state = role_scenario(
                defender,
                &[(kind, anchor)],
                &[(UnitKind::Buzzard, TilePos::new(16, 9))],
                &[],
            )
            .build()
            .unwrap();
            let defense = state
                .buildings()
                .iter()
                .find(|building| building.kind == kind)
                .unwrap()
                .id;
            let flyer = state
                .units()
                .iter()
                .find(|unit| unit.player == PlayerId(attacker))
                .unwrap()
                .id;
            let events = attack_building(&mut state, attacker, &[flyer], defense);

            assert!(
                state.building(defense).is_none(),
                "{kind:?} blocks ground only"
            );
            assert_eq!(
                state.unit(flyer).unwrap().hp,
                UnitKind::Buzzard.stats().max_hp
            );
            assert!(!events.iter().any(|event| matches!(
                event,
                Event::TurretFired { turret, .. }
                    | Event::ShellLaunched {
                        shooter: Target::Building(turret),
                        ..
                    } if *turret == defense
            )));
        }
    }
}

#[test]
fn ground_units_bypass_air_only_flak() {
    for defender in [0, 1] {
        let attacker = 1 - defender;
        let mut state = role_scenario(
            defender,
            &[(BuildingKind::FlakTurret, TilePos::new(20, 9))],
            &[(UnitKind::Scuttler, TilePos::new(18, 9))],
            &[],
        )
        .build()
        .unwrap();
        let flak = state
            .buildings()
            .iter()
            .find(|building| building.kind == BuildingKind::FlakTurret)
            .unwrap()
            .id;
        let scuttler = state
            .units()
            .iter()
            .find(|unit| unit.player == PlayerId(attacker))
            .unwrap()
            .id;
        let events = attack_building(&mut state, attacker, &[scuttler], flak);

        assert!(state.building(flak).is_none());
        assert_eq!(
            state.unit(scuttler).unwrap().hp,
            UnitKind::Scuttler.stats().max_hp
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::TurretFired { turret, .. } if *turret == flak))
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchOutcome {
    DefenseWin,
    AttackWin,
    Censored,
}

fn cost_par_outcome(
    defender: u8,
    kind: BuildingKind,
    anchors: &[TilePos],
    attackers: &[(UnitKind, TilePos)],
) -> MatchOutcome {
    let attacker = 1 - defender;
    let buildings: Vec<_> = anchors.iter().map(|anchor| (kind, *anchor)).collect();
    let mut state = role_scenario(defender, &buildings, attackers, &[])
        .build()
        .unwrap();
    let defenders: Vec<_> = state
        .buildings()
        .iter()
        .filter(|building| building.kind == kind)
        .map(|building| building.id)
        .collect();
    let attackers: Vec<_> = state
        .units()
        .iter()
        .filter(|unit| unit.player == PlayerId(attacker))
        .map(|unit| unit.id)
        .collect();
    let goal = if defender == 0 {
        anchors[anchors.len() / 2]
    } else {
        mirrored_anchor(anchors[anchors.len() / 2], kind)
    };
    state.tick(&[cmd(
        attacker,
        Command::AttackMove {
            units: attackers.clone(),
            goal,
            queue: false,
        },
    )]);

    let mut last_total = u64::MAX;
    let mut last_progress = 0u32;
    for tick in 0..3_000u32 {
        let attackers_alive = attackers.iter().filter_map(|id| state.unit(*id)).count();
        let defenders_alive = defenders
            .iter()
            .filter_map(|id| state.building(*id))
            .count();
        if attackers_alive == 0 {
            return MatchOutcome::DefenseWin;
        }
        if defenders_alive == 0 {
            return MatchOutcome::AttackWin;
        }
        let total = attackers
            .iter()
            .filter_map(|id| state.unit(*id))
            .map(|unit| u64::from(unit.hp))
            .chain(
                defenders
                    .iter()
                    .filter_map(|id| state.building(*id))
                    .map(|building| u64::from(building.hp)),
            )
            .sum();
        if total != last_total {
            last_total = total;
            last_progress = tick;
        } else if tick.saturating_sub(last_progress) >= 500 {
            return MatchOutcome::Censored;
        }
        state.tick(&[]);
    }
    MatchOutcome::Censored
}

#[test]
fn cost_par_defenses_hold_their_target_domain() {
    let turret_anchors = [
        TilePos::new(20, 6),
        TilePos::new(20, 9),
        TilePos::new(20, 12),
    ];
    let sentinel_attack = [
        (UnitKind::Sentinel, TilePos::new(13, 6)),
        (UnitKind::Sentinel, TilePos::new(13, 9)),
        (UnitKind::Sentinel, TilePos::new(13, 12)),
    ];
    let flak_anchors = turret_anchors;
    let buzzard_attack = [
        (UnitKind::Buzzard, TilePos::new(13, 7)),
        (UnitKind::Buzzard, TilePos::new(13, 11)),
    ];
    let darter_attack = [
        (UnitKind::Darter, TilePos::new(13, 6)),
        (UnitKind::Darter, TilePos::new(13, 9)),
        (UnitKind::Darter, TilePos::new(13, 12)),
    ];
    let bastion_anchors = [TilePos::new(20, 6), TilePos::new(20, 11)];
    let bastion_attack = [
        (UnitKind::Sentinel, TilePos::new(12, 5)),
        (UnitKind::Sentinel, TilePos::new(12, 7)),
        (UnitKind::Sentinel, TilePos::new(12, 9)),
        (UnitKind::Sentinel, TilePos::new(12, 11)),
        (UnitKind::Sentinel, TilePos::new(12, 13)),
        (UnitKind::Sentinel, TilePos::new(12, 15)),
    ];

    for defender in [0, 1] {
        for (kind, anchors, attackers) in [
            (
                BuildingKind::Turret,
                turret_anchors.as_slice(),
                sentinel_attack.as_slice(),
            ),
            (
                BuildingKind::FlakTurret,
                flak_anchors.as_slice(),
                buzzard_attack.as_slice(),
            ),
            (
                BuildingKind::FlakTurret,
                flak_anchors.as_slice(),
                darter_attack.as_slice(),
            ),
            (
                BuildingKind::Bastion,
                bastion_anchors.as_slice(),
                bastion_attack.as_slice(),
            ),
        ] {
            assert_eq!(
                cost_par_outcome(defender, kind, anchors, attackers),
                MatchOutcome::DefenseWin,
                "{kind:?} must decisively hold its cost-par role from seat {defender}"
            );
        }
    }
}

#[test]
fn a_moving_advance_can_dodge_an_unguided_bastion_shell() {
    for defender in [0, 1] {
        let attacker = 1 - defender;
        let mut state = role_scenario(
            defender,
            &[(BuildingKind::Bastion, TilePos::new(20, 8))],
            &[(UnitKind::Scuttler, TilePos::new(14, 9))],
            &[],
        )
        .build()
        .unwrap();
        let scuttler = state
            .units()
            .iter()
            .find(|unit| unit.player == PlayerId(attacker))
            .unwrap()
            .id;
        let goal = if defender == 0 {
            TilePos::new(14, 15)
        } else {
            mirrored_tile(TilePos::new(14, 15))
        };
        let first = state.tick(&[cmd(
            attacker,
            Command::Advance {
                units: vec![scuttler],
                goal,
                queue: false,
            },
        )]);

        let mut launched = first.events.iter().any(|event| {
            matches!(
                event,
                Event::ShellLaunched {
                    shooter: Target::Building(_),
                    ..
                }
            )
        });
        let mut landed = first
            .events
            .iter()
            .any(|event| matches!(event, Event::ShellLanded { .. }));
        for _ in 0..300u32 {
            let report = state.tick(&[]);
            launched |= report.events.iter().any(|event| {
                matches!(
                    event,
                    Event::ShellLaunched {
                        shooter: Target::Building(_),
                        ..
                    }
                )
            });
            landed |= report
                .events
                .iter()
                .any(|event| matches!(event, Event::ShellLanded { .. }));
            if landed {
                break;
            }
        }
        assert!(launched && landed, "the dodge requires a resolved shot");
        assert_eq!(
            state.unit(scuttler).unwrap().hp,
            UnitKind::Scuttler.stats().max_hp,
            "the projectile keeps its fire-time aim instead of guiding onto the mover"
        );
    }
}
