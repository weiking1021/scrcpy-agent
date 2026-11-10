//! Android launcher integration; opening an app stays in the shared app service.
use crate::{
    Shared,
    core::{
        apps::PackageName,
        bridge::NativeBridge,
        error::CoreError,
        integration::shortcuts::{ShortcutPlace, ShortcutState, display_name},
        model::{NoticeCode, NoticeLevel},
    },
};
use serde::Deserialize;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tauri::{AppHandle, State};
use tokio::sync::Notify;

#[derive(Deserialize)]
struct Capability {
    supported: bool,
}

#[derive(Deserialize)]
struct PinResult {
    accepted: bool,
}

#[tauri::command]
pub async fn shortcuts_available(
    bridge: State<'_, NativeBridge>,
) -> Result<Option<&'static str>, CoreError> {
    let result: Capability = bridge
        .call("shortcutsAvailable", json!({}))
        .await
        .map_err(|e| CoreError::Other(e.to_string()))?;
    Ok(result.supported.then_some("android_pinned"))
}

#[tauri::command]
pub async fn create_app_shortcut(
    bridge: State<'_, NativeBridge>,
    core: State<'_, Shared>,
    device: String,
    package: String,
    place: ShortcutPlace,
) -> Result<String, CoreError> {
    if place != ShortcutPlace::Desktop {
        return Err(CoreError::Unavailable);
    }
    let package = PackageName::parse(&package).ok_or(CoreError::InvalidPackage)?;
    let shown = core
        .snapshot()
        .devices
        .into_iter()
        .find(|d| d.id == device)
        .map(|d| d.label.unwrap_or(d.name))
        .ok_or(CoreError::UnknownDevice)?;
    let label = display_name(&core.apps().app_name(&device, &package), &shown);
    let result: PinResult = bridge
        .call(
            "pinAppShortcut",
            json!({
                "device": device, "package": package.as_str(), "label": label,
            }),
        )
        .await
        .map_err(|e| CoreError::Other(e.to_string()))?;
    if !result.accepted {
        return Err(CoreError::Unavailable);
    }
    Ok(label)
}

#[tauri::command]
pub async fn app_shortcut_state(
    bridge: State<'_, NativeBridge>,
    device: String,
    package: String,
) -> Result<ShortcutState, CoreError> {
    let package = PackageName::parse(&package).ok_or(CoreError::InvalidPackage)?;
    bridge
        .call(
            "appShortcutState",
            json!({"device": device, "package": package.as_str()}),
        )
        .await
        .map_err(|e| CoreError::Other(e.to_string()))
}

// Android lets the launcher/user remove pinned icons. Disabling one would leave it on the desktop.
#[tauri::command]
pub fn remove_app_shortcut(
    device: String,
    package: String,
    place: ShortcutPlace,
) -> Result<(), CoreError> {
    let _ = (device, package, place);
    Err(CoreError::Unavailable)
}

#[derive(Deserialize)]
struct AppShortcut {
    device: String,
    package: String,
}

pub async fn receive(app: AppHandle, core: Shared, bridge: NativeBridge) -> anyhow::Result<()> {
    let changed = Arc::new(Notify::new());
    let notify = changed.clone();
    bridge
        .listen("appShortcut", Arc::new(move |_| notify.notify_one()))
        .await?;
    loop {
        let pending: Vec<AppShortcut> = bridge.call("takeAppShortcuts", json!({})).await?;
        for request in pending {
            let Some(package) = PackageName::parse(&request.package) else {
                continue;
            };
            core.devices()
                .wait_until_devices_known(Duration::from_secs(20))
                .await;
            match core
                .apps()
                .open_app_from_shortcut(&request.device, &package)
                .await
            {
                Ok(_) => {}
                Err(CoreError::NotOnline | CoreError::UnknownDevice) => {
                    core.notice(
                        NoticeLevel::Info,
                        NoticeCode::DeviceNotOnline,
                        package.as_str(),
                        Some(request.device.clone()),
                    );
                    let _ =
                        crate::android::windows::open_device_window(&app, &core, &request.device);
                }
                Err(error) => core.notice(
                    NoticeLevel::Error,
                    NoticeCode::SessionFailed,
                    error.to_string(),
                    Some(request.device),
                ),
            }
        }
        changed.notified().await;
    }
}
