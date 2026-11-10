use super::*;
use crate::core::{
    adb::api::ShellOutput,
    bridge::NativeBridge,
    scrcpy::{
        protocol::{ScrcpyProtocol, TouchAction, protocol_for},
        version::ScrcpyVersion,
    },
};
use anyhow::{Context, bail};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::mpsc;

#[cfg(test)]
mod tests;

type Control = (Arc<dyn ScrcpyProtocol>, mpsc::Sender<Vec<u8>>);

pub struct AndroidHost {
    bridge: NativeBridge,
    controls: Arc<Mutex<HashMap<String, Control>>>,
    input_ready: Arc<tokio::sync::OnceCell<()>>,
    window_settings: Mutex<(crate::core::model::AndroidWindowPadding, crate::core::model::AndroidWindowMode)>,
}

impl AndroidHost {
    pub fn new(bridge: NativeBridge) -> Self {
        Self {
            bridge,
            controls: Default::default(),
            input_ready: Default::default(),
            window_settings: Mutex::new(Default::default()),
        }
    }

    pub fn set_window_settings(&self, settings: &crate::core::model::Settings) {
        *self.window_settings.lock().unwrap() = (settings.android_window_padding_dp, settings.android_window_mode);
    }
}

#[derive(serde::Deserialize)]
struct Input {
    key: String,
    #[serde(flatten)]
    event: InputEvent,
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum InputEvent {
    Touch {
        action: u8,
        pointer: i64,
        x: u32,
        y: u32,
        width: u16,
        height: u16,
        pressure: f32,
    },
    ResetVideo,
}

fn adapter(install: &Installed) -> Result<Arc<dyn ScrcpyProtocol>> {
    ScrcpyVersion::parse(&install.version)
        .and_then(|v| protocol_for(&v))
        .map(Arc::from)
        .ok_or_else(|| anyhow::anyhow!("no native presenter protocol for {}", install.version))
}

impl ScrcpyHost for AndroidHost {
    fn supports_version(&self, version: &str) -> bool {
        ScrcpyVersion::parse(version)
            .and_then(|v| protocol_for(&v))
            .is_some()
    }

    fn prepare(&self, base: std::path::PathBuf) -> HostFuture<()> {
        let bridge = self.bridge.clone();
        Box::pin(async move {
            use crate::core::scrcpy::{manager, versions::ReleaseTag};
            let tag = ReleaseTag::default_tag();
            if manager::find_version(&base, tag.as_str()).is_some() {
                return Ok(());
            }
            tokio::fs::create_dir_all(&base).await?;
            let asset = base.join(format!(".{}.bundled", tag.as_str()));
            let version = bridge
                .call::<String>("bundledServer", json!({"destination": asset}))
                .await?;
            if version != tag.as_str().trim_start_matches('v') {
                let _ = tokio::fs::remove_file(&asset).await;
                bail!("bundled scrcpy server version does not match the core");
            }
            let source = asset.clone();
            let result = tokio::task::spawn_blocking(move || {
                manager::install_bundled_server(&base, &tag, &source)
            })
            .await;
            let _ = tokio::fs::remove_file(&asset).await;
            result??;
            Ok(())
        })
    }

