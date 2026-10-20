//! The platform-independent core. It does not depend on Tauri: the host
//! application hands in a data folder and an event sink, so other front ends
//! (Android, add-ons) can reuse it.
//!
//! `Core` only puts the parts together. Each part is a service that owns what
//! it needs and does one kind of thing; what they share is in `context`.
//!
//! * `devices()` — finding, pairing, connecting; what the user decides about a device
//! * `readings()` — the facts read from the devices that are usable
//! * `sessions()` — the screens that are open, and the toolbar's buttons
//! * `apps()` — the apps of a device
//! * `files()` — sending files to a device, and opening them there
//! * `requests()` — the requests of the file manager's menu
//! * `addons()` — the menu entries of add-ons
//! * `themes()` — the looks the windows can be drawn in

pub mod adb;
pub mod addons;
pub mod app_name;
pub mod apps;
pub mod context;
pub mod device;
pub mod discovery;
pub mod error;
pub mod files;
pub mod integration;
pub mod log;
pub mod model;
pub mod requests;
pub mod scrcpy;
pub mod store;
pub mod theme;
pub mod window;

pub use context::{CoreOptions, Sink};
pub use requests::service::ShellStart;
pub use scrcpy::service::OpenedWindow;

use addons::registry::AddonService;
use apps::service::AppService;
use context::Ctx;
use device::readings::ReadingService;
use discovery::service::DeviceService;
use error::CoreError;
use files::service::FileService;
use files::transfer;
use log::{LogCode, LogLevel};
use model::*;
use requests::service::RequestService;
use scrcpy::manager::{self as scrcpy_manager, Installed};
use scrcpy::release::Offer;
use scrcpy::versions::{self, ReleaseTag};
use scrcpy::service::SessionService;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use theme::service::ThemeService;

pub struct Core {
    ctx: Arc<Ctx>,
    devices: Arc<DeviceService>,
    readings: Arc<ReadingService>,
    sessions: Arc<SessionService>,
    apps: Arc<AppService>,
    files: Arc<FileService>,
    requests: Arc<RequestService>,
    addons: Arc<AddonService>,
    themes: Arc<ThemeService>,
    last_percent: AtomicU32,
    /// The user has decided about the download that the start waits for (see `obtain_scrcpy`).
    scrcpy_decided: tokio::sync::Notify,
    /// The loops of the devices, which are replaced when the release of scrcpy is changed.
    device_task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// The releases of scrcpy, for the settings.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ScrcpyVersions {
    /// The release the program is checked against.
    pub default: String,
    /// The release in use (none while the start waits for a download).
    pub current: Option<String>,
    /// The release that is wanted: the user's choice, else the default (it may not be installed).
    pub wanted: String,
    /// The releases that are installed, the newest first.
    pub installed: Vec<String>,
    /// The folder the releases are installed in (each in a folder of its tag).
    pub folder: String,
}

impl Core {
    pub fn new(data_dir: PathBuf, sink: Sink) -> Arc<Self> {
        Self::with_options(data_dir, sink, CoreOptions::default())
    }

    pub fn with_options(data_dir: PathBuf, sink: Sink, options: CoreOptions) -> Arc<Self> {
        let ctx = Ctx::new(store::Store::open(data_dir), sink, options);
        let sessions = SessionService::new(ctx.clone());
        let files = FileService::new(ctx.clone());
        let themes = ThemeService::new(ctx.clone());
        Arc::new(Core {
            devices: DeviceService::new(ctx.clone()),
            readings: ReadingService::new(ctx.clone()),
            apps: AppService::new(ctx.clone(), sessions.clone()),
            requests: RequestService::new(ctx.clone(), files.clone()),
            addons: AddonService::new(ctx.clone(), sessions.clone(), files.clone(), themes.clone()),
            themes,
            sessions,
            files,
            ctx,
            last_percent: AtomicU32::new(u32::MAX),
            scrcpy_decided: tokio::sync::Notify::new(),
            device_task: std::sync::Mutex::new(None),
        })
    }

    pub fn devices(&self) -> &Arc<DeviceService> {
        &self.devices
    }

    pub fn readings(&self) -> &Arc<ReadingService> {
        &self.readings
    }

    pub fn sessions(&self) -> &Arc<SessionService> {
        &self.sessions
    }

    pub fn apps(&self) -> &Arc<AppService> {
        &self.apps
    }

    pub fn files(&self) -> &Arc<FileService> {
        &self.files
    }

    pub fn requests(&self) -> &Arc<RequestService> {
        &self.requests
    }

    pub fn addons(&self) -> &Arc<AddonService> {
        &self.addons
    }

    pub fn themes(&self) -> &Arc<ThemeService> {
        &self.themes
    }

