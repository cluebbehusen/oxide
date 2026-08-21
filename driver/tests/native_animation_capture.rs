//! Native-shell animation inspection harness.
//!
//! This is deliberately an ignored integration test: it opens a real GPU
//! window and writes review artifacts, so the headless workspace suite must
//! never run it implicitly. It drives the paused shell exclusively through
//! the public debug protocol. `PresentTicks` advances both simulation and
//! presentation by exact tick time, making repeated captures land on the
//! same action frames without wall-clock races.
//!
//! ```text
//! cargo test -p oxide-driver --test native_animation_capture \
//!     -- --ignored --test-threads 1 --nocapture
//! ```
//!
//! Set `OXIDE_ANIMATION_CAPTURE_DIR` to choose the output directory. Without
//! it, each invocation writes a fresh `screenshots/native-animation/run-PID`
//! tree. Every stage contains full-size frames, a quarter-size contact sheet,
//! and a manifest recording the represented tick and sim events for each
//! frame.

use anyhow::{Context, Result, bail};
use chassis::grid::TilePos;
use oxide_driver::auto::{ShellGuard, SpawnOptions, spawn_shell, ui};
use oxide_driver::client::Client;
use oxide_protocol::{Reply, Request, StateFilter, StateView};
use oxide_sim::stats::Domain;
use oxide_sim::{BuildingId, BuildingKind, Command, PlayerId, Target, UnitId, UnitKind};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_HOME: AtomicU64 = AtomicU64::new(0);

const MAP_WIDTH: i32 = 32;
const MAP_HEIGHT: i32 = 22;
const ALL_UNIT_KINDS: [UnitKind; 11] = [
    UnitKind::Harvester,
    UnitKind::Sentinel,
    UnitKind::Scuttler,
    UnitKind::Lancer,
    UnitKind::Bombard,
    UnitKind::Flakhound,
    UnitKind::Stinger,
    UnitKind::Buzzard,
    UnitKind::Darter,
    UnitKind::Talon,
    UnitKind::Wisp,
];

struct NativeCapture {
    shell: Option<ShellGuard>,
    client: Client,
    home: PathBuf,
    output: PathBuf,
}

impl NativeCapture {
    fn spawn() -> Result<Self> {
        let home = scratch_home()?;
        write_config(&home)?;
        let port = 43_000 + (std::process::id() % 1_000) as u16;
        let spawned = spawn_shell(&SpawnOptions {
            port,
            paused: true,
            home: Some(home.clone()),
        });
        let (shell, client) = match spawned {
            Ok(pair) => pair,
            Err(error) => {
                std::fs::remove_dir_all(&home).ok();
                return Err(error);
            }
        };
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let output = std::env::var_os("OXIDE_ANIMATION_CAPTURE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                root.join("screenshots/native-animation")
                    .join(format!("run-{}", std::process::id()))
            });
        std::fs::create_dir_all(&output)?;
        Ok(Self {
            shell: Some(shell),
            client,
            home,
            output,
        })
    }

    fn load(&mut self, scenario: Value) -> Result<StateView> {
        let path = self.home.join("animation-stage.json");
        std::fs::write(&path, serde_json::to_vec_pretty(&scenario)?)?;
        expect_ok(self.client.call(Request::LoadScenario {
            path: path.to_string_lossy().into_owned(),
        })?)?;
        expect_ok(self.client.call(Request::Pause)?)?;
        for _ in 0..50 {
            if ui(&mut self.client)?.mode == "playing" {
                return self.state();
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        bail!("loaded animation scenario never reached the playing screen")
    }

    fn state(&mut self) -> Result<StateView> {
        match self.client.call(Request::QueryState {
            filter: StateFilter::default(),
        })? {
            Reply::State(view) => Ok(view),
            other => bail!("query_state returned {other:?}"),
        }
    }

    fn command(&mut self, player: u8, command: Command) -> Result<()> {
        expect_ok(self.client.call(Request::SendCommand {
            player: PlayerId(player),
            command,
        })?)
    }

    fn present(&mut self, ticks: u64) -> Result<Vec<oxide_sim::Event>> {
        match self.client.call(Request::PresentTicks { ticks })? {
            Reply::Presented(view) if view.ticks == ticks => Ok(view.events),
            other => bail!("present_ticks({ticks}) returned {other:?}"),
        }
    }

    fn clear_presentation(&mut self) -> Result<()> {
        match self.client.call(Request::AdvanceTicks { ticks: 0 })? {
            Reply::Advanced(view) if view.ticks == 0 => Ok(()),
            other => bail!("advance_ticks(0) returned {other:?}"),
        }
    }

    fn capture_stage(
        &mut self,
        label: &str,
        frames: u32,
        ticks_between: u64,
    ) -> Result<Vec<oxide_sim::Event>> {
        let mut tick_steps = vec![0; frames as usize];
        tick_steps.iter_mut().skip(1).for_each(|step| {
            *step = ticks_between;
        });
        self.capture_schedule(label, &tick_steps)
    }

    fn capture_schedule(
        &mut self,
        label: &str,
        tick_steps: &[u64],
    ) -> Result<Vec<oxide_sim::Event>> {
        let dir = self.output.join(label);
        std::fs::create_dir_all(&dir)?;
        let mut paths = Vec::with_capacity(tick_steps.len());
        let mut records = Vec::with_capacity(tick_steps.len());
        let mut seen_events = Vec::new();
        for (frame, ticks) in tick_steps.iter().copied().enumerate() {
            let events = if ticks == 0 {
                Vec::new()
            } else {
                self.present(ticks)?
            };
            let state = self.state()?;
            let name = format!("frame-{frame:03}-tick-{:05}.png", state.tick);
            let path = dir.join(&name);
            match self.client.call(Request::Screenshot {
                path: Some(path.to_string_lossy().into_owned()),
            })? {
                Reply::Screenshot(_) => {}
                other => bail!("screenshot returned {other:?}"),
            }
            records.push(json!({
                "frame": frame,
                "file": name,
                "tick": state.tick,
                "ticks_since_previous": ticks,
                "events": events,
            }));
            seen_events.extend(events);
            paths.push(path);
        }
        write_sheet(&paths, &dir.join("sheet.png"))?;
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec_pretty(&json!({
                "stage": label,
                "frames": tick_steps.len(),
                "tick_steps": tick_steps,
                "captures": records,
            }))?,
        )?;
        eprintln!("captured {label} -> {}", dir.display());
        Ok(seen_events)
    }
}

