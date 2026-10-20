//! Turns adb's typed output into a stable device list and decides which
//! devices to (re)connect to. A device is identified by its hardware serial,
//! never by its address, so an IP or port change keeps the same card in the UI.

pub mod fallback;
pub mod gate;
pub mod mdns;
pub mod probe;
pub mod service;

use super::adb::{
    AdbEntry, AdbState, MdnsKind, MdnsService, PROPS_COMMAND, Props, SerialKind, parse_props,
    serial_from_instance,
};
use super::adb::api::AdbApi;
use super::error::CoreError;
use super::model::{Device, DeviceState, KnownDevice, PairingService, Transport};
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

const KNOWN_RETRY: Duration = Duration::from_secs(15);
const STALE_DISCONNECT_RETRY: Duration = Duration::from_secs(30);

/// Device properties by adb serial.
pub type PropsMap = HashMap<String, Props>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Connect(SocketAddr),
    Disconnect(SocketAddr),
}

/// When each action was last tried, so failures are not hammered.
pub type Attempts = HashMap<Action, Instant>;

/// Hardware serial for an adb entry. A device that has not authorised this
/// computer yet cannot be queried, so its serial is taken from the mDNS
/// service advertised at the same address; otherwise it would show up twice
/// (once as `ip:port`, once as its serial).
fn entry_id(e: &AdbEntry, props: &PropsMap, mdns: &[MdnsService]) -> String {
    if let Some(p) = props.get(&e.serial)
        && !p.serial.is_empty()
    {
        return p.serial.clone();
    }
    let from_mdns = match &e.kind {
        SerialKind::MdnsName { instance } => serial_from_instance(instance, MdnsKind::Connect),
        SerialKind::Network(addr) => mdns
            .iter()
            .filter(|m| m.kind != MdnsKind::Pairing && m.address == *addr)
            .find_map(MdnsService::serial),
        SerialKind::Local => None,
    };
    from_mdns.unwrap_or_else(|| e.serial.clone())
}

/// The network address of a wireless entry.
fn entry_address(e: &AdbEntry, id: &str, mdns: &[MdnsService]) -> Option<SocketAddr> {
    match &e.kind {
        SerialKind::Network(addr) => Some(*addr),
        SerialKind::MdnsName { .. } => mdns
            .iter()
            .find(|m| m.kind != MdnsKind::Pairing && m.serial().as_deref() == Some(id))
            .map(|m| m.address),
        SerialKind::Local => None,
    }
}

fn entry_state(e: &AdbEntry) -> DeviceState {
    match e.state {
        AdbState::Device => DeviceState::Online,
        AdbState::Unauthorized | AdbState::Authorizing => DeviceState::Unauthorized,
        AdbState::Offline
        | AdbState::Connecting
        | AdbState::NoPermissions
        | AdbState::Recovery
        | AdbState::Sideload
        | AdbState::Bootloader
        | AdbState::Rescue
        | AdbState::Host => DeviceState::Offline,
    }
}

fn is_better(a: &Device, b: &Device) -> bool {
    let (ra, rb) = (a.state.rank(), b.state.rank());
    ra < rb || (ra == rb && a.transport == Some(Transport::Usb) && b.transport != Some(Transport::Usb))
}

pub fn pairing_services(mdns: &[MdnsService]) -> Vec<PairingService> {
    mdns.iter()
        .filter(|m| m.kind == MdnsKind::Pairing)
        .map(|m| PairingService {
            name: m.instance.clone(),
            address: m.address.to_string(),
        })
        .collect()
}

