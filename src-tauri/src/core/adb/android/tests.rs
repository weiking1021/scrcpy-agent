use super::*;
use crate::core::bridge::{NativeEvents, NativeReply};
use serde_json::Value;

type Listener = Arc<dyn Fn(Value) + Send + Sync>;

fn native(fail: bool) -> (AndroidAdb, Arc<Mutex<Vec<Value>>>) {
    let input: Arc<Mutex<Option<Listener>>> = Default::default();
    let requests: Arc<Mutex<Vec<Value>>> = Default::default();
    let events: NativeEvents = {
        let input = input.clone();
        Arc::new(move |event, handler| {
            assert_eq!(event, "adb-push");
            *input.lock().unwrap() = Some(handler);
            Box::pin(async { Ok(json!({})) })
        })
    };
    let request = {
        let requests = requests.clone();
        Arc::new(move |operation: String, data: Value| -> NativeReply {
            assert_eq!(operation, "push");
            requests.lock().unwrap().push(data.clone());
            let handler = input.lock().unwrap().clone().unwrap();
            Box::pin(async move {
                handler(json!({"id": data["id"], "sent": 2}));
                tokio::task::yield_now().await;
                if fail {
                    Err("No space left on device".into())
                } else {
                    Ok(json!(4))
                }
            })
        })
    };
    (AndroidAdb::new(NativeBridge { request, events }), requests)
}

async fn file(name: &str) -> std::path::PathBuf {
    let path =
        std::env::temp_dir().join(format!("scrcpy-android-push-{name}-{}", std::process::id()));
    tokio::fs::write(&path, b"test").await.unwrap();
    path
}

#[tokio::test]
async fn push_uses_selected_paths_and_reports_real_progress_without_retaining_callbacks() {
    let (adb, requests) = native(false);
    let path = file("success").await;
    let reported = Mutex::new(Vec::new());
    adb.push(
        "test-device:40000",
        &path,
        "/sdcard/Download/test.txt",
        &|sent| reported.lock().unwrap().push(sent),
    )
    .await
    .unwrap();
    {
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["serial"], "test-device:40000");
        assert_eq!(requests[0]["local"], json!(path));
        assert_eq!(requests[0]["remote"], "/sdcard/Download/test.txt");
        let reported = reported.lock().unwrap();
        assert_eq!(reported.last(), Some(&4));
        assert!(reported.windows(2).all(|pair| pair[0] < pair[1]));
    }
    assert!(adb.pushes.pending.lock().unwrap().is_empty());
    tokio::fs::remove_file(path).await.unwrap();
}

#[tokio::test]
async fn remote_failure_propagates_and_releases_progress_subscription() {
    let (adb, _) = native(true);
    let path = file("failure").await;
    assert_eq!(
        adb.push("test-device:40000", &path, "/sdcard/test", &|_| {})
            .await,
        Err(AdbError::Fail("No space left on device".into()))
    );
    assert!(adb.pushes.pending.lock().unwrap().is_empty());
    tokio::fs::remove_file(path).await.unwrap();
}

#[tokio::test]
async fn invalid_sources_and_destinations_are_rejected_before_native_requests() {
    let (adb, requests) = native(false);
    assert_eq!(
        adb.push(
            "test-device:40000",
            Path::new("unused"),
            "/sdcard/a\nb",
            &|_| {}
        )
        .await,
        Err(AdbError::BadRemotePath)
    );
    let missing = std::env::temp_dir().join(format!("scrcpy-missing-push-{}", std::process::id()));
    assert!(matches!(
        adb.push("test-device:40000", &missing, "/sdcard/test", &|_| {})
            .await,
        Err(AdbError::Io(_))
    ));
    assert!(requests.lock().unwrap().is_empty());
}
