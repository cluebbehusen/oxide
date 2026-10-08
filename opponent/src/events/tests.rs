use super::*;

fn stalled(player: u8, unit: u32) -> Event {
    Event::OrderStalled {
        unit: UnitId(unit),
        player: PlayerId(player),
        pos: Vec2Fx::default(),
        reason: StallReason::NoRoute,
    }
}

fn rejected(player: u8, reason: RejectReason) -> Event {
    Event::CommandRejected {
        player: PlayerId(player),
        reason,
    }
}

#[test]
fn a_buffer_keeps_only_the_seats_own_failures_in_order() {
    let mut events = OwnEvents::default();
    events.record(
        PlayerId(1),
        &[
            rejected(0, RejectReason::NotEnoughScrap),
            stalled(1, 7),
            Event::ScrapDeposited {
                unit: oxide_sim::UnitId(0),
                foundry: oxide_sim::BuildingId(0),
                player: PlayerId(1),
                amount: 5,
            },
            stalled(0, 3),
            rejected(1, RejectReason::QueueFull),
        ],
    );
    events.record(PlayerId(1), &[rejected(1, RejectReason::BadSite)]);
    assert_eq!(
        events.take(),
        [
            OwnEvent::OrderStalled {
                unit: UnitId(7),
                pos: Vec2Fx::default(),
                reason: StallReason::NoRoute,
            },
            OwnEvent::CommandRejected {
                reason: RejectReason::QueueFull,
            },
            OwnEvent::CommandRejected {
                reason: RejectReason::BadSite,
            },
        ]
    );
    assert_eq!(events, OwnEvents::default());
}

#[test]
fn repeats_of_one_failure_keep_its_first_place() {
    let mut events = OwnEvents::default();
    let mut tick: Vec<Event> = (0..40).map(|_| stalled(0, 7)).collect();
    tick.insert(1, stalled(0, 8));
    tick.extend([
        rejected(0, RejectReason::BadSite),
        rejected(0, RejectReason::QueueFull),
        rejected(0, RejectReason::BadSite),
    ]);
    events.record(PlayerId(0), &tick);
    let wide: Vec<Event> = (100..200).map(|unit| stalled(0, unit)).collect();
    events.record(PlayerId(0), &wide);
    let kept = events.take();
    assert_eq!(kept.len(), 4 + wide.len(), "{kept:?}");
    let stalled_unit = |event: &OwnEvent| match event {
        OwnEvent::OrderStalled { unit, .. } => Some(unit.0),
        OwnEvent::CommandRejected { .. } => None,
    };
    assert_eq!(stalled_unit(&kept[0]), Some(7));
    assert_eq!(stalled_unit(&kept[1]), Some(8));
    assert_eq!(
        kept[2..4],
        [
            OwnEvent::CommandRejected {
                reason: RejectReason::BadSite
            },
            OwnEvent::CommandRejected {
                reason: RejectReason::QueueFull
            },
        ]
    );
}

#[test]
fn a_full_buffer_drops_its_oldest_events_and_refuses_to_load_over_its_cap() {
    let mut events = OwnEvents::default();
    let tick: Vec<Event> = (0..u32::try_from(CAP).unwrap() + 6)
        .map(|unit| stalled(0, unit))
        .collect();
    events.record(PlayerId(0), &tick);
    let json = serde_json::to_value(&events).unwrap();
    let kept: Vec<u32> = events
        .take()
        .into_iter()
        .map(|event| match event {
            OwnEvent::OrderStalled { unit, .. } => unit.0,
            OwnEvent::CommandRejected { .. } => unreachable!(),
        })
        .collect();
    assert_eq!(
        kept,
        (6..u32::try_from(CAP).unwrap() + 6).collect::<Vec<_>>()
    );

    assert_eq!(json.as_array().unwrap().len(), CAP);
    assert_eq!(
        (&json[0]["event"], &json[0]["unit"], &json[0]["reason"]),
        (
            &serde_json::json!("order_stalled"),
            &serde_json::json!(6),
            &serde_json::json!("no_route")
        )
    );
    let restored: OwnEvents = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(serde_json::to_value(&restored).unwrap(), json);
    let mut over = json;
    over.as_array_mut()
        .unwrap()
        .push(serde_json::json!({"event": "command_rejected", "reason": "bad_site"}));
    assert!(serde_json::from_value::<OwnEvents>(over).is_err());
}
