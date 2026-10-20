//! The devices: finding them, pairing and connecting, what the user decides
//! about each one (its name, what is set for it, forgetting it), and the loop
//! that keeps the list current.

use super::gate::{CONNECT_LIMIT, ConnectGate, GateError, run_gated};
use super::probe::{PortProbe, TcpProbe};
use super::{Monitor, classify_connect, entry_for_address, mdns};
use crate::core::adb::api::{AdbApi, AdbError, DeviceTracker};
use crate::core::adb::wire::{AdbClient, TcpConnect};
use crate::core::adb::{AdbState, MdnsKind, MdnsService, ProcessAdb, parse_user_address};
use crate::core::context::Ctx;
use crate::core::error::CoreError;
use crate::core::log::{LogCode, LogLevel};
use crate::core::model::{DeviceSettings, DeviceState, NoticeCode, NoticeLevel};
use crate::core::scrcpy::manager::Installed;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// How long to wait before asking the adb server to track devices again.
const TRACK_RETRY: Duration = Duration::from_secs(1);

/// How long to wait before asking again when the way of talking to adb in use
/// cannot be told of changes (asking costs nothing, but nothing changes soon).
const TRACK_RETRY_UNSUPPORTED: Duration = Duration::from_secs(5);

pub struct DeviceService {
    ctx: Arc<Ctx>,
    /// Shared with the device monitor; also used when pairing.
    browser: Mutex<Option<Arc<mdns::MdnsBrowser>>>,
    /// The devices have been looked at at least once.
    polled: AtomicBool,
    /// The task that follows the announcements of the adb server.
    tracker: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// The adb server that runs was started by this program (there was none), so it is stopped when the
    /// program ends. One that was there already is somebody else's (Android Studio, another scrcpy) and is left.
    started_adb: AtomicBool,
    /// Who is connecting to which device: shared by the loop of the devices and by what the user asks for.
    gate: Arc<ConnectGate>,
    /// How a port is asked whether it answers.
    probe: Arc<dyn PortProbe>,
}

impl DeviceService {
    pub(crate) fn new(ctx: Arc<Ctx>) -> Arc<Self> {
        Arc::new(DeviceService {
            gate: ConnectGate::new(),
            probe: Arc::new(TcpProbe::default()),
            ctx,
            browser: Mutex::new(None),
            polled: AtomicBool::new(false),
            tracker: Mutex::new(None),
            started_adb: AtomicBool::new(false),
        })
    }

    /// Who is connecting to which device now.
    pub fn connect_gate(&self) -> &Arc<ConnectGate> {
        &self.gate
    }

    /// Brings the adb server up with this adb. If none was answering, the one that now runs is the program's own.
    async fn bring_up_adb(&self, process: &ProcessAdb) -> anyhow::Result<()> {
        let answering = AdbClient::new(TcpConnect(self.ctx.adb_server())).version().await.is_ok();
        process.start_server().await?;
        if !answering {
            self.mark_adb_started();
        }
        Ok(())
    }

    /// Says that the adb server that runs is the program's own (it was stopped and started again by the program).
    pub(crate) fn mark_adb_started(&self) {
        self.started_adb.store(true, Ordering::Relaxed);
    }

    /// At the end of the program: stops the adb server if the program started it, with the adb of the release in use.
    pub(crate) async fn stop_adb_started(&self, installed: &Installed) {
        if self.started_adb.swap(false, Ordering::Relaxed) {
            let process = ProcessAdb { path: installed.adb() };
            let _ = tokio::time::timeout(Duration::from_secs(5), process.kill_server()).await;
        }
    }

    /// Stops following the adb server (the loop of `run` is stopped by whoever runs it). Used before the
    /// release of scrcpy, and with it the adb, is changed.
    pub(crate) fn stop_tracking(&self) {
        if let Some(tracker) = self.tracker.lock().unwrap().take() {
            tracker.abort();
        }
    }

