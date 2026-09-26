# oxide-net

`oxide-net` is the session core for host-scheduled lockstep multiplayer. Every
machine runs the full simulation; only commands and session messages cross the
network. The host decides which commands execute on which tick, and every
machine executes the same ordered batches.

The core performs no I/O and reads no clock. Callers pass received lines and a
monotonic `now`, and collect lines to send. It does not own sockets, framing,
the connection handshake, bots, recording, or `State` itself: hosts keep their
existing batch path of staged human commands, then bot commands, then record,
then `State::tick`.

## Main pieces

- `message` defines the JSON-lines wire messages. Decoding rejects unknown
  message fields and tags; a line that fails to decode is a protocol error.
- `HostSession` stamps each arriving command for the next unsealed tick, orders
  a sealed tick's human commands, gates sealing on client progress, compares
  client hash reports, and watches client liveness.
- `ClientSession` gates execution on received batches, acknowledges every
  executed batch, and watches host liveness.

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
- Each side heartbeats when it has sent nothing for `HEARTBEAT_INTERVAL`.
  `HostSession::poll` must run on the host's game loop: a host whose loop hangs
  stops heartbeating, and clients end the session after `SILENCE_TIMEOUT`.
  Clients therefore need no progress timeout of their own.

## Development

Run commands from the workspace root:

```sh
cargo test -p oxide-net --locked
cargo test -p oxide-driver --test lockstep --locked
cargo clippy -p oxide-net --all-targets --locked -- -D warnings
```
