use super::*;
use crate::Reply;
use std::net::SocketAddr;

/// A framed server with a stub answering side: no window, no game, and
/// a counter so a test can tell a dropped frame from a served one.
fn stub_server(limits: Limits) -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback");
    let addr = listener.local_addr().expect("listener address");
    let served = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&served);
    serve(listener, limits, move || {
        let counter = Arc::clone(&counter);
        move |envelope: RequestEnvelope| {
            counter.fetch_add(1, Ordering::SeqCst);
            Some(ResponseEnvelope::ok(envelope.id, Reply::Ok))
        }
    });
    (addr, served)
}

fn connect(addr: SocketAddr) -> (BufReader<TcpStream>, TcpStream) {
    let stream = TcpStream::connect(addr).expect("connect to stub server");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("client read deadline");
    let reader = BufReader::new(stream.try_clone().expect("clone client stream"));
    (reader, stream)
}

fn read_line(reader: &mut BufReader<TcpStream>) -> Option<String> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line),
    }
}

fn status(id: u64) -> String {
    format!("{{\"id\":{id},\"method\":\"status\"}}\n")
}

fn error_message(line: &str) -> String {
    let envelope: ResponseEnvelope = serde_json::from_str(line.trim()).expect("parse response");
    envelope
        .into_result()
        .expect_err("expected an error envelope")
}

#[test]
fn pipelined_requests_are_answered_in_order_and_a_partial_line_just_closes() {
    let (addr, served) = stub_server(Limits::default());
    let (mut reader, mut writer) = connect(addr);

    let pipelined = format!("{}{}", status(1), status(2));
    writer
        .write_all(pipelined.as_bytes())
        .expect("write frames");
    for id in 1..=2 {
        let line = read_line(&mut reader).expect("response line");
        let envelope: ResponseEnvelope = serde_json::from_str(line.trim()).expect("parse response");
        assert_eq!(envelope.id, id);
        envelope.into_result().expect("ok reply");
    }

    writer
        .write_all(b"{\"id\":3,\"method\":\"sta")
        .expect("write partial frame");
    writer
        .shutdown(std::net::Shutdown::Write)
        .expect("half-close");
    assert!(
        read_line(&mut reader).is_none(),
        "a line that never ends is not a request"
    );
    assert_eq!(served.load(Ordering::SeqCst), 2);
}

#[test]
fn an_oversized_frame_is_told_the_limit_and_then_shown_the_door() {
    let limits = Limits {
        max_frame_bytes: 32,
        ..Limits::default()
    };
    let (addr, served) = stub_server(limits);
    let (mut reader, mut writer) = connect(addr);

    // Exactly one byte past the limit, so the server consumes every
    // byte sent and the close stays graceful.
    writer
        .write_all(&vec![b'x'; limits.max_frame_bytes + 1])
        .expect("write oversized frame");
    let line = read_line(&mut reader).expect("refusal line");
    let message = error_message(&line);
    assert!(
        message.contains("32"),
        "the refusal must name the limit: {message}"
    );
    assert!(read_line(&mut reader).is_none(), "the connection is over");
    assert_eq!(served.load(Ordering::SeqCst), 0);
}

#[test]
fn malformed_utf8_is_answered_like_a_parse_failure_and_the_connection_lives() {
    let (addr, served) = stub_server(Limits::default());
    let (mut reader, mut writer) = connect(addr);

    writer
        .write_all(&[0xff, 0xfe, b'\n'])
        .expect("write undecodable frame");
    let line = read_line(&mut reader).expect("refusal line");
    assert!(
        error_message(&line).starts_with("bad request"),
        "an encoding failure reads like any other bad request"
    );

    writer
        .write_all(status(7).as_bytes())
        .expect("write valid frame");
    let line = read_line(&mut reader).expect("response line");
    let envelope: ResponseEnvelope = serde_json::from_str(line.trim()).expect("parse response");
    assert_eq!(envelope.id, 7);
    envelope.into_result().expect("ok reply");
    assert_eq!(served.load(Ordering::SeqCst), 1);
}

#[test]
fn malformed_json_is_refused_without_poisoning_the_connection() {
    let (addr, served) = stub_server(Limits::default());
    let (mut reader, mut writer) = connect(addr);

    writer
        .write_all(b"{\"id\":3,\"method\":}\n")
        .expect("write malformed frame");
    let line = read_line(&mut reader).expect("refusal line");
    assert!(error_message(&line).starts_with("bad request"));

    writer
        .write_all(status(8).as_bytes())
        .expect("write valid frame");
    let line = read_line(&mut reader).expect("response line");
    let envelope: ResponseEnvelope = serde_json::from_str(line.trim()).expect("parse response");
    assert_eq!(envelope.id, 8);
    envelope.into_result().expect("ok reply");
    assert_eq!(served.load(Ordering::SeqCst), 1);
}

