//! The adb program, and the typed values read from what the adb server says.
//!
//! The application talks to the adb server itself (see `wire`); the adb
//! program is only started to bring the server up. What the server lists is
//! text with a fixed grammar, and nothing here guesses with substring
//! searches. Every line is parsed into a type by exact matching; a line that
//! does not have the expected shape is dropped, never reinterpreted. Decisions
//! are made from these typed values (and from other facts such as TCP
//! reachability), never from adb's messages.

pub mod api;
pub mod cli;
pub mod select;
pub mod wire;

use anyhow::{Result, anyhow, bail};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

/// The adb program of the scrcpy release in use.
pub struct ProcessAdb {
    pub path: PathBuf,
}

#[cfg(windows)]
pub(super) fn hide_window(cmd: &mut Command) {
    // CREATE_NO_WINDOW: adb.exe is a console program and would flash a window.
    cmd.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
pub(super) fn hide_window(_cmd: &mut Command) {}

impl ProcessAdb {
    /// Stops the adb server that answers at the usual place (one that is not running is an error).
    pub async fn kill_server(&self) -> Result<()> {
        let mut cmd = Command::new(&self.path);
        cmd.arg("kill-server").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
        hide_window(&mut cmd);
        let status = timeout(Duration::from_secs(30), cmd.status())
            .await
            .map_err(|_| anyhow!("adb kill-server timed out"))??;
        if !status.success() {
            bail!("adb kill-server failed ({status})");
        }
        Ok(())
    }

    /// Starts the adb server with all stdio detached. This must not use piped
    /// output: the daemon inherits the pipes and the call would never return.
    pub async fn start_server(&self) -> Result<()> {
        let mut cmd = Command::new(&self.path);
        cmd.arg("start-server")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        hide_window(&mut cmd);
        let status = timeout(Duration::from_secs(30), cmd.status())
            .await
            .map_err(|_| anyhow!("adb start-server timed out"))??;
        if !status.success() {
            bail!("adb start-server failed ({status})");
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ devices

/// The connection state column of `adb devices`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdbState {
    Device,
    Offline,
    Unauthorized,
    Authorizing,
    Connecting,
    NoPermissions,
    Recovery,
    Sideload,
    Bootloader,
    Rescue,
    Host,
}

impl AdbState {
    fn parse(token: &str) -> Option<Self> {
        Some(match token {
            "device" => AdbState::Device,
            "offline" => AdbState::Offline,
            "unauthorized" => AdbState::Unauthorized,
            "authorizing" => AdbState::Authorizing,
            "connecting" => AdbState::Connecting,
            "recovery" => AdbState::Recovery,
            "sideload" => AdbState::Sideload,
            "bootloader" => AdbState::Bootloader,
            "rescue" => AdbState::Rescue,
            "host" => AdbState::Host,
            _ => return None,
        })
    }
}

/// What kind of identifier an adb serial is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SerialKind {
    /// `ip:port`, a device connected over the network by address.
    Network(SocketAddr),
    /// `<instance>._adb-tls-connect._tcp`, a device connected through mDNS.
    MdnsName { instance: String },
    /// Anything else: a USB serial or an emulator.
    Local,
}

impl SerialKind {
    pub fn parse(serial: &str) -> Self {
        if let Ok(addr) = serial.parse::<SocketAddr>() {
            return SerialKind::Network(addr);
        }
        match labels(serial).as_deref() {
            Some([instance, "_adb-tls-connect", "_tcp"]) => {
                SerialKind::MdnsName { instance: (*instance).to_string() }
            }
            _ => SerialKind::Local,
        }
    }
}

/// Splits a DNS-SD name into its dot-separated labels, ignoring one trailing
/// dot. Returns `None` for an empty name.
fn labels(name: &str) -> Option<Vec<&str>> {
    let name = name.strip_suffix('.').unwrap_or(name);
    (!name.is_empty()).then(|| name.split('.').collect())
}

/// One device line of `adb devices -l`.
#[derive(Debug, Clone, PartialEq)]
pub struct AdbEntry {
    /// The exact string to pass to `adb -s`.
    pub serial: String,
    pub kind: SerialKind,
    pub state: AdbState,
    pub model: Option<String>,
    pub is_usb: bool,
}

impl AdbEntry {
    pub fn is_wifi(&self) -> bool {
        !self.is_usb && !matches!(self.kind, SerialKind::Local)
    }

    pub fn network_address(&self) -> Option<SocketAddr> {
        match self.kind {
            SerialKind::Network(a) => Some(a),
            _ => None,
        }
    }
}

/// Parses the output of `adb devices -l`. Lines that do not have the shape
/// `<serial> <known state> [key:value ...]` (the header, daemon banners,
/// anything unexpected) are skipped.
pub fn parse_devices(stdout: &str) -> Vec<AdbEntry> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let mut tokens = line.split_whitespace();
        let (Some(serial), Some(state_token)) = (tokens.next(), tokens.next()) else {
            continue;
        };
        let rest: Vec<&str> = tokens.collect();
        // "no permissions" is the one state that is written with a space.
        let (state, rest) = match (state_token, rest.first().copied()) {
            ("no", Some("permissions")) => (Some(AdbState::NoPermissions), &rest[1..]),
            _ => (AdbState::parse(state_token), &rest[..]),
        };
        let Some(state) = state else { continue };

        let mut model = None;
        let mut is_usb = false;
        for token in rest {
            match token.split_once(':') {
                Some(("model", value)) => model = Some(value.replace('_', " ")),
                Some(("usb", _)) => is_usb = true,
                _ => {}
            }
        }
        out.push(AdbEntry {
            serial: serial.to_string(),
            kind: SerialKind::parse(serial),
            state,
            model,
            is_usb,
        });
    }
    out
}