impl Drop for NativeCapture {
    fn drop(&mut self) {
        drop(self.shell.take());
        if let Err(error) = std::fs::remove_dir_all(&self.home)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "warning: could not remove animation capture HOME {}: {error}",
                self.home.display()
            );
        }
    }
}

fn combat_capture_schedule(cooldown: u32) -> Vec<u64> {
    const REPORT_TICKS: u32 = 8;
    const RELOAD_SAMPLES: u32 = 8;
    let immediate = cooldown.min(REPORT_TICKS);
    let mut steps = Vec::with_capacity((1 + immediate + RELOAD_SAMPLES) as usize);
    steps.push(0);
    steps.extend(std::iter::repeat_n(1, immediate as usize));
    let remaining = cooldown.saturating_sub(immediate);
    if remaining == 0 {
        return steps;
    }
    let base = remaining / RELOAD_SAMPLES;
    let extra = remaining % RELOAD_SAMPLES;
    for sample in 0..RELOAD_SAMPLES {
        let ticks = base + u32::from(sample < extra);
        if ticks > 0 {
            steps.push(u64::from(ticks));
        }
    }
    steps
}

#[test]
#[ignore = "opens a real native window and writes visual review artifacts"]
fn captures_action_driven_animation_states_in_the_real_shell() -> Result<()> {
    let mut harness = NativeCapture::spawn()?;

    let overview = harness.load(overview_scenario())?;
    assert_eq!(overview.units.len(), ALL_UNIT_KINDS.len() + 2);
    harness.capture_stage("00-idle-overview", 1, 1)?;

    for kind in ALL_UNIT_KINDS {
        let movement = harness.load(movement_scenario(kind))?;
        let mover = unit_kind(&movement, 0, kind)?;
        let start = movement
            .units
            .iter()
            .find(|unit| unit.id == mover.0)
            .expect("selected mover exists")
            .pos;
        harness.command(
            0,
            Command::Move {
                units: vec![mover],
                goal: TilePos::new(21, 10),
                queue: false,
            },
        )?;
        harness.capture_stage(&format!("01-unit-movement/{}", kind.name()), 20, 1)?;
        let moved = harness.state()?;
        let finish = moved
            .units
            .iter()
            .find(|unit| unit.id == mover.0)
            .context("mover disappeared")?
            .pos;
        assert_ne!(start, finish, "{kind:?} movement stage did not move");
    }

    let overview = harness.load(overview_scenario())?;
    let harvester = unit_at(&overview, 0, [10, 10])?;
    harness.command(
        0,
        Command::Harvest {
            units: vec![harvester],
            node: TilePos::new(11, 10),
            queue: false,
        },
    )?;
    harness.capture_stage("02-harvester-work-and-capacity", 52, 2)?;
    assert!(
        harness
            .state()?
            .units
            .iter()
            .find(|unit| unit.id == harvester.0)
            .context("working Harvester disappeared")?
            .carrying
            >= UnitKind::Harvester
                .stats()
                .harvest
                .expect("Harvester harvest stats")
                .capacity,
        "harvest capture never reached a full cargo bay"
    );

    let overview = harness.load(overview_scenario())?;
    let builder = unit_at(&overview, 0, [6, 15])?;
    harness.command(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor: TilePos::new(7, 15),
            queue: false,
            defer: false,
        },
    )?;
    let events = harness.capture_stage("03-active-construction", 18, 20)?;
    assert!(events.iter().any(|event| matches!(
        event,
        oxide_sim::Event::BuildingCompleted {
            kind: BuildingKind::Turret,
            ..
        }
    )));

    let overview = harness.load(overview_scenario())?;
    let foundry = building(&overview, 0, BuildingKind::Foundry)?;
    harness.command(
        0,
        Command::Train {
            building: foundry,
            kind: UnitKind::Harvester,
        },
    )?;
    let events = harness.capture_stage("04-foundry-production", 12, 10)?;
    assert!(events.iter().any(|event| matches!(
        event,
        oxide_sim::Event::UnitTrained {
            kind: UnitKind::Harvester,
            ..
        }
    )));

    let overview = harness.load(overview_scenario())?;
    let fabricator = building(&overview, 0, BuildingKind::Fabricator)?;
    harness.command(
        0,
        Command::Train {
            building: fabricator,
            kind: UnitKind::Scuttler,
        },
    )?;
    let events = harness.capture_stage("05-fabricator-production", 12, 8)?;
    assert!(events.iter().any(|event| matches!(
        event,
        oxide_sim::Event::UnitTrained {
            kind: UnitKind::Scuttler,
            ..
        }
    )));

    harness.load(overview_scenario())?;
    harness.capture_stage("06-array-and-reclaimer-continuous", 20, 2)?;

    for kind in combat_kinds() {
        let view = harness.load(unit_duel_scenario(kind))?;
        let attacker = unit_kind(&view, 0, kind)?;
        let target = view
            .units
            .iter()
            .find(|unit| unit.player == 1)
            .map(|unit| UnitId(unit.id))
            .context("duel has no target")?;
        harness.command(1, Command::Surrender)?;
        harness.command(
            0,
            Command::Attack {
                units: vec![attacker],
                target: Target::Unit(target),
                queue: false,
            },
        )?;
        let events = harness.capture_schedule(
            &format!("07-unit-fire-reload/{}", kind.name()),
            &combat_capture_schedule(kind.stats().weapons[0].cooldown_ticks),
        )?;
        assert!(
            events.iter().any(|event| match event {
                oxide_sim::Event::AttackHit {
                    attacker: source, ..
                } => *source == attacker,
                oxide_sim::Event::ShellLaunched {
                    shooter: Target::Unit(source),
                    ..
                } => *source == attacker,
                _ => false,
            }),
            "{kind:?} capture never observed its shot"
        );
    }

    let view = harness.load(sentinel_sidearm_scenario())?;
    let sentinel = unit_kind(&view, 0, UnitKind::Sentinel)?;
    let target = unit_kind(&view, 1, UnitKind::Talon)?;
    harness.command(1, Command::Surrender)?;
    harness.command(
        0,
        Command::Attack {
            units: vec![sentinel],
            target: Target::Unit(target),
            queue: false,
        },
    )?;
    let events = harness.capture_schedule(
        "07-unit-fire-reload/sentinel-aa-sidearm",
        &combat_capture_schedule(UnitKind::Sentinel.stats().weapons[1].cooldown_ticks),
    )?;
    assert!(events.iter().any(|event| matches!(
        event,
        oxide_sim::Event::AttackHit {
            attacker,
            weapon: 1,
            ..
        } if *attacker == sentinel
    )));

    for kind in [
        BuildingKind::Turret,
        BuildingKind::FlakTurret,
        BuildingKind::Bastion,
    ] {
        let view = harness.load(defense_duel_scenario(kind))?;
        let gun = building(&view, 0, kind)?;
        let target = view
            .units
            .iter()
            .find(|unit| unit.player == 1)
            .map(|unit| UnitId(unit.id))
            .context("defense duel has no target")?;
        harness.command(1, Command::Surrender)?;
        harness.command(
            0,
            Command::FocusFire {
                buildings: vec![gun],
                target: Target::Unit(target),
            },
        )?;
        let events = harness.capture_schedule(
            &format!("08-defense-fire-reload/{}", kind.name().replace(' ', "-")),
            &combat_capture_schedule(kind.base_stats().weapons[0].cooldown_ticks),
        )?;
        assert!(
            events.iter().any(|event| match event {
                oxide_sim::Event::TurretFired { turret, .. } => *turret == gun,
                oxide_sim::Event::ShellLaunched {
                    shooter: Target::Building(source),
                    ..
                } => *source == gun,
                _ => false,
            }),
            "{kind:?} capture never observed its shot"
        );
    }

    let repair = harness.load(building_repair_scenario())?;
    let attacker = unit_kind(&repair, 1, UnitKind::Sentinel)?;
    let patient = building(&repair, 0, BuildingKind::Array)?;
    harness.present(1)?;
    harness.command(
        1,
        Command::Attack {
            units: vec![attacker],
            target: Target::Building(patient),
            queue: false,
        },
    )?;
    let events = harness.present(1)?;
    assert!(events.iter().any(|event| matches!(
        event,
        oxide_sim::Event::AttackHit {
            attacker: source,
            target: Target::Building(target),
            ..
        } if *source == attacker && *target == patient
    )));
    let repair = harness.state()?;
    let damaged_hp = repair
        .buildings
        .iter()
        .find(|building| building.id == patient.0)
        .context("damaged building disappeared")?
        .hp;
    let executioner = unit_kind(&repair, 0, UnitKind::Lancer)?;
    harness.command(
        0,
        Command::Attack {
            units: vec![executioner],
            target: Target::Unit(attacker),
            queue: false,
        },
    )?;
    harness.present(1)?;
    let repair = harness.state()?;
    assert!(
        repair.units.iter().all(|unit| unit.id != attacker.0),
        "the damage source must be gone before repair capture"
    );
    harness.clear_presentation()?;
    let welder = unit_kind(&repair, 0, UnitKind::Harvester)?;
    harness.command(
        0,
        Command::Repair {
            units: vec![welder],
            building: patient,
            queue: false,
        },
    )?;
    harness.capture_stage("09-field-repair", 40, 2)?;
    assert!(
        harness
            .state()?
            .buildings
            .iter()
            .find(|building| building.id == patient.0)
            .context("repaired building disappeared")?
            .hp
            > damaged_hp,
        "field-repair capture never accepted a weld"
    );

    let repair_bay = harness.load(repair_bay_scenario())?;
    let attacker = unit_kind(&repair_bay, 1, UnitKind::Sentinel)?;
    let patient = unit_kind(&repair_bay, 0, UnitKind::Sentinel)?;
    harness.command(
        1,
        Command::Attack {
            units: vec![attacker],
            target: Target::Unit(patient),
            queue: false,
        },
    )?;
    let first = harness.present(1)?;
    let hit = first.iter().any(|event| {
        matches!(
            event,
            oxide_sim::Event::AttackHit { attacker: source, target: Target::Unit(target), .. }
                if *source == attacker && *target == patient
        )
    });
    if !hit {
        harness.command(
            1,
            Command::Attack {
                units: vec![attacker],
                target: Target::Unit(patient),
                queue: false,
            },
        )?;
        let second = harness.present(1)?;
        assert!(second.iter().any(|event| matches!(
            event,
            oxide_sim::Event::AttackHit { attacker: source, target: Target::Unit(target), .. }
                if *source == attacker && *target == patient
        )));
    }
    let repair_bay = harness.state()?;
    let damaged_hp = repair_bay
        .units
        .iter()
        .find(|unit| unit.id == patient.0)
        .context("Repair Bay patient disappeared")?
        .hp;
    let executioner = unit_kind(&repair_bay, 0, UnitKind::Lancer)?;
    harness.command(
        0,
        Command::Attack {
            units: vec![executioner],
            target: Target::Unit(attacker),
            queue: false,
        },
    )?;
    harness.present(1)?;
    let repair_bay = harness.state()?;
    assert!(
        repair_bay.units.iter().all(|unit| unit.id != attacker.0),
        "the damage source must be gone before Repair Bay capture"
    );
    harness.clear_presentation()?;
    let events = harness.capture_stage("10-repair-bay-pulses", 24, 1)?;
    assert!(events.iter().any(|event| matches!(
        event,
        oxide_sim::Event::UnitRepaired {
            unit,
            source: oxide_sim::UnitRepairSource::RepairBay { .. },
            ..
        } if *unit == patient
    )));
    assert!(
        harness
            .state()?
            .units
            .iter()
            .find(|unit| unit.id == patient.0)
            .context("Repair Bay patient disappeared")?
            .hp
            > damaged_hp,
        "Repair Bay capture never increased patient hp"
    );

    eprintln!(
        "native animation review written to {}",
        harness.output.display()
    );
    Ok(())
}

