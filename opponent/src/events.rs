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
mod tests;
