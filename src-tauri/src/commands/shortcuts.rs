//! Shortcuts of an app on the desktop and in the menu of applications.

use crate::Shared;
use crate::core::apps::PackageName;
use crate::core::error::CoreError;
use crate::core::integration::file_manager_menu::data_home;
use crate::core::integration::shortcuts::{
    ShortcutKind, ShortcutPlace, ShortcutRecord, ShortcutRecords, arguments, display_name, file_name,
};
use crate::core::store;
use crate::core::window::layout::Os;
use crate::shell_menu;
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Manager, State};

/// The folders shortcuts are written to, the kind of shortcut and the program they start.
#[derive(Clone)]
pub struct ShortcutPlaces {
    pub kind: ShortcutKind,
    pub desktop: PathBuf,
    /// Where the shortcuts of the menu of applications go: on Windows a folder of this
    /// program in the Start menu, on Linux the folder of applications of the user.
    pub start_menu: PathBuf,
    /// The folder of the menu is this program's own: it is removed with its last shortcut.
    /// (On Linux it is the folder of every application of the user, and is never removed.)
    pub owns_start_menu_folder: bool,
    pub exe: PathBuf,
}

impl ShortcutPlaces {
    /// The folders of the user on this system; `None` where there are none.
    pub fn of_user<R: tauri::Runtime>(app: &AppHandle<R>) -> Option<Self> {
        let kind = shell_menu::shortcut_kind()?;
        let desktop = app.path().desktop_dir().ok()?;
        let exe = std::env::current_exe().ok()?;
        let (start_menu, owns_start_menu_folder) = match Os::current() {
            Os::Windows => {
                let programs = std::env::var_os("APPDATA")
                    .map(|a| PathBuf::from(a).join("Microsoft").join("Windows").join("Start Menu").join("Programs"))?;
                (programs.join("scrcpy-agent"), true)
            }
            _ => {
                let home = data_home(std::env::var_os("XDG_DATA_HOME").as_deref(), std::env::var_os("HOME").as_deref())?;
                (home.join("applications"), false)
            }
        };
        Some(Self { kind, desktop, start_menu, owns_start_menu_folder, exe })
    }
}

/// The kind of shortcut that can be made here, or `None` where none can (the menu hides its
/// entries then); the menu words its entries by the kind (the menu of applications is the
/// Start menu on Windows).
#[tauri::command]
pub fn shortcuts_available<R: tauri::Runtime>(app: AppHandle<R>) -> Option<ShortcutKind> {
    app.try_state::<ShortcutPlaces>().map(|places| places.kind)
}

/// Writes a shortcut that opens the app, and returns the path of the file.
#[tauri::command]
pub fn create_app_shortcut<R: tauri::Runtime>(
    app: AppHandle<R>,
    core: State<Shared>,
    device: String,
    package: String,
    place: ShortcutPlace,
) -> Result<String, CoreError> {
    let package = PackageName::parse(&package).ok_or(CoreError::InvalidPackage)?;
    let places = app.try_state::<ShortcutPlaces>().ok_or(CoreError::Unavailable)?;
    let scope = core.store().device_of(&device).ok_or(CoreError::UnknownDevice)?;
    let mut records: ShortcutRecords = scope.load::<store::Shortcuts>();
    let previous = existing_shortcut(&places, &core, &device, &package, place, &records)?;
    let target = shortcut_target(&places, &core, &device, &package, place)?;
    let arguments = arguments(&device, &package);
    let path = shell_menu::create_shortcut(
        places.kind,
        &target.folder,
        &places.exe,
        &target.file_name,
        &target.display_name,
        &arguments,
    )?;
    // One shortcut of an app in a place: what was made before, under the name the device had then, goes.
    if let Some(old) = previous.filter(|old| *old != path) {
        shell_menu::remove_shortcut_file(&old)?;
    }
    records.set(ShortcutRecord { package: package.as_str().to_string(), place, path: path.display().to_string() });
    scope.save::<store::Shortcuts>(&records)?;
    Ok(path.display().to_string())
}

