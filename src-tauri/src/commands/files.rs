//! Files and a device.

use crate::Shared;
use crate::core::error::CoreError;
use crate::core::model::{OpenSupport, SendOutcome};
use std::path::PathBuf;
use tauri::State;

/// Sends files to a device (the ones dropped on it, or chosen from the file manager's menu).
#[tauri::command]
pub async fn send_files(
    core: State<'_, Shared>,
    device: String,
    files: Vec<String>,
) -> Result<Vec<SendOutcome>, CoreError> {
    core.files().send_files(&device, files.into_iter().map(PathBuf::from).collect()).await
}

#[tauri::command]
pub async fn open_support(core: State<'_, Shared>, device: String, file: String) -> Result<OpenSupport, CoreError> {
    core.files().open_support(&device, &PathBuf::from(file)).await
}

/// Sends a file to a device and opens it there; the answer is its path on the device.
#[tauri::command]
pub async fn open_file_on_device(core: State<'_, Shared>, device: String, file: String) -> Result<String, CoreError> {
    core.files().open_on_device(&device, &PathBuf::from(file)).await
}
