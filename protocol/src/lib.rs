#![doc = include_str!("../README.md")]

pub mod framing;
pub mod input;
pub mod session;
pub mod view;

use oxide_sim::{Command, Event, OrderKey, PlayerId};
use serde::{Deserialize, Serialize};

pub use input::{Key, MouseButton, RawEvent};
pub use session::{DebugSession, check_speed, dispatch_shared};
pub use view::{
    BuildingView, CameraView, FogView, GhostView, PlayerView, RememberedTileView, StateFilter,
    StateView, StatusView, UiView, UnitView,
};

/// Default TCP port for `--debug-server`.
pub const DEFAULT_PORT: u16 = 4123;

/// Largest fast-forward request the shell will execute in one operation.
pub const MAX_ADVANCE_TICKS: u64 = 1_000_000;

/// Largest presentation-preserving step the shell will execute at once.
pub const MAX_PRESENT_TICKS: u64 = 120;

/// Ticks per second both sides budget for a synchronous advance —
/// deliberately conservative against the harness benchmarks, so a slow
/// machine still finishes inside the deadline. The client derives its
/// read timeout from this and the server its reply deadline; sharing one
/// figure is what keeps the two from ever disagreeing about a legal wait.
pub const ADVANCE_TICKS_PER_BUDGET_SECOND: u64 = 1_000;

/// Longest request line a server accepts, newline excluded. A line that
/// runs past it is answered with an error naming the limit and the
/// connection is closed — framing has to bound its own allocation, and no
/// legitimate request comes within three orders of magnitude of this.
pub const MAX_FRAME_BYTES: usize = 1 << 20;

/// Ceiling on a RESPONSE line a client should accept, newline excluded.
/// Requests are hand-sized and [`MAX_FRAME_BYTES`] bounds them; replies scale with
/// the world (a deep `query_state`, a long presented-event drain), so
/// the client's ceiling is generous rather than symmetric — a server
/// never refuses its own legal reply.
pub const MAX_RESPONSE_BYTES: usize = 64 << 20;

/// Everything a client can ask of a running shell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "method",
    content = "params",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Request {
    /// Who are you, what tick is it, are we paused.
    Status,
    /// A structured snapshot of sim state, filterable by section.
    QueryState {
        /// Which sections to include.
        #[serde(default)]
        filter: StateFilter,
    },
    /// The world as one seat honestly knows it: its own economy and command
    /// eligibility, visibility mask, entities filtered to current sight,
    /// ghost memories, remembered salvage, and radar contacts. The fog-safe
    /// counterpart to the omniscient [`Request::QueryState`] — what lets an
    /// agent play fair.
    QueryFogView {
        /// The seat whose knowledge to report.
        player: PlayerId,
    },
    /// Camera position, zoom, and visible world rectangle.
    QueryCamera,
    /// Shell mode and active menu state.
    QueryUi,
    /// Timing summary from the real window's opt-in frame profiler. A
    /// windowless session refuses this instead of reporting CPU-renderer
    /// timings as though they described the native shell.
    QueryPerformance {
        /// Clear samples after taking the snapshot.
        #[serde(default)]
        reset: bool,
    },
    /// Arm an exact native Playing-screen profile window. The current visible
    /// tick must equal `from_tick`; the shell records every active Playing
    /// frame while the live match spans the window and auto-pauses on `to_tick`.
    BeginPerformanceWindow {
        /// Exact live tick at which measurement begins.
        from_tick: u64,
        /// Exact live tick at which measurement ends and the shell pauses.
        to_tick: u64,
    },
    /// The canonical state fingerprint at the current tick.
    StateHash,
    /// Run sim ticks now (bots included), regardless of pause state, then
    /// report the resulting tick and hash. Requests above
    /// [`MAX_ADVANCE_TICKS`] are capped; the reply's `ticks` field reports
    /// what actually ran.
    AdvanceTicks {
        /// How many ticks to run.
        ticks: u64,
    },
    /// Run a small number of ticks without suppressing presentation, then
    /// return every sim event produced. This is the deterministic way for
    /// an agent to observe command rejection, shots, deaths, and transient
    /// shell feedback. Requests above [`MAX_PRESENT_TICKS`] are capped.
    PresentTicks {
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
    /// Answer to [`Request::QueryFogView`].
    Fog(FogView),
    /// Answer to [`Request::QueryCamera`].
    Camera(CameraView),
    /// Answer to [`Request::QueryUi`].
    Ui(UiView),
    /// Answer to [`Request::QueryPerformance`].
    Performance(FrameProfileView),
    /// Answer to [`Request::StateHash`].
    Hash(HashView),
    /// Answer to [`Request::AdvanceTicks`].
    Advanced(AdvancedView),
    /// Answer to [`Request::PresentTicks`].
    Presented(PresentedView),
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

/// Result of a presentation-preserving step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresentedView {
    /// Ticks actually run.
    pub ticks: u64,
    /// Tick counter afterwards.
    pub tick: u64,
    /// State hash afterwards, as hex.
    pub hash: String,
    /// Events emitted across the interval, in tick and event order.
    pub events: Vec<Event>,
}

/// Distribution summary for one frame-time measurement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimingSummary {
    /// Arithmetic mean.
    pub mean_ms: f64,
    /// Median.
    pub p50_ms: f64,
    /// 95th percentile.
    pub p95_ms: f64,
    /// 99th percentile.
    pub p99_ms: f64,
    /// Largest sample.
    pub max_ms: f64,
}

