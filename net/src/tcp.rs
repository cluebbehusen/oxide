//! Newline-delimited lines over TCP: the crate's only I/O.
//!
//! Each [`Connection`] owns a reader thread and a writer thread, so a slow
//! peer never blocks the game loop that polls it.

use crate::MAX_LINE_BYTES;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// A blocked write gives up after this long and closes the connection.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// Received lines waiting for the game loop. When they fill up, the reader
/// stops reading and TCP pushes back on the peer, so a small queue loses
/// nothing and keeps a stalled loop's memory cost bounded.
const INCOMING_LINES: usize = 64;

/// The connection has ended: the peer closed it, a read or write failed, or
/// the peer sent an oversized or non-UTF-8 line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closed;

/// A listening socket polled from the game loop.
#[derive(Debug)]
pub struct Listener {
    socket: TcpListener,
}

impl Listener {
    /// Listens on `address`. Other machines can reach it only through a
    /// non-loopback address.
    pub fn bind(address: impl ToSocketAddrs) -> io::Result<Self> {
        let socket = TcpListener::bind(address)?;
        socket.set_nonblocking(true)?;
        Ok(Self { socket })
    }

    /// The bound address, including an ephemeral port.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// The next waiting connection, if any, without blocking.
    pub fn try_accept(&self) -> io::Result<Option<Connection>> {
        match self.socket.accept() {
            Ok((stream, _)) => {
                // Accepted sockets inherit non-blocking mode on some platforms.
                stream.set_nonblocking(false)?;
                Connection::spawn(stream).map(Some)
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error),
        }
    }
}

/// One peer: lines out through a writer thread, lines in through a reader
/// thread.
///
/// Dropping a connection aborts it: the socket shuts down in both
/// directions and unsent lines are lost. To close cleanly, call
/// [`Connection::finish`] and keep polling until [`Closed`] before dropping,
/// so the final lines reach the peer.
#[derive(Debug)]
pub struct Connection {
    socket: TcpStream,
    outgoing: Option<Sender<String>>,
    incoming: Receiver<String>,
    writer: JoinHandle<()>,
}

impl Connection {
    /// Connects to `address`, such as an IP address or host name with port.
    pub fn connect(address: impl ToSocketAddrs) -> io::Result<Self> {
        Self::spawn(TcpStream::connect(address)?)
    }

    fn spawn(socket: TcpStream) -> io::Result<Self> {
        socket.set_nodelay(true)?;
        socket.set_write_timeout(Some(WRITE_TIMEOUT))?;
        let reader = socket.try_clone()?;
        let writer = socket.try_clone()?;
        let (outgoing, to_write) = mpsc::channel();
        let (read, incoming) = mpsc::sync_channel(INCOMING_LINES);
        thread::Builder::new()
            .name("oxide-net-read".into())
            .spawn(move || read_lines(reader, &read))?;
        let writer = thread::Builder::new()
            .name("oxide-net-write".into())
            .spawn(move || write_lines(writer, to_write))
            .inspect_err(|_| {
                // Wake the reader so it exits instead of outliving the error.
                let _ = socket.shutdown(Shutdown::Both);
            })?;
        Ok(Self {
            socket,
            outgoing: Some(outgoing),
            incoming,
            writer,
        })
    }

    /// Queues one line, without its newline. Lines sent after the
    /// connection closed are dropped; [`Connection::try_recv`] reports the
    /// close.
    pub fn send(&self, line: &str) {
        if let Some(outgoing) = &self.outgoing {
            let _ = outgoing.send(format!("{line}\n"));
        }
    }

    /// The next received line, if one has arrived. Every line that arrived
    /// before the connection closed is returned before [`Closed`]. After
    /// [`Connection::finish`], [`Closed`] also waits until every queued line
    /// has been written.
    pub fn try_recv(&self) -> Result<Option<String>, Closed> {
        match self.incoming.try_recv() {
            Ok(line) => Ok(Some(line)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected)
                if self.outgoing.is_none() && !self.writer.is_finished() =>
            {
                Ok(None)
            }
            Err(TryRecvError::Disconnected) => Err(Closed),
        }
    }

    /// Sends every queued line, then closes this side for writing. Further
    /// sends are dropped.
    pub fn finish(&mut self) {
        self.outgoing = None;
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // Closing one handle leaves the others open; only a shutdown wakes
        // threads blocked on the socket.
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

fn read_lines(socket: TcpStream, lines: &SyncSender<String>) {
    let mut reader = BufReader::new(socket);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        let line = match read_line(&mut reader, &mut buffer, MAX_LINE_BYTES) {
            Frame::Line => String::from_utf8(std::mem::take(&mut buffer)).ok(),
            Frame::Oversized => None,
            Frame::Closed => return,
        };
        let Some(line) = line else {
            // A malformed line ends the connection for both sides at once.
            let _ = reader.get_ref().shutdown(Shutdown::Both);
            return;
        };
        if lines.send(line).is_err() {
            return;
        }
    }
}

fn write_lines(mut socket: TcpStream, lines: Receiver<String>) {
    for line in lines {
        if socket.write_all(line.as_bytes()).is_err() {
            let _ = socket.shutdown(Shutdown::Both);
            return;
        }
    }
    let _ = socket.shutdown(Shutdown::Write);
}

#[derive(Debug, PartialEq, Eq)]
enum Frame {
    Line,
    Oversized,
    Closed,
}

/// Reads one line of at most `max` bytes into `buffer`, without its newline.
/// The same bounded read as the debug protocol's framing.
fn read_line(reader: &mut impl BufRead, buffer: &mut Vec<u8>, max: usize) -> Frame {
    match Read::take(&mut *reader, max as u64 + 1).read_until(b'\n', buffer) {
        Ok(0) => Frame::Closed,
        Ok(_) if buffer.last() == Some(&b'\n') => {
            buffer.pop();
            Frame::Line
        }
        Ok(_) if buffer.len() > max => Frame::Oversized,
        // The peer stopped mid-line.
        Ok(_) | Err(_) => Frame::Closed,
    }
}

#[cfg(test)]
mod tests;
