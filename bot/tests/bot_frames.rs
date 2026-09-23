//! Current-controller integration with mirrored public extractor frames.

mod common;

use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::stats::BuildingKind;
use oxide_sim::{Command, Faction, PlayerId, Scenario, UnitKind};

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
