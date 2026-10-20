//! The adb implementation follows the user's setting, and follows it when the
//! setting changes. Both implementations are stand-ins here: a small adb
//! server that speaks the protocol, and the scripted adb program of
//! `examples/fake_adb_cli.rs`.

mod common;

use scrcpy_agent_lib::core::adb::api::{AdbApi, AdbError, DeviceTracker, ShellOutput};
use scrcpy_agent_lib::core::adb::select::ConfiguredAdb;
use scrcpy_agent_lib::core::model::AdbBackend;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const SERIAL: &str = "TESTSERIAL001";
const LIST_LINE: &str = "TESTSERIAL001 device product:fake_global model:MODEL0001 device:fake transport_id:1";

fn example(name: &str) -> PathBuf {
    let exe = std::env::current_exe().expect("test executable path");
    let target_dir = exe.parent().and_then(Path::parent).expect("target directory");
    let path = target_dir.join("examples").join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    assert!(path.exists(), "{} has not been built. Run the full `cargo test`.", path.display());
    path
}

fn block(text: &str) -> Vec<u8> {
    [format!("{:04x}", text.len()).into_bytes(), text.as_bytes().to_vec()].concat()
}

/// An adb server that answers the device list, tracking and shell requests,
/// and remembers every service it was asked for.
fn fake_server() -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = asked.clone();
    std::thread::spawn(move || {
        let mut kept: Vec<TcpStream> = Vec::new();
        for mut stream in listener.incoming().flatten() {
            loop {
                let mut head = [0u8; 4];
                if stream.read_exact(&mut head).is_err() {
                    break;
                }
                let mut body = vec![0u8; usize::from_str_radix(std::str::from_utf8(&head).unwrap(), 16).unwrap()];
                if stream.read_exact(&mut body).is_err() {
                    break;
                }
                let service = String::from_utf8(body).unwrap();
                log.lock().unwrap().push(service.clone());
                let reply = match service.as_str() {
                    "host:devices-l" => [b"OKAY".to_vec(), block(&format!("{LIST_LINE}\n"))].concat(),
                    "host:list-forward" => {
                        [b"OKAY".to_vec(), block("FROM-SERVER tcp:27001 localabstract:scrcpy_00000001\n")].concat()
                    }
                    "host:track-devices-l" => {
                        let first = [b"OKAY".to_vec(), block(&format!("{LIST_LINE}\n"))].concat();
                        let _ = stream.write_all(&first);
                        kept.push(stream);
                        break;
                    }
                    s if s.starts_with("host:transport:") => b"OKAY".to_vec(),
                    s if s.starts_with("shell,v2:") => {
                        // Standard output "from the server", exit code 0.
                        let out = b"from the server\n";
                        let mut r = b"OKAY".to_vec();
                        r.push(1);
                        r.extend((out.len() as u32).to_le_bytes());
                        r.extend(out);
                        r.extend([3, 1, 0, 0, 0, 0]);
                        let _ = stream.write_all(&r);
                        break;
                    }
                    _ => [b"FAIL".to_vec(), block("unknown host service")].concat(),
                };
                if stream.write_all(&reply).is_err() {
                    break;
                }
            }
        }
    });
    (address, asked)
}

