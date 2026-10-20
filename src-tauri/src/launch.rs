//! What a start of the program asks for: the arguments a shortcut of an app
//! or the file manager's menu gives it, whether it is the first start or a
//! second one that was handed to the running program.

use crate::Shared;
use crate::core::error::CoreError;
use crate::core::model::{NoticeCode, NoticeLevel};
use crate::core::requests::shell_request::{self, AppRequest, ShellRequest};
use crate::windows::{open_device_window, show_main};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// Requests of the file manager's menu that came in a moment ago. The menu starts the
/// program once for every selected file, so what arrives within a short time is
/// one request (for all of them), not one question for each.
#[derive(Default)]
pub struct ShellInbox {
    pending: Mutex<Vec<ShellRequest>>,
    last: Mutex<Option<Instant>>,
    /// A task is waiting for the requests to stop coming.
    waiting: AtomicBool,
}

/// How long nothing may come before the requests that came are carried out.
const SHELL_QUIET: Duration = Duration::from_millis(600);

/// Carries out what the arguments of a start ask for. Whether they asked for
/// anything is the answer: with none, the caller decides what a start means.
pub fn handle_arguments<R: tauri::Runtime>(app: &AppHandle<R>, args: &[String]) -> bool {
    if let Some(request) = shell_request::parse_app_args(args) {
        launch_app_shortcut(app, request);
        return true;
    }
    match shell_request::parse_args(args) {
        Some(requests) => {
            requests.into_iter().for_each(|request| push_request(app, request));
            true
        }
        None => false,
    }
}

fn push_request<R: tauri::Runtime>(app: &AppHandle<R>, request: ShellRequest) {
    let Some(inbox) = app.try_state::<ShellInbox>() else { return };
    inbox.pending.lock().unwrap().push(request);
    *inbox.last.lock().unwrap() = Some(Instant::now());
    if inbox.waiting.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(inbox) = app.try_state::<ShellInbox>() else { return };
        loop {
            tokio::time::sleep(SHELL_QUIET / 3).await;
            let quiet = inbox.last.lock().unwrap().is_none_or(|t| t.elapsed() >= SHELL_QUIET);
            if quiet {
                break;
            }
        }
        let requests = std::mem::take(&mut *inbox.pending.lock().unwrap());
        inbox.waiting.store(false, Ordering::SeqCst);
        let Some(core) = app.try_state::<Shared>() else { return };
        // At the start of the program the devices are not known yet.
        core.devices().wait_until_devices_known(Duration::from_secs(20)).await;
        // The question and the progress are in the small window of the request (shown as
        // the core tells of the request); the main window is left as it is.
        for (action, files) in shell_request::group(requests) {
            core.requests().handle_shell(action, files).await;
        }
    });
}

/// A shortcut started the program: open the app, or say why not and show the
/// window of the device.
fn launch_app_shortcut<R: tauri::Runtime>(app: &AppHandle<R>, request: AppRequest) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(core) = app.try_state::<Shared>() else { return };
        core.devices().wait_until_devices_known(Duration::from_secs(20)).await;
        let core = core.inner().clone();
        match core.apps().open_app_from_shortcut(&request.device, &request.package).await {
            Ok(opened) => crate::toolbar_window::keep_app_toolbar(app, core, opened),
            Err(CoreError::NotOnline | CoreError::UnknownDevice) => {
                core.notice(
                    NoticeLevel::Info,
                    NoticeCode::DeviceNotOnline,
                    request.package.as_str(),
                    Some(request.device.clone()),
                );
                let _ = open_device_window(&app, &core, &request.device);
                show_main(&app);
            }
            Err(error) => {
                core.notice(
                    NoticeLevel::Error,
                    NoticeCode::SessionFailed,
                    error.to_string(),
                    Some(request.device.clone()),
                );
                show_main(&app);
            }
        }
    });
}
