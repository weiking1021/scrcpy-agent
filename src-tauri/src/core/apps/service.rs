//! The apps of a device: listing them, opening one in a window of its own, and
//! what the user keeps about them (favourites, the ones opened lately).

use super::{AppCache, AppDisplay, AppEntry, PackageName};
use crate::core::adb::api::AdbApi;
use crate::core::context::Ctx;
use crate::core::discovery::service::unix_now;
use crate::core::error::CoreError;
use crate::core::log::{LogCode, LogLevel};
use crate::core::model::{AppUsage, MAX_RECENT_APPS, ScrcpyMode};
use crate::core::scrcpy::service::{OpenedWindow, SessionService};
use crate::core::scrcpy::session::{self, LaunchSpec};
use crate::core::device::display;
use crate::core::store;
use std::sync::Arc;
use std::time::Duration;

/// How long listing the apps of a device may take.
const LIST_APPS_LIMIT: Duration = Duration::from_secs(40);

pub struct AppService {
    ctx: Arc<Ctx>,
    sessions: Arc<SessionService>,
}

impl AppService {
    pub(crate) fn new(ctx: Arc<Ctx>, sessions: Arc<SessionService>) -> Arc<Self> {
        Arc::new(AppService { ctx, sessions })
    }

    /// The apps of a device as they were when last listed (nothing, if never).
    pub fn cached_apps(&self, device_id: &str) -> AppCache {
        self.ctx.store.device_of(device_id).map(|device| device.load::<store::Apps>()).unwrap_or_default()
    }

    /// The apps installed on a device, as scrcpy lists them. The result is also
    /// kept for `cached_apps`.
    pub async fn list_apps(&self, device_id: &str) -> Result<Vec<AppEntry>, CoreError> {
        let (serial, _) = self.ctx.online_device(device_id)?;
        let (install, protocol) = self.ctx.app_protocol()?;
        let args = protocol.list_apps_args(&serial);
        let output = session::run_to_end(&install, &args, LIST_APPS_LIMIT).await?;
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push('\n');
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        let apps = protocol.parse_app_list(&text);
        let log = &self.ctx.logger;
        // The fact is whether scrcpy listed apps, not what it said.
        if apps.is_empty() && !output.success {
            log.log(LogLevel::Warn, LogCode::ButtonFailed, format!("listing the apps of {device_id} failed"));
            return Err(CoreError::AppListFailed(text.trim().to_string()));
        }
        log.log(LogLevel::Debug, LogCode::ButtonPressed, format!("{} apps on {device_id}", apps.len()));
        let cache = AppCache { updated_at: unix_now(), apps: apps.clone() };
        let kept = match self.ctx.store.device_of(device_id) {
            Some(device) => device.save::<store::Apps>(&cache).map(|_| ()),
            None => Ok(()),
        };
        if let Err(e) = kept {
            log.log(LogLevel::Warn, LogCode::ButtonFailed, format!("the app list of {device_id} was not kept: {e}"));
        }
        Ok(apps)
    }

    /// The size of the device's screen in the orientation it is held in now, or
    /// `None` (said in the log) if the device does not tell it in the known form.
    async fn current_screen_size(&self, serial: &str, device_id: &str) -> Option<display::ScreenSize> {
        let log = &self.ctx.logger;
        let dump = match self.ctx.adb() {
            Ok(adb) => adb.shell(serial, display::DISPLAY_DUMP_COMMAND).await,
            Err(e) => {
                log.log(LogLevel::Warn, LogCode::ButtonFailed, format!("screen size of {device_id}: {e}"));
                return None;
            }
        };
        let size = dump.ok().filter(|o| o.exit == 0).and_then(|o| display::current_size(&String::from_utf8_lossy(&o.stdout)));
        match size {
            Some(s) => log.log(
                LogLevel::Debug,
                LogCode::ButtonPressed,
                format!("screen of {device_id} is {}x{} ({:?})", s.width, s.height, s.orientation()),
            ),
            None => log.log(
                LogLevel::Warn,
                LogCode::ButtonFailed,
                format!("the screen size of {device_id} is unknown: its display has the default size"),
            ),
        }
        size
    }

    /// Opens the app of a shortcut: it is named as the list kept for the device
    /// names it (the shortcut only has the package), else by its package.
    pub async fn open_app_from_shortcut(
        self: &Arc<Self>,
        device_id: &str,
        package: &PackageName,
    ) -> Result<OpenedWindow, CoreError> {
        let name = self.app_name(device_id, package);
        self.open_app(device_id, package.as_str(), &name, None).await
    }

    /// The name of an app as the list kept for the device has it, else its package.
    pub fn app_name(&self, device_id: &str, package: &PackageName) -> String {
        self.cached_apps(device_id)
            .apps
            .into_iter()
            .find(|a| a.package == package.as_str())
            .map_or_else(|| package.as_str().to_string(), |a| a.name)
    }

