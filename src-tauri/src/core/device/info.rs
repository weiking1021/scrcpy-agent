//! What can be read from a connected device: its make, system, battery, storage
//! and screen. Each part is read by one adb shell command and parsed from the
//! exact form that command prints; a part that is not in that form is `None`,
//! it is never guessed.

use super::display::{self, ScreenSize};
use serde::Serialize;
use std::collections::HashMap;

/// The shell commands, one per part of the information.
pub const PROPS_COMMAND: &str = "getprop";
pub const BATTERY_COMMAND: &str = "dumpsys battery";
pub const STORAGE_COMMAND: &str = "df -k /data";
pub const MEMORY_COMMAND: &str = "cat /proc/meminfo";
pub const UPTIME_COMMAND: &str = "cat /proc/uptime";
pub const SIZE_COMMAND: &str = "wm size";
pub const DENSITY_COMMAND: &str = "wm density";

/// What `getprop` printed: `[name]: [value]` lines.
pub fn parse_props(output: &str) -> HashMap<String, String> {
    output
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once("]: [")?;
            let name = name.strip_prefix('[')?;
            let value = value.strip_suffix(']')?;
            Some((name.to_string(), value.to_string()))
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct System {
    pub manufacturer: Option<String>,
    pub brand: Option<String>,
    pub model: Option<String>,
    /// The device's code name.
    pub device: Option<String>,
    pub soc: Option<String>,
    pub android: Option<String>,
    pub sdk: Option<u32>,
    pub security_patch: Option<String>,
    pub build: Option<String>,
    pub abi: Option<String>,
    pub locale: Option<String>,
}

/// A property with a value (an empty one is not set).
fn prop(props: &HashMap<String, String>, name: &str) -> Option<String> {
    props.get(name).filter(|v| !v.is_empty()).cloned()
}

pub fn system_from_props(props: &HashMap<String, String>) -> System {
    let soc = match (prop(props, "ro.soc.manufacturer"), prop(props, "ro.soc.model")) {
        (Some(m), Some(model)) => Some(format!("{m} {model}")),
        (None, Some(model)) => Some(model),
        _ => None,
    };
    System {
        manufacturer: prop(props, "ro.product.manufacturer"),
        brand: prop(props, "ro.product.brand"),
        model: prop(props, "ro.product.model"),
        device: prop(props, "ro.product.device"),
        soc,
        android: prop(props, "ro.build.version.release"),
        sdk: prop(props, "ro.build.version.sdk").and_then(|v| v.parse().ok()),
        security_patch: prop(props, "ro.build.version.security_patch"),
        build: prop(props, "ro.build.display.id"),
        abi: prop(props, "ro.product.cpu.abi"),
        locale: prop(props, "persist.sys.locale").or_else(|| prop(props, "ro.product.locale")),
    }
}

/// How the battery is being used, as `BatteryManager` numbers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BatteryStatus {
    Charging,
    Discharging,
    NotCharging,
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Battery {
    /// Percent of `scale`.
    pub level: u32,
    pub scale: u32,
    /// `None` if the number is not one of those `BatteryManager` defines.
    pub status: Option<BatteryStatus>,
    /// In tenths of a degree Celsius.
    pub temperature: Option<i32>,
    /// In millivolts.
    pub voltage: Option<u32>,
    pub ac: Option<bool>,
    pub usb: Option<bool>,
    pub wireless: Option<bool>,
}

/// The `name: value` lines of `dumpsys battery`.
pub fn parse_battery(output: &str) -> Option<Battery> {
    let fields: HashMap<&str, &str> = output
        .lines()
        .skip_while(|l| l.trim_end() != "Current Battery Service state:")
        .skip(1)
        .filter_map(|l| l.split_once(": "))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect();
    let number = |k: &str| fields.get(k).and_then(|v| v.parse::<u32>().ok());
    let flag = |k: &str| match *fields.get(k)? {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    };
    let scale = number("scale").filter(|s| *s > 0)?;
    let level = number("level").filter(|l| *l <= scale)?;
    let status = match number("status") {
        Some(2) => Some(BatteryStatus::Charging),
        Some(3) => Some(BatteryStatus::Discharging),
        Some(4) => Some(BatteryStatus::NotCharging),
        Some(5) => Some(BatteryStatus::Full),
        _ => None,
    };
    Some(Battery {
        level,
        scale,
        status,
        temperature: fields.get("temperature").and_then(|v| v.parse().ok()),
        voltage: number("voltage"),
        ac: flag("AC powered"),
        usb: flag("USB powered"),
        wireless: flag("Wireless powered"),
    })
}

/// Sizes in kilobytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Storage {
    pub total_kb: u64,
    pub used_kb: u64,
    pub available_kb: u64,
}

