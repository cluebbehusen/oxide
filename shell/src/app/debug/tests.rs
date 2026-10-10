use super::*;

#[test]
fn local_request_guards_follow_screen_ownership() {
    let advance = Request::AdvanceTicks { ticks: 8 };
    let send = Request::SendCommand {
        player: oxide_sim::PlayerId(0),
        command: oxide_sim::Command::Surrender,
    };
    let camera = Request::QueryCamera;

    // The frozen final map refuses time and mutation before shared
    // dispatch can advance the hidden live match.
    assert_eq!(route(false, true, None, &advance), Route::RefuseFrozen);
    assert_eq!(route(false, true, None, &send), Route::RefuseFrozen);
    assert_eq!(route(false, true, None, &camera), Route::Local);

    // The read-only viewer bounces local mutation and answers
    // local reads.
    assert_eq!(route(true, false, None, &send), Route::RefuseViewer);
    assert_eq!(route(true, false, None, &camera), Route::Local);

    // The live screen answers everything else locally.
    assert_eq!(route(false, false, None, &send), Route::Local);
}

/// A LAN match refuses the clock and other seats before shared
/// dispatch, and only its host may pause.
#[test]
fn a_lan_match_refuses_its_clock_and_other_seats() {
    use crate::game::network::NetRole;
    let seat = oxide_sim::PlayerId(1);
    let host = Some((NetRole::Host, seat));
    let client = Some((NetRole::Client, seat));
    let send = |player| Request::SendCommand {
        player: oxide_sim::PlayerId(player),
        command: oxide_sim::Command::Surrender,
    };
    for net in [host, client] {
        for refused in [
            Request::AdvanceTicks { ticks: 1 },
            Request::PresentTicks { ticks: 1 },
            Request::SetSpeed { multiplier: 2.0 },
            send(0),
        ] {
            assert_eq!(
                route(false, false, net, &refused),
                Route::RefuseLockstep,
                "{refused:?}"
            );
        }
        assert_eq!(route(false, false, net, &send(1)), Route::Local);
        assert_eq!(
            route(false, false, net, &Request::QueryCamera),
            Route::Local
        );
    }
    for pause in [Request::Pause, Request::Resume] {
        assert_eq!(route(false, false, host, &pause), Route::Local);
        assert_eq!(route(false, false, client, &pause), Route::RefuseLockstep);
    }
    assert_eq!(
        route(true, false, client, &Request::Pause),
        Route::Local,
        "the viewer keeps its own clock"
    );
}

#[test]
fn final_map_allows_inspection_but_refuses_time_and_session_mutation() {
    for request in [
        Request::AdvanceTicks { ticks: 1 },
        Request::PresentTicks { ticks: 1 },
        Request::Resume,
        Request::SetSpeed { multiplier: 2.0 },
        Request::BeginPerformanceWindow {
            from_tick: 0,
            to_tick: 1,
        },
        Request::LoadScenario {
            path: "other.json".to_string(),
        },
    ] {
        assert!(frozen_map_refuses(&request), "{request:?}");
    }
    for request in [
        Request::Status,
        Request::QueryState {
            filter: oxide_protocol::StateFilter::default(),
        },
        Request::QueryCamera,
        Request::QueryUi,
        Request::Screenshot { path: None },
    ] {
        assert!(!frozen_map_refuses(&request), "{request:?}");
    }
}
