//! Landing and takeoff for turn-limited aircraft: the run-in onto a tile,
//! the parked airframe as a ground body, auto-land, and the ways a landing
//! goes around or lifts off again.

use oxide_sim::command::RejectReason;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::state::Order;
use oxide_sim::stats::{AUTO_LAND_IDLE_TICKS, BuildingKind};
use oxide_sim::{
    Command, Event, Faction, PlayerCommand, PlayerId, Scenario, State, Target, UnitKind,
};

use chassis::fx::{Fx, HALF};
use chassis::grid::TilePos;

fn players(scrap: u32) -> Vec<PlayerSpec> {
    vec![
        PlayerSpec {
            name: "Ferrous".into(),
            faction: Faction::Ferrous,
            team: None,
            scrap,
            bot: false,
            bot_config: None,
        },
        PlayerSpec {
            name: "Cupric".into(),
            faction: Faction::Cupric,
            team: None,
            scrap,
            bot: false,
            bot_config: None,
        },
    ]
}

/// A 24x16 open field with a rock rim: the east wall is at x = 24 and the
/// south wall at y = 16, so wall-side and corner tiles are within reach.
/// Two seats, so hostile units and buildings can be fielded; the enemy
/// Foundry on the west wall is a live target for any run-in that passes it.
fn hostile_arena(units: Vec<UnitSpec>, buildings: Vec<BuildingSpec>) -> Scenario {
    Scenario {
        name: "landing-arena".into(),
        seed: 11,
        // Both Foundries hug the west wall, well clear of the pads around
        // (16, 8) and of the run-in lines a go-around near them flies.
        map: vec![
            "########################".into(),
            "#1.....................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#2.....................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "########################".into(),
        ],
        players: players(1_000),
        units,
        buildings,
        meta: None,
    }
}

/// The same field with a single seat: no enemy anywhere, so a landing or
/// go-around can never turn into a fight.
fn arena(units: Vec<UnitSpec>, buildings: Vec<BuildingSpec>) -> Scenario {
    let mut scenario = hostile_arena(units, buildings);
    scenario.map[12] = "#......................#".into();
    scenario.players.truncate(1);
    scenario
}

fn cmd(player: u8, command: Command) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(player),
        command,
    }
}

fn unit(player: u8, kind: UnitKind, x: i32, y: i32) -> UnitSpec {
    UnitSpec { player, kind, x, y }
}

fn building(player: u8, kind: BuildingKind, x: i32, y: i32) -> BuildingSpec {
    BuildingSpec { player, kind, x, y }
}

fn land(player: u8, unit: oxide_sim::ids::UnitId, x: i32, y: i32) -> PlayerCommand {
    cmd(
        player,
        Command::Move {
            units: vec![unit],
            goal: TilePos::new(x, y),
            queue: false,
        },
    )
}

/// What a watched flight did.
struct Flight {
    /// Longest run of ticks spent motionless while airborne.
    max_still: u32,
    /// Ticks spent pressed against the world envelope.
    pinned: u32,
    /// The tick the airframe touched down, if it did.
    landed_at: Option<u64>,
    /// Hit points on the last airborne tick before touchdown.
    hp_before_touchdown: u32,
    /// Bomb releases.
    releases: Vec<u64>,
}

