//! Menu interaction regressions, driven through the automation harness
//! against a real spawned shell. Written failing against the 0.8 widget
//! (hover drove selection, activation on press), turned green by the
//! 0.9 rebuild. `#[ignore]`d because they spawn real windows — CI has
//! no display and the workspace suite must stay headless. The phase-end
//! battery runs them explicitly:
//!
//! ```sh
//! cargo test -p oxide-driver --test menu_ux -- --ignored --test-threads 1
//! ```

use anyhow::{Context, Result, bail};
use oxide_driver::client::Client;
use oxide_protocol::{Key, MouseButton, RawEvent, Reply, Request, UiView};

/// A shell child killed on drop, so a failing assert never strands a
/// window holding the port.
struct ShellGuard(std::process::Child);

impl Drop for ShellGuard {
    fn drop(&mut self) {
        self.0.kill().ok();
        self.0.wait().ok();
    }
}

/// Spawns an automation-mode shell on `port` and connects, retrying
/// while cargo builds and the window boots.
/// Spawns, connects, and walks Home -> Play so tests operate on the
/// scenario list (the widget with enough rows to scroll).
fn spawn_at_map_list(port: u16) -> Result<(ShellGuard, Client)> {
    let (guard, mut client) = spawn_shell(port)?;
    press_key(&mut client, Key::Enter)?;
    Ok((guard, client))
}

fn spawn_shell(port: u16) -> Result<(ShellGuard, Client)> {
    // Tests run with the crate dir as cwd, but the shell loads assets and
    // scenarios relative to the workspace root — spawn it from there.
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let child = std::process::Command::new("cargo")
        .args([
            "run",
            "-p",
            "oxide-shell",
            "--",
            "--debug-server",
            "--automation",
            "--port",
            &port.to_string(),
        ])
        .current_dir(root)
        .spawn()
        .context("spawning oxide-shell via cargo")?;
    let guard = ShellGuard(child);
    let addr = format!("127.0.0.1:{port}");
    for _ in 0..120 {
        if let Ok(client) = Client::connect(&addr) {
            return Ok((guard, client));
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    bail!("shell never came up on {addr}");
}

fn ui(client: &mut Client) -> Result<UiView> {
    match client.call(Request::QueryUi)? {
        Reply::Ui(view) => Ok(view),
        other => bail!("expected a ui reply, got {other:?}"),
    }
}

fn hover(client: &mut Client, x: f32, y: f32) -> Result<()> {
    client.call(Request::InjectEvent {
        event: RawEvent::MouseMove { x, y },
    })?;
    Ok(())
}

/// Sweeps the window's vertical axis and returns every y where the
/// hover highlight changed — one entry per row boundary crossed, in
/// top-to-bottom order. Resolution-independent row discovery.
fn find_rows(client: &mut Client) -> Result<Vec<(f32, usize)>> {
    // Row discovery watches the hover highlight — the pointer's only
    // effect on a healthy menu.
    let mut prev = ui(client)?.hover;
    let mut hits: Vec<(f32, usize)> = Vec::new();
    for step in 0..40 {
        let y = 200.0 + step as f32 * 15.0;
        hover(client, 640.0, y)?;
        let hovered = ui(client)?.hover;
        if hovered != prev
            && let Some(h) = hovered
        {
            hits.push((y, h));
        }
        prev = hovered;
    }
    if hits.len() < 3 {
        bail!("could not locate menu rows by sweeping ({hits:?})");
    }
    Ok(hits)
}

fn press_key(client: &mut Client, key: Key) -> Result<()> {
    client.call(Request::InjectEvent {
        event: RawEvent::KeyDown { key },
    })?;
    client.call(Request::InjectEvent {
        event: RawEvent::KeyUp { key },
    })?;
    Ok(())
}

#[test]
#[ignore = "spawns a real window; run explicitly in the phase battery"]
fn a_stationary_pointer_never_changes_the_row_beneath_it() -> Result<()> {
    let (_guard, mut client) = spawn_at_map_list(4141)?;
    let rows = find_rows(&mut client)?;
    let top_row_y = rows[0].0;

    // Push the keyboard cursor deep so the window scrolls away from the
    // top, then park the pointer on the topmost visible row and wiggle
    // it by one pixel — a real cursor at rest. Nothing about the row
    // under the pointer may change: not the hover, not the selection,
    // not the window. The 0.8 widget failed all three.
    for _ in 0..6 {
        press_key(&mut client, Key::Down)?;
    }
    hover(&mut client, 640.0, top_row_y)?;
    let settled = ui(&mut client)?;
    for i in 0..6 {
        hover(&mut client, 640.0, top_row_y + (i % 2) as f32)?;
        let now = ui(&mut client)?;
        assert_eq!(now.hover, settled.hover, "the hover crawled");
        assert_eq!(now.selected, settled.selected, "the selection crawled");
        assert_eq!(
            now.visible_range, settled.visible_range,
            "the window crawled beneath a stationary pointer"
        );
    }
    Ok(())
}

#[test]
#[ignore = "spawns a real window; run explicitly in the phase battery"]
fn menu_rows_activate_on_release_not_on_press() -> Result<()> {
    let (_guard, mut client) = spawn_at_map_list(4142)?;
    // A high row: always a scenario, never Quit — pressing Quit would
    // close the shell before any assert could speak.
    let row_y = find_rows(&mut client)?[1].0;
    let before = ui(&mut client)?.mode;

    // Press without releasing: nothing may activate yet — a press is a
    // commitment only when it releases inside the same control.
    client.call(Request::InjectEvent {
        event: RawEvent::MouseDown {
            button: MouseButton::Left,
            x: 640.0,
            y: row_y,
        },
    })?;
    let held = ui(&mut client)?.mode;
    assert_eq!(held, before, "a menu row activated on mouse-down");

    // Releasing far outside the row cancels instead of activating.
    client.call(Request::InjectEvent {
        event: RawEvent::MouseMove { x: 20.0, y: 20.0 },
    })?;
    client.call(Request::InjectEvent {
        event: RawEvent::MouseUp {
            button: MouseButton::Left,
            x: 20.0,
            y: 20.0,
        },
    })?;
    let released = ui(&mut client)?.mode;
    assert_eq!(released, before, "a drag-away release still activated");

    // And the honest path works: press and release inside the same row
    // advances to the difficulty screen.
    client.call(Request::InjectEvent {
        event: RawEvent::MouseMove { x: 640.0, y: row_y },
    })?;
    client.call(Request::InjectEvent {
        event: RawEvent::MouseDown {
            button: MouseButton::Left,
            x: 640.0,
            y: row_y,
        },
    })?;
    client.call(Request::InjectEvent {
        event: RawEvent::MouseUp {
            button: MouseButton::Left,
            x: 640.0,
            y: row_y,
        },
    })?;
    assert_eq!(
        ui(&mut client)?.mode,
        "difficulty_menu",
        "release inside the row activates"
    );
    press_key(&mut client, Key::Escape)?;
    Ok(())
}
