//! The `driver live` vocabulary: every debug-socket subcommand, its
//! translation into protocol requests (parsed fully before any socket
//! is touched), and the capture-sequence contact-sheet helper.

use crate::parse::{
    BuildingKindArg, UnitKindArg, parse_key, parse_mouse_button, parse_point, parse_tile,
};
use anyhow::{Context, Result, bail};
use clap::Subcommand;
use oxide_driver::client::Client;
use oxide_protocol::{Key, RawEvent, Request, RequestEnvelope, StateFilter};
use oxide_sim::{BuildingId, Command, PlayerId, Target, UnitId};

#[derive(Subcommand)]
pub(crate) enum LiveCmd {
    /// Tick, pause state, scenario, versions.
    Status,
    /// Structured sim snapshot.
    State {
        /// Include the ASCII map.
        #[arg(long)]
        map: bool,
    },
    /// The world as one seat honestly knows it: visibility mask, entities
    /// under current sight, ghost memories, remembered salvage, contacts.
    Fog {
        /// The seat whose knowledge to report.
        player: u8,
    },
    /// Camera pose and visible world rect.
    Camera,
    /// Shell mode and active menu state.
    Ui,
    /// Native GPU-shell frame timing collected under --profile-frames.
    Performance {
        /// Clear the completed timing window after taking the snapshot.
        #[arg(long)]
        reset: bool,
    },
    /// Canonical state fingerprint.
    Hash,
    /// Fast-forward N ticks (works while paused — that's the point).
    Advance {
        /// Tick count.
        ticks: u64,
    },
    /// Advance a small number of ticks while preserving transient
    /// presentation and returning the sim events.
    Step {
        /// Tick count (capped at 120 by the protocol).
        ticks: u64,
    },
    /// Stop the wall clock (rendering continues).
    Pause,
    /// Restart the wall clock.
    Resume,
    /// Wall-clock speed multiplier.
    Speed {
        /// e.g. 4.0 for fast-forward, 0.25 for slow motion.
        multiplier: f64,
    },
    /// Send a raw sim command as JSON (see oxide-sim's Command).
    Send {
        /// Acting player index.
        player: u8,
        /// Command JSON, e.g. `{"type":"stop","units":[3]}`.
        json: String,
    },
    /// Attack-move units to a tile (engage everything on the way).
    AttackMove {
        /// Acting player index.
        player: u8,
        /// Unit ids, comma-separated.
        #[arg(long, value_delimiter = ',')]
        units: Vec<u32>,
        /// Goal as "x,y".
        #[arg(long)]
        to: String,
        /// Append behind current orders instead of replacing them.
        #[arg(long)]
        queue: bool,
    },
    /// Resume a session from a replay file (fast-forwards, keeps recording).
    LoadReplay {
        /// Replay JSON path.
        path: String,
    },
    /// Move units to a tile.
    Move {
        /// Acting player index.
        player: u8,
        /// Unit ids, comma-separated.
        #[arg(long, value_delimiter = ',')]
        units: Vec<u32>,
        /// Goal as "x,y".
        #[arg(long)]
        to: String,
        /// Append behind current orders instead of replacing them.
        #[arg(long)]
        queue: bool,
    },
    /// Advance units to a tile, firing in-range primary weapons without
    /// stopping or chasing.
    AdvanceUnits {
        /// Acting player index.
        player: u8,
        /// Unit ids, comma-separated.
        #[arg(long, value_delimiter = ',')]
        units: Vec<u32>,
        /// Goal as "x,y".
        #[arg(long)]
        to: String,
        /// Append behind current orders instead of replacing them.
        #[arg(long)]
        queue: bool,
    },
    /// Walk units on a looping circuit, engaging everything met.
    Patrol {
        /// Acting player index.
        player: u8,
        /// Unit ids, comma-separated.
        #[arg(long, value_delimiter = ',')]
        units: Vec<u32>,
        /// Waypoint as "x,y"; repeat for each stop on the circuit.
        #[arg(long = "via")]
        via: Vec<String>,
    },
    /// Attack an enemy unit.
    AttackUnit {
        /// Acting player index.
        player: u8,
        /// Unit ids, comma-separated.
        #[arg(long, value_delimiter = ',')]
        units: Vec<u32>,
        /// Victim unit id.
        #[arg(long)]
        target: u32,
        /// Append behind current orders instead of replacing them.
        #[arg(long)]
        queue: bool,
    },
    /// Attack an enemy building.
    AttackBuilding {
        /// Acting player index.
        player: u8,
        /// Unit ids, comma-separated.
        #[arg(long, value_delimiter = ',')]
        units: Vec<u32>,
        /// Victim building id.
        #[arg(long)]
        target: u32,
        /// Append behind current orders instead of replacing them.
        #[arg(long)]
        queue: bool,
    },
    /// Put harvesters on a scrap node.
    Harvest {
        /// Acting player index.
        player: u8,
        /// Unit ids, comma-separated.
        #[arg(long, value_delimiter = ',')]
        units: Vec<u32>,
        /// Node tile as "x,y".
        #[arg(long)]
        node: String,
        /// Append behind current orders instead of replacing them.
        #[arg(long)]
        queue: bool,
    },
    /// Queue a unit at a Foundry.
    Train {
        /// Acting player index.
        player: u8,
        /// Producing building id.
        #[arg(long)]
        building: u32,
        /// What to train.
        #[arg(long, value_enum)]
        kind: UnitKindArg,
    },
    /// Start a construction site with a harvester.
    Build {
        /// Acting player index.
        player: u8,
        /// Candidate builder unit ids, comma-separated.
        #[arg(long, value_delimiter = ',')]
        units: Vec<u32>,
        /// What to construct.
        #[arg(long, value_enum)]
        kind: BuildingKindArg,
        /// Anchor tile as "x,y" (top-left of the footprint).
        #[arg(long)]
        at: String,
        /// Append behind current orders instead of replacing them.
        #[arg(long)]
        queue: bool,
        /// Defer the claim: walk out and found on arrival (validated
        /// against the seat's knowledge, paid when the ground is
        /// claimed) — the shell's explored-but-unseen mode.
        #[arg(long)]
        defer: bool,
    },
    /// Send harvesters to weld a damaged own built building.
    Repair {
        /// Acting player index.
        player: u8,
        /// Welder unit ids, comma-separated.
        #[arg(long, value_delimiter = ',')]
        units: Vec<u32>,
        /// The building to weld.
        #[arg(long)]
        building: u32,
        /// Append behind current orders instead of replacing them.
        #[arg(long)]
        queue: bool,
    },
    /// Send harvesters to weld a wounded own ground unit (billed per
    /// hp against the patient's cost; air patients refuse).
    RepairUnit {
        /// Acting player index.
        player: u8,
        /// Welder unit ids, comma-separated.
        #[arg(long, value_delimiter = ',')]
        units: Vec<u32>,
        /// The wounded machine.
        #[arg(long)]
        target: u32,
        /// Append behind current orders instead of replacing them.
        #[arg(long)]
        queue: bool,
    },
    /// Send harvesters to strip an own built building for a partial
    /// refund (Foundries refuse; sites keep Cancel).
    Salvage {
        /// Acting player index.
        player: u8,
        /// Stripper unit ids, comma-separated.
        #[arg(long, value_delimiter = ',')]
        units: Vec<u32>,
        /// The building coming down.
        #[arg(long)]
        building: u32,
        /// Append behind current orders instead of replacing them.
        #[arg(long)]
        queue: bool,
    },
    /// Scrap an own unfinished site for a partial refund.
    Cancel {
        /// Acting player index.
        player: u8,
        /// The site's building id.
        #[arg(long)]
        building: u32,
    },
    /// Set (or clear) a building's rally point.
    Rally {
        /// Acting player index.
        player: u8,
        /// The building id.
        #[arg(long)]
        building: u32,
        /// Rally tile as "x,y" (omit with --clear).
        #[arg(long, conflicts_with = "clear")]
        tile: Option<String>,
        /// Clear the rally instead.
        #[arg(long)]
        clear: bool,
    },
    /// Halt units.
    Stop {
        /// Acting player index.
        player: u8,
        /// Unit ids, comma-separated.
        #[arg(long, value_delimiter = ',')]
        units: Vec<u32>,
    },
    /// Inject a scroll-wheel event into the input funnel.
    InjectWheel {
        /// Notches; positive zooms in.
        delta: f32,
    },
    /// Inject a key press (and release).
    InjectKey {
        /// A mapped key: arrows, h/s/a/p/r/b/n/x, enter, escape, space,
        /// f1, shift, ctrl, or 1-9.
        key: String,
    },
    /// Inject a key press WITHOUT the release — held-key states (panning,
    /// modifiers) stay held until inject-key-up.
    InjectKeyDown {
        /// A mapped key, as inject-key accepts.
        key: String,
    },
    /// Inject a key release without a press.
    InjectKeyUp {
        /// A mapped key, as inject-key accepts.
        key: String,
    },
    /// Inject a chord: every key pressed in order, then released in
    /// reverse — `ctrl+1` assigns a control group exactly like a hand.
    InjectChord {
        /// Keys joined with '+', e.g. "ctrl+1" or "shift+f1".
        keys: String,
    },
    /// Inject typed text, one Text event per character — drives the
    /// save-name field the way a keyboard's char stream does.
    InjectText {
        /// Printable ASCII; anything else is refused.
        text: String,
    },
    /// Inject a cursor move.
    InjectMouseMove {
        /// Window x.
        x: f32,
        /// Window y.
        y: f32,
    },
    /// Inject a mouse-button press without releasing it.
    InjectMouseDown {
        /// "left", "right", or "middle".
        button: String,
        /// Window x.
        x: f32,
        /// Window y.
        y: f32,
    },
    /// Inject a mouse-button release without pressing it.
    InjectMouseUp {
        /// "left", "right", or "middle".
        button: String,
        /// Window x.
        x: f32,
        /// Window y.
        y: f32,
    },
    /// Inject a full click (down + up) at a window position.
    InjectClick {
        /// "left", "right", or "middle".
        button: String,
        /// Window x.
        x: f32,
        /// Window y.
        y: f32,
    },
    /// Drag between two window positions over several rendered frames.
    InjectDrag {
        /// Start as "x,y" window coordinates.
        #[arg(long)]
        from: String,
        /// End as "x,y" window coordinates.
        #[arg(long)]
        to: String,
        /// Mouse-move events between press and release (1-120).
        #[arg(long, default_value_t = 6)]
        steps: u32,
        /// "left", "right", or "middle".
        #[arg(long, default_value = "left")]
        button: String,
    },
    /// Capture the current frame to a PNG.
    Screenshot {
        /// Output path (shell-relative unless absolute).
        #[arg(short, long)]
        out: Option<String>,
    },
    /// Capture a frame sequence with sim ticks between frames, plus a
    /// downscaled contact sheet for reading motion at a glance.
    CaptureSequence {
        /// Frames to capture (2-64).
        #[arg(long, default_value_t = 8)]
        frames: u32,
        /// Sim ticks advanced between frames.
        #[arg(long, default_value_t = 5)]
        ticks_between: u64,
        /// Preserve transient effects between frames instead of using
        /// presentation-suppressing bulk advances.
        #[arg(long)]
        present: bool,
        /// Output directory for frame-NNN.png and sheet.png.
        #[arg(short, long)]
        out: std::path::PathBuf,
    },
    /// Toggle the debug overlay.
    Overlay,
    /// Swap in another scenario file.
    Load {
        /// Scenario JSON path.
        path: String,
    },
    /// Save the session replay.
    SaveReplay {
        /// Output path.
        path: String,
    },
}