/// The slowest completed frame in a native-shell timing window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlowFrameView {
    /// Active screen while the frame ran.
    pub mode: String,
    /// Visible simulation tick at frame start.
    pub tick_start: u64,
    /// Visible simulation tick after the frame's update.
    pub tick_end: u64,
    /// CPU work between frame entry and the presentation handoff.
    pub work_ms: f64,
    /// Units in the visible world after the update.
    pub units: usize,
    /// Buildings in the visible world after the update.
    pub buildings: usize,
}

/// Bounded timing snapshot collected by a real GPU-backed shell window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameProfileView {
    /// Always `"gpu"`; the field prevents accidental comparison with the
    /// headless session's schematic CPU renderer.
    pub renderer: String,
    /// Completed frames represented by the summary.
    pub frames: usize,
    /// First visible tick in the sample window.
    pub tick_start: u64,
    /// Last visible tick in the sample window.
    pub tick_end: u64,
    /// Simulation ticks presented across the sampled frames.
    pub ticks_presented: u64,
    /// CPU work performed by the shell per frame.
    pub work: TimingSummary,
    /// Wall intervals between consecutive sampled frame starts.
    pub interval: TimingSummary,
    /// Frames whose CPU work exceeded a 60 Hz budget.
    pub work_over_16_7_ms: usize,
    /// Frames whose CPU work exceeded two 60 Hz budgets.
    pub work_over_33_3_ms: usize,
    /// Largest-work frame, absent when no completed frame was sampled.
    pub slowest: Option<SlowFrameView>,
    /// Exact-window metadata when profiling was armed through
    /// [`Request::BeginPerformanceWindow`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<FrameProfileWindowView>,
}

/// State of one shell-side exact profile window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameProfileWindowView {
    /// Requested first tick.
    pub from_tick: u64,
    /// Requested exclusive end tick.
    pub to_tick: u64,
    /// Whether the shell reached `to_tick` and auto-paused.
    pub complete: bool,
    /// Wall time from the first sampled frame's start through the final
    /// sampled frame's completed CPU work.
    pub elapsed_ms: f64,
    /// Whether bounded retention evicted any sampled frame.
    pub truncated: bool,
}

/// Where a screenshot landed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotView {
    /// PNG path (relative to the server's working directory unless absolute).
    pub path: String,
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// Which renderer produced the pixels: `"gpu"` is the shell's real
    /// frame, `"cpu"` the headless session's schematic tiny-skia render.
    /// An agent judging visual polish must know which one it is reading.
    #[serde(default = "gpu_renderer")]
    pub renderer: String,
}

fn gpu_renderer() -> String {
    "gpu".to_string()
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
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RequestEnvelope {
    /// Client-chosen id, echoed in the response.
    pub id: u64,
    /// The request itself.
    #[serde(flatten)]
    pub request: Request,
}

#[derive(Default)]
enum WireParams {
    #[default]
    Missing,
    Present(serde_json::Value),
}

impl<'de> Deserialize<'de> for WireParams {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        serde_json::Value::deserialize(deserializer).map(Self::Present)
    }
}

impl<'de> Deserialize<'de> for RequestEnvelope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WireEnvelope {
            id: u64,
            method: String,
            #[serde(default)]
            params: WireParams,
        }

        let raw = WireEnvelope::deserialize(deserializer)?;
        let mut request = serde_json::Map::from_iter([(
            "method".to_string(),
            serde_json::Value::String(raw.method),
        )]);
        if let WireParams::Present(params) = raw.params {
            request.insert("params".to_string(), params);
        }
        let wire = serde_json::Value::Object(request);
        let request = serde_json::from_value(wire.clone()).map_err(D::Error::custom)?;
        reject_unknown_nested_fields(&request, &wire).map_err(D::Error::custom)?;
        Ok(Self {
            id: raw.id,
            request,
        })
    }
}

