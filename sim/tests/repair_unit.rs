//! Harvester unit-welding: the machine mirror of the building repair
//! suite. Headless scenarios through the public API only, like
//! `repair.rs` — billing exactness, the stacked-welder refund, the
//! chase-not-weld rule for walking patients, fire winning ties, and the
//! command validation ring.

use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::scenario::{PlayerSpec, UnitSpec};
use oxide_sim::{
    Command, Event, Faction, Order, PlayerCommand, PlayerId, Scenario, State, UnitId, UnitKind,
};

fn arena(units: Vec<UnitSpec>) -> Scenario {
    Scenario {
        name: "weld-arena".into(),
        seed: 42,
        map: vec![
            "################".into(),
            "#1.............#".into(),
            "#..............#".into(),
            "#.....##.......#".into(),
            "#.....##...s...#".into(),
            "#..........s...#".into(),
            "#............2.#".into(),
            "#..............#".into(),
            "################".into(),
        ],
        players: vec![
            PlayerSpec {
                name: "Ferrous".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 200,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "Cupric".into(),
                faction: Faction::Cupric,
                team: None,
                scrap: 200,
                bot: false,
                bot_config: None,
            },
        ],
        units,
        buildings: Vec::new(),
        meta: None,
    }
}

fn unit(player: u8, kind: UnitKind, x: i32, y: i32) -> UnitSpec {
    UnitSpec { player, kind, x, y }
}

fn cmd(player: u8, command: Command) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(player),
        command,
    }
}

fn run_until(
    state: &mut State,
    max_ticks: u64,
    mut stop: impl FnMut(&State, &[Event]) -> bool,
) -> Vec<Event> {
    let mut all = Vec::new();
    for _ in 0..max_ticks {
        let report = state.tick(&[]);
        let done = stop(state, &report.events);
        all.extend(report.events);
        if done {
            return all;
        }
    }
    panic!("condition not reached within {max_ticks} ticks");
}

const PATIENT_MAX: u32 = 60; // harvester max_hp, the suite's patient

/// Walks the raider beside the patient (auto-acquire does the gnawing),
/// lets it chew to at most `floor` hp, then pulls it back to its corner.
/// Returns the wound.
fn wound(state: &mut State, patient: UnitId, raider: UnitId, floor: u32) -> u32 {
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![raider],
            goal: TilePos::new(6, 2),
            queue: false,
        },
    )]);
    run_until(state, 2_000, |s, _| s.unit(patient).unwrap().hp <= floor);
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![raider],
            goal: TilePos::new(12, 6),
            queue: false,
        },
    )]);
    run_until(state, 600, |s, _| {
        s.unit(raider).unwrap().tile() == TilePos::new(12, 6)
    });
    let hp = state.unit(patient).unwrap().hp;
    assert!(
        hp > 0 && hp < PATIENT_MAX,
        "test premise: the gnawing must leave a live, weldable patient (hp {hp})"
    );
    hp
}

/// The standard cast: a welder, a patient (both harvesters — unit weld
/// steps are 0/1 hp on the harvester ramp, which the exact-billing
/// tests count on), and an enemy scuttler to do the wounding.
fn cast() -> Vec<UnitSpec> {
    vec![
        unit(0, UnitKind::Harvester, 2, 5), // welder
        unit(0, UnitKind::Harvester, 4, 2), // patient
        unit(1, UnitKind::Scuttler, 12, 6), // raider
    ]
}

