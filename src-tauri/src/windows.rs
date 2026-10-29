//! The windows of the application: which there are, how they are told apart,
//! and what the closing of one means for the rest.

use crate::Shared;
use crate::core::error::CoreError;
use tauri::{AppHandle, Manager};

/// The label of the debug console's window.
const CONSOLE_WINDOW: &str = "console";
const MAIN_WINDOW: &str = "main";
/// The label of the small window of a request from a file manager.
const REQUEST_WINDOW: &str = "shell-request";
const DEVICE_WINDOW_PREFIX: &str = "device-";

/// What the closing of a window means for the rest of the application.
#[derive(Debug, PartialEq, Eq)]
enum AfterDestroyed {
    Nothing,
    /// The window of a request is gone: a request that waits for a device is dropped, and
    /// what it showed is not shown again.
    CancelRequest,
    /// The console is gone: the core stops sending the log.
    StopLog,
    /// The main window is gone: the console and the device windows go with it
    /// (alone, they would keep the application running with nothing to use).
    CloseOthers,
}

/// The label of the window of a device: its id in hexadecimal after a prefix, so
/// any id gives a label Tauri accepts and the id can be read back from it.
pub(crate) fn device_window_label(device_id: &str) -> String {
    let hex: String = device_id.bytes().map(|b| format!("{b:02x}")).collect();
    format!("{DEVICE_WINDOW_PREFIX}{hex}")
}

/// The device id a window label stands for; `None` if it is not exactly one made
/// by `device_window_label`.
pub(crate) fn device_id_of_label(label: &str) -> Option<String> {
    let hex = label.strip_prefix(DEVICE_WINDOW_PREFIX)?;
    if hex.is_empty() || hex.len() % 2 != 0 || !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16)).collect::<Result<_, _>>().ok()?;
    String::from_utf8(bytes).ok()
}

/// Decided from the exact label of the window that was destroyed.
fn after_destroyed(label: &str) -> AfterDestroyed {
    match label {
        CONSOLE_WINDOW => AfterDestroyed::StopLog,
        MAIN_WINDOW => AfterDestroyed::CloseOthers,
        REQUEST_WINDOW => AfterDestroyed::CancelRequest,
        _ => AfterDestroyed::Nothing,
    }
}

