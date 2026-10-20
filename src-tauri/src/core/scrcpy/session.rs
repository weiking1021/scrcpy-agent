//! Launching the official scrcpy client for one device (official mode).

use super::manager::Installed;
use super::protocol::StreamPlan;
use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;

/// Audio forwarding needs Android 11 (API 30).
pub const MIN_AUDIO_SDK: u32 = 30;

/// The title of a device's scrcpy window. It names the device and carries its
/// serial, so two windows never share a title and the toolbar can tell which
/// window is the one it belongs to.
pub fn window_title(name: &str, id: &str) -> String {
    format!("{name} ({id})")
}

pub struct LaunchSpec {
    pub serial: String,
    pub title: String,
    pub sdk: Option<u32>,
    /// Makes scrcpy connect to our proxy (empty when there is none); the
    /// arguments are those of the scrcpy version's protocol adapter.
    pub proxy_args: Vec<String>,
    /// The settings that are arguments of scrcpy (how the keyboard is sent, the bit
    /// rate of the video), in the form the scrcpy version's adapter gives them.
    pub option_args: Vec<String>,
    /// Makes scrcpy open an app (empty for the device's own screen); the
    /// arguments are those of the scrcpy version's protocol adapter.
    pub app_args: Vec<String>,
    pub extra_args: Vec<String>,
}

impl LaunchSpec {
    /// Which sockets scrcpy will open with these arguments.
    pub fn stream_plan(&self) -> StreamPlan {
        // Only the exact argument counts.
        let given = |argument: &str| self.extra_args.iter().any(|a| a == argument);
        StreamPlan {
            video: !given("--no-video"),
            // `build_args` adds `--no-audio` before Android 11.
            audio: !self.sdk.is_some_and(|s| s < MIN_AUDIO_SDK) && !given("--no-audio"),
            control: !given("--no-control"),
        }
    }
}

pub fn build_args(spec: &LaunchSpec) -> Vec<String> {
    let mut a = vec![
        format!("--serial={}", spec.serial),
        format!("--window-title={}", spec.title),
    ];
    if spec.sdk.is_some_and(|s| s < MIN_AUDIO_SDK) {
        a.push("--no-audio".into());
    }
    a.extend(spec.proxy_args.iter().cloned());
    a.extend(spec.option_args.iter().cloned());
    a.extend(spec.app_args.iter().cloned());
    a.extend(spec.extra_args.iter().cloned());
    a
}

/// The title of the window of an app opened on a device: it carries the device's
/// title, so it is unique to the device, and names the app.
pub fn app_window_title(app: &str, device_name: &str, id: &str) -> String {
    format!("{app} - {}", window_title(device_name, id))
}

/// What a scrcpy that was run to its end printed.
pub struct Output {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub success: bool,
}

