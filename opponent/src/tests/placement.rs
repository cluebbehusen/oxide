use super::*;
use crate::placement::{Allowed, Refusal, check};

/// A west base with a frame, a scrap node, a sealed two-by-two pocket and a
/// visible hostile Sentinel; the east seat stays out of sight.
const YARD: [&str; 14] = [
    "####################",
    "#..................#",
    "#.......E..........#",
    "#..................#",
    "#..1.......####....#",
    "#..........#..#....#",
    "#..........#..#....#",
    "#..........####....#",
    "#.......s..........#",
    "#..................#",
    "#..................#",
    "#................2.#",
    "#..................#",
    "####################",
];

fn yard() -> Scenario {
    let mut scenario = arena(0);
    scenario.map = YARD.map(str::to_owned).to_vec();
    scenario.units = vec![
        harvester(0, 5, 6),
        harvester(0, 6, 6),
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 10,
            y: 6,
        },
        UnitSpec {
            player: 1,
            kind: UnitKind::Sentinel,
            x: 6,
            y: 9,
        },
    ];
    scenario
}

fn verdict(state: &State, kind: BuildingKind, x: i32, y: i32) -> Result<Allowed, Refusal> {
    let observation = ObservationData::fog_honest(state, PlayerId(0));
    check(&observation, kind, TilePos::new(x, y), &[])
}

#[test]
fn each_known_obstacle_refuses_a_footprint_for_its_own_reason() {
    let state = yard().build().unwrap();
    let fabricator = BuildingKind::Fabricator;
    assert_eq!(
        verdict(&state, BuildingKind::Airworks, 6, 11),
        Err(Refusal::Prerequisite)
    );
    assert_eq!(
        verdict(&state, fabricator, 15, 10),
        Err(Refusal::Unexplored)
    );
    assert_eq!(verdict(&state, fabricator, 8, 2), Err(Refusal::Frame));
    assert_eq!(
        verdict(&state, BuildingKind::Extractor, 5, 1),
        Err(Refusal::Frame)
    );
    assert!(verdict(&state, BuildingKind::Extractor, 8, 2).is_ok());
    assert_eq!(
        verdict(&state, fabricator, 8, 7),
        Err(Refusal::Terrain),
        "scrap"
    );
    assert_eq!(
        verdict(&state, fabricator, 10, 3),
        Err(Refusal::Terrain),
        "rock"
    );
    assert_eq!(verdict(&state, fabricator, 4, 5), Err(Refusal::Occupied));
    assert_eq!(verdict(&state, fabricator, 5, 3), Err(Refusal::Crowded));
    assert_eq!(verdict(&state, fabricator, 6, 9), Err(Refusal::HostileUnit));
    assert_eq!(verdict(&state, fabricator, 12, 5), Err(Refusal::NoEgress));
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    let planned = [(fabricator, TilePos::new(5, 1))];
    assert_eq!(
        check(&observation, fabricator, TilePos::new(6, 1), &planned),
        Err(Refusal::Claimed)
    );
    assert_eq!(
        verdict(&state, fabricator, 6, 1),
        Ok(Allowed { defer: false })
    );
}

#[test]
fn an_accepted_footprint_is_one_the_simulation_accepts() {
    let state = yard().build().unwrap();
    let mut accepted = 0;
    for y in 0..14 {
        for x in 0..20 {
            for kind in [BuildingKind::Fabricator, BuildingKind::Reclaimer] {
                if verdict(&state, kind, x, y).is_ok() {
                    accepted += 1;
                    let anchor = TilePos::new(x, y);
                    assert_eq!(
                        state.place_intent_refusal(PlayerId(0), kind, anchor),
                        None,
                        "{kind:?} at {anchor:?}"
                    );
                }
            }
        }
    }
    assert!(accepted > 20, "premise: open ground is accepted");
}

#[test]
fn hidden_enemies_never_change_a_verdict() {
    let quiet = yard().build().unwrap();
    let sight = ObservationData::fog_honest(&quiet, PlayerId(0));
    let unseen = (1..19)
        .flat_map(|x| (1..10).map(move |y| TilePos::new(x, y)))
        .find(|tile| !sight.visible(*tile) && quiet.map().terrain_passable(*tile))
        .expect("premise: part of the yard is out of sight");
    let mut hidden = yard();
    hidden.units.push(UnitSpec {
        player: 1,
        kind: UnitKind::Sentinel,
        x: unseen.x,
        y: unseen.y,
    });
    let hidden = hidden.build().unwrap();
    for y in 0..14 {
        for x in 0..20 {
            assert_eq!(
                verdict(&quiet, BuildingKind::Fabricator, x, y),
                verdict(&hidden, BuildingKind::Fabricator, x, y),
                "({x}, {y})"
            );
        }
    }
}