    fn start(
        &self,
        key: String,
        install: Installed,
        spec: SessionSpec,
        _proxy_args: Vec<String>,
    ) -> HostFuture<RunningSession> {
        let bridge = self.bridge.clone();
        let controls = self.controls.clone();
        let input_ready = self.input_ready.clone();
        let (window_padding, window_mode) = *self.window_settings.lock().unwrap();
        Box::pin(async move {
            if !spec.extra_args.is_empty() {
                bail!("the Android presenter does not support extra desktop client arguments");
            }
            let protocol = adapter(&install)?;
            let screen = if let Some(size) = spec.app.as_ref()
                .filter(|app| app.display == crate::core::apps::AppDisplay::VirtualDisplay)
                .and_then(|app| app.size)
            {
                size
            } else {
                use crate::core::{adb::{api::AdbApi, android::AndroidAdb}, device::display};
                let adb = AndroidAdb::new(bridge.clone());
                let output = tokio::time::timeout(Duration::from_secs(10), adb.shell(&spec.serial, display::DISPLAY_DUMP_COMMAND))
                    .await.context("remote display query timed out")??;
                if output.exit != 0 { bail!("could not read the remote display dimensions"); }
                display::current_size(&String::from_utf8_lossy(&output.stdout))
                    .context("the remote display did not report its current dimensions")?
            };
            let initial_control = spec
                .app
                .as_ref()
                .map(|app| {
                    protocol.encode_control(&ControlMessage::StartApp {
                        package: app.package.clone(),
                    })
                })
                .transpose()?;
            let options = protocol.server_options(&spec)?;
            let audio_options = protocol.audio_server_options()?;
            input_ready
                .get_or_try_init(|| async {
                    let controls = controls.clone();
                    let cancel = bridge.clone();
                    let runtime = tokio::runtime::Handle::current();
                    bridge
                        .listen(
                            "touch",
                            Arc::new(move |value| {
                                let Ok(input) = serde_json::from_value::<Input>(value) else {
                                    return;
                                };
                                let message = match input.event {
                                    InputEvent::ResetVideo => ControlMessage::ResetVideo,
                                    InputEvent::Touch {
                                        action,
                                        pointer,
                                        x,
                                        y,
                                        width,
                                        height,
                                        pressure,
                                    } => {
                                        if width == 0
                                            || height == 0
                                            || x >= u32::from(width)
                                            || y >= u32::from(height)
                                            || !pressure.is_finite()
                                        {
                                            return;
                                        }
                                        let action = match action {
                                            0 => TouchAction::Down,
                                            1 => TouchAction::Up,
                                            2 => TouchAction::Move,
                                            _ => return,
                                        };
                                        ControlMessage::InjectTouch {
                                            action,
                                            pointer_id: pointer,
                                            x,
                                            y,
                                            screen_width: width,
                                            screen_height: height,
                                            pressure: pressure.clamp(0.0, 1.0),
                                            action_button: 0,
                                            buttons: 0,
                                        }
                                    }
                                };
                                let controls = controls.lock().unwrap();
                                let Some((adapter, sender)) = controls.get(&input.key) else {
                                    return;
                                };
                                let sent = adapter
                                    .encode_control(&message)
                                    .ok()
                                    .is_some_and(|packet| sender.try_send(packet).is_ok());
                                if !sent {
                                    let cancel = cancel.clone();
                                    runtime.spawn(async move {
                                        let _ = cancel
                                            .call::<serde_json::Value>(
                                                "cancelSession",
                                                json!({"key": input.key}),
                                            )
                                            .await;
                                    });
                                }
                            }),
                        )
                        .await
                })
                .await?;
            let (sender, mut receiver) = mpsc::channel::<Vec<u8>>(256);
            controls
                .lock()
                .unwrap()
                .insert(key.clone(), (protocol, sender));
            let transport = bridge.clone();
            let control_key = key.clone();
            tokio::spawn(async move {
                while let Some(packet) = receiver.recv().await {
                    if transport
                        .call::<serde_json::Value>(
                            "sendControl",
                            json!({"key": control_key, "packet": packet}),
                        )
                        .await
                        .is_err()
                    {
                        let _ = transport
                            .call::<serde_json::Value>("cancelSession", json!({"key": control_key}))
                            .await;
                        break;
                    }
                }
            });
            let start = tokio::time::timeout(Duration::from_secs(45), bridge.call::<serde_json::Value>("startSession", json!({
                "key": key, "serial": spec.serial, "title": spec.title,
                "version": install.version.trim_start_matches('v'), "serverPath": install.server(),
                "options": options, "initialControl": initial_control,
                "windowPaddingDp": window_padding.dp(), "windowMode": window_mode,
                "audioDevice": crate::core::apps::SessionKey::parse(&key).device,
                "audioOptions": audio_options,
                "screenWidth": screen.width, "screenHeight": screen.height,
            }))).await.context("session did not become ready in time").and_then(|result| result);
            if let Err(error) = start {
                controls.lock().unwrap().remove(&key);
                let _ = bridge
                    .call::<serde_json::Value>("cancelSession", json!({"key": key}))
                    .await;
                return Err(error);
            }
            Ok(RunningSession(Box::pin(async move {
                #[derive(serde::Deserialize)]
                struct End {
                    success: bool,
                    tail: String,
                }
                let result = bridge.call::<End>("waitSession", json!({"key": key})).await;
                controls.lock().unwrap().remove(&key);
                let end = result?;
                Ok(Finished {
                    success: end.success,
                    tail: end.tail,
                })
            })))
        })
    }

    fn query(&self, install: Installed, query: Query, limit: Duration) -> HostFuture<Output> {
        let bridge = self.bridge.clone();
        Box::pin(async move {
            let protocol = adapter(&install)?;
            let Query::Apps { serial } = query;
            let output: ShellOutput = tokio::time::timeout(limit, bridge.call("queryServer", json!({
                "serial": serial, "version": install.version.trim_start_matches('v'),
                "serverPath": install.server(), "options": protocol.app_list_server_options()?,
            }))).await.context("scrcpy query did not finish in time")??;
            Ok(Output {
                stdout: output.stdout,
                stderr: output.stderr,
                success: output.exit == 0,
            })
        })
    }

    fn has_control(&self, key: &str) -> bool {
        self.controls.lock().unwrap().contains_key(key)
    }

    fn send_control(&self, key: &str, message: &ControlMessage) -> Result<()> {
        let controls = self.controls.lock().unwrap();
        let (protocol, sender) = controls
            .get(key)
            .ok_or_else(|| anyhow::anyhow!("session has no control channel"))?;
        sender
            .try_send(protocol.encode_control(message)?)
            .map_err(|_| anyhow::anyhow!("control queue is unavailable"))
    }
}
