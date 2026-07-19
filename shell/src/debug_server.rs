//! The debug server: JSON lines over TCP, one thread per connection.
//!
//! Socket threads never touch game state. Each parsed request crosses to
//! the main loop over a channel and blocks its connection until the frame
//! loop answers — requests are handled between frames, so every response
//! reflects a consistent world.

use anyhow::{Context, Result};
use oxide_protocol::{Request, RequestEnvelope, ResponseEnvelope};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, Sender, channel};

/// A request waiting for the main loop, with its return channel.
pub struct IncomingRequest {
    /// Correlation id from the client.
    pub id: u64,
    /// The request.
    pub request: Request,
    /// Where the main loop sends the response.
    pub reply: Sender<ResponseEnvelope>,
}

/// Binds the listener and starts accepting. Returns the channel the main
/// loop drains each frame. Binding failure (port taken) is fatal on purpose
/// — a silently missing debug server wastes an agent's whole session.
pub fn spawn(port: u16) -> Result<Receiver<IncomingRequest>> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .with_context(|| format!("binding debug server to 127.0.0.1:{port}"))?;
    eprintln!("debug server listening on 127.0.0.1:{port}");
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let tx = tx.clone();
            std::thread::spawn(move || handle_connection(stream, tx));
        }
    });
    Ok(rx)
}

fn handle_connection(stream: TcpStream, tx: Sender<IncomingRequest>) {
    stream.set_nodelay(true).ok();
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let envelope: RequestEnvelope = match serde_json::from_str(line) {
            Ok(envelope) => envelope,
            Err(err) => {
                if !write_response(
                    &mut writer,
                    &ResponseEnvelope::err(0, format!("bad request: {err}")),
                ) {
                    break;
                }
                continue;
            }
        };
        let (reply_tx, reply_rx) = channel();
        let sent = tx.send(IncomingRequest {
            id: envelope.id,
            request: envelope.request,
            reply: reply_tx,
        });
        if sent.is_err() {
            break; // main loop is gone; nothing left to serve
        }
        match reply_rx.recv() {
            Ok(response) => {
                if !write_response(&mut writer, &response) {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

fn write_response(writer: &mut TcpStream, response: &ResponseEnvelope) -> bool {
    let Ok(mut line) = serde_json::to_string(response) else {
        return false;
    };
    line.push('\n');
    writer
        .write_all(line.as_bytes())
        .and_then(|()| writer.flush())
        .is_ok()
}