/// The file of the shortcut of an app in a place, if there is one now: the one that was made (the
/// record of it has its path, whatever the device is called now), else one with the name it would
/// get today (made before there were records).
fn existing_shortcut(
    places: &ShortcutPlaces,
    core: &Shared,
    device: &str,
    package: &PackageName,
    place: ShortcutPlace,
    records: &ShortcutRecords,
) -> Result<Option<PathBuf>, CoreError> {
    if let Some(path) = records.find(package.as_str(), place).map(|r| PathBuf::from(&r.path)).filter(|p| p.is_file()) {
        return Ok(Some(path));
    }
    let target = shortcut_target(places, core, device, package, place)?;
    let path = target.folder.join(&target.file_name);
    Ok(path.is_file().then_some(path))
}

/// Where the shortcut of an app on a device is, and what it is called.
struct ShortcutTarget {
    folder: PathBuf,
    file_name: String,
    display_name: String,
}

fn shortcut_target(
    places: &ShortcutPlaces,
    core: &Shared,
    device: &str,
    package: &PackageName,
    place: ShortcutPlace,
) -> Result<ShortcutTarget, CoreError> {
    let shown = core
        .snapshot()
        .devices
        .iter()
        .find(|d| d.id == device)
        .map(|d| d.label.clone().unwrap_or_else(|| d.name.clone()))
        .ok_or(CoreError::UnknownDevice)?;
    let folder = match place {
        ShortcutPlace::Desktop => places.desktop.clone(),
        ShortcutPlace::StartMenu => places.start_menu.clone(),
    };
    let app = core.apps().app_name(device, package);
    Ok(ShortcutTarget { folder, file_name: file_name(&app, &shown, places.kind), display_name: display_name(&app, &shown) })
}

/// Which places have a shortcut of an app now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ShortcutState {
    pub desktop: bool,
    pub start_menu: bool,
}

#[tauri::command]
pub fn app_shortcut_state<R: tauri::Runtime>(
    app: AppHandle<R>,
    core: State<Shared>,
    device: String,
    package: String,
) -> Result<ShortcutState, CoreError> {
    let package = PackageName::parse(&package).ok_or(CoreError::InvalidPackage)?;
    let places = app.try_state::<ShortcutPlaces>().ok_or(CoreError::Unavailable)?;
    let scope = core.store().device_of(&device).ok_or(CoreError::UnknownDevice)?;
    let records = scope.load::<store::Shortcuts>();
    let has = |place| -> Result<bool, CoreError> {
        Ok(existing_shortcut(&places, &core, &device, &package, place, &records)?.is_some())
    };
    Ok(ShortcutState { desktop: has(ShortcutPlace::Desktop)?, start_menu: has(ShortcutPlace::StartMenu)? })
}

/// Removes the shortcut of an app from a place; one that is not there is not an error.
#[tauri::command]
pub fn remove_app_shortcut<R: tauri::Runtime>(
    app: AppHandle<R>,
    core: State<Shared>,
    device: String,
    package: String,
    place: ShortcutPlace,
) -> Result<(), CoreError> {
    let package = PackageName::parse(&package).ok_or(CoreError::InvalidPackage)?;
    let places = app.try_state::<ShortcutPlaces>().ok_or(CoreError::Unavailable)?;
    let scope = core.store().device_of(&device).ok_or(CoreError::UnknownDevice)?;
    let mut records = scope.load::<store::Shortcuts>();
    let existing = existing_shortcut(&places, &core, &device, &package, place, &records)?;
    if let Some(path) = &existing {
        shell_menu::remove_shortcut_file(path)?;
    }
    if records.remove(package.as_str(), place).is_some() {
        scope.save::<store::Shortcuts>(&records)?;
    }
    // The Start menu folder of this program goes with its last shortcut; the desktop and the folder
    // of the applications of the user (on Linux) are never removed.
    if place == ShortcutPlace::StartMenu
        && places.owns_start_menu_folder
        && let Some(folder) = existing.as_deref().and_then(std::path::Path::parent)
    {
        shell_menu::remove_folder_if_empty(folder)?;
    }
    Ok(())
}
