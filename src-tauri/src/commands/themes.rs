//! The looks of the windows.

use crate::Shared;
use crate::core::error::CoreError;
use crate::core::theme::service::{AppliedTheme, ThemeList};
use tauri::http::{Response, StatusCode, header};
use tauri::{AppHandle, Manager, State};

/// Every theme there is, and the folders of the user's themes that are not used.
#[tauri::command]
pub fn list_themes(core: State<Shared>) -> ThemeList {
    core.themes().list()
}

/// The theme the settings choose, with its stylesheet: every window asks for it when it opens
/// and again whenever the setting changes.
#[tauri::command]
pub fn current_theme(core: State<Shared>) -> AppliedTheme {
    core.themes().applied()
}

/// What the address `theme://` gives: the file of a theme that its path names (see `ThemeUrl`), or
/// nothing found. Not kept by the window (the file may be edited), and open to the page of the
/// program, which is at another address (a font is only used from another address if it says so).
pub fn serve<R: tauri::Runtime>(app: &AppHandle<R>, path: &str) -> Response<Vec<u8>> {
    let found = app.try_state::<Shared>().and_then(|core| core.themes().serve(path));
    let (status, content_type, body) = match found {
        Some(asset) => (StatusCode::OK, asset.content_type, asset.bytes),
        None => (StatusCode::NOT_FOUND, "text/plain; charset=utf-8", Vec::new()),
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(body)
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

/// Shows the folder of the user's themes (made if it is not there) in the file manager.
#[tauri::command]
pub fn reveal_themes_dir(core: State<Shared>) -> Result<(), CoreError> {
    let dir = core.store().ensure_themes_dir()?;
    tauri_plugin_opener::reveal_item_in_dir(dir).map_err(|e| CoreError::Other(e.to_string()))
}