// --------------------------------------------------------------------- mDNS

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MdnsKind {
    /// `_adb-tls-connect._tcp`: Android 11+ wireless debugging.
    Connect,
    /// `_adb-tls-pairing._tcp`: the pairing dialog is open on the device.
    Pairing,
    /// `_adb._tcp`: legacy `adb tcpip` mode.
    Legacy,
}

impl MdnsKind {
    /// Parses a DNS-SD service type. adb prints it without the domain
    /// (`_adb-tls-connect._tcp.`), the mDNS library with it
    /// (`_adb-tls-connect._tcp.local.`); both are accepted.
    pub fn parse(service_type: &str) -> Option<Self> {
        match labels(service_type)?.as_slice() {
            ["_adb-tls-connect", "_tcp"] | ["_adb-tls-connect", "_tcp", "local"] => {
                Some(MdnsKind::Connect)
            }
            ["_adb-tls-pairing", "_tcp"] | ["_adb-tls-pairing", "_tcp", "local"] => {
                Some(MdnsKind::Pairing)
            }
            ["_adb", "_tcp"] | ["_adb", "_tcp", "local"] => Some(MdnsKind::Legacy),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MdnsService {
    pub instance: String,
    pub kind: MdnsKind,
    pub address: SocketAddr,
}

impl MdnsService {
    /// The hardware serial encoded in the instance name, if any.
    pub fn serial(&self) -> Option<String> {
        serial_from_instance(&self.instance, self.kind)
    }
}

/// adb names its instances `adb-<serial>`, and appends a dash and six random
/// alphanumeric characters for the TLS services.
pub fn serial_from_instance(instance: &str, kind: MdnsKind) -> Option<String> {
    let rest = instance.strip_prefix("adb-")?;
    let serial = match kind {
        MdnsKind::Connect => strip_random_suffix(rest),
        MdnsKind::Legacy => rest,
        MdnsKind::Pairing => return None,
    };
    (!serial.is_empty()).then(|| serial.to_string())
}

fn strip_random_suffix(rest: &str) -> &str {
    match rest.rsplit_once('-') {
        Some((serial, suffix))
            if !serial.is_empty()
                && suffix.len() == 6
                && suffix.bytes().all(|b| b.is_ascii_alphanumeric()) =>
        {
            serial
        }
        _ => rest,
    }
}

/// Parses `adb mdns services`: one `<instance>\t<type>\t<ip:port>` line per
/// service. Anything that does not have exactly that shape is skipped.
pub fn parse_mdns(stdout: &str) -> Vec<MdnsService> {
    stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            // Tab separated; whitespace separated is accepted as a fallback.
            let tabbed: Vec<&str> = line.split('\t').map(str::trim).collect();
            let fields = if tabbed.len() == 3 { tabbed } else { line.split_whitespace().collect() };
            let [instance, service_type, address] = fields.as_slice() else {
                return None;
            };
            Some(MdnsService {
                instance: (*instance).to_string(),
                kind: MdnsKind::parse(service_type)?,
                address: address.parse().ok()?,
            })
        })
        .collect()
}

// ------------------------------------------------------------------ forwards

/// What a forward connects to on the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardRemote {
    /// `localabstract:<name>`: a socket in the device's abstract namespace,
    /// which is what scrcpy's server listens on (`scrcpy_<id>`).
    LocalAbstract(String),
    /// `tcp:<port>` on the device.
    Tcp(u16),
    /// Any other kind of target, kept as adb wrote it.
    Other(String),
}

