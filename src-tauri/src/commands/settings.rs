//! The settings, and what follows from them on this computer (the file
//! manager's menu, the language packs).

use crate::Shared;
use crate::core::error::CoreError;
use crate::core::integration::explorer_menu::MenuTexts;
use crate::core::integration::file_manager_menu::FileManager;
use crate::core::model::Settings;
use crate::core::store::LocalePack;
use crate::shell_menu;
use tauri::{AppHandle, Manager, State};

/// The texts of the file manager's menu in the user's language, as the UI last sent them.
pub struct ExplorerTexts(pub std::sync::Mutex<MenuTexts>);

pub fn explorer_texts<R: tauri::Runtime>(app: &AppHandle<R>) -> MenuTexts {
    app.try_state::<ExplorerTexts>().map(|t| t.0.lock().unwrap().clone()).unwrap_or_default()
}

#[tauri::command]
pub fn save_settings<R: tauri::Runtime>(
    app: AppHandle<R>,
    core: State<Shared>,
    settings: Settings,
) -> Result<(), CoreError> {
    let before = core.snapshot().settings;
    let (wanted, disabled) = (settings.explorer_menu, settings.disabled_file_managers.clone());
    let login = settings.launch_at_login;
    core.save_settings(settings)?;
    // The start at the login is written or removed as the setting changes, at once.
    if login != before.launch_at_login
        && let Err(error) = crate::login_start::apply(login)
    {
        // The setting says what is true.
        let mut kept = core.snapshot().settings;
        kept.launch_at_login = before.launch_at_login;
        let _ = core.save_settings(kept);
        return Err(error);
    }
    // The menu is written or removed as the setting changes, at once.
    if (wanted, &disabled) != (before.explorer_menu, &before.disabled_file_managers)
        && let Err(error) = shell_menu::apply(wanted, &disabled, &explorer_texts(&app))
    {
        // The setting says what is true.
        let mut kept = core.snapshot().settings;
        kept.explorer_menu = before.explorer_menu;
        kept.disabled_file_managers = before.disabled_file_managers;
        let _ = core.save_settings(kept);
        return Err(error);
    }
    Ok(())
}

/// The texts of the file manager's menu; if the menu is registered it is written again with them.
#[tauri::command]
pub fn set_explorer_menu_texts<R: tauri::Runtime>(
    app: AppHandle<R>,
    core: State<Shared>,
    texts: MenuTexts,
) -> Result<(), CoreError> {
    if let Some(state) = app.try_state::<ExplorerTexts>() {
        *state.0.lock().unwrap() = texts.clone();
    }
    let settings = core.snapshot().settings;
    if settings.explorer_menu { shell_menu::apply(true, &settings.disabled_file_managers, &texts) } else { Ok(()) }
}

/// Whether this system has a start at the login the settings can turn on.
#[tauri::command]
pub fn launch_at_login_available() -> bool {
    crate::login_start::available()
}

#[tauri::command]
pub fn explorer_menu_available() -> bool {
    shell_menu::available()
}

/// The file managers of this system that the menu can be written for (none on Windows,
/// which has the one Explorer); the settings window lists them.
#[tauri::command]
pub fn detected_file_managers() -> Vec<FileManager> {
    shell_menu::detected_file_managers()
}

/// Language packs the user dropped into the folder of the language packs.
#[tauri::command]
pub fn list_user_locales(core: State<Shared>) -> Vec<LocalePack> {
    core.store().locale_packs()
}

/// Shows the folder of the language packs (made if it is not there) in the file manager.
#[tauri::command]
pub fn reveal_data_dir(core: State<Shared>) -> Result<(), CoreError> {
    let dir = core.store().ensure_locales_dir()?;
    tauri_plugin_opener::reveal_item_in_dir(dir).map_err(|e| CoreError::Other(e.to_string()))
}
