//! The menu entries that add-ons put on the device cards and on the apps.

use super::buttons::{ButtonHandler, ButtonId, RegisterError};
use super::process::{ProcessId, ProcessTable};
use super::settings::{self, AddonSettingsView, SettingValue};
use crate::core::store::DataName;
use super::screen_pipe::{PipeError, PumpEnd, pump, screen_key};
use crate::core::files::service::FileService;
use crate::core::theme::service::ThemeService;
use super::menu::{
    AppMenu, AppMenuHandler, AppMenuItem, AppMenuItemId, DeviceMenu, MenuHandler, MenuItem, MenuItemId,
    MenuRegisterError,
};
use super::library::{self, AddonInfo};
use super::manifest::AddonManifest;
use super::permission::AddonPermission;
use super::engine::limits::Limits;
use super::events::{AddonEvent, current_events};
use super::runner::{CoreHost, InstalledAddon, RunState, Runner};
use crate::core::apps::PackageName;
use crate::core::context::Ctx;
use crate::core::error::CoreError;
use crate::core::log::{LogCode, LogLevel};
use crate::core::model::DeviceState;
use crate::core::scrcpy::service::SessionService;
use crate::core::window::layout::Os;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

/// The name of the file in the data folder of an add-on that holds what the user set for it.
const SETTINGS_FILE: &str = "settings";

pub struct AddonService {
    ctx: Arc<Ctx>,
    /// This service, for what the running add-ons ask of it.
    me: Weak<AddonService>,
    /// The entries of the menu on each device card.
    menu: Mutex<DeviceMenu>,
    /// The entries of the menu of an app in the list of apps.
    app_menu: Mutex<AppMenu>,
    /// The screens, whose toolbar has the buttons of the add-ons.
    sessions: Arc<SessionService>,
    /// The file types and the themes that add-ons register are kept by their services; this service takes
    /// them back with the rest.
    files: Arc<FileService>,
    themes: Arc<ThemeService>,
    /// The programs the add-ons started.
    processes: Arc<ProcessTable>,
    /// The add-ons that run.
    runner: Runner,
    /// How much each add-on may use.
    limits: Mutex<Limits>,
}

impl AddonService {
    pub(crate) fn new(
        ctx: Arc<Ctx>,
        sessions: Arc<SessionService>,
        files: Arc<FileService>,
        themes: Arc<ThemeService>,
    ) -> Arc<Self> {
        let service = Arc::new_cyclic(|me| AddonService {
            ctx,
            me: me.clone(),
            menu: Mutex::new(DeviceMenu::new()),
            app_menu: Mutex::new(AppMenu::new()),
            sessions,
            files,
            themes,
            processes: ProcessTable::new(),
            runner: Runner::default(),
            limits: Mutex::new(Limits::default()),
        });
        // What happens to the devices, the screens and the files is told to the add-ons that run.
        let me = Arc::downgrade(&service);
        service.ctx.set_addon_events(Arc::new(move |event| {
            if let Some(service) = me.upgrade() {
                service.runner.emit_all(&event);
            }
        }));
        service
    }

    /// The programs the add-ons started.
    pub fn processes(&self) -> &Arc<ProcessTable> {
        &self.processes
    }

    pub(crate) fn runner(&self) -> &Runner {
        &self.runner
    }

    /// Sets how much an add-on may use from now on (the add-ons that run keep what they started with).
    #[cfg(test)]
    pub(crate) fn set_limits(&self, limits: Limits) {
        *self.limits.lock().unwrap() = limits;
    }

    /// Starts the add-on as the list shows it (in place of the one that runs), and tells the log if it failed.
    fn run(&self, info: &AddonInfo) -> RunState {
        let host = Arc::new(CoreHost { service: self.me.clone(), ctx: self.ctx.clone() });
        let limits = *self.limits.lock().unwrap();
        let state = self.runner.start(info, host, limits);
        if let RunState::Failed(failure) = &state {
            self.ctx.logger.log(LogLevel::Warn, LogCode::Addon, format!("{} did not start: {failure:?}", info.id));
        }
        // It starts in the middle of things: it is told of what is the case already, as it would have been if it
        // had been there (the usable devices, the open screens).
        if state == RunState::Running {
            for event in current_events(&self.ctx.snapshot()) {
                self.runner.emit_to(&info.id, event);
            }
        }
        state
    }

    /// Registers a menu entry of an add-on for the device cards.
    pub fn register_menu_item(
        &self,
        addon: &str,
        id: &str,
        label: &str,
        handler: MenuHandler,
    ) -> Result<MenuItemId, MenuRegisterError> {
        self.menu.lock().unwrap().register(addon, id, label, handler)
    }

    /// Registers an entry of an add-on for the menu of an app.
    pub fn register_app_menu_item(
        &self,
        addon: &str,
        id: &str,
        label: &str,
        handler: AppMenuHandler,
    ) -> Result<AppMenuItemId, MenuRegisterError> {
        self.app_menu.lock().unwrap().register(addon, id, label, handler)
    }

    /// Puts a button of an add-on on the toolbar.
    pub fn register_toolbar_button(
        &self,
        addon: &str,
        id: &str,
        label: &str,
        handler: ButtonHandler,
    ) -> Result<ButtonId, RegisterError> {
        self.sessions.register_toolbar_button(addon, id, label, handler)
    }

    /// Takes back everything an add-on registered, wherever it is kept: its menu entries, the buttons of its
    /// toolbar, its file types and its themes. This is the one place that knows all of them.
    pub fn remove_addon(&self, addon: &str) {
        self.menu.lock().unwrap().remove_addon(addon);
        self.app_menu.lock().unwrap().remove_addon(addon);
        self.sessions.remove_addon_buttons(addon);
        self.files.remove_addon(addon);
        self.themes.remove_addon(addon);
        self.processes.remove_addon(addon);
    }

    /// Feeds the live video of a device's screen to a program of an add-on, until the screen or the program ends.
    /// Must be called inside a Tokio runtime, which runs the feeding.
    pub fn pipe_screen(&self, addon: &str, device: &str, app: Option<&str>, id: ProcessId) -> Result<(), PipeError> {
        if !self.processes.owns(addon, id) {
            return Err(PipeError::NoProcess);
        }
        let key = screen_key(device, app).ok_or(PipeError::NoScreen)?;
        let subscription = self.sessions.subscribe_video(&key).map_err(|_| PipeError::NoScreen)?;
        let (table, addon) = (self.processes.clone(), addon.to_string());
        tokio::spawn(async move {
            let end = pump(subscription, |bytes| table.write(&addon, id, bytes)).await;
            // Whether the screen or the program ended, there is nothing left to feed; the program decides what
            // it does with a closed input.
            if end == PumpEnd::StreamEnded {
                table.close_input(&addon, id);
            }
        });
        Ok(())
    }

