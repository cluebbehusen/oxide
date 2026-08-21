//! Derelict Extractor frames: the contestable economy. The map authors
//! 2x2 frames ('E'); an Extractor rebuilds only there, nothing else may
//! pave a frame over, the machine pays the strongest income in the game
//! on an escalating schedule, and its death re-derelicts the frame for
//! whoever holds the ground next.

use chassis::grid::TilePos;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::stats::{BuildingKind, EXTRACTOR_YIELD_SCHEDULE};
use oxide_sim::{Command, Event, Faction, PlayerCommand, PlayerId, Scenario, State, UnitKind};

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

/// A 20x10 arena with one frame mid-field ('E' anchor at (9, 4)).
fn arena(scrap: u32, units: Vec<UnitSpec>, buildings: Vec<BuildingSpec>) -> Scenario {
    Scenario {
        name: "extractor-arena".into(),
        seed: 7,
        map: vec![
            "####################".into(),
            "#1.................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#........E.........#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#...............2..#".into(),
            "#..................#".into(),
            "####################".into(),
        ],
        players: players(scrap),
        units,
        buildings,
        meta: None,
    }
}

fn cmd(player: u8, command: Command) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(player),
        command,
    }
}

fn build(
    player: u8,
    builder: oxide_sim::UnitId,
    kind: BuildingKind,
    anchor: TilePos,
) -> PlayerCommand {
    cmd(
        player,
        Command::Build {
            units: vec![builder],
            kind,
            anchor,
            queue: false,
            defer: false,
        },
    )
}

fn harvester(player: u8, x: i32, y: i32) -> UnitSpec {
    UnitSpec {
        player,
        kind: UnitKind::Harvester,
        x,
        y,
    }
}

const FRAME: TilePos = TilePos { x: 9, y: 4 };

#[test]
fn the_frame_parses_stays_walkable_and_renders() {
    let state = arena(100, vec![], vec![]).build().unwrap();
    assert_eq!(state.map().extractor_frames(), &[FRAME]);
    assert!(state.map().is_extractor_frame(FRAME));
    assert!(state.map().tile_in_extractor_frame(TilePos::new(10, 5)));
    assert!(!state.map().tile_in_extractor_frame(TilePos::new(11, 4)));
    // A collapsed hulk is rubble you can drive over.
    for dy in 0..2 {
        for dx in 0..2 {
            assert!(state.passable(FRAME.offset(dx, dy)));
        }
    }
    let rows = state.map().ascii_rows();
    assert_eq!(rows[4].chars().nth(9), Some('E'));
}

#[test]
fn an_extractor_stands_only_on_its_frame_and_nothing_paves_one() {
    let mut state = arena(1_000, vec![harvester(0, 5, 4)], vec![])
        .build()
        .unwrap();
    let builder = state.units()[0].id;

    // Off-frame: refused.
    let report = state.tick(&[build(
        0,
        builder,
        BuildingKind::Extractor,
        TilePos::new(3, 3),
    )]);
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. })),
        "an extractor away from any frame is refused"
    );

    // A turret overlapping the frame: refused — the ground stays
    // contestable forever.
    let report = state.tick(&[build(0, builder, BuildingKind::Turret, TilePos::new(10, 5))]);
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. })),
        "nothing may pave over a frame"
    );

    // On the frame: the restoration site stands.
    state.tick(&[build(0, builder, BuildingKind::Extractor, FRAME)]);
    assert!(
        state
            .buildings()
            .iter()
            .any(|b| b.kind == BuildingKind::Extractor && b.anchor == FRAME && !b.built),
        "the restoration site stands on the frame"
    );
}

#[test]
fn extractor_income_escalates_on_the_visible_schedule() {
    // The seat keeps a live harvester so the stranded-economy recovery
    // flow stays off and the measurement is the extractor's alone
    // (plus the late Foundry drip, which the margin absorbs).
    let mut scenario = arena(0, vec![harvester(0, 3, 2)], vec![]);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Extractor,
        x: FRAME.x,
        y: FRAME.y,
    });
    let state = scenario.build().unwrap();

    for &(from, amount, period) in EXTRACTOR_YIELD_SCHEDULE.iter() {
        // Jump the clock to the row's start and measure one full period.
        let mut value = serde_json::to_value(&state).unwrap();
        value["tick"] = serde_json::json!(from);
        let mut probe: State = serde_json::from_value(value).unwrap();
        let bank = probe.player(PlayerId(0)).scrap;
        for _ in 0..period {
            probe.tick(&[]);
        }
        let earned = probe.player(PlayerId(0)).scrap - bank;
        assert!(
            earned >= amount,
            "row starting at {from}: one period earns at least {amount}, got {earned}"
        );
    }

    // The escalation is real: measured over the same window length, the
    // late row out-earns the base row.
    let window = 200u64;
    let earn_at = |start: u64| {
        let mut value = serde_json::to_value(&state).unwrap();
        value["tick"] = serde_json::json!(start);
        let mut probe: State = serde_json::from_value(value).unwrap();
        let bank = probe.player(PlayerId(0)).scrap;
        for _ in 0..window {
            probe.tick(&[]);
        }
        probe.player(PlayerId(0)).scrap - bank
    };
    let early = earn_at(0);
    let late = earn_at(24_000);
    assert!(
        late >= early * 2,
        "held extractors compound: {early} early vs {late} late per {window} ticks"
    );
}

#[test]
fn the_contest_cycle_re_derelicts_and_rebuilds() {
    let mut scenario = arena(
        1_000,
        vec![
            harvester(1, 12, 4),
            UnitSpec {
                player: 1,
                kind: UnitKind::Sentinel,
                x: 12,
                y: 5,
            },
        ],
        vec![],
    );
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Extractor,
        x: FRAME.x,
        y: FRAME.y,
    });
    let mut state = scenario.build().unwrap();
    let machine = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Extractor)
        .unwrap()
        .id;
    let (rebuilder, raider) = (state.units()[0].id, state.units()[1].id);

    // The raider grinds the restored machine down.
    state.tick(&[cmd(
        1,
        Command::Attack {
            units: vec![raider],
            target: oxide_sim::Target::Building(machine),
            queue: false,
        },
    )]);
    let mut fell = false;
    for _ in 0..4_000 {
        let report = state.tick(&[]);
        if report
            .events
            .iter()
            .any(|e| matches!(e, Event::BuildingDestroyed { building, .. } if *building == machine))
        {
            fell = true;
            break;
        }
    }
    assert!(fell, "the machine falls to sustained fire");
    assert!(
        state.map().is_extractor_frame(FRAME),
        "the frame outlives the machine"
    );

    // The other side rebuilds on the same frame and owns the yield.
    state.tick(&[build(1, rebuilder, BuildingKind::Extractor, FRAME)]);
    assert!(
        state
            .buildings()
            .iter()
            .any(|b| b.kind == BuildingKind::Extractor
                && b.anchor == FRAME
                && b.player == PlayerId(1)),
        "the ground changes hands"
    );
}