/// Runs up to `ticks`, asserting every tick that the aircraft stays inside
/// the world and never turns faster than its rate, and stops once it has
/// landed when `until_landed` is set.
fn fly_and_watch(
    state: &mut State,
    condor: oxide_sim::ids::UnitId,
    ticks: u32,
    until_landed: bool,
) -> Flight {
    let width = Fx::from_num(state.map().width());
    let height = Fx::from_num(state.map().height());
    let rate = i16::from(UnitKind::Condor.stats().turn_rate);
    let mut flight = Flight {
        max_still: 0,
        pinned: 0,
        landed_at: None,
        hp_before_touchdown: state.unit(condor).unwrap().hp,
        releases: Vec::new(),
    };
    let mut still = 0u32;
    let mut last_pos = state.unit(condor).unwrap().pos;
    let mut last_heading = state.unit(condor).unwrap().heading;
    for _ in 0..ticks {
        let report = state.tick(&[]);
        if report.events.iter().any(
            |event| matches!(event, Event::ShellLaunched { shooter: Target::Unit(id), .. } if *id == condor),
        ) {
            flight.releases.push(state.current_tick());
        }
        let Some(unit) = state.unit(condor) else {
            break;
        };
        assert!(
            unit.pos.x >= HALF
                && unit.pos.x <= width - HALF
                && unit.pos.y >= HALF
                && unit.pos.y <= height - HALF,
            "the aircraft left the world at {:?} (tick {})",
            unit.pos,
            state.current_tick()
        );
        let delta = i16::from(unit.heading.wrapping_sub(last_heading) as i8).abs();
        assert!(
            delta <= rate,
            "heading jumped {delta} steps in one tick (rate {rate}) at tick {}",
            state.current_tick()
        );
        if unit.landed {
            if flight.landed_at.is_none() {
                flight.landed_at = Some(state.current_tick());
            }
            still = 0;
            if until_landed {
                break;
            }
        } else {
            flight.hp_before_touchdown = unit.hp;
            if unit.pos.x == HALF
                || unit.pos.x == width - HALF
                || unit.pos.y == HALF
                || unit.pos.y == height - HALF
            {
                flight.pinned += 1;
            }
            if unit.pos == last_pos {
                still += 1;
                flight.max_still = flight.max_still.max(still);
            } else {
                still = 0;
            }
        }
        last_pos = unit.pos;
        last_heading = unit.heading;
    }
    flight
}

fn land_at(state: &mut State, condor: oxide_sim::ids::UnitId, x: i32, y: i32) -> Flight {
    state.tick(&[land(0, condor, x, y)]);
    let flight = fly_and_watch(state, condor, 1_500, true);
    assert!(flight.landed_at.is_some(), "the Condor never touched down");
    assert!(
        flight.max_still < 10,
        "the Condor parked in the air for {} ticks",
        flight.max_still
    );
    assert_eq!(
        flight.pinned, 0,
        "the Condor was pressed against the world's edge"
    );
    flight
}

#[test]
fn a_condor_lands_on_the_ordered_tile_via_a_run_in() {
    let mut state = arena(vec![unit(0, UnitKind::Condor, 8, 8)], vec![])
        .build()
        .unwrap();
    let condor = state.units()[0].id;
    state.tick(&[]);
    let flight = land_at(&mut state, condor, 16, 8);
    let parked = state.unit(condor).unwrap();
    assert_eq!(parked.tile(), TilePos::new(16, 8));
    assert!(
        parked.pos.dist(TilePos::new(16, 8).center()) <= oxide_sim::stats::LANDING_TOUCHDOWN,
        "rests where it met the tile, {:?}",
        parked.pos
    );
    assert_eq!(parked.order, Order::Idle);
    assert!(parked.path.is_none());
    assert!(
        flight.landed_at.unwrap() > 40,
        "eight tiles of run-in cannot complete in {} ticks",
        flight.landed_at.unwrap()
    );
}

#[test]
fn takeoff_resumes_flight_from_the_parked_heading() {
    let mut state = arena(vec![unit(0, UnitKind::Condor, 8, 8)], vec![])
        .build()
        .unwrap();
    let condor = state.units()[0].id;
    state.tick(&[]);
    land_at(&mut state, condor, 16, 8);
    let parked_heading = state.unit(condor).unwrap().heading;
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![condor],
            goal: TilePos::new(4, 4),
            queue: false,
        },
    )]);
    let unit = state.unit(condor).unwrap();
    assert!(!unit.landed, "the move order lifts the airframe off");
    let delta = i16::from(unit.heading.wrapping_sub(parked_heading) as i8).abs();
    assert!(delta <= 2, "takeoff turned {delta} steps in its first tick");
    let mut pinned = 0;
    let mut landed = false;
    for _ in 0..600 {
        state.tick(&[]);
        let unit = state.unit(condor).unwrap();
        if unit.pos.x == HALF || unit.pos.x == Fx::from_num(23) + HALF {
            pinned += 1;
        }
        if unit.landed {
            landed = true;
            break;
        }
    }
    assert!(landed, "the move never ended in a landing");
    assert_eq!(pinned, 0);
    assert_eq!(
        state.unit(condor).unwrap().tile(),
        TilePos::new(4, 4),
        "a flier's move ends by landing on the tile it named"
    );
}