    /// The data folder: all that is kept between runs is asked of it (never a path).
    pub fn store(&self) -> &store::Store {
        &self.ctx.store
    }

    pub fn snapshot(&self) -> Snapshot {
        self.ctx.snapshot()
    }

    pub fn notice(&self, level: NoticeLevel, code: NoticeCode, detail: impl Into<String>, device: Option<String>) {
        self.ctx.notice(level, code, detail, device);
    }

    /// Opens the debug console: returns the kept lines of the log, and from
    /// now on every new line is sent as an event until it is closed.
    pub fn open_debug_console(&self) -> Vec<log::LogLine> {
        let sink = self.ctx.sink.clone();
        self.ctx.logger.attach(Arc::new(move |line| sink(Event::Log(line))))
    }

    /// Closes the debug console: lines are kept but no longer sent.
    pub fn close_debug_console(&self) {
        self.ctx.logger.detach();
    }

    pub fn save_settings(&self, mut settings: Settings) -> Result<(), CoreError> {
        // Kept in the form it is used in.
        let dir = transfer::RemoteDir::parse(&settings.send_directory).ok_or(CoreError::InvalidDirectory)?;
        settings.send_directory = if dir.as_str().is_empty() { "/".into() } else { dir.as_str().to_string() };
        self.ctx.store.global().save::<store::Settings>(&settings)?;
        self.ctx.logger.log(
            LogLevel::Info,
            LogCode::SettingsSaved,
            format!("settings saved (adb: {:?})", settings.adb_backend),
        );
        self.ctx.use_settings(settings);
        Ok(())
    }

    /// Takes back everything an add-on registered: its menu entries, its file types and its themes.
    pub fn remove_addon(&self, addon: &str) {
        self.addons.remove_addon(addon);
    }

    /// Boot sequence and the loops that keep the state current. Runs for the
    /// whole lifetime of the application.
    pub async fn run(self: Arc<Self>) {
        let installed = match self.ctx.options.preinstalled.clone() {
            Some(installed) => installed,
            None => self.obtain_scrcpy().await,
        };
        self.ctx.with_state(|i| {
            i.scrcpy = ScrcpyStatus::Ready { version: installed.version.clone() };
            i.installed = Some(installed.clone());
        });
        self.ctx.emit();

        tokio::spawn(self.readings.clone().run());
        self.start_devices(installed);
        // The loops that follow the devices are tasks of their own, which a change of the release replaces.
        std::future::pending::<()>().await;
    }

    /// At the end of the program: the loops stop, and the adb server is stopped if the program started it.
    pub async fn shutdown(&self) {
        self.stop_devices();
        if let Ok(installed) = self.ctx.installed() {
            self.devices.stop_adb_started(&installed).await;
        }
    }

    /// The loops of the devices, with the adb of this release (in place of the ones that run).
    fn start_devices(self: &Arc<Self>, installed: Installed) {
        let task = tokio::spawn(self.devices.clone().run(installed));
        if let Some(old) = self.device_task.lock().unwrap().replace(task) {
            old.abort();
        }
    }

    fn stop_devices(&self) {
        if let Some(task) = self.device_task.lock().unwrap().take() {
            task.abort();
        }
        self.devices.stop_tracking();
    }

    /// The release that is wanted: the user's choice, else the default.
    fn wanted_release(&self) -> ReleaseTag {
        self.ctx.settings().scrcpy_version.unwrap_or_else(ReleaseTag::default_tag)
    }

    fn set_wanted(&self, wanted: Option<String>) {
        self.ctx.with_state(|i| i.scrcpy_wanted = wanted);
        self.ctx.emit();
    }

    /// Tells whether the wanted release is installed (and so whether the user is to be asked).
    fn refresh_wanted(&self) {
        let wanted = self.wanted_release();
        let missing = scrcpy_manager::find_version(&self.ctx.store.scrcpy_dir(), wanted.as_str()).is_none();
        self.set_wanted(missing.then(|| wanted.as_str().to_string()));
    }

    /// Finds the scrcpy to start with. Nothing is downloaded here: when the release that is wanted is not
    /// installed the user is asked (`scrcpy_wanted`); if another is installed it serves meanwhile, and if
    /// none is, this waits until the user has downloaded one.
    async fn obtain_scrcpy(self: &Arc<Self>) -> Installed {
        loop {
            let wanted = self.wanted_release();
            let plan = versions::plan(&self.ctx.store.scrcpy_dir(), &wanted);
            self.set_wanted(plan.download_needed.then(|| wanted.as_str().to_string()));
            if let Some(installed) = plan.use_now {
                return installed;
            }
            self.set_scrcpy(ScrcpyStatus::NeedsDownload { version: wanted.as_str().to_string() });
            self.scrcpy_decided.notified().await;
        }
    }