fn reject_unknown_nested_fields(request: &Request, wire: &serde_json::Value) -> Result<(), String> {
    let (field, allowed): (&str, &[&str]) = match request {
        Request::SendCommand { command, .. } => ("command", command_wire_fields(command)),
        Request::InjectEvent { event } => ("event", event_wire_fields(event)),
        _ => return Ok(()),
    };
    let Some(object) = wire
        .get("params")
        .and_then(|params| params.get(field))
        .and_then(serde_json::Value::as_object)
    else {
        return Ok(());
    };
    if let Some(unknown) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("unknown field `{unknown}` in {field}"));
    }
    match request {
        Request::SendCommand { command, .. } => {
            reject_unknown_command_value_fields(command, object)
        }
        Request::InjectEvent { .. } => Ok(()),
        _ => unreachable!("only requests with nested wire values reach this point"),
    }
}

fn reject_unknown_command_value_fields(
    command: &Command,
    wire: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    match command {
        Command::Run { .. } | Command::Hunt { .. } | Command::Advance { .. } => {
            reject_unknown_object_fields(wire.get("goal"), "command.goal", &["x", "y"])
        }
        Command::Attack { target, .. } | Command::FocusFire { target, .. } => {
            reject_unknown_attack_target_fields(wire.get("target"), "command.target", target)
        }
        Command::CancelOrder { key, .. } => {
            let wire = wire.get("key");
            reject_unknown_object_fields(wire, "command.key", order_key_wire_fields(key))?;
            let field = |name| wire.and_then(|key| key.get(name));
            match key {
                OrderKey::Walk { .. } | OrderKey::Unload { .. } => {
                    reject_unknown_object_fields(field("tile"), "command.key.tile", &["x", "y"])
                }
                OrderKey::Land { .. } => {
                    reject_unknown_object_fields(field("pad"), "command.key.pad", &["x", "y"])
                }
                OrderKey::Harvest { .. } | OrderKey::Found { .. } => {
                    reject_unknown_object_fields(field("anchor"), "command.key.anchor", &["x", "y"])
                }
                OrderKey::Attack { objective } => reject_unknown_attack_target_fields(
                    field("objective"),
                    "command.key.objective",
                    objective,
                ),
                OrderKey::ReturnCargo
                | OrderKey::Build { .. }
                | OrderKey::Repair { .. }
                | OrderKey::Salvage { .. }
                | OrderKey::RepairUnit { .. }
                | OrderKey::Board { .. } => Ok(()),
            }
        }
        Command::Harvest { .. } => {
            reject_unknown_object_fields(wire.get("node"), "command.node", &["x", "y"])
        }
        Command::Patrol { .. } => {
            let Some(waypoints) = wire.get("waypoints").and_then(serde_json::Value::as_array)
            else {
                return Ok(());
            };
            for (index, waypoint) in waypoints.iter().enumerate() {
                reject_unknown_object_fields(
                    Some(waypoint),
                    &format!("command.waypoints[{index}]"),
                    &["x", "y"],
                )?;
            }
            Ok(())
        }
        Command::Build { .. } | Command::CancelFound { .. } => {
            reject_unknown_object_fields(wire.get("anchor"), "command.anchor", &["x", "y"])
        }
        Command::SetRally { .. } => {
            reject_unknown_object_fields(wire.get("rally"), "command.rally", &["x", "y"])
        }
        Command::Unload { .. } => {
            reject_unknown_object_fields(wire.get("at"), "command.at", &["x", "y"])
        }
        Command::ReturnCargo { .. }
        | Command::Stop { .. }
        | Command::Train { .. }
        | Command::Cancel { .. }
        | Command::Repair { .. }
        | Command::Salvage { .. }
        | Command::CancelTrain { .. }
        | Command::Surrender
        | Command::RepairUnit { .. }
        | Command::UpgradeBuilding { .. }
        | Command::Load { .. }
        | Command::ClearFocus { .. } => Ok(()),
    }
}

fn reject_unknown_attack_target_fields(
    value: Option<&serde_json::Value>,
    path: &str,
    target: &oxide_sim::AttackTarget,
) -> Result<(), String> {
    reject_unknown_object_fields(value, path, &["kind", "id"])?;
    if matches!(target, oxide_sim::AttackTarget::RememberedBuilding(_)) {
        let memory = value.and_then(|target| target.get("id"));
        reject_unknown_object_fields(
            memory,
            &format!("{path}.id"),
            &["owner", "building_kind", "anchor"],
        )?;
        reject_unknown_object_fields(
            memory.and_then(|v| v.get("anchor")),
            &format!("{path}.id.anchor"),
            &["x", "y"],
        )?;
    }
    Ok(())
}

