//! The second way to get adb's answers: run the `adb` program and read what
//! it prints. This is how the first version worked.
//!
//! What this way cannot do, compared with `wire`:
//!
//! * No announcements of device changes (`track_devices` is `Unsupported`).
//! * `adb shell` ends with the command's own exit code, but also with 1 when
//!   adb itself fails (device not found), and the two cannot be told apart.
//! * A new process for every question.
//!
//! The text of `connect` and `pair` answers is adb's, in the user's language,
//! and is for showing only here as everywhere.

use super::api::{AdbApi, AdbError, DeviceTracker, ShellOutput};
use super::{AdbEntry, Forward, MdnsService, hide_window, parse_devices, parse_forwards, parse_mdns};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// How long most commands may take, and how long the ones that wait on the
/// network and the device (`connect`, `pair`) may take.
const SHORT_LIMIT: Duration = Duration::from_secs(10);
const LONG_LIMIT: Duration = Duration::from_secs(20);
/// How long a file may take to be copied to a device: an hour.
const PUSH_LIMIT: Duration = Duration::from_secs(3600);

/// The adb program.
pub struct CliAdb {
    pub path: PathBuf,
}

/// What a finished adb process left behind.
struct Finished {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit: u8,
}

impl CliAdb {
    pub fn new(path: PathBuf) -> Self {
        CliAdb { path }
    }

    async fn run(&self, args: &[&str], limit: Duration) -> Result<Finished, AdbError> {
        let mut command = Command::new(&self.path);
        command.args(args).stdin(Stdio::null()).kill_on_drop(true);
        hide_window(&mut command);
        let output = tokio::time::timeout(limit, command.output())
            .await
            .map_err(|_| AdbError::Io(std::io::ErrorKind::TimedOut))??;
        // A process that was killed has no exit code; 255 stands for "no normal exit".
        let exit = output.status.code().map_or(255, |code| u8::try_from(code).unwrap_or(255));
        Ok(Finished { stdout: output.stdout, stderr: output.stderr, exit })
    }

    /// A command whose answer is text: a non-zero exit is adb's failure, with
    /// what it said (on the error stream, or else the output).
    async fn text(&self, args: &[&str], limit: Duration) -> Result<String, AdbError> {
        let done = self.run(args, limit).await?;
        let out = String::from_utf8_lossy(&done.stdout).trim().to_string();
        if done.exit == 0 {
            return Ok(out);
        }
        let err = String::from_utf8_lossy(&done.stderr).trim().to_string();
        Err(AdbError::Fail(if err.is_empty() { out } else { err }))
    }
}

/// There are no announcements to read.
pub struct NoTracker;

impl DeviceTracker for NoTracker {
    async fn next(&mut self) -> Result<Vec<AdbEntry>, AdbError> {
        Err(AdbError::Unsupported)
    }
}

impl AdbApi for CliAdb {
    type Tracker = NoTracker;

    async fn track_devices(&self) -> Result<NoTracker, AdbError> {
        Err(AdbError::Unsupported)
    }

    async fn devices(&self) -> Result<Vec<AdbEntry>, AdbError> {
        Ok(parse_devices(&self.text(&["devices", "-l"], SHORT_LIMIT).await?))
    }

    async fn forwards(&self) -> Result<Vec<Forward>, AdbError> {
        Ok(parse_forwards(&self.text(&["forward", "--list"], SHORT_LIMIT).await?))
    }

    async fn mdns_services(&self) -> Result<Vec<MdnsService>, AdbError> {
        Ok(parse_mdns(&self.text(&["mdns", "services"], SHORT_LIMIT).await?))
    }

    async fn shell(&self, serial: &str, command: &str) -> Result<ShellOutput, AdbError> {
        let done = self.run(&["-s", serial, "shell", command], SHORT_LIMIT).await?;
        Ok(ShellOutput { stdout: done.stdout, stderr: done.stderr, exit: done.exit })
    }

    async fn push(
        &self,
        serial: &str,
        local: &std::path::Path,
        remote: &str,
        _progress: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<(), AdbError> {
        if remote.is_empty() || remote.contains(['\0', '\n']) {
            return Err(AdbError::BadRemotePath);
        }
        let local = local.to_str().ok_or(AdbError::Io(std::io::ErrorKind::InvalidInput))?;
        // The program prints progress for people, not in a form to rely on: the
        // exit code is the verdict and nothing is reported before the end.
        self.text(&["-s", serial, "push", local, remote], PUSH_LIMIT).await.map(|_| ())
    }

    async fn connect(&self, address: SocketAddr) -> Result<String, AdbError> {
        self.text(&["connect", &address.to_string()], LONG_LIMIT).await
    }

    async fn disconnect(&self, address: SocketAddr) -> Result<String, AdbError> {
        self.text(&["disconnect", &address.to_string()], SHORT_LIMIT).await
    }

    async fn pair(&self, address: SocketAddr, code: &str) -> Result<String, AdbError> {
        self.text(&["pair", &address.to_string(), code], LONG_LIMIT).await
    }
}
