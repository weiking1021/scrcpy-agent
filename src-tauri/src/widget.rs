//! The Windows widget follows the application's language and data.
//! Optional package registration lives with the other OS integrations.
use crate::core::error::CoreError;
#[cfg(not(target_os = "android"))]
use crate::Shared;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tauri::{AppHandle, Manager};

pub type Texts = BTreeMap<String, String>;
pub struct WidgetTexts(pub Arc<Mutex<Texts>>);

pub fn defaults() -> Texts {
    let pack: BTreeMap<String, String> =
        serde_json::from_str(include_str!("../../src-ui/shared/locales/en.json")).unwrap();
    let mut texts: Texts = pack
        .into_iter()
        .filter_map(|(key, value)| {
            key.strip_prefix("widget.")
                .map(|key| (key.to_string(), value))
        })
        .collect();
    texts.insert(
        "app_name".into(),
        crate::core::app_name::DEFAULT_APP_NAME.into(),
    );
    for text in texts.values_mut() {
        *text = text.replace("{app}", crate::core::app_name::DEFAULT_APP_NAME);
    }
    texts
}

pub fn texts<R: tauri::Runtime>(app: &AppHandle<R>) -> Texts {
    app.state::<WidgetTexts>().0.lock().unwrap().clone()
}

pub fn available() -> bool {
    #[cfg(windows)]
    {
        crate::core::integration::windows11_menu::available()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub fn apply(
    enabled: bool,
    directory: &std::path::Path,
    texts: &Texts,
    pipe: &str,
) -> Result<(), CoreError> {
    #[cfg(windows)]
    {
        crate::core::integration::windows_widget::apply(enabled, directory, texts, pipe)
    }
    #[cfg(not(windows))]
    {
        let _ = (directory, texts, pipe);
        if enabled {
            Err(CoreError::Unavailable)
        } else {
            Ok(())
        }
    }
}

/// A separate installation (including a portable folder) has its own endpoint.
pub fn pipe_name(directory: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(directory.as_os_str().to_string_lossy().as_bytes());
    format!(
        "scrcpy-agent-widget-{}",
        digest[..12]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

#[cfg(not(target_os = "android"))]
pub fn start<R: tauri::Runtime>(app: &AppHandle<R>, core: Shared) {
    let directory = core.store().windows_widget_dir();
    let saved: Texts = std::fs::read(directory.join("texts.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let mut labels = defaults();
    labels.extend(saved);
    let labels = Arc::new(Mutex::new(labels));
    app.manage(WidgetTexts(labels.clone()));
    #[cfg(windows)]
    if available() {
        start_pipe(core, labels, pipe_name(&directory));
    }
    #[cfg(not(windows))]
    let _ = core;
}

#[cfg(windows)]
fn start_pipe(core: Shared, texts: Arc<Mutex<Texts>>, name: String) {
    use crate::core::{device::info::BatteryStatus, model::DeviceState};
    use tokio::{io::AsyncWriteExt, net::windows::named_pipe::ServerOptions};
    tauri::async_runtime::spawn(async move {
        let mut first = true;
        loop {
            let mut pipe = match ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create(format!(r"\\.\pipe\{name}"))
            {
                Ok(pipe) => pipe,
                Err(error) => {
                    eprintln!("the widget endpoint could not be opened: {error}");
                    break;
                }
            };
            first = false;
            if pipe.connect().await.is_err() {
                break;
            }
            let snapshot = core.snapshot();
            let infos = core.readings().device_infos();
            let devices: Vec<_> = snapshot.devices.into_iter()
                .filter(|device| snapshot.settings.windows11_widget && device.state == DeviceState::Online)
                .map(|device| {
                    let battery = infos.get(&device.id).and_then(|info| info.battery.as_ref());
                    serde_json::json!({
                        "id": device.id,
                        "name": device.label.unwrap_or(device.name),
                        "battery": battery.and_then(|b| (b.scale > 0).then(|| ((u64::from(b.level) * 100 + u64::from(b.scale) / 2) / u64::from(b.scale)).min(100))),
                        "charging": battery.is_some_and(|b| b.status == Some(BatteryStatus::Charging)),
                    })
                }).collect();
            let payload = serde_json::json!({ "enabled": snapshot.settings.windows11_widget, "devices": devices, "texts": *texts.lock().unwrap() });
            if let Ok(bytes) = serde_json::to_vec(&payload) {
                let _ =
                    tokio::time::timeout(std::time::Duration::from_secs(2), pipe.write_all(&bytes))
                        .await;
            }
        }
    });
}