fn units(ids: Vec<u32>) -> Vec<UnitId> {
    ids.into_iter().map(UnitId).collect()
}

pub(crate) fn live_requests(cmd: LiveCmd) -> Result<Vec<Request>> {
    Ok(vec![match cmd {
        LiveCmd::Status => Request::Status,
        LiveCmd::State { map } => Request::QueryState {
            filter: StateFilter {
                map,
                ..StateFilter::default()
            },
        },
        LiveCmd::Fog { player } => Request::QueryFogView {
            player: PlayerId(player),
        },
        LiveCmd::Camera => Request::QueryCamera,
        LiveCmd::Ui => Request::QueryUi,
        LiveCmd::Performance { reset } => Request::QueryPerformance { reset },
        LiveCmd::Hash => Request::StateHash,
        LiveCmd::Advance { ticks } => Request::AdvanceTicks { ticks },
        LiveCmd::Step { ticks } => Request::PresentTicks { ticks },
        LiveCmd::Pause => Request::Pause,
        LiveCmd::Resume => Request::Resume,
        LiveCmd::Speed { multiplier } => Request::SetSpeed { multiplier },
        LiveCmd::Send { player, json } => {
            let command =
                serde_json::from_str::<serde_json::Value>(&json).context("parsing command JSON")?;
            let envelope = serde_json::json!({
                "id": 0,
                "method": "send_command",
                "params": {
                    "player": player,
                    "command": command,
                },
            });
            serde_json::from_value::<RequestEnvelope>(envelope)
                .context("parsing command JSON")?
                .request
        }
        LiveCmd::Move {
            player,
            units: ids,
            to,
            queue,
        } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::Move {
                units: units(ids),
                goal: parse_tile(&to)?,
                queue,
            },
        },
        LiveCmd::AdvanceUnits {
            player,
            units: ids,
            to,
            queue,
        } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::Advance {
                units: units(ids),
                goal: parse_tile(&to)?,
                queue,
            },
        },
        LiveCmd::Patrol {
            player,
            units: ids,
            via,
        } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::Patrol {
                units: units(ids),
                waypoints: via
                    .iter()
                    .map(|w| parse_tile(w))
                    .collect::<Result<Vec<_>>>()?,
            },
        },
        LiveCmd::AttackMove {
            player,
            units: ids,
            to,
            queue,
        } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::AttackMove {
                units: units(ids),
                goal: parse_tile(&to)?,
                queue,
            },
        },
        LiveCmd::LoadReplay { path } => Request::LoadReplay { path },
        LiveCmd::AttackUnit {
            player,
            units: ids,
            target,
            queue,
        } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::Attack {
                units: units(ids),
                target: Target::Unit(UnitId(target)),
                queue,
            },
        },
        LiveCmd::AttackBuilding {
            player,
            units: ids,
            target,
            queue,
        } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::Attack {
                units: units(ids),
                target: Target::Building(BuildingId(target)),
                queue,
            },
        },
        LiveCmd::Harvest {
            player,
            units: ids,
            node,
            queue,
        } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::Harvest {
                units: units(ids),
                node: parse_tile(&node)?,
                queue,
            },
        },
        LiveCmd::Train {
            player,
            building,
            kind,
        } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::Train {
                building: BuildingId(building),
                kind: kind.into(),
            },
        },
        LiveCmd::Stop { player, units: ids } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::Stop { units: units(ids) },
        },
        LiveCmd::Build {
            player,
            units: ids,
            kind,
            at,
            queue,
            defer,
        } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::Build {
                units: units(ids),
                kind: kind.into(),
                anchor: parse_tile(&at)?,
                queue,
                defer,
            },
        },
        LiveCmd::Repair {
            player,
            units: ids,
            building,
            queue,
        } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::Repair {
                units: units(ids),
                building: BuildingId(building),
                queue,
            },
        },
        LiveCmd::RepairUnit {
            player,
            units: ids,
            target,
            queue,
        } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::RepairUnit {
                units: units(ids),
                target: UnitId(target),
                queue,
            },
        },
        LiveCmd::Salvage {
            player,
            units: ids,
            building,
            queue,
        } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::Salvage {
                units: units(ids),
                building: BuildingId(building),
                queue,
            },
        },
        LiveCmd::Cancel { player, building } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::Cancel {
                building: BuildingId(building),
            },
        },
        LiveCmd::Rally {
            player,
            building,
            tile,
            clear,
        } => {
            let rally = match (tile, clear) {
                (Some(t), _) => Some(parse_tile(&t)?),
                (None, true) => None,
                (None, false) => bail!("pass --tile x,y or --clear"),
            };
            Request::SendCommand {
                player: PlayerId(player),
                command: Command::SetRally {
                    building: BuildingId(building),
                    rally,
                },
            }
        }
        LiveCmd::InjectWheel { delta } => Request::InjectEvent {
            event: RawEvent::Wheel { delta },
        },
        LiveCmd::InjectKey { key } => {
            // A tap: down then up, so held-key panning can't get stuck on.
            let key = parse_key(&key)?;
            return Ok(vec![
                Request::InjectEvent {
                    event: RawEvent::KeyDown { key },
                },
                Request::InjectEvent {
                    event: RawEvent::KeyUp { key },
                },
            ]);
        }
        LiveCmd::InjectKeyDown { key } => Request::InjectEvent {
            event: RawEvent::KeyDown {
                key: parse_key(&key)?,
            },
        },
        LiveCmd::InjectKeyUp { key } => Request::InjectEvent {
            event: RawEvent::KeyUp {
                key: parse_key(&key)?,
            },
        },
        LiveCmd::InjectChord { keys } => {
            let keys: Vec<Key> = keys
                .split('+')
                .map(|part| parse_key(part.trim()))
                .collect::<Result<_>>()?;
            if keys.is_empty() {
                bail!("a chord needs at least one key");
            }
            // Down in written order, up in reverse — modifiers wrap the
            // core key the way a hand holds them.
            let mut requests: Vec<Request> = keys
                .iter()
                .map(|&key| Request::InjectEvent {
                    event: RawEvent::KeyDown { key },
                })
                .collect();
            requests.extend(keys.iter().rev().map(|&key| Request::InjectEvent {
                event: RawEvent::KeyUp { key },
            }));
            return Ok(requests);
        }
        LiveCmd::InjectText { text } => {
            // The shell filters ingest to printable ASCII; refusing here
            // tells the caller instead of silently dropping characters.
            if let Some(bad) = text.chars().find(|c| !('\u{20}'..='\u{7e}').contains(c)) {
                bail!("inject-text takes printable ASCII only (got {bad:?})");
            }
            return Ok(text
                .chars()
                .map(|ch| Request::InjectEvent {
                    event: RawEvent::Text { ch },
                })
                .collect());
        }
        LiveCmd::InjectMouseMove { x, y } => Request::InjectEvent {
            event: RawEvent::MouseMove { x, y },
        },
        LiveCmd::InjectMouseDown { button, x, y } => Request::InjectEvent {
            event: RawEvent::MouseDown {
                button: parse_mouse_button(&button)?,
                x,
                y,
            },
        },
        LiveCmd::InjectMouseUp { button, x, y } => Request::InjectEvent {
            event: RawEvent::MouseUp {
                button: parse_mouse_button(&button)?,
                x,
                y,
            },
        },
        LiveCmd::InjectClick { button, x, y } => {
            let button = parse_mouse_button(&button)?;
            // A click is a pair; the shell treats a lone down as a drag start.
            return Ok(vec![
                Request::InjectEvent {
                    event: RawEvent::MouseDown { button, x, y },
                },
                Request::InjectEvent {
                    event: RawEvent::MouseUp { button, x, y },
                },
            ]);
        }
        LiveCmd::InjectDrag {
            from,
            to,
            steps,
            button,
        } => {
            if !(1..=120).contains(&steps) {
                bail!("drag steps must be within 1..=120");
            }
            let (from_x, from_y) = parse_point(&from)?;
            let (to_x, to_y) = parse_point(&to)?;
            let button = parse_mouse_button(&button)?;
            let mut requests = Vec::with_capacity(steps as usize + 2);
            requests.push(Request::InjectEvent {
                event: RawEvent::MouseDown {
                    button,
                    x: from_x,
                    y: from_y,
                },
            });
            for step in 1..=steps {
                let t = step as f32 / steps as f32;
                requests.push(Request::InjectEvent {
                    event: RawEvent::MouseMove {
                        x: from_x + (to_x - from_x) * t,
                        y: from_y + (to_y - from_y) * t,
                    },
                });
            }
            requests.push(Request::InjectEvent {
                event: RawEvent::MouseUp {
                    button,
                    x: to_x,
                    y: to_y,
                },
            });
            return Ok(requests);
        }
        LiveCmd::Screenshot { out } => Request::Screenshot { path: out },
        LiveCmd::CaptureSequence { .. } => {
            bail!("capture-sequence is executed directly, not mapped to requests")
        }
        LiveCmd::Overlay => Request::ToggleOverlay,
        LiveCmd::Load { path } => Request::LoadScenario { path },
        LiveCmd::SaveReplay { path } => Request::SaveReplay { path },
    }])
}