    /// The entries of a device card's menu.
    pub fn device_menu(&self, device_id: &str) -> Result<Vec<MenuItem>, CoreError> {
        let device = self.ctx.device(device_id).ok_or(CoreError::UnknownDevice)?;
        Ok(self.menu.lock().unwrap().items(device.state == DeviceState::Online))
    }

    /// Carries out an add-on's menu entry for a device. The built-in entries
    /// are handled by the UI (they open dialogs), so they have no action here.
    pub fn run_menu_item(&self, device_id: &str, item: &MenuItemId) -> Result<(), CoreError> {
        self.ctx.online_device(device_id)?;
        let handler = self.menu.lock().unwrap().handler(item).ok_or(CoreError::NotAvailable)?;
        handler(device_id);
        Ok(())
    }

    /// The entries of the menu of an app of a device; they are enabled while the
    /// device is ready.
    pub fn app_menu(&self, device_id: &str, package: &str) -> Result<Vec<AppMenuItem>, CoreError> {
        PackageName::parse(package).ok_or(CoreError::InvalidPackage)?;
        let device = self.ctx.device(device_id).ok_or(CoreError::UnknownDevice)?;
        Ok(self.app_menu.lock().unwrap().items(device.state == DeviceState::Online))
    }

    /// The installed add-ons, each read again from its folder, with what each is doing.
    pub fn installed(&self) -> Vec<InstalledAddon> {
        library::list(&self.ctx.store.global(), Os::current())
            .into_iter()
            .map(|info| InstalledAddon {
                run: if info.enabled { self.runner.state_of(&info.id) } else { RunState::Disabled },
                denied: self.runner.denied_of(&info.id),
                info,
            })
            .collect()
    }

    /// Starts every installed add-on that can run (when the program starts). It returns when each has run
    /// its `main.js` to the end or failed, so it is called from a thread that may wait.
    pub fn start_installed(&self) {
        for info in library::list(&self.ctx.store.global(), Os::current()) {
            self.run(&info);
        }
    }

    /// What the add-on in a folder is and asks for, to show before it is installed.
    pub fn inspect_folder(&self, folder: &Path) -> Result<AddonManifest, CoreError> {
        library::inspect_folder(&self.ctx.store.global(), folder, Os::current()).map_err(CoreError::Addon)
    }

    /// Installs the add-on in a folder with the permissions the user gave.
    pub fn install_from_folder(&self, folder: &Path, granted: &[AddonPermission]) -> Result<AddonInfo, CoreError> {
        let info = library::install_from_folder(&self.ctx.store.global(), folder, Os::current(), granted)
            .map_err(CoreError::Addon)?;
        self.note(format!("installed {} with {} of the permissions it asks for", info.id, info.granted.len()));
        self.run(&info);
        Ok(info)
    }

    /// The user decides again which permissions an installed add-on has.
    pub fn decide_permissions(&self, id: &str, granted: &[AddonPermission]) -> Result<AddonInfo, CoreError> {
        let info = library::accept_permissions(&self.ctx.store.global(), id, Os::current(), granted)
            .map_err(CoreError::Addon)?;
        self.note(format!("permissions of {id} decided: {} given", info.granted.len()));
        // It starts again with what it was given now.
        self.run(&info);
        Ok(info)
    }

    /// Switches an installed add-on off or on: off, it stops and what it registered goes; on, it starts again.
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<AddonInfo, CoreError> {
        let info = library::set_enabled(&self.ctx.store.global(), id, Os::current(), enabled).map_err(CoreError::Addon)?;
        self.note(format!("{id} switched {}", if enabled { "on" } else { "off" }));
        self.run(&info);
        Ok(info)
    }

    /// Reads an installed add-on from its folder again and starts it again (a changed `main.js` takes effect; what it
    /// registered goes and what the new one registers comes). What the user gave it stays.
    pub fn reload(&self, id: &str) -> Result<AddonInfo, CoreError> {
        let info = library::list(&self.ctx.store.global(), Os::current())
            .into_iter()
            .find(|info| info.id == id)
            .ok_or(CoreError::Addon(library::InstallError::NotInstalled))?;
        self.note(format!("reloaded {id}"));
        self.run(&info);
        Ok(info)
    }

    /// The settings of an installed add-on: what it declared and the value of each now.
    pub fn settings_of(&self, id: &str) -> Result<AddonSettingsView, CoreError> {
        let specs = self.declared_settings(id)?;
        let values = settings::values_of(&specs, &self.kept_settings(id));
        Ok(AddonSettingsView { specs, values })
    }

    /// The user sets one setting of an add-on: the value is checked against what was declared, kept, and the add-on that
    /// runs is told. Gives the value as it is kept.
    pub fn set_setting(&self, id: &str, key: &str, value: &serde_json::Value) -> Result<SettingValue, CoreError> {
        let specs = self.declared_settings(id)?;
        let spec = specs.iter().find(|s| s.id == key).ok_or(CoreError::UnknownSetting)?;
        let kept = settings::validate_value(spec, value).map_err(CoreError::InvalidSetting)?;
        // What is kept is every value that is good now (the rest are defaults), with this one changed.
        let mut values = settings::values_of(&specs, &self.kept_settings(id));
        values.insert(key.to_string(), kept.clone());
        let scope = self.ctx.store.addon(id).ok_or(CoreError::Addon(library::InstallError::NotInstalled))?;
        let name = DataName::new(SETTINGS_FILE).ok_or(CoreError::Addon(library::InstallError::Storage))?;
        let text = serde_json::to_value(&values).map_err(|e| CoreError::Storage(e.to_string()))?;
        scope.save_value(&name, &text).map_err(|e| CoreError::Storage(e.to_string()))?;
        self.runner.emit_to(id, AddonEvent::SettingChanged { key: key.to_string(), value: kept.clone() });
        Ok(kept)
    }

    /// The value of one setting of an add-on now; `None` when it did not declare it (what `host.settings.get` reads).
    pub fn setting_value(&self, id: &str, key: &str) -> Option<SettingValue> {
        let specs = self.declared_settings(id).ok()?;
        specs.iter().any(|s| s.id == key).then(|| settings::values_of(&specs, &self.kept_settings(id)).remove(key)).flatten()
    }

