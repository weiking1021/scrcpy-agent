//! What the application asks of adb, independent of how it is done.
//!
//! There are two ways to get these answers, and `Core::adb_for` is the one
//! line that picks between them:
//!
//! * `wire::AdbClient` speaks the adb server's own protocol over TCP:
//!   typed failures, exit codes, pushed device lists.
//! * `cli::CliAdb` runs the `adb` program and reads what it prints, which
//!   is how the first version worked.
//!
//! Both give the same typed answers. Anything an implementation cannot do is
//! reported as `AdbError::Unsupported`, never faked.

use super::{AdbEntry, Forward, MdnsService};
use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdbError {
    /// adb refused or failed; the text is adb's, for showing only.
    #[error("adb refused: {0}")]
    Fail(String),
    /// The first four bytes of an answer were neither `OKAY` nor `FAIL`.
    #[error("adb sent an unknown status {0:?}")]
    UnknownStatus([u8; 4]),
    /// A length was not four hexadecimal digits.
    #[error("adb sent a bad length {0:?}")]
    BadLength([u8; 4]),
    /// The connection ended in the middle of an answer.
    #[error("the connection to adb ended in the middle of an answer")]
    Truncated,
    /// The request does not fit the four-digit length.
    #[error("the request is too long ({0} bytes)")]
    RequestTooLong(usize),
    /// A `shell,v2:` packet with an id that is not 1, 2 or 3.
    #[error("unknown shell packet {0}")]
    UnknownShellPacket(u8),
    /// A `shell,v2:` exit packet that does not carry exactly one byte.
    #[error("a shell exit packet of {0} bytes")]
    BadExitPacket(usize),
    /// A `shell,v2:` stream that ended without an exit packet.
    #[error("the shell command's stream ended without an exit code")]
    MissingExit,
    /// The answer to `host:version` was not a hexadecimal number.
    #[error("adb sent a bad version {0:?}")]
    BadVersion(Vec<u8>),
    /// The connection itself failed, or the adb program could not be run.
    #[error("adb could not be reached: {0:?}")]
    Io(std::io::ErrorKind),
    /// The server closed a connection that was meant to stay open (it was
    /// stopped or restarted).
    #[error("adb closed the connection")]
    Closed,
    /// This implementation cannot do what was asked.
    #[error("not supported by this way of talking to adb")]
    Unsupported,
    /// A path on the device that cannot be sent: empty, or with a character the
    /// sync protocol cannot carry (a zero byte, a line break).
    #[error("not a usable path on the device")]
    BadRemotePath,
}

impl From<std::io::Error> for AdbError {
    fn from(error: std::io::Error) -> Self {
        match error.kind() {
            std::io::ErrorKind::UnexpectedEof => AdbError::Truncated,
            kind => AdbError::Io(kind),
        }
    }
}

/// What a command run on a device produced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShellOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit: u8,
}

/// The device lists adb announces whenever the list changes.
pub trait DeviceTracker: Send {
    /// The next list, as soon as it is announced. Every announcement holds the
    /// whole list, not the difference. `Err(Closed)` when adb ends the
    /// connection.
    fn next(&mut self) -> impl Future<Output = Result<Vec<AdbEntry>, AdbError>> + Send;
}

/// What the application asks of adb. Tests of the logic above it use a fake.
pub trait AdbApi: Send + Sync + 'static {
    type Tracker: DeviceTracker + 'static;

    /// The current device list, and then a new one at each change. An
    /// implementation that cannot be told of changes gives `Unsupported`; the
    /// caller then keeps looking at intervals.
    fn track_devices(&self) -> impl Future<Output = Result<Self::Tracker, AdbError>> + Send;

    /// The devices adb knows (`adb devices -l`).
    fn devices(&self) -> impl Future<Output = Result<Vec<AdbEntry>, AdbError>> + Send;

    /// The forwards adb has set up (`adb forward --list`).
    fn forwards(&self) -> impl Future<Output = Result<Vec<Forward>, AdbError>> + Send;

    /// The wireless-debugging services adb has found on the network.
    fn mdns_services(&self) -> impl Future<Output = Result<Vec<MdnsService>, AdbError>> + Send;

    /// Runs a command on a device. The command is given to the device's shell
    /// as one string, so the caller builds it from values it controls.
    fn shell(&self, serial: &str, command: &str) -> impl Future<Output = Result<ShellOutput, AdbError>> + Send;

    /// Copies a file of this computer to a path on a device. `progress` is told
    /// how many bytes the device has been sent so far, as they are sent (an
    /// implementation that cannot know says nothing until the end). An
    /// implementation that cannot do it at all gives `Unsupported`.
    fn push(
        &self,
        _serial: &str,
        _local: &Path,
        _remote: &str,
        _progress: &(dyn Fn(u64) + Send + Sync),
    ) -> impl Future<Output = Result<(), AdbError>> + Send {
        async { Err(AdbError::Unsupported) }
    }

    /// Connects to a device. The text is adb's own answer, for showing only:
    /// it is the same whether or not the device is then connected.
    fn connect(&self, address: SocketAddr) -> impl Future<Output = Result<String, AdbError>> + Send;

    fn disconnect(&self, address: SocketAddr) -> impl Future<Output = Result<String, AdbError>> + Send;

    /// Pairs with a device. The text is adb's own answer, for showing only: a
    /// failed pairing is also answered as a success. The caller checks that
    /// the code is six digits.
    fn pair(&self, address: SocketAddr, code: &str) -> impl Future<Output = Result<String, AdbError>> + Send;
}
