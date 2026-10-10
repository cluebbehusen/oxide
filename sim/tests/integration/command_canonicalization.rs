//! A command's unit list is a set. A repeated id must buy nothing — not a
//! second queued leg, not a second application — on any verb that carries
//! one.

use crate::common;

use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, ScenarioMode};
use oxide_sim::stats::BuildingKind;
use oxide_sim::{
    BuildingId, Command, Event, Faction, Order, OrderKey, PlayerId, Scenario, State, Target,
    UnitId, UnitKind,
};

use common::{cmd, run_until, unit};

/// One arm per [`Command`] variant, contiguous indices. Adding a variant
/// stops this file compiling, which is the point: the new verb then has to
/// be placed in one of the two tag lists below.
fn command_tag(command: &Command) -> usize {
    match command {
        Command::Run { .. } => 0,
        Command::Attack { .. } => 1,
        Command::Hunt { .. } => 2,
        Command::Harvest { .. } => 3,
        Command::Patrol { .. } => 4,
        Command::Build { .. } => 5,
        Command::Cancel { .. } => 6,
        Command::Repair { .. } => 7,
        Command::Salvage { .. } => 8,
        Command::Stop { .. } => 9,
        Command::Train { .. } => 10,
        Command::CancelTrain { .. } => 11,
        Command::SetRally { .. } => 12,
        Command::Surrender => 13,
        Command::RepairUnit { .. } => 14,
        Command::Advance { .. } => 15,
        Command::FocusFire { .. } => 16,
        Command::CancelFound { .. } => 17,
        Command::UpgradeBuilding { .. } => 18,
        Command::Load { .. } => 19,
        Command::Unload { .. } => 20,
        Command::ClearFocus { .. } => 21,
        Command::ReturnCargo { .. } => 22,
        Command::CancelOrder { .. } => 23,
    }
}

const COMMAND_VARIANTS: usize = 24;

/// The verbs that carry a unit list — every one of them owes this file a
/// duplicate-id row.
const UNIT_BEARING_TAGS: [usize; 13] = [0, 1, 2, 3, 4, 5, 7, 8, 9, 14, 15, 19, 22];

/// The verbs that address a building alone, with no list to canonicalize.
const BUILDING_ONLY_TAGS: [usize; 5] = [6, 10, 11, 12, 18];

/// The one verb whose building operand is a canonicalized set.
const BUILDING_BEARING_TAGS: [usize; 2] = [16, 21];

/// The one verb that addresses a logical site, with no entity list to canonicalize.
const SITE_ONLY_TAGS: [usize; 1] = [17];

/// The one verb that addresses a single unit, with no list to canonicalize.
const UNIT_ONLY_TAGS: [usize; 1] = [20];

/// The verbs that name no entity at all — nothing to canonicalize.
const OPERANDLESS_TAGS: [usize; 1] = [13];

/// The one verb that names a subject unit beside a list of others: the list
/// canonicalizes, and the subject edits once even when the list repeats it.
const SUBJECT_WITH_UNITS_TAGS: [usize; 1] = [23];

/// A quiet field with a legal target for every unit-bearing verb: a guard
/// that can see an enemy without being in aggro of it, a worker beside a
/// scrap node and buildable ground, an own turret the raid left
/// wounded — welded and stripped alike — and an own sentinel its own
/// skirmish left weldable.
struct Stage {
    state: State,
    guard: UnitId,
    worker: UnitId,
    transport: UnitId,
    enemy: Target,
    turret: BuildingId,
    patient: UnitId,
    node: TilePos,
    ground: TilePos,
    anchor: TilePos,
}

fn map() -> Vec<String> {
    let mut rows = vec![vec!['#'; 24]; 16];
    for row in rows.iter_mut().take(15).skip(1) {
        for cell in row.iter_mut().take(23).skip(1) {
            *cell = '.';
        }
    }
    rows[1][1] = '1';
    rows[13][21] = '2';
    rows[6][7] = 's';
    rows.into_iter().map(|r| r.into_iter().collect()).collect()
}