/// One `adb forward`: connections to a local TCP port go to the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forward {
    /// The adb serial of the device it is for.
    pub serial: String,
    pub local_port: u16,
    pub remote: ForwardRemote,
}

/// Parses the list of forwards: one `<serial> <local> <remote>` line each, the
/// local side `tcp:<port>`. A line of any other shape is skipped.
pub fn parse_forwards(text: &str) -> Vec<Forward> {
    text.lines().filter_map(parse_forward).collect()
}

/// A port as plain digits, from 1 to 65535.
fn parse_port(text: &str) -> Option<u16> {
    // `parse` also accepts a leading `+`.
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse::<u16>().ok())
        .flatten()
        .filter(|port| *port != 0)
}

fn parse_forward(line: &str) -> Option<Forward> {
    let mut tokens = line.split_whitespace();
    let (serial, local, remote) = (tokens.next()?, tokens.next()?, tokens.next()?);
    if tokens.next().is_some() {
        return None;
    }
    let local_port = parse_port(local.strip_prefix("tcp:")?)?;
    let remote = if let Some(name) = remote.strip_prefix("localabstract:") {
        ForwardRemote::LocalAbstract((!name.is_empty()).then(|| name.to_string())?)
    } else if let Some(port) = remote.strip_prefix("tcp:") {
        ForwardRemote::Tcp(parse_port(port)?)
    } else {
        ForwardRemote::Other(remote.to_string())
    };
    Some(Forward { serial: serial.to_string(), local_port, remote })
}

// -------------------------------------------------------------- device props

/// Result of the one-off property query run for each newly connected device.
#[derive(Debug, Clone, PartialEq)]
pub struct Props {
    pub serial: String,
    pub model: String,
    pub android: String,
    pub sdk: Option<u32>,
}

pub const PROPS_COMMAND: &str = "getprop ro.serialno; getprop ro.product.model; \
     getprop ro.build.version.release; getprop ro.build.version.sdk";

/// The command prints exactly four lines, one per property, in that order. If
/// anything else precedes them, only the last four lines are used.
pub fn parse_props(stdout: &str) -> Option<Props> {
    let lines: Vec<&str> = stdout.lines().map(str::trim).collect();
    let [serial, model, android, sdk] = lines.get(lines.len().checked_sub(4)?..)? else {
        return None;
    };
    Some(Props {
        // Android reports "unknown" when a device has no serial number.
        serial: if serial.eq_ignore_ascii_case("unknown") {
            String::new()
        } else {
            (*serial).to_string()
        },
        model: (*model).to_string(),
        android: (*android).to_string(),
        sdk: sdk.parse().ok(),
    })
}

// ------------------------------------------------------------------- input

/// Parses an address typed by the user. Only IP literals with a non-zero port
/// are accepted, so the value can never be mistaken for an adb option.
pub fn parse_user_address(text: &str) -> Option<SocketAddr> {
    text.trim()
        .parse::<SocketAddr>()
        .ok()
        .filter(|a| a.port() != 0 && !a.ip().is_unspecified() && !a.ip().is_multicast())
}

/// The adb programs of the scrcpy releases, run as programs of the system (see `versions::switch_adb`).
pub struct SystemAdb;

impl crate::core::scrcpy::versions::AdbProcess for SystemAdb {
    async fn kill_server(&self, adb: &std::path::Path) -> std::result::Result<(), String> {
        ProcessAdb { path: adb.to_path_buf() }.kill_server().await.map_err(|e| format!("{e:#}"))
    }