pub fn build_devices(
    entries: &[AdbEntry],
    mdns: &[MdnsService],
    props: &PropsMap,
    known: &[KnownDevice],
) -> Vec<Device> {
    let mut by_id: BTreeMap<String, Device> = BTreeMap::new();

    for e in entries {
        let id = entry_id(e, props, mdns);
        let p = props.get(&e.serial);
        let kn = known.iter().find(|k| k.id == id);
        let transport = if e.is_wifi() { Transport::Wifi } else { Transport::Usb };
        let address = if transport == Transport::Wifi {
            entry_address(e, &id, mdns).map(|a| a.to_string())
        } else {
            None
        };
        let name = p
            .map(|p| p.model.clone())
            .filter(|m| !m.is_empty())
            .or_else(|| e.model.clone())
            .or_else(|| kn.map(|k| k.model.clone()))
            .unwrap_or_else(|| id.clone());
        let state = entry_state(e);
        let dev = Device {
            id: id.clone(),
            name,
            label: kn.and_then(|k| k.label.clone()),
            state,
            transport: Some(transport),
            adb_serial: (state == DeviceState::Online).then(|| e.serial.clone()),
            address,
            android: p.map(|p| p.android.clone()).filter(|s| !s.is_empty()),
            sdk: p.and_then(|p| p.sdk).or_else(|| kn.and_then(|k| k.sdk)),
            known: kn.is_some() || state == DeviceState::Online,
        };
        match by_id.get(&id) {
            Some(cur) if !is_better(&dev, cur) => {}
            _ => {
                by_id.insert(id, dev);
            }
        }
    }

    for m in mdns.iter().filter(|m| m.kind != MdnsKind::Pairing) {
        let Some(id) = m.serial() else { continue };
        if by_id.contains_key(&id) {
            continue;
        }
        let kn = known.iter().find(|k| k.id == id);
        by_id.insert(
            id.clone(),
            Device {
                name: kn.map(|k| k.model.clone()).unwrap_or_else(|| id.clone()),
                label: kn.and_then(|k| k.label.clone()),
                id,
                state: DeviceState::Discovered,
                transport: Some(Transport::Wifi),
                adb_serial: None,
                address: Some(m.address.to_string()),
                android: kn.and_then(|k| k.android.clone()),
                sdk: kn.and_then(|k| k.sdk),
                known: kn.is_some(),
            },
        );
    }

    for k in known {
        by_id.entry(k.id.clone()).or_insert_with(|| Device {
            id: k.id.clone(),
            name: k.model.clone(),
            label: k.label.clone(),
            state: DeviceState::Disconnected,
            transport: None,
            adb_serial: None,
            address: k.last_address.clone(),
            android: k.android.clone(),
            sdk: k.sdk,
            known: true,
        });
    }

    let mut out: Vec<Device> = by_id.into_values().collect();
    out.sort_by(|a, b| {
        a.state
            .rank()
            .cmp(&b.state.rank())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// Decides which `adb connect` / `adb disconnect` calls to make.
///
/// Only devices that were paired before (the `known` list) are ever connected
/// automatically, never strangers on the same network. The most recently used
/// device comes first.
pub fn plan_actions(
    entries: &[AdbEntry],
    mdns: &[MdnsService],
    props: &PropsMap,
    known: &[KnownDevice],
    now: Instant,
    attempts: &mut Attempts,
) -> Vec<Action> {
    let mut throttled = |action: Action, min: Duration| -> bool {
        if let Some(t) = attempts.get(&action)
            && now.duration_since(*t) < min
        {
            return false;
        }
        attempts.insert(action, now);
        true
    };

    let mut candidates: Vec<(u64, Vec<Action>)> = Vec::new();
    for m in mdns.iter().filter(|m| m.kind != MdnsKind::Pairing) {
        let Some(serial) = m.serial() else { continue };
        let Some(kn) = known.iter().find(|k| k.id == serial) else { continue };
        let online = entries
            .iter()
            .any(|e| e.state == AdbState::Device && entry_id(e, props, mdns) == serial);
        // Waiting for the user to approve on the phone: reconnecting would
        // only raise the prompt again.
        let waiting = entries.iter().any(|e| {
            e.network_address() == Some(m.address)
                && matches!(e.state, AdbState::Unauthorized | AdbState::Authorizing)
        });
        if online || waiting {
            continue;
        }
        if !throttled(Action::Connect(m.address), KNOWN_RETRY) {
            continue;
        }
        let mut actions = Vec::new();
        if entries
            .iter()
            .any(|e| e.network_address() == Some(m.address) && e.state != AdbState::Device)
        {
            actions.push(Action::Disconnect(m.address));
        }
        actions.push(Action::Connect(m.address));
        candidates.push((kn.last_seen, actions));
    }
    candidates.sort_by_key(|(last_seen, _)| std::cmp::Reverse(*last_seen));
    let mut out: Vec<Action> = candidates.into_iter().flat_map(|(_, a)| a).collect();

    // Wireless entries that went offline are usually left behind by an IP or
    // port change; drop them so the device can be re-added at its new address.
    for e in entries {
        let Some(addr) = e.network_address() else { continue };
        if e.state == AdbState::Offline
            && !out.contains(&Action::Disconnect(addr))
            && throttled(Action::Disconnect(addr), STALE_DISCONNECT_RETRY)
        {
            out.push(Action::Disconnect(addr));
        }
    }
    out
}

/// The adb entry that represents a connection to `address`, if there is one.
/// A device connected through mDNS is listed under its service name instead
/// of `ip:port`, so both forms are checked.
pub fn entry_for_address<'a>(
    entries: &'a [AdbEntry],
    mdns: &[MdnsService],
    address: SocketAddr,
) -> Option<&'a AdbEntry> {
    entries.iter().find(|e| match &e.kind {
        SerialKind::Network(a) => *a == address,
        SerialKind::MdnsName { instance } => mdns
            .iter()
            .any(|m| m.kind == MdnsKind::Connect && m.instance == *instance && m.address == address),
        SerialKind::Local => false,
    })
}

/// Judges a connect attempt from facts instead of adb's message text (which
/// is not a stable interface and comes with exit code 0 even on failure).
///
/// * `state`: the adb state of the entry for the address, if one exists.
/// * `port_open`: whether a plain TCP connection to the address succeeds.
///
/// A listed entry means adb is connected (an unauthorised one still waits for
/// the user's approval on the device). With no entry, an open port means the
/// device is alive and turned adb away, which is what an unpaired device
/// does; a closed port means it is asleep or the port is outdated.
pub fn classify_connect(state: Option<AdbState>, port_open: bool) -> Result<(), CoreError> {
    match state {
        Some(AdbState::Device | AdbState::Unauthorized | AdbState::Authorizing) => Ok(()),
        Some(_) => Err(CoreError::Unreachable),
        None if port_open => Err(CoreError::NotPaired),
        None => Err(CoreError::Unreachable),
    }
}

/// A device seen online during a poll; used to update the persisted list.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub id: String,
    pub model: String,
    pub android: Option<String>,
    pub sdk: Option<u32>,
    pub address: Option<SocketAddr>,
}