#[test]
fn generated_animation_capture_scenarios_are_valid() -> Result<()> {
    let mut scenarios = vec![
        overview_scenario(),
        building_repair_scenario(),
        repair_bay_scenario(),
    ];
    scenarios.extend(ALL_UNIT_KINDS.map(movement_scenario));
    scenarios.extend(combat_kinds().map(unit_duel_scenario));
    scenarios.push(sentinel_sidearm_scenario());
    scenarios.extend(
        [
            BuildingKind::Turret,
            BuildingKind::FlakTurret,
            BuildingKind::Bastion,
        ]
        .map(defense_duel_scenario),
    );
    for value in scenarios {
        let parsed = oxide_sim::Scenario::from_json(&serde_json::to_string(&value)?)?;
        parsed.build()?;
    }
    Ok(())
}

#[test]
fn combat_capture_keeps_report_ticks_and_spans_one_reload() {
    let steps = combat_capture_schedule(100);
    assert_eq!(steps[0], 0);
    assert!(steps[1..=8].iter().all(|step| *step == 1));
    assert_eq!(steps.iter().sum::<u64>(), 100);

    let short = combat_capture_schedule(4);
    assert_eq!(short, vec![0, 1, 1, 1, 1]);
}

fn expect_ok(reply: Reply) -> Result<()> {
    match reply {
        Reply::Ok => Ok(()),
        other => bail!("expected ok reply, got {other:?}"),
    }
}

