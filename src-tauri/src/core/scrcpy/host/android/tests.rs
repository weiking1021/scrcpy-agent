use super::*;
use crate::core::{bridge::NativeEvents, device::display::ScreenSize};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};

type Calls = Arc<Mutex<Vec<(String, Value)>>>;
type Listener = Arc<dyn Fn(Value) + Send + Sync>;

struct Native {
    host: AndroidHost,
    calls: Calls,
    input: Arc<Mutex<Option<Listener>>>,
    fail_start: Arc<AtomicBool>,
}

impl Native {
    fn new() -> Self {
        let calls: Calls = Default::default();
        let input: Arc<Mutex<Option<Listener>>> = Default::default();
        let fail_start = Arc::new(AtomicBool::new(false));
        let request = {
            let calls = calls.clone();
            let fail = fail_start.clone();
            Arc::new(
                move |operation: String, data: Value| -> crate::core::bridge::NativeReply {
                    calls.lock().unwrap().push((operation.clone(), data));
                    let reply = match operation.as_str() {
                        "startSession" if fail.swap(false, Ordering::SeqCst) => {
                            Err("server rejected launch".into())
                        }
                        "waitSession" => Ok(json!({"success": true, "tail": ""})),
                        "queryServer" => Ok(json!({"stdout": [65, 66], "stderr": [], "exit": 0})),
                        "shell" => Ok(json!({
                            "stdout": b"mOverrideDisplayInfo=DisplayInfo{\"Display\", displayId 0\", real 1080 x 2520, x\n".to_vec(),
                            "stderr": [], "exit": 0,
                        })),
                        _ => Ok(json!({})),
                    };
                    Box::pin(async move { reply })
                },
            )
        };
        let events: NativeEvents = {
            let input = input.clone();
            Arc::new(move |event, handler| {
                assert_eq!(event, "touch");
                *input.lock().unwrap() = Some(handler);
                Box::pin(async { Ok(json!({})) })
            })
        };
        Self {
            host: AndroidHost::new(NativeBridge { request, events }),
            calls,
            input,
            fail_start,
        }
    }

    fn request(&self, operation: &str) -> Value {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .find(|(op, _)| op == operation)
            .unwrap()
            .1
            .clone()
    }
}

fn install() -> Installed {
    Installed {
        version: "v5.0.1".into(),
        root: std::env::temp_dir().join("scrcpy-host-test"),
    }
}

fn spec() -> SessionSpec {
    SessionSpec {
        serial: "test-device:40000".into(),
        title: "Test screen".into(),
        sdk: Some(34),
        app: None,
        keyboard: KeyboardMode::default(),
        bit_rate: BitRateMbps::default(),
        max_size: MaxSize::default(),
        extra_args: Vec::new(),
    }
}