fn stage() -> Stage {
    let seat = |name: &str, faction| PlayerSpec {
        name: name.into(),
        faction,
        team: None,
        scrap: 500,
        bot: false,
        bot_config: None,
    };
    let mut state = Scenario {
        mode: ScenarioMode::Match,
        name: "canonicalization-arena".into(),
        map: map(),
        players: vec![
            seat("Ferrous", Faction::Ferrous),
            seat("Cupric", Faction::Cupric),
        ],
        units: vec![
            unit(0, UnitKind::Sentinel, 5, 4),
            unit(0, UnitKind::Harvester, 5, 5),
            // Six tiles out: inside the guard's sight, outside its aggro,
            // so both sides stand still until commanded.
            unit(1, UnitKind::Scuttler, 11, 4),
            unit(1, UnitKind::Scuttler, 14, 9),
            // The patient's skirmish: a sentinel spawned in mutual aggro
            // with a scuttler it beats — wounded, still standing.
            unit(0, UnitKind::Sentinel, 3, 11),
            unit(1, UnitKind::Scuttler, 4, 12),
            // The load family's carrier, hovering out of every fight.
            unit(0, UnitKind::Skyhook, 8, 3),
        ],
        buildings: vec![
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Turret,
                x: 14,
                y: 10,
            },
            // The upgrade family's tech gate: Heavy Turret requires a
            // standing Fabricator.
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Fabricator,
                x: 1,
                y: 7,
            },
        ],
        meta: None,
    }
    .build()
    .unwrap();

    let mut data = serde_json::to_value(&state).unwrap();
    data["units"][1]["worker"]["carrying"] = serde_json::json!(3);
    state = serde_json::from_value(data).unwrap();
    let guard = state.units()[0].id;
    let worker = state.units()[1].id;
    let enemy = Target::Unit(state.units()[2].id);
    let raider = state.units()[3].id;
    let patient = state.units()[4].id;
    let biter = state.units()[5].id;
    let transport = state.units()[6].id;
    let turret = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Turret)
        .expect("the staged turret")
        .id;

    // The raid: the scuttler gnaws the turret and dies to it, leaving the
    // scars that make a repair order legal.
    state.tick(&[cmd(
        1,
        Command::Attack {
            units: vec![raider],
            target: Target::Building(turret).into(),
            queue: false,
        },
    )]);
    let mut fallen = Vec::new();
    run_until(&mut state, 600, |_, events| {
        fallen.extend(events.iter().filter_map(|e| match e {
            Event::UnitDied { unit, .. } => Some(*unit),
            _ => None,
        }));
        fallen.contains(&raider) && fallen.contains(&biter)
    });
    let hp = state.building(turret).unwrap().hp;
    assert!(
        hp < BuildingKind::Turret.base_stats().max_hp,
        "test premise: the raid must leave the turret weldable (hp {hp})"
    );
    let patient_hp = state.unit(patient).unwrap().hp;
    assert!(
        patient_hp < UnitKind::Sentinel.stats().max_hp,
        "test premise: the skirmish must leave the sentinel weldable (hp {patient_hp})"
    );
    assert_eq!(state.unit(guard).unwrap().order, Order::Idle);
    assert_eq!(state.unit(worker).unwrap().order, Order::Idle);
    run_until(&mut state, 60, |s, _| {
        s.unit(patient).unwrap().order == Order::Idle
    });

    Stage {
        state,
        guard,
        worker,
        transport,
        enemy,
        turret,
        patient,
        node: TilePos::new(7, 6),
        ground: TilePos::new(8, 8),
        anchor: TilePos::new(6, 7),
    }
}

/// A verb under test: who it addresses, and how to phrase it for a given
/// unit list.
struct Family {
    name: &'static str,
    actor: UnitId,
    make: Box<dyn Fn(Vec<UnitId>, bool) -> Command>,
}

