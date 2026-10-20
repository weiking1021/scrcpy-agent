//! What every service of the core shares: the data folder, the log, the event
//! sink, the state the UI is shown, and the way adb is talked to.
//!
//! A service owns what only it needs (its registries, its locks, its loops) and
//! asks this for the rest. Nothing here knows what a service does with it.

use super::addons::events::{AddonEvent, opened_event, snapshot_events, transfer_event};
use super::adb::select::ConfiguredAdb;
use super::discovery::PollResult;
use super::error::CoreError;
use super::log::Logger;
use super::model::*;
use super::scrcpy::manager::Installed;
use super::scrcpy::protocol::{ScrcpyProtocol, protocol_for};
use super::scrcpy::version::ScrcpyVersion;
use super::store::{self, Store};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

/// Where the UI and the add-ons are told what happens.
pub type Sink = Arc<dyn Fn(Event) + Send + Sync>;

/// How many lines of the debug log are kept.
const LOG_CAPACITY: usize = 2000;

/// Where the adb server listens unless told otherwise.
const DEFAULT_ADB_SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 5037);

/// Ways to run the core differently from the normal start-up. The defaults
/// are what the application uses; the integration tests change them.
#[derive(Default)]
pub struct CoreOptions {
    /// Use this scrcpy install instead of looking one up and downloading it.
    pub preinstalled: Option<Installed>,
    /// Do not start our own mDNS browser; only adb's list is used.
    pub disable_own_mdns: bool,
    /// Talk to the adb server at this address instead of the usual
    /// `127.0.0.1:5037`.
    pub adb_server: Option<SocketAddr>,
    /// How long the device loop waits before looking again when nothing woke
    /// it. Normally two seconds.
    pub poll_interval: Option<Duration>,
}

/// The state that the UI is shown. Reached through `Ctx::with_state`, so the
/// lock is held only while a service reads or changes it.
pub(crate) struct State {
    pub scrcpy: ScrcpyStatus,
    /// The release that is wanted while it is not installed (see `Snapshot::scrcpy_wanted`).
    pub scrcpy_wanted: Option<String>,
    pub devices: Vec<Device>,
    pub pairing: Vec<PairingService>,
    pub sessions: BTreeSet<String>,
    pub settings: Settings,
    pub known: Vec<KnownDevice>,
    pub installed: Option<Installed>,
    pub last_emitted: Option<Snapshot>,
}

pub(crate) struct Ctx {
    pub options: CoreOptions,
    pub store: Store,
    pub sink: Sink,
    /// The debug log; the newest lines are kept, and shown live while the
    /// debug console is open.
    pub logger: Logger,
    /// Wakes the device loop: something changed that it should look at now.
    pub wake: Notify,
    /// How adb is talked to; shared with every `ConfiguredAdb`, so a change in
    /// the settings applies to the next call.
    adb_backend: Arc<Mutex<AdbBackend>>,
    state: Mutex<State>,
    /// Where what happens (a device, a screen, a file) is told to the add-ons; set by the add-on service.
    addon_events: Mutex<Option<AddonEventSink>>,
}

/// What is done with an event for the add-ons.
pub type AddonEventSink = Arc<dyn Fn(AddonEvent) + Send + Sync>;

impl Ctx {
    pub fn new(store: Store, sink: Sink, options: CoreOptions) -> Arc<Self> {
        let known = store.known_devices();
        let settings = store.global().load::<store::Settings>();
        let devices = super::discovery::build_devices(&[], &[], &Default::default(), &known);
        Arc::new(Ctx {
            adb_backend: Arc::new(Mutex::new(settings.adb_backend)),
            state: Mutex::new(State {
                scrcpy: ScrcpyStatus::Checking,
                scrcpy_wanted: None,
                devices,
                pairing: Vec::new(),
                sessions: BTreeSet::new(),
                settings,
                known,
                installed: None,
                last_emitted: None,
            }),
            wake: Notify::new(),
            addon_events: Mutex::new(None),
            options,
            store,
            sink,
            logger: Logger::new(LOG_CAPACITY),
        })
    }

