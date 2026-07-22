//! Menu interaction regressions, driven through the automation harness
//! against a real spawned shell. Both tests pin bugs the 0.9 screen
//! rework must fix and are `#[ignore]`d until Phase D lands the fix —
//! they need a window, so they run locally, not in CI:
//!
//! ```sh
//! cargo test -p oxide-driver --test menu_ux -- --ignored
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

/// Sweeps the window's vertical axis and returns every y where hovering
/// changed the menu selection — one entry per row boundary crossed, in
/// top-to-bottom order. Resolution-independent row discovery.
fn find_rows(client: &mut Client) -> Result<Vec<(f32, usize)>> {
    // Only a *change* in selection proves the cursor crossed onto a row
    // — the first swept y sits above the menu and reads back whatever
    // was already selected, which must not count as a hit.
    let mut prev = ui(client)?.selected;
    let mut hits: Vec<(f32, usize)> = Vec::new();
    for step in 0..40 {
        let y = 200.0 + step as f32 * 15.0;
        hover(client, 640.0, y)?;
        let selected = ui(client)?.selected;
        if selected != prev
            && let Some(s) = selected
        {
            hits.push((y, s));
        }
        prev = selected;
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
#[ignore = "pins the 0.8 hover/scroll feedback loop; un-ignore when Phase D separates scroll state from selection"]
fn a_stationary_pointer_never_changes_the_row_beneath_it() -> Result<()> {
    let (_guard, mut client) = spawn_shell(4141)?;
    let rows = find_rows(&mut client)?;
    let top_row_y = rows[0].0;

    // Park the selection deep by hovering the lowest discovered row so
    // the scroll window shifts away from the top, then park the pointer
    // on the topmost visible row and wiggle it by one pixel — a real
    // cursor at rest. The item under the pointer must not change: today
    // hover drives selection, the scroll window recenters on selection,
    // and the list crawls upward beneath the still cursor.
    let deep_row_y = rows[rows.len() - 1].0;
    hover(&mut client, 640.0, deep_row_y)?;
    hover(&mut client, 640.0, top_row_y)?;
    let settled = ui(&mut client)?.selected;
    for i in 0..6 {
        hover(&mut client, 640.0, top_row_y + (i % 2) as f32)?;
        let now = ui(&mut client)?.selected;
        assert_eq!(
            now, settled,
            "the list crawled beneath a stationary pointer"
        );
    }
    Ok(())
}

#[test]
#[ignore = "pins mouse-down menu activation; un-ignore when Phase D activates on release inside the same control"]
fn menu_rows_activate_on_release_not_on_press() -> Result<()> {
    let (_guard, mut client) = spawn_shell(4142)?;
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

    // Escape resets any state the probe left behind.
    press_key(&mut client, Key::Escape)?;
    Ok(())
}
