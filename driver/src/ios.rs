//! `cargo ios`: build the iPad app through the Xcode wrapper in `ios/`,
//! install it, and launch it on a paired iPad or an iPad simulator. With
//! several targets and no `--device`, it lists them and asks, remembering
//! the choice for next time.

use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Somewhere the app can run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// What the device calls itself.
    pub name: String,
    /// The identifier Xcode and the install tools accept.
    pub id: String,
    /// Real hardware or a simulator.
    pub kind: DeviceKind,
    /// A short description for the picker.
    pub detail: String,
}

/// How the app reaches a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    /// A paired iPad, over a cable or the local network.
    Physical,
    /// An iPad simulator on this Mac.
    Simulator {
        /// Whether it is running already.
        booted: bool,
    },
}

impl std::fmt::Display for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.name, self.detail)
    }
}

/// Paired physical iPads from `xcrun devicectl list devices --json-output`.
pub fn physical_ipads(devicectl: &Value) -> Vec<Device> {
    let devices = devicectl["result"]["devices"].as_array();
    devices
        .into_iter()
        .flatten()
        .filter(|device| {
            let hardware = &device["hardwareProperties"];
            hardware["reality"] == "physical"
                && hardware["deviceType"] == "iPad"
                && device["connectionProperties"]["pairingState"] == "paired"
        })
        .filter_map(|device| {
            let hardware = &device["hardwareProperties"];
            let id = hardware["udid"].as_str()?;
            let name = device["deviceProperties"]["name"].as_str().unwrap_or(id);
            let model = hardware["marketingName"].as_str().unwrap_or("iPad");
            let link = match device["connectionProperties"]["transportType"].as_str() {
                Some("localNetwork") => "Wi-Fi",
                Some("wired") => "USB",
                _ => "paired",
            };
            Some(Device {
                name: name.to_string(),
                id: id.to_string(),
                kind: DeviceKind::Physical,
                detail: format!("{model}, {link}"),
            })
        })
        .collect()
}

/// Available iPad simulators from `xcrun simctl list devices available --json`.
pub fn ipad_simulators(simctl: &Value) -> Vec<Device> {
    let Some(runtimes) = simctl["devices"].as_object() else {
        return Vec::new();
    };
    let mut simulators: Vec<Device> = runtimes
        .iter()
        .filter_map(|(runtime, devices)| Some((runtime_label(runtime)?, devices.as_array()?)))
        .flat_map(|(runtime, devices)| {
            devices.iter().filter_map(move |device| {
                let family = device["deviceTypeIdentifier"].as_str()?;
                if !family.contains(".iPad") || device["isAvailable"] == false {
                    return None;
                }
                let booted = device["state"] == "Booted";
                Some(Device {
                    name: device["name"].as_str()?.to_string(),
                    id: device["udid"].as_str()?.to_string(),
                    kind: DeviceKind::Simulator { booted },
                    detail: format!(
                        "simulator, {runtime}{}",
                        if booted { ", booted" } else { "" }
                    ),
                })
            })
        })
        .collect();
    simulators.sort_by(|a, b| {
        let booted = |d: &Device| matches!(d.kind, DeviceKind::Simulator { booted: true });
        booted(b)
            .cmp(&booted(a))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.detail.cmp(&b.detail))
    });
    simulators
}

/// "iOS 27.0" from "com.apple.CoreSimulator.SimRuntime.iOS-27-0"; `None`
/// for runtimes that cannot run an iPad app.
fn runtime_label(runtime: &str) -> Option<String> {
    let version = runtime.rsplit('.').next()?.strip_prefix("iOS-")?;
    Some(format!("iOS {}", version.replace('-', ".")))
}

