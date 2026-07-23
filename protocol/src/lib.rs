//! The debug protocol: how anything outside the process drives a running
//! shell.
//!
//! Wire format is JSON Lines over TCP — one request object per line, one
//! response object per line, correlated by `id`:
//!
//! ```text
//! → {"id":1,"method":"advance_ticks","params":{"ticks":10}}
//! ← {"id":1,"ok":{"kind":"advanced","ticks":10,"tick":52,"hash":"0x9c…"}}
//! → {"id":2,"method":"nonsense"}
//! ← {"id":2,"err":"unknown method"}
//! ```
//!
//! Design intent: everything here is data an agent can read raw. Positions
//! come as floats (presentation precision is fine — exactness lives in the
//! sim and is fingerprinted by the state hash), hashes come as hex strings
//! (u64s don't survive every JSON tool), and the map comes back as ASCII.
//!
//! This crate is types only — no sockets. The shell serves it, the driver
//! speaks it, and both stay in lockstep by construction.

pub mod input;
pub mod view;

use oxide_sim::{Command, PlayerCommand, PlayerId};
use serde::{Deserialize, Serialize};

pub use input::{Key, MouseButton, RawEvent};
pub use view::{
    BuildingView, CameraView, PlayerView, StateFilter, StateView, StatusView, UiView, UnitView,
};

/// Default TCP port for `--debug-server`.
pub const DEFAULT_PORT: u16 = 4123;

/// Everything a client can ask of a running shell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Request {
    /// Who are you, what tick is it, are we paused.
    Status,
    /// A structured snapshot of sim state, filterable by section.
    QueryState {
        /// Which sections to include.
        #[serde(default)]
        filter: StateFilter,
    },
    /// Camera position, zoom, and visible world rectangle.
    QueryCamera,
    /// Shell mode and active menu state.
    QueryUi,
    /// The canonical state fingerprint at the current tick.
    StateHash,
    /// Run sim ticks now (bots included), regardless of pause state, then
    /// report the resulting tick and hash. Requests above one million
    /// ticks are capped; the reply's `ticks` field reports what actually
    /// ran.
    AdvanceTicks {
        /// How many ticks to run.
        ticks: u64,
    },
    /// Stop the wall clock driving the sim. Rendering continues.
    Pause,
    /// Resume wall-clock ticking.
    Resume,
    /// Scale wall-clock time (2.0 = double speed). Sim ticks are unchanged
    /// in size; they just fire more or less often.
    SetSpeed {
        /// Multiplier applied to real time.
        multiplier: f64,
    },
    /// Issue a game command as `player`, stamped for the next tick — the
    /// exact same funnel mouse clicks use.
    SendCommand {
        /// Acting player.
        player: PlayerId,
        /// The command.
        command: Command,
    },
    /// Push a synthetic input event into the shell's funnel,
    /// indistinguishable from hardware input.
    InjectEvent {
        /// The event.
        event: RawEvent,
    },
    /// Write the current frame to a PNG and return its path.
    Screenshot {
        /// Target path; defaults to `screenshots/tick-N.png`.
        #[serde(default)]
        path: Option<String>,
    },
    /// Toggle the debug overlay (grid, ids, paths, hp).
    ToggleOverlay,
    /// Replace the current match with a scenario file.
    LoadScenario {
        /// Path to a scenario JSON.
        path: String,
    },
    /// Resume a session from a replay file: rebuild its scenario, re-run
    /// every recorded tick (fast — the sim does thousands per second), and
    /// keep recording from there. In a deterministic sim, this *is* loading
    /// a save.
    LoadReplay {
        /// Path to a replay JSON.
        path: String,
    },
    /// Write the session so far as a replay JSON.
    SaveReplay {
        /// Target path.
        path: String,
    },
}

/// Successful response payloads, tagged by `kind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reply {
    /// Generic acknowledgement.
    Ok,
    /// Answer to [`Request::Status`].
    Status(StatusView),
    /// Answer to [`Request::QueryState`].
    State(StateView),
    /// Answer to [`Request::QueryCamera`].
    Camera(CameraView),
    /// Answer to [`Request::QueryUi`].
    Ui(UiView),
    /// Answer to [`Request::StateHash`].
    Hash(HashView),
    /// Answer to [`Request::AdvanceTicks`].
    Advanced(AdvancedView),
    /// Answer to [`Request::Screenshot`].
    Screenshot(ScreenshotView),
    /// Answer to [`Request::ToggleOverlay`].
    Overlay(OverlayView),
    /// Answer to [`Request::SaveReplay`].
    Saved(SavedView),
}

/// Tick + fingerprint pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HashView {
    /// Current tick.
    pub tick: u64,
    /// State hash as `0x`-prefixed hex.
    pub hash: String,
}

/// Result of a fast-forward.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdvancedView {
    /// Ticks actually run.
    pub ticks: u64,
    /// Tick counter afterwards.
    pub tick: u64,
    /// State hash afterwards, as hex.
    pub hash: String,
}

/// Where a screenshot landed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotView {
    /// PNG path (relative to the shell's working directory unless absolute).
    pub path: String,
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
}

