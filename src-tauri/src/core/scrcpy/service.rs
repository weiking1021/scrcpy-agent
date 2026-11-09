//! The screens that are open: starting scrcpy for a device or one of its apps,
//! keeping track of it until it ends, and acting on it while it runs (the
//! toolbar's buttons, the proxy's video and control socket).

use super::manager::Installed;
use super::protocol::{self, ControlMessage, ScrcpyProtocol};
use super::proxy::{Proxy, VideoSubscription};
use super::session;
use super::host::SessionSpec;
use super::version::ScrcpyVersion;
use crate::core::addons::buttons::{self, ButtonHandler, ButtonId, ButtonRegistry, RegisterError, ToolbarButton};
use crate::core::apps::SessionKey;
use crate::core::context::Ctx;
use crate::core::error::CoreError;
use crate::core::log::{LogCode, LogLevel};
use crate::core::model::{BitRateMbps, DeviceState, KeyboardMode, MaxSize, NoticeCode, NoticeLevel};
use crate::core::model::ScrcpyMode;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// What opening a device's screen produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedWindow {
    /// The title of the scrcpy window, unique to the device.
    pub title: String,
    /// What the window is called inside the core (`apps::SessionKey`): the
    /// toolbar of the window goes by it.
    pub key: String,
}

pub struct SessionService {
    ctx: Arc<Ctx>,
    /// The toolbar buttons that exist: built-in ones and those of add-ons.
    buttons: Mutex<ButtonRegistry>,
    /// The proxy of each screen that is open with one, by session key.
    proxies: Mutex<HashMap<String, Proxy>>,
    /// For each device, the session (key) that keeps it awake, if one does.
    awake_owner: Mutex<HashMap<String, String>>,
}

impl SessionService {
    pub(crate) fn new(ctx: Arc<Ctx>) -> Arc<Self> {
        Arc::new(SessionService {
            ctx,
            buttons: Mutex::new(ButtonRegistry::new()),
            proxies: Mutex::new(HashMap::new()),
            awake_owner: Mutex::new(HashMap::new()),
        })
    }

    /// Puts a button of an add-on on the toolbar.
    pub fn register_toolbar_button(
        &self,
        addon: &str,
        id: &str,
        label: &str,
        handler: ButtonHandler,
    ) -> Result<ButtonId, RegisterError> {
        self.buttons.lock().unwrap().register(addon, id, label, handler)
    }

    /// Takes away the buttons of an add-on.
    pub fn remove_addon_buttons(&self, addon: &str) {
        self.buttons.lock().unwrap().remove_addon(addon);
    }

    /// Opens the scrcpy window of a device. `async` on purpose: it starts a
    /// child process and a task, which need the Tokio runtime to be running.
    pub async fn open(self: &Arc<Self>, id: &str) -> Result<OpenedWindow, CoreError> {
        let (install, spec, mode) = self.ctx.with_state(|i| {
            let install = i.installed.clone().ok_or(CoreError::NotReady)?;
            if i.sessions.contains(id) {
                return Err(CoreError::AlreadyOpen);
            }
            let dev = i.devices.iter().find(|d| d.id == id).ok_or(CoreError::UnknownDevice)?;
            let serial = match (dev.state, &dev.adb_serial) {
                (DeviceState::Online, Some(s)) => s.clone(),
                _ => return Err(CoreError::NotOnline),
            };
            let shown_name = dev.label.as_deref().unwrap_or(&dev.name);
            let own = i.known.iter().find(|k| k.id == id).map(|k| k.settings.clone()).unwrap_or_default();
            let spec = SessionSpec {
                serial,
                title: session::window_title(shown_name, &dev.id),
                sdk: dev.sdk,
                app: None,
                keyboard: own.keyboard_mode.unwrap_or(i.settings.keyboard_mode),
                bit_rate: own.video_bit_rate_mbps.unwrap_or(i.settings.video_bit_rate_mbps),
                max_size: own.max_size,
                extra_args: i.settings.scrcpy_extra_args.clone(),
            };
            Ok((install, spec, i.settings.scrcpy_mode))
        })?;
        if spec.sdk.is_some_and(|s| s < session::MIN_AUDIO_SDK) {
            self.ctx.notice(NoticeLevel::Info, NoticeCode::AudioUnavailable, "", Some(id.to_string()));
        }
        self.check_options(id, &install, spec.keyboard, spec.bit_rate, spec.max_size);
        let proxy = if mode == ScrcpyMode::Proxy { self.start_proxy(&install, &spec, id).await } else { None };
        self.run_session(id, id, &install, spec, proxy).await
    }

