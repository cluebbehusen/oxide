//! The seat's own order failures, which the host buffers between decisions.

use chassis::fx::Vec2Fx;
use oxide_sim::command::RejectReason;
use oxide_sim::{Event, PlayerId, StallReason, UnitId};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Events a buffer holds, the oldest dropped first beyond it: a computation
/// bound that normal play stays under, since the buffer keeps one stall per
/// unit and reason and one rejection per reason.
const CAP: usize = 1_024;

/// One of the seat's own order failures, as the simulation reported it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum OwnEvent {
    /// A unit gave up its order.
    OrderStalled {
        /// The unit that gave up.
        unit: UnitId,
        /// Where it stood when it gave up.
        pos: Vec2Fx,
        /// Why.
        reason: StallReason,
    },
    /// A command was dropped. The event names no command, so it cannot be
    /// tied to a particular one.
    CommandRejected {
        /// Why.
        reason: RejectReason,
    },
}

impl OwnEvent {
    /// Whether `other` reports the same failure again: the same unit stalling
    /// for the same reason, or a rejection for the same reason.
    fn repeats(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::OrderStalled { unit, reason, .. },
                Self::OrderStalled {
                    unit: again,
                    reason: why,
                    ..
                },
            ) => unit == again && reason == why,
            (Self::CommandRejected { reason }, Self::CommandRejected { reason: why }) => {
                reason == why
            }
            _ => false,
        }
    }

    fn of(player: PlayerId, event: &Event) -> Option<Self> {
        match *event {
            Event::OrderStalled {
                unit,
                player: owner,
                pos,
                reason,
            } if owner == player => Some(Self::OrderStalled { unit, pos, reason }),
            Event::CommandRejected {
                player: issuer,
                reason,
            } if issuer == player => Some(Self::CommandRejected { reason }),
            _ => None,
        }
    }
}

/// A bounded, ordered buffer of one seat's own events. The host keeps it
/// beside the controller and records every tick; the seat's next decision
/// consumes it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<OwnEvent>", into = "Vec<OwnEvent>")]
pub struct OwnEvents(VecDeque<OwnEvent>);

impl OwnEvents {
    /// Appends `player`'s own stalls and rejections from one tick's events,
    /// in report order. A repeat of a stall of the same unit for the same
    /// reason, or of a rejection for the same reason, replaces the earlier
    /// one in its place.
    pub fn record(&mut self, player: PlayerId, events: &[Event]) {
        for event in events
            .iter()
            .filter_map(|event| OwnEvent::of(player, event))
        {
            if let Some(held) = self.0.iter_mut().find(|held| held.repeats(&event)) {
                *held = event;
                continue;
            }
            if self.0.len() == CAP {
                self.0.pop_front();
            }
            self.0.push_back(event);
        }
    }

    pub(crate) fn take(&mut self) -> Vec<OwnEvent> {
        std::mem::take(&mut self.0).into()
    }
}

impl TryFrom<Vec<OwnEvent>> for OwnEvents {
    type Error = String;

    fn try_from(events: Vec<OwnEvent>) -> Result<Self, Self::Error> {
        if events.len() > CAP {
            return Err(format!("own-event buffer exceeds {CAP} events"));
        }
        Ok(Self(events.into()))
    }
}

impl From<OwnEvents> for Vec<OwnEvent> {
    fn from(events: OwnEvents) -> Self {
        events.0.into()
    }
}

#[cfg(test)]
mod tests {
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
        let tick: Vec<Event> = (0..CAP as u32 + 6).map(|unit| stalled(0, unit)).collect();
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
        assert_eq!(kept, (6..CAP as u32 + 6).collect::<Vec<_>>());

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
}
