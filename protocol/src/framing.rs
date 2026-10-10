//! The framed JSON-lines transport both debug servers run.
//!
//! One request object per line, one response object per line, correlated
//! by `id`. This module knows only bytes and framing: it takes a handler
//! factory and never mentions a shell or a session, so the windowed shell
//! and the windowless `oxide-driver session` run the same loop with their
//! own answering sides.
//!
//! Every bound a network server needs lives in [`Limits`]. The idle read
//! deadline is generous because a paused driven-mode agent legitimately
//! parks between commands. The reply deadline sits past what the driver's
//! client budgets for the longest legal advance, so the client always gives
//! up first and no answer arrives at a peer that stopped listening.

use crate::{
    ADVANCE_TICKS_PER_BUDGET_SECOND, MAX_ADVANCE_TICKS, MAX_FRAME_BYTES, Request, RequestEnvelope,
    ResponseEnvelope,
};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

/// Connections served at once: enough for an agent, its editor, and a
/// stray abandoned session.
const MAX_CLIENTS: usize = 8;

/// How long a connection may sit silent before it is closed (see the module
/// docs).
const IDLE_TIMEOUT: Duration = Duration::from_mins(30);

/// How long a peer that stopped reading may stall a response.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a socket thread waits for the answering side. Past the
/// client's own deadline for the longest legal advance, so the client is
/// always the side that gives up.
const REPLY_TIMEOUT: Duration =
    Duration::from_secs(60 + MAX_ADVANCE_TICKS / ADVANCE_TICKS_PER_BUDGET_SECOND);

/// A request waiting for the answering side, with its return channel.
pub struct IncomingRequest {
    /// Correlation id from the client.
    pub id: u64,
    /// The request.
    pub request: Request,
    /// Where the answering side sends the response.
    pub reply: Sender<ResponseEnvelope>,
}

/// The resource bounds every framed connection is held to.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Longest request line accepted, newline excluded.
    pub max_frame_bytes: usize,
    /// Connections served at once; the next one is refused and closed.
    pub max_clients: usize,
    /// How long a connection may sit silent before it is closed.
    pub idle_timeout: Duration,
    /// How long a stalled peer may block a response write.
    pub write_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_frame_bytes: MAX_FRAME_BYTES,
            max_clients: MAX_CLIENTS,
            idle_timeout: IDLE_TIMEOUT,
            write_timeout: WRITE_TIMEOUT,
        }
    }
}

/// Starts accepting on `listener` and returns the channel the answering
/// side drains — per frame in the shell, in a blocking loop in the
/// headless session. Socket threads never touch game state: each parsed
/// request crosses this channel and blocks its connection until the
/// answering side replies, so every response reflects a settled world.
pub fn incoming(listener: TcpListener, limits: Limits) -> Receiver<IncomingRequest> {
    let (tx, rx) = channel();
    serve(listener, limits, move || {
        let tx = tx.clone();
        move |envelope: RequestEnvelope| {
            let id = envelope.id;
            let (reply_tx, reply_rx) = channel();
            tx.send(IncomingRequest {
                id,
                request: envelope.request,
                reply: reply_tx,
            })
            .ok()?; // the answering side is gone; nothing left to serve
            match reply_rx.recv_timeout(REPLY_TIMEOUT) {
                Ok(response) => Some(response),
                Err(RecvTimeoutError::Timeout) => Some(ResponseEnvelope::err(
                    id,
                    format!(
                        "the server did not answer within {} seconds",
                        REPLY_TIMEOUT.as_secs()
                    ),
                )),
                Err(RecvTimeoutError::Disconnected) => None,
            }
        }
    });
    rx
}

