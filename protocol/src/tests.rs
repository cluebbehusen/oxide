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

/// Exactly one sample per wire-enum variant. Paired with an exhaustive
/// tag match, a new variant cannot reach the wire unexercised: the match
/// stops compiling until it is indexed, and this stops passing until it
/// is sampled.
pub(crate) fn assert_every_tag_sampled(
    tags: impl IntoIterator<Item = usize>,
    variants: usize,
    what: &str,
) {
    let mut samples = vec![0usize; variants];
    for tag in tags {
        samples[tag] += 1;
    }
    for (tag, count) in samples.iter().enumerate() {
        assert_eq!(
            *count, 1,
            "{what} variant {tag} has {count} samples; every variant needs exactly one"
        );
    }
}

/// Contiguous index per [`Request`] variant, in declaration order.
fn request_tag(request: &Request) -> usize {
    match request {
        Request::Status => 0,
        Request::QueryState { .. } => 1,
        Request::QueryFogView { .. } => 2,
        Request::QueryCamera => 3,
        Request::QueryUi => 4,
        Request::QueryPerformance { .. } => 5,
        Request::BeginPerformanceWindow { .. } => 6,
        Request::StateHash => 7,
        Request::AdvanceTicks { .. } => 8,
        Request::PresentTicks { .. } => 9,
        Request::Pause => 10,
        Request::Resume => 11,
        Request::SetSpeed { .. } => 12,
        Request::SendCommand { .. } => 13,
        Request::InjectEvent { .. } => 14,
        Request::Screenshot { .. } => 15,
        Request::ToggleOverlay => 16,
        Request::LoadScenario { .. } => 17,
        Request::LoadReplay { .. } => 18,
        Request::SaveReplay { .. } => 19,
    }
}

const REQUEST_VARIANTS: usize = 20;

/// Contiguous index per [`Reply`] variant, in declaration order.
fn reply_tag(reply: &Reply) -> usize {
    match reply {
        Reply::Ok => 0,
        Reply::Status(_) => 1,
        Reply::State(_) => 2,
        Reply::Fog(_) => 3,
        Reply::Camera(_) => 4,
        Reply::Ui(_) => 5,
        Reply::Performance(_) => 6,
        Reply::Hash(_) => 7,
        Reply::Advanced(_) => 8,
        Reply::Presented(_) => 9,
        Reply::Screenshot(_) => 10,
        Reply::Overlay(_) => 11,
        Reply::Saved(_) => 12,
    }
}

const REPLY_VARIANTS: usize = 13;

/// Contiguous index per [`Command`] variant, in declaration order.
/// The sim's verbs ride this wire through [`Request::SendCommand`],
/// so they carry the same obligation as the protocol's own enums: a
/// new verb stops this match compiling until it is indexed, and the
/// sample test stops passing until it is exercised on the wire.
fn command_tag(command: &Command) -> usize {
    match command {
        Command::Run { .. } => 0,
        Command::Attack { .. } => 1,
        Command::Hunt { .. } => 2,
        Command::Harvest { .. } => 3,
        Command::Patrol { .. } => 4,
        Command::Stop { .. } => 5,
        Command::Train { .. } => 6,
        Command::Build { .. } => 7,
        Command::Cancel { .. } => 8,
        Command::Repair { .. } => 9,
        Command::Salvage { .. } => 10,
        Command::CancelTrain { .. } => 11,
        Command::SetRally { .. } => 12,
        Command::Surrender => 13,
        Command::RepairUnit { .. } => 14,
        Command::Advance { .. } => 15,
        Command::FocusFire { .. } => 16,
        Command::CancelFound { .. } => 17,
        Command::UpgradeBuilding { .. } => 18,
        Command::Load { .. } => 19,
        Command::Unload { .. } => 20,
        Command::ClearFocus { .. } => 21,
        Command::ReturnCargo { .. } => 22,
        Command::CancelOrder { .. } => 23,
    }
}

const COMMAND_VARIANTS: usize = 24;