    async fn start_server(&self, adb: &std::path::Path) -> std::result::Result<(), String> {
        ProcessAdb { path: adb.to_path_buf() }.start_server().await.map_err(|e| format!("{e:#}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn parses_devices_list() {
        let text = "* daemon started successfully\n\
List of devices attached\n\
TESTDEV0001            device usb:1-2 product:testprod model:TEST_M1 device:testdev transport_id:1\n\
192.168.1.20:37000     device product:testprod model:TEST_M1 device:testdev transport_id:2\n\
adb-TESTDEV0001-x7Qk2p._adb-tls-connect._tcp device product:testprod model:TEST_M1 transport_id:3\n\
192.168.1.99:5555      offline transport_id:4\n\
ZZZ                    unauthorized usb:1-3 transport_id:5\n\
YYY                    no permissions (user in plugdev group; are you root?); see [x]\n\n";
        let e = parse_devices(text);
        assert_eq!(e.len(), 6);
        assert_eq!(e[0].serial, "TESTDEV0001");
        assert!(e[0].is_usb && !e[0].is_wifi());
        assert_eq!(e[0].kind, SerialKind::Local);
        assert_eq!(e[0].model.as_deref(), Some("TEST M1"));
        assert_eq!(e[1].kind, SerialKind::Network(addr("192.168.1.20:37000")));
        assert!(e[1].is_wifi());
        assert_eq!(
            e[2].kind,
            SerialKind::MdnsName { instance: "adb-TESTDEV0001-x7Qk2p".into() }
        );
        assert_eq!(e[3].state, AdbState::Offline);
        assert_eq!(e[4].state, AdbState::Unauthorized);
        assert_eq!(e[5].state, AdbState::NoPermissions);
    }

    #[test]
    fn lines_of_unknown_shape_are_dropped_not_guessed() {
        // A banner, a header and a line with a state we do not know.
        let text = "* daemon not running; starting now at tcp:5037\n\
List of devices attached\n\
ABC mystery-state usb:1-1\n\
lonely\n";
        assert!(parse_devices(text).is_empty());
    }

    #[test]
    fn a_serial_that_merely_contains_a_colon_is_not_an_address() {
        assert_eq!(SerialKind::parse("not:an:address"), SerialKind::Local);
        assert_eq!(SerialKind::parse("emulator-5554"), SerialKind::Local);
        // The mDNS form needs the exact service labels.
        assert_eq!(SerialKind::parse("x._adb._tcp"), SerialKind::Local);
        assert_eq!(
            SerialKind::parse("adb-A-b._adb-tls-connect._tcp."),
            SerialKind::MdnsName { instance: "adb-A-b".into() }
        );
    }

    #[test]
    fn parses_real_mdns_output() {
        // The form `mdns services` prints (tab separated); every value is invented.
        let text = "List of discovered mdns services\n\
adb-TESTSERIAL001\t_adb._tcp.\t192.168.250.41:5555\n\
adb-ABCDE12345-Zq7Rt3\t_adb-tls-connect._tcp.\t192.168.250.146:40400\n\
adb-ABCDE12345-aaaaaa\t_adb-tls-pairing._tcp.\t192.168.250.146:39000\n\
something\t_other._tcp.\t1.2.3.4:5\n\
broken\t_adb._tcp.\tnot-an-address\n";
        let s = parse_mdns(text);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].kind, MdnsKind::Legacy);
        assert_eq!(s[0].serial().as_deref(), Some("TESTSERIAL001"));
        assert_eq!(s[0].address, addr("192.168.250.41:5555"));
        assert_eq!(s[1].kind, MdnsKind::Connect);
        assert_eq!(s[1].serial().as_deref(), Some("ABCDE12345"));
        assert_eq!(s[2].kind, MdnsKind::Pairing);
        assert_eq!(s[2].serial(), None);
    }