    /// Reads or changes the shared state. The lock is held for the call only;
    /// the closure must not call back into the context.
    pub fn with_state<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.state.lock().unwrap())
    }

    // ---- telling the outside ----

    pub fn snapshot(&self) -> Snapshot {
        self.with_state(|i| Snapshot {
            scrcpy: i.scrcpy.clone(),
            scrcpy_wanted: i.scrcpy_wanted.clone(),
            devices: i.devices.clone(),
            pairing: i.pairing.clone(),
            sessions: i.sessions.iter().cloned().collect(),
            settings: i.settings.clone(),
            data_dir: self.store.root().display().to_string(),
        })
    }

    /// Says where the events for the add-ons go.
    pub fn set_addon_events(&self, sink: AddonEventSink) {
        *self.addon_events.lock().unwrap() = Some(sink);
    }

    /// Sends the state to the UI, but only when it changed.
    pub fn emit(&self) {
        let snap = self.snapshot();
        // `None` is that the state did not change; `Some(None)` that this is the first (nothing to compare with).
        let before = self.with_state(|i| {
            if i.last_emitted.as_ref() == Some(&snap) {
                return None;
            }
            Some(i.last_emitted.replace(snap.clone()))
        });
        let Some(before) = before else { return };
        (self.sink)(Event::State(Box::new(snap.clone())));
        if let Some(before) = before {
            for event in snapshot_events(&before, &snap) {
                self.tell_addons(event);
            }
        }
    }

    pub fn send(&self, event: Event) {
        let for_addons = match &event {
            Event::Transfer(update) => transfer_event(update),
            Event::Opened(update) => Some(opened_event(update)),
            _ => None,
        };
        (self.sink)(event);
        if let Some(event) = for_addons {
            self.tell_addons(event);
        }
    }

    /// Tells an event to the add-ons (if they are listening: the sink is set by the add-on service).
    fn tell_addons(&self, event: AddonEvent) {
        let sink = self.addon_events.lock().unwrap().clone();
        if let Some(sink) = sink {
            sink(event);
        }
    }

    pub fn notice(&self, level: NoticeLevel, code: NoticeCode, detail: impl Into<String>, device: Option<String>) {
        self.send(Event::Notice(Notice { level, code, detail: detail.into(), device }));
    }

    // ---- settings ----

    pub fn settings(&self) -> Settings {
        self.with_state(|i| i.settings.clone())
    }

    /// Makes `settings` the ones in use: the way of talking to adb changes at
    /// the next call, the UI is told, and the device loop looks again.
    pub fn use_settings(&self, settings: Settings) {
        *self.adb_backend.lock().unwrap() = settings.adb_backend;
        self.with_state(|i| i.settings = settings);
        self.emit();
        self.wake.notify_one();
    }

    // ---- adb and scrcpy ----

    /// Where the adb server listens.
    pub(crate) fn adb_server(&self) -> SocketAddr {
        self.options.adb_server.unwrap_or(DEFAULT_ADB_SERVER)
    }

    /// How the application talks to adb: the way the user's setting says, read
    /// at every call (see `adb::select`). Callers only see `AdbApi`.
    pub fn make_adb(&self, installed: &Installed) -> ConfiguredAdb {
        ConfiguredAdb::new(self.adb_backend.clone(), self.adb_server(), installed.adb())
    }

    /// The way to talk to adb. Only once scrcpy (and with it adb) is ready.
    pub fn adb(&self) -> Result<ConfiguredAdb, CoreError> {
        Ok(self.make_adb(&self.installed()?))
    }

    /// The installed scrcpy. Only once it is ready.
    pub fn installed(&self) -> Result<Installed, CoreError> {
        self.with_state(|i| i.installed.clone()).ok_or(CoreError::NotReady)
    }

    /// The installed scrcpy and the adapter of its version.
    pub fn app_protocol(&self) -> Result<(Installed, Box<dyn ScrcpyProtocol>), CoreError> {
        let install = self.installed()?;
        let protocol = ScrcpyVersion::parse(&install.version)
            .and_then(|v| protocol_for(&v))
            .ok_or(CoreError::UnsupportedScrcpy)?;
        Ok((install, protocol))
    }

    // ---- devices ----

    /// The device with this id, as it is shown.
    pub fn device(&self, device_id: &str) -> Option<Device> {
        self.with_state(|i| i.devices.iter().find(|d| d.id == device_id).cloned())
    }

    /// The online device with this id: its adb serial and the name it is shown with.
    pub fn online_device(&self, device_id: &str) -> Result<(String, String), CoreError> {
        self.with_state(|i| {
            let dev = i.devices.iter().find(|d| d.id == device_id).ok_or(CoreError::UnknownDevice)?;
            match (dev.state, &dev.adb_serial) {
                (DeviceState::Online, Some(s)) => Ok((s.clone(), dev.label.clone().unwrap_or_else(|| dev.name.clone()))),
                _ => Err(CoreError::NotOnline),
            }
        })
    }

    /// What is kept about this device, if it is known.
    pub fn known_device(&self, device_id: &str) -> Option<KnownDevice> {
        self.with_state(|i| i.known.iter().find(|k| k.id == device_id).cloned())
    }

    /// Changes what is kept about a known device and keeps it. `change` says
    /// whether it changed anything; nothing is written when it did not.
    pub fn update_known(&self, device_id: &str, change: impl FnOnce(&mut KnownDevice) -> bool) -> Result<(), CoreError> {
        self.with_state(|i| {
            let known = i.known.iter_mut().find(|k| k.id == device_id).ok_or(CoreError::UnknownDevice)?;
            if change(known) {
                self.store.save_known(known)?;
            }
            Ok(())
        })
    }

    /// Takes in a look at the devices: what is known about them, what is
    /// listed, and the pairing services. A name the user gave while the look
    /// was being made is not taken back.
    pub fn apply_poll(&self, r: PollResult, now: u64) {
        self.with_state(|i| {
            let mut dirty = false;
            for s in &r.seen {
                let address = s.address.map(|a| a.to_string());
                match i.known.iter_mut().find(|k| k.id == s.id) {
                    Some(k) => {
                        let changed = k.model != s.model
                            || (s.android.is_some() && k.android != s.android)
                            || (s.sdk.is_some() && k.sdk != s.sdk)
                            || (address.is_some() && k.last_address != address);
                        if changed || now.saturating_sub(k.last_seen) > 300 {
                            k.model = s.model.clone();
                            if s.android.is_some() {
                                k.android = s.android.clone();
                            }
                            if s.sdk.is_some() {
                                k.sdk = s.sdk;
                            }
                            if address.is_some() {
                                k.last_address = address;
                            }
                            k.last_seen = now;
                            dirty = true;
                        }
                    }
                    None => {
                        i.known.push(KnownDevice {
                            id: s.id.clone(),
                            model: s.model.clone(),
                            label: None,
                            android: s.android.clone(),
                            sdk: s.sdk,
                            last_address: address,
                            last_seen: now,
                            first_seen: now,
                            settings: Default::default(),
                            apps: Default::default(),
                        });
                        dirty = true;
                    }
                }
            }
            if dirty {
                let _ = i.known.iter().try_for_each(|k| self.store.save_known(k));
            }
            i.devices = r.devices;
            let known = std::mem::take(&mut i.known);
            for d in i.devices.iter_mut() {
                if let Some(k) = known.iter().find(|k| k.id == d.id) {
                    d.label = k.label.clone();
                }
            }
            i.known = known;
            i.pairing = r.pairing;
        });
        self.emit();
    }
}