#[derive(Debug)]
pub struct PollResult {
    pub devices: Vec<Device>,
    pub pairing: Vec<PairingService>,
    pub seen: Vec<Seen>,
}

pub struct Monitor<A: AdbApi + Send + Sync + 'static> {
    adb: Arc<A>,
    props: PropsMap,
    attempts: Attempts,
    browser: Option<Arc<mdns::MdnsBrowser>>,
    logger: Option<super::log::Logger>,
    /// Who is connecting to which device now (shared with whoever else connects).
    gate: Arc<gate::ConnectGate>,
    /// How a port is asked whether it answers.
    probe: Arc<dyn probe::PortProbe>,
    /// How each paired device fared when its last address was tried.
    fallback: Arc<Mutex<fallback::FallbackState>>,
    /// The longest a connection to one device is waited for.
    connect_limit: Duration,
    /// Told when a connection has ended, so the list is read again at once.
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    /// The connections going on in the background.
    connecting: tokio::task::JoinSet<()>,
}

/// One connection to make in the background.
struct Job {
    address: SocketAddr,
    /// Drop the leftover entry of adb at the address first.
    drop_stale_entry: bool,
    connect: bool,
    /// Set for a paired device that is tried at its last address: its id; the port is asked first and the outcome is
    /// kept for the waits between tries.
    via_probe: Option<String>,
}

