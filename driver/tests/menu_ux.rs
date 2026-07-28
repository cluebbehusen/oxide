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
//!
//! Every spawned shell gets its own scratch HOME and platform-specific
//! config/data roots. The guard removes that tree only after the child
//! has exited, so even autosave-producing walks cannot touch real data.

use anyhow::{Result, bail};
use oxide_driver::auto::{
    ShellGuard, SpawnOptions, activate_labeled, assert_mode, press_key, spawn_shell, ui,
};
use oxide_driver::client::Client;
use oxide_protocol::{Key, MouseButton, RawEvent, Request};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_HOME: AtomicU64 = AtomicU64::new(0);

/// Owns both the spawned shell and its isolated HOME. Dropping the
/// shell first matters: a final autosave must finish before the
/// scratch tree is removed.
struct MenuShellGuard {
    shell: Option<ShellGuard>,
    home: PathBuf,
}

impl Drop for MenuShellGuard {
    fn drop(&mut self) {
        drop(self.shell.take());
        if let Err(error) = std::fs::remove_dir_all(&self.home)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "warning: could not remove menu UX scratch HOME {}: {error}",
                self.home.display()
            );
        }
    }
}

fn scratch_home(port: u16) -> Result<PathBuf> {
    loop {
        let id = NEXT_HOME.fetch_add(1, Ordering::Relaxed);
        let home =
            std::env::temp_dir().join(format!("oxide-menu-ux-{}-{port}-{id}", std::process::id()));
        match std::fs::create_dir(&home) {
            Ok(()) => return Ok(home),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
}

/// Spawns an automation-mode shell (via the shared harness vocabulary)
/// and walks Home -> Play so tests operate on the scenario list (the
/// widget with enough rows to scroll).
fn spawn_at_map_list(port: u16) -> Result<(MenuShellGuard, Client)> {
    let (guard, mut client) = spawn(port)?;
    // By label, never by blind Enter: if a fixture later adds an
    // autosave, Home's first row becomes Continue.
    activate_labeled(&mut client, "play")?;
    Ok((guard, client))
}

fn spawn(port: u16) -> Result<(MenuShellGuard, Client)> {
    let home = scratch_home(port)?;
    let spawned = spawn_shell(&SpawnOptions {
        port,
        paused: false,
        home: Some(home.clone()),
    });
    match spawned {
        Ok((shell, client)) => Ok((
            MenuShellGuard {
                shell: Some(shell),
                home,
            },
            client,
        )),
        Err(error) => {
            std::fs::remove_dir_all(home).ok();
            Err(error)
        }
    }
}

fn hover(client: &mut Client, x: f32, y: f32) -> Result<()> {
    client.call(Request::InjectEvent {
        event: RawEvent::MouseMove { x, y },
    })?;
    Ok(())
}

/// The x the row tests probe and click at. The front door is a card
/// GRID since 0.11: the window's horizontal center (640 at the pinned
/// 1280x800) is a column gutter where no card hovers, so the probe
/// column sits inside the second card column instead.
const GRID_X: f32 = 500.0;

/// Sweeps the window's vertical axis and returns every y where the
/// hover highlight changed — one entry per row boundary crossed, in
/// top-to-bottom order. Resolution-independent row discovery. The
/// front door is a thumbnail grid since 0.11: two tall card rows fit
/// the default window where six menu rows once did, so the sweep
/// steps fine and two hits suffice.
fn find_rows(client: &mut Client) -> Result<Vec<(f32, usize)>> {
    // Row discovery watches the hover highlight — the pointer's only
    // effect on a healthy menu.
    let mut prev = ui(client)?.hover;
    let mut hits: Vec<(f32, usize)> = Vec::new();
    for step in 0..64 {
        let y = 160.0 + step as f32 * 10.0;
        hover(client, GRID_X, y)?;
        let hovered = ui(client)?.hover;
        if hovered != prev
            && let Some(h) = hovered
        {
            hits.push((y, h));
        }
        prev = hovered;
    }
    if hits.len() < 2 {
        bail!("could not locate menu rows by sweeping ({hits:?})");
    }
    Ok(hits)
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
    hover(&mut client, GRID_X, top_row_y)?;
    let settled = ui(&mut client)?;
    for i in 0..6 {
        hover(&mut client, GRID_X, top_row_y + (i % 2) as f32)?;
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
            x: GRID_X,
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

    // And the honest path works: a click inside the card selects it,
    // and a second click on the selected card advances to match setup.
    client.call(Request::InjectEvent {
        event: RawEvent::MouseMove {
            x: GRID_X,
            y: row_y,
        },
    })?;
    for _ in 0..2 {
        client.call(Request::InjectEvent {
            event: RawEvent::MouseDown {
                button: MouseButton::Left,
                x: GRID_X,
                y: row_y,
            },
        })?;
        client.call(Request::InjectEvent {
            event: RawEvent::MouseUp {
                button: MouseButton::Left,
                x: GRID_X,
                y: row_y,
            },
        })?;
    }
    assert_eq!(
        ui(&mut client)?.mode,
        "match_setup",
        "click selects, click again activates"
    );
    press_key(&mut client, Key::Escape)?;
    Ok(())
}

#[test]
#[ignore = "spawns a real window; run explicitly in the phase battery"]
fn every_screen_transition_answers_the_walk() -> Result<()> {
    // One pass over the whole screen graph, asserting the shell's own
    // mode report at each hop. Navigation is by row LABEL, never by
    // index: Home's rows shift with resumable state.
    let (_guard, mut client) = spawn(4143)?;
    assert_mode(&mut client, "home", "boot")?;

    activate_labeled(&mut client, "settings")?;
    assert_mode(&mut client, "settings", "Home > Settings")?;
    activate_labeled(&mut client, "controls")?;
    assert_mode(&mut client, "controls", "Settings > Controls")?;
    press_key(&mut client, Key::Escape)?;
    assert_mode(&mut client, "settings", "Controls > Esc")?;
    press_key(&mut client, Key::Escape)?;
    assert_mode(&mut client, "home", "Settings > Esc")?;

    activate_labeled(&mut client, "replays")?;
    assert_mode(&mut client, "replays", "Home > Replays")?;
    press_key(&mut client, Key::Escape)?;
    assert_mode(&mut client, "home", "Replays > Esc")?;

    activate_labeled(&mut client, "play")?;
    assert_mode(&mut client, "main_menu", "Home > Play")?;
    activate_labeled(&mut client, "skirmish")?;
    assert_mode(&mut client, "match_setup", "map picked")?;
    // Back walks to the grid without losing the draft.
    press_key(&mut client, Key::Escape)?;
    assert_mode(&mut client, "main_menu", "setup > Esc")?;
    activate_labeled(&mut client, "skirmish")?;
    assert_mode(&mut client, "match_setup", "map re-picked")?;
    // Start is preselected: Enter launches the classic matchup.
    press_key(&mut client, Key::Enter)?;
    assert_mode(&mut client, "playing", "Start launches the match")?;

    press_key(&mut client, Key::Escape)?;
    assert_mode(&mut client, "pause_menu", "Playing > Esc")?;
    activate_labeled(&mut client, "resume")?;
    assert_mode(&mut client, "playing", "pause > Resume")?;
    press_key(&mut client, Key::Escape)?;
    activate_labeled(&mut client, "restart")?;
    assert_mode(&mut client, "confirm_pause", "destructive choices confirm")?;
    // Cancel must sit preselected: bare Enter declines the destruction.
    press_key(&mut client, Key::Enter)?;
    assert_mode(&mut client, "pause_menu", "confirm > default is Cancel")?;
    activate_labeled(&mut client, "main menu")?;
    activate_labeled(&mut client, "main menu")?;
    assert_mode(&mut client, "home", "pause > Main Menu confirmed")?;
    Ok(())
}

#[test]
#[ignore = "spawns a real window; run explicitly in the phase battery"]
fn a_modifier_held_on_another_screen_still_captures_its_chord() -> Result<()> {
    // The 0.9 regression: Controls tracked modifier edges only inside
    // its own arm, so a Ctrl pressed in Settings read as unheld and a
    // rebind captured a bare key. Modifier truth is global now, and
    // this walks the exact failing path. The shared spawn helper keeps
    // the persisted rebind inside this test's scratch HOME.
    let (_guard, mut client) = spawn(4144)?;

    activate_labeled(&mut client, "settings")?;
    // Ctrl goes down HERE, on the Settings screen...
    client.call(Request::InjectEvent {
        event: RawEvent::KeyDown { key: Key::Ctrl },
    })?;
    activate_labeled(&mut client, "controls")?;
    assert_mode(&mut client, "controls", "Settings > Controls")?;
    // ...arm the first row and press the key while Ctrl is still held.
    press_key(&mut client, Key::Enter)?;
    press_key(&mut client, Key::K)?;
    client.call(Request::InjectEvent {
        event: RawEvent::KeyUp { key: Key::Ctrl },
    })?;
    let view = ui(&mut client)?;
    let bound = view
        .items
        .iter()
        .find(|item| item.contains("Ctrl+K"))
        .cloned();
    if bound.is_none() {
        bail!(
            "no row shows Ctrl+K after a held-modifier rebind; rows: {:?}",
            view.items
        );
    }
    Ok(())
}
