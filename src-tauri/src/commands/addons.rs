//! The add-ons: the entries they put on devices and on apps, and installing and removing them.

use crate::Shared;
use crate::core::addons::library::AddonInfo;
use crate::core::addons::runner::InstalledAddon;
use crate::core::addons::manifest::AddonManifest;
use crate::core::addons::menu::{AppMenuItem, AppMenuItemId, MenuItem, MenuItemId};
use crate::core::addons::permission::AddonPermission;
use crate::core::addons::settings::{AddonSettingsView, SettingValue};
use crate::core::error::CoreError;
use std::path::PathBuf;
use tauri::State;

#[tauri::command]
pub fn device_menu(core: State<Shared>, device: String) -> Result<Vec<MenuItem>, CoreError> {
    core.addons().device_menu(&device)
}

#[tauri::command]
pub fn run_menu_item(core: State<Shared>, device: String, item: MenuItemId) -> Result<(), CoreError> {
    core.addons().run_menu_item(&device, &item)
}

#[tauri::command]
pub fn app_menu(core: State<Shared>, device: String, package: String) -> Result<Vec<AppMenuItem>, CoreError> {
    core.addons().app_menu(&device, &package)
}

#[tauri::command]
pub fn run_app_menu_item(
    core: State<Shared>,
    device: String,
    package: String,
    item: AppMenuItemId,
) -> Result<(), CoreError> {
    core.addons().run_app_menu_item(&device, &package, &item)
}

/// Asks the user for the folder of an add-on with the system's own dialog; `None` if they cancel.
/// The page gets the path from here and cannot ask for any other dialog.
#[cfg(desktop)]
#[tauri::command]
pub async fn choose_addon_folder<R: tauri::Runtime>(
    window: tauri::Window<R>,
    title: String,
) -> Result<Option<PathBuf>, CoreError> {
    use tauri_plugin_dialog::DialogExt;
    let picked = window.dialog().file().set_title(title).set_parent(&window).blocking_pick_folder();
    match picked {
        None => Ok(None),
        Some(path) => path.into_path().map(Some).map_err(|e| CoreError::Other(e.to_string())),
    }
}

/// Where there is no such dialog (the add-ons are not installed from a folder there).
#[cfg(not(desktop))]
#[tauri::command]
pub async fn choose_addon_folder(_title: String) -> Result<Option<PathBuf>, CoreError> {
    Err(CoreError::Unavailable)
}

/// The installed add-ons, each read again from its folder.
#[tauri::command]
pub fn list_addons(core: State<Shared>) -> Vec<InstalledAddon> {
    core.addons().installed()
}

/// What the add-on in a folder is and asks for, shown before it is installed.
#[tauri::command]
pub fn inspect_addon_folder(core: State<Shared>, folder: PathBuf) -> Result<AddonManifest, CoreError> {
    core.addons().inspect_folder(&folder)
}

/// Runs what starts or stops an add-on on a thread that may wait (starting one waits for its `main.js`),
/// so that the window is not held.
async fn blocking<T: Send + 'static>(
    core: &Shared,
    work: impl FnOnce(&crate::core::Core) -> T + Send + 'static,
) -> Result<T, CoreError> {
    let core = core.clone();
    tauri::async_runtime::spawn_blocking(move || work(&core))
        .await
        .map_err(|e| CoreError::Other(e.to_string()))
}

/// Installs the add-on in a folder, with the permissions the user gave, and starts it.
#[tauri::command]
pub async fn install_addon(
    core: State<'_, Shared>,
    folder: PathBuf,
    granted: Vec<AddonPermission>,
) -> Result<AddonInfo, CoreError> {
    blocking(&core, move |core| core.addons().install_from_folder(&folder, &granted)).await?
}

/// The user decides again which permissions an installed add-on has; it starts again with them.
#[tauri::command]
pub async fn decide_addon_permissions(
    core: State<'_, Shared>,
    id: String,
    granted: Vec<AddonPermission>,
) -> Result<AddonInfo, CoreError> {
    blocking(&core, move |core| core.addons().decide_permissions(&id, &granted)).await?
}

/// The settings an installed add-on declares, and the value of each now.
#[tauri::command]
pub fn addon_settings(core: State<'_, Shared>, id: String) -> Result<AddonSettingsView, CoreError> {
    core.addons().settings_of(&id)
}

/// The user sets one setting of an add-on; the value is checked, kept, and the add-on that runs is told.
#[tauri::command]
pub fn set_addon_setting(
    core: State<'_, Shared>,
    id: String,
    key: String,
    value: serde_json::Value,
) -> Result<SettingValue, CoreError> {
    core.addons().set_setting(&id, &key, &value)
}

/// Reads an installed add-on from its folder again and starts it again.
#[tauri::command]
pub async fn reload_addon(core: State<'_, Shared>, id: String) -> Result<AddonInfo, CoreError> {
    blocking(&core, move |core| core.addons().reload(&id)).await?
}

/// Switches an installed add-on off (it stops, what it registered goes) or on (it starts again).
#[tauri::command]
pub async fn set_addon_enabled(core: State<'_, Shared>, id: String, enabled: bool) -> Result<AddonInfo, CoreError> {
    blocking(&core, move |core| core.addons().set_enabled(&id, enabled)).await?
}

/// Stops and removes an installed add-on (its folder and its data stay); whether there was one.
#[tauri::command]
pub async fn uninstall_addon(core: State<'_, Shared>, id: String) -> Result<bool, CoreError> {
    blocking(&core, move |core| core.addons().uninstall(&id)).await
}