fn unit_at(view: &StateView, player: u8, tile: [i32; 2]) -> Result<UnitId> {
    view.units
        .iter()
        .find(|unit| unit.player == player && unit.tile == tile)
        .map(|unit| UnitId(unit.id))
        .with_context(|| format!("no player-{player} unit at {tile:?}"))
}

fn unit_kind(view: &StateView, player: u8, kind: UnitKind) -> Result<UnitId> {
    view.units
        .iter()
        .find(|unit| unit.player == player && unit.kind == kind)
        .map(|unit| UnitId(unit.id))
        .with_context(|| format!("no player-{player} {kind:?}"))
}

fn building(view: &StateView, player: u8, kind: BuildingKind) -> Result<BuildingId> {
    view.buildings
        .iter()
        .find(|building| building.player == player && building.kind == kind)
        .map(|building| BuildingId(building.id))
        .with_context(|| format!("no player-{player} {kind:?}"))
}

fn scratch_home() -> Result<PathBuf> {
    loop {
        let id = NEXT_HOME.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "oxide-native-animation-{}-{id}",
            std::process::id()
        ));
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
}

fn write_config(home: &Path) -> Result<()> {
    let config = serde_json::to_vec_pretty(&json!({
        "version": 1,
        "bindings": { "bindings": [] },
        "volumes": { "master": 0.0, "effects": 0.0, "ui": 0.0, "music": 0.0 },
        "ui_scale": 1.0,
        "camera": { "pan_speed": 1.0, "edge_pan": false, "zoom_inverted": false },
        "window": [1280, 800],
        "reduced_motion": false,
        "colorblind": false
    }))?;
    for dir in [
        home.join("Library/Application Support/Oxide"),
        home.join(".config/oxide"),
        home.join("AppData/Oxide"),
    ] {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("config.json"), &config)?;
    }
    Ok(())
}

