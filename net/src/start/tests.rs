use super::*;

const HOST: PlayerId = PlayerId(0);
const CLIENTS: [PlayerId; 2] = [PlayerId(1), PlayerId(2)];
const HASH: u64 = 0xfeed;

fn secs(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

/// Twin Forges with seats 0 to 2 human and seat 3 a bot.
fn scenario() -> Scenario {
    let mut scenario = Scenario::from_json(include_str!("../../../scenarios/twin-forges.json"))
        .expect("the shipped map parses");
    for (seat, player) in scenario.players.iter_mut().enumerate() {
        player.bot = seat == 3;
    }
    scenario
}

fn ready(hash: u64) -> String {
    JoinMessage::Ready { hash }.encode()
}

#[test]
fn each_client_gets_its_own_start_and_go_waits_for_every_ready() {
    let scenario = scenario();
    let mut barrier = StartBarrier::new(&scenario, HOST, &CLIENTS, HASH, secs(0));
    let starts = barrier.take_outgoing();
    assert_eq!(starts.len(), CLIENTS.len());
    for (seat, line) in starts {
        assert_eq!(
            LobbyMessage::decode(&line).unwrap(),
            LobbyMessage::Start {
                seat,
                scenario: Box::new(scenario.clone())
            }
        );
    }
    barrier.receive(CLIENTS[1], &ready(HASH)).unwrap();
    assert!(barrier.poll(secs(1)).unwrap().is_none());
    assert!(
        barrier.take_outgoing().is_empty(),
        "no Go before every Ready"
    );
    barrier.receive(CLIENTS[0], &ready(HASH)).unwrap();
    let mut session = barrier
        .poll(secs(2))
        .unwrap()
        .expect("every client is ready");
    let go = LobbyMessage::Go.encode();
    assert_eq!(
        barrier.take_outgoing(),
        vec![(CLIENTS[0], go.clone()), (CLIENTS[1], go)]
    );
    assert!(
        session.seal(secs(2)).is_some(),
        "the session starts at tick zero"
    );
}

#[test]
fn a_mismatched_hash_or_a_stray_line_fails_the_start() {
    let scenario = scenario();
    let fresh = || StartBarrier::new(&scenario, HOST, &CLIENTS, HASH, secs(0));
    let seat = CLIENTS[0];
    assert_eq!(
        fresh().receive(seat, &ready(HASH + 1)),
        Err(StartFailed::Mismatch { seat })
    );
    let mut barrier = fresh();
    barrier.receive(seat, &ready(HASH)).unwrap();
    assert_eq!(
        barrier.receive(seat, &ready(HASH)),
        Err(StartFailed::Protocol { seat }),
        "a second Ready"
    );
    for line in [JoinMessage::hello("abc").encode(), "{".to_owned()] {
        assert_eq!(
            fresh().receive(seat, &line),
            Err(StartFailed::Protocol { seat }),
            "{line}"
        );
    }
}

#[test]
fn the_start_times_out_at_its_deadline() {
    let scenario = scenario();
    let mut barrier = StartBarrier::new(&scenario, HOST, &CLIENTS, HASH, secs(5));
    barrier.receive(CLIENTS[0], &ready(HASH)).unwrap();
    let deadline = secs(5) + START_TIMEOUT;
    assert!(
        barrier
            .poll(deadline.checked_sub(Duration::from_millis(1)).unwrap())
            .unwrap()
            .is_none()
    );
    assert_eq!(barrier.poll(deadline).unwrap_err(), StartFailed::TimedOut);
}

#[test]
fn the_seats_must_be_exactly_the_scenarios_humans() {
    let scenario = scenario();
    for clients in [
        &[PlayerId(1)][..],
        &[PlayerId(1), PlayerId(2), PlayerId(3)],
        &[PlayerId(1), PlayerId(3)],
    ] {
        let result =
            std::panic::catch_unwind(|| StartBarrier::new(&scenario, HOST, clients, HASH, secs(0)));
        assert!(result.is_err(), "{clients:?}");
    }
}

#[test]
#[should_panic(expected = "the match already started")]
fn polling_after_the_start_is_a_caller_bug() {
    let scenario = scenario();
    let mut barrier = StartBarrier::new(&scenario, HOST, &CLIENTS, HASH, secs(0));
    for seat in CLIENTS {
        barrier.receive(seat, &ready(HASH)).unwrap();
    }
    barrier.poll(secs(0)).unwrap();
    let _ = barrier.poll(secs(0));
}

#[test]
#[should_panic(expected = "is not a client seat")]
fn lines_from_unknown_seats_are_a_caller_bug() {
    let scenario = scenario();
    let _ = StartBarrier::new(&scenario, HOST, &CLIENTS, HASH, secs(0))
        .receive(PlayerId(3), &ready(HASH));
}