#[test]
fn ground_guns_hit_a_parked_condor() {
    // Nothing may be in the Condor's reach when it sets down, so the gun is
    // a Bombard beyond the Condor's sight, shooting what a Sentinel spotter
    // sees from just outside the Condor's acquisition range.
    let mut state = hostile_arena(
        vec![
            unit(0, UnitKind::Condor, 20, 3),
            unit(1, UnitKind::Bombard, 16, 15),
            unit(1, UnitKind::Sentinel, 16, 14),
        ],
        vec![],
    )
    .build()
    .unwrap();
    let condor = state.units()[0].id;
    state.tick(&[]);
    let flight = land_at(&mut state, condor, 16, 8);
    assert_eq!(
        flight.hp_before_touchdown,
        UnitKind::Condor.stats().max_hp,
        "a ground-only gun cannot touch the airframe while it flies"
    );
    // The shelling starts the moment the airframe is a ground body; the
    // Condor answers like any idle unit, so only the first hits are
    // guaranteed to land on it parked.
    for _ in 0..300 {
        if state.unit(condor).unwrap().hp < flight.hp_before_touchdown {
            return;
        }
        state.tick(&[]);
    }
    panic!("the Bombard never hit the parked Condor");
}

#[test]
fn flak_ignores_a_parked_condor() {
    // Anti-air cannot cover a ground body: ordered onto the parked Condor,
    // an enemy Flakhound is sent walking while a ground-only Lancer gets
    // the attack; both flip the moment the airframe is up. Both watch the
    // pad from outside the Condor's acquisition range and their own.
    let mut state = hostile_arena(
        vec![
            unit(0, UnitKind::Condor, 8, 8),
            unit(1, UnitKind::Flakhound, 22, 10),
            unit(1, UnitKind::Lancer, 22, 8),
        ],
        vec![],
    )
    .build()
    .unwrap();
    let condor = state.units()[0].id;
    let flak = state.units()[1].id;
    let lancer = state.units()[2].id;
    state.tick(&[]);
    land_at(&mut state, condor, 16, 8);
    let attack = || {
        cmd(
            1,
            Command::Attack {
                units: vec![flak, lancer],
                target: Target::Unit(condor),
                queue: false,
            },
        )
    };
    state.tick(&[attack()]);
    assert!(
        matches!(state.unit(flak).unwrap().order, Order::Move { .. }),
        "flak was let loose on a parked airframe: {:?}",
        state.unit(flak).unwrap().order
    );
    assert!(
        matches!(
            state.unit(lancer).unwrap().order,
            Order::Attack { target: Target::Unit(id), .. } if id == condor
        ),
        "a ground gun could not attack the parked airframe: {:?}",
        state.unit(lancer).unwrap().order
    );
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![condor],
            goal: TilePos::new(8, 4),
            queue: false,
        },
    )]);
    assert!(!state.unit(condor).unwrap().landed, "the move lifts it off");
    state.tick(&[attack()]);
    assert!(
        matches!(
            state.unit(flak).unwrap().order,
            Order::Attack { target: Target::Unit(id), .. } if id == condor
        ),
        "flak lost the airborne Condor: {:?}",
        state.unit(flak).unwrap().order
    );
    assert!(
        matches!(state.unit(lancer).unwrap().order, Order::Move { .. }),
        "a ground gun was let loose on an airborne Condor: {:?}",
        state.unit(lancer).unwrap().order
    );
}

