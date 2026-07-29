//! The shell's debug-server doorstep: bind the port, announce it, and
//! hand the connection loop to the shared framed transport
//! ([`oxide_protocol::framing`] — the same loop the windowless
//! `oxide-driver session` runs, so the two servers cannot drift at the
//! byte level).
//!
//! Socket threads never touch game state. Each parsed request crosses to
//! the main loop over the returned channel and blocks its connection
//! until the frame loop answers — requests are handled between frames,
//! so every response reflects a consistent world.

use anyhow::{Context, Result};
use oxide_protocol::framing::incoming;
use std::net::TcpListener;
use std::sync::mpsc::Receiver;

pub use oxide_protocol::framing::{IncomingRequest, Limits};

/// Binds the listener and starts accepting under `limits`. Returns the
/// channel the main loop drains each frame. Binding failure (port taken)
/// is fatal on purpose — a silently missing debug server wastes an
/// agent's whole session.
pub fn spawn(port: u16, limits: Limits) -> Result<Receiver<IncomingRequest>> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .with_context(|| format!("binding debug server to 127.0.0.1:{port}"))?;
    eprintln!("debug server listening on 127.0.0.1:{port}");
    Ok(incoming(listener, limits))
}