fn families(stage: &Stage) -> Vec<Family> {
    let Stage {
        guard,
        worker,
        transport,
        enemy,
        turret,
        patient,
        node,
        ground,
        anchor,
        ..
    } = *stage;
    vec![
        Family {
            name: "return-cargo",
            actor: worker,
            make: Box::new(move |units, _| Command::ReturnCargo {
                units,
                foundry: None,
                repair: false,
            }),
        },
        Family {
            name: "run",
            actor: guard,
            make: Box::new(move |units, queue| Command::Run {
                units,
                goal: ground,
                queue,
            }),
        },
        Family {
            name: "attack",
            actor: guard,
            make: Box::new(move |units, queue| Command::Attack {
                units,
                target: enemy.into(),
                queue,
            }),
        },
        Family {
            name: "hunt",
            actor: guard,
            make: Box::new(move |units, queue| Command::Hunt {
                units,
                goal: ground,
                queue,
            }),
        },
        Family {
            name: "advance",
            actor: guard,
            make: Box::new(move |units, queue| Command::Advance {
                units,
                goal: ground,
                queue,
            }),
        },
        Family {
            name: "harvest",
            actor: worker,
            make: Box::new(move |units, queue| Command::Harvest { units, node, queue }),
        },
        Family {
            name: "patrol",
            actor: guard,
            make: Box::new(move |units, _| Command::Patrol {
                units,
                waypoints: vec![ground, TilePos::new(9, 9)],
            }),
        },
        Family {
            name: "build",
            actor: worker,
            make: Box::new(move |units, queue| Command::Build {
                units,
                kind: BuildingKind::Turret,
                anchor,
                queue,
                defer: false,
            }),
        },
        Family {
            name: "repair",
            actor: worker,
            make: Box::new(move |units, queue| Command::Repair {
                units,
                building: turret,
                queue,
            }),
        },
        Family {
            name: "salvage",
            actor: worker,
            make: Box::new(move |units, queue| Command::Salvage {
                units,
                building: turret,
                queue,
            }),
        },
        Family {
            name: "stop",
            actor: guard,
            make: Box::new(move |units, _| Command::Stop { units }),
        },
        Family {
            name: "repair-unit",
            actor: worker,
            make: Box::new(move |units, queue| Command::RepairUnit {
                units,
                target: patient,
                queue,
            }),
        },
        Family {
            name: "load",
            actor: guard,
            make: Box::new(move |units, queue| Command::Load {
                units,
                transport,
                queue,
            }),
        },
    ]
}

#[test]
fn every_verb_is_sorted_into_a_tag_list() {
    let stage = stage();
    let mut covered: Vec<usize> = families(&stage)
        .iter()
        .map(|f| command_tag(&(f.make)(Vec::new(), false)))
        .collect();
    covered.sort_unstable();
    assert_eq!(
        covered, UNIT_BEARING_TAGS,
        "a unit-bearing verb without a duplicate-id row"
    );
    let mut all: Vec<usize> = UNIT_BEARING_TAGS
        .into_iter()
        .chain(BUILDING_ONLY_TAGS)
        .chain(BUILDING_BEARING_TAGS)
        .chain(SITE_ONLY_TAGS)
        .chain(UNIT_ONLY_TAGS)
        .chain(OPERANDLESS_TAGS)
        .chain(SUBJECT_WITH_UNITS_TAGS)
        .collect();
    all.sort_unstable();
    assert_eq!(
        all,
        (0..COMMAND_VARIANTS).collect::<Vec<_>>(),
        "every command is unit-bearing, subject-with-units, site-only, building-bearing, building-only, or operandless"
    );
}

/// The direct comparison: a tripled id must leave the world bit-identical
/// to the same command sent once.
#[test]
fn a_tripled_id_lands_exactly_what_a_single_id_lands() {
    let stage = stage();
    for family in families(&stage) {
        for queue in [false, true] {
            let mut once = stage.state.clone();
            once.tick(&[cmd(0, (family.make)(vec![family.actor], queue))]);
            let mut thrice = stage.state.clone();
            thrice.tick(&[cmd(0, (family.make)(vec![family.actor; 3], queue))]);
            assert_eq!(
                once.hash(),
                thrice.hash(),
                "{} with queue={queue} read a repeated id as three units",
                family.name
            );
        }
    }
}

/// The append case: three copies of one id must take one queue slot, not
/// three.
#[test]
fn a_tripled_append_takes_one_queue_slot() {
    let stage = stage();
    for family in families(&stage) {
        let mut state = stage.state.clone();
        // Something to queue behind — an idle unit takes the order
        // outright and the append never fires.
        state.tick(&[cmd(
            0,
            Command::Run {
                units: vec![family.actor],
                goal: TilePos::new(4, 9),
                queue: false,
            },
        )]);
        let before = state.unit(family.actor).unwrap().queue.len();
        state.tick(&[cmd(0, (family.make)(vec![family.actor; 3], true))]);
        let unit = state.unit(family.actor).unwrap();
        // Patrol and Stop rewrite the program wholesale and carry no queue
        // flag; every other verb parks exactly one leg.
        let expected = match family.name {
            "patrol" => 1, // the second waypoint, waiting its turn
            "stop" | "return-cargo" => 0,
            _ => before + 1,
        };
        assert_eq!(
            unit.queue.len(),
            expected,
            "{} appended a repeated id more than once",
            family.name
        );
    }
}

