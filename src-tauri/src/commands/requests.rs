//! The small window of a request from a file manager.

use crate::Shared;
use crate::core::error::CoreError;
use crate::core::requests::request::RequestView;
use tauri::State;

/// The request the small window shows: the window asks for it when it starts, and is
/// told of each change as `request` events.
#[tauri::command]
pub fn current_request(core: State<Shared>) -> Option<RequestView> {
    core.requests().current_request()
}

/// The small window of request `id` is closing (itself or by the user).
#[tauri::command]
pub fn dismiss_request(core: State<Shared>, id: u64) {
    core.requests().dismiss_request(id)
}

#[tauri::command]
pub async fn resolve_choice(core: State<'_, Shared>, device: String) -> Result<(), CoreError> {
    core.requests().resolve_choice(&device).await
}

#[tauri::command]
pub fn cancel_choice(core: State<Shared>) {
    core.requests().cancel_choice()
}