#[tokio::test]
async fn app_launch_uses_selected_server_and_shared_control_encoding() {
    let native = Native::new();
    native.host.set_window_settings(&crate::core::model::Settings {
        android_window_padding_dp: crate::core::model::AndroidWindowPadding::try_from(24).unwrap(),
        android_window_mode: crate::core::model::AndroidWindowMode::Overlay,
        ..Default::default()
    });
    let package = PackageName::parse("com.example.notes").unwrap();
    let mut spec = spec();
    spec.bit_rate = BitRateMbps::new(12).unwrap();
    spec.app = Some(AppLaunch {
        package: package.clone(),
        display: AppDisplay::VirtualDisplay,
        size: ScreenSize::new(1280, 720),
        keep_awake: true,
    });
    let running = native
        .host
        .start(
            "device#app:com.example.notes".into(),
            install(),
            spec,
            Vec::new(),
        )
        .await
        .unwrap();
    let key = "device#app:com.example.notes";
    let request = native.request("startSession");
    assert_eq!(request["serverPath"], json!(install().server()));
    assert_eq!(request["version"], "5.0.1");
    assert_eq!(request["windowPaddingDp"], 24);
    assert_eq!(request["windowMode"], "overlay");
    assert_eq!(request["audioDevice"], "device");
    assert_eq!((request["screenWidth"].as_u64(), request["screenHeight"].as_u64()), (Some(1280), Some(720)));
    let audio = request["audioOptions"].as_array().unwrap();
    for option in ["video=false", "audio=true", "control=false", "audio_codec=raw", "audio_source=playback", "send_dummy_byte=false", "send_device_meta=false"] {
        assert!(audio.contains(&json!(option)), "missing audio option {option}");
    }
    let options = request["options"].as_array().unwrap();
    for option in [
        "new_display=1280x720",
        "video_bit_rate=12000000",
        "stay_awake=true",
        "screen_off_timeout=3600000",
    ] {
        assert!(options.contains(&json!(option)), "missing {option}");
    }
    let packet = adapter(&install())
        .unwrap()
        .encode_control(&ControlMessage::StartApp { package })
        .unwrap();
    assert_eq!(request["initialControl"], json!(packet));
    assert!(native.host.has_control(key));

    let handler = native.input.lock().unwrap().clone().unwrap();
    handler(
        json!({"key": key, "kind": "touch", "action": 0, "pointer": 2,
        "x": 300, "y": 200, "width": 1280, "height": 720, "pressure": 1.0}),
    );
    tokio::time::timeout(Duration::from_secs(1), async {
        while !native
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|(op, _)| op == "sendControl")
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let packet = adapter(&install())
        .unwrap()
        .encode_control(&ControlMessage::InjectTouch {
            action: TouchAction::Down,
            pointer_id: 2,
            x: 300,
            y: 200,
            screen_width: 1280,
            screen_height: 720,
            pressure: 1.0,
            action_button: 0,
            buttons: 0,
        })
        .unwrap();
    assert_eq!(
        native.request("sendControl"),
        json!({"key": key, "packet": packet})
    );
    assert!(running.0.await.unwrap().success);
    assert!(!native.host.has_control(key));
    assert!(
        native
            .host
            .send_control(key, &ControlMessage::ResetVideo)
            .is_err()
    );
}

#[tokio::test]
async fn failed_start_releases_control_and_allows_retry() {
    let native = Native::new();
    native.fail_start.store(true, Ordering::SeqCst);
    assert!(
        native
            .host
            .start("device".into(), install(), spec(), Vec::new())
            .await
            .is_err()
    );
    assert!(!native.host.has_control("device"));
    assert_eq!(native.request("cancelSession"), json!({"key": "device"}));
    let running = native
        .host
        .start("device".into(), install(), spec(), Vec::new())
        .await
        .unwrap();
    assert!(running.0.await.unwrap().success);
    assert!(!native.host.has_control("device"));
}

#[tokio::test]
async fn physical_screen_dimensions_are_read_before_opening_the_window() {
    let native = Native::new();
    let running = native.host.start("device".into(), install(), spec(), Vec::new()).await.unwrap();
    assert_eq!(native.request("shell")["command"], crate::core::device::display::DISPLAY_DUMP_COMMAND);
    let request = native.request("startSession");
    assert_eq!((request["screenWidth"].as_u64(), request["screenHeight"].as_u64()), (Some(1080), Some(2520)));
    assert!(running.0.await.unwrap().success);
}

#[tokio::test]
async fn app_query_runs_selected_server_and_returns_canonical_shell_output() {
    let native = Native::new();
    let result = native
        .host
        .query(
            install(),
            Query::Apps {
                serial: "test-device:40000".into(),
            },
            Duration::from_secs(1),
        )
        .await
        .unwrap();
    assert_eq!(result.stdout, b"AB");
    assert!(result.success);
    let request = native.request("queryServer");
    assert_eq!(request["serverPath"], json!(install().server()));
    assert_eq!(
        request["options"],
        json!(["list_apps=true", "cleanup=true"])
    );
    assert!(!native.host.supports_version("v4.0"));
    assert!(native.host.supports_version("v5.0.1"));
}

#[tokio::test]
async fn desktop_arguments_are_rejected_before_opening_a_native_window() {
    let native = Native::new();
    let mut spec = spec();
    spec.extra_args.push("--no-video".into());
    assert!(
        native
            .host
            .start("device".into(), install(), spec, Vec::new())
            .await
            .is_err()
    );
    assert!(native.calls.lock().unwrap().is_empty());
}