#[test]
fn an_omitted_screenshot_path_survives_the_roundtrip() {
    // The sampled variant carries a path; its defaulted form is pinned
    // here, since serde only writes what is present.
    let req = Request::Screenshot { path: None };
    assert_eq!(roundtrip(&req), req);
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

#[test]
fn an_empty_visible_window_is_zero_zero_not_absent() {
    // main_menu's grid reports the run of cards actually drawn; a
    // window showing none is `[0, 0]`, which must stay distinct on
    // the wire from a mode with no menu (absent field). Scrolled
    // windows report a real sub-range, not `[0, items.len()]`.
    let ui = UiView {
        mode: "main_menu".into(),
        title: Some("OXIDE".into()),
        selected: Some(0),
        items: vec!["Skirmish".into()],
        visible_range: Some([0, 0]),
        hover: None,
        chrome: None,
        panel_regions: None,
        menu_button: None,
        pause_status: None,
        group_column: None,
    };
    let json = serde_json::to_string(&ResponseEnvelope::ok(3, Reply::Ui(ui.clone()))).unwrap();
    assert!(
        json.contains(r#""visible_range":[0,0]"#),
        "the empty window rides the wire explicitly: {json}"
    );
    assert_eq!(reply_roundtrip(&Reply::Ui(ui.clone())), Reply::Ui(ui));
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
        Request::QueryFogView {
            player: PlayerId(0),
        },
        Request::QueryCamera,
        Request::QueryUi,
        Request::QueryPerformance { reset: true },
        Request::BeginPerformanceWindow {
            from_tick: 100,
            to_tick: 200,
        },
        Request::StateHash,
        Request::AdvanceTicks { ticks: 12 },
        Request::PresentTicks { ticks: 2 },
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
    assert_every_tag_sampled(
        requests.iter().map(request_tag),
        REQUEST_VARIANTS,
        "request",
    );
    for req in requests {
        assert_eq!(
            roundtrip(&req),
            req,
            "request variant did not survive: {req:?}"
        );
    }
}

#[test]
fn every_command_variant_survives_the_send_command_wire() {
    use chassis::grid::TilePos;
    use oxide_sim::{BuildingId, BuildingKind, Target, UnitKind};
    let commands = vec![
        Command::ReturnCargo {
            units: vec![UnitId(4)],
            foundry: Some(BuildingId(0)),
            repair: true,
        },
        Command::Run {
            units: vec![UnitId(1)],
            goal: TilePos::new(3, 4),
            queue: true,
        },
        Command::Attack {
            units: vec![UnitId(2)],
            target: Target::Building(BuildingId(1)).into(),
            queue: false,
        },
        Command::Hunt {
            units: vec![UnitId(3)],
            goal: TilePos::new(5, 6),
            queue: false,
        },
        Command::Harvest {
            units: vec![UnitId(4)],
            node: TilePos::new(7, 2),
            queue: false,
        },
        Command::Patrol {
            units: vec![UnitId(5)],
            waypoints: vec![TilePos::new(1, 1), TilePos::new(2, 2)],
        },
        Command::Stop {
            units: vec![UnitId(6)],
        },
        Command::Train {
            building: BuildingId(0),
            kind: UnitKind::Harvester,
        },
        Command::Build {
            units: vec![UnitId(7)],
            kind: BuildingKind::Turret,
            anchor: TilePos::new(9, 9),
            queue: false,
            defer: false,
        },
        Command::Cancel {
            building: BuildingId(2),
        },
        Command::Repair {
            units: vec![UnitId(8)],
            building: BuildingId(3),
            queue: false,
        },
        Command::Salvage {
            units: vec![UnitId(9)],
            building: BuildingId(4),
            queue: false,
        },
        Command::CancelTrain {
            building: BuildingId(5),
            index: 1,
        },
        Command::SetRally {
            building: BuildingId(6),
            rally: Some(TilePos::new(4, 4)),
        },
        Command::Surrender,
        Command::RepairUnit {
            units: vec![UnitId(10)],
            target: UnitId(11),
            queue: false,
        },
        Command::Advance {
            units: vec![UnitId(12)],
            goal: TilePos::new(8, 5),
            queue: true,
        },
        Command::FocusFire {
            buildings: vec![BuildingId(8), BuildingId(7)],
            target: Target::Unit(UnitId(13)).into(),
        },
        Command::CancelFound {
            kind: BuildingKind::Array,
            anchor: TilePos::new(7, 5),
        },
        Command::UpgradeBuilding {
            building: BuildingId(2),
        },
        Command::Load {
            units: vec![UnitId(5), UnitId(6)],
            transport: UnitId(9),
            queue: false,
        },
        Command::ClearFocus {
            buildings: vec![BuildingId(0)],
        },
        Command::Unload {
            transport: UnitId(9),
            at: TilePos::new(11, 3),
            queue: true,
        },
        Command::CancelOrder {
            unit: UnitId(3),
            key: oxide_sim::OrderKey::Walk {
                tile: TilePos::new(6, 2),
            },
            from_end: 1,
            units: vec![UnitId(4), UnitId(5)],
        },
    ];
    assert_every_tag_sampled(
        commands.iter().map(command_tag),
        COMMAND_VARIANTS,
        "command",
    );
    for command in commands {
        let req = Request::SendCommand {
            player: PlayerId(0),
            command,
        };
        assert_eq!(
            roundtrip(&req),
            req,
            "command variant did not survive: {req:?}"
        );
    }
    // Unit variants ride as a bare tag — the exact string a client
    // sends for a concession.
    let json = serde_json::to_string(&Command::Surrender).unwrap();
    assert_eq!(json, r#"{"type":"surrender"}"#);
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
        Reply::Fog(FogView::capture(&state, PlayerId(0))),
        Reply::Camera(CameraView {
            center: [1.0, 2.0],
            zoom: 32.0,
            viewport: [800.0, 600.0],
            world_rect: [0.0, 0.0, 25.0, 18.0],
        }),
        Reply::Ui(UiView {
            panel_regions: Some([[0.0, 500.0, 228.0, 300.0], [228.0, 724.0, 500.0, 76.0]]),
            menu_button: Some([1238.0, 3.0, 34.0, 34.0]),
            pause_status: Some([1180.0, 3.0, 46.0, 34.0]),
            group_column: Some([1196.0, 380.0, 60.0, 236.0]),
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
        Reply::Performance(FrameProfileView {
            renderer: "gpu".into(),
            frames: 3,
            tick_start: 100,
            tick_end: 108,
            ticks_presented: 8,
            work: TimingSummary {
                mean_ms: 4.0,
                p50_ms: 3.0,
                p95_ms: 6.0,
                p99_ms: 6.0,
                max_ms: 6.0,
            },
            interval: TimingSummary {
                mean_ms: 16.7,
                p50_ms: 16.7,
                p95_ms: 17.0,
                p99_ms: 17.0,
                max_ms: 17.0,
            },
            work_over_16_7_ms: 0,
            work_over_33_3_ms: 0,
            slowest: Some(SlowFrameView {
                mode: "playback".into(),
                tick_start: 105,
                tick_end: 108,
                work_ms: 6.0,
                units: 20,
                buildings: 8,
            }),
            window: Some(FrameProfileWindowView {
                from_tick: 100,
                to_tick: 108,
                complete: true,
                elapsed_ms: 51.2,
                truncated: false,
            }),
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
        Reply::Presented(PresentedView {
            ticks: 2,
            tick: 17,
            hash: hash_hex(0xbcde),
            events: vec![Event::CommandRejected {
                player: PlayerId(0),
                reason: oxide_sim::command::RejectReason::BadSite,
            }],
        }),
        Reply::Screenshot(ScreenshotView {
            path: "shots/x.png".into(),
            width: 800,
            height: 600,
            renderer: "cpu".into(),
        }),
        Reply::Overlay(OverlayView { enabled: true }),
        Reply::Saved(SavedView {
            path: "replays/x.json".into(),
            commands: 42,
        }),
    ];
    assert_every_tag_sampled(replies.iter().map(reply_tag), REPLY_VARIANTS, "reply");
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
        r#"{"id":3,"id":4,"method":"status"}"#,       // duplicate id
    ] {
        assert!(
            serde_json::from_str::<RequestEnvelope>(line).is_err(),
            "expected a parse error for {line:?}"
        );
    }
}

#[test]
fn request_typos_are_rejected_instead_of_silently_ignored() {
    for line in [
        r#"{"id":4,"method":"advance_ticks","params":{"ticks":10,"tcks":20}}"#,
        r#"{"id":5,"method":"status","methdo":"pause"}"#,
        r#"{"id":6,"method":"send_command","params":{"player":0,"command":{"type":"stop","units":[],"unitz":[1]}}}"#,
        r#"{"id":7,"method":"inject_event","params":{"event":{"type":"mouse_move","x":1,"y":2,"why":"typo"}}}"#,
    ] {
        let error = serde_json::from_str::<RequestEnvelope>(line)
            .expect_err("a misspelled request field must fail closed");
        assert!(error.to_string().contains("unknown field"), "{error}");
    }
}

#[test]
fn typos_inside_command_values_are_rejected() {
    for (line, path) in [
        (
            r#"{"id":15,"method":"send_command","params":{"player":0,"command":{"type":"attack","units":[1],"target":{"kind":"remembered_building","id":{"owner":1,"building_kind":"reclaimer","anchor":{"x":3,"y":4,"z":0}}}}}}"#,
            "command.target.id.anchor",
        ),
        (
            r#"{"id":16,"method":"send_command","params":{"player":0,"command":{"type":"focus_fire","buildings":[1],"target":{"kind":"contact","id":0,"domain":"ground"}}}}"#,
            "command.target",
        ),
        (
            r#"{"id":8,"method":"send_command","params":{"player":0,"command":{"type":"run","units":[],"goal":{"x":3,"y":4,"z":99}}}}"#,
            "command.goal",
        ),
        (
            r#"{"id":9,"method":"send_command","params":{"player":0,"command":{"type":"attack","units":[],"target":{"kind":"unit","id":2,"player":1}}}}"#,
            "command.target",
        ),
        (
            r#"{"id":10,"method":"send_command","params":{"player":0,"command":{"type":"harvest","units":[],"node":{"x":3,"y":4,"amount":99}}}}"#,
            "command.node",
        ),
        (
            r#"{"id":11,"method":"send_command","params":{"player":0,"command":{"type":"patrol","units":[],"waypoints":[{"x":3,"y":4},{"x":5,"y":6,"wait":10}]}}}"#,
            "command.waypoints[1]",
        ),
        (
            r#"{"id":12,"method":"send_command","params":{"player":0,"command":{"type":"build","units":[],"kind":"turret","anchor":{"x":3,"y":4,"rotation":1}}}}"#,
            "command.anchor",
        ),
        (
            r#"{"id":13,"method":"send_command","params":{"player":0,"command":{"type":"set_rally","building":2,"rally":{"x":3,"y":4,"radius":2}}}}"#,
            "command.rally",
        ),
        (
            r#"{"id":14,"method":"send_command","params":{"player":0,"command":{"type":"unload","transport":2,"at":{"x":3,"y":4,"spread":2},"queue":false}}}"#,
            "command.at",
        ),
        (
            r#"{"id":17,"method":"send_command","params":{"player":0,"command":{"type":"cancel_order","unit":1,"key":{"order":"walk","tile":{"x":3,"y":4},"verb":"move"},"from_end":0}}}"#,
            "command.key",
        ),
        (
            r#"{"id":18,"method":"send_command","params":{"player":0,"command":{"type":"cancel_order","unit":1,"key":{"order":"walk","tile":{"x":3,"y":4,"z":1}},"from_end":0}}}"#,
            "command.key.tile",
        ),
        (
            r#"{"id":19,"method":"send_command","params":{"player":0,"command":{"type":"cancel_order","unit":1,"key":{"order":"found","kind":"turret","anchor":{"x":3,"y":4,"w":2}},"from_end":0}}}"#,
            "command.key.anchor",
        ),
        (
            r#"{"id":20,"method":"send_command","params":{"player":0,"command":{"type":"cancel_order","unit":1,"key":{"order":"attack","objective":{"kind":"remembered_building","id":{"owner":1,"building_kind":"reclaimer","anchor":{"x":3,"y":4,"z":0}}}},"from_end":0}}}"#,
            "command.key.objective.id.anchor",
        ),
        (
            r#"{"id":21,"method":"send_command","params":{"player":0,"command":{"type":"cancel_order","unit":1,"key":{"order":"attack","objective":{"kind":"contact","id":0,"domain":"air"}},"from_end":0}}}"#,
            "command.key.objective",
        ),
    ] {
        assert!(
            serde_json::from_str::<RequestEnvelope>(line).is_err(),
            "{path}: an unknown field nested inside a command must fail closed"
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
