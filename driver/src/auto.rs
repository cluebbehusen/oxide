//! Driving vocabulary for the automation-mode shell, shared by the
//! screenshot suite and the menu UX battery: spawn a real window that
//! ignores hardware input, read the shell's own UI report, and walk
//! menus by row label — never by index, because Home's rows shift with
//! resumable state on the host machine.

use crate::client::Client;
use anyhow::{Context, Result, bail};
use oxide_protocol::{Key, RawEvent, Reply, Request, UiView};
use std::path::PathBuf;

/// A shell child killed on drop, so a failing walk never strands a
/// window holding the port.
pub struct ShellGuard(std::process::Child);

impl Drop for ShellGuard {
    fn drop(&mut self) {
        self.0.kill().ok();
        self.0.wait().ok();
    }
}

/// How to spawn the automation shell.
pub struct SpawnOptions {
    /// Debug-server port (each caller picks its own to avoid clashes).
    pub port: u16,
    /// Start with the sim clock paused (driven mode).
    pub paused: bool,
    /// Override HOME for the child, isolating persisted config and
    /// autosaves from the host user's real state.
    pub home: Option<PathBuf>,
}

/// Spawns an automation-mode shell and connects, retrying while cargo
/// builds and the window boots. The window is pinned to 1280x800: the
/// persisted config carries whatever size the user last dragged, and
/// both suites depend on stable geometry.
pub fn spawn_shell(opts: &SpawnOptions) -> Result<(ShellGuard, Client)> {
    // Callers run with varying cwds, but the shell loads assets and
    // scenarios relative to the workspace root — spawn it from there.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut command = std::process::Command::new("cargo");
    command
        .args([
            "run",
            "-p",
            "oxide-shell",
            "--",
            "--debug-server",
            "--automation",
            "--window",
            "1280x800",
            "--port",
            &opts.port.to_string(),
        ])
        .current_dir(root);
    if opts.paused {
        command.arg("--paused");
    }
    if let Some(home) = &opts.home {
        // HOME alone is not hermetic: Windows resolves config and
        // autosaves through APPDATA, and Linux prefers XDG_CONFIG_HOME
        // / XDG_DATA_HOME over HOME when they are set — a host with
        // those exported would leak its real settings into (or worse,
        // take rebinds from) a supposedly throwaway shell. Point every
        // platform's root into the scratch tree.
        command.env("HOME", home);
        command.env("XDG_CONFIG_HOME", home.join(".config"));
        command.env("XDG_DATA_HOME", home.join(".local/share"));
        command.env("APPDATA", home.join("AppData"));
    }
    let child = command.spawn().context("spawning oxide-shell via cargo")?;
    let guard = ShellGuard(child);
    let addr = format!("127.0.0.1:{}", opts.port);
    for _ in 0..240 {
        if let Ok(client) = Client::connect(&addr) {
            return Ok((guard, client));
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    bail!("shell never came up on {addr}");
}

/// The shell's own report of what screen it shows.
pub fn ui(client: &mut Client) -> Result<UiView> {
    match client.call(Request::QueryUi)? {
        Reply::Ui(view) => Ok(view),
        other => bail!("expected a ui reply, got {other:?}"),
    }
}

/// Injects one raw event into the real input funnel.
pub fn inject(client: &mut Client, event: RawEvent) -> Result<()> {
    client.call(Request::InjectEvent { event })?;
    Ok(())
}

/// A full key press: down then up.
pub fn press_key(client: &mut Client, key: Key) -> Result<()> {
    inject(client, RawEvent::KeyDown { key })?;
    inject(client, RawEvent::KeyUp { key })
}

/// Selects the row whose label contains `needle` (case-insensitive)
/// with keyboard navigation, then activates it with Enter.
pub fn activate_labeled(client: &mut Client, needle: &str) -> Result<()> {
    let view = ui(client)?;
    let lower = needle.to_lowercase();
    // Exact label first ('play' must never land on 'Replays'), then
    // substring for rows that carry decorations.
    let target = view
        .items
        .iter()
        .position(|item| item.to_lowercase() == lower)
        .or_else(|| {
            view.items
                .iter()
                .position(|item| item.to_lowercase().contains(&lower))
        })
        .with_context(|| format!("no row containing '{needle}' in {:?}", view.items))?;
    let selected = view.selected.unwrap_or(0);
    let (key, steps) = if target >= selected {
        (Key::Down, target - selected)
    } else {
        (Key::Up, selected - target)
    };
    for _ in 0..steps {
        press_key(client, key)?;
    }
    press_key(client, Key::Enter)
}

/// Fails loudly when the shell is not on the expected screen.
pub fn assert_mode(client: &mut Client, expected: &str, at: &str) -> Result<()> {
    let mode = ui(client)?.mode;
    if mode != expected {
        bail!("after {at}: expected mode '{expected}', shell reports '{mode}'");
    }
    Ok(())
}
