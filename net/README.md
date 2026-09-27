# oxide-net

`oxide-net` carries host-scheduled lockstep multiplayer. Every machine runs the
full simulation; only commands and session messages cross the network. The host
decides which commands execute on which tick, and every machine executes the
same ordered batches.

The session core and start barrier perform no I/O and read no clock. Callers
pass received lines and a monotonic `now`, and collect lines to send. The `tcp`
module is the crate's only I/O. The crate does not own bots, recording, or
`State` itself: hosts keep their existing batch path of staged human commands,
then bot commands, then record, then `State::tick`.

## Main pieces

- `message` defines the JSON-lines wire messages: `JoinMessage` and
  `LobbyMessage` before the match, `ClientMessage` and `HostMessage` after Go.
  Decoding rejects unknown tags and unknown fields on messages that carry data;
  a line that fails to decode is a protocol error.
- `StartBarrier` takes a frozen roster to tick zero.
- `HostSession` stamps each arriving command for the next unsealed tick, orders
  a sealed tick's human commands, gates sealing on client progress, compares
  client hash reports, and watches client liveness.
- `ClientSession` gates execution on received batches, acknowledges every
  executed batch, and watches host liveness.
- `tcp` provides `Listener` and `Connection`, which move lines between machines.

## Joining and starting

- Each side opens with `Hello`, carrying `PROTOCOL_VERSION` and the build
  commit, and hangs up if the other side's does not match. Hello's shape never
  changes, so mismatched builds can still explain the refusal; any other wire
  change bumps `PROTOCOL_VERSION`. Local changes on top of a matching commit are
  the players' responsibility.
- The host freezes the roster, which must hold exactly the scenario's human
  seats, and `StartBarrier` sends each client its seat and the `Scenario`. Each
  client builds the match and replies `Ready` with its tick-zero `State::hash`.
  When every hash matches the host's, the barrier queues `Go` and returns the
  `HostSession`, whose liveness clocks start there. Clients start their
  `ClientSession` on `Go`.
- A mismatched hash, a stray line, a seated connection closing, or
  `START_TIMEOUT` fails the start. The host closes every seated connection and
  keeps listening; clients rejoin, so no line from a failed start can reach the
  next one.

## Session contract

- A batch for tick N turns state N into state N+1. Clients acknowledge each
  batch with the resulting tick and attach `State::hash` when that tick is a
  multiple of `HASH_INTERVAL`.
- Human seats go first in a sealed batch, in an order that rotates each tick
  (starting at `tick % human seats`), so neither the host nor the lowest-latency
  client always wins same-tick conflicts. Each seat's commands keep arrival
  order. Hosts append bot commands after the sealed humans.
- The host seals tick N only while N is less than every live client's
  acknowledged tick plus `LEAD_CAP`.
- A client the host has been blocked on for `PROGRESS_TIMEOUT` is dropped, even
  if its heartbeats continue. A client silent for `SILENCE_TIMEOUT`, a closed
  connection, or a protocol violation also drops it. A dropped seat leaves the
  progress gate at once, and its `Surrender` joins the next sealed batch.
- A mismatched hash report halts the session and tells every client.
  `HostSession::caught_up` reports when every live client has acknowledged every
  published batch; a host ending a decided match waits for it so the final
  reports are still checked.
- Each side heartbeats when it has sent nothing for `HEARTBEAT_INTERVAL`.
  `HostSession::poll` must run on the host's game loop: a host whose loop hangs
  stops heartbeating, and clients end the session after `SILENCE_TIMEOUT`.
  Clients therefore need no progress timeout of their own.

## Transport

- Lines are newline-delimited and at most `MAX_LINE_BYTES`. An oversized or
  non-UTF-8 line shuts the connection down in both directions.
- Each `Connection` has a reader thread and a writer thread, so a slow peer
  never blocks the game loop that polls it. Received lines wait in a bounded
  queue; when the game loop falls behind, the reader stops reading and TCP
  pushes back on the peer. Outgoing lines are bounded by the session's lead cap
  and progress timeout. A write that stalls for ten seconds or fails closes the
  connection. There is no read timeout: the lobby may wait indefinitely, and
  in-match liveness belongs to the session core.
- Dropping a `Connection` aborts it and discards unsent lines. To close cleanly,
  call `finish` and keep polling until `Closed` before dropping; after `finish`,
  `Closed` waits until every queued line has been written, so the final lines
  reach the peer.
- `Listener::try_accept` never blocks, so the game loop can poll it. Bind a
  non-loopback address for other machines to reach.

## Development

Run commands from the workspace root:

```sh
cargo test -p oxide-net --locked
cargo test -p oxide-driver --test lockstep --locked
cargo clippy -p oxide-net --all-targets --locked -- -D warnings
```
