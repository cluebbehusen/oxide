//! One dispatcher over every debug-session kind.
//!
//! Three servers answer this protocol — the live shell, the shell's
//! read-only replay viewer, and the windowless `oxide-driver session` —
//! and the request space splits by capability, not by server:
//!
//! * **Shared** — the state reads ([`Request::Status`],
//!   [`Request::QueryState`], [`Request::QueryFogView`],
//!   [`Request::StateHash`]) and the driven clock
//!   ([`Request::AdvanceTicks`], [`Request::PresentTicks`],
//!   [`Request::Pause`], [`Request::Resume`], [`Request::SetSpeed`]).
//!   Every session kind answers these through [`DebugSession`], and
//!   [`dispatch_shared`] is the single implementation of the
//!   request-to-reply plumbing. A session without a wall clock refuses
//!   the pause family from inside its clock methods.
//! * **Window-shaped** — camera, UI, input injection, screenshots, the
//!   overlay, and native-frame profiling. [`dispatch_shared`] returns
//!   `None`; the shell answers them against the screen the window shows,
//!   the headless session refuses them in words.
//! * **Mutating** — commands and scenario/replay swaps. Also `None`
//!   here: live and headless sessions implement them, the replay viewer
//!   refuses them wholesale.
//!
//! Splitting this way is what retired the twin dispatchers whose
//! near-identical arms kept drifting apart — `PresentTicks` shipped
//! hand-copied twice on the day it was born.

use crate::{
    AdvancedView, FogView, HashView, PresentedView, Reply, Request, StateView, StatusView,
};
use oxide_sim::State;

/// A world the debug protocol can be served against: the live shell's
/// `Game`, the replay viewer, or the headless session. Implementations
/// keep their own clock semantics — the viewer's `advance` seeks its
/// record instead of simulating, and may run fewer ticks than asked
/// near the record's end — but every reply shape is the trait's.
pub trait DebugSession {
    /// Transport identity: tick, clock stance, scenario, result.
    fn status(&self) -> StatusView;

    /// The world every state-shaped request reads.
    fn state(&self) -> &State;

    /// Runs up to `ticks` sim ticks (already capped by the dispatcher)
    /// without presentation, reporting what actually ran.
    fn advance(&mut self, ticks: u64) -> AdvancedView;

    /// Runs up to `ticks` sim ticks (already capped) while retaining
    /// presentation, returning the interval's events.
    fn present(&mut self, ticks: u64) -> PresentedView;

    /// Stops or resumes this session's wall clock; `Err` when the
    /// session has no wall clock to stop.
    fn set_paused(&mut self, paused: bool) -> Result<(), String>;

    /// Scales this session's wall clock; `Err` when out of range or the
    /// session has no wall clock. Validate with [`check_speed`].
    fn set_speed(&mut self, multiplier: f64) -> Result<(), String>;
}

/// Validates a wall-clock speed multiplier — one range, one refusal
/// message, however many sessions carry a clock.
pub fn check_speed(multiplier: f64) -> Result<(), String> {
    if multiplier.is_finite() && (0.05..=64.0).contains(&multiplier) {
        Ok(())
    } else {
        Err(format!("speed multiplier {multiplier} outside 0.05..=64"))
    }
}

/// Answers a shared request against any session. Returns `None` for the
/// window-shaped and mutating requests, which stay with the caller —
/// that boundary IS the capability split documented on this module.
pub fn dispatch_shared(
    session: &mut dyn DebugSession,
    request: &Request,
) -> Option<Result<Reply, String>> {
    Some(match request {
        Request::Status => Ok(Reply::Status(session.status())),
        Request::QueryState { filter } => {
            Ok(Reply::State(StateView::capture(session.state(), *filter)))
        }
        Request::QueryFogView { player } => {
            if (player.0 as usize) < session.state().players().len() {
                Ok(Reply::Fog(FogView::capture(session.state(), *player)))
            } else {
                Err(format!("no such player {player}"))
            }
        }
        Request::StateHash => Ok(Reply::Hash(HashView {
            tick: session.state().current_tick(),
            hash: crate::hash_hex(session.state().hash()),
        })),
        Request::AdvanceTicks { ticks } => Ok(Reply::Advanced(
            session.advance((*ticks).min(crate::MAX_ADVANCE_TICKS)),
        )),
        Request::PresentTicks { ticks } => Ok(Reply::Presented(
            session.present((*ticks).min(crate::MAX_PRESENT_TICKS)),
        )),
        Request::Pause => session.set_paused(true).map(|()| Reply::Ok),
        Request::Resume => session.set_paused(false).map(|()| Reply::Ok),
        Request::SetSpeed { multiplier } => session.set_speed(*multiplier).map(|()| Reply::Ok),
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
