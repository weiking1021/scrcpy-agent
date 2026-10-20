//! `CliAdb` against a stand-in adb program (`examples/fake_adb_cli.rs`) that
//! answers with output and exit codes in the form the real adb 37.0.1 has (all
//! values are invented).

mod common;

use scrcpy_agent_lib::core::adb::{AdbState, MdnsKind, SerialKind};
use scrcpy_agent_lib::core::adb::api::{AdbApi, AdbError, ShellOutput};
use scrcpy_agent_lib::core::adb::cli::CliAdb;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

const SERIAL: &str = "adb-a1b2c3d4-Ab12Cd._adb-tls-connect._tcp.";

fn example(name: &str) -> PathBuf {
    let exe = std::env::current_exe().expect("test executable path");
    let target_dir = exe.parent().and_then(Path::parent).expect("target directory");
    let path = target_dir.join("examples").join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    assert!(
        path.exists(),
        "{} has not been built. Run the full `cargo test`: a single test target does not build the examples.",
        path.display()
    );
    path
}

fn addr(s: &str) -> SocketAddr {
    s.parse().unwrap()
}

/// A scripted adb program in a folder of its own.
struct Fake {
    dir: PathBuf,
    adb: CliAdb,
}

impl Fake {
    fn new(name: &str, script: Value) -> Fake {
        let dir = std::env::temp_dir().join(format!("scrcpy-agent-cli-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("adb{}", std::env::consts::EXE_SUFFIX));
        common::place_program(&example("fake_adb_cli"), &path);
        std::fs::write(dir.join("script.json"), script.to_string()).unwrap();
        Fake { dir, adb: CliAdb::new(path) }
    }

    /// The arguments of every call, in order.
    fn calls(&self) -> Vec<Vec<String>> {
        std::fs::read_to_string(self.dir.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn call(args: &[&str], stdout: &str, stderr: &str, exit: i32) -> Value {
    json!({ "args": args, "stdout": stdout, "stderr": stderr, "exit": exit })
}

#[tokio::test]
async fn the_device_list_is_the_parsed_output_of_devices_l() {
    let fake = Fake::new(
        "devices",
        json!([call(
            &["devices", "-l"],
            "List of devices attached\r\nadb-a1b2c3d4-Ab12Cd._adb-tls-connect._tcp. device product:fake_global model:MODEL0001 device:fake transport_id:16\r\n\r\n",
            "",
            0
        )]),
    );
    let devices = fake.adb.devices().await.unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].serial, SERIAL);
    assert_eq!(devices[0].kind, SerialKind::MdnsName { instance: "adb-a1b2c3d4-Ab12Cd".into() });
    assert_eq!(devices[0].state, AdbState::Device);
    assert_eq!(devices[0].model.as_deref(), Some("MODEL0001"));
    assert_eq!(fake.calls(), [["devices", "-l"]]);
}

#[tokio::test]
async fn the_mdns_services_are_the_parsed_output_of_mdns_services() {
    let fake = Fake::new(
        "mdns",
        json!([call(
            &["mdns", "services"],
            "List of discovered mdns services\r\nadb-ABCDE12345-Zq7Rt3\t_adb-tls-connect._tcp\t192.168.250.146:41500\r\n\r\n",
            "",
            0
        )]),
    );
    let services = fake.adb.mdns_services().await.unwrap();
    assert_eq!(services.len(), 1);
    assert_eq!(services[0].serial().as_deref(), Some("ABCDE12345"));
    assert_eq!(services[0].kind, MdnsKind::Connect);
    assert_eq!(services[0].address, addr("192.168.250.146:41500"));
}

#[tokio::test]
async fn the_forwards_are_the_parsed_output_of_forward_list() {
    use scrcpy_agent_lib::core::adb::{Forward, ForwardRemote};
    let fake = Fake::new(
        "forwards",
        json!([call(&["forward", "--list"], "SERIAL-A tcp:27183 localabstract:scrcpy_1a2b3c4d\r\n", "", 0)]),
    );
    assert_eq!(
        fake.adb.forwards().await,
        Ok(vec![Forward {
            serial: "SERIAL-A".into(),
            local_port: 27183,
            remote: ForwardRemote::LocalAbstract("scrcpy_1a2b3c4d".into())
        }])
    );
    assert_eq!(fake.calls(), [["forward", "--list"]]);

    let failing = Fake::new("forwards-fail", json!([call(&["forward", "--list"], "", "error: no server\r\n", 1)]));
    assert_eq!(failing.adb.forwards().await, Err(AdbError::Fail("error: no server".into())));
}

/// What adb says when `connect` does not work: free text in the language of the user's
/// computer. Invented sentences in three languages (the third, Traditional Chinese, is
/// written in escapes so that no source line holds it).
const CONNECT_SAID_IN_THREE_LANGUAGES: [&str; 3] = [
    "cannot connect to 127.0.0.1:1: Verbindung nicht möglich, da der Zielcomputer sie verweigerte. (10061)",
    "cannot connect to 127.0.0.1:1: Не удалось подключиться: компьютер отверг запрос. (10061)",
    "cannot connect to 127.0.0.1:1: \u{7121}\u{6cd5}\u{9023}\u{7dda}\u{3002} (10061)",
];

#[tokio::test]
async fn connect_gives_adbs_own_words_even_when_it_did_not_work_whatever_the_language() {
    // The behaviour is real: adb exits 0 and says it in the computer's language.
    for (index, said) in CONNECT_SAID_IN_THREE_LANGUAGES.into_iter().enumerate() {
        let fake = Fake::new(
            &format!("connect-{index}"),
            json!([call(&["connect", "127.0.0.1:1"], &format!("{said}\r\n"), "", 0)]),
        );
        assert_eq!(fake.adb.connect(addr("127.0.0.1:1")).await, Ok(said.to_string()));
    }
}

#[tokio::test]
async fn a_command_adb_fails_is_adbs_failure_with_what_it_said() {
    let fake = Fake::new(
        "failures",
        json!([
            call(&["disconnect", "127.0.0.1:1"], "", "error: no such device '127.0.0.1:1'\r\n", 1),
            call(
                &["pair", "127.0.0.1:1", "123456"],
                "",
                "error: protocol fault (couldn't read status message): No error\r\n",
                1
            ),
        ]),
    );
    assert_eq!(
        fake.adb.disconnect(addr("127.0.0.1:1")).await,
        Err(AdbError::Fail("error: no such device '127.0.0.1:1'".into()))
    );
    assert_eq!(
        fake.adb.pair(addr("127.0.0.1:1"), "123456").await,
        Err(AdbError::Fail("error: protocol fault (couldn't read status message): No error".into()))
    );
}

#[tokio::test]
async fn a_shell_command_is_one_argument_after_the_device_and_ends_with_its_exit_code() {
    let fake = Fake::new(
        "shell",
        json!([
            call(&["-s", SERIAL, "shell", "getprop ro.build.version.sdk"], "30\n", "", 0),
            call(&["-s", SERIAL, "shell", "exit 3"], "", "", 3),
        ]),
    );
    assert_eq!(
        fake.adb.shell(SERIAL, "getprop ro.build.version.sdk").await,
        Ok(ShellOutput { stdout: b"30\n".to_vec(), stderr: Vec::new(), exit: 0 })
    );
    assert_eq!(fake.adb.shell(SERIAL, "exit 3").await.unwrap().exit, 3);
}

#[tokio::test]
async fn adbs_own_failure_to_find_the_device_looks_like_an_exit_code_of_1() {
    // Real. This is the limit of the command line: adb's failure and the
    // command's own exit code 1 cannot be told apart. (The adb server's
    // protocol does not have this problem.)
    let fake = Fake::new(
        "shell-missing",
        json!([call(&["-s", "nonexistent", "shell", "exit 3"], "", "adb.exe: device 'nonexistent' not found\r\n", 1)]),
    );
    assert_eq!(
        fake.adb.shell("nonexistent", "exit 3").await,
        Ok(ShellOutput {
            stdout: Vec::new(),
            stderr: b"adb.exe: device 'nonexistent' not found\r\n".to_vec(),
            exit: 1
        })
    );
}

#[tokio::test]
async fn a_push_is_one_adb_call_with_the_device_the_file_and_the_path_and_ends_with_its_exit_code() {
    let local = std::env::temp_dir().join("scrcpy-agent-cli-push-source.txt");
    let local_arg = local.to_str().unwrap().to_string();
    let fake = Fake::new(
        "push",
        json!([
            call(&["-s", SERIAL, "push", &local_arg, "/sdcard/Download/x.txt"], "1 file pushed, 0 skipped.\n", "", 0),
            call(
                &["-s", SERIAL, "push", &local_arg, "/sdcard/Download/y.txt"],
                "",
                "adb: error: failed to copy: remote couldn't create file: No space left on device\n",
                1
            ),
        ]),
    );
    let nothing = |_: u64| {};
    assert_eq!(fake.adb.push(SERIAL, &local, "/sdcard/Download/x.txt", &nothing).await, Ok(()));
    assert_eq!(
        fake.adb.push(SERIAL, &local, "/sdcard/Download/y.txt", &nothing).await,
        Err(AdbError::Fail("adb: error: failed to copy: remote couldn't create file: No space left on device".into()))
    );
    assert_eq!(
        fake.calls(),
        [
            vec!["-s".to_string(), SERIAL.to_string(), "push".into(), local_arg.clone(), "/sdcard/Download/x.txt".into()],
            vec!["-s".to_string(), SERIAL.to_string(), "push".into(), local_arg, "/sdcard/Download/y.txt".into()],
        ]
    );
}

#[tokio::test]
async fn a_path_that_cannot_be_sent_is_refused_without_starting_adb() {
    let fake = Fake::new("push-bad", json!([]));
    let nothing = |_: u64| {};
    let local = std::env::temp_dir().join("scrcpy-agent-cli-push-source.txt");
    for bad in ["", "/sdcard/with\nbreak", "/sdcard/with\0nul"] {
        assert_eq!(fake.adb.push(SERIAL, &local, bad, &nothing).await, Err(AdbError::BadRemotePath), "{bad:?}");
    }
    assert!(fake.calls().is_empty(), "no process should have been started");
}

#[tokio::test]
async fn announcements_are_not_available_from_the_command_line() {
    let fake = Fake::new("track", json!([]));
    assert_eq!(fake.adb.track_devices().await.err(), Some(AdbError::Unsupported));
    assert!(fake.calls().is_empty(), "no process should have been started");
}

#[tokio::test]
async fn a_missing_adb_program_is_an_io_error_not_a_hang() {
    let adb = CliAdb::new(std::env::temp_dir().join("scrcpy-agent-no-such-adb-program"));
    assert_eq!(adb.devices().await, Err(AdbError::Io(std::io::ErrorKind::NotFound)));
}

#[tokio::test]
async fn a_call_nobody_scripted_is_a_failure_with_the_programs_exit_code_text() {
    // The stand-in exits with 99 for unscripted calls; for a text command that is a failure.
    let fake = Fake::new("unscripted", json!([]));
    let result = fake.adb.devices().await;
    assert!(matches!(result, Err(AdbError::Fail(ref text)) if text == "fake adb: no scripted answer for [\"devices\", \"-l\"]"), "{result:?}");
}