    #[test]
    fn service_types_are_matched_exactly() {
        assert_eq!(MdnsKind::parse("_adb._tcp."), Some(MdnsKind::Legacy));
        assert_eq!(MdnsKind::parse("_adb-tls-connect._tcp"), Some(MdnsKind::Connect));
        // The form the mDNS library reports, with the domain.
        assert_eq!(MdnsKind::parse("_adb-tls-connect._tcp.local."), Some(MdnsKind::Connect));
        assert_eq!(MdnsKind::parse("_adb-tls-pairing._tcp.local."), Some(MdnsKind::Pairing));
        assert_eq!(MdnsKind::parse("_adb._tcp.local."), Some(MdnsKind::Legacy));
        assert_eq!(MdnsKind::parse("_adb._tcp.example."), None);
        // Longer or look-alike names must not match by accident.
        assert_eq!(MdnsKind::parse("_adb-tls-connect-x._tcp."), None);
        assert_eq!(MdnsKind::parse("_myadb._tcp."), None);
        assert_eq!(MdnsKind::parse("_adb._udp."), None);
        assert_eq!(MdnsKind::parse(""), None);
    }

    #[test]
    fn instance_serials() {
        assert_eq!(
            serial_from_instance("adb-AB-CD-x7Qk2p", MdnsKind::Connect).as_deref(),
            Some("AB-CD")
        );
        // A trailing part that is not six alphanumerics is part of the serial.
        assert_eq!(
            serial_from_instance("adb-AB-CD", MdnsKind::Connect).as_deref(),
            Some("AB-CD")
        );
        assert_eq!(serial_from_instance("nope", MdnsKind::Legacy), None);
    }

    #[test]
    fn forwards_are_parsed_into_their_serial_local_port_and_target() {
        // The shape is what `adb forward --list` prints; serials are invented.
        let text = "SERIAL-A tcp:27183 localabstract:scrcpy_1a2b3c4d\r\n\
                    SERIAL-B tcp:5000 tcp:6000\n\
                    SERIAL-A tcp:27184 jdwp:1234\n";
        assert_eq!(
            parse_forwards(text),
            [
                Forward {
                    serial: "SERIAL-A".into(),
                    local_port: 27183,
                    remote: ForwardRemote::LocalAbstract("scrcpy_1a2b3c4d".into())
                },
                Forward { serial: "SERIAL-B".into(), local_port: 5000, remote: ForwardRemote::Tcp(6000) },
                Forward { serial: "SERIAL-A".into(), local_port: 27184, remote: ForwardRemote::Other("jdwp:1234".into()) },
            ]
        );
        assert_eq!(parse_forwards(""), Vec::new());
    }

    #[test]
    fn forward_lines_of_another_shape_are_dropped_not_guessed() {
        for bad in [
            "only-two tcp:1",
            "four tokens tcp:1 localabstract:x",
            "S localabstract:x tcp:1",   // the local side is always a TCP port
            "S tcp:0 localabstract:x",   // port 0 is not a port to connect to
            "S tcp:99999 localabstract:x",
            "S tcp:+5 localabstract:x",
            "S tcp: localabstract:x",
            "S tcp:12x localabstract:x",
            "S tcp:1 localabstract:",    // an empty name
            "S tcp:1 tcp:0",
            "S tcp:1 tcp:abc",
        ] {
            assert_eq!(parse_forwards(bad), Vec::new(), "{bad:?}");
        }
    }

    #[test]
    fn parses_props_from_the_last_four_lines() {
        let p = parse_props("TESTDEV0001\nTEST-M1\n11\n30\n").unwrap();
        assert_eq!(p.serial, "TESTDEV0001");
        assert_eq!(p.sdk, Some(30));
        let noisy = parse_props("some banner\nTESTDEV0001\nTEST-M1\n11\n30\n").unwrap();
        assert_eq!(noisy, p);
        assert!(parse_props("TESTDEV0001\n").is_none());
        assert_eq!(parse_props("unknown\nm\n11\n30\n").unwrap().serial, "");
    }

    #[test]
    fn user_addresses() {
        assert_eq!(parse_user_address(" 192.168.1.2:5555 "), Some(addr("192.168.1.2:5555")));
        assert!(parse_user_address("[fe80::1]:5555").is_some());
        assert!(parse_user_address("-s:5555").is_none());
        assert!(parse_user_address("192.168.1.2").is_none());
        assert!(parse_user_address("192.168.1.2:0").is_none());
        assert!(parse_user_address("192.168.1.2:99999").is_none());
        assert!(parse_user_address("0.0.0.0:5555").is_none());
        assert!(parse_user_address("phone.local:5555").is_none());
    }
}
