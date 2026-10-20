//! The screens of devices and their toolbars.

use crate::Shared;
use crate::core::addons::buttons::{ButtonId, ToolbarButton};
use crate::core::error::CoreError;
use tauri::{AppHandle, State};

// Commands that start processes or tasks must be `async`: plain commands run
// on the main thread, outside the Tokio runtime, and would crash the app.
#[tauri::command]
pub async fn open_device<R: tauri::Runtime>(
    app: AppHandle<R>,
    core: State<'_, Shared>,
    id: String,
) -> Result<(), CoreError> {
    open_screen(app, core.inner().clone(), &id).await
}

/// Opens the screen of a device, with its toolbar when the settings have one. The window
/// of the list, the window of a device and the tray menu all open a screen this way.
pub(crate) async fn open_screen<R: tauri::Runtime>(
    app: AppHandle<R>,
    core: Shared,
    id: &str,
) -> Result<(), CoreError> {
    let opened = core.sessions().open(id).await?;
    let toolbar = core.snapshot().settings.toolbar;
    if toolbar.enabled {
        tauri::async_runtime::spawn(crate::toolbar_window::keep_toolbar(
            app,
            core,
            id.to_string(),
            opened.title,
            toolbar,
        ));
    }
    Ok(())
}

#[tauri::command]
pub fn toolbar_buttons(core: State<Shared>, device: Option<String>) -> Vec<ToolbarButton> {
    core.sessions().toolbar_buttons(device.as_deref())
}

/// Every button of the toolbar, shown or not, in the order of the toolbar: the list the user arranges in the settings.
#[tauri::command]
pub fn toolbar_layout(core: State<Shared>) -> Vec<crate::core::addons::buttons::LayoutEntry> {
    core.sessions().toolbar_layout()
}

#[tauri::command]
pub async fn press_button(core: State<'_, Shared>, device: String, button: ButtonId) -> Result<(), CoreError> {
    core.sessions().press_button(&device, &button).await
}
