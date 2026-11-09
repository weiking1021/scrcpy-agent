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

#[tauri::command]
pub fn host_platform() -> &'static str { std::env::consts::OS }

pub fn explorer_texts<R: tauri::Runtime>(app: &AppHandle<R>) -> MenuTexts {
    app.try_state::<ExplorerTexts>().map(|t| t.0.lock().unwrap().clone()).unwrap_or_default()
}

#[tauri::command]
pub async fn save_settings<R: tauri::Runtime>(
    app: AppHandle<R>,
    core: State<'_, Shared>,
    settings: Settings,
) -> Result<(), CoreError> {
    let before = core.snapshot().settings;
    let (wanted, disabled) = (settings.explorer_menu, settings.disabled_file_managers.clone());
    let login = settings.launch_at_login;
    let modern = settings.windows11_explorer_menu;
    let widget = settings.windows11_widget;
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
    if modern != before.windows11_explorer_menu {
        let directory = core.store().windows11_menu_dir();
        let texts = explorer_texts(&app);
        let result = tauri::async_runtime::spawn_blocking(move || shell_menu::apply_windows11(modern, &directory, &texts))
            .await.map_err(|e| CoreError::ExplorerMenu(e.to_string()))
            .and_then(|result| result);
        if let Err(error) = result {
            let mut kept = core.snapshot().settings;
            kept.windows11_explorer_menu = before.windows11_explorer_menu;
            let _ = core.save_settings(kept);
            return Err(error);
        }
    }
    if widget != before.windows11_widget {
        let directory = core.store().windows_widget_dir();
        let texts = crate::widget::texts(&app);
        let pipe = crate::widget::pipe_name(&directory);
        let result = tauri::async_runtime::spawn_blocking(move || crate::widget::apply(widget, &directory, &texts, &pipe))
            .await.map_err(|e| CoreError::WindowsWidget(e.to_string())).and_then(|result| result);
        if let Err(error) = result {
            let mut kept = core.snapshot().settings;
            kept.windows11_widget = before.windows11_widget;
            let _ = core.save_settings(kept);
            return Err(error);
        }
    }
    Ok(())
}

#[tauri::command]
pub fn windows_widget_available() -> bool { crate::widget::available() }

/// The widget uses the same resolved language pack (including user packs) as the UI.
#[tauri::command]
pub async fn set_windows_widget_texts<R: tauri::Runtime>(
    app: AppHandle<R>, core: State<'_, Shared>, texts: crate::widget::Texts,
) -> Result<(), CoreError> {
    if !crate::widget::available() { return Ok(()); }
    let mut resolved = crate::widget::defaults();
    resolved.extend(texts);
    {
        let state = app.state::<crate::widget::WidgetTexts>();
        let mut current = state.0.lock().unwrap();
        if *current == resolved { return Ok(()); }
        *current = resolved.clone();
    }
    let directory = core.store().windows_widget_dir();
    std::fs::create_dir_all(&directory).map_err(|e| CoreError::WindowsWidget(e.to_string()))?;
    std::fs::write(directory.join("texts.json"), serde_json::to_vec(&resolved).map_err(|e| CoreError::WindowsWidget(e.to_string()))?)
        .map_err(|e| CoreError::WindowsWidget(e.to_string()))?;
    if core.snapshot().settings.windows11_widget {
        let pipe = crate::widget::pipe_name(&directory);
        tauri::async_runtime::spawn_blocking(move || crate::widget::apply(true, &directory, &resolved, &pipe))
            .await.map_err(|e| CoreError::WindowsWidget(e.to_string()))??;
    }
    Ok(())
}

/// The texts of the file manager's menu; if the menu is registered it is written again with them.
#[tauri::command]
pub async fn set_explorer_menu_texts<R: tauri::Runtime>(
    app: AppHandle<R>,
    core: State<'_, Shared>,
    texts: MenuTexts,
) -> Result<(), CoreError> {
    if let Some(state) = app.try_state::<ExplorerTexts>() {
        *state.0.lock().unwrap() = texts.clone();
    }
    let settings = core.snapshot().settings;
    if settings.explorer_menu { shell_menu::apply(true, &settings.disabled_file_managers, &texts)?; }
    if settings.windows11_explorer_menu {
        let directory = core.store().windows11_menu_dir();
        tauri::async_runtime::spawn_blocking(move || shell_menu::apply_windows11(true, &directory, &texts))
            .await.map_err(|e| CoreError::ExplorerMenu(e.to_string()))??;
    }
    Ok(())
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

#[tauri::command]
pub fn windows11_menu_available() -> bool {
    shell_menu::windows11_available()
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