    fn set_scrcpy(&self, status: ScrcpyStatus) {
        self.ctx.with_state(|i| i.scrcpy = status);
        self.ctx.emit();
    }

    /// The releases that are installed, the one in use and the default.
    pub fn scrcpy_versions(&self) -> ScrcpyVersions {
        ScrcpyVersions {
            default: ReleaseTag::default_tag().as_str().to_string(),
            current: self.ctx.with_state(|i| i.installed.as_ref().map(|i| i.version.clone())),
            wanted: self.wanted_release().as_str().to_string(),
            installed: scrcpy_manager::installed_versions(&self.ctx.store.scrcpy_dir())
                .into_iter()
                .map(|i| i.version)
                .collect(),
            folder: self.ctx.store.scrcpy_dir().display().to_string(),
        }
    }

    /// What there is to download of a release (asks GitHub; downloads nothing).
    pub async fn scrcpy_offer(&self, tag: &ReleaseTag) -> Result<Offer, CoreError> {
        scrcpy_manager::fetch_offer(tag).await.map_err(CoreError::ScrcpyRelease)
    }

    /// The newest releases of scrcpy (asks GitHub).
    pub async fn scrcpy_available(&self) -> Result<Vec<ReleaseTag>, CoreError> {
        scrcpy_manager::fetch_list().await.map_err(CoreError::ScrcpyRelease)
    }

    /// Whether the release in use cannot be changed now: it would stop the adb that the open screens use.
    fn screens_block_a_change(&self) -> bool {
        self.ctx.with_state(|i| i.installed.is_some() && !i.sessions.is_empty())
    }

    /// Downloads a release the user asked for (after being told what it is), then uses it.
    pub async fn download_scrcpy(self: &Arc<Self>, tag: ReleaseTag) -> Result<(), CoreError> {
        let base = self.ctx.store.scrcpy_dir();
        if scrcpy_manager::find_version(&base, tag.as_str()).is_none() {
            if self.screens_block_a_change() {
                return Err(CoreError::ScreensOpen);
            }
            let before = self.ctx.with_state(|i| i.scrcpy.clone());
            let offer = self.scrcpy_offer(&tag).await?;
            let me = self.clone();
            let progress = move |tag: &str, p: f32| me.on_progress(tag, p);
            let downloaded = scrcpy_manager::download(&base, &offer, &progress).await;
            self.set_scrcpy(before);
            downloaded.map_err(CoreError::ScrcpyRelease)?;
        }
        self.use_scrcpy(&tag).await
    }

    /// Makes an installed release the one in use, and keeps the choice. The adb server is stopped and started
    /// again with the adb of that release (one that runs keeps serving whatever else is started), so it is not
    /// done while screens are open.
    pub async fn use_scrcpy(self: &Arc<Self>, tag: &ReleaseTag) -> Result<(), CoreError> {
        let new = scrcpy_manager::find_version(&self.ctx.store.scrcpy_dir(), tag.as_str())
            .ok_or(CoreError::ScrcpyNotInstalled)?;
        let current = self.ctx.with_state(|i| i.installed.clone());
        if current.as_ref().is_some_and(|current| *current != new) && self.screens_block_a_change() {
            return Err(CoreError::ScreensOpen);
        }
        let mut settings = self.ctx.settings();
        // The default is not a choice: it follows the program when the program's default changes.
        settings.scrcpy_version = (*tag != ReleaseTag::default_tag()).then(|| tag.clone());
        self.save_settings(settings)?;
        match current {
            // Still starting: it was waiting for this.
            None => self.scrcpy_decided.notify_one(),
            Some(old) if old == new => self.refresh_wanted(),
            Some(old) => {
                self.stop_devices();
                if let Err(versions::SwitchError::ServerNotStarted(text)) =
                    versions::switch_adb(&old, &new, &adb::SystemAdb).await
                {
                    // The release in use goes on.
                    self.start_devices(old);
                    return Err(CoreError::AdbNotStarted(text));
                }
                // The server was stopped and started again here: the one that runs is the program's own now.
                self.devices.mark_adb_started();
                self.ctx.with_state(|i| {
                    i.scrcpy = ScrcpyStatus::Ready { version: new.version.clone() };
                    i.installed = Some(new.clone());
                });
                self.refresh_wanted();
                self.start_devices(new);
            }
        }
        Ok(())
    }

    fn on_progress(&self, tag: &str, p: f32) {
        let percent = (p * 100.0) as u32;
        if self.last_percent.swap(percent, Ordering::Relaxed) != percent {
            self.set_scrcpy(ScrcpyStatus::Downloading { version: tag.to_string(), progress: p });
        }
    }
}