#[test]
fn a_parked_condor_scrambles_at_an_enemy_in_reach() {
    let mut state = hostile_arena(
        vec![
            unit(0, UnitKind::Condor, 8, 8),
            unit(1, UnitKind::Harvester, 4, 12),
        ],
        vec![],
    )
    .build()
    .unwrap();
    let condor = state.units()[0].id;
    let harvester = state.units()[1].id;
    state.tick(&[]);
    land_at(&mut state, condor, 16, 8);
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![harvester],
            goal: TilePos::new(15, 9),
            queue: false,
        },
    )]);
    let mut scrambled = false;
    for _ in 0..600 {
        state.tick(&[]);
        let unit = state.unit(condor).unwrap();
        if !unit.landed && matches!(unit.order, Order::Attack { .. }) {
            scrambled = true;
            break;
        }
    }
    assert!(scrambled, "the parked Condor never took off at the enemy");
    let flight = fly_and_watch(&mut state, condor, 600, false);
    assert!(
        !flight.releases.is_empty(),
        "the scramble never turned into a pass"
    );
}

#[test]
fn an_idle_condor_lands_itself_after_orbiting() {
    // Parked far from the enemy Foundry so nothing enters acquisition range.
    let mut state = arena(vec![unit(0, UnitKind::Condor, 16, 4)], vec![])
        .build()
        .unwrap();
    let condor = state.units()[0].id;
    let start = state.unit(condor).unwrap().tile();
    for _ in 0..u32::from(AUTO_LAND_IDLE_TICKS) - 5 {
        state.tick(&[]);
        assert!(
            !state.unit(condor).unwrap().landed,
            "landed before the idle orbit ran out"
        );
        assert_eq!(state.unit(condor).unwrap().order, Order::Idle);
    }
    let flight = fly_and_watch(&mut state, condor, 600, true);
    assert!(
        flight.landed_at.is_some(),
        "an idle airframe sets itself down"
    );
    assert_eq!(flight.pinned, 0);
    let parked = state.unit(condor).unwrap();
    assert!(
        parked.tile().chebyshev(start) <= oxide_sim::stats::AUTO_LAND_SCAN_RADIUS + 4,
        "auto-land wandered to {:?} from {:?}",
        parked.tile(),
        start
    );
}

#[test]
fn a_tile_filled_during_the_approach_sends_the_landing_around() {
    let mut state = arena(
        vec![
            unit(0, UnitKind::Condor, 8, 8),
            unit(0, UnitKind::Sentinel, 14, 8),
        ],
        vec![],
    )
    .build()
    .unwrap();
    let condor = state.units()[0].id;
    let sentinel = state.units()[1].id;
    state.tick(&[]);
    state.tick(&[
        land(0, condor, 16, 8),
        cmd(
            0,
            Command::Move {
                units: vec![sentinel],
                goal: TilePos::new(16, 8),
                queue: false,
            },
        ),
    ]);
    let flight = fly_and_watch(&mut state, condor, 1_500, true);
    assert!(flight.landed_at.is_some(), "the Condor never found ground");
    assert_eq!(flight.pinned, 0);
    let parked = state.unit(condor).unwrap();
    let blocker = state.unit(sentinel).unwrap().tile();
    assert_ne!(parked.tile(), blocker, "landed on top of the Sentinel");
    assert!(
        parked.tile().chebyshev(TilePos::new(16, 8)) <= oxide_sim::stats::LANDING_REPLAN_RADIUS,
        "went around to {:?}",
        parked.tile()
    );
}

#[test]
fn a_site_claimed_under_a_parked_condor_lifts_it_off() {
    let mut state = arena(
        vec![
            unit(0, UnitKind::Condor, 8, 8),
            unit(0, UnitKind::Harvester, 15, 11),
        ],
        vec![],
    )
    .build()
    .unwrap();
    let condor = state.units()[0].id;
    let harvester = state.units()[1].id;
    state.tick(&[]);
    land_at(&mut state, condor, 16, 8);
    let report = state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![harvester],
            kind: BuildingKind::Turret,
            anchor: TilePos::new(16, 8),
            queue: false,
            defer: false,
        },
    )]);
    assert!(
        !report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. })),
        "an own site may claim the ground under an own parked airframe"
    );
    let mut lifted = false;
    for _ in 0..3 {
        state.tick(&[]);
        if !state.unit(condor).unwrap().landed {
            lifted = true;
            break;
        }
    }
    assert!(
        lifted,
        "the parked Condor stayed under the claimed footprint"
    );
    let flight = fly_and_watch(&mut state, condor, 900, true);
    assert!(
        flight.landed_at.is_some(),
        "the evicted Condor never landed again"
    );
    assert_eq!(flight.pinned, 0);
    assert_ne!(state.unit(condor).unwrap().tile(), TilePos::new(16, 8));
}