/// Overlay toggle result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayView {
    /// Whether the overlay is now on.
    pub enabled: bool,
}

/// Where a replay landed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedView {
    /// Replay path.
    pub path: String,
    /// Commands recorded so far.
    pub commands: usize,
}

/// A request with its correlation id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    /// Client-chosen id, echoed in the response.
    pub id: u64,
    /// The request itself.
    #[serde(flatten)]
    pub request: Request,
}

/// A response with its correlation id. Internally an enum, so "both ok and
/// err" or "neither" are unrepresentable; on the wire it keeps the exact
/// original shape (`{"id":…,"ok":{…}}` / `{"id":…,"err":"…"}`), which the
/// `wire_shape_is_stable` test pins.
#[derive(Debug, Clone, PartialEq)]
pub struct ResponseEnvelope {
    /// Echo of the request id (0 when the request was unparseable).
    pub id: u64,
    outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq)]
enum Outcome {
    Ok(Reply),
    Err(String),
}

impl ResponseEnvelope {
    /// A success response.
    pub fn ok(id: u64, reply: Reply) -> Self {
        Self {
            id,
            outcome: Outcome::Ok(reply),
        }
    }

    /// A failure response.
    pub fn err(id: u64, message: impl Into<String>) -> Self {
        Self {
            id,
            outcome: Outcome::Err(message.into()),
        }
    }

    /// Consumes the envelope into the reply or the error message.
    pub fn into_result(self) -> Result<Reply, String> {
        match self.outcome {
            Outcome::Ok(reply) => Ok(reply),
            Outcome::Err(message) => Err(message),
        }
    }
}

impl Serialize for ResponseEnvelope {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct as _;
        let mut s = serializer.serialize_struct("ResponseEnvelope", 2)?;
        s.serialize_field("id", &self.id)?;
        match &self.outcome {
            Outcome::Ok(reply) => s.serialize_field("ok", reply)?,
            Outcome::Err(message) => s.serialize_field("err", message)?,
        }
        s.end()
    }
}

impl<'de> Deserialize<'de> for ResponseEnvelope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            id: u64,
            #[serde(default)]
            ok: Option<Reply>,
            #[serde(default)]
            err: Option<String>,
        }
        let raw = Raw::deserialize(deserializer)?;
        match (raw.ok, raw.err) {
            (Some(reply), None) => Ok(ResponseEnvelope::ok(raw.id, reply)),
            (None, Some(message)) => Ok(ResponseEnvelope::err(raw.id, message)),
            (Some(_), Some(_)) => Err(serde::de::Error::custom("response has both ok and err")),
            (None, None) => Err(serde::de::Error::custom("response has neither ok nor err")),
        }
    }
}

/// Formats a state hash the way the protocol expects it.
pub fn hash_hex(hash: u64) -> String {
    format!("{hash:#018x}")
}

/// Convenience: a [`Request::SendCommand`] for `player`.
pub fn send_command(player: PlayerId, command: Command) -> Request {
    Request::SendCommand { player, command }
}