fn write_sheet(frames: &[PathBuf], path: &Path) -> Result<()> {
    let first_path = frames.first().context("contact sheet has no frames")?;
    let first = tiny_skia::Pixmap::decode_png(&std::fs::read(first_path)?)
        .context("decoding first animation frame")?;
    const SCALE: f32 = 0.25;
    let tile_w = (first.width() as f32 * SCALE).ceil() as u32;
    let tile_h = (first.height() as f32 * SCALE).ceil() as u32;
    let columns = (frames.len() as f32).sqrt().ceil() as u32;
    let rows = (frames.len() as u32).div_ceil(columns);
    let mut sheet = tiny_skia::Pixmap::new(columns * tile_w, rows * tile_h)
        .context("allocating animation contact sheet")?;
    for (index, frame_path) in frames.iter().enumerate() {
        let frame = tiny_skia::Pixmap::decode_png(&std::fs::read(frame_path)?)
            .with_context(|| format!("decoding {}", frame_path.display()))?;
        let col = index as u32 % columns;
        let row = index as u32 / columns;
        sheet.draw_pixmap(
            0,
            0,
            frame.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            tiny_skia::Transform::from_scale(SCALE, SCALE)
                .post_translate((col * tile_w) as f32, (row * tile_h) as f32),
            None,
        );
    }
    sheet.save_png(path)?;
    Ok(())
}

