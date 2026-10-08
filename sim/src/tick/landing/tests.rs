use crate::Faction;
use crate::command::Command;
use crate::ids::PlayerId;
use crate::scenario::{PlayerSpec, Scenario, ScenarioMode, UnitSpec};
use crate::stats::UnitKind;
use crate::{PlayerCommand, State};
use chassis::fx::Fx;
use chassis::grid::TilePos;

fn arena() -> State {
    let mut rows = vec!["########################".to_string()];
    for _ in 0..14 {
        rows.push("#......................#".to_string());
    }
    rows.push("########################".to_string());
    rows[1] = "#1.....................#".to_string();
    rows[13] = "#....................2.#".to_string();
    Scenario {
        mode: ScenarioMode::Match,
        name: "mirror".into(),
        seed: 3,
        map: rows,
        players: vec![
            PlayerSpec {
                name: "Ferrous".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "Cupric".into(),
                faction: Faction::Cupric,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
        ],
        units: vec![
            UnitSpec {
                player: 0,
                kind: UnitKind::Condor,
                x: 6,
                y: 5,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Condor,
                x: 17,
                y: 10,
            },
        ],
        buildings: Vec::new(),
        meta: None,
    }
    .build()
    .expect("the mirror arena builds")
}

#[test]
fn a_handoff_pad_is_chosen_only_from_ground_its_side_has_explored() {
    let state = arena();
    let condor = &state.units[0];
    let player = condor.player;
    let vision = state.vision(player);
    let around = TilePos::new(13, 5);
    assert!(
        !vision.explored(around),
        "premise: the destination lies past the fog line"
    );
    let pick = |explored_by| {
        super::nearest_landable(
            &state,
            condor.kind.stats(),
            condor.id,
            around,
            condor.pos,
            condor.heading,
            3,
            None,
            super::Pick::Nearest,
            explored_by,
        )
    };
    assert_eq!(
        pick(None),
        Some(around),
        "premise: unfiltered, the destination itself is the nearest pad"
    );
    let pad = pick(Some(player)).expect("explored ground lies within reach");
    assert!(
        vision.explored(pad),
        "a handoff never picks a pad its side has not seen"
    );
}

/// Two aircraft placed as half-turn images of each other, given
/// half-turn images of the same landing, must fly half-turn images of
/// the same approach and park at the same moment: every choice on the
/// way down is taken in a heading-relative frame.
#[test]
fn a_landing_mirrors_exactly_under_a_map_half_turn() {
    let mut state = arena();
    let (width, height) = (Fx::from_num(24), Fx::from_num(16));
    let a = state.units[0].id;
    let b = state.units[1].id;
    state.units[0].heading = 20;
    state.units[1].heading = 20u8.wrapping_add(128);
    let mirror = |p: chassis::fx::Vec2Fx| chassis::fx::Vec2Fx::new(width - p.x, height - p.y);
    assert_eq!(
        state.units[1].pos,
        mirror(state.units[0].pos),
        "premise: mirrored starts"
    );
    state.tick(&[
        PlayerCommand {
            player: PlayerId(0),
            command: Command::Run {
                units: vec![a],
                goal: TilePos::new(14, 7),
                queue: false,
            },
        },
        PlayerCommand {
            player: PlayerId(0),
            command: Command::Run {
                units: vec![b],
                goal: TilePos::new(23 - 14, 15 - 7),
                queue: false,
            },
        },
    ]);
    let mut landed_at = None;
    for _ in 0..1_500 {
        state.tick(&[]);
        let (ua, ub) = (state.unit(a).unwrap(), state.unit(b).unwrap());
        assert_eq!(
            ub.pos,
            mirror(ua.pos),
            "positions diverged at tick {}",
            state.tick
        );
        assert_eq!(
            ub.heading,
            ua.heading.wrapping_add(128),
            "headings diverged"
        );
        assert_eq!(ub.landed, ua.landed, "one landed before the other");
        if ua.landed {
            landed_at = Some(state.tick);
            break;
        }
    }
    assert!(landed_at.is_some(), "neither aircraft landed");
}
