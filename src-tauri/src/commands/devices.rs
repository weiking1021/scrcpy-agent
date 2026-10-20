//! Devices: pairing, connecting, what the user decides about one, and what is
//! read from it.

use crate::Shared;
use crate::core::error::CoreError;
use crate::core::model::{DeviceDetails, DeviceSettings, Snapshot};
use tauri::{AppHandle, State};

#[tauri::command]
pub fn get_snapshot(core: State<Shared>) -> Snapshot {
    core.snapshot()
}

#[tauri::command]
pub async fn pair_device(core: State<'_, Shared>, address: String, code: String) -> Result<(), CoreError> {
    core.devices().pair(address, code).await
}

#[tauri::command]
pub async fn connect_device(core: State<'_, Shared>, address: String) -> Result<(), CoreError> {
    core.devices().connect(address).await
}

#[tauri::command]
pub fn forget_device(core: State<Shared>, id: String) -> Result<(), CoreError> {
    core.devices().forget(&id)
}

#[tauri::command]
pub fn set_device_label(core: State<Shared>, device: String, label: Option<String>) -> Result<(), CoreError> {
    core.devices().set_label(&device, label.as_deref())
}

/// What is set for one device alone.
#[tauri::command]
pub fn device_settings(core: State<Shared>, device: String) -> DeviceSettings {
    core.devices().device_settings(&device)
}

#[tauri::command]
pub fn set_device_settings(core: State<Shared>, device: String, settings: DeviceSettings) -> Result<(), CoreError> {
    core.devices().set_device_settings(&device, settings)
}

#[tauri::command]
pub async fn device_details(core: State<'_, Shared>, device: String) -> Result<DeviceDetails, CoreError> {
    core.readings().device_details(&device).await
}

/// What was last read from each usable device, for the windows that open later.
#[tauri::command]
pub fn device_infos(core: State<Shared>) -> std::collections::HashMap<String, crate::core::device::info::DeviceInfo> {
    core.readings().device_infos()
}

#[tauri::command]
pub async fn refresh_device_info(core: State<'_, Shared>, device: String) -> Result<(), CoreError> {
    core.readings().refresh_device_info(&device).await
}

/// Opens the window of a device (or brings it forward): what is known about the
/// device and its apps.
#[tauri::command]
pub async fn open_device_window<R: tauri::Runtime>(
    app: AppHandle<R>,
    core: State<'_, Shared>,
    device: String,
) -> Result<(), CoreError> {
    crate::windows::open_device_window(&app, &core, &device)
}