fn empty_map(scrap: &[TilePos]) -> Vec<String> {
    let mut map = vec![vec![b'.'; MAP_WIDTH as usize]; MAP_HEIGHT as usize];
    for x in 0..MAP_WIDTH {
        map[0][x as usize] = b'#';
        map[(MAP_HEIGHT - 1) as usize][x as usize] = b'#';
    }
    for y in 0..MAP_HEIGHT {
        map[y as usize][0] = b'#';
        map[y as usize][(MAP_WIDTH - 1) as usize] = b'#';
    }
    map[2][2] = b'1';
    map[2][28] = b'3';
    map[18][28] = b'2';
    for tile in scrap {
        map[tile.y as usize][tile.x as usize] = b's';
    }
    map.into_iter()
        .map(|row| String::from_utf8(row).expect("ASCII map"))
        .collect()
}

fn scenario(name: &str, scrap: &[TilePos], units: Vec<Value>, buildings: Vec<Value>) -> Value {
    json!({
        "name": name,
        "seed": 20260802,
        "players": [
            { "name": "Ferrous", "faction": "ferrous", "scrap": 5000, "bot": false },
            {
                "name": "Cupric Target",
                "faction": "cupric",
                "scrap": 0,
                "bot": true,
                "bot_config": { "level": "easy", "aggression": 0 }
            },
            {
                "name": "Cupric Observer",
                "faction": "cupric",
                "scrap": 0,
                "bot": true,
                "bot_config": { "level": "easy", "aggression": 0 }
            }
        ],
        "map": empty_map(scrap),
        "units": units,
        "buildings": buildings
    })
}

fn unit(player: u8, kind: UnitKind, x: i32, y: i32) -> Value {
    json!({ "player": player, "kind": kind, "x": x, "y": y })
}

fn structure(player: u8, kind: BuildingKind, x: i32, y: i32) -> Value {
    json!({ "player": player, "kind": kind, "x": x, "y": y })
}

fn overview_scenario() -> Value {
    let units = vec![
        unit(0, UnitKind::Harvester, 10, 10),
        unit(0, UnitKind::Harvester, 6, 13),
        unit(0, UnitKind::Harvester, 6, 15),
        unit(0, UnitKind::Sentinel, 13, 10),
        unit(0, UnitKind::Scuttler, 14, 10),
        unit(0, UnitKind::Lancer, 15, 10),
        unit(0, UnitKind::Bombard, 16, 10),
        unit(0, UnitKind::Flakhound, 17, 10),
        unit(0, UnitKind::Stinger, 18, 10),
        unit(0, UnitKind::Buzzard, 20, 10),
        unit(0, UnitKind::Darter, 21, 10),
        unit(0, UnitKind::Talon, 22, 10),
        unit(0, UnitKind::Wisp, 23, 10),
    ];
    let buildings = vec![
        structure(0, BuildingKind::Fabricator, 7, 5),
        structure(0, BuildingKind::Array, 10, 5),
        structure(0, BuildingKind::Reclaimer, 12, 5),
        structure(0, BuildingKind::RepairBay, 14, 5),
        structure(0, BuildingKind::Turret, 17, 5),
        structure(0, BuildingKind::FlakTurret, 19, 5),
        structure(0, BuildingKind::Bastion, 21, 5),
    ];
    scenario(
        "Native Animation Overview",
        &[TilePos::new(11, 10)],
        units,
        buildings,
    )
}

