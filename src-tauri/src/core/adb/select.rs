//! The adb implementation the user's settings ask for.
//!
//! Which of the two ways of talking to adb is used is read on every call, so
//! changing the setting takes effect at once, without a restart. This is the
//! only place that looks at the setting; everything else uses `AdbApi`.

use super::api::{AdbApi, AdbError, ShellOutput};
use super::cli::CliAdb;
use super::wire::{AdbClient, TcpConnect, Tracker};
use super::{AdbEntry, Forward, MdnsService};
use crate::core::model::AdbBackend;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Talks to adb the way the shared setting says.
pub struct ConfiguredAdb {
    /// The setting; whoever changes it changes it for every holder of this `Arc`.
    pub backend: Arc<Mutex<AdbBackend>>,
    /// Where the adb server listens (for the server way).
    pub server: SocketAddr,
    /// The adb program (for the command-line way).
    pub program: PathBuf,
}

impl ConfiguredAdb {
    pub fn new(backend: Arc<Mutex<AdbBackend>>, server: SocketAddr, program: PathBuf) -> Self {
        ConfiguredAdb { backend, server, program }
    }

    fn current(&self) -> AdbBackend {
        *self.backend.lock().unwrap()
    }

    fn wire(&self) -> AdbClient<TcpConnect> {
        AdbClient::new(TcpConnect(self.server))
    }

    fn cli(&self) -> CliAdb {
        CliAdb::new(self.program.clone())
    }
}

impl AdbApi for ConfiguredAdb {
    /// Only the server way can announce changes; the other gives `Unsupported`.
    type Tracker = Tracker<tokio::net::TcpStream>;

    async fn track_devices(&self) -> Result<Self::Tracker, AdbError> {
        match self.current() {
            AdbBackend::Server => self.wire().track_devices().await,
            AdbBackend::CommandLine => Err(AdbError::Unsupported),
        }
    }

    async fn devices(&self) -> Result<Vec<AdbEntry>, AdbError> {
        match self.current() {
            AdbBackend::Server => self.wire().devices().await,
            AdbBackend::CommandLine => self.cli().devices().await,
        }
    }

    async fn forwards(&self) -> Result<Vec<Forward>, AdbError> {
        match self.current() {
            AdbBackend::Server => self.wire().forwards().await,
            AdbBackend::CommandLine => self.cli().forwards().await,
        }
    }

    async fn mdns_services(&self) -> Result<Vec<MdnsService>, AdbError> {
        match self.current() {
            AdbBackend::Server => self.wire().mdns_services().await,
            AdbBackend::CommandLine => self.cli().mdns_services().await,
        }
    }

    async fn shell(&self, serial: &str, command: &str) -> Result<ShellOutput, AdbError> {
        match self.current() {
            AdbBackend::Server => self.wire().shell(serial, command).await,
            AdbBackend::CommandLine => self.cli().shell(serial, command).await,
        }
    }

    async fn push(
        &self,
        serial: &str,
        local: &std::path::Path,
        remote: &str,
        progress: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<(), AdbError> {
        match self.current() {
            AdbBackend::Server => self.wire().push(serial, local, remote, progress).await,
            AdbBackend::CommandLine => self.cli().push(serial, local, remote, progress).await,
        }
    }

    async fn connect(&self, address: SocketAddr) -> Result<String, AdbError> {
        match self.current() {
            AdbBackend::Server => self.wire().connect(address).await,
            AdbBackend::CommandLine => self.cli().connect(address).await,
        }
    }

    async fn disconnect(&self, address: SocketAddr) -> Result<String, AdbError> {
        match self.current() {
            AdbBackend::Server => self.wire().disconnect(address).await,
            AdbBackend::CommandLine => self.cli().disconnect(address).await,
        }
    }

    async fn pair(&self, address: SocketAddr, code: &str) -> Result<String, AdbError> {
        match self.current() {
            AdbBackend::Server => self.wire().pair(address, code).await,
            AdbBackend::CommandLine => self.cli().pair(address, code).await,
        }
    }
}
