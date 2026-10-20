//! The add-ons that are running: one `AddonRuntime` for each installed add-on that can run, started when
//! the program starts or the add-on is installed, restarted when the user decides its permissions again, and
//! stopped when it is removed. What each add-on is doing now (running, why it did not start, what it tried
//! that it had no permission for) is told in the list.
//!
//! `CoreHost` is what a running add-on asks things of: the program's log, menus and devices.

use super::engine::host::{AddonHost, AddonLogLevel, HostFuture};
use super::events::AddonEvent;
use super::engine::limits::Limits;
use super::engine::runtime::{AddonRuntime, StartError};
use super::library::{AddonInfo, AddonStatus, MAIN_SCRIPT};
use super::menu::{MenuHandler, MenuRegisterError};
use super::permission::{AddonPermission, Granted};
use super::registry::AddonService;
use crate::core::adb::api::AdbApi;
use crate::core::context::Ctx;
use crate::core::error::CoreError;
use crate::core::log::{LogCode, LogLevel};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

/// The largest `main.js` the program reads (a bundle of a project may be large).
pub const MAX_SCRIPT_BYTES: u64 = 4 * 1024 * 1024;
/// The most output of a command on a device that an add-on is given.
pub const MAX_SHELL_OUTPUT_BYTES: usize = 1024 * 1024;

/// Why an add-on did not start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StartFailure {
    /// `main.js` did not parse or threw (the text is the script's own, for showing only).
    Script(String),
    /// `main.js` ran longer than its limit of time.
    Timeout,
    /// `main.js` could not be read, or is larger than the program reads.
    Unreadable,
    /// The thread could not be made (the text is the system's, for showing only).
    Thread(String),
}

impl From<StartError> for StartFailure {
    fn from(error: StartError) -> Self {
        match error {
            StartError::Script(text) => StartFailure::Script(text),
            StartError::Timeout => StartFailure::Timeout,
            StartError::Thread(text) => StartFailure::Thread(text),
        }
    }
}

/// What an installed add-on is doing now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Running,
    /// It was not started: it is not ready (see `AddonStatus`), or the program has not started it yet.
    NotRunning,
    /// The user switched it off: it is installed, with what it was given, and does not run.
    Disabled,
    Failed(StartFailure),
}

/// An installed add-on for the list, with what it is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstalledAddon {
    #[serde(flatten)]
    pub info: AddonInfo,
    pub run: RunState,
    /// What it tried to use since it started without having the permission.
    pub denied: Vec<AddonPermission>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The running add-ons and what became of each.
#[derive(Default)]
pub struct Runner {
    running: Mutex<HashMap<String, AddonRuntime>>,
    failed: Mutex<HashMap<String, StartFailure>>,
    denied: Mutex<HashMap<String, Vec<AddonPermission>>>,
}

impl Runner {
    pub fn state_of(&self, id: &str) -> RunState {
        if locked(&self.running).contains_key(id) {
            RunState::Running
        } else if let Some(failure) = locked(&self.failed).get(id) {
            RunState::Failed(failure.clone())
        } else {
            RunState::NotRunning
        }
    }

    pub fn denied_of(&self, id: &str) -> Vec<AddonPermission> {
        locked(&self.denied).get(id).cloned().unwrap_or_default()
    }

    /// Tells every add-on that runs of an event.
    pub fn emit_all(&self, event: &AddonEvent) {
        for runtime in locked(&self.running).values() {
            runtime.emit(event.clone());
        }
    }

    /// Tells one add-on of an event.
    pub fn emit_to(&self, id: &str, event: AddonEvent) {
        if let Some(runtime) = locked(&self.running).get(id) {
            runtime.emit(event);
        }
    }

    /// Writes down that an add-on called something it had no permission for (once for each permission).
    pub fn record_denied(&self, id: &str, permission: AddonPermission) {
        let mut denied = locked(&self.denied);
        let list = denied.entry(id.to_string()).or_default();
        if !list.contains(&permission) {
            list.push(permission);
        }
    }

    /// Stops the add-on (the thread has ended when this returns) and forgets what was told of it.
    pub fn stop(&self, id: &str) {
        let runtime = locked(&self.running).remove(id);
        locked(&self.failed).remove(id);
        locked(&self.denied).remove(id);
        // Dropped here, after the locks are free: stopping waits for the thread, which may ask for them.
        drop(runtime);
    }

    /// Starts the add-on if it can run, in place of the one that runs (if it does). Returns when its
    /// `main.js` has run to its end (or failed).
    pub fn start(&self, info: &AddonInfo, host: Arc<dyn AddonHost>, limits: Limits) -> RunState {
        self.stop(&info.id);
        if !info.enabled {
            return RunState::Disabled;
        }
        if info.status != AddonStatus::Ready || info.manifest.is_none() {
            return RunState::NotRunning;
        }
        let failed = |failure: StartFailure| {
            locked(&self.failed).insert(info.id.clone(), failure.clone());
            RunState::Failed(failure)
        };
        let script = info.folder.join(MAIN_SCRIPT);
        let readable = std::fs::metadata(&script).is_ok_and(|m| m.is_file() && m.len() <= MAX_SCRIPT_BYTES);
        let Some(text) = readable.then(|| std::fs::read_to_string(&script).ok()).flatten() else {
            return failed(StartFailure::Unreadable);
        };
        match AddonRuntime::start(&info.id, &text, Granted::new(&info.granted), host, limits) {
            Ok(runtime) => {
                locked(&self.running).insert(info.id.clone(), runtime);
                RunState::Running
            }
            Err(error) => failed(error.into()),
        }
    }
}

