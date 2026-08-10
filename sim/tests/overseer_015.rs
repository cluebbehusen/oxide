//! The Overseer: the scripted commander with the 0.15 tree switched
//! on. Given a teched base, a rich bank, and a known derelict frame, it
//! must restore the Extractor, raise the Airworks, and lift a works a
//! tier — the acts the frozen ladder cannot take.

use oxide_sim::bot::Brain;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::stats::BuildingKind;
use oxide_sim::{Faction, PlayerId, Scenario, UnitKind};

#[test]
fn the_overseer_climbs_the_whole_tree() {
    let scenario = Scenario {
        name: "overseer-proving-ground".into(),
        seed: 23,
        map: vec![
            "############################".into(),
            "#1.....................ss..#".into(),
            "#......................ss..#".into(),
            "#..........E...............#".into(),
            "#..........................#".into(),
            "#..ss......................#".into(),
            "#..ss......................#".into(),
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
                scrap: 2_000,
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
    for _ in 0..14_000u32 {
        let commands = overseer.act(&state);
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
        if extractor_seen && airworks_seen && crucible_seen && tier_seen && warden_seen {
            return;
        }
    }
    panic!(
        "the Overseer left rungs unclimbed: extractor {extractor_seen}, airworks \
         {airworks_seen}, crucible {crucible_seen}, tier {tier_seen}, warden {warden_seen}"
    );
}
