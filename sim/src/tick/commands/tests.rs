use super::*;
use crate::state::Leash;
use chassis::grid::TilePos;

#[test]
fn an_append_a_full_queue_refuses_leaves_the_unit_untouched() {
    let mut state = crate::Scenario::skirmish().build().unwrap();
    let tick = state.tick;
    let unit = &mut state.units[0];
    let busy = Order::Harvest {
        node: TilePos::new(5, 5),
        anchor: TilePos::new(5, 5),
        retiring: false,
    };
    unit.order = busy;
    unit.queue = std::iter::repeat_n(busy, ORDER_QUEUE_CAP).collect();
    unit.leash = Some(Leash {
        anchor: unit.tile(),
        patience: 1,
        cooldown: 2,
    });
    unit.settled = 5;
    unit.worker_mut().danger_retry_at = Some(tick + 3);
    let (id, player) = (unit.id, unit.player);
    let before = state.units[0].clone();
    let mut events = Vec::new();
    apply(
        &mut state,
        &[PlayerCommand {
            player,
            command: crate::Command::Run {
                units: vec![id],
                goal: TilePos::new(8, 8),
                queue: true,
            },
        }],
        &mut events,
    );
    assert!(events.iter().any(|event| matches!(
        event,
        Event::CommandRejected {
            reason: RejectReason::QueueFull,
            ..
        }
    )));
    assert_eq!(state.units[0], before);
}