/// The device a `--device` query names: an exact identifier, an exact
/// name (ignoring case), or the one device whose name contains it.
pub fn find<'a>(devices: &'a [Device], query: &str) -> Result<&'a Device> {
    if let Some(device) = devices.iter().find(|d| d.id.eq_ignore_ascii_case(query)) {
        return Ok(device);
    }
    // Two runtimes can each carry a simulator of the same model, so an
    // exact name is only as good as it is unique.
    let wanted = query.to_lowercase();
    let exact: Vec<&Device> = devices
        .iter()
        .filter(|d| d.name.to_lowercase() == wanted)
        .collect();
    let matches = if exact.is_empty() {
        devices
            .iter()
            .filter(|d| d.name.to_lowercase().contains(&wanted))
            .collect()
    } else {
        exact
    };
    match matches.as_slice() {
        [device] => Ok(device),
        [] => bail!("no device matches {query:?}\n{}", listing(devices)),
        _ => bail!(
            "{query:?} matches more than one device; pick one by id or number\n{}",
            listing(devices)
        ),
    }
}

/// The picker's default: the device chosen last time, else the first.
pub fn default_choice(devices: &[Device], remembered: Option<&str>) -> usize {
    remembered
        .and_then(|id| devices.iter().position(|d| d.id == id))
        .unwrap_or(0)
}

/// Reads one picker answer: blank takes the default, a number picks from
/// the list, anything else is a `--device` style query.
pub fn choose(devices: &[Device], answer: &str, default: usize) -> Result<usize> {
    let answer = answer.trim();
    if answer.is_empty() {
        return Ok(default);
    }
    if let Ok(number) = answer.parse::<usize>() {
        if (1..=devices.len()).contains(&number) {
            return Ok(number - 1);
        }
        bail!("pick a number from 1 to {}", devices.len());
    }
    let device = find(devices, answer)?;
    Ok(devices
        .iter()
        .position(|d| d == device)
        .expect("find returns one of these devices"))
}

/// The numbered device list the picker shows.
pub fn listing(devices: &[Device]) -> String {
    devices
        .iter()
        .enumerate()
        .map(|(i, device)| format!("  {}) {device}\n", i + 1))
        .collect()
}

/// Builds, installs, and launches the app. `device` skips the picker;
/// `list` prints the targets and stops.
pub fn run(device: Option<&str>, list: bool) -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("cargo ios needs macOS with Xcode");
    }
    let root = workspace_root();
    let devices = discover()?;
    if list {
        print!("{}", listing(&devices));
        return Ok(());
    }
    let remembered_path = root.join("target").join("ios-device");
    let target = match device {
        Some(query) => find(&devices, query)?.clone(),
        None => pick(
            &devices,
            std::fs::read_to_string(&remembered_path).ok().as_deref(),
        )?,
    };
    // Best effort: forgetting the last device only costs a keypress.
    let _ = std::fs::create_dir_all(root.join("target"));
    let _ = std::fs::write(&remembered_path, &target.id);

    println!("building for {target}");
    let app = build(&root, &target)?;
    let bundle = bundle_id(&app)?;
    match target.kind {
        DeviceKind::Physical => install_physical(&target, &app, &bundle),
        DeviceKind::Simulator { booted } => install_simulator(&target, booted, &app, &bundle),
    }
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Paired iPads first, then simulators.
fn discover() -> Result<Vec<Device>> {
    combine(list_physical(), list_simulators())
}

/// Joins both discoveries. A tool that fails is reported rather than read
/// as "no devices": its error prints as a warning when the other tool
/// found something to run on, and becomes the error when nothing did.
pub fn combine(
    physical: std::result::Result<Vec<Device>, String>,
    simulators: std::result::Result<Vec<Device>, String>,
) -> Result<Vec<Device>> {
    let mut devices = Vec::new();
    let mut problems = Vec::new();
    for found in [physical, simulators] {
        match found {
            Ok(found) => devices.extend(found),
            Err(problem) => problems.push(problem),
        }
    }
    if devices.is_empty() {
        if problems.is_empty() {
            bail!(
                "no paired iPad or iPad simulator found; pair an iPad in Xcode or \
                 add an iPad simulator"
            );
        }
        bail!("no devices found:\n{}", problems.join("\n"));
    }
    for problem in &problems {
        eprintln!("warning: {problem}");
    }
    Ok(devices)
}

