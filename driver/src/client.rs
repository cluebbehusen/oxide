//! A blocking debug-protocol client: one TCP connection, JSON lines,
//! sequential request/response.

use anyhow::{Context, Result, bail};
use oxide_protocol::{
    ADVANCE_TICKS_PER_BUDGET_SECOND, MAX_ADVANCE_TICKS, Reply, Request, RequestEnvelope,
    ResponseEnvelope,
};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Normal protocol calls should fail promptly when a peer stalls.
const ORDINARY_READ_TIMEOUT: Duration = Duration::from_secs(30);

fn read_timeout_for(request: &Request) -> Duration {
    match request {
        Request::AdvanceTicks { ticks } => {
            // Budgeted from the protocol's shared figure, so this side's
            // deadline can never undercut the server's own.
            let seconds = (*ticks)
                .min(MAX_ADVANCE_TICKS)
                .div_ceil(ADVANCE_TICKS_PER_BUDGET_SECOND);
            ORDINARY_READ_TIMEOUT.saturating_add(Duration::from_secs(seconds))
        }
        _ => ORDINARY_READ_TIMEOUT,
    }
}

fn read_response(reader: &mut impl BufRead, max_bytes: usize) -> Result<ResponseEnvelope> {
    let mut raw = Vec::new();
    let read = std::io::Read::take(reader, max_bytes as u64 + 1)
        .read_until(b'\n', &mut raw)
        .context("reading shell response")?;
    if read == 0 {
        bail!("shell closed the connection");
    }
    let terminated = raw.last() == Some(&b'\n');
    if terminated {
        raw.pop();
    }
    if raw.len() > max_bytes {
        bail!("shell response exceeded the {max_bytes} byte response limit");
    }
    if !terminated {
        bail!("shell closed the connection before terminating its response line");
    }
    let response = String::from_utf8(raw).context("shell response is not UTF-8")?;
    serde_json::from_str(response.trim()).context("parsing shell response")
}

/// A connected client.
pub struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    next_id: u64,
}

impl Client {
    /// Connects to a debug-protocol server, e.g. `127.0.0.1:4123` —
    /// a shell's `--debug-server` or a windowless `oxide-driver session`.
    pub fn connect(addr: &str) -> Result<Self> {
        let stream =
            TcpStream::connect(addr).with_context(|| format!("connecting to shell at {addr}"))?;
        stream.set_nodelay(true).ok();
        stream
            .set_read_timeout(Some(ORDINARY_READ_TIMEOUT))
            .context("setting shell read timeout")?;
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .context("setting shell write timeout")?;
        let reader = BufReader::new(stream.try_clone().context("cloning stream")?);
        Ok(Self {
            reader,
            writer: stream,
            next_id: 1,
        })
    }

    /// Sends one request and waits for its response.
    pub fn call(&mut self, request: Request) -> Result<Reply> {
        self.reader
            .get_ref()
            .set_read_timeout(Some(read_timeout_for(&request)))
            .context("setting request read timeout")?;
        let id = self.next_id;
        self.next_id += 1;
        let mut line = serde_json::to_string(&RequestEnvelope { id, request })?;
        line.push('\n');
        self.writer.write_all(line.as_bytes())?;
        self.writer.flush()?;

        // Bounded, but at the RESPONSE ceiling: replies legitimately
        // dwarf the request-line cap (a deep query_state is not a
        // hand-typed line), while a wedged or hostile peer still must
        // not grow this allocation without limit.
        let envelope = read_response(&mut self.reader, oxide_protocol::MAX_RESPONSE_BYTES)?;
        // id 0 is the transport speaking, not a reply: the server sends
        // unsolicited refusals (connection cap, oversized frame) under
        // it, and correlating first would bury the actionable message.
        if envelope.id == 0 {
            let message = envelope
                .into_result()
                .err()
                .unwrap_or_else(|| "unsolicited transport notice".to_string());
            bail!("shell refused the connection: {message}");
        }
        if envelope.id != id {
            bail!("response id {} for request {id}", envelope.id);
        }
        envelope
            .into_result()
            .map_err(|message| anyhow::anyhow!("shell error: {message}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};
    use std::net::TcpListener;

    fn encoded(response: ResponseEnvelope) -> Vec<u8> {
        let mut bytes = serde_json::to_vec(&response).unwrap();
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
        let exact = encoded(response.clone());
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
        let refusal =
            call_against(encoded(ResponseEnvelope::err(0, "too many clients"))).unwrap_err();
        assert!(
            refusal
                .to_string()
                .contains("refused the connection: too many clients"),
            "{refusal:#}"
        );

        let unsolicited_ok = call_against(encoded(ResponseEnvelope::ok(0, Reply::Ok))).unwrap_err();
        assert!(
            unsolicited_ok
                .to_string()
                .contains("unsolicited transport notice"),
            "{unsolicited_ok:#}"
        );

        let mismatch = call_against(encoded(ResponseEnvelope::ok(99, Reply::Ok))).unwrap_err();
        assert!(
            mismatch
                .to_string()
                .contains("response id 99 for request 1"),
            "{mismatch:#}"
        );

        let server_error =
            call_against(encoded(ResponseEnvelope::err(1, "bad command"))).unwrap_err();
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
                    Request::AdvanceTicks { ticks } => {
                        Reply::Advanced(oxide_protocol::AdvancedView {
                            ticks,
                            tick: ticks,
                            hash: "0x0000000000000000".to_string(),
                        })
                    }
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
}