/// Re-associates a send-command request with sim types.
pub fn to_player_command(player: PlayerId, command: Command) -> PlayerCommand {
    PlayerCommand { player, command }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_sim::UnitId;

    fn roundtrip(req: &Request) -> Request {
        let envelope = RequestEnvelope {
            id: 7,
            request: req.clone(),
        };
        let json = serde_json::to_string(&envelope).unwrap();
        let back: RequestEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, 7);
        back.request
    }

    #[test]
    fn requests_roundtrip() {
        for req in [
            Request::Status,
            Request::QueryState {
                filter: StateFilter::default(),
            },
            Request::QueryUi,
            Request::AdvanceTicks { ticks: 99 },
            Request::SendCommand {
                player: PlayerId(0),
                command: Command::Stop {
                    units: vec![UnitId(3)],
                },
            },
            Request::InjectEvent {
                event: RawEvent::Wheel { delta: -1.0 },
            },
            Request::Screenshot { path: None },
        ] {
            assert_eq!(roundtrip(&req), req);
        }
    }

    #[test]
    fn wire_shape_is_stable() {
        // The exact strings agents see; breaking these breaks every client.
        let json = serde_json::to_string(&RequestEnvelope {
            id: 1,
            request: Request::AdvanceTicks { ticks: 10 },
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"id":1,"method":"advance_ticks","params":{"ticks":10}}"#
        );

        let json = serde_json::to_string(&ResponseEnvelope::err(2, "unknown method")).unwrap();
        assert_eq!(json, r#"{"id":2,"err":"unknown method"}"#);
    }

    #[test]
    fn unit_variant_requests_need_no_params() {
        let req: RequestEnvelope = serde_json::from_str(r#"{"id":5,"method":"status"}"#).unwrap();
        assert_eq!(req.request, Request::Status);
    }

    #[test]
    fn hash_hex_is_fixed_width() {
        assert_eq!(hash_hex(0x1234), "0x0000000000001234");
    }

    fn reply_roundtrip(reply: &Reply) -> Reply {
        let json = serde_json::to_string(&ResponseEnvelope::ok(9, reply.clone())).unwrap();
        let back: ResponseEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, 9);
        back.into_result().expect("an ok envelope yields its reply")
    }

    #[test]
    fn every_request_variant_survives_an_envelope_roundtrip() {
        let requests = vec![
            Request::Status,
            Request::QueryState {
                filter: StateFilter::default(),
            },
            Request::QueryCamera,
            Request::QueryUi,
            Request::StateHash,
            Request::AdvanceTicks { ticks: 12 },
            Request::Pause,
            Request::Resume,
            Request::SetSpeed { multiplier: 2.5 },
            Request::SendCommand {
                player: PlayerId(1),
                command: Command::Stop {
                    units: vec![UnitId(4)],
                },
            },
            Request::InjectEvent {
                event: RawEvent::KeyDown { key: Key::Space },
            },
            Request::Screenshot {
                path: Some("shots/tick-1.png".into()),
            },
            Request::ToggleOverlay,
            Request::LoadScenario {
                path: "scenarios/x.json".into(),
            },
            Request::LoadReplay {
                path: "replays/x.json".into(),
            },
            Request::SaveReplay {
                path: "replays/y.json".into(),
            },
        ];
        // A new method with no entry here escapes the wire round-trip.
        assert_eq!(requests.len(), 16);
        for req in requests {
            assert_eq!(
                roundtrip(&req),
                req,
                "request variant did not survive: {req:?}"
            );
        }
    }

    #[test]
    fn every_reply_variant_survives_an_envelope_roundtrip() {
        let state = oxide_sim::Scenario::skirmish().build().unwrap();
        let full = StateView::capture(
            &state,
            StateFilter {
                map: true,
                ..StateFilter::default()
            },
        );
        let replies = vec![
            Reply::Ok,
            Reply::Status(StatusView {
                tick: 5,
                paused: true,
                speed: 1.0,
                scenario: "skirmish".into(),
                sim_version: "9.9.9".into(),
                result: Some(oxide_sim::GameResult::Victory { team: 1 }),
                recorded_commands: 3,
            }),
            Reply::State(full),
            Reply::Camera(CameraView {
                center: [1.0, 2.0],
                zoom: 32.0,
                viewport: [800.0, 600.0],
                world_rect: [0.0, 0.0, 25.0, 18.0],
            }),
            Reply::Ui(UiView {
                mode: "main_menu".into(),
                title: Some("Oxide".into()),
                selected: Some(2),
                items: vec!["Skirmish".into(), "Quit".into()],
                visible_range: Some([0, 2]),
                hover: Some(1),
                chrome: Some([
                    32.0, 764.0, 1048.0, 616.0, 220.0, 150.0, 900.0, 0.0, 500.0, 60.0, 200.0,
                ]),
            }),
            Reply::Hash(HashView {
                tick: 5,
                hash: hash_hex(0x1234),
            }),
            Reply::Advanced(AdvancedView {
                ticks: 10,
                tick: 15,
                hash: hash_hex(0xabcd),
            }),
            Reply::Screenshot(ScreenshotView {
                path: "shots/x.png".into(),
                width: 800,
                height: 600,
            }),
            Reply::Overlay(OverlayView { enabled: true }),
            Reply::Saved(SavedView {
                path: "replays/x.json".into(),
                commands: 42,
            }),
        ];
        // A new reply kind with no entry here escapes the wire round-trip.
        assert_eq!(replies.len(), 10);
        for reply in replies {
            assert_eq!(
                reply_roundtrip(&reply),
                reply,
                "reply variant did not survive: {reply:?}"
            );
        }
    }

    #[test]
    fn malformed_request_lines_error_instead_of_panicking() {
        // A debug socket feeds untrusted lines; every one of these must come
        // back as a parse error, never a panic that takes the shell down.
        for line in [
            r#"{"id":3,"method":"nonsense"}"#,            // unknown method tag
            "this is not json at all",                    // not JSON
            r#"{"method":"status"}"#,                     // missing id
            r#"{"id":"not-a-number","method":"status"}"#, // id wrong type
            r#"{"id":4,"method":"advance_ticks"}"#,       // required params absent
        ] {
            assert!(
                serde_json::from_str::<RequestEnvelope>(line).is_err(),
                "expected a parse error for {line:?}"
            );
        }
    }

    #[test]
    fn a_response_with_both_ok_and_err_is_rejected() {
        // The envelope is an enum on the wire: "ok and err at once" is a
        // corrupt frame, not two answers.
        let err =
            serde_json::from_str::<ResponseEnvelope>(r#"{"id":1,"ok":{"kind":"ok"},"err":"boom"}"#)
                .unwrap_err();
        assert!(err.to_string().contains("both"), "{err}");
    }

    #[test]
    fn a_response_with_neither_ok_nor_err_is_rejected() {
        let err = serde_json::from_str::<ResponseEnvelope>(r#"{"id":1}"#).unwrap_err();
        assert!(err.to_string().contains("neither"), "{err}");
    }
}
