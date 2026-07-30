//! The framed JSON-lines transport both debug servers run.
//!
//! One request object per line, one response object per line, correlated
//! by `id`. This module knows only bytes and framing: it takes a handler
//! factory and never mentions a shell or a session, so the windowed shell
//! and the windowless `oxide-driver session` drive the identical loop with
//! their own answering sides — the same lockstep-by-construction argument
//! as the request types themselves.
//!
//! Every bound a network server needs lives in [`Limits`]. The idle read
//! deadline is deliberately generous — half an hour — because a paused
//! driven-mode agent legitimately parks between commands and killing that
//! session would be a worse failure than leaking the thread it replaces.
//! The reply deadline instead sits just past what the driver's client
//! budgets for the longest legal advance, so the client always gives up
//! first and no answer arrives at a peer that stopped listening.

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

/// Connections served at once. Enough for an agent, its editor, and a
/// stray abandoned session; far short of a thread leak.
const MAX_CLIENTS: usize = 8;

/// How long a connection may sit silent before it is closed. Generous on
/// purpose (see the module docs).
const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

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
        live.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
            (n < max).then_some(n + 1)
        })
        .ok()?;
        Some(Self(Arc::clone(live)))
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
mod tests {
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
            let envelope: ResponseEnvelope =
                serde_json::from_str(line.trim()).expect("parse response");
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
            let line = read_line(&mut reader).expect("response line");
            if serde_json::from_str::<ResponseEnvelope>(line.trim())
                .expect("parse response")
                .into_result()
                .is_ok()
            {
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
}