/// Accepts framed JSON-lines connections until the listener dies, giving
/// each its own thread and its own handler. A handler answering `None`
/// closes its connection.
pub fn serve<F, H>(listener: TcpListener, limits: Limits, make_handler: F)
where
    F: Fn() -> H + Send + 'static,
    H: Fn(RequestEnvelope) -> Option<ResponseEnvelope> + Send + 'static,
{
    std::thread::spawn(move || {
        let live = Arc::new(AtomicUsize::new(0));
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let Some(slot) = Slot::claim(&live, limits.max_clients) else {
                refuse(stream, &limits);
                continue;
            };
            let handler = make_handler();
            std::thread::spawn(move || {
                let _slot = slot;
                serve_connection(stream, limits, &handler);
            });
        }
    });
}

/// One live connection's seat, released when its thread ends.
struct Slot(Arc<AtomicUsize>);

impl Slot {
    fn claim(live: &Arc<AtomicUsize>, max: usize) -> Option<Self> {
        let mut n = live.load(Ordering::SeqCst);
        while n < max {
            match live.compare_exchange_weak(n, n + 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return Some(Self(Arc::clone(live))),
                Err(current) => n = current,
            }
        }
        None
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Tells a client past the cap why it is leaving, rather than dropping it
/// into an unexplained closed socket.
fn refuse(mut stream: TcpStream, limits: &Limits) {
    stream.set_write_timeout(Some(limits.write_timeout)).ok();
    write_response(
        &mut stream,
        &ResponseEnvelope::err(
            0,
            format!(
                "debug server is at its connection limit ({}); disconnect another client first",
                limits.max_clients
            ),
        ),
    );
}

fn serve_connection<H>(stream: TcpStream, limits: Limits, handler: &H)
where
    H: Fn(RequestEnvelope) -> Option<ResponseEnvelope>,
{
    stream.set_nodelay(true).ok();
    stream.set_read_timeout(Some(limits.idle_timeout)).ok();
    stream.set_write_timeout(Some(limits.write_timeout)).ok();
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(stream);
    let mut frame = Vec::new();
    loop {
        let line = match read_frame(&mut reader, &mut frame, limits.max_frame_bytes) {
            Frame::Line => frame.as_slice(),
            Frame::Oversized => {
                write_response(
                    &mut writer,
                    &ResponseEnvelope::err(
                        0,
                        format!(
                            "request line exceeds the {}-byte frame limit",
                            limits.max_frame_bytes
                        ),
                    ),
                );
                return;
            }
            Frame::Closed => return,
        };
        let text = match std::str::from_utf8(line) {
            Ok(text) => text.trim(),
            Err(err) => {
                if !write_response(
                    &mut writer,
                    &ResponseEnvelope::err(0, format!("bad request: {err}")),
                ) {
                    return;
                }
                continue;
            }
        };
        if text.is_empty() {
            continue;
        }
        let envelope: RequestEnvelope = match serde_json::from_str(text) {
            Ok(envelope) => envelope,
            Err(err) => {
                if !write_response(
                    &mut writer,
                    &ResponseEnvelope::err(0, format!("bad request: {err}")),
                ) {
                    return;
                }
                continue;
            }
        };
        let Some(response) = handler(envelope) else {
            return;
        };
        if !write_response(&mut writer, &response) {
            return;
        }
    }
}

/// What one framing read produced.
enum Frame {
    /// `buf` holds a complete line, newline stripped.
    Line,
    /// The line ran past the limit; nothing was buffered beyond it.
    Oversized,
    /// End of stream, an idle deadline, or a broken connection.
    Closed,
}

/// Reads one newline-terminated frame into `buf`, never allocating past
/// `max` bytes of payload.
fn read_frame(reader: &mut BufReader<TcpStream>, buf: &mut Vec<u8>, max: usize) -> Frame {
    buf.clear();
    match reader.take(max as u64 + 1).read_until(b'\n', buf) {
        Ok(0) => Frame::Closed,
        Ok(_) => {
            if buf.last() == Some(&b'\n') {
                buf.pop();
                Frame::Line
            } else if buf.len() > max {
                Frame::Oversized
            } else {
                Frame::Closed // the peer stopped mid-line
            }
        }
        Err(_) => Frame::Closed,
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

#[cfg(test)]
mod tests;