fn order_key_wire_fields(key: &OrderKey) -> &'static [&'static str] {
    match key {
        OrderKey::Walk { tile: _ } | OrderKey::Unload { tile: _ } => &["order", "tile"],
        OrderKey::Attack { objective: _ } => &["order", "objective"],
        OrderKey::Land { pad: _ } => &["order", "pad"],
        OrderKey::Harvest { anchor: _ } => &["order", "anchor"],
        OrderKey::ReturnCargo => &["order"],
        OrderKey::Build { site: _ } => &["order", "site"],
        OrderKey::Found { kind: _, anchor: _ } => &["order", "kind", "anchor"],
        OrderKey::Repair { building: _ } | OrderKey::Salvage { building: _ } => {
            &["order", "building"]
        }
        OrderKey::RepairUnit { unit: _ } => &["order", "unit"],
        OrderKey::Board { transport: _ } => &["order", "transport"],
    }
}

fn reject_unknown_object_fields(
    value: Option<&serde_json::Value>,
    path: &str,
    allowed: &[&str],
) -> Result<(), String> {
    let Some(object) = value.and_then(serde_json::Value::as_object) else {
        return Ok(());
    };
    if let Some(unknown) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("unknown field `{unknown}` in {path}"));
    }
    Ok(())
}

fn command_wire_fields(command: &Command) -> &'static [&'static str] {
    match command {
        Command::ClearFocus { .. } => &["type", "buildings"],
        Command::Run {
            units: _,
            goal: _,
            queue: _,
        }
        | Command::Hunt {
            units: _,
            goal: _,
            queue: _,
        }
        | Command::Advance {
            units: _,
            goal: _,
            queue: _,
        } => &["type", "units", "goal", "queue"],
        Command::Attack {
            units: _,
            target: _,
            queue: _,
        } => &["type", "units", "target", "queue"],
        Command::Harvest {
            units: _,
            node: _,
            queue: _,
        } => &["type", "units", "node", "queue"],
        Command::Patrol {
            units: _,
            waypoints: _,
        } => &["type", "units", "waypoints"],
        Command::ReturnCargo {
            units: _,
            foundry: _,
            repair: _,
        } => &["type", "units", "foundry", "repair"],
        Command::Stop { units: _ } => &["type", "units"],
        Command::Train {
            building: _,
            kind: _,
        } => &["type", "building", "kind"],
        Command::Build {
            units: _,
            kind: _,
            anchor: _,
            queue: _,
            defer: _,
        } => &["type", "units", "kind", "anchor", "queue", "defer"],
        Command::Cancel { building: _ } | Command::UpgradeBuilding { building: _ } => {
            &["type", "building"]
        }
        Command::Repair {
            units: _,
            building: _,
            queue: _,
        }
        | Command::Salvage {
            units: _,
            building: _,
            queue: _,
        } => &["type", "units", "building", "queue"],
        Command::CancelTrain {
            building: _,
            index: _,
        } => &["type", "building", "index"],
        Command::SetRally {
            building: _,
            rally: _,
        } => &["type", "building", "rally"],
        Command::Surrender => &["type"],
        Command::RepairUnit {
            units: _,
            target: _,
            queue: _,
        } => &["type", "units", "target", "queue"],
        Command::FocusFire {
            buildings: _,
            target: _,
        } => &["type", "buildings", "target"],
        Command::CancelFound { kind: _, anchor: _ } => &["type", "kind", "anchor"],
        Command::Load {
            units: _,
            transport: _,
            queue: _,
        } => &["type", "units", "transport", "queue"],
        Command::Unload {
            transport: _,
            at: _,
            queue: _,
        } => &["type", "transport", "at", "queue"],
        Command::CancelOrder {
            unit: _,
            key: _,
            from_end: _,
            units: _,
        } => &["type", "unit", "key", "from_end", "units"],
    }
}

fn event_wire_fields(event: &RawEvent) -> &'static [&'static str] {
    match event {
        RawEvent::MouseMove { x: _, y: _ } => &["type", "x", "y"],
        RawEvent::MouseDown {
            button: _,
            x: _,
            y: _,
        }
        | RawEvent::MouseUp {
            button: _,
            x: _,
            y: _,
        } => &["type", "button", "x", "y"],
        RawEvent::Wheel { delta: _ } => &["type", "delta"],
        RawEvent::KeyDown { key: _ } | RawEvent::KeyUp { key: _ } => &["type", "key"],
        RawEvent::TouchDown { id: _, x: _, y: _ }
        | RawEvent::TouchMove { id: _, x: _, y: _ }
        | RawEvent::TouchUp { id: _, x: _, y: _ } => &["type", "id", "x", "y"],
        RawEvent::Text { ch: _ } => &["type", "ch"],
    }
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
    Ok(Box<Reply>),
    Err(String),
}

impl ResponseEnvelope {
    /// A success response.
    pub fn ok(id: u64, reply: Reply) -> Self {
        Self {
            id,
            outcome: Outcome::Ok(Box::new(reply)),
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
            Outcome::Ok(reply) => Ok(*reply),
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

#[cfg(test)]
mod tests;