#[test]
fn harvesters_weld_wounded_machines_for_a_price() {
    let mut state = arena(cast()).build().unwrap();
    let (welder, patient, raider) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let hurt = wound(&mut state, patient, raider, 45);
    let bank_before = state.player(PlayerId(0)).scrap;
    state.tick(&[cmd(
        0,
        Command::RepairUnit {
            units: vec![welder],
            target: patient,
            queue: false,
        },
    )]);
    run_until(&mut state, 600, |s, _| {
        s.unit(patient).unwrap().hp == PATIENT_MAX
    });
    let spent = bank_before - state.player(PlayerId(0)).scrap;
    let healed = PATIENT_MAX - hurt;
    assert!(spent > 0, "welding a machine is never free");
    assert!(
        spent < healed,
        "but under a scrap per hp on the harvester's price (spent {spent} for {healed} hp)"
    );
    // The job wraps up on its own; the patient was never re-ordered.
    run_until(&mut state, 20, |s, _| {
        s.unit(welder).unwrap().order == Order::Idle
    });
    assert_eq!(state.unit(patient).unwrap().order, Order::Idle);
}

#[test]
fn the_torch_bills_its_first_scrap_before_free_hp_can_land() {
    // Billing lands at each interval's start: the first coin drops with
    // (or before) the first welded hp, so chip welds pay their coin.
    let mut state = arena(cast()).build().unwrap();
    let (welder, patient, raider) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    wound(&mut state, patient, raider, 45);
    let bank_before = state.player(PlayerId(0)).scrap;
    state.tick(&[cmd(
        0,
        Command::RepairUnit {
            units: vec![welder],
            target: patient,
            queue: false,
        },
    )]);
    run_until(&mut state, 100, |s, _| {
        s.player(PlayerId(0)).scrap < bank_before
    });
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        bank_before - 1,
        "exactly one scrap up front, not a free first interval"
    );
}

#[test]
fn a_rejected_welders_prepaid_coin_comes_back() {
    // Two FRESH welders join a patient one hp short of full. Their
    // meters run in phase — both bill their first coin on the tick
    // their first hp comes due — but the ceiling accepts one step and
    // rejects the other whole. The rejected welder's coin must come
    // back at resolution, exactly like the building ledger.
    let mut units = cast();
    units.push(unit(0, UnitKind::Harvester, 2, 6));
    units.push(unit(0, UnitKind::Harvester, 2, 7));
    let mut state = arena(units).build().unwrap();
    let (opener, patient, raider) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let fresh = vec![state.units()[3].id, state.units()[4].id];
    wound(&mut state, patient, raider, 45);
    // Park the fresh pair in body contact BEFORE they are needed, so
    // their first weld ticks are their first meter ticks.
    for (torch, park) in fresh.iter().zip([TilePos::new(3, 2), TilePos::new(5, 2)]) {
        state.tick(&[cmd(
            0,
            Command::Move {
                units: vec![*torch],
                goal: park,
                queue: false,
            },
        )]);
        run_until(&mut state, 300, |s, _| {
            s.unit(*torch).unwrap().tile() == park
        });
    }
    // The opener welds to exactly one hp short (harvester steps are
    // never more than 1 hp per tick: ramp 60 over 100 ticks).
    state.tick(&[cmd(
        0,
        Command::RepairUnit {
            units: vec![opener],
            target: patient,
            queue: false,
        },
    )]);
    run_until(&mut state, 900, |s, _| {
        s.unit(patient).unwrap().hp >= PATIENT_MAX - 1
    });
    assert_eq!(
        state.unit(patient).unwrap().hp,
        PATIENT_MAX - 1,
        "test premise: stopped one hp short"
    );
    state.tick(&[cmd(
        0,
        Command::Stop {
            units: vec![opener],
        },
    )]);
    // Both fresh torches take the last hp together.
    let bank_before = state.player(PlayerId(0)).scrap;
    state.tick(&[cmd(
        0,
        Command::RepairUnit {
            units: fresh.clone(),
            target: patient,
            queue: false,
        },
    )]);
    run_until(&mut state, 60, |s, _| {
        s.unit(patient).unwrap().hp == PATIENT_MAX
    });
    let spent = bank_before - state.player(PlayerId(0)).scrap;
    assert_eq!(
        spent, 1,
        "one hp landed, one scrap billed — the rejected step's coin refunds"
    );
}