/// The asymmetric case: an idle unit takes the order itself, and the
/// repeats must not append clones of the order it was just given.
#[test]
fn a_tripled_order_to_an_idle_unit_queues_nothing() {
    let stage = stage();
    for family in families(&stage) {
        let mut state = stage.state.clone();
        assert_eq!(state.unit(family.actor).unwrap().order, Order::Idle);
        state.tick(&[cmd(0, (family.make)(vec![family.actor; 3], true))]);
        let unit = state.unit(family.actor).unwrap();
        let expected = match family.name {
            "patrol" => 1, // the second waypoint, waiting its turn
            _ => 0,
        };
        assert_eq!(
            unit.queue.len(),
            expected,
            "{} cloned itself behind the order it had just set",
            family.name
        );
    }
}

/// Canonicalizing at dispatch must not swallow the handlers' own verdict:
/// ownership and capability still decide, and repeating an id the seat
/// cannot command changes nothing about what it hears back.
#[test]
fn a_tripled_foreign_id_is_still_no_valid_units() {
    let stage = stage();
    let Target::Unit(foreign) = stage.enemy else {
        unreachable!("the staged enemy is a unit")
    };
    let refusal = vec![Event::CommandRejected {
        player: PlayerId(0),
        reason: RejectReason::NoValidUnits,
    }];
    for family in families(&stage) {
        for count in [1, 3] {
            let mut state = stage.state.clone();
            let report = state.tick(&[cmd(0, (family.make)(vec![foreign; count], false))]);
            let rejects: Vec<Event> = report
                .events
                .into_iter()
                .filter(|e| matches!(e, Event::CommandRejected { .. }))
                .collect();
            assert_eq!(rejects, refusal, "{} with {count} unowned ids", family.name);
        }
    }
}

fn walk_program(state: &State, id: UnitId) -> Vec<TilePos> {
    let unit = state.unit(id).unwrap();
    std::iter::once(&unit.order)
        .chain(&unit.queue)
        .map(|order| match order.key(state, unit.player) {
            Some(OrderKey::Walk { tile }) => tile,
            other => panic!("the staged program walks, not {other:?}"),
        })
        .collect()
}

/// A subject repeated inside its own list, and another unit repeated there,
/// must each edit once: on the staged `[A, B, A]`, removing the first visit
/// to A a second time would take the other visit as well.
#[test]
fn a_repeated_cancellation_subject_edits_once() {
    let stage = stage();
    let (subject, other) = (stage.guard, stage.worker);
    let (a, b) = (stage.ground, TilePos::new(9, 9));
    let mut state = stage.state.clone();
    let legs: Vec<_> = [a, b, a]
        .into_iter()
        .enumerate()
        .map(|(leg, goal)| {
            cmd(
                0,
                Command::Run {
                    units: vec![subject, other],
                    goal,
                    queue: leg > 0,
                },
            )
        })
        .collect();
    state.tick(&legs);
    for id in [subject, other] {
        assert_eq!(walk_program(&state, id), [a, b, a], "test premise");
    }
    let cancel = |units: Vec<UnitId>| {
        cmd(
            0,
            Command::CancelOrder {
                unit: subject,
                key: OrderKey::Walk { tile: a },
                from_end: 1,
                units,
            },
        )
    };

    let mut once = state.clone();
    once.tick(&[cancel(vec![other])]);
    let mut repeated = state.clone();
    repeated.tick(&[cancel(vec![other, subject, other, subject, other])]);
    assert_eq!(once.hash(), repeated.hash());
    for id in [subject, other] {
        assert_eq!(walk_program(&repeated, id), [b, a]);
    }

    let Target::Unit(foreign) = stage.enemy else {
        unreachable!("the staged enemy is a unit")
    };
    // A foreign subject refuses the whole command, however often the list
    // names a unit that holds the order.
    let mut control = state.clone();
    control.tick(&[]);
    let report = state.tick(&[cmd(
        0,
        Command::CancelOrder {
            unit: foreign,
            key: OrderKey::Walk { tile: a },
            from_end: 0,
            units: vec![subject, other, subject, other],
        },
    )]);
    assert!(report.events.contains(&Event::CommandRejected {
        player: PlayerId(0),
        reason: RejectReason::NoValidUnits,
    }));
    assert_eq!(state.hash(), control.hash());
}
