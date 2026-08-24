# oxide-protocol

`oxide-protocol` is the shared contract between a running Oxide session and the
tools that inspect or drive it. It defines JSON-lines requests and replies,
fog-honest and omniscient views, raw input events, and the bounded TCP framing
used by both the live shell and the windowless driver session.

This crate describes and dispatches the wire vocabulary. It does not own game
rules, rendering, or shell-specific behavior; capability-specific requests are
handled or explicitly refused by the session serving them.

## Main pieces

- `Request`, `Reply`, and their envelopes are the public wire types.
- `view` turns exact simulation state into readable protocol snapshots.
- `input` defines the hardware-neutral event stream used by automation.
- `framing` owns line limits, deadlines, connection handling, and response
  correlation.
- `session` implements the request surface shared by live, replay, and
  windowless sessions.

Request decoding is strict: unknown envelope or method-parameter fields,
including fields inside commands and injected input events, are errors. A
misspelled harness instruction cannot appear to succeed.

## Development

Run commands from the workspace root:

```sh
cargo test -p oxide-protocol --locked
cargo test -p oxide-driver --test session_parity --locked
cargo clippy -p oxide-protocol --all-targets --locked -- -D warnings
```