/// Runs scrcpy with these arguments until it ends (for those that print
/// something and exit, such as listing the apps), within `limit`.
pub async fn run_to_end(install: &Installed, args: &[String], limit: std::time::Duration) -> Result<Output> {
    let mut cmd = Command::new(install.scrcpy());
    cmd.args(args)
        .current_dir(&install.root)
        .env("ADB", install.adb())
        .env("SCRCPY_SERVER_PATH", install.server())
        .stdin(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let output = tokio::time::timeout(limit, cmd.output())
        .await
        .context("scrcpy did not finish in time")?
        .with_context(|| format!("cannot start {}", install.scrcpy().display()))?;
    Ok(Output { stdout: output.stdout, stderr: output.stderr, success: output.status.success() })
}

pub struct Finished {
    pub success: bool,
    /// The last lines scrcpy printed; useful when it failed.
    pub tail: String,
}

/// Starts scrcpy and returns a handle to wait on it.
pub struct Running {
    child: tokio::process::Child,
    tail: Arc<Mutex<VecDeque<String>>>,
    readers: Vec<tokio::task::JoinHandle<()>>,
}

/// How long to wait for the output readers after scrcpy has exited. A process
/// that scrcpy started (such as the adb server) can keep the pipes open.
const READER_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

impl Running {
    pub async fn wait(mut self) -> Result<Finished> {
        let status = self.child.wait().await?;
        for reader in self.readers {
            // Each reader ends when its pipe closes; wait for the last lines.
            let _ = tokio::time::timeout(READER_GRACE, reader).await;
        }
        let tail = self.tail.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n");
        Ok(Finished { success: status.success(), tail })
    }
}

/// Must be called from inside the Tokio runtime: tokio's process and task
/// APIs panic without one, which would take the whole application down. That
/// mistake is reported as an error instead.
pub fn spawn(install: &Installed, spec: &LaunchSpec) -> Result<Running> {
    tokio::runtime::Handle::try_current()
        .context("scrcpy must be started from inside the async runtime")?;
    let mut cmd = Command::new(install.scrcpy());
    cmd.args(build_args(spec))
        .current_dir(&install.root)
        // Use the matching adb and server from the same release.
        .env("ADB", install.adb())
        .env("SCRCPY_SERVER_PATH", install.server())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(false);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console, scrcpy has its own window
    let mut child = cmd
        .spawn()
        .with_context(|| format!("cannot start {}", install.scrcpy().display()))?;

    let tail = Arc::new(Mutex::new(VecDeque::new()));
    let mut readers = Vec::new();
    if let Some(out) = child.stdout.take() {
        readers.push(tokio::spawn(collect(out, tail.clone())));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(tokio::spawn(collect(err, tail.clone())));
    }
    Ok(Running { child, tail, readers })
}

/// Keeps the last lines a stream printed. Lines are read as bytes: Windows
/// programs may write in the system code page rather than UTF-8, and reading
/// them as text would stop at the first such character and lose the rest.
async fn collect(stream: impl AsyncRead + Unpin, tail: Arc<Mutex<VecDeque<String>>>) {
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buf).trim_end().to_string();
                let mut t = tail.lock().unwrap();
                if t.len() >= 12 {
                    t.pop_front();
                }
                t.push_back(line);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(sdk: Option<u32>) -> LaunchSpec {
        LaunchSpec {
            serial: "192.168.1.5:40000".into(),
            title: "Pixel 7".into(),
            sdk,
            proxy_args: Vec::new(),
            app_args: Vec::new(),
            option_args: Vec::new(),
            extra_args: vec!["--max-fps=60".into()],
        }
    }

    #[test]
    fn the_sockets_scrcpy_opens_follow_from_its_arguments_and_the_android_version() {
        let plan = |sdk, extra: &[&str]| {
            LaunchSpec { sdk, extra_args: extra.iter().map(|a| a.to_string()).collect(), ..spec(sdk) }.stream_plan()
        };
        let all = StreamPlan { video: true, audio: true, control: true };
        assert_eq!(plan(Some(34), &[]), all);
        assert_eq!(plan(None, &[]), all);
        // Before Android 11 no audio is forwarded: scrcpy is told not to.
        assert_eq!(plan(Some(29), &[]), StreamPlan { audio: false, ..all });
        assert_eq!(plan(Some(34), &["--no-audio"]), StreamPlan { audio: false, ..all });
        assert_eq!(plan(Some(34), &["--no-control"]), StreamPlan { control: false, ..all });
        assert_eq!(plan(Some(34), &["--no-video"]), StreamPlan { video: false, ..all });
        assert_eq!(
            plan(Some(34), &["--max-fps=60", "--no-audio", "--no-control"]),
            StreamPlan { video: true, audio: false, control: false }
        );
        // Only the exact argument counts: nothing is read into others.
        assert_eq!(plan(Some(34), &["--no-audio=true", "--no-audio-playback", "no-control", "--window-title=--no-video"]), all);
    }

    #[test]
    fn the_window_of_an_app_is_titled_with_the_app_and_the_devices_own_title() {
        assert_eq!(app_window_title("Notes", "Pixel 7", "AAA111"), "Notes - Pixel 7 (AAA111)");
        // Two apps on one device, and one app on two devices, never share a title.
        assert_ne!(app_window_title("Notes", "Pixel 7", "AAA111"), app_window_title("Mail", "Pixel 7", "AAA111"));
        assert_ne!(app_window_title("Notes", "Pixel 7", "AAA111"), app_window_title("Notes", "Pixel 7", "BBB222"));
        // And none is the title of the device's own screen.
        assert_ne!(app_window_title("Notes", "Pixel 7", "AAA111"), window_title("Pixel 7", "AAA111"));
    }

    #[test]
    fn the_option_arguments_come_after_the_proxy_arguments_and_before_the_app_and_the_users_own() {
        let spec = LaunchSpec {
            proxy_args: vec!["--proxy".into()],
            option_args: vec!["--keyboard=uhid".into(), "--video-bit-rate=12M".into()],
            app_args: vec!["--start-app=a.b".into()],
            ..spec(Some(34))
        };
        assert_eq!(
            build_args(&spec),
            [
                "--serial=192.168.1.5:40000",
                "--window-title=Pixel 7",
                "--proxy",
                "--keyboard=uhid",
                "--video-bit-rate=12M",
                "--start-app=a.b",
                "--max-fps=60"
            ]
        );
    }

    #[test]
    fn the_app_arguments_follow_the_proxy_arguments_and_come_before_the_users_own() {
        let spec = LaunchSpec {
            proxy_args: vec!["--proxy".into()],
            app_args: vec!["--new-display".into(), "--start-app=a.b".into()],
            ..spec(Some(34))
        };
        assert_eq!(
            build_args(&spec),
            [
                "--serial=192.168.1.5:40000",
                "--window-title=Pixel 7",
                "--proxy",
                "--new-display",
                "--start-app=a.b",
                "--max-fps=60"
            ]
        );
    }

    #[test]
    fn the_proxy_arguments_come_before_the_users_own_arguments() {
        let with_proxy = LaunchSpec { proxy_args: vec!["--proxy-a".into(), "--proxy-b=1".into()], ..spec(Some(34)) };
        let args = build_args(&with_proxy);
        assert_eq!(
            args,
            [
                "--serial=192.168.1.5:40000",
                "--window-title=Pixel 7",
                "--proxy-a",
                "--proxy-b=1",
                "--max-fps=60"
            ]
        );
        // Without a proxy nothing of it is there.
        assert_eq!(build_args(&spec(Some(34))), ["--serial=192.168.1.5:40000", "--window-title=Pixel 7", "--max-fps=60"]);
    }

    #[test]
    fn the_window_title_names_the_device_and_carries_its_serial() {
        assert_eq!(window_title("Test Tablet", "a1b2c3d4"), "Test Tablet (a1b2c3d4)");
        // Two devices of the same model still get different titles.
        assert_ne!(window_title("Pixel 7", "AAA111"), window_title("Pixel 7", "BBB222"));
    }

    #[test]
    fn starting_outside_the_runtime_is_an_error_not_a_crash() {
        // This used to bring the whole application down when a command that
        // ran on the main thread started scrcpy.
        let install = Installed { version: "v0.0.0".into(), root: "nowhere".into() };
        assert!(spawn(&install, &spec(Some(34))).is_err());
    }

    /// Runs a real child process through `spawn` and `wait`. `cmd.exe`
    /// stands in for scrcpy: it ends by itself and prints something about the
    /// unknown command, which must be captured.
    #[cfg(windows)]
    #[tokio::test]
    async fn runs_the_client_and_captures_its_output() {
        let root = std::env::temp_dir().join(format!("scrcpy-agent-session-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::copy(r"C:\Windows\System32\cmd.exe", root.join("scrcpy.exe")).unwrap();
        let install = Installed { version: "v0.0.0".into(), root: root.clone() };

        let running = spawn(&install, &spec(Some(34))).unwrap();
        let done = tokio::time::timeout(std::time::Duration::from_secs(15), running.wait())
            .await
            .expect("the child process did not finish")
            .unwrap();
        assert!(!done.tail.is_empty(), "the output of the child was not captured");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn android_11_keeps_audio() {
        let a = build_args(&spec(Some(30)));
        assert!(!a.contains(&"--no-audio".to_string()));
        assert_eq!(a[0], "--serial=192.168.1.5:40000");
        assert_eq!(a.last().unwrap(), "--max-fps=60");
    }

    #[test]
    fn android_10_has_no_audio() {
        assert!(build_args(&spec(Some(29))).contains(&"--no-audio".to_string()));
    }

    #[test]
    fn unknown_sdk_tries_audio() {
        assert!(!build_args(&spec(None)).contains(&"--no-audio".to_string()));
    }
}
