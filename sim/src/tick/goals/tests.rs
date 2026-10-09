use super::*;
use crate::scenario::{PlayerSpec, ScenarioMode, UnitSpec};
use crate::state::Faction;
use crate::stats::UnitKind;
use crate::{Command, PlayerCommand, Scenario};

/// Rock, peaks and a pit around open ground, small enough for a central
/// Kestrel to see every tile.
const FIELD: [&str; 9] = [
    "...........",
    ".##.....^^.",
    ".##.....^^.",
    "....#......",
    "...###.....",
    "....#......",
    "...........",
    ".~~.....##.",
    "...........",
];

fn world(map: &[&str], units: &[(UnitKind, i32, i32)]) -> State {
    Scenario {
        mode: ScenarioMode::Sandbox,
        name: "goals".into(),
        seed: 5,
        map: map.iter().map(|row| (*row).to_owned()).collect(),
        players: vec![PlayerSpec {
            name: "p0".into(),
            faction: Faction::Ferrous,
            team: None,
            scrap: 0,
            bot: false,
            bot_config: None,
        }],
        units: units
            .iter()
            .map(|&(kind, x, y)| UnitSpec {
                player: 0,
                kind,
                x,
                y,
            })
            .collect(),
        buildings: Vec::new(),
        meta: None,
    }
    .build()
    .expect("goals fixture builds")
}

fn tiles(state: &State) -> impl Iterator<Item = TilePos> {
    let (w, h) = (state.map.width(), state.map.height());
    (0..h).flat_map(move |y| (0..w).map(move |x| TilePos::new(x, y)))
}

/// The reference tiles for a group of `count`: the snapped center, then
/// the first `count` spread slots around it.
fn snap_and_spread(
    state: &State,
    clicked: TilePos,
    domain: Domain,
    reverse: bool,
    count: usize,
) -> Option<Vec<TilePos>> {
    let center = group_domain_goal(state, clicked, domain, reverse)?;
    Some(spread_goals_by(center, count, reverse, |t| {
        state.passable_for(domain, t)
    }))
}

#[test]
fn explored_slots_are_the_snap_and_spread_tiles_for_every_group_size() {
    let state = world(&FIELD, &[(UnitKind::Kestrel, 5, 4)]);
    for clicked in tiles(&state) {
        for domain in [Domain::Ground, Domain::Air] {
            for reverse in [false, true] {
                let slots = slots(&state, clicked, domain, reverse);
                for count in [1, 2, 5, 13, 40, MAX_SLOTS, MAX_SLOTS + 31] {
                    let expected = snap_and_spread(&state, clicked, domain, reverse, count)
                        .expect("premise: every click here has open ground near it");
                    for (rank, tile) in expected.into_iter().enumerate() {
                        let goal = resolved(clicked, &slots, rank);
                        assert_eq!(goal.target(), tile, "{clicked} {domain:?} {rank}");
                        assert_eq!(goal.tile(), clicked);
                        assert!(goal.canonical());
                    }
                }
            }
        }
    }
}

#[test]
fn a_click_with_no_open_ground_near_it_keeps_the_clicked_tile() {
    let mut map = vec!["#########"; 9];
    map[0] = "........#";
    let state = world(&map, &[(UnitKind::Sentinel, 0, 0)]);
    let clicked = TilePos::new(5, 6);
    assert_eq!(
        group_domain_goal(&state, clicked, Domain::Ground, false),
        None
    );
    assert!(slots(&state, clicked, Domain::Ground, false).is_empty());
    for rank in [0, 3, MAX_SLOTS] {
        assert_eq!(resolved(clicked, &[], rank), Goal::at(clicked));
    }
}

#[test]
fn a_rank_past_the_last_slot_shares_it() {
    let state = world(&FIELD, &[(UnitKind::Kestrel, 5, 4)]);
    let clicked = TilePos::new(5, 4);
    for reverse in [false, true] {
        let slots = slots(&state, clicked, Domain::Ground, reverse);
        assert_eq!(slots.len(), MAX_SLOTS);
        let last = *slots.last().expect("open ground");
        for rank in [MAX_SLOTS - 1, MAX_SLOTS, usize::from(u8::MAX), usize::MAX] {
            assert_eq!(resolved(clicked, &slots, rank).target(), last);
        }
    }
}