impl<A: AdbApi + Send + Sync + 'static> Monitor<A> {
    pub fn new(adb: A) -> Self {
        Monitor {
            adb: Arc::new(adb),
            props: HashMap::new(),
            attempts: HashMap::new(),
            browser: None,
            logger: None,
            gate: gate::ConnectGate::new(),
            probe: Arc::new(probe::TcpProbe::default()),
            fallback: Arc::default(),
            connect_limit: gate::CONNECT_LIMIT,
            wake: None,
            connecting: tokio::task::JoinSet::new(),
        }
    }

    /// Shares the record of who is connecting to which device with the rest of the program.
    pub fn with_gate(mut self, gate: Arc<gate::ConnectGate>) -> Self {
        self.gate = gate;
        self
    }

    /// Changes how a port is asked whether it answers.
    pub fn with_probe(mut self, probe: Arc<dyn probe::PortProbe>) -> Self {
        self.probe = probe;
        self
    }

    /// Changes the longest a connection to one device is waited for.
    pub fn with_connect_limit(mut self, limit: Duration) -> Self {
        self.connect_limit = limit;
        self
    }

    /// Calls `wake` whenever a connection has ended.
    pub fn with_wake(mut self, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.wake = Some(wake);
        self
    }

    /// Waits until the connections going on in the background have ended.
    pub async fn settle(&mut self) {
        while self.connecting.join_next().await.is_some() {}
    }

    /// Writes what the monitor does on its own (connecting known devices) to the debug log.
    pub fn with_logger(mut self, logger: super::log::Logger) -> Self {
        self.logger = Some(logger);
        self
    }

    /// Adds our own mDNS browser as a second source next to adb's own list.
    pub fn with_browser(mut self, browser: Arc<mdns::MdnsBrowser>) -> Self {
        self.browser = Some(browser);
        self
    }

    pub fn adb(&self) -> &A {
        &self.adb
    }

    /// Begins one connection in the background, unless something is connecting to that device already (it is
    /// skipped, and that is not a failure).
    fn start(&mut self, job: Job) {
        let Some(permit) = self.gate.try_begin(job.address) else { return };
        if let Some(logger) = &self.logger {
            let text = match (&job.via_probe, job.connect) {
                (Some(_), _) => format!("trying the paired device at {}", job.address),
                (None, true) => format!("connecting to the known device at {}", job.address),
                (None, false) => format!("dropping the stale entry at {}", job.address),
            };
            logger.log(super::log::LogLevel::Info, super::log::LogCode::AutoConnect, text);
        }
        let (adb, probe, fallback, wake, logger) =
            (self.adb.clone(), self.probe.clone(), self.fallback.clone(), self.wake.clone(), self.logger.clone());
        let limit = self.connect_limit;
        self.connecting.spawn(async move {
            // The permit goes back when this task ends, however it ends.
            let _permit = permit;
            let work = async {
                if let Some(id) = &job.via_probe {
                    // The port is asked first: adb takes more than twenty seconds to give up on an address that
                    // does not answer.
                    if !probe.is_open(job.address).await {
                        return Err(id.clone());
                    }
                }
                // Failures are expected (device asleep, not paired); retried later.
                if job.drop_stale_entry {
                    let _ = adb.disconnect(job.address).await;
                }
                if job.connect {
                    let _ = adb.connect(job.address).await;
                }
                match &job.via_probe {
                    Some(id) => {
                        // adb's answer is not read: the device is connected when adb's list says so.
                        let services = adb.mdns_services().await.unwrap_or_default();
                        let listed = adb.devices().await.unwrap_or_default();
                        let up = entry_for_address(&listed, &services, job.address).is_some_and(|e| e.state == AdbState::Device);
                        if up { Ok(()) } else { Err(id.clone()) }
                    }
                    None => Ok(()),
                }
            };
            let outcome = tokio::time::timeout(limit, work).await;
            if let Some(id) = &job.via_probe {
                let mut state = fallback.lock().unwrap_or_else(PoisonError::into_inner);
                match &outcome {
                    Ok(Ok(())) => state.record_success(id),
                    Ok(Err(_)) | Err(_) => state.record_failure(id, Instant::now()),
                }
                if let Some(logger) = &logger {
                    let (level, text) = match &outcome {
                        Ok(Ok(())) => (super::log::LogLevel::Info, format!("the paired device at {} is connected", job.address)),
                        Ok(Err(_)) => (super::log::LogLevel::Debug, format!("the paired device at {} did not come back", job.address)),
                        Err(_) => (super::log::LogLevel::Warn, format!("the connection to {} did not end in time", job.address)),
                    };
                    logger.log(level, super::log::LogCode::AutoConnect, text);
                }
            }
            if let Some(wake) = wake {
                wake();
            }
        });
    }

    pub async fn poll_once(&mut self, known: &[KnownDevice]) -> anyhow::Result<PollResult> {
        let entries = self.adb.devices().await?;
        // mDNS may be unavailable on some systems; USB devices still work.
        let from_adb = self.adb.mdns_services().await.unwrap_or_default();
        let mdns = match &self.browser {
            Some(b) => mdns::merge(b.snapshot(), from_adb),
            None => from_adb,
        };

        self.props
            .retain(|serial, _| entries.iter().any(|e| &e.serial == serial));
        for e in entries.iter().filter(|e| e.state == AdbState::Device) {
            if self.props.contains_key(&e.serial) {
                continue;
            }
            if let Ok(out) = self.adb.shell(&e.serial, PROPS_COMMAND).await
                && out.exit == 0
                && let Some(p) = parse_props(&String::from_utf8_lossy(&out.stdout))
            {
                self.props.insert(e.serial.clone(), p);
            }
        }

        let actions = plan_actions(
            &entries,
            &mdns,
            &self.props,
            known,
            Instant::now(),
            &mut self.attempts,
        );
        // A device that is connected or announces itself starts from the first wait if it is lost again.
        {
            let mut fallback = self.fallback.lock().unwrap_or_else(PoisonError::into_inner);
            for device in known {
                let connected = entries
                    .iter()
                    .any(|e| e.state == AdbState::Device && entry_id(e, &self.props, &mdns) == device.id);
                let announced =
                    mdns.iter().any(|m| m.kind != MdnsKind::Pairing && m.serial().as_deref() == Some(device.id.as_str()));
                if connected || announced {
                    fallback.record_success(&device.id);
                }
            }
        }

        // What mDNS announced goes in priority order (most recently used device first). Each device is connected to in
        // a task of its own: one that does not answer holds up nobody else, and not the poll either.
        let mut jobs: Vec<Job> = Vec::new();
        for a in &actions {
            let (address, stale, connect) = match a {
                Action::Disconnect(address) => (*address, true, false),
                Action::Connect(address) => (*address, false, true),
            };
            match jobs.iter_mut().find(|j| j.address == address) {
                Some(job) => {
                    job.drop_stale_entry |= stale;
                    job.connect |= connect;
                }
                None => jobs.push(Job { address, drop_stale_entry: stale, connect, via_probe: None }),
            }
        }
        for job in jobs {
            self.start(job);
        }

        // The paired devices that announce nothing: the address they were last connected at.
        let planned = fallback::plan_fallback(
            known,
            &entries,
            &mdns,
            &self.props,
            &self.fallback.lock().unwrap_or_else(PoisonError::into_inner),
            Instant::now(),
            fallback::MAX_PER_ROUND,
        );
        for candidate in planned {
            self.start(Job {
                address: candidate.address,
                drop_stale_entry: candidate.drop_stale_entry,
                connect: true,
                via_probe: Some(candidate.id),
            });
        }

        let seen = entries
            .iter()
            .filter(|e| e.state == AdbState::Device)
            .map(|e| {
                let id = entry_id(e, &self.props, &mdns);
                let p = self.props.get(&e.serial);
                Seen {
                    model: p
                        .map(|p| p.model.clone())
                        .filter(|m| !m.is_empty())
                        .or_else(|| e.model.clone())
                        .unwrap_or_else(|| id.clone()),
                    android: p.map(|p| p.android.clone()).filter(|s| !s.is_empty()),
                    sdk: p.and_then(|p| p.sdk),
                    address: entry_address(e, &id, &mdns),
                    id,
                }
            })
            .collect();

        // The connections go on in the background; each wakes the loop when it ends, and the list is read again then.
        Ok(PollResult {
            devices: build_devices(&entries, &mdns, &self.props, known),
            pairing: pairing_services(&mdns),
            seen,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::adb::{parse_devices, parse_mdns};
    use crate::core::adb::api::{AdbError, DeviceTracker, ShellOutput};
    use std::sync::Mutex;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    fn known(id: &str) -> KnownDevice {
        KnownDevice {
            id: id.into(),
            model: "Pixel 7".into(),
            label: None,
            android: Some("14".into()),
            sdk: Some(34),
            last_address: Some("192.168.1.5:40000".into()),
            last_seen: 0,
            first_seen: 0,
            settings: Default::default(),
            apps: Default::default(),
        }
    }

    fn props(serial: &str) -> Props {
        Props {
            serial: serial.into(),
            model: "Pixel 7".into(),
            android: "14".into(),
            sdk: Some(34),
        }
    }

    #[test]
    fn usb_and_wifi_of_same_device_merge_into_one_card() {
        let entries = parse_devices(
            "ABC123 device usb:1-1 model:Pixel_7\n192.168.1.5:40000 device model:Pixel_7\n",
        );
        let mut p = PropsMap::new();
        p.insert("ABC123".into(), props("ABC123"));
        p.insert("192.168.1.5:40000".into(), props("ABC123"));
        let d = build_devices(&entries, &[], &p, &[]);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].transport, Some(Transport::Usb));
        assert_eq!(d[0].adb_serial.as_deref(), Some("ABC123"));
    }

    #[test]
    fn ip_change_keeps_the_same_device_id() {
        let known = [known("ABC123")];
        let mut p = PropsMap::new();
        p.insert("192.168.1.77:41111".into(), props("ABC123"));
        let entries = parse_devices("192.168.1.77:41111 device model:Pixel_7\n");
        let d = build_devices(&entries, &[], &p, &known);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].id, "ABC123");
        assert_eq!(d[0].address.as_deref(), Some("192.168.1.77:41111"));
    }

    #[test]
    fn known_but_unseen_is_listed_as_disconnected() {
        let d = build_devices(&[], &[], &PropsMap::new(), &[known("ABC123")]);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].state, DeviceState::Disconnected);
    }

    #[test]
    fn discovered_device_is_listed_with_its_address() {
        let mdns = parse_mdns("adb-XYZ\t_adb._tcp.\t10.0.0.9:5555\n");
        let d = build_devices(&[], &mdns, &PropsMap::new(), &[]);
        assert_eq!(d[0].state, DeviceState::Discovered);
        assert_eq!(d[0].address.as_deref(), Some("10.0.0.9:5555"));
    }

    #[test]
    fn known_device_at_new_address_is_reconnected_and_throttled() {
        let known = [known("ABC123")];
        let mdns = parse_mdns("adb-ABC123-aaaaaa\t_adb-tls-connect._tcp.\t192.168.1.77:41111\n");
        let entries = parse_devices("192.168.1.5:40000 offline\n");
        let mut attempts = Attempts::new();
        let t0 = Instant::now();
        let a = plan_actions(&entries, &mdns, &PropsMap::new(), &known, t0, &mut attempts);
        assert!(a.contains(&Action::Connect(addr("192.168.1.77:41111"))));
        // The stale entry at the old address is dropped too.
        assert!(a.contains(&Action::Disconnect(addr("192.168.1.5:40000"))));
        // Immediately afterwards nothing is retried.
        let again = plan_actions(&entries, &mdns, &PropsMap::new(), &known, t0, &mut attempts);
        assert!(!again.iter().any(|x| matches!(x, Action::Connect(_))));
    }

    #[test]
    fn strangers_on_the_network_are_never_connected_automatically() {
        // Think of a café: many devices advertise adb, none of them is ours.
        let mdns = parse_mdns(
            "adb-NEW1\t_adb._tcp.\t10.0.0.9:5555\n\
             adb-NEW2-abcdef\t_adb-tls-connect._tcp.\t10.0.0.10:40000\n",
        );
        let a = plan_actions(&[], &mdns, &PropsMap::new(), &[], Instant::now(), &mut Attempts::new());
        assert!(a.is_empty());
        // ...but they are still listed so the user can connect by hand.
        let d = build_devices(&[], &mdns, &PropsMap::new(), &[]);
        assert_eq!(d.len(), 2);
        assert!(d.iter().all(|d| d.state == DeviceState::Discovered && !d.known));
    }

    #[test]
    fn most_recently_used_device_is_connected_first() {
        let mut older = known("OLD");
        older.last_seen = 100;
        let mut newer = known("NEW");
        newer.last_seen = 900;
        // The older device is listed first by mDNS on purpose.
        let mdns = parse_mdns(
            "adb-OLD-aaaaaa\t_adb-tls-connect._tcp.\t10.0.0.1:40001\n\
             adb-NEW-bbbbbb\t_adb-tls-connect._tcp.\t10.0.0.2:40002\n",
        );
        let a = plan_actions(
            &[],
            &mdns,
            &PropsMap::new(),
            &[older, newer],
            Instant::now(),
            &mut Attempts::new(),
        );
        assert_eq!(
            a,
            vec![
                Action::Connect(addr("10.0.0.2:40002")),
                Action::Connect(addr("10.0.0.1:40001")),
            ]
        );
    }

    #[test]
    fn device_waiting_for_approval_is_not_prompted_again() {
        let known = [known("ABC123")];
        let mdns = parse_mdns("adb-ABC123\t_adb._tcp.\t10.0.0.1:5555\n");
        let entries = parse_devices("10.0.0.1:5555 unauthorized\n");
        let a = plan_actions(&entries, &mdns, &PropsMap::new(), &known, Instant::now(), &mut Attempts::new());
        assert!(a.is_empty());
    }

    #[test]
    fn connect_outcome_is_judged_from_facts_not_from_adb_text() {
        assert!(classify_connect(Some(AdbState::Device), false).is_ok());
        // Connected, waiting for the user's approval on the device.
        assert!(classify_connect(Some(AdbState::Unauthorized), false).is_ok());
        assert!(matches!(
            classify_connect(Some(AdbState::Offline), true),
            Err(CoreError::Unreachable)
        ));
        // Not listed: an open port means the device refused us, i.e. not paired.
        assert!(matches!(classify_connect(None, true), Err(CoreError::NotPaired)));
        assert!(matches!(classify_connect(None, false), Err(CoreError::Unreachable)));
    }

    #[test]
    fn finds_the_entry_for_an_address_in_both_naming_forms() {
        let mdns = parse_mdns("adb-T1-abcdef\t_adb-tls-connect._tcp.\t10.0.0.5:34400\n");
        let target = addr("10.0.0.5:34400");
        let by_ip = parse_devices("10.0.0.5:34400 device\n");
        assert!(entry_for_address(&by_ip, &mdns, target).is_some());
        let by_name = parse_devices("adb-T1-abcdef._adb-tls-connect._tcp device\n");
        assert!(entry_for_address(&by_name, &mdns, target).is_some());
        assert!(entry_for_address(&by_name, &mdns, addr("10.0.0.9:1111")).is_none());
    }

    #[test]
    fn unauthorized_device_and_its_mdns_service_are_one_card() {
        // Regression: the device cannot be queried before it approves this
        // computer, so it used to appear twice with the same IP.
        let entries = parse_devices("192.168.250.41:5555 unauthorized transport_id:2\n");
        let mdns = parse_mdns("adb-TESTSERIAL001\t_adb._tcp.\t192.168.250.41:5555\n");
        let d = build_devices(&entries, &mdns, &PropsMap::new(), &[]);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].id, "TESTSERIAL001");
        assert_eq!(d[0].state, DeviceState::Unauthorized);
        assert_eq!(d[0].address.as_deref(), Some("192.168.250.41:5555"));
    }

    #[test]
    fn online_device_is_not_reconnected() {
        let known = [known("ABC123")];
        let mdns = parse_mdns("adb-ABC123-aaaaaa\t_adb-tls-connect._tcp.\t192.168.1.5:40000\n");
        let entries = parse_devices("ABC123 device usb:1-1\n");
        let mut p = PropsMap::new();
        p.insert("ABC123".into(), props("ABC123"));
        let a = plan_actions(&entries, &mdns, &p, &known, Instant::now(), &mut Attempts::new());
        assert!(a.is_empty());
    }

    /// Fake adb that records the calls it receives.
    struct FakeAdb {
        calls: Mutex<Vec<String>>,
    }

    /// The monitor does not track; the fake offers no pushes.
    struct NoTracker;

    impl DeviceTracker for NoTracker {
        async fn next(&mut self) -> Result<Vec<AdbEntry>, AdbError> {
            Err(AdbError::Closed)
        }
    }

    impl AdbApi for FakeAdb {
        type Tracker = NoTracker;
        async fn track_devices(&self) -> Result<NoTracker, AdbError> {
            Err(AdbError::Closed)
        }
        async fn devices(&self) -> Result<Vec<AdbEntry>, AdbError> {
            self.calls.lock().unwrap().push("devices".into());
            Ok(Vec::new())
        }
        async fn forwards(&self) -> Result<Vec<crate::core::adb::Forward>, AdbError> {
            Ok(Vec::new())
        }
        async fn mdns_services(&self) -> Result<Vec<MdnsService>, AdbError> {
            self.calls.lock().unwrap().push("mdns".into());
            Ok(parse_mdns("adb-ABC123-aaaaaa\t_adb-tls-connect._tcp.\t192.168.1.77:41111\n"))
        }
        async fn shell(&self, serial: &str, command: &str) -> Result<ShellOutput, AdbError> {
            self.calls.lock().unwrap().push(format!("shell {serial} {command}"));
            Ok(ShellOutput::default())
        }
        async fn connect(&self, address: SocketAddr) -> Result<String, AdbError> {
            self.calls.lock().unwrap().push(format!("connect {address}"));
            Ok(String::new())
        }
        async fn disconnect(&self, address: SocketAddr) -> Result<String, AdbError> {
            self.calls.lock().unwrap().push(format!("disconnect {address}"));
            Ok(String::new())
        }
        async fn pair(&self, address: SocketAddr, code: &str) -> Result<String, AdbError> {
            self.calls.lock().unwrap().push(format!("pair {address} {code}"));
            Ok(String::new())
        }
    }

    #[tokio::test]
    async fn what_the_monitor_does_on_its_own_is_written_to_the_debug_log() {
        use crate::core::log::{LogCode, LogLevel, Logger};
        let logger = Logger::new(50);
        let mut m = Monitor::new(FakeAdb { calls: Mutex::new(Vec::new()) }).with_logger(logger.clone());
        m.poll_once(&[known("ABC123")]).await.unwrap();
        m.settle().await;
        let lines = logger.history();
        let summary: Vec<_> = lines.iter().map(|l| (l.level, l.code)).collect();
        // One connect to the known device that mDNS found.
        assert_eq!(summary, [(LogLevel::Info, LogCode::AutoConnect)]);
        // A stranger's address would not be here: only the connection made.
        assert_eq!(m.adb().calls.lock().unwrap().iter().filter(|c| c.starts_with("connect ")).count(), 1);
    }

    #[tokio::test]
    async fn monitor_connects_a_known_device_found_by_mdns() {
        let mut m = Monitor::new(FakeAdb { calls: Mutex::new(Vec::new()) });
        let r = m.poll_once(&[known("ABC123")]).await.unwrap();
        m.settle().await;
        assert!(m
            .adb()
            .calls
            .lock()
            .unwrap()
            .contains(&"connect 192.168.1.77:41111".to_string()));
        assert_eq!(r.devices.len(), 1);
    }

    // ---- the connection at the last address of a paired device, without mDNS (addresses are invented; the
    // fakes say what adb and the network answer, nothing is connected to)

    /// An adb that knows which addresses are connected, answers no mDNS (or the text it was given) and can be made to
    /// never answer a connection for one address.
    struct ScriptedAdb {
        calls: Mutex<Vec<String>>,
        connected: Mutex<Vec<SocketAddr>>,
        mdns: String,
        /// A connection to this IP address never ends.
        hangs_at: Option<std::net::IpAddr>,
        /// Whether a connection that ends brings the device into the list.
        connects: bool,
    }

    impl ScriptedAdb {
        fn new() -> Self {
            ScriptedAdb { calls: Mutex::new(Vec::new()), connected: Mutex::new(Vec::new()), mdns: String::new(), hangs_at: None, connects: true }
        }
        fn called(&self, call: &str) -> usize {
            self.calls.lock().unwrap().iter().filter(|c| c.as_str() == call).count()
        }
    }

    impl AdbApi for ScriptedAdb {
        type Tracker = NoTracker;
        async fn track_devices(&self) -> Result<NoTracker, AdbError> {
            Err(AdbError::Closed)
        }
        async fn devices(&self) -> Result<Vec<AdbEntry>, AdbError> {
            let listed: String = self.connected.lock().unwrap().iter().map(|a| format!("{a} device\n")).collect();
            Ok(parse_devices(&listed))
        }
        async fn forwards(&self) -> Result<Vec<crate::core::adb::Forward>, AdbError> {
            Ok(Vec::new())
        }
        async fn mdns_services(&self) -> Result<Vec<MdnsService>, AdbError> {
            Ok(parse_mdns(&self.mdns))
        }
        async fn shell(&self, _serial: &str, _command: &str) -> Result<ShellOutput, AdbError> {
            Ok(ShellOutput::default())
        }
        async fn connect(&self, address: SocketAddr) -> Result<String, AdbError> {
            self.calls.lock().unwrap().push(format!("connect {address}"));
            if self.hangs_at == Some(address.ip()) {
                std::future::pending::<()>().await;
            }
            if self.connects {
                self.connected.lock().unwrap().push(address);
            }
            Ok(String::new())
        }
        async fn disconnect(&self, address: SocketAddr) -> Result<String, AdbError> {
            self.calls.lock().unwrap().push(format!("disconnect {address}"));
            self.connected.lock().unwrap().retain(|a| *a != address);
            Ok(String::new())
        }
        async fn pair(&self, _address: SocketAddr, _code: &str) -> Result<String, AdbError> {
            Ok(String::new())
        }
    }

    /// A network where the ports in `open` answer; it counts what it is asked.
    struct FakeProbe {
        open: Vec<SocketAddr>,
        asked: Mutex<Vec<SocketAddr>>,
    }

    impl FakeProbe {
        fn answering(open: &[&str]) -> Arc<FakeProbe> {
            Arc::new(FakeProbe { open: open.iter().map(|a| addr(a)).collect(), asked: Mutex::new(Vec::new()) })
        }
        fn asked(&self) -> Vec<SocketAddr> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl probe::PortProbe for FakeProbe {
        fn is_open(&self, address: SocketAddr) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + '_>> {
            self.asked.lock().unwrap().push(address);
            let open = self.open.contains(&address);
            Box::pin(async move { open })
        }
    }

    fn paired(id: &str, address: &str, last_seen: u64) -> KnownDevice {
        KnownDevice { last_address: Some(address.into()), last_seen, ..known(id) }
    }

    fn monitor(adb: ScriptedAdb, probe: &Arc<FakeProbe>) -> Monitor<ScriptedAdb> {
        Monitor::new(adb).with_probe(probe.clone()).with_connect_limit(Duration::from_millis(600))
    }

    #[tokio::test]
    async fn a_paired_device_that_does_not_announce_itself_is_connected_at_its_last_address_when_the_port_answers() {
        let probe = FakeProbe::answering(&["192.0.2.10:40000"]);
        let mut m = monitor(ScriptedAdb::new(), &probe);
        let r = m.poll_once(&[paired("AAA", "192.0.2.10:40000", 5)]).await.unwrap();
        m.settle().await;
        assert_eq!(probe.asked(), [addr("192.0.2.10:40000")], "the port is asked before adb is");
        assert_eq!(m.adb().called("connect 192.0.2.10:40000"), 1);
        // Right after, the list is read again: the device is in it.
        assert!(r.devices.iter().any(|d| d.state == DeviceState::Online) || m.adb().connected.lock().unwrap().len() == 1);
    }

    #[tokio::test]
    async fn a_port_that_does_not_answer_is_not_connected_to_and_not_asked_again_at_once() {
        let probe = FakeProbe::answering(&[]);
        let mut m = monitor(ScriptedAdb::new(), &probe);
        let paired = [paired("AAA", "192.0.2.10:40000", 5)];
        m.poll_once(&paired).await.unwrap();
        m.settle().await;
        assert_eq!(probe.asked().len(), 1, "the port is asked once");
        assert_eq!(m.adb().called("connect 192.0.2.10:40000"), 0, "adb is not asked to connect to a port that is closed");
        m.poll_once(&paired).await.unwrap();
        m.settle().await;
        assert_eq!(probe.asked().len(), 1, "it is left alone for its wait: the port is not asked again at once");
    }

    #[tokio::test]
    async fn a_connection_that_does_not_bring_the_device_back_is_not_tried_again_at_once() {
        let probe = FakeProbe::answering(&["192.0.2.10:40000"]);
        let mut adb = ScriptedAdb::new();
        adb.connects = false;
        let mut m = monitor(adb, &probe);
        let paired = [paired("AAA", "192.0.2.10:40000", 5)];
        m.poll_once(&paired).await.unwrap();
        m.settle().await;
        assert_eq!(m.adb().called("connect 192.0.2.10:40000"), 1);
        m.poll_once(&paired).await.unwrap();
        m.settle().await;
        assert_eq!(m.adb().called("connect 192.0.2.10:40000"), 1, "the second try waits for the wait after the failure");
    }

    #[tokio::test]
    async fn a_device_that_is_being_connected_to_is_skipped_and_it_is_not_a_failure() {
        let probe = FakeProbe::answering(&["192.0.2.10:40000"]);
        let gate = gate::ConnectGate::new();
        let mut m = monitor(ScriptedAdb::new(), &probe).with_gate(gate.clone());
        let paired = [paired("AAA", "192.0.2.10:40000", 5)];
        let held = gate.try_begin(addr("192.0.2.10:45678")).expect("a permit");
        m.poll_once(&paired).await.unwrap();
        m.settle().await;
        assert_eq!(m.adb().called("connect 192.0.2.10:40000"), 0, "connected to a device that something else was connecting to");
        assert!(probe.asked().is_empty(), "the port of a busy device is not asked");
        drop(held);
        m.poll_once(&paired).await.unwrap();
        m.settle().await;
        assert_eq!(m.adb().called("connect 192.0.2.10:40000"), 1, "being busy counted as a failure: the device was left alone");
    }

    #[tokio::test]
    async fn a_device_that_does_not_answer_holds_up_neither_the_poll_nor_the_other_devices() {
        let probe = FakeProbe::answering(&["192.0.2.10:40000", "192.0.2.11:40000"]);
        let mut adb = ScriptedAdb::new();
        adb.hangs_at = Some("192.0.2.10".parse().unwrap());
        let gate = gate::ConnectGate::new();
        let mut m = monitor(adb, &probe).with_gate(gate.clone());
        let paired = [paired("AAA", "192.0.2.10:40000", 9), paired("BBB", "192.0.2.11:40000", 5)];
        let began = std::time::Instant::now();
        m.poll_once(&paired).await.unwrap();
        assert!(began.elapsed() < Duration::from_millis(300), "the poll waited {:?} for a device", began.elapsed());
        // The other device is connected while the first is still hanging.
        for _ in 0..100 {
            if m.adb().called("connect 192.0.2.11:40000") == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(m.adb().called("connect 192.0.2.11:40000"), 1);
        assert!(gate.is_busy("192.0.2.10".parse().unwrap()), "the hanging connection holds its device");
        // At the limit it is given up and the device is free.
        m.settle().await;
        assert!(!gate.is_busy("192.0.2.10".parse().unwrap()));
    }

    #[tokio::test]
    async fn a_device_that_announces_itself_is_connected_by_its_announcement_and_its_port_is_not_asked() {
        let probe = FakeProbe::answering(&["192.0.2.10:40000"]);
        let mut adb = ScriptedAdb::new();
        adb.mdns = "adb-AAA-x1y2z3\t_adb-tls-connect._tcp.\t192.0.2.10:41000\n".into();
        let mut m = monitor(adb, &probe);
        m.poll_once(&[paired("AAA", "192.0.2.10:40000", 5)]).await.unwrap();
        m.settle().await;
        assert_eq!(m.adb().called("connect 192.0.2.10:41000"), 1, "the announced port is the one connected to");
        assert_eq!(m.adb().called("connect 192.0.2.10:40000"), 0);
        assert!(probe.asked().is_empty());
    }
}
