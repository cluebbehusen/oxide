//! Current-controller integration with mirrored public extractor frames.

mod common;

use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::stats::BuildingKind;
use oxide_sim::{Command, Faction, PlayerId, Scenario, UnitKind};

#[test]
fn a_partially_discovered_frame_receives_an_accepted_deferred_bot_build() {
    use chassis::grid::TilePos;
    use common::simulation::{building, cmd, open_arena_with, unit};

    let frame = TilePos::new(13, 13);
    let mut scenario = open_arena_with(36, 28, vec![unit(0, UnitKind::Harvester, 7, 14)], |rows| {
        rows[13][13] = 'E'
    });
    scenario.map[1].replace_range(1..2, ".");
    scenario.map[4].replace_range(4..5, "1");
    scenario.players[0].scrap = 1_000;
    scenario.buildings = vec![building(0, BuildingKind::Fabricator, 2, 9)];
    scenario
        .units
        .extend((0..4).map(|i| unit(0, UnitKind::Harvester, 3 + i, 7)));
    scenario
        .units
        .extend((0..5).map(|i| unit(0, UnitKind::Sentinel, 2 + i, 3)));

    for remembered in [false, true] {
        let mut state = scenario.build().unwrap();
        let scout = state.units()[0].id;
        if remembered {
            state.tick(&[cmd(
                0,
                Command::Move {
                    units: vec![scout],
                    goal: TilePos::new(3, 14),
                    queue: false,
                },
            )]);
            for _ in 0..200 {
                state.tick(&[]);
            }
        }
        let mut bot = common::standard_brain(&scenario, PlayerId(0));
        while !oxide_bot::difficulty::strategic_admission_tick(state.current_tick()) {
            state.tick(&[]);
        }
        let explored = (0..2)
            .flat_map(|dy| (0..2).map(move |dx| frame.offset(dx, dy)))
            .filter(|tile| state.vision(PlayerId(0)).explored(*tile))
            .collect::<Vec<_>>();
        assert_eq!(explored, vec![frame.offset(0, 1)]);
        assert_eq!(state.vision(PlayerId(0)).visible(explored[0]), !remembered);
        let commands = bot.act(&state);
        assert!(
            commands.iter().any(|command| matches!(command.command,
                Command::Build { kind: BuildingKind::Extractor, anchor, defer: true, .. }
                    if anchor == frame
            )),
            "a discovered frame should receive a deferred build (remembered={remembered}): {commands:?}"
        );
        let report = state.tick(&commands);
        assert!(
            report
                .events
                .iter()
                .all(|event| !matches!(event, oxide_sim::Event::CommandRejected { .. })),
            "{report:?}"
        );
        let site = state
            .buildings()
            .iter()
            .find(|b| b.kind == BuildingKind::Extractor)
            .unwrap();
        assert!(site.provisional);
        let site_id = site.id;
        for _ in 0..1_000 {
            state.tick(&[]);
            if state.building(site_id).unwrap().built {
                break;
            }
        }
        assert!(state.building(site_id).unwrap().built);
        state.validate_invariants().unwrap();
    }
}

#[test]
fn a_mirrored_seat_claims_the_real_frame() {
    // The east-half home flips the seat's whole frame of reference,
    // and the derelict frame's mirror image — x = 28-2-11 = 15 — does
    // not hold a frame. A commander whose orientation forgot to flip
    // known_frames aimed its restoration there and never built an
    // Extractor at all; the shipped maps put mirrored seats in this
    // position on every 180-degree pair.
    let mut scenario = Scenario {
        mode: Default::default(),
        name: "mirrored-frame-claim".into(),
        seed: 41,
        map: vec![
            "############################".into(),
            "#.2.............1..........#".into(),
            "#..........................#".into(),
            "#..........E...............#".into(),
            "#..........................#".into(),
            "#....................ss....#".into(),
            "#....................ss....#".into(),
            "#..........................#".into(),
            "############################".into(),
        ],
        players: vec![
            PlayerSpec {
                name: "Standard East".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 3_500,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "Idle West".into(),
                faction: Faction::Cupric,
                team: None,
                scrap: 150,
                bot: false,
                bot_config: None,
            },
        ],
        units: vec![
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 13,
                y: 3,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 22,
                y: 3,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 20,
                y: 4,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 19,
                y: 2,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 18,
                y: 3,
            },
        ],
        buildings: vec![BuildingSpec {
            player: 0,
            kind: BuildingKind::Fabricator,
            x: 15,
            y: 4,
        }],
        meta: None,
    };
    scenario.units.extend((0..5).map(|index| UnitSpec {
        player: 0,
        kind: UnitKind::Sentinel,
        x: 20 + index,
        y: 7,
    }));
    let mut state = scenario.build().expect("mirrored proving ground builds");
    let anchor = chassis::grid::TilePos::new(11, 3);
    let wrong_anchor = chassis::grid::TilePos::new(15, 4);
    assert!(state.map().is_extractor_frame(anchor));
    assert!(!state.map().is_extractor_frame(wrong_anchor));
    assert!(state.can_see(PlayerId(0), anchor));
    let mut bot = common::standard_brain(&scenario, PlayerId(0));
    let mut claimed = None;
    let mut progressed = false;
    for _ in 0..300 {
        let commands = bot.act(&state);
        for command in &commands {
            if let Command::Build {
                kind: BuildingKind::Extractor,
                anchor: target,
                ..
            } = command.command
            {
                assert_eq!(
                    target, anchor,
                    "the mirrored command must target the real frame"
                );
                claimed = Some(target);
            }
        }
        let report = state.tick(&commands);
        assert!(
            report
                .events
                .iter()
                .all(|event| !matches!(event, oxide_sim::Event::CommandRejected { .. })),
            "{report:?}"
        );
        if let Some(site) = state.buildings().iter().find(|building| {
            building.player == PlayerId(0)
                && building.kind == BuildingKind::Extractor
                && building.anchor == anchor
        }) {
            progressed = site.built || site.hp > BuildingKind::Extractor.base_stats().max_hp / 5;
            if progressed {
                break;
            }
        }
    }
    assert_eq!(
        claimed,
        Some(anchor),
        "funded visible frame received no build command"
    );
    assert!(
        progressed,
        "the accepted Extractor must receive construction work"
    );
}