    /// The folder an installed add-on is linked to (what `host.addon.folder` gives); `None` when it is not installed.
    pub fn folder_of(&self, id: &str) -> Option<PathBuf> {
        library::list(&self.ctx.store.global(), Os::current()).into_iter().find(|info| info.id == id).map(|info| info.folder)
    }

    /// The settings an installed add-on declares in its manifest now.
    fn declared_settings(&self, id: &str) -> Result<Vec<settings::SettingSpec>, CoreError> {
        library::list(&self.ctx.store.global(), Os::current())
            .into_iter()
            .find(|info| info.id == id)
            .ok_or(CoreError::Addon(library::InstallError::NotInstalled))
            .map(|info| info.manifest.map(|m| m.settings).unwrap_or_default())
    }

    /// What was kept of the settings of an add-on (any JSON; `null` when nothing was).
    fn kept_settings(&self, id: &str) -> serde_json::Value {
        match (self.ctx.store.addon(id), DataName::new(SETTINGS_FILE)) {
            (Some(scope), Some(name)) => scope.load_value(&name),
            _ => serde_json::Value::Null,
        }
    }

    /// Removes an installed add-on: what it registered goes, its folder and its data stay.
    pub fn uninstall(&self, id: &str) -> bool {
        self.runner.stop(id);
        let removed = library::remove(&self.ctx.store.global(), id);
        if removed {
            self.remove_addon(id);
            self.note(format!("removed {id}"));
        }
        removed
    }

    fn note(&self, message: String) {
        self.ctx.logger.log(LogLevel::Info, LogCode::Addon, message);
    }