#[test]
fn a_frame_exactly_at_the_payload_limit_is_accepted() {
    let mut frame = status(12);
    frame.pop();
    let limit = frame.len() + 7;
    frame.extend(std::iter::repeat_n(' ', 7));
    frame.push('\n');

    let limits = Limits {
        max_frame_bytes: limit,
        ..Limits::default()
    };
    let (addr, served) = stub_server(limits);
    let (mut reader, mut writer) = connect(addr);
    writer
        .write_all(frame.as_bytes())
        .expect("write exact frame");

    let line = read_line(&mut reader).expect("response line");
    let envelope: ResponseEnvelope = serde_json::from_str(line.trim()).expect("parse response");
    assert_eq!(envelope.id, 12);
    envelope.into_result().expect("ok reply");
    assert_eq!(served.load(Ordering::SeqCst), 1);
}

#[test]
fn dropping_a_dispatched_reply_closes_only_that_connection() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback");
    let addr = listener.local_addr().expect("listener address");
    let incoming = incoming(listener, Limits::default());
    let (mut reader, mut writer) = connect(addr);
    writer
        .write_all(status(1).as_bytes())
        .expect("write request");

    let request = incoming
        .recv_timeout(Duration::from_secs(2))
        .expect("request reached answering side before the test deadline");
    drop(request.reply);
    assert!(
        read_line(&mut reader).is_none(),
        "a vanished answering side cannot leave its client hanging"
    );
}

#[test]
fn an_abrupt_disconnect_releases_the_connection_it_held() {
    let limits = Limits {
        max_clients: 1,
        ..Limits::default()
    };
    let (addr, _) = stub_server(limits);
    {
        let (_reader, mut writer) = connect(addr);
        writer
            .write_all(status(1).as_bytes())
            .expect("write request");
    } // dropped without ever reading the answer

    // The seat is released on the socket thread, so give it a moment
    // rather than a promise.
    for attempt in 0..200 {
        let (mut reader, mut writer) = connect(addr);
        writer
            .write_all(status(2).as_bytes())
            .expect("write request");
        // Windows may reset a refused socket whose request was not read.
        let released = read_line(&mut reader).is_some_and(|line| {
            serde_json::from_str::<ResponseEnvelope>(line.trim())
                .expect("parse response")
                .into_result()
                .is_ok()
        });
        if released {
            return;
        }
        assert!(attempt < 199, "the abandoned connection never let go");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_client_past_the_cap_is_refused_in_words_while_the_parked_one_plays_on() {
    let limits = Limits {
        max_clients: 1,
        ..Limits::default()
    };
    let (addr, _) = stub_server(limits);
    let (mut parked_reader, mut parked_writer) = connect(addr);
    parked_writer
        .write_all(status(1).as_bytes())
        .expect("write request");
    read_line(&mut parked_reader).expect("response line");

    let (mut reader, _writer) = connect(addr);
    let line = read_line(&mut reader).expect("refusal line");
    let message = error_message(&line);
    assert!(
        message.contains("connection limit"),
        "the refusal must say why: {message}"
    );
    assert!(
        read_line(&mut reader).is_none(),
        "a refused client is not left hanging"
    );

    parked_writer
        .write_all(status(2).as_bytes())
        .expect("write request");
    let line = read_line(&mut parked_reader).expect("response line");
    let envelope: ResponseEnvelope = serde_json::from_str(line.trim()).expect("parse response");
    assert_eq!(envelope.id, 2);
    envelope.into_result().expect("ok reply");
}

#[test]
fn a_silent_connection_is_closed_when_its_idle_deadline_passes() {
    let limits = Limits {
        idle_timeout: Duration::from_millis(150),
        ..Limits::default()
    };
    let (addr, _) = stub_server(limits);
    let (mut reader, _writer) = connect(addr);
    assert!(
        read_line(&mut reader).is_none(),
        "an idle deadline closes the connection instead of parking a thread"
    );
}

#[test]
fn a_handler_with_nobody_behind_it_closes_the_connection() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback");
    let addr = listener.local_addr().expect("listener address");
    serve(listener, Limits::default(), || {
        |_: RequestEnvelope| -> Option<ResponseEnvelope> { None }
    });
    let (mut reader, mut writer) = connect(addr);
    writer
        .write_all(status(1).as_bytes())
        .expect("write request");
    assert!(
        read_line(&mut reader).is_none(),
        "an unanswerable request ends the connection"
    );
}