/// What the add-ons ask of the program: its log, its menus and its devices.
pub(crate) struct CoreHost {
    pub(crate) service: Weak<AddonService>,
    pub(crate) ctx: Arc<Ctx>,
}

fn log_level(level: AddonLogLevel) -> LogLevel {
    match level {
        AddonLogLevel::Info => LogLevel::Info,
        AddonLogLevel::Warn => LogLevel::Warn,
        AddonLogLevel::Error => LogLevel::Error,
    }
}

impl AddonHost for CoreHost {
    fn log(&self, addon: &str, level: AddonLogLevel, text: &str) {
        self.ctx.logger.log(log_level(level), LogCode::Addon, format!("[{addon}] {text}"));
    }

    fn register_device_menu_item(
        &self,
        addon: &str,
        id: &str,
        label: &str,
        handler: MenuHandler,
    ) -> Result<(), MenuRegisterError> {
        match self.service.upgrade() {
            Some(service) => service.register_menu_item(addon, id, label, handler).map(|_| ()),
            None => Err(MenuRegisterError::InvalidId),
        }
    }

    fn register_app_menu_item(
        &self,
        addon: &str,
        id: &str,
        label: &str,
        handler: super::menu::AppMenuHandler,
    ) -> Result<(), MenuRegisterError> {
        match self.service.upgrade() {
            Some(service) => service.register_app_menu_item(addon, id, label, handler).map(|_| ()),
            None => Err(MenuRegisterError::InvalidId),
        }
    }

    fn register_toolbar_button(
        &self,
        addon: &str,
        id: &str,
        label: &str,
        handler: super::buttons::ButtonHandler,
    ) -> Result<(), MenuRegisterError> {
        match self.service.upgrade() {
            Some(service) => service.register_toolbar_button(addon, id, label, handler).map(|_| ()).map_err(Into::into),
            None => Err(MenuRegisterError::InvalidId),
        }
    }

    fn spawn_process(
        &self,
        addon: &str,
        spec: super::process::ProcessSpec,
        listener: super::process::ProcessListener,
    ) -> Result<super::process::ProcessId, super::process::SpawnError> {
        match self.service.upgrade() {
            Some(service) => service.processes().spawn(addon, spec, listener),
            None => Err(super::process::SpawnError::Failed),
        }
    }

    fn kill_process(&self, addon: &str, id: super::process::ProcessId) -> bool {
        self.service.upgrade().is_some_and(|service| service.processes().kill(addon, id))
    }

    fn close_process_input(&self, addon: &str, id: super::process::ProcessId) -> bool {
        self.service.upgrade().is_some_and(|service| service.processes().close_input(addon, id))
    }

    fn pipe_screen(
        &self,
        addon: &str,
        device: &str,
        app: Option<&str>,
        id: super::process::ProcessId,
    ) -> Result<(), super::screen_pipe::PipeError> {
        match self.service.upgrade() {
            Some(service) => service.pipe_screen(addon, device, app, id),
            None => Err(super::screen_pipe::PipeError::NoScreen),
        }
    }

    fn remove_registrations(&self, addon: &str) {
        if let Some(service) = self.service.upgrade() {
            service.remove_addon(addon);
        }
    }

    fn device_shell(&self, addon: &str, device: &str, command: &str) -> HostFuture<Result<String, CoreError>> {
        let (ctx, addon, device, command) = (self.ctx.clone(), addon.to_string(), device.to_string(), command.to_string());
        Box::pin(async move {
            let (serial, _) = ctx.online_device(&device)?;
            ctx.logger.log(LogLevel::Debug, LogCode::Addon, format!("[{addon}] shell on {device}: {command}"));
            let output = AdbApi::shell(&ctx.adb()?, &serial, &command).await?;
            if output.exit != 0 {
                return Err(CoreError::CommandFailed(output.exit));
            }
            let mut bytes = output.stdout;
            bytes.truncate(MAX_SHELL_OUTPUT_BYTES);
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        })
    }

    fn setting(&self, addon: &str, key: &str) -> Option<super::settings::SettingValue> {
        self.service.upgrade()?.setting_value(addon, key)
    }

    fn folder(&self, addon: &str) -> Option<std::path::PathBuf> {
        self.service.upgrade()?.folder_of(addon)
    }

    fn permission_denied(&self, addon: &str, permission: AddonPermission) {
        if let Some(service) = self.service.upgrade() {
            service.runner().record_denied(addon, permission);
        }
        self.ctx.logger.log(
            LogLevel::Warn,
            LogCode::Addon,
            format!("[{addon}] tried to use {permission:?} without the permission"),
        );
    }
}
