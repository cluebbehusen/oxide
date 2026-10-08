use super::*;

#[test]
fn a_reachable_goal_serializes_like_the_tile_it_replaced() {
    let tile = TilePos::new(7, -3);
    let mut goal = Goal::at(tile);
    goal.settle_for(tile);
    assert_eq!(goal.endpoint, None);
    assert_eq!(
        chassis::hash::state_hash(&goal),
        chassis::hash::state_hash(&tile)
    );
    assert_eq!(
        serde_json::to_string(&goal).unwrap(),
        serde_json::to_string(&tile).unwrap()
    );
    let parsed: Goal = serde_json::from_str(r#"{"x":7,"y":-3}"#).unwrap();
    assert_eq!(parsed, goal);
    assert_eq!(Goal::from(tile), goal);
    assert_eq!(Goal::slotted(tile, tile), goal);
}

#[test]
fn an_endpoint_is_kept_only_when_it_differs_from_the_target() {
    let mut goal = Goal::at(TilePos::new(4, 4));
    assert!(!goal.short());
    goal.settle_for(TilePos::new(2, 4));
    assert_eq!(goal.endpoint, Some(TilePos::new(2, 4)));
    assert_eq!(goal.destination(), TilePos::new(2, 4));
    assert_eq!(goal.target(), TilePos::new(4, 4));
    assert!(goal.short());
    assert!(goal.canonical());
    goal.settle_for(TilePos::new(4, 4));
    assert_eq!(goal.endpoint, None);
    assert_eq!(goal.destination(), goal.tile());
    goal.endpoint = Some(goal.tile());
    assert!(!goal.canonical());
}

#[test]
fn a_slot_redirects_the_target_but_keeps_the_clicked_tile() {
    let tile = TilePos::new(10, 5);
    let slot = TilePos::new(11, 5);
    let mut goal = Goal::slotted(tile, slot);
    assert_eq!(goal.aim, Aim::Slot(slot));
    assert_eq!(goal.tile(), tile);
    assert_eq!(goal.target(), slot);
    assert_eq!(goal.destination(), slot);
    assert!(goal.canonical());
    goal.settle_for(slot);
    assert_eq!(goal.endpoint, None, "the slot itself is no endpoint");
    goal.settle_for(tile);
    assert_eq!(goal.endpoint, Some(tile), "the clicked tile can be one");
    assert!(goal.canonical());

    let pending = Goal::pending(tile, 3, true);
    assert!(pending.is_pending());
    assert_eq!(pending.target(), tile);
    let json = serde_json::to_string(&pending).unwrap();
    assert_eq!(
        json,
        r#"{"x":10,"y":5,"aim":{"pending":{"rank":3,"reverse":true}}}"#
    );
    assert_eq!(serde_json::from_str::<Goal>(&json).unwrap(), pending);
    let json = serde_json::to_string(&Goal::slotted(tile, slot)).unwrap();
    assert_eq!(json, r#"{"x":10,"y":5,"aim":{"slot":{"x":11,"y":5}}}"#);
}

#[test]
fn a_slot_on_the_clicked_tile_is_not_canonical() {
    let tile = TilePos::new(3, 3);
    let goal = Goal {
        aim: Aim::Slot(tile),
        ..Goal::at(tile)
    };
    assert!(!goal.canonical());
    let goal = Goal {
        endpoint: Some(TilePos::new(4, 3)),
        ..Goal::slotted(tile, TilePos::new(4, 3))
    };
    assert!(!goal.canonical(), "an endpoint on the slot");
}

#[test]
fn adopting_a_reissue_keeps_the_endpoint_only_for_the_same_target() {
    let tile = TilePos::new(8, 8);
    let slot = TilePos::new(9, 8);
    let mut goal = Goal::slotted(tile, slot);
    goal.settle_for(TilePos::new(2, 2));
    let before = goal;

    goal.adopt(Goal::slotted(tile, slot));
    assert_eq!(goal, before, "the same slot keeps its endpoint");

    goal.adopt(Goal::pending(tile, 1, false));
    assert_eq!(
        goal.aim,
        Aim::Pending {
            rank: 1,
            reverse: false
        }
    );
    assert_eq!(goal.endpoint, None, "a new target resolves afresh");

    let mut pending = Goal::pending(tile, 0, false);
    pending.settle_for(TilePos::new(2, 2));
    pending.adopt(Goal::pending(tile, 4, true));
    assert_eq!(
        pending.aim,
        Aim::Pending {
            rank: 4,
            reverse: true
        }
    );
    assert_eq!(pending.endpoint, Some(TilePos::new(2, 2)));
}
