//! What a running add-on may ask of the program. The engine only knows this; the program implements
//! it, and the tests of the engine use a fake.

use crate::core::addons::buttons::ButtonHandler;
use crate::core::addons::menu::{AppMenuHandler, MenuHandler, MenuRegisterError};
use crate::core::addons::permission::AddonPermission;
use crate::core::addons::process::{ProcessId, ProcessListener, ProcessSpec, SpawnError};
use crate::core::addons::screen_pipe::PipeError;
use crate::core::addons::settings::SettingValue;
use crate::core::error::CoreError;
use std::future::Future;
use std::pin::Pin;

/// A result that comes later.
pub type HostFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// How serious a line of an add-on's log is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddonLogLevel {
    Info,
    Warn,
    Error,
}

pub trait AddonHost: Send + Sync + 'static {
    /// A line of the add-on for the debug log (already as short as the limit says).
    fn log(&self, addon: &str, level: AddonLogLevel, text: &str);

    /// Puts an entry in the menu of the device cards. `handler` runs the add-on's function.
    fn register_device_menu_item(
        &self,
        addon: &str,
        id: &str,
        label: &str,
        handler: MenuHandler,
    ) -> Result<(), MenuRegisterError>;

    /// Puts an entry in the menu of an app. `handler` runs the add-on's function, given the device and the package.
    fn register_app_menu_item(
        &self,
        addon: &str,
        id: &str,
        label: &str,
        handler: AppMenuHandler,
    ) -> Result<(), MenuRegisterError>;

    /// Puts a button on the toolbar. `handler` runs the add-on's function, given the device and the package of
    /// the app the toolbar belongs to (none for the screen of the device).
    fn register_toolbar_button(
        &self,
        addon: &str,
        id: &str,
        label: &str,
        handler: ButtonHandler,
    ) -> Result<(), MenuRegisterError>;

    /// Takes away everything the add-on registered.
    fn remove_registrations(&self, addon: &str);

    /// Runs a shell command on a device (the add-on has the permission: that is checked before).
    fn device_shell(&self, addon: &str, device: &str, command: &str) -> HostFuture<Result<String, CoreError>>;

    /// Starts a program for the add-on (the add-on has the permission: that is checked before). `listener` is told
    /// of what the program does.
    fn spawn_process(
        &self,
        addon: &str,
        spec: ProcessSpec,
        listener: ProcessListener,
    ) -> Result<ProcessId, SpawnError>;

    /// Ends a program of the add-on; whether there was one running.
    fn kill_process(&self, addon: &str, id: ProcessId) -> bool;

    /// Closes the input of a program of the add-on; whether there was one open.
    fn close_process_input(&self, addon: &str, id: ProcessId) -> bool;

    /// Feeds the live video of a device's screen to a program of the add-on (the add-on has the permission: that
    /// is checked before).
    /// `app` is the package for the window of an app on the device, none for the screen of the device itself.
    fn pipe_screen(&self, addon: &str, device: &str, app: Option<&str>, id: ProcessId) -> Result<(), PipeError>;

    /// The value of a setting of the add-on now (what the user set, else the default); `None` for a setting it did not
    /// declare.
    fn setting(&self, addon: &str, key: &str) -> Option<SettingValue>;

    /// The folder the add-on is installed from (where its `addon.json` and `main.js` are); `None` when it is not
    /// installed any more.
    fn folder(&self, addon: &str) -> Option<std::path::PathBuf>;

    /// The add-on called something that needs a permission it was not given.
    fn permission_denied(&self, addon: &str, permission: AddonPermission);
}