/// `df -k`: a header line, then `<filesystem> <blocks> <used> <available> <use%> <mount>`.
pub fn parse_storage(output: &str) -> Option<Storage> {
    let mut lines = output.lines();
    let header: Vec<&str> = lines.next()?.split_whitespace().collect();
    if header.get(1..4)? != ["1K-blocks", "Used", "Available"] {
        return None;
    }
    let row: Vec<&str> = lines.next()?.split_whitespace().collect();
    if row.len() < 6 || !row[4].ends_with('%') {
        return None;
    }
    Some(Storage { total_kb: row[1].parse().ok()?, used_kb: row[2].parse().ok()?, available_kb: row[3].parse().ok()? })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Memory {
    pub total_kb: u64,
    pub available_kb: u64,
}

/// `MemTotal: <n> kB` and `MemAvailable: <n> kB` of `/proc/meminfo`.
pub fn parse_memory(output: &str) -> Option<Memory> {
    let kb = |name: &str| {
        output.lines().find_map(|l| {
            let value = l.strip_prefix(name)?.strip_prefix(':')?.trim().strip_suffix(" kB")?;
            value.trim().parse::<u64>().ok()
        })
    };
    Some(Memory { total_kb: kb("MemTotal")?, available_kb: kb("MemAvailable")? })
}

/// Seconds since the device started: the first number of `/proc/uptime`.
pub fn parse_uptime(output: &str) -> Option<u64> {
    let seconds: f64 = output.split_whitespace().next()?.parse().ok()?;
    (seconds.is_finite() && seconds >= 0.0).then_some(seconds as u64)
}

/// `Physical size: <w>x<h>` (and, if it was changed, `Override size:` after it).
pub fn parse_wm_size(output: &str) -> Option<ScreenSize> {
    let size = output.lines().find_map(|l| l.strip_prefix("Physical size: "))?;
    let (w, h) = size.trim().split_once('x')?;
    ScreenSize::new(w.parse().ok()?, h.parse().ok()?)
}

/// `Physical density: <n>`.
pub fn parse_wm_density(output: &str) -> Option<u32> {
    let value = output.lines().find_map(|l| l.strip_prefix("Physical density: "))?;
    value.trim().parse().ok().filter(|d| *d > 0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Screen {
    /// The size the screen has as it is held now.
    pub width: u32,
    pub height: u32,
    /// The panel's natural size.
    pub physical_width: u32,
    pub physical_height: u32,
    pub density: Option<u32>,
}

/// Everything read from the device. A part that could not be read is `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceInfo {
    pub system: System,
    pub battery: Option<Battery>,
    pub storage: Option<Storage>,
    pub memory: Option<Memory>,
    pub uptime_seconds: Option<u64>,
    pub screen: Option<Screen>,
}

/// What the commands printed, in the order of the constants above.
pub struct Outputs<'a> {
    pub props: &'a str,
    pub battery: &'a str,
    pub storage: &'a str,
    pub memory: &'a str,
    pub uptime: &'a str,
    pub size: &'a str,
    pub density: &'a str,
    /// The output of `display::DISPLAY_DUMP_COMMAND`.
    pub display: &'a str,
}

pub fn assemble(out: &Outputs) -> DeviceInfo {
    let physical = parse_wm_size(out.size);
    let current = display::current_size(out.display).or(physical);
    let screen = match (current, physical) {
        (Some(c), Some(p)) => Some(Screen {
            width: c.width,
            height: c.height,
            physical_width: p.width,
            physical_height: p.height,
            density: parse_wm_density(out.density),
        }),
        _ => None,
    };
    DeviceInfo {
        system: system_from_props(&parse_props(out.props)),
        battery: parse_battery(out.battery),
        storage: parse_storage(out.storage),
        memory: parse_memory(out.memory),
        uptime_seconds: parse_uptime(out.uptime),
        screen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every value below is invented. Only the form of each line (what each
    // command prints) was checked against a real device, once; none of these
    // numbers or names comes from one.
    const PROPS: &str = "\
[persist.sys.locale]: [en-US]
[ro.build.display.id]: [BUILD.0001 test-keys]
[ro.build.version.release]: [11]
[ro.build.version.sdk]: [30]
[ro.build.version.security_patch]: [2000-01-01]
[ro.hardware]: [hw0]
[ro.product.brand]: [Brand]
[ro.product.cpu.abi]: [x86_64]
[ro.product.device]: [codename]
[ro.product.locale]: [en-GB]
[ro.product.manufacturer]: [Maker]
[ro.product.model]: [MODEL0001]
[ro.soc.manufacturer]: [Foundry]
[ro.soc.model]: [CHIP01]
[ro.empty]: []
";

    const BATTERY: &str = "Current Battery Service state:
  AC powered: false
  USB powered: true
  Wireless powered: false
  Max charging current: 0
  Charge counter: 1000
  status: 2
  health: 2
  present: true
  level: 61
  scale: 100
  voltage: 4000
  temperature: 250
  technology: Li-poly
";

    const STORAGE: &str = "Filesystem     1K-blocks      Used Available Use% Mounted on
/dev/block/dm-0   1000000    400000    600000  40% /data/user/0
";

    const MEMORY: &str = "MemTotal:        4000000 kB
MemFree:          500000 kB
MemAvailable:    2500000 kB
";

    #[test]
    fn the_system_is_read_from_the_properties() {
        let props = parse_props(PROPS);
        assert_eq!(props.len(), 15);
        assert_eq!(
            system_from_props(&props),
            System {
                manufacturer: Some("Maker".into()),
                brand: Some("Brand".into()),
                model: Some("MODEL0001".into()),
                device: Some("codename".into()),
                soc: Some("Foundry CHIP01".into()),
                android: Some("11".into()),
                sdk: Some(30),
                security_patch: Some("2000-01-01".into()),
                build: Some("BUILD.0001 test-keys".into()),
                abi: Some("x86_64".into()),
                locale: Some("en-US".into()),
            }
        );
    }

    #[test]
    fn properties_that_are_missing_or_empty_are_none_and_lines_of_another_form_are_dropped() {
        let props = parse_props("[ro.product.model]: [X]\n[ro.product.brand]: []\nnot a property\n[broken]: value\n");
        assert_eq!(props.len(), 2);
        let system = system_from_props(&props);
        assert_eq!(system.model.as_deref(), Some("X"));
        assert_eq!(system.brand, None);
        assert_eq!(system.sdk, None);
        // The locale falls back to the product's when the user's is not set.
        let fallback = system_from_props(&parse_props("[ro.product.locale]: [en-GB]\n"));
        assert_eq!(fallback.locale.as_deref(), Some("en-GB"));
        // A chip without a maker is named by its model only.
        assert_eq!(system_from_props(&parse_props("[ro.soc.model]: [S1]\n")).soc.as_deref(), Some("S1"));
    }

    #[test]
    fn the_battery_is_read_from_its_service_state() {
        assert_eq!(
            parse_battery(BATTERY),
            Some(Battery {
                level: 61,
                scale: 100,
                status: Some(BatteryStatus::Charging),
                temperature: Some(250),
                voltage: Some(4000),
                ac: Some(false),
                usb: Some(true),
                wireless: Some(false),
            })
        );
    }

    #[test]
    fn a_battery_state_that_does_not_add_up_is_not_read() {
        assert_eq!(parse_battery(""), None);
        // Fields before the header are not the service state.
        assert_eq!(parse_battery("level: 61\nscale: 100\n"), None);
        assert_eq!(parse_battery("Current Battery Service state:\n  level: 101\n  scale: 100\n"), None);
        assert_eq!(parse_battery("Current Battery Service state:\n  level: 5\n  scale: 0\n"), None);
        // An unknown status number is not turned into a status.
        let odd = BATTERY.replace("status: 2", "status: 9");
        assert_eq!(parse_battery(&odd).unwrap().status, None);
    }

    #[test]
    fn the_storage_is_the_row_under_the_known_header() {
        assert_eq!(
            parse_storage(STORAGE),
            Some(Storage { total_kb: 1000000, used_kb: 400000, available_kb: 600000 })
        );
        assert_eq!(parse_storage(""), None);
        assert_eq!(parse_storage("Filesystem Size Used Avail Use% Mounted on\n/dev/x 1 2 3 4% /data\n"), None);
        assert_eq!(parse_storage("Filesystem 1K-blocks Used Available Use% Mounted on\n/dev/x a 2 3 4% /data\n"), None);
        assert_eq!(parse_storage("Filesystem 1K-blocks Used Available Use% Mounted on\n"), None);
    }

    #[test]
    fn the_memory_is_the_total_and_the_available_amount() {
        assert_eq!(parse_memory(MEMORY), Some(Memory { total_kb: 4000000, available_kb: 2500000 }));
        assert_eq!(parse_memory("MemTotal: 5 kB\n"), None);
        assert_eq!(parse_memory("MemTotal: five kB\nMemAvailable: 1 kB\n"), None);
    }

    #[test]
    fn uptime_is_the_first_number() {
        assert_eq!(parse_uptime("3600.50 7200.25\n"), Some(3600));
        assert_eq!(parse_uptime(""), None);
        assert_eq!(parse_uptime("soon 1\n"), None);
        assert_eq!(parse_uptime("-5 1\n"), None);
    }

    #[test]
    fn the_screen_size_and_density_are_read_from_wm() {
        assert_eq!(parse_wm_size("Physical size: 1200x1920\n"), ScreenSize::new(1200, 1920));
        assert_eq!(
            parse_wm_size("Physical size: 1200x1920\nOverride size: 600x960\n"),
            ScreenSize::new(1200, 1920)
        );
        assert_eq!(parse_wm_size("Physical size: 0x1920\n"), None);
        assert_eq!(parse_wm_size("Physical size: wide\n"), None);
        assert_eq!(parse_wm_density("Physical density: 240\n"), Some(240));
        assert_eq!(parse_wm_density("Physical density: 0\n"), None);
        assert_eq!(parse_wm_density(""), None);
    }

    #[test]
    fn the_whole_information_is_put_together_and_a_missing_part_stays_missing() {
        let display = "    mOverrideDisplayInfo=DisplayInfo{\"Display\", displayId 0\", real 1920 x 1200, x\n";
        let info = assemble(&Outputs {
            props: PROPS,
            battery: BATTERY,
            storage: STORAGE,
            memory: MEMORY,
            uptime: "3600.50 7200.25\n",
            size: "Physical size: 1200x1920\n",
            density: "Physical density: 240\n",
            display,
        });
        assert_eq!(info.system.model.as_deref(), Some("MODEL0001"));
        assert_eq!(
            info.screen,
            Some(Screen { width: 1920, height: 1200, physical_width: 1200, physical_height: 1920, density: Some(240) })
        );
        assert_eq!(info.uptime_seconds, Some(3600));

        let nothing = assemble(&Outputs {
            props: "",
            battery: "",
            storage: "",
            memory: "",
            uptime: "",
            size: "",
            density: "",
            display: "",
        });
        assert_eq!(nothing.battery, None);
        assert_eq!(nothing.storage, None);
        assert_eq!(nothing.memory, None);
        assert_eq!(nothing.screen, None);
        assert_eq!(nothing.system.model, None);

        // Without the live size, the panel's own is what there is.
        let panel_only = assemble(&Outputs {
            size: "Physical size: 1200x1920\n",
            ..Outputs {
                props: "",
                battery: "",
                storage: "",
                memory: "",
                uptime: "",
                size: "",
                density: "",
                display: "",
            }
        });
        let screen = panel_only.screen.unwrap();
        assert_eq!((screen.width, screen.height, screen.density), (1200, 1920, None));
    }
}
