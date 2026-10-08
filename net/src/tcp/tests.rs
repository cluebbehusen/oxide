use super::*;
use std::time::Instant;

const DEADLINE: Duration = Duration::from_secs(10);

fn wait<T>(mut poll: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(value) = poll() {
            return value;
        }
        assert!(start.elapsed() < DEADLINE, "timed out");
        thread::sleep(Duration::from_millis(1));
    }
}

fn recv(connection: &Connection) -> Result<String, Closed> {
    wait(|| connection.try_recv().transpose())
}

fn pair() -> (Connection, Connection) {
    let listener = Listener::bind("127.0.0.1:0").unwrap();
    assert!(
        listener.try_accept().unwrap().is_none(),
        "nobody has connected"
    );
    let client = Connection::connect(listener.local_addr().unwrap()).unwrap();
    let host = wait(|| listener.try_accept().unwrap());
    (host, client)
}

fn accepted_from_raw() -> (Connection, TcpStream) {
    let listener = Listener::bind("127.0.0.1:0").unwrap();
    let raw = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    raw.set_read_timeout(Some(DEADLINE)).unwrap();
    (wait(|| listener.try_accept().unwrap()), raw)
}

#[test]
fn a_bounded_read_accepts_the_limit_and_refuses_beyond_it() {
    let read = |bytes: &[u8]| {
        let mut buffer = Vec::new();
        let frame = read_line(&mut &bytes[..], &mut buffer, 4);
        (frame, buffer)
    };
    assert_eq!(read(b"abcd\nnext"), (Frame::Line, b"abcd".to_vec()));
    assert_eq!(read(b"abcde\n").0, Frame::Oversized);
    assert_eq!(read(b"ab").0, Frame::Closed, "a partial line at EOF");
    assert_eq!(read(b"").0, Frame::Closed);
}

#[test]
fn lines_cross_both_ways_and_finish_delivers_every_line() {
    let (host, mut client) = pair();
    let longest = "a".repeat(MAX_LINE_BYTES);
    client.send("hello");
    client.send(&longest);
    assert_eq!(recv(&host).unwrap(), "hello");
    assert_eq!(recv(&host).unwrap(), longest);
    for index in 0..100 {
        host.send(&format!("line {index}"));
    }
    client.send("last");
    client.finish();
    client.send("after finish");
    assert_eq!(recv(&host).unwrap(), "last");
    assert_eq!(recv(&host), Err(Closed), "the client finished");
    for index in 0..100 {
        assert_eq!(recv(&client).unwrap(), format!("line {index}"));
    }
    drop(host);
    assert_eq!(recv(&client), Err(Closed), "the host went away");
}

/// Reads until the stream ends, returning the bytes read, or `None` if
/// it stalls instead.
fn read_to_end(raw: &mut TcpStream) -> Option<usize> {
    let mut received = 0;
    let mut chunk = vec![0; 1 << 16];
    loop {
        match raw.read(&mut chunk) {
            Ok(0) => return Some(received),
            Ok(read) => received += read,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return None;
            }
            // A reset also ends the stream.
            Err(_) => return Some(received),
        }
    }
}

#[test]
fn an_oversized_or_non_utf8_line_closes_the_connection_both_ways() {
    let oversized = vec![b'a'; MAX_LINE_BYTES + 1];
    let malformed: [&[u8]; 2] = [&oversized, &[0xff, b'\n']];
    for bytes in malformed {
        let (host, mut raw) = accepted_from_raw();
        raw.write_all(bytes).unwrap();
        assert_eq!(recv(&host), Err(Closed));
        host.send("after the close");
        assert_eq!(
            read_to_end(&mut raw),
            Some(0),
            "the peer sees the close and nothing after it"
        );
    }
}

#[test]
fn finishing_both_sides_at_once_still_delivers_every_line() {
    let (host, mut client) = pair();
    let line = "b".repeat(4 << 10);
    let lines = 8192;
    let mut host = Some(host);
    let sender = host.as_mut().unwrap();
    for _ in 0..lines {
        sender.send(&line);
    }
    sender.finish();
    client.send("bye");
    client.finish();

    // The client reads nothing yet, so the host's writer stays blocked
    // behind a full pipe while the host already has the client's end of
    // stream. The host must not report the clean close before its own
    // lines are written; dropping on that report would discard them.
    let pause = Instant::now();
    while pause.elapsed() < Duration::from_millis(200) {
        match host.as_ref().unwrap().try_recv() {
            Ok(Some(received)) => assert_eq!(received, "bye"),
            Ok(None) => thread::sleep(Duration::from_millis(1)),
            Err(Closed) => {
                host = None;
                break;
            }
        }
    }

    let mut received = 0;
    let start = Instant::now();
    loop {
        assert!(start.elapsed() < DEADLINE, "timed out");
        if let Some(connection) = &host
            && connection.try_recv() == Err(Closed)
        {
            host = None;
        }
        match client.try_recv() {
            Ok(Some(_)) => received += 1,
            Ok(None) => {}
            Err(Closed) => break,
        }
    }
    assert_eq!(received, lines, "every host line reached the client");
}

#[test]
fn dropping_a_connection_frees_a_writer_whose_peer_stopped_reading() {
    let (host, mut raw) = accepted_from_raw();
    let line = "a".repeat(MAX_LINE_BYTES - 1);
    let lines = 64;
    let queued = Instant::now();
    for _ in 0..lines {
        host.send(&line);
    }
    assert!(
        queued.elapsed() < DEADLINE,
        "sending never waits on the peer"
    );
    thread::sleep(Duration::from_millis(50));
    drop(host);
    let received = read_to_end(&mut raw).expect("the stream ended instead of stalling");
    assert!(
        received < lines * MAX_LINE_BYTES,
        "the backlog was discarded"
    );
}
