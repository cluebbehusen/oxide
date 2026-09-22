//! Current-controller integration with mirrored public extractor frames.

mod common;

use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::stats::BuildingKind;
use oxide_sim::{Command, Faction, PlayerId, Scenario, UnitKind};

#[test]
fn a_mirrored_seat_claims_the_real_frame() {
    // The east-half home flips the seat's whole frame of reference,
    // and the derelict frame's mirror image — x = 28-2-11 = 15 — is
    // bare ground. A commander whose orientation forgot to flip
    // known_frames aimed its restoration there and never built an
    // Extractor at all; the shipped maps put mirrored seats in this
    // position on every 180-degree pair.
    let scenario = Scenario {
        name: "mirrored-frame-claim".into(),
        seed: 41,
        map: vec![
            "############################".into(),
            "#..ss..................1...#".into(),
            "#..ss...2..................#".into(),
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
                x: 21,
                y: 2,
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
            UnitSpec {
                player: 1,
                kind: UnitKind::Harvester,
                x: 2,
                y: 6,
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
    let mut state = scenario.build().expect("mirrored proving ground builds");
    let mut bot = common::standard_brain(&scenario, PlayerId(0));

    let mut extractor_stood = false;
    for _ in 0..14_000u32 {
        let commands = bot.act(&state);
        for command in &commands {
            if let Command::Build { kind, anchor, .. } = command.command
                && kind == BuildingKind::Extractor
            {
                assert!(
                    state.map().is_extractor_frame(anchor),
                    "an Extractor claim left for {anchor:?}, which holds no frame"
                );
            }
        }
        state.tick(&commands);
        if state
            .buildings()
            .iter()
            .any(|b| b.player == PlayerId(0) && b.kind == BuildingKind::Extractor && b.built)
        {
            extractor_stood = true;
            break;
        }
    }
    assert!(
        extractor_stood,
        "the mirrored seat never restored the frame it can see"
    );
}