fn list_physical() -> std::result::Result<Vec<Device>, String> {
    let json = std::env::temp_dir().join(format!("oxide-ios-devices-{}.json", std::process::id()));
    let output = Command::new("xcrun")
        .args(["devicectl", "list", "devices", "--quiet", "--json-output"])
        .arg(&json)
        .output()
        .map_err(|error| {
            format!("could not run xcrun devicectl (is full Xcode installed?): {error}")
        })?;
    let text = std::fs::read_to_string(&json).unwrap_or_default();
    let _ = std::fs::remove_file(&json);
    if !output.status.success() {
        return Err(format!("xcrun devicectl failed:\n{}", tool_output(&output)));
    }
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| format!("xcrun devicectl wrote unreadable JSON: {error}"))?;
    Ok(physical_ipads(&value))
}

fn list_simulators() -> std::result::Result<Vec<Device>, String> {
    let output = Command::new("xcrun")
        .args(["simctl", "list", "devices", "available", "--json"])
        .output()
        .map_err(|error| format!("could not run xcrun simctl: {error}"))?;
    if !output.status.success() {
        return Err(format!("xcrun simctl failed:\n{}", tool_output(&output)));
    }
    let value: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("xcrun simctl printed unreadable JSON: {error}"))?;
    Ok(ipad_simulators(&value))
}

fn pick(devices: &[Device], remembered: Option<&str>) -> Result<Device> {
    if let [only] = devices {
        return Ok(only.clone());
    }
    if !std::io::stdin().is_terminal() {
        bail!(
            "several devices are available; name one with --device\n{}",
            listing(devices)
        );
    }
    let default = default_choice(devices, remembered.map(str::trim));
    println!("Devices:\n{}", listing(devices));
    loop {
        print!("Choose a device [{}]: ", default + 1);
        std::io::stdout().flush()?;
        let mut answer = String::new();
        if std::io::stdin().lock().read_line(&mut answer)? == 0 {
            bail!("no device chosen");
        }
        match choose(devices, &answer, default) {
            Ok(index) => return Ok(devices[index].clone()),
            Err(error) => eprintln!("{error}"),
        }
    }
}

fn build(root: &Path, target: &Device) -> Result<PathBuf> {
    let status = Command::new("xcodebuild")
        .current_dir(root)
        .args([
            "-project",
            "ios/Oxide.xcodeproj",
            "-scheme",
            "Oxide",
            "-configuration",
            "Release",
            "-derivedDataPath",
            "target/ios-xcode",
            "-allowProvisioningUpdates",
            "-quiet",
            "-destination",
        ])
        .arg(format!("id={}", target.id))
        .arg("build")
        .status()
        .context("running xcodebuild")?;
    if !status.success() {
        bail!("xcodebuild failed");
    }
    let products = match target.kind {
        DeviceKind::Physical => "Release-iphoneos",
        DeviceKind::Simulator { .. } => "Release-iphonesimulator",
    };
    Ok(root
        .join("target/ios-xcode/Build/Products")
        .join(products)
        .join("Oxide.app"))
}

