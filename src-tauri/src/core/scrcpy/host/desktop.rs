use super::*;
use crate::core::scrcpy::{
    protocol::protocol_for,
    session::{self, LaunchSpec},
    version::ScrcpyVersion,
};

pub struct DesktopHost;

impl ScrcpyHost for DesktopHost {
    fn manages_adb_server(&self) -> bool {
        true
    }
    fn supports_proxy(&self) -> bool {
        true
    }

    fn start(
        &self,
        _key: String,
        install: Installed,
        spec: SessionSpec,
        proxy_args: Vec<String>,
    ) -> HostFuture<RunningSession> {
        Box::pin(async move {
            let adapter = ScrcpyVersion::parse(&install.version).and_then(|v| protocol_for(&v));
            let option_args = adapter
                .as_ref()
                .map(|p| {
                    [
                        p.keyboard_args(spec.keyboard),
                        p.video_bit_rate_args(spec.bit_rate),
                        p.max_size_args(spec.max_size),
                    ]
                    .concat()
                })
                .unwrap_or_default();
            let app_args = match &spec.app {
                Some(app) => adapter
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("unsupported scrcpy app protocol"))?
                    .start_app_args(&app.package, app.display, app.size, app.keep_awake),
                None => Vec::new(),
            };
            let process = session::spawn(
                &install,
                &LaunchSpec {
                    serial: spec.serial,
                    title: spec.title,
                    sdk: spec.sdk,
                    proxy_args,
                    option_args,
                    app_args,
                    extra_args: spec.extra_args,
                },
            )?;
            Ok(RunningSession(Box::pin(process.wait())))
        })
    }

    fn query(&self, install: Installed, query: Query, limit: Duration) -> HostFuture<Output> {
        Box::pin(async move {
            let adapter = ScrcpyVersion::parse(&install.version)
                .and_then(|v| protocol_for(&v))
                .ok_or_else(|| anyhow::anyhow!("unsupported scrcpy query protocol"))?;
            let args = match query {
                Query::Apps { serial } => adapter.list_apps_args(&serial),
            };
            session::run_to_end(&install, &args, limit).await
        })
    }
}