/// Opens the debug console in a window of its own, or brings it forward.
pub(crate) fn open_console_window<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<(), CoreError> {
    if let Some(window) = app.get_webview_window(CONSOLE_WINDOW) {
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(app, CONSOLE_WINDOW, tauri::WebviewUrl::App("console.html".into()))
        .title("scrcpy-agent debug console")
        .inner_size(900.0, 560.0)
        .build()
        .map_err(|e| CoreError::Other(e.to_string()))?;
    Ok(())
}

/// Opens the window of a device, or brings it forward. Also used by the tray
/// menu, which has no command to go through.
pub(crate) fn open_device_window<R: tauri::Runtime>(
    app: &AppHandle<R>,
    core: &Shared,
    device: &str,
) -> Result<(), CoreError> {
    let reported = core.snapshot().devices.iter().find(|d| d.id == device).map(|d| d.label.clone().unwrap_or_else(|| d.name.clone()));
    let title = reported.ok_or(CoreError::UnknownDevice)?;
    let label = device_window_label(device);
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(app, label, tauri::WebviewUrl::App("device.html".into()))
        .title(title)
        .inner_size(760.0, 760.0)
        .min_inner_size(520.0, 480.0)
        .build()
        .map_err(|e| CoreError::Other(e.to_string()))?;
    Ok(())
}

/// Follows the latest request, not a possibly obsolete event. Called by a
/// single worker, so an older show cannot run after a newer close.
pub(crate) fn sync_request_window<R: tauri::Runtime>(app: &AppHandle<R>, shown: &mut Option<u64>) {
    let Some(core) = app.try_state::<Shared>() else { return };
    let current = core.requests().current_request().map(|view| view.id);
    if current == *shown { return; }
    if current.is_some() {
        if show_request_window(app).is_ok() { *shown = current; }
    } else {
        close_request_window(app);
        *shown = None;
    }
}

/// Shows the small window once per request. Progress only updates its contents.
fn show_request_window<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<(), tauri::Error> {
    if let Some(window) = app.get_webview_window(REQUEST_WINDOW) {
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(app, REQUEST_WINDOW, tauri::WebviewUrl::App("request.html".into()))
        .title("scrcpy-agent")
        .inner_size(440.0, 360.0)
        .min_inner_size(360.0, 220.0)
        .center()
        .always_on_top(true)
        .build()?;
    Ok(())
}

/// Keep one webview for later requests. Destroying and immediately recreating
/// it lets the old Destroyed event cancel the new request.
fn close_request_window<R: tauri::Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(REQUEST_WINDOW) {
        let _ = window.hide();
    }
}

/// What closing a window does.
#[derive(Debug, PartialEq, Eq)]
enum CloseAction {
    /// The window closes.
    Allow,
    /// The window is hidden and the application keeps running in the tray.
    HideToTray,
}

/// Only the main window goes to the tray, and only when the setting says so; the
/// console and the device windows just close.
fn close_action(label: &str, close_to_tray: bool) -> CloseAction {
    if label == MAIN_WINDOW && close_to_tray { CloseAction::HideToTray } else { CloseAction::Allow }
}

/// Brings the main window back from the tray (or from behind other windows).
pub(crate) fn show_main<R: tauri::Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

pub(crate) fn on_window_event<R: tauri::Runtime>(window: &tauri::Window<R>, event: &tauri::WindowEvent) {
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        if window.label() == REQUEST_WINDOW {
            api.prevent_close();
            if let Some(core) = window.try_state::<Shared>()
                && let Some(view) = core.requests().current_request()
            {
                core.requests().dismiss_request(view.id);
            }
            let _ = window.hide();
            return;
        }
        let close_to_tray = window.try_state::<Shared>().is_some_and(|core| core.snapshot().settings.close_to_tray);
        if close_action(window.label(), close_to_tray) == CloseAction::HideToTray {
            api.prevent_close();
            let _ = window.hide();
        }
        return;
    }
    if !matches!(event, tauri::WindowEvent::Destroyed) {
        return;
    }
    match after_destroyed(window.label()) {
        AfterDestroyed::Nothing => {}
        AfterDestroyed::CancelRequest => {
            if let Some(core) = window.try_state::<Shared>() {
                core.requests().cancel_choice();
                if let Some(view) = core.requests().current_request() {
                    core.requests().dismiss_request(view.id);
                }
            }
        }
        AfterDestroyed::StopLog => {
            if let Some(core) = window.try_state::<Shared>() {
                core.close_debug_console();
            }
        }
        AfterDestroyed::CloseOthers => {
            for (label, other) in window.app_handle().webview_windows() {
                if label == CONSOLE_WINDOW || label == REQUEST_WINDOW || device_id_of_label(&label).is_some() {
                    let _ = other.destroy();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_the_console_stops_the_log_and_closing_the_main_window_closes_the_console() {
        assert_eq!(after_destroyed("console"), AfterDestroyed::StopLog);
        assert_eq!(after_destroyed("main"), AfterDestroyed::CloseOthers);
    }

    #[test]
    fn closing_the_window_of_a_request_drops_a_request_that_waits() {
        assert_eq!(after_destroyed("shell-request"), AfterDestroyed::CancelRequest);
        for label in ["shell-request-2", "Shell-Request", "request", "shell-request "] {
            assert_eq!(after_destroyed(label), AfterDestroyed::Nothing, "{label:?}");
        }
    }

    #[test]
    fn only_the_main_window_goes_to_the_tray_and_only_when_the_setting_is_on() {
        assert_eq!(close_action("main", true), CloseAction::HideToTray);
        assert_eq!(close_action("main", false), CloseAction::Allow);
        for label in ["console", "device-4142", "toolbar-1", "main2", "Main", ""] {
            assert_eq!(close_action(label, true), CloseAction::Allow, "{label:?}");
        }
    }

    #[test]
    fn a_device_window_label_carries_the_id_and_only_such_a_label_gives_one_back() {
        for id in ["TESTDEV0001", "192.168.1.5:40000", "../x y", "Gerät", ""] {
            let label = device_window_label(id);
            assert!(label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'), "{label}");
            if id.is_empty() {
                assert_eq!(device_id_of_label(&label), None);
            } else {
                assert_eq!(device_id_of_label(&label).as_deref(), Some(id));
            }
        }
        assert_eq!(device_window_label("AB"), "device-4142");
        for bad in ["", "device-", "device-4", "device-4g", "device-4A42", "Device-4142", "toolbar-4142", "device-ff"] {
            assert_eq!(device_id_of_label(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn other_windows_change_nothing_and_labels_are_matched_exactly() {
        for label in ["toolbar-1", "toolbar-12", "console-2", "my-console", "Console", "main2", "", "mainconsole"] {
            assert_eq!(after_destroyed(label), AfterDestroyed::Nothing, "{label:?}");
        }
    }
}