    /// Starts scrcpy for a session and keeps track of it until it ends. `key`
    /// names the session (the device id for its own screen), `device_id` is the
    /// device it belongs to, for the notices.
    pub(crate) async fn run_session(
        self: &Arc<Self>,
        key: &str,
        device_id: &str,
        install: &Installed,
        mut spec: SessionSpec,
        proxy: Option<(Proxy, Vec<String>)>,
    ) -> Result<OpenedWindow, CoreError> {
        if !self.ctx.with_state(|i| i.sessions.insert(key.to_string())) { return Err(CoreError::AlreadyOpen); }
        if let Some(app) = &mut spec.app {
            app.keep_awake = app.keep_awake && self.claim_keep_awake(device_id, key);
        }
        self.ctx.emit();
        let title = spec.title.clone();
        let proxy_args = proxy.as_ref().map(|(_, args)| args.clone()).unwrap_or_default();
        let running = match self.ctx.scrcpy_host.start(key.into(), install.clone(), spec, proxy_args).await {
            Ok(running) => running,
            Err(error) => {
                self.ctx.with_state(|i| i.sessions.remove(key));
                self.awake_owner.lock().unwrap().retain(|_, owner| owner != key);
                self.ctx.emit();
                return Err(error.into());
            }
        };
        if let Some((proxy, _)) = proxy {
            self.proxies.lock().unwrap().insert(key.to_string(), proxy);
        }
        self.ctx.logger.log(LogLevel::Info, LogCode::SessionStarted, format!("screen {key} opened for {device_id}"));
        self.ctx.emit();

        let opened_key = key.to_string();
        let me = self.clone();
        let key = key.to_string();
        let id = device_id.to_string();
        tokio::spawn(async move {
            let result = running.0.await;
            // The proxy goes with its screen.
            me.proxies.lock().unwrap().remove(&key);
            me.awake_owner.lock().unwrap().retain(|_, owner| *owner != key);
            me.ctx.with_state(|i| i.sessions.remove(&key));
            let log = &me.ctx.logger;
            match result {
                Ok(f) if !f.success => {
                    log.log(LogLevel::Warn, LogCode::SessionEnded, format!("scrcpy for {id} ended with an error"));
                    me.ctx.notice(NoticeLevel::Error, NoticeCode::SessionFailed, f.tail, Some(id.clone()))
                }
                Err(e) => {
                    log.log(LogLevel::Warn, LogCode::SessionEnded, format!("scrcpy for {id} failed: {e:#}"));
                    me.ctx.notice(NoticeLevel::Error, NoticeCode::SessionFailed, format!("{e:#}"), Some(id.clone()))
                }
                Ok(f) => {
                    log.log(LogLevel::Info, LogCode::SessionEnded, format!("scrcpy {key} for {id} ended"));
                    // What scrcpy printed: for finding out why a window stayed empty.
                    for line in f.tail.lines() {
                        log.log(LogLevel::Debug, LogCode::ScrcpyOutput, format!("scrcpy {key}: {line}"));
                    }
                }
            }
            me.ctx.emit();
        });
        Ok(OpenedWindow { title, key: opened_key })
    }