#[test]
fn a_group_command_on_explored_ground_takes_its_slots_at_issue() {
    let units = [
        (UnitKind::Sentinel, 0, 0),
        (UnitKind::Kestrel, 10, 8),
        (UnitKind::Sentinel, 0, 8),
        (UnitKind::Kestrel, 5, 8),
        (UnitKind::Sentinel, 6, 0),
        (UnitKind::Sentinel, 10, 0),
    ];
    let mut state = world(&FIELD, &units);
    let clicked = TilePos::new(4, 4);
    assert!(state.vision(PlayerId(0)).explored(clicked));
    let ids: Vec<UnitId> = state.units.iter().map(|u| u.id).collect();
    let command = PlayerCommand {
        player: PlayerId(0),
        command: Command::Run {
            units: ids.clone(),
            goal: clicked,
            queue: false,
        },
    };
    let before = state.clone();
    super::super::commands::apply(&mut state, &[command], &mut Vec::new());
    for domain in [Domain::Ground, Domain::Air] {
        let half: Vec<UnitId> = ids
            .iter()
            .copied()
            .filter(|id| before.unit(*id).unwrap().kind.stats().domain == domain)
            .collect();
        let reverse = spread_scan_reversed(&before, clicked, &half);
        let expected = snap_and_spread(&before, clicked, domain, reverse, half.len())
            .expect("open ground near the click");
        for (id, tile) in half.iter().zip(expected) {
            let Order::Run { goal } = state.unit(*id).unwrap().order else {
                panic!("unit {id} got a move");
            };
            assert_eq!(goal.tile(), clicked);
            assert_eq!(goal.target(), tile, "unit {id}");
            assert!(!goal.is_pending());
        }
    }
}

#[test]
fn exposure_resolves_every_pending_goal_a_unit_carries() {
    let mut state = world(&FIELD, &[(UnitKind::Sentinel, 5, 6)]);
    let explored = TilePos::new(4, 4);
    let unexplored = TilePos::new(40, 40);
    let reverse = true;
    let slots = slots(&state, explored, Domain::Ground, reverse);
    let unit = &mut state.units[0];
    unit.order = Order::Attack {
        target: crate::AttackTarget::Unit(unit.id),
        pursue: false,
        resume: Some(Goal::pending(explored, 2, reverse)),
    };
    let mut stale = Goal::pending(explored, 4, reverse);
    stale.endpoint = Some(TilePos::new(0, 0));
    unit.queue = [
        Order::Run { goal: stale },
        Order::Hunt {
            goal: Goal::pending(unexplored, 1, false),
        },
        Order::Advance {
            goal: Goal::pending(explored, 200, reverse),
        },
    ]
    .into();
    expose(&mut state);
    let unit = &state.units[0];
    let Order::Attack { resume, .. } = unit.order else {
        panic!("the engagement stands");
    };
    assert_eq!(resume, Some(Goal::slotted(explored, slots[2])));
    assert_eq!(
        unit.queue[0],
        Order::Run {
            goal: Goal::slotted(explored, slots[4])
        },
        "the endpoint resolved for the clicked tile is dropped"
    );
    assert_eq!(
        unit.queue[1],
        Order::Hunt {
            goal: Goal::pending(unexplored, 1, false)
        },
        "an unexplored click keeps waiting"
    );
    assert_eq!(
        unit.queue[2],
        Order::Advance {
            goal: Goal::slotted(explored, slots[MAX_SLOTS - 1])
        }
    );
}

#[test]
fn exposure_leaves_units_without_pending_goals_untouched() {
    let mut state = world(&FIELD, &[(UnitKind::Sentinel, 5, 6)]);
    let mut goal = Goal::slotted(TilePos::new(4, 4), TilePos::new(6, 6));
    goal.endpoint = Some(TilePos::new(6, 7));
    state.units[0].order = Order::Run { goal };
    let before = state.clone();
    expose(&mut state);
    assert_eq!(state, before);
}