#[test]
fn a_walking_patient_is_chased_not_welded() {
    // The both-stationary rule: the torch never rides along with a
    // retreat. The welder trails the walking patient and the wound
    // stays open until the patient stops — and the patient's own
    // program is never evicted by the weld.
    let mut state = arena(cast()).build().unwrap();
    let (welder, patient, raider) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let hurt = wound(&mut state, patient, raider, 45);
    // Outside the parked raider's aggro — the walk must stay a walk.
    let goal = TilePos::new(6, 7);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![patient],
            goal,
            queue: false,
        },
    )]);
    state.tick(&[cmd(
        0,
        Command::RepairUnit {
            units: vec![welder],
            target: patient,
            queue: false,
        },
    )]);
    run_until(&mut state, 600, |s, _| {
        s.unit(patient).unwrap().tile() == goal
    });
    assert_eq!(
        state.unit(patient).unwrap().hp,
        hurt,
        "no hp landed while the patient walked"
    );
    // Parked, the chase closes and the weld begins.
    run_until(&mut state, 200, |s, _| s.unit(patient).unwrap().hp > hurt);
}

#[test]
fn fire_wins_the_tick_and_the_dead_forfeit_their_welds() {
    // A patient under enough fire dies mid-weld: buffered heals land
    // only on machines the volley left standing, so the torch never
    // outbids the guns on the tick they win — and the welder's job
    // simply ends.
    let mut units = cast();
    units.push(unit(1, UnitKind::Scuttler, 12, 7));
    let mut state = arena(units).build().unwrap();
    let (welder, patient, r1) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let r2 = state.units()[3].id;
    let hurt = wound(&mut state, patient, r1, 40);
    state.tick(&[cmd(
        0,
        Command::RepairUnit {
            units: vec![welder],
            target: patient,
            queue: false,
        },
    )]);
    // The weld is live before the guns come back.
    run_until(&mut state, 100, |s, _| s.unit(patient).unwrap().hp > hurt);
    // Two gnawing scuttlers out-pace one torch (20 hp/s vs 12).
    for raider in [r1, r2] {
        state.tick(&[cmd(
            1,
            Command::Move {
                units: vec![raider],
                goal: TilePos::new(5, 3),
                queue: false,
            },
        )]);
    }
    let events = run_until(&mut state, 2_000, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::UnitDied { unit, .. } if *unit == patient))
    });
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::UnitDied { unit, .. } if *unit == patient)),
        "the patient must fall despite an active welder"
    );
    assert!(state.unit(patient).is_none(), "nothing resurrects");
    // The welder learns the job is over and stands down (unless the
    // raiders turned on it — a corpse gives no orders either way).
    run_until(&mut state, 20, |s, _| {
        s.unit(welder).is_none_or(|u| u.order == Order::Idle)
    });
}

#[test]
fn reissued_welds_still_pay_for_the_torch_time() {
    // The no-op reissue rule: re-clicking the exact weld keeps the
    // billing meter, so a re-commanded welder never re-enters the
    // prepaid stretch and heals for free.
    let mut state = arena(cast()).build().unwrap();
    let (welder, patient, raider) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    wound(&mut state, patient, raider, 45);
    let bank_before = state.player(PlayerId(0)).scrap;
    let mut healed = false;
    for _ in 0..150 {
        state.tick(&[cmd(
            0,
            Command::RepairUnit {
                units: vec![welder],
                target: patient,
                queue: false,
            },
        )]);
        for _ in 0..3 {
            state.tick(&[]);
        }
        if state.unit(patient).unwrap().hp == PATIENT_MAX {
            healed = true;
            break;
        }
    }
    assert!(healed, "the weld must finish under re-command");
    assert!(
        state.player(PlayerId(0)).scrap < bank_before,
        "and the torch must have been paid for ({} -> {})",
        bank_before,
        state.player(PlayerId(0)).scrap
    );
}