#[test]
fn a_wall_side_tile_is_landed_and_left_without_touching_the_wall() {
    let mut state = arena(vec![unit(0, UnitKind::Condor, 8, 8)], vec![])
        .build()
        .unwrap();
    let condor = state.units()[0].id;
    state.tick(&[]);
    land_at(&mut state, condor, 22, 8);
    assert_eq!(state.unit(condor).unwrap().tile(), TilePos::new(22, 8));
    // Whatever heading it parked on one tile from the east wall, the
    // takeoff from it must fly clear without ever pressing on the wall.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![condor],
            goal: TilePos::new(6, 8),
            queue: false,
        },
    )]);
    let flight = fly_and_watch(&mut state, condor, 400, false);
    assert_eq!(
        flight.pinned, 0,
        "takeoff from the wall-side tile touched the wall"
    );
    assert!(flight.max_still < 10);
}

#[test]
fn a_corner_tile_is_snapped_to_ground_the_airframe_can_leave() {
    let mut state = arena(vec![unit(0, UnitKind::Condor, 8, 8)], vec![])
        .build()
        .unwrap();
    let condor = state.units()[0].id;
    state.tick(&[]);
    let report = state.tick(&[land(0, condor, 22, 14)]);
    if report
        .events
        .iter()
        .any(|e| matches!(e, Event::CommandRejected { .. }))
    {
        return;
    }
    let flight = fly_and_watch(&mut state, condor, 1_500, true);
    assert!(flight.landed_at.is_some());
    assert_eq!(flight.pinned, 0);
    assert_ne!(
        state.unit(condor).unwrap().tile(),
        TilePos::new(22, 14),
        "the corner itself is unlandable"
    );
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![condor],
            goal: TilePos::new(6, 6),
            queue: false,
        },
    )]);
    let flight = fly_and_watch(&mut state, condor, 400, false);
    assert_eq!(
        flight.pinned, 0,
        "takeoff from the snapped tile touched the wall"
    );
    assert!(flight.max_still < 10);
}

#[test]
fn an_enemy_cannot_found_over_a_parked_condor() {
    let mut state = hostile_arena(
        vec![
            unit(0, UnitKind::Condor, 8, 8),
            unit(1, UnitKind::Harvester, 5, 12),
        ],
        vec![],
    )
    .build()
    .unwrap();
    let condor = state.units()[0].id;
    let harvester = state.units()[1].id;
    state.tick(&[]);
    land_at(&mut state, condor, 16, 8);
    let report = state.tick(&[cmd(
        1,
        Command::Build {
            units: vec![harvester],
            kind: BuildingKind::Turret,
            anchor: TilePos::new(16, 8),
            queue: false,
            defer: false,
        },
    )]);
    assert!(
        report.events.iter().any(|e| matches!(
            e,
            Event::CommandRejected {
                reason: RejectReason::BadSite,
                ..
            }
        )),
        "a parked airframe holds its ground like any hostile body"
    );
}

#[test]
fn a_charge_under_the_pad_fires_on_touchdown() {
    let mut state = hostile_arena(
        vec![unit(0, UnitKind::Condor, 8, 8)],
        vec![building(1, BuildingKind::ScuttleCharge, 16, 8)],
    )
    .build()
    .unwrap();
    let condor = state.units()[0].id;
    state.tick(&[]);
    state.tick(&[land(0, condor, 16, 8)]);
    for _ in 0..1_500 {
        let report = state.tick(&[]);
        let detonated = report
            .events
            .iter()
            .any(|e| matches!(e, Event::ChargeDetonated { .. }));
        let unit = state.unit(condor).unwrap();
        if unit.landed || detonated {
            assert!(detonated, "the charge lay quiet under a body on its tile");
            assert!(
                unit.hp < UnitKind::Condor.stats().max_hp,
                "the blast spared the airframe sitting on it"
            );
            return;
        }
    }
    panic!("the Condor never touched down");
}

