//! Typed calls to a native host. No device or scrcpy policy lives in this transport.
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{future::Future, pin::Pin, sync::Arc};

pub type NativeReply = Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>;
pub type NativeRequest = Arc<dyn Fn(String, Value) -> NativeReply + Send + Sync>;
pub type NativeEvents =
    Arc<dyn Fn(String, Arc<dyn Fn(Value) + Send + Sync>) -> NativeReply + Send + Sync>;

#[derive(Clone)]
pub struct NativeBridge {
    pub request: NativeRequest,
    pub events: NativeEvents,
}

impl NativeBridge {
    pub async fn call<T: DeserializeOwned>(
        &self,
        operation: &str,
        data: Value,
    ) -> anyhow::Result<T> {
        let reply = (self.request)(operation.into(), data)
            .await
            .map_err(anyhow::Error::msg)?;
        Ok(serde_json::from_value(reply)?)
    }

    pub async fn listen(
        &self,
        event: &str,
        handler: Arc<dyn Fn(Value) + Send + Sync>,
    ) -> anyhow::Result<()> {
        (self.events)(event.into(), handler)
            .await
            .map_err(anyhow::Error::msg)?;
        Ok(())
    }
}
