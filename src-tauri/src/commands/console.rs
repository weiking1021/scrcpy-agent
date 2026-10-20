//! The debug console.

use crate::Shared;
use crate::core::error::CoreError;
use crate::core::log::LogLine;
use tauri::{AppHandle, State};

/// Opens the debug console; the answer is the log so far, and new lines follow
/// as `log` events.
#[tauri::command]
pub fn open_debug_console(core: State<Shared>) -> Vec<LogLine> {
    core.open_debug_console()
}

#[tauri::command]
pub fn close_debug_console(core: State<Shared>) {
    core.close_debug_console();
}

/// Opens the debug console in a window of its own (or brings it forward), so
/// it can stay open next to the main window while the user works.
#[tauri::command]
pub async fn open_console_window<R: tauri::Runtime>(app: AppHandle<R>) -> Result<(), CoreError> {
    crate::windows::open_console_window(&app)
}
