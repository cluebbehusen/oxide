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

use anyhow::{Result, bail};
use oxide_driver::auto::{
    ShellGuard, SpawnOptions, activate_labeled, assert_mode, press_key, spawn_shell, ui,
};
use oxide_driver::client::Client;
use oxide_protocol::{Key, MouseButton, RawEvent, Request};

/// Spawns an automation-mode shell (via the shared harness vocabulary)
/// and walks Home -> Play so tests operate on the scenario list (the
/// widget with enough rows to scroll).
fn spawn_at_map_list(port: u16) -> Result<(ShellGuard, Client)> {
    let (guard, mut client) = spawn(port)?;
    // By label, never by blind Enter: with autosaves on this machine,
    // Home's first row is Continue and Enter would resume a match.
    activate_labeled(&mut client, "play")?;
    Ok((guard, client))
}

fn spawn(port: u16) -> Result<(ShellGuard, Client)> {
    spawn_shell(&SpawnOptions {
        port,
        paused: false,
        home: None,
    })
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

#[test]
#[ignore = "spawns a real window; run explicitly in the phase battery"]
fn every_screen_transition_answers_the_walk() -> Result<()> {
    // One pass over the whole screen graph, asserting the shell's own
    // mode report at each hop. Navigation is by row LABEL, never by
    // index: Home's rows shift with resumable state, and the walk must
    // not depend on this machine's autosaves.
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
    assert_mode(&mut client, "difficulty_menu", "map picked")?;
    activate_labeled(&mut client, "medium")?;
    assert_mode(&mut client, "personality_menu", "difficulty picked")?;
    // Back walks the wizard without losing the draft.
    press_key(&mut client, Key::Escape)?;
    assert_mode(&mut client, "difficulty_menu", "personality > Esc")?;
    activate_labeled(&mut client, "medium")?;
    activate_labeled(&mut client, "surprise")?;
    assert_mode(&mut client, "faction_menu", "personality picked")?;
    activate_labeled(&mut client, "ferrous")?;
    assert_mode(&mut client, "playing", "faction picked starts the match")?;

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
    // this walks the exact failing path. The spawned shell gets a
    // throwaway HOME so the rebind persists into a temp config, never
    // this machine's real one.
    let tmp = std::env::temp_dir().join(format!("oxide-menu-ux-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;
    let (_guard, mut client) = spawn_shell(&SpawnOptions {
        port: 4144,
        paused: false,
        home: Some(tmp.clone()),
    })?;

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
    std::fs::remove_dir_all(&tmp).ok();
    Ok(())
}