    /// Carries out an add-on's entry of the menu of an app. The built-in entries
    /// are carried out by the UI with their own commands.
    pub fn run_app_menu_item(&self, device_id: &str, package: &str, item: &AppMenuItemId) -> Result<(), CoreError> {
        PackageName::parse(package).ok_or(CoreError::InvalidPackage)?;
        self.ctx.online_device(device_id)?;
        let handler = self.app_menu.lock().unwrap().handler(item).ok_or(CoreError::NotAvailable)?;
        handler(device_id, package);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::addons::runner::StartFailure;
    use crate::core::context::CoreOptions;
    use crate::core::store::Store;
    use std::path::PathBuf;

    // The folder is made in the temporary folder; nothing in it is run.

    fn start(name: &str) -> (Arc<AddonService>, Arc<Ctx>, PathBuf) {
        let root = std::env::temp_dir().join(format!("scrcpy-agent-addon-service-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let ctx = Ctx::new(Store::open(root.join("data")), Arc::new(|_event| {}), CoreOptions::default());
        let service = AddonService::new(
            ctx.clone(),
            SessionService::new(ctx.clone()),
            FileService::new(ctx.clone()),
            ThemeService::new(ctx.clone()),
        );
        (service, ctx, root)
    }

    fn make_addon(root: &Path, id: &str, permissions: &str) -> PathBuf {
        let folder = root.join("addons").join(id);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("addon.json"),
            format!(r#"{{"id":"{id}","name":"Name","version":"1","permissions":{permissions},"api":1}}"#),
        )
        .unwrap();
        std::fs::write(folder.join("main.js"), "// made up").unwrap();
        folder
    }

    fn addon_lines(ctx: &Ctx) -> Vec<String> {
        ctx.logger.history().into_iter().filter(|l| l.code == LogCode::Addon).map(|l| l.message).collect()
    }

    #[test]
    fn removing_an_add_on_takes_away_what_it_registered_and_only_that_and_says_so_in_the_log() {
        let (service, ctx, root) = start("uninstall");
        let folder = make_addon(&root, "mine", "[]");
        let other = make_addon(&root, "theirs", "[]");
        assert!(service.install_from_folder(&folder, &[]).is_ok());
        assert!(service.install_from_folder(&other, &[]).is_ok());
        let noop: MenuHandler = Arc::new(|_| {});
        service.register_menu_item("mine", "go", "Go", noop.clone()).unwrap();
        service.register_menu_item("theirs", "go", "Go", noop).unwrap();
        let app_noop: AppMenuHandler = Arc::new(|_, _| {});
        service.register_app_menu_item("mine", "go", "Go", app_noop).unwrap();
        assert!(service.uninstall("mine"));
        let menu = service.menu.lock().unwrap().items(true);
        assert_eq!(menu.len(), 2, "the open-app entry and the other add-on's: {menu:?}");
        let built_in = crate::core::addons::menu::AppMenu::new().items(true).len();
        assert_eq!(service.app_menu.lock().unwrap().items(true).len(), built_in, "only the built-in entries are left");
        assert_eq!(service.installed().iter().map(|a| a.info.id.as_str()).collect::<Vec<_>>(), ["theirs"]);
        let lines = addon_lines(&ctx);
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[2].contains("mine"), "the line names the add-on: {lines:?}");
        // Removing it again changes nothing and says nothing.
        assert!(!service.uninstall("mine"));
        assert_eq!(addon_lines(&ctx).len(), 3);
    }

    #[test]
    fn installing_and_deciding_are_told_in_the_log_with_how_many_permissions_were_given() {
        let (service, ctx, root) = start("log");
        let folder = make_addon(&root, "asks", r#"["device_shell","run_process"]"#);
        let info = service.install_from_folder(&folder, &[AddonPermission::RunProcess]);
        assert_eq!(info.map(|i| i.granted), Ok(vec![AddonPermission::RunProcess]));
        let decided = service.decide_permissions("asks", &AddonPermission::ALL);
        assert_eq!(decided.map(|i| i.granted.len()), Ok(2));
        let lines = addon_lines(&ctx);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("asks") && lines[0].contains('1'), "{lines:?}");
        assert!(lines[1].contains("asks") && lines[1].contains('2'), "{lines:?}");
        // A refusal is an error of the add-on kind and writes nothing.
        let missing = service.install_from_folder(&root.join("nowhere"), &[]);
        assert_eq!(missing.map(|_| ()), Err(CoreError::Addon(library::InstallError::NotAFolder)));
        assert_eq!(addon_lines(&ctx).len(), 2);
    }

    // The add-ons here are real: their `main.js` is run by the engine, on threads of their own. What they
    // do reaches the log and the menu of this service.

    /// An add-on folder with this `main.js`.
    fn make_script_addon(root: &Path, id: &str, permissions: &str, script: &str) -> PathBuf {
        let folder = make_addon(root, id, permissions);
        std::fs::write(folder.join("main.js"), script).unwrap();
        folder
    }

    /// Waits (at most five seconds) for a line of the log of the add-ons to be exactly this.
    fn wait_for_line(ctx: &Ctx, line: &str) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if addon_lines(ctx).iter().any(|l| l == line) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        false
    }

    fn runs(service: &AddonService) -> Vec<RunState> {
        service.installed().into_iter().map(|a| a.run).collect()
    }

    fn entries_of(service: &AddonService, addon: &str) -> usize {
        let menu = service.menu.lock().unwrap().items(true);
        menu.iter().filter(|i| matches!(&i.id, MenuItemId::Addon { addon: owner, .. } if owner == addon)).count()
    }

    #[test]
    fn an_installed_add_on_runs_its_main_js_and_the_list_says_it_runs() {
        let (service, ctx, root) = start("runs");
        let script = r#"host.log("hello from main"); host.deviceMenu.register({ id: "say", label: "Say", onClick: async () => {} });"#;
        let folder = make_script_addon(&root, "hello", "[]", script);
        let installed = service.install_from_folder(&folder, &[]);
        assert!(installed.is_ok(), "{installed:?}");
        assert_eq!(runs(&service), vec![RunState::Running]);
        assert_eq!(entries_of(&service, "hello"), 1);
        assert_eq!(
            addon_lines(&ctx),
            vec!["installed hello with 0 of the permissions it asks for".to_string(), "[hello] hello from main".to_string()]
        );
    }

    #[test]
    fn an_add_on_whose_main_js_fails_is_listed_as_failed_with_the_reason_and_leaves_nothing_registered() {
        let (service, ctx, root) = start("fails");
        service.set_limits(Limits { init_time: std::time::Duration::from_millis(300), ..Limits::default() });
        let script = r#"host.deviceMenu.register({ id: "x", label: "X", onClick: async () => {} }); throw new Error("boom");"#;
        let folder = make_script_addon(&root, "broken", "[]", script);
        assert!(service.install_from_folder(&folder, &[]).is_ok(), "it is installed all the same");
        let run = runs(&service).remove(0);
        assert!(matches!(&run, RunState::Failed(StartFailure::Script(text)) if text.contains("boom")), "{run:?}");
        assert_eq!(entries_of(&service, "broken"), 0);
        assert!(addon_lines(&ctx).iter().any(|l| l.contains("broken") && l.contains("boom")), "{:?}", addon_lines(&ctx));
        // An add-on that never ends is a timeout, and a script that cannot be read is said so.
        let endless = make_script_addon(&root, "endless", "[]", "while (true) {}");
        assert!(service.install_from_folder(&endless, &[]).is_ok());
        let states: Vec<(String, RunState)> = service.installed().into_iter().map(|a| (a.info.id, a.run)).collect();
        assert_eq!(states[1], ("endless".to_string(), RunState::Failed(StartFailure::Timeout)));
    }

    #[test]
    fn removing_an_add_on_stops_it_and_what_its_timers_would_do_does_not_happen() {
        let (service, ctx, root) = start("stops");
        let folder = make_script_addon(&root, "ticker", "[]", r#"setInterval(() => host.log("tick"), 20);"#);
        assert!(service.install_from_folder(&folder, &[]).is_ok());
        assert!(wait_for_line(&ctx, "[ticker] tick"), "{:?}", addon_lines(&ctx));
        assert!(service.uninstall("ticker"));
        let ticks = |ctx: &Ctx| addon_lines(ctx).iter().filter(|l| *l == "[ticker] tick").count();
        let at_removal = ticks(&ctx);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(ticks(&ctx), at_removal);
        assert_eq!(runs(&service), Vec::<RunState>::new());
    }

    #[test]
    fn what_an_add_on_tries_without_the_permission_is_told_in_the_list_and_deciding_again_runs_it_anew() {
        let (service, ctx, root) = start("denied");
        let script = r#"
            host.deviceShell("no-such-device", "ls").then(
                () => host.log("ran"),
                (e) => host.log(e.name + ":" + (e.permission || e.code)),
            );
        "#;
        let folder = make_script_addon(&root, "asks", r#"["device_shell"]"#, script);
        assert!(service.install_from_folder(&folder, &[]).is_ok());
        assert!(wait_for_line(&ctx, "[asks] PermissionDenied:device_shell"), "{:?}", addon_lines(&ctx));
        assert_eq!(service.installed()[0].denied, vec![AddonPermission::DeviceShell]);
        // The user gives it. It starts again; the command now goes to the program, which has no such device.
        let decided = service.decide_permissions("asks", &[AddonPermission::DeviceShell]);
        assert!(decided.is_ok(), "{decided:?}");
        assert!(wait_for_line(&ctx, "[asks] DeviceError:unknown_device"), "{:?}", addon_lines(&ctx));
        assert_eq!(service.installed()[0].denied, Vec::<AddonPermission>::new());
        assert_eq!(runs(&service), vec![RunState::Running]);
    }

    #[test]
    fn add_ons_that_were_installed_before_start_when_the_program_starts_and_one_that_asks_for_more_does_not() {
        let (service, ctx, root) = start("startup");
        let global = ctx.store.global();
        let fine = make_script_addon(&root, "fine", "[]", r#"host.log("fine started");"#);
        let grows = make_script_addon(&root, "grows", "[]", r#"host.log("grows started");"#);
        for folder in [&fine, &grows] {
            let installed = library::install_from_folder(&global, folder, Os::current(), &[]);
            assert!(installed.is_ok(), "{installed:?}");
        }
        std::fs::write(
            grows.join("addon.json"),
            r#"{"id":"grows","name":"Name","version":"1","permissions":["run_process"],"api":1}"#,
        )
        .unwrap();
        assert_eq!(runs(&service), vec![RunState::NotRunning, RunState::NotRunning]);
        service.start_installed();
        assert_eq!(runs(&service), vec![RunState::Running, RunState::NotRunning]);
        assert_eq!(addon_lines(&ctx), vec!["[fine] fine started".to_string()]);
    }

    fn has_entry(service: &AddonService, addon: &str, id: &str) -> bool {
        let menu = service.menu.lock().unwrap().items(true);
        menu.iter().any(|i| matches!(&i.id, MenuItemId::Addon { addon: owner, id: entry } if owner == addon && entry == id))
    }

    #[test]
    fn reloading_runs_the_changed_main_js_in_place_of_the_old_and_what_the_user_gave_stays() {
        let (service, ctx, root) = start("reload");
        let first = r#"host.log("version 1"); host.deviceMenu.register({ id: "one", label: "One", onClick: async () => {} });"#;
        let folder = make_script_addon(&root, "again", r#"["device_shell"]"#, first);
        assert!(service.install_from_folder(&folder, &[AddonPermission::DeviceShell]).is_ok());
        assert!(has_entry(&service, "again", "one"));
        let second = r#"host.log("version 2"); host.deviceMenu.register({ id: "two", label: "Two", onClick: async () => {} });"#;
        std::fs::write(folder.join("main.js"), second).unwrap();
        let reloaded = service.reload("again");
        assert_eq!(reloaded.map(|i| (i.status, i.granted)), Ok((library::AddonStatus::Ready, vec![AddonPermission::DeviceShell])));
        assert_eq!(runs(&service), vec![RunState::Running]);
        assert!(!has_entry(&service, "again", "one"), "the entry of the old main.js is still there");
        assert!(has_entry(&service, "again", "two"), "the entry of the new main.js is not there");
        assert!(addon_lines(&ctx).contains(&"[again] version 2".to_string()), "{:?}", addon_lines(&ctx));
    }

    #[test]
    fn switching_an_add_on_off_stops_it_and_takes_what_it_registered_and_switching_on_runs_it_anew() {
        let (service, ctx, root) = start("switch");
        let script = r#"host.log("started"); host.deviceMenu.register({ id: "a", label: "A", onClick: async () => {} });
            host.appMenu.register({ id: "a", label: "A", onClick: async () => {} });
            host.toolbar.register({ id: "a", label: "A", onClick: async () => {} });"#;
        let folder = make_script_addon(&root, "toggled", r#"["device_shell"]"#, script);
        assert!(service.install_from_folder(&folder, &[AddonPermission::DeviceShell]).is_ok());
        assert_eq!(runs(&service), vec![RunState::Running]);
        let button = ButtonId::Addon { addon: "toggled".into(), id: "a".into() };

        let off = service.set_enabled("toggled", false);
        assert_eq!(off.as_ref().map(|i| i.enabled), Ok(false), "{off:?}");
        assert_eq!(runs(&service), vec![RunState::Disabled]);
        assert!(!has_entry(&service, "toggled", "a"), "the entry of the device menu stayed");
        let app_id = AppMenuItemId::Addon { addon: "toggled".into(), id: "a".into() };
        assert!(!service.app_menu.lock().unwrap().items(true).iter().any(|i| i.id == app_id), "the entry of the app menu stayed");
        assert!(!service.sessions.toolbar_buttons(None).iter().any(|b| b.id == button), "the button stayed");
        let listed = service.installed();
        assert_eq!((listed[0].info.enabled, listed[0].info.granted.clone()), (false, vec![AddonPermission::DeviceShell]));

        // Reloading or deciding again does not switch it on.
        assert!(service.reload("toggled").is_ok());
        assert!(service.decide_permissions("toggled", &[AddonPermission::DeviceShell]).is_ok());
        assert_eq!(runs(&service), vec![RunState::Disabled]);
        assert!(!has_entry(&service, "toggled", "a"));

        let on = service.set_enabled("toggled", true);
        assert_eq!(on.as_ref().map(|i| i.enabled), Ok(true), "{on:?}");
        assert_eq!(runs(&service), vec![RunState::Running]);
        assert!(has_entry(&service, "toggled", "a"));
        assert!(service.sessions.toolbar_buttons(None).iter().any(|b| b.id == button));
        assert_eq!(addon_lines(&ctx).iter().filter(|l| l.as_str() == "[toggled] started").count(), 2, "once at install, once when switched on; never while off");
        assert_eq!(
            service.set_enabled("nobody", false).map(|_| ()),
            Err(CoreError::Addon(library::InstallError::NotInstalled))
        );
    }

    #[test]
    fn an_add_on_that_was_switched_off_does_not_start_with_the_program_and_runs_when_switched_on() {
        let (service, ctx, root) = start("switch-start");
        let folder = make_script_addon(&root, "quiet", "[]", r#"host.log("ran"); host.deviceMenu.register({ id: "a", label: "A", onClick: async () => {} });"#);
        assert!(service.install_from_folder(&folder, &[]).is_ok());
        assert!(service.set_enabled("quiet", false).is_ok());
        let before = addon_lines(&ctx).iter().filter(|l| l.as_str() == "[quiet] ran").count();
        // As when the program starts again: every installed add-on is started.
        service.start_installed();
        assert_eq!(runs(&service), vec![RunState::Disabled]);
        assert!(!has_entry(&service, "quiet", "a"));
        assert_eq!(addon_lines(&ctx).iter().filter(|l| l.as_str() == "[quiet] ran").count(), before, "main.js ran although it is off");
    }

    #[test]
    fn an_add_on_whose_changed_main_js_fails_says_why_and_runs_again_when_it_is_put_right() {
        let (service, _ctx, root) = start("reload-fails");
        let folder = make_script_addon(&root, "fragile", "[]", r#"host.deviceMenu.register({ id: "a", label: "A", onClick: async () => {} });"#);
        assert!(service.install_from_folder(&folder, &[]).is_ok());
        std::fs::write(folder.join("main.js"), r#"throw new Error("changed and broken");"#).unwrap();
        let reloaded = service.reload("fragile");
        assert!(reloaded.is_ok(), "the reload itself went well: {reloaded:?}");
        let run = runs(&service).remove(0);
        assert!(matches!(&run, RunState::Failed(StartFailure::Script(text)) if text.contains("changed and broken")), "{run:?}");
        assert!(!has_entry(&service, "fragile", "a"));
        std::fs::write(folder.join("main.js"), r#"host.deviceMenu.register({ id: "b", label: "B", onClick: async () => {} });"#).unwrap();
        assert!(service.reload("fragile").is_ok());
        assert_eq!(runs(&service), vec![RunState::Running]);
        assert!(has_entry(&service, "fragile", "b"));
    }

    #[test]
    fn an_add_on_that_now_asks_for_more_is_not_run_by_a_reload_and_one_that_is_not_installed_is_an_error() {
        let (service, ctx, root) = start("reload-grows");
        let folder = make_script_addon(&root, "grows", "[]", r#"host.log("first");"#);
        assert!(service.install_from_folder(&folder, &[]).is_ok());
        std::fs::write(folder.join("main.js"), r#"host.log("second");"#).unwrap();
        std::fs::write(
            folder.join("addon.json"),
            r#"{"id":"grows","name":"Name","version":"1","permissions":["run_process"],"api":1}"#,
        )
        .unwrap();
        let reloaded = service.reload("grows");
        assert_eq!(reloaded.map(|i| i.status), Ok(library::AddonStatus::PermissionsChanged));
        assert_eq!(runs(&service), vec![RunState::NotRunning]);
        assert!(!addon_lines(&ctx).contains(&"[grows] second".to_string()), "it ran: {:?}", addon_lines(&ctx));
        assert_eq!(service.reload("nobody").map(|_| ()), Err(CoreError::Addon(library::InstallError::NotInstalled)));
    }

    // ---- events

    use crate::core::model::{Device, DeviceState, Event, OpenedUpdate, TransferPhase, TransferUpdate};

    /// An add-on that writes down every event it is told of.
    const LISTENER: &str = r#"
        for (const name of ["device.online", "device.offline", "device.added", "screen.started", "screen.ended", "files.sent", "files.opened"]) {
            host.on(name, (e) => host.log(name + " " + JSON.stringify(e)));
        }
    "#;

    fn phone(state: DeviceState) -> Device {
        Device {
            id: "d1".into(),
            name: "Phone".into(),
            label: None,
            state,
            transport: None,
            adb_serial: None,
            address: None,
            android: None,
            sdk: None,
            known: true,
        }
    }

    /// The lines the add-on `listener` wrote.
    fn heard(ctx: &Ctx) -> Vec<String> {
        addon_lines(ctx).into_iter().filter_map(|l| l.strip_prefix("[listener] ").map(String::from)).collect()
    }

    fn wait_for_heard(ctx: &Ctx, line: &str) -> bool {
        wait_for_line(ctx, &format!("[listener] {line}"))
    }

    #[test]
    fn what_happens_to_the_devices_and_the_screens_is_told_to_the_add_ons_that_listen() {
        let (service, ctx, root) = start("events");
        // The state the changes are seen against.
        ctx.emit();
        let folder = make_script_addon(&root, "listener", "[]", LISTENER);
        assert!(service.install_from_folder(&folder, &[]).is_ok());
        ctx.with_state(|i| i.devices.push(phone(DeviceState::Online)));
        ctx.emit();
        assert!(wait_for_heard(&ctx, r#"device.added {"device":"d1","name":"Phone"}"#), "{:?}", heard(&ctx));
        ctx.with_state(|i| {
            i.sessions.insert("d1".into());
        });
        ctx.emit();
        assert!(wait_for_heard(&ctx, r#"screen.started {"app":null,"device":"d1"}"#), "{:?}", heard(&ctx));
        ctx.with_state(|i| {
            i.devices[0].state = DeviceState::Disconnected;
            i.sessions.clear();
        });
        ctx.emit();
        assert!(wait_for_heard(&ctx, r#"screen.ended {"app":null,"device":"d1"}"#), "{:?}", heard(&ctx));
        assert_eq!(
            heard(&ctx),
            vec![
                r#"device.online {"device":"d1","name":"Phone"}"#,
                r#"device.added {"device":"d1","name":"Phone"}"#,
                r#"screen.started {"app":null,"device":"d1"}"#,
                r#"device.offline {"device":"d1","name":"Phone"}"#,
                r#"screen.ended {"app":null,"device":"d1"}"#,
            ]
        );
    }

    #[test]
    fn a_file_that_was_sent_or_opened_is_told_to_the_add_ons_that_listen() {
        let (service, ctx, root) = start("events-files");
        let folder = make_script_addon(&root, "listener", "[]", LISTENER);
        assert!(service.install_from_folder(&folder, &[]).is_ok());
        let update = |phase| TransferUpdate { id: 1, device: "d1".into(), name: "a.txt".into(), sent: 5, total: 10, phase, error: None };
        // The progress of a sending is not an event; its end is.
        ctx.send(Event::Transfer(Box::new(update(TransferPhase::Sending))));
        ctx.send(Event::Transfer(Box::new(update(TransferPhase::Done))));
        ctx.send(Event::Opened(Box::new(OpenedUpdate {
            device: "d1".into(),
            file: "b.pdf".into(),
            remote: None,
            error: Some(CoreError::NoApp),
        })));
        assert!(wait_for_heard(&ctx, r#"files.opened {"device":"d1","error":"no_app","file":"b.pdf","ok":false}"#), "{:?}", heard(&ctx));
        assert_eq!(
            heard(&ctx),
            vec![
                r#"files.sent {"device":"d1","error":null,"name":"a.txt","ok":true}"#,
                r#"files.opened {"device":"d1","error":"no_app","file":"b.pdf","ok":false}"#,
            ]
        );
    }

    #[test]
    fn an_add_on_that_starts_is_told_of_what_is_the_case_already_the_usable_devices_and_the_open_screens() {
        let (service, ctx, root) = start("events-replay");
        ctx.with_state(|i| {
            i.devices.push(phone(DeviceState::Online));
            let mut asleep = phone(DeviceState::Disconnected);
            asleep.id = "d2".into();
            i.devices.push(asleep);
            i.sessions.insert("d1".into());
        });
        ctx.emit();
        let folder = make_script_addon(&root, "listener", "[]", LISTENER);
        assert!(service.install_from_folder(&folder, &[]).is_ok());
        assert!(wait_for_heard(&ctx, r#"screen.started {"app":null,"device":"d1"}"#), "{:?}", heard(&ctx));
        // The one that is asleep is not told; a device that was there is not "added" now.
        assert_eq!(
            heard(&ctx),
            vec![r#"device.online {"device":"d1","name":"Phone"}"#, r#"screen.started {"app":null,"device":"d1"}"#]
        );
        // After a reload it is told again, because the add-on starts again.
        assert!(service.reload("listener").is_ok());
        assert!(wait_for_line_count(&ctx, "[listener] screen.started {\"app\":null,\"device\":\"d1\"}", 2), "{:?}", heard(&ctx));
    }

    /// Waits (at most five seconds) for a line to have been written this many times.
    fn wait_for_line_count(ctx: &Ctx, line: &str, count: usize) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if addon_lines(ctx).iter().filter(|l| *l == line).count() >= count {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn a_removed_add_on_is_told_of_nothing_more() {
        let (service, ctx, root) = start("events-removed");
        ctx.emit();
        let folder = make_script_addon(&root, "listener", "[]", LISTENER);
        assert!(service.install_from_folder(&folder, &[]).is_ok());
        assert!(service.uninstall("listener"));
        let before = addon_lines(&ctx).len();
        ctx.with_state(|i| i.devices.push(phone(DeviceState::Online)));
        ctx.emit();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(addon_lines(&ctx).len(), before);
    }

    // ---- the menu of an app and the toolbar

    /// A device that is usable (with the serial adb knows it by, made up).
    fn usable_phone() -> Device {
        Device { adb_serial: Some("serial-1".into()), ..phone(DeviceState::Online) }
    }

    fn block_on<T>(work: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(work)
    }

    #[test]
    fn an_add_on_puts_an_entry_in_the_menu_of_the_apps_and_choosing_it_runs_its_function_with_the_device_and_the_app() {
        let (service, ctx, root) = start("app-menu");
        ctx.with_state(|i| i.devices.push(usable_phone()));
        let script = r#"host.appMenu.register({ id: "go", label: "Go there", onClick: async (device, app) => host.log("chosen " + device + " " + app) });"#;
        let folder = make_script_addon(&root, "menus", "[]", script);
        assert!(service.install_from_folder(&folder, &[]).is_ok());
        let id = AppMenuItemId::Addon { addon: "menus".into(), id: "go".into() };
        let items = service.app_menu("d1", "com.example.app").unwrap_or_default();
        assert_eq!(items.iter().find(|i| i.id == id).map(|i| (i.label.clone(), i.enabled)), Some((Some("Go there".to_string()), true)));
        assert_eq!(service.run_app_menu_item("d1", "com.example.app", &id), Ok(()));
        assert!(wait_for_line(&ctx, "[menus] chosen d1 com.example.app"), "{:?}", addon_lines(&ctx));
    }

    #[test]
    fn an_add_on_puts_a_button_on_the_toolbar_with_its_text_and_pressing_it_runs_its_function_with_the_device_and_the_app() {
        use crate::core::addons::buttons::ButtonId;
        let (service, ctx, root) = start("toolbar");
        ctx.with_state(|i| i.devices.push(usable_phone()));
        let script = r#"host.toolbar.register({ id: "tap", label: "Tap it", onClick: async (device, app) => host.log("pressed " + device + " " + app) });"#;
        let folder = make_script_addon(&root, "tools", "[]", script);
        assert!(service.install_from_folder(&folder, &[]).is_ok());
        let button = ButtonId::Addon { addon: "tools".into(), id: "tap".into() };
        let listed = service.sessions.toolbar_buttons(None);
        assert_eq!(
            listed.iter().find(|b| b.id == button).map(|b| b.label.clone()),
            Some(Some("Tap it".to_string())),
            "{listed:?}"
        );
        // The built-in buttons have no text of their own here.
        assert!(listed.iter().filter(|b| b.id != button).all(|b| b.label.is_none()));
        // On the screen of the device, and on the window of an app of it.
        assert_eq!(block_on(service.sessions.press_button("d1", &button)), Ok(()));
        assert!(wait_for_line(&ctx, "[tools] pressed d1 null"), "{:?}", addon_lines(&ctx));
        assert_eq!(block_on(service.sessions.press_button("d1#app:com.example.app", &button)), Ok(()));
        assert!(wait_for_line(&ctx, "[tools] pressed d1 com.example.app"), "{:?}", addon_lines(&ctx));
    }

    fn layout_of(buttons: &[(ButtonId, bool)]) -> crate::core::model::Settings {
        let prefs = buttons.iter().map(|(id, visible)| crate::core::addons::buttons::ButtonPref { id: id.clone(), visible: *visible }).collect();
        crate::core::model::Settings {
            toolbar: crate::core::model::ToolbarSettings { buttons: prefs, ..Default::default() },
            ..Default::default()
        }
    }

    /// An add-on folder that declares settings (a list as JSON text) and has this `main.js`.
    fn make_addon_with_settings(root: &Path, id: &str, settings: &str, script: &str) -> PathBuf {
        let folder = root.join("addons").join(id);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("addon.json"),
            format!(r#"{{"id":"{id}","name":"Name","version":"1","permissions":[],"api":1,"settings":{settings}}}"#),
        )
        .unwrap();
        std::fs::write(folder.join("main.js"), script).unwrap();
        folder
    }

    const TUNED: &str = r#"[
        {"id":"level","type":"number","label":"Level","default":5,"min":0,"max":10},
        {"id":"on","type":"boolean","label":"On","default":false}
    ]"#;

    #[test]
    fn an_add_on_that_declares_settings_has_them_with_their_defaults_and_what_the_user_sets_is_checked_and_kept() {
        use crate::core::addons::settings::ValueProblem;
        use serde_json::json;
        let (service, _ctx, root) = start("settings");
        let folder = make_addon_with_settings(&root, "tuned", TUNED, "// made up");
        assert!(service.install_from_folder(&folder, &[]).is_ok());

        let view = service.settings_of("tuned").expect("the settings of an installed add-on");
        assert_eq!(view.specs.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["level", "on"]);
        assert_eq!(view.values.get("level"), Some(&SettingValue::Number(5)));
        assert_eq!(view.values.get("on"), Some(&SettingValue::Boolean(false)));

        // What is allowed is kept, and read back (by the page, and by the add-on).
        assert_eq!(service.set_setting("tuned", "level", &json!(8)), Ok(SettingValue::Number(8)));
        assert_eq!(service.settings_of("tuned").unwrap().values.get("level"), Some(&SettingValue::Number(8)));
        assert_eq!(service.setting_value("tuned", "level"), Some(SettingValue::Number(8)));
        assert_eq!(service.setting_value("tuned", "on"), Some(SettingValue::Boolean(false)), "a default is a value too");
        assert_eq!(service.setting_value("tuned", "nothing"), None);

        // What is not allowed is refused with the reason, and nothing changes.
        assert_eq!(service.set_setting("tuned", "level", &json!(11)), Err(CoreError::InvalidSetting(ValueProblem::OutOfRange)));
        assert_eq!(service.set_setting("tuned", "level", &json!("x")), Err(CoreError::InvalidSetting(ValueProblem::WrongType)));
        assert_eq!(service.set_setting("tuned", "gone", &json!(1)), Err(CoreError::UnknownSetting));
        assert_eq!(service.setting_value("tuned", "level"), Some(SettingValue::Number(8)));
        let missing = CoreError::Addon(library::InstallError::NotInstalled);
        assert_eq!(service.set_setting("nobody", "level", &json!(1)), Err(missing.clone()));
        assert_eq!(service.settings_of("nobody").map(|_| ()), Err(missing));

        // The add-on is changed so that what was kept does not fit any more: that setting is its default, the others stay.
        assert_eq!(service.set_setting("tuned", "on", &json!(true)), Ok(SettingValue::Boolean(true)));
        make_addon_with_settings(&root, "tuned", &TUNED.replace(r#""max":10"#, r#""max":5"#), "// made up");
        let after = service.settings_of("tuned").unwrap();
        assert_eq!(after.values.get("level"), Some(&SettingValue::Number(5)));
        assert_eq!(after.values.get("on"), Some(&SettingValue::Boolean(true)));
    }

    #[test]
    fn the_add_on_that_runs_is_told_when_the_user_changes_one_of_its_settings_and_only_when_a_change_was_kept() {
        use serde_json::json;
        let (service, ctx, root) = start("settings-told");
        let script = r#"host.on("settings.changed", (d) => host.log("changed " + d.key + "=" + d.value));"#;
        let folder = make_addon_with_settings(&root, "tuned", TUNED, script);
        assert!(service.install_from_folder(&folder, &[]).is_ok());
        assert_eq!(service.set_setting("tuned", "level", &json!(9)), Ok(SettingValue::Number(9)));
        assert!(wait_for_line(&ctx, "[tuned] changed level=9"), "{:?}", addon_lines(&ctx));
        // A change that is refused is not told.
        assert!(service.set_setting("tuned", "level", &json!(99)).is_err());
        assert_eq!(service.set_setting("tuned", "on", &json!(true)), Ok(SettingValue::Boolean(true)));
        assert!(wait_for_line(&ctx, "[tuned] changed on=true"), "{:?}", addon_lines(&ctx));
        let told: Vec<String> = addon_lines(&ctx).into_iter().filter(|l| l.contains("changed")).collect();
        assert_eq!(told, ["[tuned] changed level=9", "[tuned] changed on=true"]);
    }

    #[test]
    fn an_add_on_that_runs_learns_the_folder_it_is_installed_from() {
        let (service, ctx, root) = start("folder");
        let folder = make_script_addon(&root, "where", "[]", r#"host.log(host.addon.folder());"#);
        let installed = service.install_from_folder(&folder, &[]).expect("installed");
        let path = installed.folder.to_str().expect("the folder is made of text").to_string();
        assert!(wait_for_line(&ctx, &format!("[where] {path}")), "{:?}", addon_lines(&ctx));
    }

    #[test]
    fn the_toolbar_shows_the_buttons_the_user_arranged_in_the_order_they_arranged_and_the_settings_list_has_them_all() {
        let (service, ctx, root) = start("layout");
        let folder = make_script_addon(
            &root,
            "tools",
            "[]",
            r#"host.toolbar.register({ id: "tap", label: "Tap it", onClick: async () => {} });"#,
        );
        assert!(service.install_from_folder(&folder, &[]).is_ok());
        let tap = ButtonId::Addon { addon: "tools".into(), id: "tap".into() };
        let ids = |service: &AddonService| service.sessions.toolbar_buttons(None).into_iter().map(|b| b.id).collect::<Vec<_>>();
        let before = ids(&service);
        assert_eq!(&before[..2], &[ButtonId::Back, ButtonId::Home], "the default order, the add-on's button last: {before:?}");
        assert_eq!(before.last(), Some(&tap));

        // The user puts the add-on's button first, hides Home and moves Rotate to the second place.
        ctx.use_settings(layout_of(&[(tap.clone(), true), (ButtonId::Rotate, true), (ButtonId::Home, false)]));
        let after = ids(&service);
        assert_eq!(&after[..2], &[tap.clone(), ButtonId::Rotate]);
        assert!(!after.contains(&ButtonId::Home), "a hidden button is not on the toolbar");
        assert_eq!(after.len(), before.len() - 1);
        // The button of an add-on has the text the add-on gave it, in the toolbar and in the list of the settings.
        let listed = service.sessions.toolbar_buttons(None);
        assert_eq!(listed[0].label.as_deref(), Some("Tap it"));

        // The list in the settings has every button, the hidden one too, in the same order.
        let layout = service.sessions.toolbar_layout();
        assert_eq!(layout.len(), before.len());
        assert_eq!(layout.iter().map(|e| e.id.clone()).take(3).collect::<Vec<_>>(), vec![tap, ButtonId::Rotate, ButtonId::Home]);
        assert_eq!(layout.iter().find(|e| e.id == ButtonId::Home).map(|e| e.visible), Some(false));
        assert_eq!(layout[0].label.as_deref(), Some("Tap it"));
        assert!(layout.iter().filter(|e| e.visible).map(|e| e.id.clone()).eq(after.iter().cloned()), "what is shown is what the list says is shown");

        // The add-on is gone: its button leaves the toolbar and the list, and its place in the layout is kept for when it is back.
        assert!(service.uninstall("tools"));
        assert!(service.sessions.toolbar_layout().iter().all(|e| !matches!(e.id, ButtonId::Addon { .. })));
    }

    #[test]
    fn a_button_of_an_add_on_is_not_pressed_for_a_device_that_is_not_usable_and_goes_with_its_add_on() {
        use crate::core::addons::buttons::ButtonId;
        let (service, ctx, root) = start("toolbar-gone");
        ctx.with_state(|i| {
            let mut asleep = usable_phone();
            asleep.state = DeviceState::Disconnected;
            asleep.adb_serial = None;
            i.devices.push(asleep);
        });
        let script = r#"
            host.toolbar.register({ id: "tap", label: "Tap", onClick: async () => host.log("pressed") });
            host.appMenu.register({ id: "go", label: "Go", onClick: async () => host.log("chosen") });
        "#;
        let folder = make_script_addon(&root, "tools", "[]", script);
        assert!(service.install_from_folder(&folder, &[]).is_ok());
        let button = ButtonId::Addon { addon: "tools".into(), id: "tap".into() };
        assert_eq!(block_on(service.sessions.press_button("d1", &button)), Err(CoreError::NotOnline));
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(!addon_lines(&ctx).iter().any(|l| l == "[tools] pressed"), "{:?}", addon_lines(&ctx));
        // Removing the add-on takes its button and its entry away.
        assert!(service.uninstall("tools"));
        assert!(!service.sessions.toolbar_buttons(None).iter().any(|b| b.id == button));
        let id = AppMenuItemId::Addon { addon: "tools".into(), id: "go".into() };
        assert!(!service.app_menu.lock().unwrap().items(true).iter().any(|i| i.id == id));
    }
}
