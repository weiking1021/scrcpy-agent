//! Convert Android's staged URI imports into the same requests as desktop file menus.
use super::*;
use crate::core::requests::shell_request::ShellAction;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use tokio::sync::Notify;

#[derive(Deserialize)]
struct SharedFiles {
    batch: String,
    action: String,
    files: Vec<PathBuf>,
}

struct Import {
    bridge: NativeBridge,
    batch: String,
}

impl Drop for Import {
    fn drop(&mut self) {
        let bridge = self.bridge.clone();
        let batch = self.batch.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = bridge
                    .call::<serde_json::Value>("releaseShare", json!({"batch": batch}))
                    .await;
            });
        }
    }
}

pub async fn receive(core: Shared, bridge: NativeBridge) -> anyhow::Result<()> {
    let changed = Arc::new(Notify::new());
    let notify = changed.clone();
    // Listen before draining: a share received during startup cannot be lost.
    bridge
        .listen("share", Arc::new(move |_| notify.notify_one()))
        .await?;
    loop {
        let pending: Vec<SharedFiles> = bridge.call("takeShares", json!({})).await?;
        for request in pending {
            let resource = Import {
                bridge: bridge.clone(),
                batch: request.batch,
            };
            let Some(action) = ShellAction::parse(&request.action) else {
                continue;
            };
            if request.files.is_empty() {
                continue;
            }
            core.devices()
                .wait_until_devices_known(std::time::Duration::from_secs(20))
                .await;
            core.requests()
                .handle_shell_with_resource(action, request.files, resource)
                .await;
        }
        changed.notified().await;
    }
}
