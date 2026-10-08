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

    /// The session's status line.
    pub fn status(&mut self) -> Result<oxide_protocol::StatusView> {
        match self.call(Request::Status)? {
            Reply::Status(view) => Ok(view),
            other => wrong_reply("status", &other),
        }
    }

    /// The state view `filter` selects.
    pub fn state(
        &mut self,
        filter: oxide_protocol::StateFilter,
    ) -> Result<oxide_protocol::StateView> {
        match self.call(Request::QueryState { filter })? {
            Reply::State(view) => Ok(view),
            other => wrong_reply("state", &other),
        }
    }

    /// The authoritative state hash.
    pub fn state_hash(&mut self) -> Result<oxide_protocol::HashView> {
        match self.call(Request::StateHash)? {
            Reply::Hash(view) => Ok(view),
            other => wrong_reply("hash", &other),
        }
    }

    /// Where the camera is looking.
    pub fn camera(&mut self) -> Result<oxide_protocol::CameraView> {
        match self.call(Request::QueryCamera)? {
            Reply::Camera(view) => Ok(view),
            other => wrong_reply("camera", &other),
        }
    }

    /// The interface as the shell currently presents it.
    pub fn ui(&mut self) -> Result<oxide_protocol::UiView> {
        match self.call(Request::QueryUi)? {
            Reply::Ui(view) => Ok(view),
            other => wrong_reply("ui", &other),
        }
    }

    /// Steps the simulation `ticks` ticks without presenting them.
    pub fn advance(&mut self, ticks: u64) -> Result<oxide_protocol::AdvancedView> {
        match self.call(Request::AdvanceTicks { ticks })? {
            Reply::Advanced(view) => Ok(view),
            other => wrong_reply("advanced", &other),
        }
    }

    /// Steps `ticks` ticks through the presentation path, keeping their events.
    pub fn present(&mut self, ticks: u64) -> Result<oxide_protocol::PresentedView> {
        match self.call(Request::PresentTicks { ticks })? {
            Reply::Presented(view) => Ok(view),
            other => wrong_reply("presented", &other),
        }
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

fn wrong_reply<T>(expected: &str, got: &Reply) -> Result<T> {
    bail!("expected a {expected} reply, got {got:?}")
}

#[cfg(test)]
mod tests;
