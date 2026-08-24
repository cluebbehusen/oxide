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
            .collect::<Vec<_>>()
            .join("");
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
/// window boots. The window is pinned to 1280x800: the
/// persisted config carries whatever size the user last dragged, and
/// both suites depend on stable geometry.
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
        // / XDG_DATA_HOME over HOME when they are set — a host with
        // those exported would leak its real settings into (or worse,
        // take rebinds from) a supposedly throwaway shell. Point every
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

fn labeled_activation_keys(view: &UiView, needle: &str) -> Result<Vec<Key>> {
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
    let selected = view.selected.unwrap_or(0);
    let (key, steps) = if target >= selected {
        (Key::Down, target - selected)
    } else {
        (Key::Up, selected - target)
    };
    let mut keys = vec![key; steps];
    keys.push(Key::Enter);
    Ok(keys)
}

/// Selects the row whose label contains `needle` (case-insensitive)
/// with keyboard navigation, then activates it with Enter.
pub fn activate_labeled(client: &mut Client, needle: &str) -> Result<()> {
    let view = ui(client)?;
    for key in labeled_activation_keys(&view, needle)? {
        press_key(client, key)?;
    }
    Ok(())
}

/// Fails loudly when the shell is not on the expected screen.
pub fn assert_mode(client: &mut Client, expected: &str, at: &str) -> Result<()> {
    let mode = ui(client)?.mode;
    if mode != expected {
        bail!("after {at}: expected mode '{expected}', shell reports '{mode}'");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn menu(items: &[&str], selected: Option<usize>) -> UiView {
        UiView {
            mode: "home".to_string(),
            title: None,
            selected,
            items: items.iter().map(|item| (*item).to_string()).collect(),
            visible_range: None,
            hover: None,
            chrome: None,
        }
    }

    #[test]
    fn cargo_artifact_selection_ignores_noise_and_other_targets() {
        let output = br#"
not json
{"reason":"compiler-artifact","target":{"name":"oxide-sim"},"executable":"/tmp/wrong"}
{"reason":"compiler-artifact","target":{"name":"Oxide"},"executable":null}
{"reason":"build-finished","success":true}
{"reason":"compiler-artifact","target":{"name":"Oxide"},"executable":"/tmp/Oxide"}
"#;
        assert_eq!(
            shell_executable_from_cargo_output(output),
            Some(PathBuf::from("/tmp/Oxide"))
        );
        assert_eq!(shell_executable_from_cargo_output(b"not json\n"), None);
    }

    #[test]
    fn isolated_shells_override_every_platform_writable_root() {
        let home = PathBuf::from("/tmp/oxide-isolated-home");
        let mut command = std::process::Command::new("Oxide");
        command.env("HOME", "/host/home");
        command.env("XDG_CONFIG_HOME", "/host/config");
        command.env("XDG_DATA_HOME", "/host/data");
        command.env("APPDATA", "C:/host/data");

        isolate_home(&mut command, &home);

        let value = |key: &str| {
            command
                .get_envs()
                .find(|(name, _)| *name == OsStr::new(key))
                .and_then(|(_, value)| value)
                .map(std::ffi::OsStr::to_owned)
        };
        assert_eq!(value("HOME"), Some(home.clone().into_os_string()));
        assert_eq!(
            value("XDG_CONFIG_HOME"),
            Some(home.join(".config").into_os_string())
        );
        assert_eq!(
            value("XDG_DATA_HOME"),
            Some(home.join(".local/share").into_os_string())
        );
        assert_eq!(
            value("APPDATA"),
            Some(home.join("AppData").into_os_string())
        );
    }

    #[test]
    fn labeled_activation_prefers_an_exact_row_over_an_earlier_substring() {
        let view = menu(&["REPLAYS", "PLAY", "SETTINGS"], Some(0));
        assert_eq!(
            labeled_activation_keys(&view, "play").unwrap(),
            vec![Key::Down, Key::Enter]
        );
    }

    #[test]
    fn labeled_activation_navigates_in_both_directions_and_defaults_to_the_first_row() {
        let view = menu(&["PLAY", "SETTINGS", "QUIT"], Some(2));
        assert_eq!(
            labeled_activation_keys(&view, "play").unwrap(),
            vec![Key::Up, Key::Up, Key::Enter]
        );

        let no_selection = menu(&["PLAY", "SETTINGS", "QUIT"], None);
        assert_eq!(
            labeled_activation_keys(&no_selection, "settings").unwrap(),
            vec![Key::Down, Key::Enter]
        );
    }

    #[test]
    fn labeled_activation_reports_the_visible_rows_when_no_row_matches() {
        let view = menu(&["PLAY", "SETTINGS"], Some(0));
        let error = labeled_activation_keys(&view, "credits").unwrap_err();
        assert_eq!(
            error.to_string(),
            "no row containing 'credits' in [\"PLAY\", \"SETTINGS\"]"
        );
    }
}