    /// Starts the proxy for a screen if the user turned it on and it can be
    /// done: the scrcpy version needs a protocol adapter, and adb has to answer.
    /// Otherwise the screen opens without it, which is said in the log and to
    /// the user. Returns the proxy and the arguments that send scrcpy to it.
    pub(crate) async fn start_proxy(
        &self,
        install: &Installed,
        spec: &SessionSpec,
        device_id: &str,
    ) -> Option<(Proxy, Vec<String>)> {
        if !self.ctx.scrcpy_host.supports_proxy() { return None; }
        let unavailable = |why: String| {
            self.ctx.logger.log(LogLevel::Warn, LogCode::ProxyUnavailable, format!("no proxy for {device_id}: {why}"));
            self.ctx.notice(NoticeLevel::Info, NoticeCode::ProxyUnavailable, why, Some(device_id.to_string()));
            None
        };
        let Some(version) = ScrcpyVersion::parse(&install.version) else {
            return unavailable(format!("the scrcpy version {:?} is not a version number", install.version));
        };
        let Some(adapter) = protocol::protocol_for(&version) else {
            return unavailable(format!("scrcpy {version} has not been checked for the proxy"));
        };
        let adapter: Arc<dyn ScrcpyProtocol> = Arc::from(adapter);
        let adb = self.ctx.make_adb(install);
        // The forward scrcpy will create is told from the ones that exist now.
        let before = match crate::core::adb::api::AdbApi::forwards(&adb).await {
            Ok(forwards) => forwards,
            Err(e) => return unavailable(format!("adb could not list its forwards: {e}")),
        };
        let started = Proxy::start(
            adb,
            spec.serial.clone(),
            before,
            adapter.clone(),
            spec.stream_plan(),
            self.ctx.logger.clone(),
        )
        .await;
        match started {
            Ok(proxy) => {
                let args = adapter.proxy_args(proxy.port());
                Some((proxy, args))
            }
            Err(e) => unavailable(format!("the proxy could not listen: {e}")),
        }
    }

    /// Warn when the official desktop client must keep defaults for unchecked options.
    fn check_options(
        &self,
        device_id: &str,
        install: &Installed,
        keyboard: KeyboardMode,
        bit_rate: BitRateMbps,
        max_size: MaxSize,
    ) {
        if keyboard == KeyboardMode::Sdk && bit_rate.mbps().is_none() && max_size.pixels().is_none() {
            return;
        }
        match self.ctx.app_protocol() {
            Ok(_) => {}
            Err(_) => {
                let why = format!(
                    "the scrcpy version {} has not been checked for the keyboard mode {keyboard:?}, the bit rate {:?} and the size {:?}",
                    install.version,
                    bit_rate.mbps(),
                    max_size.pixels()
                );
                self.ctx.logger.log(LogLevel::Warn, LogCode::ProxyUnavailable, format!("{device_id}: {why}"));
                self.ctx.notice(NoticeLevel::Info, NoticeCode::OptionsUnavailable, why, Some(device_id.to_string()));
            }
        }
    }

    /// Whether the scrcpy window of this device is open.
    pub fn has_session(&self, id: &str) -> bool {
        self.ctx.with_state(|i| i.sessions.contains(id))
    }

    /// Follows the video of a screen that was opened with the proxy: the stream
    /// so far, in a form that can be decoded from its first packet, and then
    /// every new event. For add-ons.
    pub fn subscribe_video(&self, device_id: &str) -> Result<VideoSubscription, CoreError> {
        let proxies = self.proxies.lock().unwrap();
        proxies.get(device_id).map(Proxy::subscribe).ok_or(CoreError::NoProxy)
    }

    /// Sends a control message to a device whose screen is open with the
    /// proxy, between two of scrcpy's own. For add-ons and the built-in
    /// buttons.
    pub fn send_control(&self, device_id: &str, message: &ControlMessage) -> Result<(), CoreError> {
        let proxies = self.proxies.lock().unwrap();
        if let Some(proxy) = proxies.get(device_id) {
            return proxy.send(message).map_err(|_| CoreError::ControlUnavailable);
        }
        drop(proxies);
        if !self.ctx.scrcpy_host.has_control(device_id) { return Err(CoreError::NoProxy); }
        self.ctx.scrcpy_host.send_control(device_id, message).map_err(|_| CoreError::ControlUnavailable)
    }

