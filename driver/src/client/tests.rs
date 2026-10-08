use super::*;
use std::io::{Cursor, Read};
use std::net::TcpListener;

fn encoded(response: &ResponseEnvelope) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(response).unwrap();
    bytes.push(b'\n');
    bytes
}

fn call_against(response: Vec<u8>) -> Result<Reply> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback");
    let addr = listener.local_addr().expect("listener address");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept client");
        let mut request = Vec::new();
        loop {
            let mut byte = [0u8; 1];
            if stream.read_exact(&mut byte).is_err() || byte[0] == b'\n' {
                break;
            }
            request.push(byte[0]);
        }
        assert!(!request.is_empty(), "client sent a request");
        stream.write_all(&response).expect("write stub response");
    });
    let mut client = Client::connect(&addr.to_string()).expect("connect client");
    let result = client.call(Request::Status);
    server.join().expect("server thread");
    result
}

#[test]
fn response_reader_enforces_complete_bounded_frames() {
    let response = ResponseEnvelope::ok(1, Reply::Ok);
    let exact = encoded(&response);
    let payload_len = exact.len() - 1;
    let parsed = read_response(&mut Cursor::new(exact), payload_len).unwrap();
    assert_eq!(parsed, response);

    let mut oversized = serde_json::to_vec(&response).unwrap();
    oversized.extend_from_slice(b"  ");
    oversized.push(b'\n');
    let error = read_response(&mut Cursor::new(oversized), payload_len + 1).unwrap_err();
    assert!(error.to_string().contains("exceeded"), "{error:#}");

    let unterminated = serde_json::to_vec(&response).unwrap();
    let error = read_response(&mut Cursor::new(unterminated), payload_len).unwrap_err();
    assert!(error.to_string().contains("terminating"), "{error:#}");

    let error = read_response(&mut Cursor::new(Vec::<u8>::new()), payload_len).unwrap_err();
    assert!(
        error.to_string().contains("closed the connection"),
        "{error:#}"
    );
}

#[test]
fn client_preserves_transport_correlation_and_server_errors() {
    let refusal = call_against(encoded(&ResponseEnvelope::err(0, "too many clients"))).unwrap_err();
    assert!(
        refusal
            .to_string()
            .contains("refused the connection: too many clients"),
        "{refusal:#}"
    );

    let unsolicited_ok = call_against(encoded(&ResponseEnvelope::ok(0, Reply::Ok))).unwrap_err();
    assert!(
        unsolicited_ok
            .to_string()
            .contains("unsolicited transport notice"),
        "{unsolicited_ok:#}"
    );

    let mismatch = call_against(encoded(&ResponseEnvelope::ok(99, Reply::Ok))).unwrap_err();
    assert!(
        mismatch
            .to_string()
            .contains("response id 99 for request 1"),
        "{mismatch:#}"
    );

    let server_error = call_against(encoded(&ResponseEnvelope::err(1, "bad command"))).unwrap_err();
    assert!(
        server_error
            .to_string()
            .contains("shell error: bad command"),
        "{server_error:#}"
    );
}

#[test]
fn long_advance_scales_the_deadline_and_the_next_call_restores_it() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback");
    let addr = listener.local_addr().expect("listener address");
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept client");
        let mut reader = BufReader::new(stream.try_clone().expect("clone server stream"));
        let mut writer = stream;
        for _ in 0..2 {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).expect("read request") > 0);
            let request: RequestEnvelope =
                serde_json::from_str(line.trim()).expect("parse request");
            let reply = match request.request {
                Request::AdvanceTicks { ticks } => Reply::Advanced(oxide_protocol::AdvancedView {
                    ticks,
                    tick: ticks,
                    hash: "0x0000000000000000".to_string(),
                }),
                _ => Reply::Ok,
            };
            let mut response = serde_json::to_string(&ResponseEnvelope::ok(request.id, reply))
                .expect("serialize response");
            response.push('\n');
            writer
                .write_all(response.as_bytes())
                .expect("write response");
            writer.flush().expect("flush response");
        }
    });

    let mut client = Client::connect(&addr.to_string()).expect("connect client");
    assert_eq!(
        client.reader.get_ref().read_timeout().expect("timeout"),
        Some(ORDINARY_READ_TIMEOUT)
    );

    let long = Request::AdvanceTicks {
        ticks: MAX_ADVANCE_TICKS,
    };
    let long_timeout = read_timeout_for(&long);
    assert!(
        long_timeout > Duration::from_secs(180),
        "the maximum legal advance needs substantially more than 30 seconds"
    );
    assert_eq!(
        read_timeout_for(&Request::AdvanceTicks { ticks: u64::MAX }),
        long_timeout,
        "the deadline follows the server cap, not an oversized request"
    );
    client.call(long).expect("advance reply");
    assert_eq!(
        client.reader.get_ref().read_timeout().expect("timeout"),
        Some(long_timeout)
    );

    client.call(Request::Status).expect("ordinary reply");
    assert_eq!(
        client.reader.get_ref().read_timeout().expect("timeout"),
        Some(ORDINARY_READ_TIMEOUT)
    );
    server.join().expect("server thread");
}
