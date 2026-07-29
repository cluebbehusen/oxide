//! A blocking debug-protocol client: one TCP connection, JSON lines,
//! sequential request/response.

use anyhow::{Context, Result, bail};
use oxide_protocol::{
    ADVANCE_TICKS_PER_BUDGET_SECOND, MAX_ADVANCE_TICKS, Reply, Request, RequestEnvelope,
    ResponseEnvelope,
};
use std::io::{BufReader, Write};
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

        // Bounded like the server side: a wedged or hostile peer that
        // streams a newline-free response must not grow this
        // allocation without limit.
        let mut raw = Vec::new();
        let read = std::io::BufRead::read_until(
            &mut std::io::Read::take(&mut self.reader, oxide_protocol::MAX_FRAME_BYTES as u64 + 1),
            b'\n',
            &mut raw,
        )?;
        if read == 0 {
            bail!("shell closed the connection");
        }
        if raw.len() > oxide_protocol::MAX_FRAME_BYTES {
            bail!(
                "shell response exceeded the {} byte frame limit",
                oxide_protocol::MAX_FRAME_BYTES
            );
        }
        let response = String::from_utf8(raw).context("shell response is not UTF-8")?;
        let envelope: ResponseEnvelope =
            serde_json::from_str(response.trim()).context("parsing shell response")?;
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
    use std::io::BufRead;
    use std::net::TcpListener;

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