#[test]
fn a_landing_survives_a_save_and_load_mid_approach_and_parked() {
    let mut state = arena(vec![unit(0, UnitKind::Condor, 8, 8)], vec![])
        .build()
        .unwrap();
    let condor = state.units()[0].id;
    state.tick(&[]);
    state.tick(&[land(0, condor, 16, 8)]);
    for _ in 0..40 {
        state.tick(&[]);
    }
    let round_trip = |state: &State| -> State {
        let json = serde_json::to_string(state).unwrap();
        let restored: State = serde_json::from_str(&json).unwrap();
        restored
            .validate_invariants()
            .expect("snapshot is coherent");
        assert_eq!(state.hash(), restored.hash(), "round trip must be lossless");
        restored
    };
    let mut restored = round_trip(&state);
    for _ in 0..1_500 {
        state.tick(&[]);
        restored.tick(&[]);
        assert_eq!(
            state.hash(),
            restored.hash(),
            "the reloaded approach diverged"
        );
        if state.unit(condor).unwrap().landed {
            break;
        }
    }
    assert!(state.unit(condor).unwrap().landed, "never landed");
    let mut restored = round_trip(&state);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![condor],
            goal: TilePos::new(4, 4),
            queue: false,
        },
    )]);
    restored.tick(&[cmd(
        0,
        Command::Move {
            units: vec![condor],
            goal: TilePos::new(4, 4),
            queue: false,
        },
    )]);
    for _ in 0..100 {
        state.tick(&[]);
        restored.tick(&[]);
        assert_eq!(
            state.hash(),
            restored.hash(),
            "the reloaded takeoff diverged"
        );
    }
    assert!(!state.unit(condor).unwrap().landed);
}

#[test]
fn a_parked_condor_can_be_welded_until_it_takes_off() {
    let mut state = hostile_arena(
        vec![
            unit(0, UnitKind::Condor, 8, 8),
            unit(0, UnitKind::Harvester, 10, 2),
        ],
        vec![building(1, BuildingKind::FlakTurret, 19, 8)],
    )
    .build()
    .unwrap();
    let condor = state.units()[0].id;
    let welder = state.units()[1].id;
    let max_hp = UnitKind::Condor.stats().max_hp;
    state.tick(&[]);
    // Fly past the flak to take some damage.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![condor],
            goal: TilePos::new(16, 8),
            queue: false,
        },
    )]);
    for _ in 0..300 {
        state.tick(&[]);
        if state.unit(condor).unwrap().hp < max_hp {
            break;
        }
    }
    assert!(
        state.unit(condor).unwrap().hp < max_hp,
        "premise: the flak never scratched it"
    );
    // Airborne, it is no weld patient.
    let report = state.tick(&[cmd(
        0,
        Command::RepairUnit {
            units: vec![welder],
            target: condor,
            queue: false,
        },
    )]);
    assert!(
        report.events.iter().any(|e| matches!(
            e,
            Event::CommandRejected {
                reason: RejectReason::InvalidTarget,
                ..
            }
        )),
        "an airborne Condor cannot be welded"
    );
    // Parked out of the flak's reach and away from anything it would
    // scramble at, it is.
    state.tick(&[land(0, condor, 12, 4)]);
    let flight = fly_and_watch(&mut state, condor, 1_500, true);
    assert!(flight.landed_at.is_some(), "the Condor never parked");
    let hp_parked = state.unit(condor).unwrap().hp;
    let report = state.tick(&[cmd(
        0,
        Command::RepairUnit {
            units: vec![welder],
            target: condor,
            queue: false,
        },
    )]);
    assert!(
        !report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. })),
        "a parked Condor is a legal weld patient"
    );
    let mut welded = false;
    for _ in 0..400 {
        state.tick(&[]);
        let unit = state.unit(condor).unwrap();
        assert!(unit.landed, "the patient left the ground during the weld");
        if unit.hp > hp_parked {
            welded = true;
            break;
        }
    }
    assert!(welded, "the torch never gained the parked Condor a point");
    // Takeoff ends the job instead of sending the welder chasing the sky.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![condor],
            goal: TilePos::new(4, 4),
            queue: false,
        },
    )]);
    for _ in 0..5 {
        state.tick(&[]);
    }
    assert!(!state.unit(condor).unwrap().landed);
    assert_ne!(
        state.unit(welder).unwrap().order,
        Order::RepairUnit { unit: condor },
        "the welder kept a job on an airframe that took off"
    );
}

