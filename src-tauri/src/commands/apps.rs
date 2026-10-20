//! The apps of a device.

use crate::Shared;
use crate::core::apps::{AppCache, AppDisplay, AppEntry};
use crate::core::error::CoreError;
use crate::core::model::AppUsage;
use tauri::{AppHandle, State};

#[tauri::command]
pub async fn list_apps(core: State<'_, Shared>, device: String) -> Result<Vec<AppEntry>, CoreError> {
    core.apps().list_apps(&device).await
}

#[tauri::command]
pub async fn open_app<R: tauri::Runtime>(
    app: AppHandle<R>,
    core: State<'_, Shared>,
    device: String,
    package: String,
    name: String,
    display: Option<AppDisplay>,
) -> Result<(), CoreError> {
    let opened = core.apps().open_app(&device, &package, &name, display).await?;
    crate::toolbar_window::keep_app_toolbar(app, core.inner().clone(), opened);
    Ok(())
}

#[tauri::command]
pub fn cached_apps(core: State<Shared>, device: String) -> AppCache {
    core.apps().cached_apps(&device)
}

/// The favourite apps of a device and the ones opened from this program lately.
#[tauri::command]
pub fn app_usage(core: State<Shared>, device: String) -> AppUsage {
    core.apps().app_usage(&device)
}

#[tauri::command]
pub fn set_app_favorite(core: State<Shared>, device: String, package: String, favorite: bool) -> Result<(), CoreError> {
    core.apps().set_app_favorite(&device, &package, favorite)
}