    /// Opens an app of a device in a window of its own: on a display of its own or
    /// on the device's screen as the device's settings say, unless `display` says.
    pub async fn open_app(
        self: &Arc<Self>,
        device_id: &str,
        package: &str,
        name: &str,
        display: Option<AppDisplay>,
    ) -> Result<OpenedWindow, CoreError> {
        let package = PackageName::parse(package).ok_or(CoreError::InvalidPackage)?;
        let (serial, device_name) = self.ctx.online_device(device_id)?;
        let (install, protocol) = self.ctx.app_protocol()?;
        let key = format!("{device_id}#app:{}", package.as_str());
        let (sdk, extra_args, keyboard, bit_rate, max_size, display_of_device, keep_awake, mode, toolbar) =
            self.ctx.with_state(|i| {
                if i.sessions.contains(&key) {
                    return Err(CoreError::AlreadyOpen);
                }
                let sdk = i.devices.iter().find(|d| d.id == device_id).and_then(|d| d.sdk);
                // What is set for this device wins over the general settings.
                let own = i.known.iter().find(|k| k.id == device_id).map(|k| k.settings.clone()).unwrap_or_default();
                let display =
                    if own.apps_in_virtual_display { AppDisplay::VirtualDisplay } else { AppDisplay::MainDisplay };
                Ok((
                    sdk,
                    i.settings.scrcpy_extra_args.clone(),
                    own.keyboard_mode.unwrap_or(i.settings.keyboard_mode),
                    own.video_bit_rate_mbps.unwrap_or(i.settings.video_bit_rate_mbps),
                    own.max_size,
                    display,
                    own.keep_awake,
                    i.settings.scrcpy_mode,
                    i.settings.toolbar.enabled,
                ))
            })?;
        let display = display.unwrap_or(display_of_device);
        // A display of its own is made in the orientation the device is held in now.
        let size = if display == AppDisplay::VirtualDisplay {
            self.current_screen_size(&serial, device_id).await
        } else {
            None
        };
        let keep_awake = keep_awake && self.sessions.claim_keep_awake(device_id, &key);
        let shown = if name.trim().is_empty() { package.as_str() } else { name.trim() };
        let mut spec = LaunchSpec {
            serial,
            title: session::app_window_title(shown, &device_name, device_id),
            sdk,
            proxy_args: Vec::new(),
            app_args: protocol.start_app_args(&package, display, size, keep_awake),
            option_args: [
                protocol.keyboard_args(keyboard),
                protocol.video_bit_rate_args(bit_rate),
                protocol.max_size_args(max_size),
            ]
            .concat(),
            extra_args,
        };
        // The toolbar of the window acts through the control socket (a command through
        // adb would reach the device's own screen, not this window), so the window
        // goes through the proxy when there is a toolbar, as well as when the proxy
        // mode is on.
        let proxy = if mode == ScrcpyMode::Proxy || toolbar {
            self.sessions.start_proxy(&install, &spec, device_id).await
        } else {
            None
        };
        if let Some((_, args)) = &proxy {
            spec.proxy_args = args.clone();
        }
        let opened = self.sessions.run_session(&key, device_id, &install, spec, proxy);
        if opened.is_err() && keep_awake {
            self.sessions.release_keep_awake(device_id);
        }
        if opened.is_ok() {
            self.record_recent_app(device_id, &package);
        }
        opened
    }

    /// The favourite apps of a device and the ones opened from this program lately.
    pub fn app_usage(&self, device_id: &str) -> AppUsage {
        self.ctx.known_device(device_id).map(|k| k.apps).unwrap_or_default()
    }

    /// Marks an app of a device as a favourite, or takes the mark away. Marking
    /// again changes nothing, and taking away what is not marked is no error.
    pub fn set_app_favorite(&self, device_id: &str, package: &str, favorite: bool) -> Result<(), CoreError> {
        let package = PackageName::parse(package).ok_or(CoreError::InvalidPackage)?;
        self.ctx.update_known(device_id, |known| {
            let favorites = &mut known.apps.favorites;
            let marked = favorites.iter().any(|p| p == package.as_str());
            if favorite && !marked {
                favorites.push(package.as_str().to_string());
                true
            } else if !favorite && marked {
                favorites.retain(|p| p != package.as_str());
                true
            } else {
                false
            }
        })
    }

    /// An app was opened from this program: it is the newest of the recent ones.
    fn record_recent_app(&self, device_id: &str, package: &PackageName) {
        let kept = self.ctx.update_known(device_id, |known| {
            let recent = &mut known.apps.recent;
            recent.retain(|p| p != package.as_str());
            recent.insert(0, package.as_str().to_string());
            recent.truncate(MAX_RECENT_APPS);
            true
        });
        match kept {
            // A device that is not known keeps nothing.
            Ok(()) | Err(CoreError::UnknownDevice) => {}
            Err(e) => self.ctx.logger.log(
                LogLevel::Warn,
                LogCode::ButtonFailed,
                format!("the recent apps of {device_id} were not kept: {e}"),
            ),
        }
    }
}