fn combat_kinds() -> [UnitKind; 10] {
    [
        UnitKind::Sentinel,
        UnitKind::Scuttler,
        UnitKind::Lancer,
        UnitKind::Bombard,
        UnitKind::Flakhound,
        UnitKind::Stinger,
        UnitKind::Buzzard,
        UnitKind::Darter,
        UnitKind::Talon,
        UnitKind::Wisp,
    ]
}

fn movement_scenario(kind: UnitKind) -> Value {
    scenario(
        &format!("Native {:?} Movement", kind),
        &[],
        vec![unit(0, kind, 15, 10)],
        Vec::new(),
    )
}

fn unit_duel_scenario(attacker: UnitKind) -> Value {
    let weapon = attacker.stats().weapons[0];
    let target = if weapon.targets.ground {
        UnitKind::Harvester
    } else if attacker.stats().domain == Domain::Ground {
        UnitKind::Talon
    } else {
        UnitKind::Darter
    };
    let distance = match attacker {
        UnitKind::Scuttler => 1,
        UnitKind::Sentinel | UnitKind::Darter => 2,
        UnitKind::Buzzard | UnitKind::Talon | UnitKind::Wisp => 3,
        UnitKind::Flakhound | UnitKind::Stinger => 4,
        UnitKind::Lancer => 5,
        UnitKind::Bombard => 9,
        UnitKind::Warden => 2,
        UnitKind::Shrike | UnitKind::Sylph => 3,
        UnitKind::Condor | UnitKind::Moth => 2,
        UnitKind::Breaker => 3,
        UnitKind::Avalanche => 8,
        UnitKind::Harvester
        | UnitKind::Tender
        | UnitKind::Excavator
        | UnitKind::Kestrel
        | UnitKind::Gnat
        | UnitKind::Skyhook
        | UnitKind::Sapper => unreachable!("the combat roster excludes unarmed machines"),
    };
    let units = vec![
        unit(0, attacker, 15, 10),
        unit(1, target, 15 + distance, 10),
    ];
    let buildings = if attacker == UnitKind::Bombard {
        vec![structure(0, BuildingKind::Array, 21, 12)]
    } else {
        Vec::new()
    };
    scenario(
        &format!("Native {:?} Fire", attacker),
        &[],
        units,
        buildings,
    )
}

fn sentinel_sidearm_scenario() -> Value {
    scenario(
        "Native Sentinel AA Sidearm",
        &[],
        vec![
            unit(0, UnitKind::Sentinel, 15, 10),
            unit(1, UnitKind::Talon, 17, 10),
        ],
        Vec::new(),
    )
}

fn defense_duel_scenario(defense: BuildingKind) -> Value {
    let target = if defense == BuildingKind::FlakTurret {
        UnitKind::Talon
    } else {
        UnitKind::Harvester
    };
    let target_x = if defense == BuildingKind::Bastion {
        23
    } else {
        19
    };
    let mut buildings = vec![structure(0, defense, 14, 10)];
    if defense == BuildingKind::Bastion {
        buildings.push(structure(0, BuildingKind::Array, 21, 12));
    }
    scenario(
        &format!("Native {:?} Fire", defense),
        &[],
        vec![unit(1, target, target_x, 10)],
        buildings,
    )
}

fn building_repair_scenario() -> Value {
    scenario(
        "Native Field Repair",
        &[],
        vec![
            unit(0, UnitKind::Harvester, 12, 10),
            unit(0, UnitKind::Lancer, 13, 8),
            unit(1, UnitKind::Sentinel, 18, 10),
        ],
        vec![structure(0, BuildingKind::Array, 16, 10)],
    )
}

fn repair_bay_scenario() -> Value {
    scenario(
        "Native Repair Bay",
        &[],
        vec![
            unit(0, UnitKind::Sentinel, 18, 11),
            unit(0, UnitKind::Lancer, 15, 9),
            unit(1, UnitKind::Sentinel, 20, 11),
        ],
        vec![structure(0, BuildingKind::RepairBay, 15, 10)],
    )
}