/// Drives a capture run: advance, screenshot, repeat, then tile every
/// frame (quarter scale) into one contact sheet for reading motion at a
/// glance. Frames land as `frame-NNN.png` beside `sheet.png`.
pub(crate) fn capture_sequence(
    addr: &str,
    frames: u32,
    ticks_between: u64,
    present: bool,
    out: &std::path::Path,
) -> Result<()> {
    if !(2..=64).contains(&frames) {
        bail!("frames must be within 2..=64");
    }
    if present && ticks_between > oxide_protocol::MAX_PRESENT_TICKS {
        bail!(
            "--present supports at most {} ticks between frames",
            oxide_protocol::MAX_PRESENT_TICKS
        );
    }
    std::fs::create_dir_all(out)?;
    let out = out.canonicalize()?;
    let mut client = Client::connect(addr)?;
    let mut paths = Vec::new();
    for i in 0..frames {
        if i > 0 {
            let request = if present {
                Request::PresentTicks {
                    ticks: ticks_between,
                }
            } else {
                Request::AdvanceTicks {
                    ticks: ticks_between,
                }
            };
            client.call(request)?;
        }
        let path = out.join(format!("frame-{i:03}.png"));
        client.call(Request::Screenshot {
            path: Some(path.to_string_lossy().into_owned()),
        })?;
        paths.push(path);
    }

    let first = tiny_skia::Pixmap::decode_png(&std::fs::read(&paths[0])?)
        .context("decoding first frame")?;
    const SHEET_SCALE: f32 = 0.25;
    let tile_w = (first.width() as f32 * SHEET_SCALE).ceil() as u32;
    let tile_h = (first.height() as f32 * SHEET_SCALE).ceil() as u32;
    let columns = (frames as f32).sqrt().ceil() as u32;
    let rows = frames.div_ceil(columns);
    let mut sheet = tiny_skia::Pixmap::new(columns * tile_w, rows * tile_h)
        .context("allocating contact sheet")?;
    for (i, path) in paths.iter().enumerate() {
        let frame = tiny_skia::Pixmap::decode_png(&std::fs::read(path)?)
            .with_context(|| format!("decoding {}", path.display()))?;
        let (col, row) = (i as u32 % columns, i as u32 / columns);
        sheet.draw_pixmap(
            0,
            0,
            frame.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            tiny_skia::Transform::from_scale(SHEET_SCALE, SHEET_SCALE)
                .post_translate((col * tile_w) as f32, (row * tile_h) as f32),
            None,
        );
    }
    let sheet_path = out.join("sheet.png");
    sheet.save_png(&sheet_path)?;
    eprintln!("wrote {} frames and {}", frames, sheet_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_protocol::MouseButton;

    #[test]
    fn step_uses_the_presentation_preserving_protocol_method() {
        assert_eq!(
            live_requests(LiveCmd::Step { ticks: 7 }).unwrap(),
            vec![Request::PresentTicks { ticks: 7 }]
        );
    }

    #[test]
    fn advance_units_speaks_the_zero_chase_sim_verb() {
        assert_eq!(
            live_requests(LiveCmd::AdvanceUnits {
                player: 1,
                units: vec![7, 3],
                to: "12,9".to_string(),
                queue: true,
            })
            .unwrap(),
            vec![Request::SendCommand {
                player: PlayerId(1),
                command: Command::Advance {
                    units: vec![UnitId(7), UnitId(3)],
                    goal: chassis::grid::TilePos::new(12, 9),
                    queue: true,
                },
            }]
        );
    }

    #[test]
    fn raw_commands_use_the_strict_debug_wire_boundary() {
        let request = live_requests(LiveCmd::Send {
            player: 1,
            json: r#"{"type":"stop","units":[7,3]}"#.to_string(),
        })
        .unwrap();
        assert_eq!(
            request,
            vec![Request::SendCommand {
                player: PlayerId(1),
                command: Command::Stop {
                    units: vec![UnitId(7), UnitId(3)],
                },
            }]
        );

        let error = live_requests(LiveCmd::Send {
            player: 1,
            json: r#"{"type":"stop","units":[7],"unitz":[3]}"#.to_string(),
        })
        .expect_err("unknown command fields must fail before connecting");
        assert!(
            format!("{error:#}").contains("unknown field `unitz` in command"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn presented_capture_rejects_an_unbounded_interval_before_connecting() {
        let error = capture_sequence(
            "127.0.0.1:1",
            2,
            oxide_protocol::MAX_PRESENT_TICKS + 1,
            true,
            std::path::Path::new("not-created"),
        )
        .expect_err("the interval is invalid independently of a live shell");
        assert!(error.to_string().contains("at most 120"));
    }

    #[test]
    fn a_chord_presses_in_order_and_releases_in_reverse() {
        let requests = live_requests(LiveCmd::InjectChord {
            keys: "ctrl+1".to_string(),
        })
        .unwrap();
        let events: Vec<&RawEvent> = requests
            .iter()
            .map(|r| match r {
                Request::InjectEvent { event } => event,
                other => panic!("chords are pure injections, got {other:?}"),
            })
            .collect();
        assert!(
            matches!(
                events[..],
                [
                    RawEvent::KeyDown { key: Key::Ctrl },
                    RawEvent::KeyDown { key: Key::Num1 },
                    RawEvent::KeyUp { key: Key::Num1 },
                    RawEvent::KeyUp { key: Key::Ctrl },
                ]
            ),
            "modifiers must wrap the core key: {events:?}"
        );
    }

    #[test]
    fn a_chord_of_nonsense_fails_before_touching_the_socket() {
        assert!(
            live_requests(LiveCmd::InjectChord {
                keys: "ctrl+florb".to_string(),
            })
            .is_err()
        );
    }

    #[test]
    fn drag_expands_to_press_moves_and_release() {
        let requests = live_requests(LiveCmd::InjectDrag {
            from: "10,20".to_string(),
            to: "40,50".to_string(),
            steps: 3,
            button: "left".to_string(),
        })
        .unwrap();
        assert_eq!(requests.len(), 5);
        assert_eq!(
            requests[0],
            Request::InjectEvent {
                event: RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x: 10.0,
                    y: 20.0,
                }
            }
        );
        assert_eq!(
            requests[2],
            Request::InjectEvent {
                event: RawEvent::MouseMove { x: 30.0, y: 40.0 }
            }
        );
        assert_eq!(
            requests[4],
            Request::InjectEvent {
                event: RawEvent::MouseUp {
                    button: MouseButton::Left,
                    x: 40.0,
                    y: 50.0,
                }
            }
        );
    }

    #[test]
    fn drag_rejects_unbounded_event_counts() {
        let err = live_requests(LiveCmd::InjectDrag {
            from: "0,0".to_string(),
            to: "1,1".to_string(),
            steps: 121,
            button: "left".to_string(),
        })
        .unwrap_err();
        assert!(err.to_string().contains("1..=120"));
    }

    #[test]
    fn every_protocol_key_is_cli_addressable() {
        fn canonical_spelling(key: Key) -> &'static str {
            match key {
                Key::Up => "up",
                Key::Down => "down",
                Key::Left => "left",
                Key::Right => "right",
                Key::H => "h",
                Key::S => "s",
                Key::A => "a",
                Key::P => "p",
                Key::R => "r",
                Key::B => "b",
                Key::N => "n",
                Key::X => "x",
                Key::Enter => "enter",
                Key::Escape => "escape",
                Key::Space => "space",
                Key::F1 => "f1",
                Key::Shift => "shift",
                Key::Ctrl => "ctrl",
                Key::Num1 => "1",
                Key::Num2 => "2",
                Key::Num3 => "3",
                Key::Num4 => "4",
                Key::Num5 => "5",
                Key::Num6 => "6",
                Key::Num7 => "7",
                Key::Num8 => "8",
                Key::Num9 => "9",
                Key::PageUp => "pageup",
                Key::PageDown => "pagedown",
                Key::Home => "home",
                Key::End => "end",
                Key::F5 => "f5",
                Key::F6 => "f6",
                Key::F7 => "f7",
                Key::F8 => "f8",
                Key::C => "c",
                Key::D => "d",
                Key::E => "e",
                Key::F => "f",
                Key::G => "g",
                Key::I => "i",
                Key::J => "j",
                Key::K => "k",
                Key::L => "l",
                Key::M => "m",
                Key::O => "o",
                Key::Q => "q",
                Key::T => "t",
                Key::U => "u",
                Key::V => "v",
                Key::W => "w",
                Key::Y => "y",
                Key::Z => "z",
                Key::Backspace => "backspace",
            }
        }

        for key in [
            Key::Up,
            Key::Down,
            Key::Left,
            Key::Right,
            Key::H,
            Key::S,
            Key::A,
            Key::P,
            Key::R,
            Key::B,
            Key::N,
            Key::X,
            Key::Enter,
            Key::Escape,
            Key::Space,
            Key::F1,
            Key::Shift,
            Key::Ctrl,
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
            Key::PageUp,
            Key::PageDown,
            Key::Home,
            Key::End,
            Key::F5,
            Key::F6,
            Key::F7,
            Key::F8,
            Key::C,
            Key::D,
            Key::E,
            Key::F,
            Key::G,
            Key::I,
            Key::J,
            Key::K,
            Key::L,
            Key::M,
            Key::O,
            Key::Q,
            Key::T,
            Key::U,
            Key::V,
            Key::W,
            Key::Y,
            Key::Z,
            Key::Backspace,
        ] {
            assert_eq!(
                parse_key(canonical_spelling(key)).unwrap(),
                key,
                "CLI spelling did not round-trip {key:?}"
            );
        }
    }
}