#[test]
fn a_tile_ahead_on_a_diagonal_is_landed_straight_in() {
    // Heading east with the tile ahead and off to one side: the aircraft
    // lines up on that bearing and lands on the ordered tile, with no
    // run-in swinging it out to a cardinal approach.
    let mut state = arena(vec![unit(0, UnitKind::Condor, 8, 8)], vec![])
        .build()
        .unwrap();
    let condor = state.units()[0].id;
    state.tick(&[]);
    assert!(
        state.unit(condor).unwrap().heading <= 4,
        "premise: eastbound"
    );
    state.tick(&[land(0, condor, 17, 12)]);
    state.tick(&[]);
    let path = state.unit(condor).unwrap().path.clone().expect("a route");
    assert_eq!(
        path.waypoints,
        vec![TilePos::new(17, 12)],
        "straight in, no initial point"
    );
    let flight = fly_and_watch(&mut state, condor, 400, true);
    let landed_at = flight.landed_at.expect("touched down");
    assert!(
        landed_at < 200,
        "a ten-tile straight-in took {landed_at} ticks"
    );
    let parked = state.unit(condor).unwrap();
    assert_eq!(parked.tile(), TilePos::new(17, 12));
    let bearing = i16::from(parked.heading.wrapping_sub(24) as i8).abs();
    assert!(
        bearing <= 12,
        "parked on heading {}, not along the approach",
        parked.heading
    );
}

#[test]
fn a_right_click_far_from_any_enemy_ends_by_landing_on_the_tile() {
    let mut state = arena(vec![unit(0, UnitKind::Condor, 4, 4)], vec![])
        .build()
        .unwrap();
    let condor = state.units()[0].id;
    state.tick(&[]);
    state.tick(&[cmd(
        0,
        Command::Advance {
            units: vec![condor],
            goal: TilePos::new(18, 11),
            queue: false,
        },
    )]);
    let flight = fly_and_watch(&mut state, condor, 1_500, true);
    assert!(flight.landed_at.is_some(), "the advance never set down");
    assert_eq!(flight.pinned, 0);
    let parked = state.unit(condor).unwrap();
    assert_eq!(
        parked.tile(),
        TilePos::new(18, 11),
        "a flier's ground destination is the tile it lands on"
    );
    assert_eq!(parked.order, Order::Idle);
}

#[test]
fn a_right_click_beside_an_enemy_stays_airborne_and_fights() {
    let mut state = hostile_arena(
        vec![unit(0, UnitKind::Condor, 4, 4)],
        vec![building(1, BuildingKind::Foundry, 18, 10)],
    )
    .build()
    .unwrap();
    let condor = state.units()[0].id;
    state.tick(&[]);
    state.tick(&[cmd(
        0,
        Command::Advance {
            units: vec![condor],
            goal: TilePos::new(16, 8),
            queue: false,
        },
    )]);
    let flight = fly_and_watch(&mut state, condor, 600, true);
    assert!(
        flight.landed_at.is_none(),
        "touched down at tick {:?} with an enemy in reach of the tile",
        flight.landed_at
    );
    assert!(
        !flight.releases.is_empty(),
        "the aircraft neither landed nor attacked"
    );
}

#[test]
fn a_queued_stop_is_flown_over_and_only_the_last_one_landed_on() {
    let mut state = arena(vec![unit(0, UnitKind::Condor, 4, 4)], vec![])
        .build()
        .unwrap();
    let condor = state.units()[0].id;
    state.tick(&[]);
    state.tick(&[
        cmd(
            0,
            Command::Move {
                units: vec![condor],
                goal: TilePos::new(12, 8),
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![condor],
                goal: TilePos::new(18, 12),
                queue: true,
            },
        ),
    ]);
    let flight = fly_and_watch(&mut state, condor, 1_500, true);
    assert!(flight.landed_at.is_some(), "the program never set down");
    assert_eq!(
        state.unit(condor).unwrap().tile(),
        TilePos::new(18, 12),
        "the first touchdown must be the program's last stop"
    );
}

