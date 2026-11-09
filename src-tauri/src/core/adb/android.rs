//! Android's direct adbd transport. Device policy stays in the shared services.
use super::api::{AdbApi, AdbError, DeviceTracker, ShellOutput};
use super::{AdbEntry, Forward, MdnsService, parse_devices, parse_mdns};
use crate::core::bridge::NativeBridge;
use serde_json::json;
use std::net::SocketAddr;
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::{OnceCell, watch};

#[cfg(test)]
mod tests;

#[derive(Clone)]
pub struct AndroidAdb {
    bridge: NativeBridge,
    pushes: Arc<Pushes>,
}

#[derive(Default)]
struct Pushes {
    ready: OnceCell<()>,
    next: AtomicU64,
    pending: Mutex<HashMap<u64, watch::Sender<u64>>>,
}

struct PushSubscription {
    pushes: Arc<Pushes>,
    id: u64,
}
impl Drop for PushSubscription {
    fn drop(&mut self) {
        self.pushes.pending.lock().unwrap().remove(&self.id);
    }
}

impl AndroidAdb {
    pub fn new(bridge: NativeBridge) -> Self {
        Self {
            bridge,
            pushes: Default::default(),
        }
    }

    async fn listen_for_pushes(&self) -> anyhow::Result<()> {
        self.pushes
            .ready
            .get_or_try_init(|| async {
                let pushes = Arc::downgrade(&self.pushes);
                self.bridge
                    .listen(
                        "adb-push",
                        Arc::new(move |value| {
                            #[derive(serde::Deserialize)]
                            struct Progress {
                                id: u64,
                                sent: u64,
                            }
                            let Ok(event) = serde_json::from_value::<Progress>(value) else {
                                return;
                            };
                            let Some(pushes) = pushes.upgrade() else {
                                return;
                            };
                            if let Some(sender) = pushes.pending.lock().unwrap().get(&event.id) {
                                sender.send_replace(event.sent);
                            }
                        }),
                    )
                    .await
            })
            .await?;
        Ok(())
    }
}

fn failure(error: anyhow::Error) -> AdbError {
    AdbError::Fail(error.to_string())
}

pub struct NoTracker;
impl DeviceTracker for NoTracker {
    async fn next(&mut self) -> Result<Vec<AdbEntry>, AdbError> {
        Err(AdbError::Unsupported)
    }
}

impl AdbApi for AndroidAdb {
    type Tracker = NoTracker;
    async fn track_devices(&self) -> Result<NoTracker, AdbError> {
        Err(AdbError::Unsupported)
    }
    async fn devices(&self) -> Result<Vec<AdbEntry>, AdbError> {
        let text: String = self
            .bridge
            .call("devices", json!({}))
            .await
            .map_err(failure)?;
        Ok(parse_devices(&text))
    }
    async fn mdns_services(&self) -> Result<Vec<MdnsService>, AdbError> {
        let text: String = self.bridge.call("mdns", json!({})).await.map_err(failure)?;
        Ok(parse_mdns(&text))
    }
    async fn forwards(&self) -> Result<Vec<Forward>, AdbError> {
        Err(AdbError::Unsupported)
    }
    async fn shell(&self, serial: &str, command: &str) -> Result<ShellOutput, AdbError> {
        self.bridge
            .call("shell", json!({"serial": serial, "command": command}))
            .await
            .map_err(failure)
    }
    async fn connect(&self, address: SocketAddr) -> Result<String, AdbError> {
        self.bridge
            .call("connect", json!({"address": address}))
            .await
            .map_err(failure)
    }
    async fn disconnect(&self, address: SocketAddr) -> Result<String, AdbError> {
        self.bridge
            .call("disconnect", json!({"address": address}))
            .await
            .map_err(failure)
    }
    async fn pair(&self, address: SocketAddr, code: &str) -> Result<String, AdbError> {
        self.bridge
            .call("pair", json!({"address": address, "code": code}))
            .await
            .map_err(failure)
    }

    async fn push(
        &self,
        serial: &str,
        local: &Path,
        remote: &str,
        progress: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<(), AdbError> {
        if remote.is_empty() || remote.contains(['\0', '\n']) {
            return Err(AdbError::BadRemotePath);
        }
        let local = local
            .to_str()
            .ok_or(AdbError::Io(std::io::ErrorKind::InvalidInput))?;
        if !tokio::fs::metadata(local).await?.is_file() {
            return Err(AdbError::Io(std::io::ErrorKind::InvalidInput));
        }
        self.listen_for_pushes().await.map_err(failure)?;
        let id = self.pushes.next.fetch_add(1, Ordering::Relaxed);
        let (sender, mut receiver) = watch::channel(0);
        self.pushes.pending.lock().unwrap().insert(id, sender);
        let _subscription = PushSubscription {
            pushes: self.pushes.clone(),
            id,
        };
        let request = self.bridge.call::<u64>(
            "push",
            json!({"id": id, "serial": serial, "local": local, "remote": remote}),
        );
        tokio::pin!(request);
        let mut reported = 0;
        loop {
            tokio::select! {
                result = &mut request => {
                    let sent = result.map_err(failure)?;
                    if sent > reported { progress(sent); }
                    return Ok(());
                }
                changed = receiver.changed() => {
                    if changed.is_err() { continue; }
                    let sent = *receiver.borrow_and_update();
                    if sent > reported { progress(sent); reported = sent; }
                }
            }
        }
    }
}