#[test]
fn a_queued_weld_waits_its_turn_then_welds() {
    let mut state = arena(cast()).build().unwrap();
    let (welder, patient, raider) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let hurt = wound(&mut state, patient, raider, 45);
    let waypoint = TilePos::new(10, 2);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![welder],
            goal: waypoint,
            queue: false,
        },
    )]);
    state.tick(&[cmd(
        0,
        Command::RepairUnit {
            units: vec![welder],
            target: patient,
            queue: true,
        },
    )]);
    assert!(
        matches!(state.unit(welder).unwrap().order, Order::Move { .. }),
        "the march survives the shift-weld"
    );
    assert_eq!(state.unit(welder).unwrap().queue.len(), 1);
    run_until(&mut state, 400, |s, _| {
        s.unit(welder).unwrap().tile() == waypoint
    });
    run_until(&mut state, 2_000, |s, _| s.unit(patient).unwrap().hp > hurt);
}

#[test]
fn weld_refuses_the_healthy_the_foreign_the_flying_and_the_selfish() {
    let mut units = cast();
    // The own guard parks in the far corner, outside aggro of the
    // raider's whole corridor; the enemy pair sits across the map so
    // nothing acquires anything until asked.
    units.push(unit(0, UnitKind::Sentinel, 1, 7)); // a non-worker crew
    units.push(unit(0, UnitKind::Buzzard, 4, 7)); // the future air patient
    units.push(unit(1, UnitKind::Sentinel, 12, 5)); // the poke that wounds it
    units.push(unit(1, UnitKind::Sentinel, 13, 5)); // its second gun
    let mut state = arena(units).build().unwrap();
    let (welder, patient, raider) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let (guard, flyer) = (state.units()[3].id, state.units()[4].id);

    let rejected = |state: &mut State, command: Command, reason: RejectReason| {
        let report = state.tick(&[cmd(0, command)]);
        assert!(
            report.events.contains(&Event::CommandRejected {
                player: PlayerId(0),
                reason,
            }),
            "expected {reason:?}, got {:?}",
            report.events
        );
    };

    // Full health leaves nothing to do.
    rejected(
        &mut state,
        Command::RepairUnit {
            units: vec![welder],
            target: patient,
            queue: false,
        },
        RejectReason::InvalidTarget,
    );
    // Foreign machines are not patients.
    rejected(
        &mut state,
        Command::RepairUnit {
            units: vec![welder],
            target: raider,
            queue: false,
        },
        RejectReason::InvalidTarget,
    );
    // A combat unit holds no torch.
    let hurt = wound(&mut state, patient, raider, 45);
    assert!(hurt < PATIENT_MAX);
    rejected(
        &mut state,
        Command::RepairUnit {
            units: vec![guard],
            target: patient,
            queue: false,
        },
        RejectReason::NoValidUnits,
    );
    // A wounded machine cannot weld itself: the patient drops out of
    // its own crew, and alone that leaves no valid units.
    rejected(
        &mut state,
        Command::RepairUnit {
            units: vec![patient],
            target: patient,
            queue: false,
        },
        RejectReason::NoValidUnits,
    );
    // The air rule: the buzzard flies into the enemy pair's aggro and
    // their skyward pokes wound it; the wounded flyer still refuses
    // the ground torch — that patient waits for a facility that owns
    // the sky.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![flyer],
            goal: TilePos::new(10, 6),
            queue: false,
        },
    )]);
    run_until(&mut state, 2_000, |s, _| {
        s.unit(flyer)
            .is_none_or(|u| u.hp < UnitKind::Buzzard.stats().max_hp)
    });
    assert!(
        state.unit(flyer).is_some(),
        "test premise: the flyer survives its wounding"
    );
    // Pull the flyer home; validation cares only that it is wounded air.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![flyer],
            goal: TilePos::new(4, 7),
            queue: false,
        },
    )]);
    rejected(
        &mut state,
        Command::RepairUnit {
            units: vec![welder],
            target: flyer,
            queue: false,
        },
        RejectReason::InvalidTarget,
    );
}
