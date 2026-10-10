//! Driving vocabulary for the automation-mode shell, shared by the
//! screenshot suite and the menu UX battery: spawn a real window that
//! ignores hardware input, read the shell's own UI report, and walk
//! menus by row label — never by index, because Home's rows shift with
//! resumable state on the host machine.

use crate::client::Client;
use anyhow::{Context, Result, bail};
use oxide_protocol::{Key, RawEvent, Request, UiView};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// A shell child killed on drop, so a failing walk never strands a
/// window holding the port.
pub struct ShellGuard(std::process::Child);

impl ShellGuard {
    pub(crate) fn new(child: std::process::Child) -> Self {
        Self(child)
    }
}

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
    /// Override HOME and the writable working directory for the child,
    /// isolating config, autosaves, replay discovery, and default output
    /// from the host user's real state.
    pub home: Option<PathBuf>,
}

/// Builds the shell with the caller's normal Rust environment and returns
/// Cargo's exact executable path. The separation matters: an isolated HOME
/// belongs on the game process, not the rustup shim that launches Cargo.
pub(crate) fn build_shell_executable() -> Result<PathBuf> {
    build_shell_executable_for(false)
}

/// Builds either the ordinary development shell or the optimized shell used
/// for native frame profiling. Callers choose explicitly because debug-build
/// timing is not representative of the shipped executable.
pub(crate) fn build_shell_executable_for(release: bool) -> Result<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut command = std::process::Command::new("cargo");
    command.args([
        "build",
        "-p",
        "oxide-shell",
        "--locked",
        "--message-format=json-render-diagnostics",
    ]);
    if release {
        command.arg("--release");
    }
    let output = command
        .current_dir(&root)
        .output()
        .context("building oxide-shell via cargo")?;
    if !output.status.success() {
        let rendered = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter_map(|message| message["message"]["rendered"].as_str().map(str::to_owned))
            .collect::<String>();
        bail!(
            "building oxide-shell failed:\n{rendered}{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    shell_executable_from_cargo_output(&output.stdout)
        .context("cargo did not report the Oxide executable")
}

fn shell_executable_from_cargo_output(stdout: &[u8]) -> Option<PathBuf> {
    for line in String::from_utf8_lossy(stdout).lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if message["reason"] == "compiler-artifact"
            && message["target"]["name"] == "Oxide"
            && let Some(executable) = message["executable"].as_str()
        {
            return Some(PathBuf::from(executable));
        }
    }
    None
}

pub(crate) fn isolate_home(command: &mut std::process::Command, home: &std::path::Path) {
    command.env("HOME", home);
    command.env("XDG_CONFIG_HOME", home.join(".config"));
    command.env("XDG_DATA_HOME", home.join(".local/share"));
    command.env("APPDATA", home.join("AppData"));
}