/// The bundle ID the build signed, which honors a contributor's own ID
/// from `ios/Local.xcconfig`.
fn bundle_id(app: &Path) -> Result<String> {
    let output = Command::new("plutil")
        .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"])
        .arg(app.join("Info.plist"))
        .output()
        .context("reading the app's bundle ID")?;
    if !output.status.success() {
        bail!("could not read the bundle ID from {}", app.display());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn install_physical(target: &Device, app: &Path, bundle: &str) -> Result<()> {
    println!("installing on {}", target.name);
    let install = Command::new("xcrun")
        .args([
            "devicectl",
            "device",
            "install",
            "app",
            "--device",
            &target.id,
        ])
        .arg(app)
        .output()
        .context("running xcrun devicectl")?;
    if !install.status.success() {
        bail!("install failed:\n{}", tool_output(&install));
    }
    println!("launching");
    let launch = Command::new("xcrun")
        .args([
            "devicectl",
            "device",
            "process",
            "launch",
            "--device",
            &target.id,
        ])
        .args(["--terminate-existing", bundle])
        .output()
        .context("running xcrun devicectl")?;
    if launch.status.success() {
        println!("running on {}", target.name);
        return Ok(());
    }
    let report = tool_output(&launch);
    if report.contains("Locked") || report.contains("unlocked") {
        println!(
            "installed; {} is locked, so unlock it and open Oxide yourself",
            target.name
        );
        return Ok(());
    }
    bail!("launch failed:\n{report}")
}

fn install_simulator(target: &Device, booted: bool, app: &Path, bundle: &str) -> Result<()> {
    if !booted {
        println!("booting {}", target.name);
        let boot = Command::new("xcrun")
            .args(["simctl", "boot", &target.id])
            .output()
            .context("running xcrun simctl")?;
        if !boot.status.success() {
            bail!("boot failed:\n{}", tool_output(&boot));
        }
    }
    // Brings the Simulator window forward; the app runs either way, and
    // some Xcode installs ship no standalone Simulator app to open.
    let shown = Command::new("open")
        .args(["-a", "Simulator"])
        .output()
        .is_ok_and(|output| output.status.success());
    if !shown {
        println!("no Simulator app to open; use Xcode > Open Developer Tool > Simulator to watch");
    }
    println!("installing on {}", target.name);
    let install = Command::new("xcrun")
        .args(["simctl", "install", &target.id])
        .arg(app)
        .output()
        .context("running xcrun simctl")?;
    if !install.status.success() {
        bail!("install failed:\n{}", tool_output(&install));
    }
    let launch = Command::new("xcrun")
        .args([
            "simctl",
            "launch",
            "--terminate-running-process",
            &target.id,
            bundle,
        ])
        .output()
        .context("running xcrun simctl")?;
    if !launch.status.success() {
        bail!("launch failed:\n{}", tool_output(&launch));
    }
    println!("running on {}", target.name);
    Ok(())
}

fn tool_output(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn devicectl() -> Value {
        json!({"result": {"devices": [
            {
                "hardwareProperties": {"udid": "00008132-AAAA", "reality": "physical",
                    "deviceType": "iPad", "marketingName": "iPad Pro 11-inch (M4)"},
                "connectionProperties": {"pairingState": "paired", "transportType": "localNetwork"},
                "deviceProperties": {"name": "Connor's iPad"}
            },
            {
                "hardwareProperties": {"udid": "00008132-BBBB", "reality": "physical",
                    "deviceType": "iPhone", "marketingName": "iPhone 17"},
                "connectionProperties": {"pairingState": "paired", "transportType": "wired"},
                "deviceProperties": {"name": "Phone"}
            },
            {
                "hardwareProperties": {"udid": "00008132-CCCC", "reality": "physical",
                    "deviceType": "iPad", "marketingName": "iPad mini"},
                "connectionProperties": {"pairingState": "unpaired", "transportType": "wired"},
                "deviceProperties": {"name": "Stranger's iPad"}
            },
            {
                "hardwareProperties": {"udid": "SIM-1", "reality": "simulated",
                    "deviceType": "iPad", "marketingName": "iPad Pro 13-inch (M5)"},
                "connectionProperties": {"pairingState": "paired", "transportType": "sameMachine"},
                "deviceProperties": {"name": "iPad Pro 13-inch (M5)"}
            }
        ]}})
    }

    fn simctl() -> Value {
        json!({"devices": {
            "com.apple.CoreSimulator.SimRuntime.iOS-27-0": [
                {"name": "iPad mini (A17 Pro)", "udid": "SIM-2", "state": "Shutdown",
                 "isAvailable": true,
                 "deviceTypeIdentifier": "com.apple.CoreSimulator.SimDeviceType.iPad-mini-A17-Pro"},
                {"name": "iPad Pro 13-inch (M5)", "udid": "SIM-1", "state": "Booted",
                 "isAvailable": true,
                 "deviceTypeIdentifier": "com.apple.CoreSimulator.SimDeviceType.iPad-Pro-13-inch-M5-12GB"},
                {"name": "iPhone 17", "udid": "SIM-3", "state": "Shutdown", "isAvailable": true,
                 "deviceTypeIdentifier": "com.apple.CoreSimulator.SimDeviceType.iPhone-17"}
            ],
            "com.apple.CoreSimulator.SimRuntime.watchOS-12-0": [
                {"name": "Apple Watch", "udid": "SIM-4", "state": "Shutdown", "isAvailable": true,
                 "deviceTypeIdentifier": "com.apple.CoreSimulator.SimDeviceType.Apple-Watch"}
            ]
        }})
    }

    fn all() -> Vec<Device> {
        physical_ipads(&devicectl())
            .into_iter()
            .chain(ipad_simulators(&simctl()))
            .collect()
    }

    #[test]
    fn only_paired_ipads_and_ipad_simulators_are_targets() {
        let devices = all();
        let names: Vec<_> = devices.iter().map(|d| d.to_string()).collect();
        assert_eq!(
            names,
            [
                "Connor's iPad (iPad Pro 11-inch (M4), Wi-Fi)",
                "iPad Pro 13-inch (M5) (simulator, iOS 27.0, booted)",
                "iPad mini (A17 Pro) (simulator, iOS 27.0)",
            ]
        );
        assert_eq!(devices[0].kind, DeviceKind::Physical);
        assert_eq!(devices[1].kind, DeviceKind::Simulator { booted: true });
        assert!(physical_ipads(&Value::Null).is_empty());
        assert!(ipad_simulators(&Value::Null).is_empty());
    }

    #[test]
    fn a_device_query_takes_ids_names_and_unique_fragments() {
        let devices = all();
        assert_eq!(
            find(&devices, "00008132-aaaa").unwrap().name,
            "Connor's iPad"
        );
        assert_eq!(find(&devices, "ipad mini (a17 pro)").unwrap().id, "SIM-2");
        assert_eq!(find(&devices, "connor").unwrap().id, "00008132-AAAA");
        assert!(find(&devices, "iPad").is_err(), "ambiguous");
        assert!(find(&devices, "watch").is_err(), "no match");
    }

    #[test]
    fn a_name_shared_across_runtimes_must_be_picked_by_id() {
        let mut devices = all();
        let mut twin = devices[1].clone();
        twin.id = "SIM-9".to_string();
        twin.detail = "simulator, iOS 26.0".to_string();
        devices.push(twin);
        let error = find(&devices, "iPad Pro 13-inch (M5)")
            .unwrap_err()
            .to_string();
        assert!(error.contains("more than one"), "{error}");
        assert_eq!(
            find(&devices, "SIM-9").unwrap().detail,
            "simulator, iOS 26.0"
        );
    }

    #[test]
    fn a_failing_tool_is_reported_not_read_as_no_devices() {
        let ipad = || Ok(physical_ipads(&devicectl()));
        let broken = || Err("xcrun simctl failed:\nCoreSimulator is broken".to_string());
        assert_eq!(
            combine(ipad(), broken()).unwrap().len(),
            1,
            "the iPad still works"
        );
        let error = combine(Ok(Vec::new()), broken()).unwrap_err().to_string();
        assert!(error.contains("CoreSimulator is broken"), "{error}");
        let error = combine(Ok(Vec::new()), Ok(Vec::new()))
            .unwrap_err()
            .to_string();
        assert!(error.contains("no paired iPad"), "{error}");
    }

    #[test]
    fn the_picker_defaults_to_the_last_choice_and_reads_numbers_or_names() {
        let devices = all();
        assert_eq!(default_choice(&devices, None), 0);
        assert_eq!(default_choice(&devices, Some("SIM-2")), 2);
        assert_eq!(default_choice(&devices, Some("gone")), 0);
        assert_eq!(choose(&devices, "\n", 2).unwrap(), 2);
        assert_eq!(choose(&devices, " 2 ", 0).unwrap(), 1);
        assert!(choose(&devices, "9", 0).is_err());
        assert_eq!(choose(&devices, "mini", 0).unwrap(), 2);
        assert_eq!(
            listing(&devices).lines().next(),
            Some("  1) Connor's iPad (iPad Pro 11-inch (M4), Wi-Fi)")
        );
    }
}