/// The scripted adb program, in a folder of its own.
fn fake_program(name: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("scrcpy-agent-select-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("adb{}", std::env::consts::EXE_SUFFIX));
    common::place_program(&example("fake_adb_cli"), &path);
    let script: Vec<Value> = vec![
        json!({ "args": ["devices", "-l"], "exit": 0,
                "stdout": format!("List of devices attached\r\n{LIST_LINE}\r\n\r\n"), "stderr": "" }),
        json!({ "args": ["-s", SERIAL, "shell", "getprop x"], "exit": 0, "stdout": "from the program\n", "stderr": "" }),
        json!({ "args": ["forward", "--list"], "exit": 0,
                "stdout": "FROM-PROGRAM tcp:27002 localabstract:scrcpy_00000002\n", "stderr": "" }),
    ];
    std::fs::write(dir.join("script.json"), Value::Array(script).to_string()).unwrap();
    (dir, path)
}

fn program_calls(dir: &Path) -> Vec<Vec<String>> {
    std::fs::read_to_string(dir.join("calls.log"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

struct Setup {
    adb: ConfiguredAdb,
    setting: Arc<Mutex<AdbBackend>>,
    asked_server: Arc<Mutex<Vec<String>>>,
    dir: PathBuf,
}

impl Setup {
    fn new(name: &str, backend: AdbBackend) -> Setup {
        let (server, asked_server) = fake_server();
        let (dir, program) = fake_program(name);
        let setting = Arc::new(Mutex::new(backend));
        Setup { adb: ConfiguredAdb::new(setting.clone(), server, program), setting, asked_server, dir }
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test]
async fn with_the_command_line_chosen_the_adb_program_answers_and_the_server_is_not_asked() {
    let s = Setup::new("cli", AdbBackend::CommandLine);
    let devices = s.adb.devices().await.unwrap();
    assert_eq!(devices.iter().map(|d| d.serial.as_str()).collect::<Vec<_>>(), [SERIAL]);
    assert_eq!(program_calls(&s.dir), [["devices", "-l"]]);
    assert!(s.asked_server.lock().unwrap().is_empty(), "{:?}", s.asked_server.lock().unwrap());
}

#[tokio::test]
async fn with_the_server_chosen_the_server_answers_and_the_program_is_not_run() {
    let s = Setup::new("server", AdbBackend::Server);
    let devices = s.adb.devices().await.unwrap();
    assert_eq!(devices.iter().map(|d| d.serial.as_str()).collect::<Vec<_>>(), [SERIAL]);
    assert_eq!(*s.asked_server.lock().unwrap(), ["host:devices-l"]);
    assert!(program_calls(&s.dir).is_empty());
}

#[tokio::test]
async fn changing_the_setting_changes_who_answers_at_once() {
    let s = Setup::new("switch", AdbBackend::Server);
    s.adb.devices().await.unwrap();
    *s.setting.lock().unwrap() = AdbBackend::CommandLine;
    s.adb.devices().await.unwrap();
    *s.setting.lock().unwrap() = AdbBackend::Server;
    s.adb.devices().await.unwrap();
    // Server, program, server: one call each time, to whoever was chosen then.
    assert_eq!(*s.asked_server.lock().unwrap(), ["host:devices-l", "host:devices-l"]);
    assert_eq!(program_calls(&s.dir), [["devices", "-l"]]);
}

#[tokio::test]
async fn a_shell_command_goes_the_chosen_way() {
    let s = Setup::new("shell", AdbBackend::Server);
    let from_server = s.adb.shell(SERIAL, "getprop x").await.unwrap();
    assert_eq!(from_server, ShellOutput { stdout: b"from the server\n".to_vec(), stderr: Vec::new(), exit: 0 });
    *s.setting.lock().unwrap() = AdbBackend::CommandLine;
    let from_program = s.adb.shell(SERIAL, "getprop x").await.unwrap();
    assert_eq!(from_program, ShellOutput { stdout: b"from the program\n".to_vec(), stderr: Vec::new(), exit: 0 });
}

#[tokio::test]
async fn the_forwards_come_from_the_chosen_way() {
    let s = Setup::new("forwards", AdbBackend::Server);
    let serials = |forwards: Vec<scrcpy_agent_lib::core::adb::Forward>| -> Vec<String> {
        forwards.into_iter().map(|f| f.serial).collect()
    };
    assert_eq!(serials(s.adb.forwards().await.unwrap()), ["FROM-SERVER"]);
    *s.setting.lock().unwrap() = AdbBackend::CommandLine;
    assert_eq!(serials(s.adb.forwards().await.unwrap()), ["FROM-PROGRAM"]);
}

#[tokio::test]
async fn announcements_come_only_from_the_server_way() {
    let s = Setup::new("track", AdbBackend::CommandLine);
    assert_eq!(s.adb.track_devices().await.err(), Some(AdbError::Unsupported));
    assert!(s.asked_server.lock().unwrap().is_empty());

    *s.setting.lock().unwrap() = AdbBackend::Server;
    let mut tracker = s.adb.track_devices().await.unwrap();
    let first = tracker.next().await.unwrap();
    assert_eq!(first.iter().map(|d| d.serial.as_str()).collect::<Vec<_>>(), [SERIAL]);
    assert_eq!(*s.asked_server.lock().unwrap(), ["host:track-devices-l"]);
}
