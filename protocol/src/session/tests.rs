use super::*;
use crate::hash_hex;

/// The smallest session: a bare sim state and a clock that answers,
/// enough to pin the dispatcher's own behavior.
struct Bare {
    state: State,
    paused: bool,
    speed: f64,
}

impl DebugSession for Bare {
    fn status(&self) -> StatusView {
        StatusView {
            tick: self.state.current_tick(),
            paused: self.paused,
            speed: self.speed,
            scenario: "bare".to_string(),
            sim_version: 1,
            result: self.state.result(),
            recorded_commands: 0,
        }
    }

    fn state(&self) -> &State {
        &self.state
    }

    // The clock methods echo what the dispatcher hands them, so the
    // double pins the dispatcher's capping, not sim behavior.
    fn advance(&mut self, ticks: u64) -> AdvancedView {
        AdvancedView {
            ticks,
            tick: self.state.current_tick(),
            hash: hash_hex(self.state.hash()),
        }
    }

    fn present(&mut self, ticks: u64) -> PresentedView {
        PresentedView {
            ticks,
            tick: self.state.current_tick(),
            hash: hash_hex(self.state.hash()),
            events: Vec::new(),
        }
    }

    fn set_paused(&mut self, paused: bool) -> Result<(), String> {
        self.paused = paused;
        Ok(())
    }

    fn set_speed(&mut self, multiplier: f64) -> Result<(), String> {
        check_speed(multiplier)?;
        self.speed = multiplier;
        Ok(())
    }
}

fn bare() -> Bare {
    Bare {
        state: oxide_sim::Scenario::skirmish().build().expect("builds"),
        paused: false,
        speed: 1.0,
    }
}

#[test]
fn the_capability_split_is_exactly_nine_shared_requests() {
    let mut session = bare();
    let shared = [
        Request::Status,
        Request::QueryState {
            filter: crate::StateFilter::default(),
        },
        Request::QueryFogView {
            player: oxide_sim::PlayerId(0),
        },
        Request::StateHash,
        Request::AdvanceTicks { ticks: 1 },
        Request::PresentTicks { ticks: 1 },
        Request::Pause,
        Request::Resume,
        Request::SetSpeed { multiplier: 2.0 },
    ];
    for request in shared {
        assert!(
            dispatch_shared(&mut session, &request).is_some(),
            "{request:?} belongs to the shared surface"
        );
    }
    // Window-shaped and mutating requests stay with the caller, which
    // implements or refuses them.
    let unshared = [
        Request::QueryCamera,
        Request::QueryUi,
        Request::QueryPerformance { reset: false },
        Request::BeginPerformanceWindow {
            from_tick: 10,
            to_tick: 20,
        },
        Request::InjectEvent {
            event: crate::RawEvent::KeyDown {
                key: crate::Key::Space,
            },
        },
        Request::Screenshot { path: None },
        Request::ToggleOverlay,
        Request::SendCommand {
            player: oxide_sim::PlayerId(0),
            command: oxide_sim::Command::Stop { units: vec![] },
        },
        Request::LoadScenario {
            path: "x.json".to_string(),
        },
        Request::LoadReplay {
            path: "x.json".to_string(),
        },
        Request::SaveReplay {
            path: "x.json".to_string(),
        },
    ];
    for request in unshared {
        assert!(
            dispatch_shared(&mut session, &request).is_none(),
            "{request:?} is not the dispatcher's to answer"
        );
    }
}

#[test]
fn the_dispatcher_caps_ticks_and_validates_seats() {
    let mut session = bare();
    let Some(Ok(Reply::Advanced(view))) = dispatch_shared(
        &mut session,
        &Request::AdvanceTicks {
            ticks: crate::MAX_ADVANCE_TICKS.saturating_add(500),
        },
    ) else {
        panic!("advance answers");
    };
    assert_eq!(view.ticks, crate::MAX_ADVANCE_TICKS, "requests are capped");
    let missing = dispatch_shared(
        &mut session,
        &Request::QueryFogView {
            player: oxide_sim::PlayerId(9),
        },
    )
    .expect("fog is shared")
    .expect_err("seat 9 does not exist");
    assert!(missing.contains("no such player"));
    let too_fast = dispatch_shared(&mut session, &Request::SetSpeed { multiplier: 1000.0 })
        .expect("speed is shared")
        .expect_err("1000x is out of range");
    assert!(too_fast.contains("outside 0.05..=64"));
}

#[test]
fn speed_validation_uses_closed_finite_bounds() {
    for accepted in [0.05, 1.0, 64.0] {
        check_speed(accepted).expect("boundary is legal");
    }
    for rejected in [
        0.0,
        0.049,
        64.001,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
    ] {
        assert!(check_speed(rejected).is_err(), "accepted {rejected:?}");
    }
}