#[test]
fn an_idle_condor_sets_down_on_a_tile_ahead_within_seconds() {
    // An orbiting aircraft always has a tile a few lengths ahead it can
    // land on straight in; choosing that over the nearest tile keeps the
    // self-landing from flying a whole run-in for a tile one step behind.
    let mut state = arena(vec![unit(0, UnitKind::Condor, 12, 8)], vec![])
        .build()
        .unwrap();
    let condor = state.units()[0].id;
    let flight = fly_and_watch(&mut state, condor, 600, true);
    let landed_at = flight.landed_at.expect("an idle airframe sets itself down");
    assert!(
        landed_at <= u64::from(AUTO_LAND_IDLE_TICKS) + 120,
        "the self-landing took {} ticks after the orbit ran out",
        landed_at - u64::from(AUTO_LAND_IDLE_TICKS)
    );
    assert_eq!(flight.pinned, 0);
}

#[test]
fn an_unseen_enemy_never_pulls_a_landing_into_an_attack() {
    // The Array sits inside the pad's acquisition radius but far beyond
    // the Condor's sight when the move hands over to the landing. Judging
    // acquisition from the tile must still wait for the player to see it.
    let mut state = hostile_arena(
        vec![unit(0, UnitKind::Condor, 4, 8)],
        vec![building(1, BuildingKind::Array, 20, 9)],
    )
    .build()
    .unwrap();
    let condor = state.units()[0].id;
    state.tick(&[]);
    state.tick(&[cmd(
        0,
        Command::Advance {
            units: vec![condor],
            goal: TilePos::new(16, 8),
            queue: false,
        },
    )]);
    let mut attacked = false;
    for _ in 0..900 {
        state.tick(&[]);
        let unit = state.unit(condor).unwrap();
        if let Order::Attack {
            target: Target::Building(id),
            ..
        } = unit.order
        {
            let seen = state
                .building(id)
                .map(|b| b.tiles().any(|t| state.vision(PlayerId(0)).visible(t)))
                .unwrap_or(false);
            assert!(
                seen,
                "the landing turned on a building the player cannot see at tick {} from {:?}",
                state.current_tick(),
                unit.pos
            );
            attacked = true;
            break;
        }
        if unit.landed {
            break;
        }
    }
    assert!(
        attacked,
        "premise: the Array inside the pad's reach is found once seen"
    );
}

#[test]
fn two_condors_landing_inward_on_adjacent_tiles_keep_their_distance() {
    // Each touches down up to a third of a tile past its center toward the
    // other; without clearance around the resting point two immovable
    // parked bodies would overlap for good.
    let mut state = arena(
        vec![
            unit(0, UnitKind::Condor, 18, 8),
            unit(0, UnitKind::Condor, 4, 8),
        ],
        vec![],
    )
    .build()
    .unwrap();
    let east = state.units()[0].id;
    let west = state.units()[1].id;
    state.tick(&[]);
    state.tick(&[
        cmd(
            0,
            Command::Move {
                units: vec![east],
                goal: TilePos::new(22, 8),
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![east],
                goal: TilePos::new(16, 8),
                queue: true,
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![west],
                goal: TilePos::new(17, 8),
                queue: false,
            },
        ),
    ]);
    for _ in 0..1_500 {
        state.tick(&[]);
        if [east, west]
            .iter()
            .all(|id| state.unit(*id).is_some_and(|u| u.landed))
        {
            break;
        }
    }
    let (a, b) = (state.unit(east).unwrap(), state.unit(west).unwrap());
    assert!(a.landed && b.landed, "both airframes park");
    let clearance = UnitKind::Condor.stats().radius * 2;
    assert!(
        a.pos.dist(b.pos) >= clearance,
        "parked {:.2} apart, inside the combined radius {:.2}",
        a.pos.dist(b.pos).to_num::<f64>(),
        clearance.to_num::<f64>()
    );
}
