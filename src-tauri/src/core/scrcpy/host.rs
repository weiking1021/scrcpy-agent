//! Platform execution boundary. Services describe intent; hosts own processes and presentation.
use super::{
    manager::Installed,
    protocol::{ControlMessage, StreamPlan},
    session::{Finished, Output},
};
use crate::core::{
    apps::{AppDisplay, PackageName},
    device::display::ScreenSize,
    model::{BitRateMbps, KeyboardMode, MaxSize},
};
use anyhow::Result;
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

#[cfg(any(target_os = "android", test))]
pub mod android;
#[cfg(not(target_os = "android"))]
mod desktop;

pub type HostFuture<T> = Pin<Box<dyn Future<Output = Result<T>> + Send>>;

#[derive(Clone)]
pub struct AppLaunch {
    pub package: PackageName,
    pub display: AppDisplay,
    pub size: Option<ScreenSize>,
    pub keep_awake: bool,
}

#[derive(Clone)]
pub struct SessionSpec {
    pub serial: String,
    pub title: String,
    pub sdk: Option<u32>,
    pub app: Option<AppLaunch>,
    pub keyboard: KeyboardMode,
    pub bit_rate: BitRateMbps,
    pub max_size: MaxSize,
    pub extra_args: Vec<String>,
}

impl SessionSpec {
    pub fn stream_plan(&self) -> StreamPlan {
        let given = |argument: &str| self.extra_args.iter().any(|a| a == argument);
        StreamPlan {
            video: !given("--no-video"),
            audio: !self.sdk.is_some_and(|s| s < super::session::MIN_AUDIO_SDK)
                && !given("--no-audio"),
            control: !given("--no-control"),
        }
    }
}

pub enum Query {
    Apps { serial: String },
}

/// The owner of a running session waits for this once; completion also owns cleanup.
pub struct RunningSession(pub HostFuture<Finished>);

pub trait ScrcpyHost: Send + Sync {
    fn prepare(&self, _base: std::path::PathBuf) -> HostFuture<()> {
        Box::pin(async { Ok(()) })
    }
    fn supports_version(&self, _version: &str) -> bool {
        true
    }
    fn manages_adb_server(&self) -> bool {
        false
    }
    fn supports_proxy(&self) -> bool {
        false
    }
    fn start(
        &self,
        key: String,
        install: Installed,
        spec: SessionSpec,
        proxy_args: Vec<String>,
    ) -> HostFuture<RunningSession>;
    fn query(&self, install: Installed, query: Query, limit: Duration) -> HostFuture<Output>;
    fn has_control(&self, _key: &str) -> bool {
        false
    }
    fn send_control(&self, _key: &str, _message: &ControlMessage) -> Result<()> {
        anyhow::bail!("session has no control channel")
    }
}

pub fn system_host() -> Arc<dyn ScrcpyHost> {
    #[cfg(not(target_os = "android"))]
    {
        Arc::new(desktop::DesktopHost)
    }
    #[cfg(target_os = "android")]
    {
        Arc::new(MissingHost)
    }
}

#[cfg(target_os = "android")]
struct MissingHost;
#[cfg(target_os = "android")]
impl ScrcpyHost for MissingHost {
    fn start(
        &self,
        _key: String,
        _install: Installed,
        _spec: SessionSpec,
        _proxy_args: Vec<String>,
    ) -> HostFuture<RunningSession> {
        Box::pin(async { anyhow::bail!("Android scrcpy host was not supplied") })
    }
    fn query(&self, _install: Installed, _query: Query, _limit: Duration) -> HostFuture<Output> {
        Box::pin(async { anyhow::bail!("Android scrcpy host was not supplied") })
    }
}