    /// Brings the adb server up, then keeps the device list current for the
    /// whole lifetime of the application.
    pub(crate) async fn run(self: Arc<Self>, installed: Installed) {
        // The adb program only brings the server up; everything else is said
        // to the server itself.
        let process = ProcessAdb { path: installed.adb() };
        while let Err(e) = self.bring_up_adb(&process).await {
            self.ctx.notice(NoticeLevel::Error, NoticeCode::AdbStartFailed, format!("{e:#}"), None);
            tokio::time::sleep(Duration::from_secs(10)).await;
        }

        let me = self.clone();
        let tracking_adb = self.ctx.make_adb(&installed);
        self.stop_tracking();
        let tracker = tokio::spawn(async move {
            watch_device_list(&tracking_adb, &me.ctx.wake, TRACK_RETRY, TRACK_RETRY_UNSUPPORTED).await
        });
        *self.tracker.lock().unwrap() = Some(tracker);

        let ctx = self.ctx.clone();
        let mut monitor = Monitor::new(self.ctx.make_adb(&installed))
            .with_logger(self.ctx.logger.clone())
            .with_gate(self.gate.clone())
            .with_wake(Arc::new(move || ctx.wake.notify_one()));
        // The browser of an earlier run (before the release of scrcpy was changed) is the one that goes on.
        let browser = self.browser.lock().unwrap().clone();
        if let Some(browser) = browser {
            monitor = monitor.with_browser(browser);
        } else if !self.ctx.options.disable_own_mdns {
            match mdns::MdnsBrowser::start() {
                Ok(browser) => {
                    let browser = Arc::new(browser);
                    *self.browser.lock().unwrap() = Some(browser.clone());
                    monitor = monitor.with_browser(browser);
                }
                // Without it only adb's own (less complete) mDNS list is used.
                Err(e) => self.ctx.notice(NoticeLevel::Info, NoticeCode::MdnsUnavailable, format!("{e:#}"), None),
            }
        }
        loop {
            let known = self.ctx.with_state(|i| i.known.clone());
            match monitor.poll_once(&known).await {
                Ok(r) => {
                    self.ctx.apply_poll(r, unix_now());
                    self.polled.store(true, Ordering::Relaxed);
                }
                Err(e) => {
                    self.ctx.logger.log(
                        LogLevel::Warn,
                        LogCode::PollFailed,
                        format!("could not look at the devices: {e:#}"),
                    );
                    // The adb server may have been killed; bring it back.
                    let _ = self.bring_up_adb(&process).await;
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(self.ctx.options.poll_interval.unwrap_or(DEFAULT_POLL_INTERVAL)) => {}
                _ = self.ctx.wake.notified() => {}
            }
        }
    }

    /// Waits until the devices have been looked at (at most `limit`), so a request
    /// that comes in as the application starts is not answered with "no device".
    /// Whether it did not time out is the answer.
    pub async fn wait_until_devices_known(&self, limit: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + limit;
        while !self.polled.load(Ordering::Relaxed) {
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        true
    }

    /// mDNS services from our own browser merged with adb's list.
    async fn services(&self, adb: &impl AdbApi) -> Vec<MdnsService> {
        let from_adb = adb.mdns_services().await.unwrap_or_default();
        let browser = self.browser.lock().unwrap().clone();
        match browser {
            Some(b) => mdns::merge(b.snapshot(), from_adb),
            None => from_adb,
        }
    }

    /// The adb state of the connection to `address`, read from adb's device list.
    async fn state_of(
        &self,
        adb: &impl AdbApi,
        services: &[MdnsService],
        address: SocketAddr,
    ) -> Result<Option<AdbState>, CoreError> {
        let entries = adb.devices().await?;
        Ok(entry_for_address(&entries, services, address).map(|e| e.state))
    }

    pub async fn pair(&self, address: String, code: String) -> Result<(), CoreError> {
        let target = parse_user_address(&address).ok_or(CoreError::InvalidAddress)?;
        if !valid_pairing_code(&code) {
            return Err(CoreError::InvalidPairingCode);
        }
        // The code is a secret: it is not written to the log.
        self.ctx.logger.log(LogLevel::Info, LogCode::PairAttempt, format!("pairing with {target}"));
        let result = self.pair_with(target, &code).await;
        match &result {
            Ok(()) => self.ctx.logger.log(LogLevel::Info, LogCode::PairOutcome, "paired and connected"),
            Err(e) => self.ctx.logger.log(LogLevel::Warn, LogCode::PairOutcome, format!("not paired: {e}")),
        }
        result
    }

    async fn pair_with(&self, target: SocketAddr, code: &str) -> Result<(), CoreError> {
        let adb = self.ctx.adb()?;
        let message = adb.pair(target, code).await?;
        // After pairing the device still has to be connected, on a different
        // port than the pairing one. Success is decided by the device showing
        // up in adb's list; adb's message is only shown if that never happens.
        let mut connected = false;
        for _ in 0..8 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let services = self.services(&adb).await;
            let Some(m) = services.iter().find(|m| m.kind == MdnsKind::Connect && m.address.ip() == target.ip()) else {
                continue;
            };
            // Not while something else is connecting to this device, and not for longer than the limit.
            if run_gated(&self.gate, m.address, CONNECT_LIMIT, adb.connect(m.address)).await.is_err() {
                continue;
            }
            if self.state_of(&adb, &services, m.address).await?.is_some_and(|s| s != AdbState::Offline) {
                connected = true;
                break;
            }
        }
        self.ctx.wake.notify_one();
        if connected { Ok(()) } else { Err(CoreError::PairNotConfirmed(message)) }
    }

    pub async fn connect(&self, address: String) -> Result<(), CoreError> {
        let target = parse_user_address(&address).ok_or(CoreError::InvalidAddress)?;
        // Only one connection to a device at a time, and no more than the limit for it: a device that does not answer
        // is not asked twice, and does not keep the device out for good.
        let work = async {
            self.ctx.logger.log(LogLevel::Info, LogCode::ConnectAttempt, format!("connecting to {target}"));
            self.connect_to(target).await
        };
        let result = match run_gated(&self.gate, target, CONNECT_LIMIT, work).await {
            Ok(result) => result,
            Err(GateError::Busy) => return Err(CoreError::ConnectInProgress),
            Err(GateError::TimedOut) => Err(CoreError::Unreachable),
        };
        match &result {
            Ok(()) => self.ctx.logger.log(LogLevel::Info, LogCode::ConnectOutcome, "connected"),
            Err(e) => self.ctx.logger.log(LogLevel::Warn, LogCode::ConnectOutcome, format!("not connected: {e}")),
        }
        result
    }

    async fn connect_to(&self, target: SocketAddr) -> Result<(), CoreError> {
        let adb = self.ctx.adb()?;
        // Drop any stale or unauthorised entry first. Connecting to an address
        // that is already listed does nothing, so the phone would never show
        // its approval prompt again.
        let _ = adb.disconnect(target).await;
        // adb's reply text is deliberately ignored; the outcome is read from
        // its device list below.
        let _ = adb.connect(target).await;

        let services = self.services(&adb).await;
        let mut state = None;
        for _ in 0..5 {
            state = self.state_of(&adb, &services, target).await?;
            if state.is_some_and(|s| s != AdbState::Offline) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        let port_open = state.is_none() && self.probe.is_open(target).await;
        self.ctx.wake.notify_one();
        classify_connect(state, port_open)
    }

    /// Gives a device a name of the user's own, or takes it away (`None`).
    pub fn set_label(&self, id: &str, label: Option<&str>) -> Result<(), CoreError> {
        let label = match label.map(str::trim) {
            None => None,
            Some(l) if !l.is_empty() && l.chars().count() <= 64 && !l.chars().any(char::is_control) => {
                Some(l.to_string())
            }
            Some(_) => return Err(CoreError::InvalidLabel),
        };
        self.ctx.with_state(|i| {
            let known = i.known.iter_mut().find(|k| k.id == id).ok_or(CoreError::UnknownDevice)?;
            known.label = label.clone();
            self.ctx.store.save_known(known)?;
            for d in i.devices.iter_mut().filter(|d| d.id == id) {
                d.label = label.clone();
            }
            Ok::<(), CoreError>(())
        })?;
        self.ctx.emit();
        Ok(())
    }

    /// Forgets a device: what is kept about it goes, and it leaves the list
    /// unless it is connected.
    pub fn forget(&self, id: &str) -> Result<(), CoreError> {
        self.ctx.with_state(|i| {
            i.known.retain(|k| k.id != id);
            i.devices.retain(|d| !(d.id == id && d.state == DeviceState::Disconnected));
            // The folder of the device goes with everything kept in it (its apps, what is set for it).
            self.ctx.store.forget_device(id)
        })?;
        self.ctx.emit();
        self.ctx.wake.notify_one();
        Ok(())
    }

    /// What is set for this device alone (the defaults for one that is not known).
    pub fn device_settings(&self, device_id: &str) -> DeviceSettings {
        self.ctx.known_device(device_id).map(|k| k.settings).unwrap_or_default()
    }

    /// Keeps what is set for a device, which takes effect for the screens and
    /// windows opened afterwards.
    pub fn set_device_settings(&self, device_id: &str, settings: DeviceSettings) -> Result<(), CoreError> {
        self.ctx.update_known(device_id, |known| {
            known.settings = settings;
            true
        })
    }
}

/// Wakes the device loop whenever adb announces that its list changed, and
/// asks again after `retry` if the connection ends (adb was restarted). When
/// the way of talking to adb in use cannot announce changes at all, it asks
/// again only after `unsupported_retry`: the setting may be changed to a way
/// that can, and the loop relies on looking at intervals meanwhile.
async fn watch_device_list<A: AdbApi>(adb: &A, wake: &Notify, retry: Duration, unsupported_retry: Duration) {
    loop {
        let wait = match adb.track_devices().await {
            Ok(mut tracker) => {
                while tracker.next().await.is_ok() {
                    wake.notify_one();
                }
                retry
            }
            Err(AdbError::Unsupported) => unsupported_retry,
            Err(_) => retry,
        };
        tokio::time::sleep(wait).await;
    }
}

pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A wireless-debugging pairing code is exactly six decimal digits.
fn valid_pairing_code(code: &str) -> bool {
    code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::adb::api::ShellOutput;
    use crate::core::adb::{AdbEntry, Forward};

    /// An adb whose way of working is chosen by the test: it counts how often
    /// it is asked to track devices, and answers that with `error`.
    struct Untrackable {
        asked: std::sync::atomic::AtomicUsize,
        error: AdbError,
    }

    struct Never;
    impl DeviceTracker for Never {
        async fn next(&mut self) -> Result<Vec<AdbEntry>, AdbError> {
            Err(AdbError::Closed)
        }
    }

    impl AdbApi for Untrackable {
        type Tracker = Never;
        async fn track_devices(&self) -> Result<Never, AdbError> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            Err(self.error.clone())
        }
        async fn devices(&self) -> Result<Vec<AdbEntry>, AdbError> {
            Err(self.error.clone())
        }
        async fn forwards(&self) -> Result<Vec<Forward>, AdbError> {
            Err(self.error.clone())
        }
        async fn mdns_services(&self) -> Result<Vec<MdnsService>, AdbError> {
            Err(self.error.clone())
        }
        async fn shell(&self, _: &str, _: &str) -> Result<ShellOutput, AdbError> {
            Err(self.error.clone())
        }
        async fn connect(&self, _: SocketAddr) -> Result<String, AdbError> {
            Err(self.error.clone())
        }
        async fn disconnect(&self, _: SocketAddr) -> Result<String, AdbError> {
            Err(self.error.clone())
        }
        async fn pair(&self, _: SocketAddr, _: &str) -> Result<String, AdbError> {
            Err(self.error.clone())
        }
    }

    /// Runs the watcher for `for_how_long` and returns how often it asked adb
    /// to track devices. It never finishes by itself.
    async fn asks_within(error: AdbError, for_how_long: Duration) -> usize {
        let adb = Untrackable { asked: Default::default(), error };
        let wake = Notify::new();
        let watching = watch_device_list(&adb, &wake, Duration::from_millis(10), Duration::from_secs(60));
        let finished = tokio::time::timeout(for_how_long, watching).await;
        assert!(finished.is_err(), "the watcher gave up");
        adb.asked.load(Ordering::SeqCst)
    }

    #[tokio::test]
    async fn a_way_of_talking_to_adb_that_cannot_announce_changes_is_asked_only_rarely() {
        // Asked once; the next time is a minute away, not ten milliseconds.
        assert_eq!(asks_within(AdbError::Unsupported, Duration::from_millis(300)).await, 1);
    }

    #[tokio::test]
    async fn watching_asks_again_soon_after_a_failure_that_may_pass() {
        let asked = asks_within(AdbError::Io(std::io::ErrorKind::ConnectionRefused), Duration::from_millis(300)).await;
        assert!(asked >= 3, "asked {asked} times");
    }

    #[test]
    fn pairing_codes_are_six_ascii_digits() {
        assert!(valid_pairing_code("123456"));
        assert!(!valid_pairing_code("12345"));
        assert!(!valid_pairing_code("1234567"));
        assert!(!valid_pairing_code("12345a"));
        // Six characters, but not ASCII digits.
        assert!(!valid_pairing_code("\u{ff11}\u{ff12}\u{ff13}\u{ff14}\u{ff15}\u{ff16}"));
    }
}
