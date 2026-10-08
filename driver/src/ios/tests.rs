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
    let names: Vec<_> = devices
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
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