    /// Says that the session `key` keeps the device awake, unless another
    /// session of the device already does. scrcpy puts back the settings it
    /// found when it started: a second window that also changed them would find
    /// the changed ones and leave them so when it ends last, so only one window
    /// of a device keeps it awake.
    fn claim_keep_awake(&self, device_id: &str, key: &str) -> bool {
        match self.awake_owner.lock().unwrap().entry(device_id.to_string()) {
            std::collections::hash_map::Entry::Occupied(_) => false,
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(key.to_string());
                true
            }
        }
    }

    /// Carries out what a toolbar button does on a screen: the device's own (its
    /// id) or the window of an app (its session key).
    pub async fn press_button(&self, session: &str, button: &ButtonId) -> Result<(), CoreError> {
        let key = SessionKey::parse(session);
        let device_id = key.device.as_str();
        let is_app = key.app.is_some();
        let log = &self.ctx.logger;
        let serial = self.ctx.with_state(|i| {
            let device = i.devices.iter().find(|d| d.id == device_id).ok_or(CoreError::UnknownDevice)?;
            match (device.state, &device.adb_serial) {
                (DeviceState::Online, Some(serial)) => Ok(serial.clone()),
                _ => Err(CoreError::NotOnline),
            }
        })?;
        if matches!(button, ButtonId::Addon { .. }) {
            let handler = self.buttons.lock().unwrap().handler(button).ok_or(CoreError::NotAvailable)?;
            handler(device_id, key.app.as_ref().map(|app| app.as_str()));
            return Ok(());
        }
        let Some(command) = buttons::device_command(button) else {
            // A button that can only act through the control socket, such as
            // rotating the screen: no proxy or no control socket, no action.
            let messages = buttons::control_only_messages(button).ok_or(CoreError::NotAvailable)?;
            log.log(
                LogLevel::Debug,
                LogCode::ButtonPressed,
                format!("{button:?} on {device_id}: {} control message(s) queued", messages.len()),
            );
            for message in &messages {
                if let Err(e) = self.send_control(session, message) {
                    log.log(LogLevel::Warn, LogCode::ButtonFailed, format!("{button:?} on {session}: {e}"));
                    return Err(e);
                }
            }
            return Ok(());
        };

        // With a proxy the control socket does it, which is faster; without one,
        // or when it cannot be used, a command on the device does. Not for the
        // window of an app: a command through adb would act on the device's own
        // screen, so there the control socket is the only way.
        let messages = buttons::control_messages(&command);
        match messages.iter().try_for_each(|m| self.send_control(session, m)) {
            Ok(()) => {
                log.log(
                    LogLevel::Debug,
                    LogCode::ButtonPressed,
                    format!("{button:?} on {session}: {} control message(s) queued", messages.len()),
                );
                return Ok(());
            }
            Err(e) if is_app => {
                log.log(LogLevel::Warn, LogCode::ButtonFailed, format!("{button:?} on {session}: {e}"));
                return Err(e);
            }
            Err(_) => {}
        }
        let shell = buttons::shell_command(&command);
        log.log(LogLevel::Debug, LogCode::ButtonPressed, format!("{button:?} on {device_id}: {shell}"));
        let result = match crate::core::adb::api::AdbApi::shell(&self.ctx.adb()?, &serial, &shell).await {
            Ok(out) if out.exit == 0 => Ok(()),
            Ok(out) => Err(CoreError::CommandFailed(out.exit)),
            Err(e) => Err(e.into()),
        };
        if let Err(e) = &result {
            log.log(LogLevel::Warn, LogCode::ButtonFailed, format!("{button:?} on {device_id}: {e}"));
        }
        result
    }

    /// The buttons of the toolbar, in the order they are shown. With a device,
    /// the buttons that need the proxy's control socket are available when its
    /// screen has one.
    /// Every button there is, shown or not, in the order of the toolbar, with the text of the ones add-ons made: the
    /// list the user arranges in the settings.
    pub fn toolbar_layout(&self) -> Vec<buttons::LayoutEntry> {
        let prefs = self.ctx.with_state(|i| i.settings.toolbar.buttons.clone());
        let registry = self.buttons.lock().unwrap();
        buttons::resolve_layout(&registry.all(), &prefs)
            .into_iter()
            .map(|pref| buttons::LayoutEntry { label: registry.label_of(&pref.id), id: pref.id, visible: pref.visible })
            .collect()
    }

    pub fn toolbar_buttons(&self, device_id: Option<&str>) -> Vec<ToolbarButton> {
        let prefs = self.ctx.with_state(|i| i.settings.toolbar.buttons.clone());
        let registry = self.buttons.lock().unwrap();
        let available = registry.all();
        let control = device_id.is_some_and(|id| self.proxies.lock().unwrap().get(id).is_some_and(Proxy::offers_control)
            || self.ctx.scrcpy_host.has_control(id));
        // The window of an app is reached through the control socket only.
        let window_of_app = device_id.is_some_and(|id| SessionKey::parse(id).app.is_some());
        let mut listed = buttons::toolbar_buttons(&available, &prefs, control, !window_of_app);
        for button in &mut listed {
            button.label = registry.label_of(&button.id);
        }
        listed
    }
}
