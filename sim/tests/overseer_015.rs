//! The Overseer: the scripted commander with the 0.15 tree switched
//! on. Given a teched base, a rich bank, and a known derelict frame, it
//! must restore the Extractor, raise the Airworks, and lift a works a
//! tier — the acts the frozen ladder cannot take.

use oxide_sim::bot::Brain;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::stats::BuildingKind;
use oxide_sim::{Command, Faction, PlayerId, Scenario, UnitKind};

#[test]
fn the_overseer_climbs_the_whole_tree() {
    // The rock pocket on the east side holds two idle enemy harvesters
    // in permanent view of the Array beside it: the raid-minded
    // purchases (Scuttler, air wing) fire only while the enemy shows an
    // economy, and the pocket keeps that showing alive.
    let scenario = Scenario {
        name: "overseer-proving-ground".into(),
        seed: 23,
        map: vec![
            "############################".into(),
            "#1.....................ss..#".into(),
            "#......................ss..#".into(),
            "#..........E...............#".into(),
            "#...............######.....#".into(),
            "#..ss...........#....#.....#".into(),
            "#..ss...........######.....#".into(),
            "#..........................#".into(),
            "#..........................#".into(),
            "#..........................#".into(),
            "#.......................2..#".into(),
            "#..........................#".into(),
            "############################".into(),
        ],
        players: vec![
            PlayerSpec {
                name: "Overseer Ferrous".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 3_500,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "Idle Cupric".into(),
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
                x: 5,
                y: 3,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 6,
                y: 4,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 7,
                y: 3,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 5,
                y: 5,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 7,
                y: 5,
            },
            // A standing raid party: with the Scuttler quota already
            // met, the production chain reaches the air-wing purchase
            // instead of parking on the cheaper raider forever.
            UnitSpec {
                player: 0,
                kind: UnitKind::Scuttler,
                x: 10,
                y: 5,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Scuttler,
                x: 11,
                y: 5,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Scuttler,
                x: 10,
                y: 6,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Scuttler,
                x: 11,
                y: 6,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Harvester,
                x: 18,
                y: 5,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Harvester,
                x: 19,
                y: 5,
            },
        ],
        buildings: vec![
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Fabricator,
                x: 3,
                y: 7,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Reclaimer,
                x: 8,
                y: 7,
            },
            // The eye on the pocket: keeps the enemy economy showing.
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Array,
                x: 14,
                y: 4,
            },
        ],
        meta: None,
    };
    let mut state = scenario.build().expect("proving ground builds");
    let mut overseer = Brain::overseer(PlayerId(0), scenario.seed);

    let mut extractor_seen = false;
    let mut airworks_seen = false;
    let mut crucible_seen = false;
    let mut tier_seen = false;
    let mut warden_seen = false;
    let mut wing_seen = false;
    for _ in 0..14_000u32 {
        let commands = overseer.act(&state);
        // The closed tree, pinned at the command source: the Scuttler
        // homes at the Foundry and the wings at the Airworks — never
        // the transitional Fabricator roster.
        for command in &commands {
            if let Command::Train { building, kind } = command.command {
                let producer = state.building(building).map(|b| b.kind);
                match kind {
                    UnitKind::Scuttler => assert_eq!(
                        producer,
                        Some(BuildingKind::Foundry),
                        "a Scuttler order left for a {producer:?}"
                    ),
                    UnitKind::Buzzard => assert_eq!(
                        producer,
                        Some(BuildingKind::Airworks),
                        "a wing order left for a {producer:?}"
                    ),
                    _ => {}
                }
            }
        }
        state.tick(&commands);
        for b in state.buildings() {
            match b.kind {
                BuildingKind::Extractor => extractor_seen = true,
                BuildingKind::Airworks => airworks_seen = true,
                BuildingKind::Crucible => crucible_seen = true,
                _ => {}
            }
            if b.tier > 0 {
                tier_seen = true;
            }
        }
        if state.units().iter().any(|u| u.kind == UnitKind::Warden) {
            warden_seen = true;
        }
        // A live Buzzard proves the Airworks queue was accepted end to
        // end, not merely emitted.
        if state
            .units()
            .iter()
            .any(|u| u.player == PlayerId(0) && u.kind == UnitKind::Buzzard)
        {
            wing_seen = true;
        }
        if extractor_seen && airworks_seen && crucible_seen && tier_seen && warden_seen && wing_seen
        {
            return;
        }
    }
    panic!(
        "the Overseer left rungs unclimbed: extractor {extractor_seen}, airworks \
         {airworks_seen}, crucible {crucible_seen}, tier {tier_seen}, warden {warden_seen}, \
         wing {wing_seen}"
    );
}