/// Builds and spawns an automation-mode shell, then connects while the
/// window boots. The window is pinned to 1280x800 because both suites
/// depend on stable geometry and a persisted config may carry another size.
pub fn spawn_shell(opts: &SpawnOptions) -> Result<(ShellGuard, Client)> {
    // Read-only resources stay rooted at the workspace even when the
    // writable working directory is isolated below.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let executable = build_shell_executable()?;
    let mut command = std::process::Command::new(executable);
    command
        .args([
            "--debug-server",
            "--automation",
            "--window",
            "1280x800",
            "--port",
            &opts.port.to_string(),
        ])
        .env("OXIDE_RESOURCE_ROOT", &root);
    if opts.paused {
        command.arg("--paused");
    }
    if let Some(home) = &opts.home {
        // HOME alone is not hermetic: Windows resolves config and
        // autosaves through APPDATA, and Linux prefers XDG_CONFIG_HOME
        // and XDG_DATA_HOME over HOME when they are set. Point every
        // platform's root and all relative writable paths into the
        // scratch tree.
        isolate_home(&mut command, home);
        command.current_dir(home);
    } else {
        command.current_dir(root);
    }
    let child = command.spawn().context("spawning built Oxide shell")?;
    let guard = ShellGuard::new(child);
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
    client.ui()
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

/// A walk of the cursor to a labeled row, steered by where each key
/// press lands rather than by index arithmetic: menus wrap, skip section
/// headings, and grids move by rows, so only the shell knows where a press
/// goes. It presses Down until the cursor reaches the row or comes back to
/// a row it already visited, then Right, which reaches every cell of a
/// grid in reading order.
#[derive(Debug)]
struct Walk {
    target: usize,
    key: Key,
    seen: Vec<usize>,
    presses: usize,
    limit: usize,
}

/// What a walk does next.
#[derive(Debug, PartialEq, Eq)]
enum Stride {
    Press(Key),
    Arrived,
    Stuck,
}

impl Walk {
    /// A walk to the row whose label equals `needle`, or else contains it
    /// (case-insensitive).
    fn to_label(view: &UiView, needle: &str) -> Result<Self> {
        let lower = needle.to_lowercase();
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
        Ok(Self {
            target,
            key: Key::Down,
            seen: Vec::new(),
            presses: 0,
            limit: 2 * view.items.len() + 2,
        })
    }

    /// The next key, given the row the cursor rests on.
    fn next(&mut self, selected: usize) -> Stride {
        if selected == self.target {
            return Stride::Arrived;
        }
        if self.seen.contains(&selected) {
            if self.key == Key::Right {
                return Stride::Stuck;
            }
            self.key = Key::Right;
            self.seen.clear();
        }
        if self.presses >= self.limit {
            return Stride::Stuck;
        }
        self.seen.push(selected);
        self.presses += 1;
        Stride::Press(self.key)
    }
}

/// Selects the row whose label contains `needle` (case-insensitive)
/// with keyboard navigation, then activates it with Enter.
pub fn activate_labeled(client: &mut Client, needle: &str) -> Result<()> {
    let view = ui(client)?;
    let mut walk = Walk::to_label(&view, needle)?;
    let mut selected = view.selected.unwrap_or(0);
    loop {
        match walk.next(selected) {
            Stride::Press(key) => {
                press_key(client, key)?;
                selected = ui(client)?.selected.unwrap_or(0);
            }
            Stride::Arrived => return press_key(client, Key::Enter),
            Stride::Stuck => bail!("the cursor never reached '{needle}' in {:?}", view.items),
        }
    }
}

/// Fails loudly when the shell is not on the expected screen.
pub fn assert_mode(client: &mut Client, expected: &str, at: &str) -> Result<()> {
    let mode = ui(client)?.mode;
    if mode != expected {
        bail!("after {at}: expected mode '{expected}', shell reports '{mode}'");
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum ModeWait {
    Arrived,
    Transitional,
    Unexpected,
}

fn classify_mode(mode: &str, expected: &str, transitional: &[&str]) -> ModeWait {
    if mode == expected {
        ModeWait::Arrived
    } else if transitional.contains(&mode) {
        ModeWait::Transitional
    } else {
        ModeWait::Unexpected
    }
}

/// Like [`assert_mode`], but rides out the named `transitional` screens
/// a step legitimately passes through while background work finishes,
/// such as the saving screen that holds a leave until its autosave
/// lands. Any other screen fails at once; a transitional screen fails
/// only if it outlasts a bounded wait.
pub fn wait_for_mode(
    client: &mut Client,
    expected: &str,
    transitional: &[&str],
    at: &str,
) -> Result<()> {
    const TIMEOUT: Duration = Duration::from_secs(10);
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let mode = ui(client)?.mode;
        match classify_mode(&mode, expected, transitional) {
            ModeWait::Arrived => return Ok(()),
            ModeWait::Unexpected => {
                bail!("after {at}: expected mode '{expected}', shell reports '{mode}'")
            }
            ModeWait::Transitional if Instant::now() >= deadline => bail!(
                "after {at}: expected mode '{expected}', shell still reports '{mode}' after {}s",
                TIMEOUT.as_secs()
            ),
            ModeWait::Transitional => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

#[cfg(test)]
mod tests;
