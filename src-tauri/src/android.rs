//! Android host glue; commands, state, device policy and pages are shared.
use crate::{
    Shared,
    core::{
        Core, CoreOptions, Sink, adb::android::AndroidAdb, bridge::NativeBridge, error::CoreError,
        scrcpy::host::android::AndroidHost,
    },
};
use std::sync::Arc;
use tauri::{Emitter, Manager};

mod shares;

// Called on the plugin's JVM thread before native requests can use HTTPS.
#[jni::jni_mangle("io.github.weiking1021.scrcpyagent.android.DevicePlugin")]
pub fn initialize_tls<'caller>(
    mut env: jni::EnvUnowned<'caller>,
    _plugin: jni::objects::JObject<'caller>,
    context: jni::objects::JObject<'caller>,
) {
    use jni::errors::ThrowRuntimeExAndDefault;
    env.with_env(|env| rustls_platform_verifier::android::init_with_env(env, context))
        .resolve::<ThrowRuntimeExAndDefault>();
}

pub fn run() {
    let plugin = tauri::plugin::Builder::<tauri::Wry, ()>::new("android-device")
        .setup(|app, api| {
            let native = api.register_android_plugin(
                "io.github.weiking1021.scrcpyagent.android",
                "DevicePlugin",
            )?;
            let events = native.clone();
            let bridge = NativeBridge {
                request: Arc::new(move |operation, data| {
                    let native = native.clone();
                    Box::pin(async move {
                        native
                            .run_mobile_plugin_async::<serde_json::Value>(
                                "execute",
                                serde_json::json!({"operation": operation, "data": data}),
                            )
                            .await
                            .map_err(|e| e.to_string())
                    })
                }),
                events: Arc::new(move |event, handler| {
                    let native = events.clone();
                    Box::pin(async move {
                        let channel = tauri::ipc::Channel::<serde_json::Value>::new(move |body| {
                            handler(body.deserialize()?);
                            Ok(())
                        });
                        native
                            .run_mobile_plugin_async::<serde_json::Value>(
                                "registerListener",
                                serde_json::json!({"event": event, "handler": channel}),
                            )
                            .await
                            .map_err(|e| e.to_string())
                    })
                }),
            };
            app.manage(bridge);
            Ok(())
        })
        .build();
    crate::configure(
        tauri::Builder::default()
            .plugin(plugin)
            .plugin(tauri_plugin_opener::init()),
    )
    .setup(|app| {
        let handle = app.handle().clone();
        let (requests, mut changed) = tokio::sync::watch::channel(());
        let request_app = handle.clone();
        tauri::async_runtime::spawn(async move {
            let mut shown = None;
            while changed.changed().await.is_ok() {
                windows::sync_request_page(&request_app, &mut shown);
            }
        });
        let sink: Sink = Arc::new(move |event| {
            if matches!(&event, crate::core::model::Event::Request(_)) {
                requests.send_replace(());
            }
            let _ = handle.emit("core-event", event);
        });
        let root = app.path().app_data_dir()?;
        let options = CoreOptions {
            android: Some(AndroidAdb::new(app.state::<NativeBridge>().inner().clone())),
            scrcpy_host: Some(Arc::new(AndroidHost::new(
                app.state::<NativeBridge>().inner().clone(),
            ))),
            disable_own_mdns: true, // Android's NSD implementation supplies the same services through AdbApi.
            ..Default::default()
        };
        let core = Core::with_options(root, sink, options);
        app.manage(core.clone());
        tauri::async_runtime::spawn(core.clone().run());
        let bridge = app.state::<NativeBridge>().inner().clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = shares::receive(core, bridge).await {
                eprintln!("Android share receiver failed: {error:#}");
            }
        });
        Ok(())
    })
    .run(tauri::generate_context!())
    .expect("error while running Android application");
}

// Android has no tray. Keep the shared UI's translated identity command harmless.
pub mod tray {
    #[tauri::command]
    pub fn set_tray_texts(texts: serde_json::Value) {
        let _ = texts;
    }
}

// Mobile navigation reuses the existing pages inside the one Android webview.
pub mod windows {
    use super::*;
    pub fn sync_request_page<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        shown: &mut Option<u64>,
    ) {
        let Some(core) = app.try_state::<Shared>() else {
            return;
        };
        let current = core.requests().current_request().map(|view| view.id);
        if current == *shown {
            return;
        }
        let Some(window) = app.get_webview_window("main") else {
            return;
        };
        let Ok(url) = window.url() else { return };
        let result = if current.is_some() {
            if url.path() == "/request.html" {
                Ok(())
            } else {
                page(app, "/request.html", None)
            }
        } else if url.path() == "/request.html" {
            page(app, "/index.html", None)
        } else {
            Ok(())
        };
        if result.is_ok() {
            *shown = current;
        }
    }
    fn page<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        path: &str,
        device: Option<&str>,
    ) -> Result<(), CoreError> {
        let window = app
            .get_webview_window("main")
            .ok_or(CoreError::Unavailable)?;
        let mut url = window.url().map_err(|e| CoreError::Other(e.to_string()))?;
        url.set_path(path);
        url.set_query(None);
        if let Some(device) = device {
            url.query_pairs_mut().append_pair("device", device);
        }
        window
            .navigate(url)
            .map_err(|e| CoreError::Other(e.to_string()))
    }
    pub fn open_device_window<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        _core: &Shared,
        device: &str,
    ) -> Result<(), CoreError> {
        page(app, "/device.html", Some(device))
    }
    pub fn open_console_window<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
    ) -> Result<(), CoreError> {
        page(app, "/console.html", None)
    }
}
