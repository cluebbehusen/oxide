//! A blocking debug-protocol client: one TCP connection, JSON lines,
//! sequential request/response.

use anyhow::{Context, Result, bail};
use oxide_protocol::{Reply, Request, RequestEnvelope, ResponseEnvelope};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

/// A connected client.
pub struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    next_id: u64,
}

impl Client {
    /// Connects to a shell's `--debug-server` socket, e.g.
    /// `127.0.0.1:4123`.
    pub fn connect(addr: &str) -> Result<Self> {
        let stream =
            TcpStream::connect(addr).with_context(|| format!("connecting to shell at {addr}"))?;
        stream.set_nodelay(true).ok();
        // A peer that accepts and then stalls must not hang the client
        // forever; thirty seconds comfortably covers a million-tick
        // advance while still failing hung shells.
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(30)))
            .ok();
        stream
            .set_write_timeout(Some(std::time::Duration::from_secs(10)))
            .ok();
        let reader = BufReader::new(stream.try_clone().context("cloning stream")?);
        Ok(Self {
            reader,
            writer: stream,
            next_id: 1,
        })
    }

    /// Sends one request and waits for its response.
    pub fn call(&mut self, request: Request) -> Result<Reply> {
        let id = self.next_id;
        self.next_id += 1;
        let mut line = serde_json::to_string(&RequestEnvelope { id, request })?;
        line.push('\n');
        self.writer.write_all(line.as_bytes())?;
        self.writer.flush()?;

        let mut response = String::new();
        let read = self.reader.read_line(&mut response)?;
        if read == 0 {
            bail!("shell closed the connection");
        }
        let envelope: ResponseEnvelope =
            serde_json::from_str(response.trim()).context("parsing shell response")?;
        if envelope.id != id {
            bail!("response id {} for request {id}", envelope.id);
        }
        envelope
            .into_result()
            .map_err(|message| anyhow::anyhow!("shell error: {message}"))
    }
}
