//! Whole-application tests.
//!
//! These run the real core, the real command dispatch of the Tauri app (the
//! same `configure` the application uses, driven through Tauri's IPC test
//! runtime), the real adb protocol client over a real TCP connection, and real
//! child processes. Only the outside world is a stand-in: a fake adb server
//! (below, speaking the adb protocol), `fake_adb` (which only starts that
//! server) and `fake_scrcpy` (see `examples/`). Their input is in the form real
//! adb and scrcpy output has; every value in it is invented.
//!
//! The point of these tests is to catch what unit tests of my own functions
//! cannot: threading and runtime mistakes in the commands, the shape of what
//! the UI receives, process handling, and the behaviour across a whole poll.

mod common;

use scrcpy_agent_lib::configure;
use scrcpy_agent_lib::core::adb::PROPS_COMMAND;
use scrcpy_agent_lib::core::model::{
    AdbBackend, DeviceState, Event, NoticeCode, ScrcpyMode, ScrcpyStatus, Settings, Snapshot,
};
use scrcpy_agent_lib::core::scrcpy::manager::Installed;
use scrcpy_agent_lib::core::{Core, CoreOptions, Sink, store};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder, mock_context, noop_assets};
use tauri::webview::InvokeRequest;

// ------------------------------------------------------------- fixtures

/// What `host:devices-l` answers with one authorised USB device.
const ONE_USB_DEVICE: &str =
    "TESTDEV0001            device usb:1-2 product:testprod model:TEST_M1 device:testdev transport_id:1\n";
const NO_DEVICES: &str = "";
/// The four properties the application asks for, one per line.
const PROPS: &str = "TESTDEV0001\nTEST-M1\n11\n30\n";
/// Invented mDNS services in the form the command line prints them (tab
/// separated). Assumption, not verified against the server: `host:mdns:services`
/// answers with these lines.
const REAL_MDNS: &str = "\
adb-TESTSERIAL001\t_adb._tcp.\t192.168.250.41:5555\n\
adb-ABCDE12345-Zq7Rt3\t_adb-tls-connect._tcp.\t192.168.250.146:41900\n\
adb-a1b2c3d4-Ab12Cd\t_adb-tls-connect._tcp.\t192.168.250.139:36600\n";
const NO_MDNS: &str = "";

struct Scenario {
    devices: &'static str,
    mdns: &'static str,
    scrcpy_fails: bool,
    /// Ids of devices that were paired before.
    known: &'static [&'static str],
    /// Shell commands that end with the given exit code instead of 0.
    failing_commands: &'static [(&'static str, u8)],
    /// How long the device loop waits between looks; `None` is the normal two seconds.
    poll_interval: Option<Duration>,
    /// The version of the scrcpy that is "installed". No protocol adapter
    /// exists for the default, which is on purpose: it is for tests that do not
    /// care.
    scrcpy_version: &'static str,
    scrcpy_mode: ScrcpyMode,
    /// What the fake device sends on its first connection (the video socket);
    /// with it, there is a fake device and a forward to it.
    device_stream: Option<Vec<u8>>,
    /// scrcpy opens many connections back to back, and the device notes the
    /// order it accepted them in.
    connection_burst: bool,
    /// Extra arguments for scrcpy, from the settings.
    extra_args: Vec<String>,
    /// A control message the fake scrcpy sends in two parts, with a pause between.
    split_message: Option<Vec<u8>>,
}

impl Scenario {
    fn usb_device() -> Self {
        Scenario {
            devices: ONE_USB_DEVICE,
            mdns: NO_MDNS,
            scrcpy_fails: false,
            known: &[],
            failing_commands: &[],
            poll_interval: None,
            scrcpy_version: "v0.0.0",
            scrcpy_mode: ScrcpyMode::Official,
            device_stream: None,
            connection_burst: false,
            extra_args: Vec::new(),
            split_message: None,
        }
    }

    /// scrcpy 5.0.1 with the proxy on, in front of a fake device that sends `stream`.
    fn proxy(stream: Vec<u8>) -> Self {
        Scenario {
            scrcpy_version: "v5.0.1",
            scrcpy_mode: ScrcpyMode::Proxy,
            device_stream: Some(stream),
            ..Scenario::usb_device()
        }
    }

    /// No device yet, and a device loop that looks only once a minute: what
    /// appears afterwards got there because the adb server said so.
    fn nothing_yet_and_a_slow_poll() -> Self {
        Scenario {
            devices: NO_DEVICES,
            poll_interval: Some(Duration::from_secs(60)),
            ..Scenario::usb_device()
        }
    }
}

// ------------------------------------------------------- the fake adb server

/// What the fake adb server was asked.
#[derive(Default)]
struct Requests {
    /// Every service name, in the order received.
    services: Vec<String>,
    /// Every shell command: the device it was for and the command.
    shells: Vec<(String, String)>,
}

/// An adb server for the tests: it listens on a free local port and speaks
/// the protocol (length-prefixed requests, OKAY/FAIL, `shell,v2:` packets) with
/// the answers the scenario gives. It is written independently of the
/// application's own client, which is checked against bytes from a real server.
struct FakeAdbServer {
    address: SocketAddr,
    requests: Arc<Mutex<Requests>>,
    state: Arc<Mutex<ServerState>>,
}

/// What can change while the server runs.
struct ServerState {
    /// The device list the server reports.
    devices: String,
    /// The connections that asked to be told about changes.
    trackers: Vec<TcpStream>,
    /// Close every new connection at once, as a server that is down would.
    refuse: bool,
    /// The forward scrcpy "creates": the device's serial and the local port.
    /// It is listed from the second time the list is asked for, as a forward
    /// that appears after the application has looked once.
    forward: Option<(String, u16)>,
    forward_asked: usize,
    /// A forward of an earlier scrcpy that is still listed, from the first
    /// look on, pointing where nothing listens.
    stale_forward: Option<(String, u16)>,
    /// The files that were pushed to the device, by their path there.
    files: std::collections::BTreeMap<String, Vec<u8>>,
}

impl FakeAdbServer {
    fn start(scenario: &Scenario) -> FakeAdbServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the fake adb server");
        let address = listener.local_addr().unwrap();
        let requests: Arc<Mutex<Requests>> = Arc::default();
        let state = Arc::new(Mutex::new(ServerState {
            devices: scenario.devices.to_string(),
            trackers: Vec::new(),
            refuse: false,
            forward: None,
            forward_asked: 0,
            stale_forward: None,
            files: Default::default(),
        }));
        let answers = Arc::new((scenario.mdns.to_string(), scenario.failing_commands.to_vec()));
        let (log, shared) = (requests.clone(), state.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (answers, log, shared) = (answers.clone(), log.clone(), shared.clone());
                std::thread::spawn(move || serve(stream, &answers, &log, &shared));
            }
        });
        FakeAdbServer { address, requests, state }
    }

    /// Changes the device list and tells every connection that is tracking it.
    fn set_devices(&self, devices: &str) {
        let mut state = self.state.lock().unwrap();
        state.devices = devices.to_string();
        let push = block(devices);
        // A connection that cannot be written to is gone; forget it.
        state.trackers.retain_mut(|t| t.write_all(&push).is_ok());
    }

    /// The files pushed to the device so far, by their path there.
    fn pushed(&self) -> std::collections::BTreeMap<String, Vec<u8>> {
        self.state.lock().unwrap().files.clone()
    }

    /// Lists a forward of an earlier scrcpy, which is there from the first look.
    fn set_stale_forward(&self, serial: &str, local_port: u16) {
        self.state.lock().unwrap().stale_forward = Some((serial.to_string(), local_port));
    }

    /// Makes the forward to the fake device exist (from the second look at the list).
    fn set_forward(&self, serial: &str, local_port: u16) {
        self.state.lock().unwrap().forward = Some((serial.to_string(), local_port));
    }

    /// From now on every connection is closed at once (or accepted again).
    fn refuse_connections(&self, refuse: bool) {
        self.state.lock().unwrap().refuse = refuse;
    }

    /// Ends every tracking connection, as a restarted server would.
    fn drop_trackers(&self) {
        for tracker in self.state.lock().unwrap().trackers.drain(..) {
            let _ = tracker.shutdown(std::net::Shutdown::Both);
        }
    }
}

type Answers = (String, Vec<(&'static str, u8)>);

fn block(text: &str) -> Vec<u8> {
    let mut bytes = format!("{:04x}", text.len()).into_bytes();
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

fn packet(id: u8, data: &[u8]) -> Vec<u8> {
    let mut bytes = vec![id];
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(data);
    bytes
}

fn serve(mut stream: TcpStream, answers: &Answers, log: &Mutex<Requests>, shared: &Mutex<ServerState>) {
    if shared.lock().unwrap().refuse {
        return;
    }
    let mut device = None;
    loop {
        let mut head = [0u8; 4];
        if stream.read_exact(&mut head).is_err() {
            return;
        }
        let len = usize::from_str_radix(std::str::from_utf8(&head).unwrap(), 16).unwrap();
        let mut body = vec![0u8; len];
        if stream.read_exact(&mut body).is_err() {
            return;
        }
        let service = String::from_utf8(body).unwrap();
        log.lock().unwrap().services.push(service.clone());

        let reply: Vec<u8> = if service == "host:devices-l" {
            [b"OKAY".to_vec(), block(&shared.lock().unwrap().devices)].concat()
        } else if service == "host:track-devices-l" {
            // The current list now, further lists as the test changes it. The
            // connection stays open: the server keeps a copy of the socket.
            let mut state = shared.lock().unwrap();
            let first = [b"OKAY".to_vec(), block(&state.devices)].concat();
            if stream.write_all(&first).is_err() {
                return;
            }
            if let Ok(copy) = stream.try_clone() {
                state.trackers.push(copy);
            }
            return;
        } else if service == "host:list-forward" {
            let mut state = shared.lock().unwrap();
            state.forward_asked += 1;
            let mut text = String::new();
            if let Some((serial, port)) = &state.stale_forward {
                text.push_str(&format!("{serial} tcp:{port} localabstract:scrcpy_00c0ffee\n"));
            }
            if let Some((serial, port)) = &state.forward
                && state.forward_asked > 1
            {
                text.push_str(&format!("{serial} tcp:{port} localabstract:scrcpy_1a2b3c4d\n"));
            }
            [b"OKAY".to_vec(), block(&text)].concat()
        } else if service == "host:mdns:services" {
            [b"OKAY".to_vec(), block(&answers.0)].concat()
        } else if let Some(serial) = service.strip_prefix("host:transport:") {
            device = Some(serial.to_string());
            b"OKAY".to_vec()
        } else if let Some(command) = service.strip_prefix("shell,v2:") {
            let Some(serial) = device.clone() else {
                let _ = stream.write_all(&[b"FAIL".to_vec(), block("no transport")].concat());
                return;
            };
            log.lock().unwrap().shells.push((serial, command.to_string()));
            // `ls -1 '<folder>'` lists the pushed files of that folder.
            let listing = command
                .strip_prefix("ls -1 '")
                .and_then(|rest| rest.strip_suffix('\''))
                .map(|dir| {
                    let prefix = format!("{dir}/");
                    let state = shared.lock().unwrap();
                    state
                        .files
                        .keys()
                        .filter_map(|path| path.strip_prefix(&prefix).filter(|name| !name.contains('/')))
                        .map(|name| format!("{name}\n"))
                        .collect::<String>()
                });
            // The apps of the invented device: two can open plain text, three can show some file
            // (the answer for any type), none a PDF.
            let chooser = "priority=0 preferredOrder=0 match=0x0 specificIndex=-1 isDefault=false\nandroid/com.android.internal.app.ResolverActivity\n";
            let apps = if command.ends_with(" 'text/plain'") && command.starts_with("cmd package query-activities") {
                Some("2 activities found:\n  com.example.reader/com.example.reader.ViewActivity\n  com.example.editor/.EditActivity\n")
            } else if command.ends_with(" 'text/plain'") && command.starts_with("cmd package resolve-activity") {
                Some(chooser)
            } else if command.ends_with(" '*/*'") && command.starts_with("cmd package query-activities") {
                Some("3 activities found:\n  com.example.reader/com.example.reader.ViewActivity\n  com.example.editor/.EditActivity\n  com.example.files/.OpenActivity\n")
            } else if command.ends_with(" '*/*'") && command.starts_with("cmd package resolve-activity") {
                Some(chooser)
            } else if command.starts_with("cmd package ") {
                Some("No activities found\n")
            } else {
                None
            };
            let stdout = if command == PROPS_COMMAND { PROPS } else { apps.or(listing.as_deref()).unwrap_or("") };
            let exit = answers.1.iter().find(|(c, _)| *c == command).map_or(0, |(_, code)| *code);
            let mut reply = b"OKAY".to_vec();
            reply.extend(packet(1, stdout.as_bytes()));
            reply.extend(packet(3, &[exit]));
            let _ = stream.write_all(&reply);
            return;
        } else if service == "sync:" {
            if device.is_none() || stream.write_all(b"OKAY").is_err() {
                return;
            }
            let (mut path, mut data) = (String::new(), Vec::new());
            loop {
                let mut head = [0u8; 8];
                if stream.read_exact(&mut head).is_err() {
                    return;
                }
                let number = u32::from_le_bytes(head[4..].try_into().unwrap());
                match &head[..4] {
                    b"SEND" => {
                        let mut spec = vec![0u8; number as usize];
                        if stream.read_exact(&mut spec).is_err() {
                            return;
                        }
                        let spec = String::from_utf8(spec).unwrap();
                        // The path is what is before the last comma, the mode after it.
                        path = spec.rsplit_once(',').map(|(p, _)| p.to_string()).unwrap();
                        data.clear();
                    }
                    b"DATA" => {
                        let mut piece = vec![0u8; number as usize];
                        if stream.read_exact(&mut piece).is_err() {
                            return;
                        }
                        data.extend(piece);
                    }
                    b"DONE" => {
                        shared.lock().unwrap().files.insert(path.clone(), std::mem::take(&mut data));
                        let _ = stream.write_all(&[b"OKAY".to_vec(), 0u32.to_le_bytes().to_vec()].concat());
                    }
                    _ => return,
                }
            }
        } else if ["host:connect:", "host:disconnect:", "host:pair:"].iter().any(|p| service.starts_with(p)) {
            [b"OKAY".to_vec(), block("")].concat()
        } else {
            [b"FAIL".to_vec(), block("unknown host service")].concat()
        };
        if stream.write_all(&reply).is_err() {
            return;
        }
    }
}

// ------------------------------------------------------------ the fake device

/// The device's end of the forward: its first connection is the video socket,
/// the second the control socket. It keeps what the client sent on the second.
struct FakeDevice {
    port: u16,
    control_received: Arc<Mutex<Vec<u8>>>,
    /// In burst mode: the first byte of each connection after the video one, in
    /// the order the device accepted them.
    accepted_order: Arc<Mutex<Vec<u8>>>,
}

impl FakeDevice {
    /// With `burst`, the connections after the video one are only noted down
    /// (by their first byte) in the order they arrive, instead of being the control socket.
    fn start(video: Vec<u8>, burst: bool) -> FakeDevice {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the fake device");
        let port = listener.local_addr().unwrap().port();
        let control_received: Arc<Mutex<Vec<u8>>> = Arc::default();
        let accepted_order: Arc<Mutex<Vec<u8>>> = Arc::default();
        let (kept, order) = (control_received.clone(), accepted_order.clone());
        std::thread::spawn(move || {
            for (index, mut stream) in listener.incoming().flatten().enumerate() {
                match index {
                    0 => {
                        // In pieces that cut through the device name and the
                        // packets, as the network does.
                        let cuts = [50, 120];
                        let mut sent = 0;
                        for cut in cuts.into_iter().chain(std::iter::repeat(1000)) {
                            let end = (sent + cut).min(video.len());
                            if stream.write_all(&video[sent..end]).is_err() {
                                break;
                            }
                            sent = end;
                            if sent == video.len() {
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        let _ = stream.shutdown(std::net::Shutdown::Write);
                    }
                    _ if burst => {
                        // Noted in the order accepted: the first byte is read here, before the
                        // next connection is accepted. (Read by a thread of each connection, the
                        // order noted would be that of whichever thread ran first, which under load
                        // is not the order the connections were made in.) The connection is kept open.
                        let mut first = [0u8; 1];
                        if stream.read_exact(&mut first).is_ok() {
                            order.lock().unwrap().push(first[0]);
                        }
                        std::thread::spawn(move || {
                            let mut rest = Vec::new();
                            let _ = stream.read_to_end(&mut rest);
                        });
                    }
                    1 => {
                        // The audio socket: starts with a codec id.
                        std::thread::spawn(move || {
                            let _ = stream.write_all(b"opus");
                            let mut rest = Vec::new();
                            let _ = stream.read_to_end(&mut rest);
                        });
                    }
                    2 => {
                        // The control socket: everything the client sends is kept,
                        // and the device answers once it has heard something.
                        let kept = kept.clone();
                        std::thread::spawn(move || {
                            let mut buffer = [0u8; 4096];
                            let mut answered = false;
                            while let Ok(n) = stream.read(&mut buffer) {
                                if n == 0 {
                                    break;
                                }
                                kept.lock().unwrap().extend_from_slice(&buffer[..n]);
                                if !answered {
                                    answered = true;
                                    let _ = stream.write_all(b"device-reply");
                                }
                            }
                        });
                    }
                    _ => {}
                }
            }
        });
        FakeDevice { port, control_received, accepted_order }
    }
}

// What the video socket of scrcpy 5.0.1 carries (see `scrcpy_v5`): a dummy byte,
// the device name in 64 bytes, the codec, a session packet, then packets of a
// 12 byte header and a payload. The payloads here are invented.

use scrcpy_agent_lib::core::scrcpy::protocol::{PacketKind, VideoCodec, VideoEvent, VideoPacket};

fn payload(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 251) as u8).collect()
}

fn media_header(kind: PacketKind, pts_us: u64, size: usize) -> Vec<u8> {
    let flags: u64 = match kind {
        PacketKind::Config => 0x40 << 56,
        PacketKind::Key => 0x20 << 56,
        PacketKind::Delta => 0,
    };
    [(flags | pts_us).to_be_bytes().to_vec(), (size as u32).to_be_bytes().to_vec()].concat()
}

/// The whole video socket for these packets, with the codec as given.
fn video_socket(codec: &[u8; 4], packets: &[(PacketKind, u64, usize)]) -> Vec<u8> {
    let mut bytes = vec![0u8];
    let mut name = b"Test Device".to_vec();
    name.resize(64, 0);
    bytes.extend(name);
    bytes.extend(codec);
    // The session packet: first bit set, then width and height.
    bytes.extend([0x80, 0, 0, 0]);
    bytes.extend(1280u32.to_be_bytes());
    bytes.extend(720u32.to_be_bytes());
    for (kind, pts, size) in packets {
        bytes.extend(media_header(*kind, *pts, *size));
        bytes.extend(payload(*size));
    }
    bytes
}

fn packet_event(kind: PacketKind, pts_us: u64, size: usize) -> VideoEvent {
    VideoEvent::Packet(VideoPacket { kind, pts_us, payload: payload(size) })
}

fn stream_start() -> Vec<VideoEvent> {
    vec![
        VideoEvent::DeviceName("Test Device".into()),
        VideoEvent::Codec(VideoCodec::H264),
        VideoEvent::Session { width: 1280, height: 720 },
    ]
}

/// One group of pictures: the parameter packet, a key frame and three frames after it.
const ONE_GROUP: [(PacketKind, u64, usize); 5] = [
    (PacketKind::Config, 0, 24),
    (PacketKind::Key, 100_000, 2000),
    (PacketKind::Delta, 200_000, 300),
    (PacketKind::Delta, 300_000, 250),
    (PacketKind::Delta, 400_000, 200),
];

// -------------------------------------------------------------- harness

struct Harness {
    dir: PathBuf,
    install: Installed,
    device: Option<FakeDevice>,
    adb_server: FakeAdbServer,
    events: Arc<Mutex<Vec<Event>>>,
    core: Arc<Core>,
    runtime: Option<tokio::runtime::Runtime>,
    _app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
}

/// Where Cargo put the example programs next to this test executable.
fn example(name: &str) -> PathBuf {
    let exe = std::env::current_exe().expect("test executable path");
    let target_dir = exe.parent().and_then(Path::parent).expect("target directory");
    let path = target_dir.join("examples").join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    assert!(
        path.exists(),
        "{} has not been built. Run the full `cargo test` (or add `--examples`): \
         running a single test target does not build the example programs.",
        path.display()
    );
    path
}

impl Harness {
    fn start(name: &str, scenario: Scenario) -> Harness {
        let dir = std::env::temp_dir().join(format!("scrcpy-agent-it-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("scrcpy");
        std::fs::create_dir_all(&root).unwrap();
        let suffix = std::env::consts::EXE_SUFFIX;
        common::place_program(&example("fake_adb"), &root.join(format!("adb{suffix}")));
        common::place_program(&example("fake_scrcpy"), &root.join(format!("scrcpy{suffix}")));
        std::fs::write(root.join("scrcpy-server"), b"").unwrap();
        if scenario.scrcpy_fails {
            std::fs::write(root.join("scrcpy-mode.txt"), "fail").unwrap();
        }
        let install = Installed { version: scenario.scrcpy_version.into(), root };

        let data = dir.join("data");
        let settings = Settings {
            scrcpy_mode: scenario.scrcpy_mode,
            scrcpy_extra_args: scenario.extra_args.clone(),
            ..Settings::default()
        };
        let data_store = store::Store::open(data.clone());
        data_store.global().save::<store::Settings>(&settings).unwrap();
        let known: Vec<_> = scenario
            .known
            .iter()
            .map(|id| scrcpy_agent_lib::core::model::KnownDevice {
                id: (*id).to_string(),
                model: "Known phone".into(),
                label: None,
                android: Some("14".into()),
                sdk: Some(34),
                last_address: None,
                last_seen: 1000,
                first_seen: 500,
                settings: Default::default(),
                apps: Default::default(),
            })
            .collect();
        for device in &known {
            data_store.save_known(device).unwrap();
        }

        let events: Arc<Mutex<Vec<Event>>> = Arc::default();
        let sink: Sink = {
            let events = events.clone();
            Arc::new(move |e| events.lock().unwrap().push(e))
        };
        let adb_server = FakeAdbServer::start(&scenario);
        if scenario.connection_burst {
            std::fs::write(install.root.join("scrcpy-burst.txt"), b"burst").unwrap();
        }
        if let Some(message) = &scenario.split_message {
            std::fs::write(install.root.join("scrcpy-split.bin"), message).unwrap();
        }
        let device = scenario.device_stream.clone().map(|video| FakeDevice::start(video, scenario.connection_burst));
        if let Some(device) = &device {
            adb_server.set_forward("TESTDEV0001", device.port);
        }
        let core = Core::with_options(
            data,
            sink,
            CoreOptions {
                preinstalled: Some(install.clone()),
                disable_own_mdns: true,
                adb_server: Some(adb_server.address),
                poll_interval: scenario.poll_interval,
            },
        );

        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        runtime.spawn(core.clone().run());

        let app = configure(mock_builder().manage(core.clone()))
            .build(mock_context(noop_assets()))
            .unwrap();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        Harness { dir, install, device, adb_server, events, core, runtime: Some(runtime), _app: app, webview }
    }

    /// Calls a command the way the UI does. This runs on the test's own
    /// thread, which has no Tokio runtime, just like Tauri's main thread.
    fn ipc(&self, cmd: &str, body: Value) -> Result<Value, Value> {
        let request = InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            // The address of the pages of the application: Tauri takes a request from another
            // address for one from a remote page, and refuses it.
            url: if cfg!(any(windows, target_os = "android")) { "http://tauri.localhost" } else { "tauri://localhost" }
                .parse()
                .unwrap(),
            body: InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        };
        get_ipc_response(&self.webview, request).map(|b| b.deserialize::<Value>().unwrap())
    }

    /// Waits until `check` returns a value for the current state.
    fn wait_for<T>(&self, what: &str, check: impl Fn(&Snapshot) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            let snapshot = self.core.snapshot();
            if let Some(found) = check(&snapshot) {
                return found;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}; state: {snapshot:#?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn wait_until_ready(&self) {
        self.wait_for("scrcpy to be ready", |s| {
            matches!(s.scrcpy, ScrcpyStatus::Ready { .. }).then_some(())
        });
    }

    fn wait_for_online_device(&self) -> String {
        self.wait_for("a device to come online", |s| {
            s.devices.iter().find(|d| d.state == DeviceState::Online).map(|d| d.id.clone())
        })
    }

    /// The lines a fake program wrote into the install folder.
    fn lines_of(&self, file: &str) -> Vec<String> {
        std::fs::read_to_string(self.install.root.join(file))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// The arguments the adb program was run with, one call per entry.
    fn adb_program_calls(&self) -> Vec<Vec<String>> {
        self.lines_of("calls.log")
            .iter()
            .map(|line| serde_json::from_str(line).expect("a call logged by fake_adb"))
            .collect()
    }

    /// The services the adb server was asked for, in order.
    fn adb_services(&self) -> Vec<String> {
        self.adb_server.requests.lock().unwrap().services.clone()
    }

    /// The shell commands run on devices, as (device, command), other than the
    /// start-up property query and the readings the core makes of every usable
    /// device on its own (battery, storage and the rest).
    fn device_commands(&self) -> Vec<(String, String)> {
        use scrcpy_agent_lib::core::device::{display, info as di};
        let readings = [
            di::PROPS_COMMAND,
            di::BATTERY_COMMAND,
            di::STORAGE_COMMAND,
            di::MEMORY_COMMAND,
            di::UPTIME_COMMAND,
            di::SIZE_COMMAND,
            di::DENSITY_COMMAND,
            display::DISPLAY_DUMP_COMMAND,
        ];
        let shells = self.adb_server.requests.lock().unwrap().shells.clone();
        shells
            .into_iter()
            .filter(|(_, command)| command != PROPS_COMMAND && !readings.contains(&command.as_str()))
            .collect()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_background();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ---------------------------------------------------------------- tests

#[test]
fn a_connected_device_is_listed_with_its_details_and_remembered() {
    let h = Harness::start("listed", Scenario::usb_device());
    let id = h.wait_for_online_device();
    assert_eq!(id, "TESTDEV0001");

    let device = h.core.snapshot().devices.into_iter().find(|d| d.id == id).unwrap();
    assert_eq!(device.name, "TEST-M1");
    assert_eq!(device.android.as_deref(), Some("11"));
    assert_eq!(device.sdk, Some(30));

    // It is stored, so it is still listed after a restart.
    let known = h.core.store().known_devices();
    assert_eq!(known.len(), 1);
    assert_eq!(known[0].id, "TESTDEV0001");
    assert_eq!(known[0].model, "TEST-M1");

    // The adb program is run only to start the server; everything else goes
    // to the server itself (the fake program fails on any other call).
    let program_calls = h.adb_program_calls();
    assert!(!program_calls.is_empty());
    assert!(program_calls.iter().all(|c| c.as_slice() == ["start-server"]), "{program_calls:?}");
    // The device list, the mDNS services and the device's properties were asked for.
    let services = h.adb_services();
    assert!(services.iter().any(|s| s == "host:devices-l"), "{services:?}");
    assert!(services.iter().any(|s| s == "host:mdns:services"), "{services:?}");
    assert!(services.iter().any(|s| s == "host:transport:TESTDEV0001"), "{services:?}");
}

/// Regression test for the crash that closed the whole application when a
/// screen was opened: the command used to run on the main thread, outside the
/// Tokio runtime, and the process start panicked.
#[test]
fn opening_a_screen_from_the_ui_runs_scrcpy_and_the_app_stays_alive() {
    let h = Harness::start("open", Scenario::usb_device());
    let id = h.wait_for_online_device();

    assert_eq!(h.ipc("open_device", json!({ "id": id })), Ok(Value::Null));

    h.wait_for("the screen session to end", |s| {
        let started = h.events.lock().unwrap().iter().any(
            |e| matches!(e, Event::State(snap) if snap.sessions.contains(&id)),
        );
        (started && s.sessions.is_empty()).then_some(())
    });

    let args = h.lines_of("scrcpy-args.txt");
    assert!(args.iter().any(|a| a == "--serial=TESTDEV0001"), "arguments: {args:?}");
    // The title carries the serial, so the toolbar can find exactly this window.
    assert!(args.iter().any(|a| a == "--window-title=TEST-M1 (TESTDEV0001)"), "arguments: {args:?}");
    // Android 11 (SDK 30) can forward audio, so audio must not be switched off.
    assert!(!args.iter().any(|a| a == "--no-audio"), "arguments: {args:?}");
    // scrcpy has to use the adb that belongs to the same release.
    assert_eq!(h.lines_of("scrcpy-adb.txt"), [h.install.adb().display().to_string()]);

    // The application still answers commands afterwards.
    assert!(h.ipc("get_snapshot", Value::Null).is_ok());
}

#[test]
fn a_failing_scrcpy_is_reported_together_with_what_it_printed() {
    let h = Harness::start("fails", Scenario { scrcpy_fails: true, ..Scenario::usb_device() });
    let id = h.wait_for_online_device();
    assert_eq!(h.ipc("open_device", json!({ "id": id })), Ok(Value::Null));

    h.wait_for("the session to end", |s| {
        let started = h.events.lock().unwrap().iter().any(
            |e| matches!(e, Event::State(snap) if snap.sessions.contains(&id)),
        );
        (started && s.sessions.is_empty()).then_some(())
    });
    let events = h.events.lock().unwrap();
    let detail = events
        .iter()
        .find_map(|e| match e {
            Event::Notice(n) if n.code == NoticeCode::SessionFailed => Some(n.detail.clone()),
            _ => None,
        })
        .expect("a session_failed notice");
    assert!(
        detail.lines().any(|l| l == "ERROR: Could not connect to the device"),
        "detail: {detail:?}"
    );
}

/// What the UI receives is a contract with `src-ui/shared/types.ts`; these pin its shape.
#[test]
fn errors_and_state_reach_the_ui_in_the_shape_it_expects() {
    let h = Harness::start("contract", Scenario::usb_device());
    h.wait_until_ready();
    h.wait_for_online_device();

    assert_eq!(
        h.ipc("open_device", json!({ "id": "NOBODY" })),
        Err(json!({ "code": "unknown_device" }))
    );
    assert_eq!(
        h.ipc("connect_device", json!({ "address": "not an address" })),
        Err(json!({ "code": "invalid_address" }))
    );
    assert_eq!(
        h.ipc("pair_device", json!({ "address": "192.168.1.2:37000", "code": "12" })),
        Err(json!({ "code": "invalid_pairing_code" }))
    );

    let state = h.ipc("get_snapshot", Value::Null).unwrap();
    assert_eq!(state["scrcpy"]["phase"], "ready");
    assert_eq!(state["devices"][0]["id"], "TESTDEV0001");
    assert_eq!(state["devices"][0]["state"], "online");
    assert_eq!(state["devices"][0]["transport"], "usb");
    assert!(state["sessions"].as_array().unwrap().is_empty());
    assert_eq!(state["settings"]["adb_backend"], "server");
    assert_eq!(state["settings"]["scrcpy_mode"], "official");
}

#[test]
fn the_scrcpy_mode_is_saved_and_kept() {
    let h = Harness::start("scrcpy-mode", Scenario::usb_device());
    h.wait_until_ready();
    let mut settings = h.ipc("get_snapshot", Value::Null).unwrap()["settings"].clone();
    settings["scrcpy_mode"] = json!("proxy");
    assert_eq!(h.ipc("save_settings", json!({ "settings": settings })), Ok(Value::Null));
    assert_eq!(h.core.store().global().load::<store::Settings>().scrcpy_mode, ScrcpyMode::Proxy);
    assert_eq!(h.ipc("get_snapshot", Value::Null).unwrap()["settings"]["scrcpy_mode"], "proxy");
}

#[test]
fn the_themes_are_listed_and_the_chosen_one_is_applied_with_the_programs_own_look_as_the_fallback() {
    use scrcpy_agent_lib::core::theme::ThemeBase;
    let h = Harness::start("themes", Scenario::usb_device());
    h.wait_until_ready();
    let listed = h.ipc("list_themes", Value::Null).unwrap();
    let ids: Vec<&str> = listed["themes"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["default", "high-contrast", "compact", "terminal", "paper", "brutalist", "classic"]);
    assert_eq!(listed["problems"], json!([]));
    let own = json!({ "id": "default", "base": "auto", "css": "" });
    assert_eq!(h.ipc("current_theme", Value::Null), Ok(own.clone()));

    h.core.themes().register_theme("night-pack", "night", "Night", ThemeBase::Dark, "body { color: red; }").unwrap();
    let mut settings = h.ipc("get_snapshot", Value::Null).unwrap()["settings"].clone();
    settings["theme"] = json!("night-pack/night");
    assert_eq!(h.ipc("save_settings", json!({ "settings": settings })), Ok(Value::Null));
    assert_eq!(h.core.store().global().load::<store::Settings>().theme.as_str(), "night-pack/night");
    assert_eq!(
        h.ipc("current_theme", Value::Null),
        Ok(json!({ "id": "night-pack/night", "base": "dark", "css": "body { color: red; }" }))
    );

    // The add-on goes: the look is the program's own, and the choice stays (it is back with the add-on).
    h.core.remove_addon("night-pack");
    assert_eq!(h.ipc("current_theme", Value::Null), Ok(own));
    assert_eq!(h.ipc("get_snapshot", Value::Null).unwrap()["settings"]["theme"], "night-pack/night");
}

/// Waits (at most ten seconds) until `check` holds.
fn wait_until(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn the_adb_setting_is_saved_and_changes_who_answers_without_a_restart() {
    let h = Harness::start("backend", Scenario::usb_device());
    h.wait_for_online_device();
    let mut settings = h.ipc("get_snapshot", Value::Null).unwrap()["settings"].clone();

    // To the command line: the adb program is asked for the device list. (The
    // stand-in program only starts the server, so it fails; what counts here
    // is who is asked.)
    settings["adb_backend"] = json!("command_line");
    assert_eq!(h.ipc("save_settings", json!({ "settings": settings })), Ok(Value::Null));
    assert_eq!(h.core.store().global().load::<store::Settings>().adb_backend, AdbBackend::CommandLine);
    wait_until("the adb program to be asked for the device list", || {
        h.adb_program_calls().iter().any(|c| c.as_slice() == ["devices", "-l"])
    });

    // Back to the server: it is asked again.
    settings["adb_backend"] = json!("server");
    assert_eq!(h.ipc("save_settings", json!({ "settings": settings })), Ok(Value::Null));
    let asked_before = h.adb_services().len();
    wait_until("the adb server to be asked again", || h.adb_services().len() > asked_before);
}

#[test]
fn the_toolbar_lists_every_button_and_marks_the_ones_that_have_an_action() {
    let h = Harness::start("toolbar-list", Scenario::usb_device());
    h.wait_until_ready();
    let expected = json!([
        { "id": "back", "label": null, "enabled": true, "unavailable": null },
        { "id": "home", "label": null, "enabled": true, "unavailable": null },
        { "id": "app_switch", "label": null, "enabled": true, "unavailable": null },
        { "id": "power", "label": null, "enabled": true, "unavailable": null },
        { "id": "volume_up", "label": null, "enabled": true, "unavailable": null },
        { "id": "volume_down", "label": null, "enabled": true, "unavailable": null },
        { "id": "rotate", "label": null, "enabled": false, "unavailable": "needs_proxy" },
        { "id": "notifications", "label": null, "enabled": true, "unavailable": null },
        { "id": "send_file", "label": null, "enabled": false, "unavailable": "not_yet" },
        { "id": "screenshot", "label": null, "enabled": false, "unavailable": "not_yet" },
    ]);
    assert_eq!(h.ipc("toolbar_buttons", Value::Null), Ok(expected));
}

fn command(device: &str, command: &str) -> (String, String) {
    (device.to_string(), command.to_string())
}

#[test]
fn a_toolbar_button_presses_its_key_on_the_device() {
    let h = Harness::start("button-key", Scenario::usb_device());
    let id = h.wait_for_online_device();
    assert_eq!(h.ipc("press_button", json!({ "device": id, "button": "back" })), Ok(Value::Null));
    assert_eq!(h.device_commands(), [command("TESTDEV0001", "input keyevent 4")]);
}

#[test]
fn the_notifications_button_opens_the_notification_panel() {
    let h = Harness::start("button-notifications", Scenario::usb_device());
    let id = h.wait_for_online_device();
    assert_eq!(h.ipc("press_button", json!({ "device": id, "button": "notifications" })), Ok(Value::Null));
    assert_eq!(h.device_commands(), [command("TESTDEV0001", "cmd statusbar expand-notifications")]);
}

/// Waits until the fake server has a tracking connection, so a change it
/// pushes is heard.
fn wait_for_tracker(h: &Harness) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while h.adb_server.state.lock().unwrap().trackers.is_empty() {
        assert!(Instant::now() < deadline, "the application never asked to track devices: {:?}", h.adb_services());
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_change_the_adb_server_announces_reaches_the_ui_without_waiting_for_the_next_look() {
    let h = Harness::start("push", Scenario::nothing_yet_and_a_slow_poll());
    h.wait_until_ready();
    wait_for_tracker(&h);
    let before = Instant::now();
    h.adb_server.set_devices(ONE_USB_DEVICE);
    let id = h.wait_for_online_device();
    assert_eq!(id, "TESTDEV0001");
    // The device loop looks once a minute here, so only the announcement can
    // explain a device appearing within seconds.
    assert!(before.elapsed() < Duration::from_secs(10), "took {:?}", before.elapsed());
}

#[test]
fn tracking_comes_back_after_the_adb_server_drops_the_connection() {
    let h = Harness::start("push-resume", Scenario::nothing_yet_and_a_slow_poll());
    h.wait_until_ready();
    wait_for_tracker(&h);
    h.adb_server.drop_trackers();
    // The application asks again and is told the current list.
    wait_for_tracker(&h);
    h.adb_server.set_devices(ONE_USB_DEVICE);
    assert_eq!(h.wait_for_online_device(), "TESTDEV0001");
}

// ------------------------------------------------------------------ the proxy

impl Harness {
    /// The file a fake program left in the install folder, once it is there.
    fn file_of_scrcpy(&self, name: &str) -> Vec<u8> {
        let path = self.install.root.join(name);
        wait_until(name, || path.exists());
        // It is written in one go, but give it a moment to be complete.
        std::thread::sleep(Duration::from_millis(100));
        std::fs::read(path).unwrap()
    }

    /// Opens the screen and returns the arguments scrcpy was started with.
    fn open_and_read_arguments(&self) -> (String, Vec<String>) {
        let id = self.wait_for_online_device();
        assert_eq!(self.ipc("open_device", json!({ "id": id })), Ok(Value::Null));
        let args = String::from_utf8(self.file_of_scrcpy("scrcpy-args.txt")).unwrap();
        (id, args.lines().map(str::to_string).collect())
    }

    /// Lets the fake scrcpy finish, and waits for the screen session to end.
    fn release_scrcpy(&self, id: &str) {
        std::fs::write(self.install.root.join("scrcpy-release.txt"), b"go").unwrap();
        wait_until("the screen session to end", || !self.core.sessions().has_session(id));
    }
}

fn tunnel_port_of(args: &[String]) -> Option<u16> {
    args.iter().find_map(|a| a.strip_prefix("--tunnel-port=")?.parse().ok())
}

#[test]
fn with_the_proxy_on_scrcpy_connects_to_it_and_everything_is_relayed_unchanged_both_ways() {
    let video = video_socket(b"h264", &ONE_GROUP);
    let h = Harness::start("proxy", Scenario::proxy(video.clone()));
    let (id, args) = h.open_and_read_arguments();

    // scrcpy was told to use the forward tunnel, at a port that is not the device's.
    assert!(args.iter().any(|a| a == "--force-adb-forward"), "{args:?}");
    let port = tunnel_port_of(&args).expect("a --tunnel-port argument");
    assert_ne!(port, h.device.as_ref().unwrap().port);

    // What the device sent reached scrcpy unchanged, and so did the other direction.
    assert_eq!(h.file_of_scrcpy("scrcpy-received.bin"), video);
    assert_eq!(h.file_of_scrcpy("scrcpy-control-reply.bin"), b"device-reply");
    let device = h.device.as_ref().unwrap();
    wait_until("the device to receive the control message", || !device.control_received.lock().unwrap().is_empty());
    // (the fake scrcpy sends one control message: collapse the panels, type 7)
    assert_eq!(*device.control_received.lock().unwrap(), [7]);

    // The video was also taken apart for whoever follows it.
    let mut expected = stream_start();
    expected.extend(ONE_GROUP.iter().map(|(kind, pts, size)| packet_event(*kind, *pts, *size)));
    wait_until("the whole video to be followed", || {
        h.core.sessions().subscribe_video(&id).is_ok_and(|s| s.initial.len() == expected.len())
    });
    assert_eq!(h.core.sessions().subscribe_video(&id).unwrap().initial, expected);

    // The proxy is written to the debug log, and ends with the screen.
    assert!(level_and_code(&open_console(&h)).contains(&pair("info", "proxy_started")));
    h.release_scrcpy(&id);
    assert!(matches!(h.core.sessions().subscribe_video(&id), Err(scrcpy_agent_lib::core::error::CoreError::NoProxy)));
    wait_until("the proxy to be logged as stopped", || {
        level_and_code(&open_console(&h)).contains(&pair("info", "proxy_stopped"))
    });
}

// What the fake scrcpy's control socket carried to the fake device.
fn control_bytes(h: &Harness) -> Vec<u8> {
    h.device.as_ref().unwrap().control_received.lock().unwrap().clone()
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
}

/// Key code 4 (back) pressed and released, as the control socket carries it.
const BACK_KEY_DOWN: &str = "0000000000040000000000000000";
const BACK_KEY_UP: &str = "0001000000040000000000000000";
/// A mouse move in the layout of scrcpy 5.0.1's client (x 640, y 360 on a screen
/// of 1280 x 720; the values are invented).
const MOUSE_MOVE: &str = "0207ffffffffffffffff0000028000000168050002d0ffff0000000000000000";

#[test]
fn a_control_message_is_written_between_the_clients_own_never_in_the_middle_of_one() {
    use scrcpy_agent_lib::core::scrcpy::protocol::ControlMessage;
    let message = hex(MOUSE_MOVE);
    let scenario = Scenario { split_message: Some(message.clone()), ..Scenario::proxy(video_socket(b"h264", &ONE_GROUP)) };
    let h = Harness::start("control-split", scenario);
    let (id, _) = h.open_and_read_arguments();
    wait_until("the first part of the message", || control_bytes(&h).len() == 10);

    // The client has sent 10 bytes of a 32 byte message: ours must wait.
    assert!(h.core.sessions().send_control(&id, &ControlMessage::ExpandSettingsPanel).is_ok());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(control_bytes(&h), message[..10], "our message went into the middle of the client's");

    // When the client's message is whole, ours follows it.
    std::fs::write(h.install.root.join("scrcpy-split-go.txt"), b"go").unwrap();
    wait_until("both messages", || control_bytes(&h).len() == 33);
    assert_eq!(control_bytes(&h), [message, vec![6]].concat());
    h.release_scrcpy(&id);
}

#[test]
fn a_control_message_is_written_at_once_when_the_client_is_between_messages() {
    use scrcpy_agent_lib::core::scrcpy::protocol::ControlMessage;
    let h = Harness::start("control-idle", Scenario::proxy(video_socket(b"h264", &ONE_GROUP)));
    let (id, _) = h.open_and_read_arguments();
    wait_until("the client's message", || control_bytes(&h) == [7]);
    assert!(h.core.sessions().send_control(&id, &ControlMessage::RotateDevice).is_ok());
    wait_until("ours", || control_bytes(&h) == [7, 11]);
    h.release_scrcpy(&id);
}

#[test]
fn a_toolbar_button_acts_through_the_control_socket_when_there_is_a_proxy() {
    let h = Harness::start("control-button", Scenario::proxy(video_socket(b"h264", &ONE_GROUP)));
    let (id, _) = h.open_and_read_arguments();
    wait_until("the client's message", || control_bytes(&h) == [7]);
    assert_eq!(h.ipc("press_button", json!({ "device": id, "button": "back" })), Ok(Value::Null));
    assert_eq!(h.ipc("press_button", json!({ "device": id, "button": "notifications" })), Ok(Value::Null));
    wait_until("the key and the panel", || control_bytes(&h).len() == 1 + 14 + 14 + 1);
    assert_eq!(control_bytes(&h), [vec![7], hex(BACK_KEY_DOWN), hex(BACK_KEY_UP), vec![5]].concat());
    // Nothing was run with adb.
    assert_eq!(h.device_commands(), Vec::<(String, String)>::new());
    h.release_scrcpy(&id);
}

#[test]
fn a_toolbar_button_falls_back_to_adb_when_scrcpy_has_no_control_socket() {
    // scrcpy is told not to open one: there is nothing to send on.
    let scenario = Scenario { extra_args: vec!["--no-control".into()], ..Scenario::proxy(video_socket(b"h264", &ONE_GROUP)) };
    let h = Harness::start("control-fallback", scenario);
    let (id, _) = h.open_and_read_arguments();
    assert_eq!(h.ipc("press_button", json!({ "device": id, "button": "back" })), Ok(Value::Null));
    assert_eq!(h.device_commands(), [command("TESTDEV0001", "input keyevent 4")]);
    assert!(matches!(
        h.core.sessions().send_control(&id, &scrcpy_agent_lib::core::scrcpy::protocol::ControlMessage::RotateDevice),
        Err(scrcpy_agent_lib::core::error::CoreError::ControlUnavailable)
    ));
    h.release_scrcpy(&id);
}

#[test]
fn without_a_proxy_there_is_no_way_to_send_control_messages() {
    let scenario = Scenario { scrcpy_version: "v5.0.1", ..Scenario::usb_device() };
    let h = Harness::start("control-none", scenario);
    let (id, _) = h.open_and_read_arguments();
    assert!(matches!(
        h.core.sessions().send_control(&id, &scrcpy_agent_lib::core::scrcpy::protocol::ControlMessage::RotateDevice),
        Err(scrcpy_agent_lib::core::error::CoreError::NoProxy)
    ));
}

/// Regression test for a real failure: scrcpy opens its audio and control
/// sockets one right after the other, the device gives each its role by the order
/// they arrive, and a proxy that connected them from separate tasks swapped
/// them (scrcpy read the audio codec as a control message and gave up).
#[test]
fn connections_reach_the_device_in_the_order_scrcpy_made_them() {
    let scenario = Scenario { connection_burst: true, ..Scenario::proxy(video_socket(b"h264", &ONE_GROUP)) };
    let h = Harness::start("proxy-order", scenario);
    let (id, _) = h.open_and_read_arguments();
    let device = h.device.as_ref().unwrap();
    // Wait for them (at most a few seconds), then compare what the device saw.
    let deadline = Instant::now() + Duration::from_secs(5);
    while device.accepted_order.lock().unwrap().len() < 20 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(*device.accepted_order.lock().unwrap(), (0u8..20).collect::<Vec<_>>());
    h.release_scrcpy(&id);
}

#[test]
fn a_forward_that_was_listed_before_scrcpy_started_is_not_taken_for_its_own() {
    let video = video_socket(b"h264", &ONE_GROUP);
    let h = Harness::start("proxy-stale", Scenario::proxy(video.clone()));
    // An old forward of the same kind, listed first and leading nowhere.
    let dead_port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    h.adb_server.set_stale_forward("TESTDEV0001", dead_port);
    let (id, _) = h.open_and_read_arguments();
    // The proxy used the new forward: scrcpy got the video.
    assert_eq!(h.file_of_scrcpy("scrcpy-received.bin"), video);
    h.release_scrcpy(&id);
}

#[test]
fn someone_who_follows_late_starts_from_the_last_key_frame() {
    let two_groups = [
        (PacketKind::Config, 0, 24),
        (PacketKind::Key, 100_000, 2000),
        (PacketKind::Delta, 200_000, 300),
        (PacketKind::Key, 300_000, 1800),
        (PacketKind::Delta, 400_000, 200),
    ];
    let h = Harness::start("proxy-late", Scenario::proxy(video_socket(b"h264", &two_groups)));
    let (id, _) = h.open_and_read_arguments();
    let mut expected = stream_start();
    expected.extend([
        packet_event(PacketKind::Config, 0, 24),
        packet_event(PacketKind::Key, 300_000, 1800),
        packet_event(PacketKind::Delta, 400_000, 200),
    ]);
    wait_until("the last frame to be followed", || {
        h.core.sessions().subscribe_video(&id).is_ok_and(|s| s.initial.last() == expected.last())
    });
    assert_eq!(h.core.sessions().subscribe_video(&id).unwrap().initial, expected);
    h.release_scrcpy(&id);
}

#[test]
fn a_subscriber_is_told_of_what_comes_after_it_joined() {
    let h = Harness::start("proxy-live", Scenario::proxy(video_socket(b"h264", &ONE_GROUP)));
    let (id, _) = h.open_and_read_arguments();
    wait_until("the video to be followed", || h.core.sessions().subscribe_video(&id).is_ok_and(|s| s.initial.len() == 8));
    // Nothing more comes from this device, so nothing new is announced...
    let mut subscription = h.core.sessions().subscribe_video(&id).unwrap();
    assert!(subscription.receiver.try_recv().is_err());
    h.release_scrcpy(&id);
}

#[test]
fn with_the_standard_mode_there_is_no_proxy_and_scrcpy_connects_by_itself() {
    let scenario = Scenario { scrcpy_version: "v5.0.1", ..Scenario::usb_device() };
    let h = Harness::start("proxy-off", scenario);
    let (id, args) = h.open_and_read_arguments();
    assert!(!args.iter().any(|a| a == "--force-adb-forward" || a.starts_with("--tunnel-port=")), "{args:?}");
    assert!(matches!(h.core.sessions().subscribe_video(&id), Err(scrcpy_agent_lib::core::error::CoreError::NoProxy)));
    // The forwards were never looked at.
    assert!(!h.adb_services().iter().any(|s| s == "host:list-forward"), "{:?}", h.adb_services());
}

#[test]
fn a_scrcpy_version_without_proxy_support_opens_normally_and_says_so() {
    // The "installed" version is one no protocol adapter exists for.
    let scenario = Scenario { scrcpy_mode: ScrcpyMode::Proxy, ..Scenario::usb_device() };
    let h = Harness::start("proxy-unsupported", scenario);
    let (id, args) = h.open_and_read_arguments();
    assert!(!args.iter().any(|a| a == "--force-adb-forward" || a.starts_with("--tunnel-port=")), "{args:?}");
    assert!(matches!(h.core.sessions().subscribe_video(&id), Err(scrcpy_agent_lib::core::error::CoreError::NoProxy)));

    wait_until("the notice", || {
        h.events.lock().unwrap().iter().any(|e| matches!(e, Event::Notice(n) if n.code == NoticeCode::ProxyUnavailable))
    });
    assert!(level_and_code(&open_console(&h)).contains(&pair("warn", "proxy_unavailable")));
}

#[test]
fn the_menu_of_a_device_card_has_the_built_in_entry_and_those_of_addons() {
    let h = Harness::start("menu", Scenario::usb_device());
    let id = h.wait_for_online_device();
    assert_eq!(
        h.ipc("device_menu", json!({ "device": id })),
        Ok(json!([{ "id": "open_app", "label": null, "enabled": true }]))
    );
    let chosen: std::sync::Arc<Mutex<Vec<String>>> = Default::default();
    let record = chosen.clone();
    h.core
        .addons()
        .register_menu_item("clicker", "go", "Go", std::sync::Arc::new(move |d| record.lock().unwrap().push(d.to_string())))
        .unwrap();
    let item = json!({ "addon": { "addon": "clicker", "id": "go" } });
    assert_eq!(
        h.ipc("device_menu", json!({ "device": id })),
        Ok(json!([
            { "id": "open_app", "label": null, "enabled": true },
            { "id": item, "label": "Go", "enabled": true },
        ]))
    );
    assert_eq!(h.ipc("run_menu_item", json!({ "device": id, "item": item })), Ok(Value::Null));
    assert_eq!(*chosen.lock().unwrap(), std::slice::from_ref(&id));
    // The built-in entry opens a dialog in the UI; it has no action in the core.
    assert_eq!(
        h.ipc("run_menu_item", json!({ "device": id, "item": "open_app" })),
        Err(json!({ "code": "not_available" }))
    );
    assert_eq!(
        h.ipc("device_menu", json!({ "device": "nobody" })),
        Err(json!({ "code": "unknown_device" }))
    );
}

#[test]
fn an_app_is_refused_when_its_name_is_not_a_package_or_scrcpy_is_not_one_that_was_checked() {
    let h = Harness::start("apps-refused", Scenario::usb_device());
    let id = h.wait_for_online_device();
    let open = |package: &str| {
        h.ipc("open_app", json!({ "device": id, "package": package, "name": "x" }))
    };
    for bad in ["", "+com.example.app", "?name", "nodots", "com.example.app --x"] {
        assert_eq!(open(bad), Err(json!({ "code": "invalid_package" })), "{bad:?}");
    }
    // The "installed" scrcpy is one no adapter exists for.
    assert_eq!(open("com.example.app"), Err(json!({ "code": "unsupported_scrcpy" })));
    assert_eq!(h.ipc("list_apps", json!({ "device": id })), Err(json!({ "code": "unsupported_scrcpy" })));
    assert_eq!(
        h.ipc("list_apps", json!({ "device": "nobody" })),
        Err(json!({ "code": "unknown_device" }))
    );
}

#[test]
fn an_app_opens_in_a_scrcpy_window_of_its_own_and_only_once() {
    let scenario = Scenario { scrcpy_version: "v5.0.1", ..Scenario::usb_device() };
    let h = Harness::start("app-open", scenario);
    let id = h.wait_for_online_device();
    // Not about the toolbar, whose window goes through the proxy (there is no device behind it here).
    disable_toolbar(&h);
    let open = || {
        h.ipc("open_app", json!({ "device": id, "package": "com.example.notes", "name": "Notes" }))
    };
    assert_eq!(open(), Ok(Value::Null));
    let args = String::from_utf8(h.file_of_scrcpy("scrcpy-args.txt")).unwrap();
    let args: Vec<&str> = args.lines().collect();
    assert!(args.contains(&"--start-app=com.example.notes"), "{args:?}");
    assert!(args.iter().any(|a| a.starts_with("--new-display")), "{args:?}");
    assert!(args.iter().any(|a| a.starts_with("--window-title=Notes - ")), "{args:?}");
    // The device's own screen is not the one that is open, the app is.
    let key = format!("{id}#app:com.example.notes");
    assert!(h.core.snapshot().sessions.contains(&key));
    assert!(!h.core.sessions().has_session(&id));
    assert_eq!(open(), Err(json!({ "code": "already_open" })));
    h.release_scrcpy(&key);
}

#[test]
fn a_device_can_be_given_a_name_of_its_own_which_is_kept_and_can_be_taken_away() {
    let h = Harness::start("label", Scenario::usb_device());
    let id = h.wait_for_online_device();
    let label_of = || {
        h.ipc("get_snapshot", Value::Null).unwrap()["devices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == json!(id))
            .map(|d| d["label"].clone())
            .unwrap()
    };
    assert_eq!(label_of(), Value::Null);
    assert_eq!(h.ipc("set_device_label", json!({ "device": id, "label": "  Living room  " })), Ok(Value::Null));
    assert_eq!(label_of(), json!("Living room"));
    let saved = h.core.store().known_devices();
    assert_eq!(saved.iter().find(|k| k.id == id).unwrap().label.as_deref(), Some("Living room"));
    // The details carry both the user's name and the one the device reports.
    let details = h.ipc("device_details", json!({ "device": id })).unwrap();
    assert_eq!(details["device"]["label"], json!("Living room"));
    assert!(details["reported_name"].as_str().is_some_and(|n| !n.is_empty()));

    for bad in ["", "   ", "two\nlines", &"x".repeat(65)] {
        assert_eq!(
            h.ipc("set_device_label", json!({ "device": id, "label": bad })),
            Err(json!({ "code": "invalid_label" })),
            "{bad:?}"
        );
    }
    assert_eq!(label_of(), json!("Living room"));
    assert_eq!(
        h.ipc("set_device_label", json!({ "device": "nobody", "label": "x" })),
        Err(json!({ "code": "unknown_device" }))
    );
    assert_eq!(h.ipc("set_device_label", json!({ "device": id, "label": Value::Null })), Ok(Value::Null));
    assert_eq!(label_of(), Value::Null);
}

/// A file of invented content in a folder of its own.
fn local_file(dir: &str, name: &str, content: &[u8]) -> String {
    let folder = std::env::temp_dir().join(format!("scrcpy-agent-send-{}-{dir}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join(name);
    std::fs::write(&path, content).unwrap();
    path.to_str().unwrap().to_string()
}

fn send(h: &Harness, device: &str, files: &[&str]) -> Result<Value, Value> {
    h.ipc("send_files", json!({ "device": device, "files": files }))
}

#[test]
fn files_arrive_in_the_folder_unchanged_and_a_taken_name_gets_a_number() {
    let h = Harness::start("send", Scenario::usb_device());
    let id = h.wait_for_online_device();
    let first = local_file("a", "report.txt", b"first report");
    let second = local_file("b", "report.txt", b"second report");
    let big_content: Vec<u8> = (0..300_000).map(|i| (i % 251) as u8).collect();
    let big = local_file("c", "big.bin", &big_content);

    let outcomes = send(&h, &id, &[&first, &second, &big]).unwrap();
    let dir = "/sdcard/Download/scrcpy-agent";
    assert_eq!(
        outcomes,
        json!([
            { "file": first, "remote": format!("{dir}/report.txt"), "error": null },
            { "file": second, "remote": format!("{dir}/report (1).txt"), "error": null },
            { "file": big, "remote": format!("{dir}/big.bin"), "error": null },
        ])
    );
    let pushed = h.adb_server.pushed();
    assert_eq!(pushed[&format!("{dir}/report.txt")], b"first report");
    assert_eq!(pushed[&format!("{dir}/report (1).txt")], b"second report");
    assert_eq!(pushed[&format!("{dir}/big.bin")], big_content);
    // The folder is made first (quoted for the shell), and the names there are asked for.
    let commands = h.device_commands();
    assert_eq!(commands[0], command("TESTDEV0001", &format!("mkdir -p '{dir}'")));
    assert_eq!(commands[1], command("TESTDEV0001", &format!("ls -1 '{dir}'")));

    // A second batch sees what the first left.
    let again = send(&h, &id, &[&first]).unwrap();
    assert_eq!(again[0]["remote"], json!(format!("{dir}/report (2).txt")));
}

#[test]
fn a_file_that_cannot_be_sent_does_not_stop_the_others_and_has_its_own_reason() {
    let h = Harness::start("send-mixed", Scenario::usb_device());
    let id = h.wait_for_online_device();
    let good = local_file("good", "ok.txt", b"fine");
    let folder = std::path::Path::new(&good).parent().unwrap().to_str().unwrap().to_string();
    let missing = format!("{folder}/gone.txt");
    let outcomes = send(&h, &id, &[&folder, &missing, &good]).unwrap();
    assert_eq!(outcomes[0], json!({ "file": folder, "remote": null, "error": { "code": "not_a_file" } }));
    assert_eq!(outcomes[1], json!({ "file": missing, "remote": null, "error": { "code": "not_a_file" } }));
    assert_eq!(outcomes[2]["remote"], json!("/sdcard/Download/scrcpy-agent/ok.txt"));
    assert_eq!(h.adb_server.pushed().len(), 1);
}

#[test]
fn sending_is_reported_as_it_goes_and_ends_with_everything_sent() {
    let h = Harness::start("send-events", Scenario::usb_device());
    let id = h.wait_for_online_device();
    // Three megabytes: progress is passed on at every megabyte at least.
    let size = 3 * 1024 * 1024;
    let content = vec![7u8; size];
    let file = local_file("ev", "events.bin", &content);
    let missing = format!("{file}.gone");
    send(&h, &id, &[&file, &missing]).unwrap();

    let updates: Vec<_> = h
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            Event::Transfer(t) => Some((**t).clone()),
            _ => None,
        })
        .collect();
    let mine: Vec<_> = updates.iter().filter(|u| u.name == "events.bin").collect();
    assert!(mine.len() >= 3, "a start, progress and the end: {mine:?}");
    assert!(mine.iter().all(|u| u.id == mine[0].id && u.device == id && u.total == size as u64));
    assert_eq!((mine[0].sent, mine[0].phase), (0, scrcpy_agent_lib::core::model::TransferPhase::Sending));
    let last = mine.last().unwrap();
    assert_eq!((last.sent, last.phase), (size as u64, scrcpy_agent_lib::core::model::TransferPhase::Done));
    assert!(mine.windows(2).all(|w| w[0].sent <= w[1].sent), "progress never goes back");
    // A file that was not sent is reported as failed, with the reason.
    let failed: Vec<_> = updates.iter().filter(|u| u.phase == scrcpy_agent_lib::core::model::TransferPhase::Failed).collect();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].error, Some(scrcpy_agent_lib::core::error::CoreError::NotAFile));
}

#[test]
fn files_are_not_sent_to_a_device_that_is_not_there_and_a_bad_folder_is_refused() {
    let h = Harness::start("send-refused", Scenario::usb_device());
    let id = h.wait_for_online_device();
    let file = local_file("r", "r.txt", b"x");
    assert_eq!(send(&h, "nobody", &[&file]), Err(json!({ "code": "unknown_device" })));

    let mut settings = h.ipc("get_snapshot", Value::Null).unwrap()["settings"].clone();
    for bad in ["relative/path", "/sdcard/../data", "", "/a\nb"] {
        settings["send_directory"] = json!(bad);
        assert_eq!(
            h.ipc("save_settings", json!({ "settings": settings })),
            Err(json!({ "code": "invalid_directory" })),
            "{bad:?}"
        );
    }
    // Nothing was sent by any of that.
    assert!(h.adb_server.pushed().is_empty());
    // And a folder that is fine is used at once.
    settings["send_directory"] = json!("/sdcard/Documents/mine/");
    assert_eq!(h.ipc("save_settings", json!({ "settings": settings })), Ok(Value::Null));
    let outcomes = send(&h, &id, &[&file]).unwrap();
    assert_eq!(outcomes[0]["remote"], json!("/sdcard/Documents/mine/r.txt"));
}

#[test]
fn a_file_is_sent_and_then_opened_with_an_app_of_the_device() {
    let h = Harness::start("open-file", Scenario::usb_device());
    let id = h.wait_for_online_device();
    let note = local_file("o1", "my note.txt", b"some text");
    assert_eq!(
        h.ipc("open_file_on_device", json!({ "device": id, "file": note })),
        Ok(json!("/sdcard/Download/scrcpy-agent/my note.txt"))
    );
    assert_eq!(h.adb_server.pushed()["/sdcard/Download/scrcpy-agent/my note.txt"], b"some text");
    let commands: Vec<String> = h.device_commands().into_iter().map(|(_, c)| c).collect();
    // The apps are asked first, then the file is sent, then the app is started.
    assert_eq!(
        commands,
        [
            "cmd package query-activities --brief -a android.intent.action.VIEW -d 'file:///sdcard/Download/scrcpy-agent/my%20note.txt' -t 'text/plain'".to_string(),
            "mkdir -p '/sdcard/Download/scrcpy-agent'".into(),
            "ls -1 '/sdcard/Download/scrcpy-agent'".into(),
            "am start -a android.intent.action.VIEW -d 'file:///sdcard/Download/scrcpy-agent/my%20note.txt' -t 'text/plain'".into(),
        ]
    );
}

#[test]
fn nothing_is_sent_for_a_file_that_the_device_cannot_open() {
    let h = Harness::start("open-file-refused", Scenario::usb_device());
    let id = h.wait_for_online_device();
    let pdf = local_file("o2", "paper.pdf", b"%PDF");
    assert_eq!(h.ipc("open_file_on_device", json!({ "device": id, "file": pdf })), Err(json!({ "code": "no_app" })));
    assert_eq!(
        h.ipc("open_file_on_device", json!({ "device": "nobody", "file": pdf })),
        Err(json!({ "code": "unknown_device" }))
    );
    assert!(h.adb_server.pushed().is_empty());
    assert!(!h.device_commands().iter().any(|(_, c)| c.starts_with("am start")));
}

#[test]
fn a_file_of_a_kind_that_is_not_known_is_given_no_type_and_the_device_decides_what_opens_it() {
    let h = Harness::start("open-file-any", Scenario::usb_device());
    let id = h.wait_for_online_device();
    let log = local_file("o3", "server.log", b"line");
    assert_eq!(
        h.ipc("open_file_on_device", json!({ "device": id, "file": log })),
        Ok(json!("/sdcard/Download/scrcpy-agent/server.log"))
    );
    assert_eq!(h.adb_server.pushed()["/sdcard/Download/scrcpy-agent/server.log"], b"line");
    let commands: Vec<String> = h.device_commands().into_iter().map(|(_, c)| c).collect();
    // The apps that can show any file are asked, and the file is started with the same pattern.
    assert_eq!(
        commands,
        [
            "cmd package query-activities --brief -a android.intent.action.VIEW -d 'file:///sdcard/Download/scrcpy-agent/server.log' -t '*/*'".to_string(),
            "mkdir -p '/sdcard/Download/scrcpy-agent'".into(),
            "ls -1 '/sdcard/Download/scrcpy-agent'".into(),
            "am start -a android.intent.action.VIEW -d 'file:///sdcard/Download/scrcpy-agent/server.log' -t '*/*'".into(),
        ]
    );
}

#[test]
fn a_kind_of_file_that_an_addon_named_is_opened_with_that_type_until_the_addon_is_taken_back() {
    let h = Harness::start("open-file-addon-type", Scenario::usb_device());
    let id = h.wait_for_online_device();
    let log = local_file("o4", "server.log", b"line");
    let support = || h.ipc("open_support", json!({ "device": id, "file": log }));
    // Nothing names it: the device is asked about every app that can show a file.
    assert_eq!(support(), Ok(json!({ "kind": "apps", "count": 3, "is_default": false, "type_known": false })));
    assert_eq!(h.core.files().register_file_type("viewer", "log", "text/plain"), Ok(()));
    assert_eq!(support(), Ok(json!({ "kind": "apps", "count": 2, "is_default": false, "type_known": true })));
    assert_eq!(
        h.ipc("open_file_on_device", json!({ "device": id, "file": log })),
        Ok(json!("/sdcard/Download/scrcpy-agent/server.log"))
    );
    let commands: Vec<String> = h.device_commands().into_iter().map(|(_, c)| c).collect();
    assert_eq!(
        commands.last().map(String::as_str),
        Some("am start -a android.intent.action.VIEW -d 'file:///sdcard/Download/scrcpy-agent/server.log' -t 'text/plain'")
    );
    // Taking the add-on back takes its kind of file with it.
    h.core.remove_addon("viewer");
    assert_eq!(support(), Ok(json!({ "kind": "apps", "count": 3, "is_default": false, "type_known": false })));
}

#[test]
fn a_device_says_whether_it_can_open_a_file_and_whether_it_has_a_default_app_for_it() {
    let h = Harness::start("open-support", Scenario::usb_device());
    let id = h.wait_for_online_device();
    let note = local_file("s1", "a.txt", b"x");
    let pdf = local_file("s2", "b.pdf", b"x");
    let strange = local_file("s3", "c.zzqq9", b"x");
    let support = |file: &str| h.ipc("open_support", json!({ "device": id, "file": file }));
    // Two apps, none made the default: the device would ask.
    assert_eq!(support(&note), Ok(json!({ "kind": "apps", "count": 2, "is_default": false, "type_known": true })));
    // Both questions have the shape of the start, with the address the file will have: a question about
    // the type alone makes the device forget the app the user made the default for it.
    let commands: Vec<String> = h.device_commands().into_iter().map(|(_, c)| c).collect();
    assert_eq!(
        commands,
        [
            "cmd package query-activities --brief -a android.intent.action.VIEW -d 'file:///sdcard/Download/scrcpy-agent/a.txt' -t 'text/plain'",
            "cmd package resolve-activity --brief -a android.intent.action.VIEW -d 'file:///sdcard/Download/scrcpy-agent/a.txt' -t 'text/plain'",
        ]
    );
    assert_eq!(support(&pdf), Ok(json!({ "kind": "no_app" })));
    // A kind that is not known: the device is asked about every app that can show a file.
    assert_eq!(support(&strange), Ok(json!({ "kind": "apps", "count": 3, "is_default": false, "type_known": false })));
    assert_eq!(
        h.ipc("open_support", json!({ "device": "nobody", "file": note })),
        Err(json!({ "code": "unknown_device" }))
    );
    // Asking sends nothing.
    assert!(h.adb_server.pushed().is_empty());
}

impl Harness {
    /// Runs a future of the core on the runtime of the test's application.
    fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        self.runtime.as_ref().unwrap().block_on(future)
    }

    fn shell(&self, action: scrcpy_agent_lib::core::requests::shell_request::ShellAction, files: &[&str]) -> scrcpy_agent_lib::core::ShellStart {
        self.block_on(self.core.requests().handle_shell(action, files.iter().map(Into::into).collect()))
    }
}

fn set_default_device(h: &Harness, device: Option<&str>) {
    let mut settings = h.ipc("get_snapshot", Value::Null).unwrap()["settings"].clone();
    settings["default_device"] = json!(device);
    assert_eq!(h.ipc("save_settings", json!({ "settings": settings })), Ok(Value::Null));
}

#[test]
fn a_request_for_the_default_device_goes_to_it_and_the_user_is_asked_when_there_is_none() {
    use scrcpy_agent_lib::core::requests::shell_request::ShellAction;
    use scrcpy_agent_lib::core::ShellStart;
    let h = Harness::start("shell-default", Scenario::usb_device());
    let id = h.wait_for_online_device();
    let a = local_file("sh1", "a.txt", b"aaa");
    let b = local_file("sh2", "b.txt", b"bbb");

    // No default device: nothing is sent, the user is asked.
    assert_eq!(h.shell(ShellAction::SendDefault, &[&a]), ShellStart::NeedsChoice);
    let waiting = h.core.requests().pending_choice().expect("a request waits");
    assert_eq!((waiting.action, waiting.files.clone(), waiting.no_default), (ShellAction::SendDefault, vec![a.clone()], true));
    assert!(h.adb_server.pushed().is_empty());

    // The choice sends what waited, to the device chosen, and nothing waits any more.
    assert_eq!(h.ipc("resolve_choice", json!({ "device": id })), Ok(Value::Null));
    wait_until("the file to arrive", || !h.adb_server.pushed().is_empty());
    assert_eq!(h.adb_server.pushed()["/sdcard/Download/scrcpy-agent/a.txt"], b"aaa");
    assert!(h.core.requests().pending_choice().is_none());
    assert_eq!(h.ipc("resolve_choice", json!({ "device": id })), Err(json!({ "code": "not_available" })));

    // With a default device a request is carried out at once, for all its files.
    set_default_device(&h, Some(&id));
    assert_eq!(h.shell(ShellAction::SendDefault, &[&b]), ShellStart::Running);
    wait_until("the second file to arrive", || h.adb_server.pushed().len() == 2);
    assert_eq!(h.adb_server.pushed()["/sdcard/Download/scrcpy-agent/b.txt"], b"bbb");

    // A default device that is not there is the same as none.
    set_default_device(&h, Some("not-a-device"));
    assert_eq!(h.shell(ShellAction::SendDefault, &[&a]), ShellStart::NeedsChoice);
    assert!(h.core.requests().pending_choice().unwrap().no_default);
}

#[test]
fn a_request_to_choose_a_device_asks_even_when_there_is_a_default_and_can_be_dropped() {
    use scrcpy_agent_lib::core::requests::shell_request::ShellAction;
    use scrcpy_agent_lib::core::ShellStart;
    let h = Harness::start("shell-choose", Scenario::usb_device());
    let id = h.wait_for_online_device();
    set_default_device(&h, Some(&id));
    let a = local_file("sc1", "c.txt", b"ccc");
    assert_eq!(h.shell(ShellAction::OpenChoose, &[&a]), ShellStart::NeedsChoice);
    let waiting = h.core.requests().pending_choice().unwrap();
    assert_eq!((waiting.action, waiting.no_default), (ShellAction::OpenChoose, false));
    // A newer request takes the place of the one that waited.
    assert_eq!(h.shell(ShellAction::SendChoose, &[&a]), ShellStart::NeedsChoice);
    assert_eq!(h.core.requests().pending_choice().unwrap().action, ShellAction::SendChoose);
    h.core.requests().cancel_choice();
    assert!(h.core.requests().pending_choice().is_none());
    // A device that is not there leaves the request waiting.
    h.shell(ShellAction::SendChoose, &[&a]);
    assert_eq!(h.ipc("resolve_choice", json!({ "device": "nobody" })), Err(json!({ "code": "unknown_device" })));
    assert!(h.core.requests().pending_choice().is_some());
    assert!(h.adb_server.pushed().is_empty());
}

/// What the small window was told since `from` events: the phase of each view, one
/// word for each change, and "closed" when it was told there is none.
fn request_phases(h: &Harness, from: usize) -> Vec<String> {
    let mut phases: Vec<String> = Vec::new();
    for event in h.events.lock().unwrap().iter().skip(from) {
        let json = serde_json::to_value(event).unwrap();
        if json["kind"] != "request" {
            continue;
        }
        let word = json["data"]["phase"].as_str().unwrap_or("closed").to_string();
        if phases.last() != Some(&word) {
            phases.push(word);
        }
    }
    phases
}

fn current_request(h: &Harness) -> Value {
    serde_json::to_value(h.core.requests().current_request()).unwrap()
}

#[test]
fn a_request_of_a_file_manager_is_followed_by_its_view_from_the_question_to_the_end() {
    use scrcpy_agent_lib::core::requests::shell_request::ShellAction;
    let h = Harness::start("request-view", Scenario::usb_device());
    let id = h.wait_for_online_device();
    let a = local_file("rv1", "a.txt", b"aaa");
    let b = local_file("rv2", "b.txt", b"bbb");

    // Asked first: the view has the files waiting and no device.
    let from = h.events.lock().unwrap().len();
    h.shell(ShellAction::SendChoose, &[&a]);
    let view = current_request(&h);
    assert_eq!(view["phase"], "choosing");
    assert_eq!(view["device"], Value::Null);
    assert_eq!(view["items"], json!([{ "file": a, "state": "waiting" }]));
    // Then the device is chosen, the file is sent, and the window may close itself soon.
    assert_eq!(h.ipc("resolve_choice", json!({ "device": id })), Ok(Value::Null));
    wait_until("the view to be done", || current_request(&h)["phase"] == "done");
    let view = current_request(&h);
    assert_eq!((view["device"].as_str(), view["close_after_ms"].as_u64()), (Some(id.as_str()), Some(2000)));
    assert_eq!(view["items"], json!([{ "file": a, "state": "done" }]));
    assert_eq!(request_phases(&h, from), ["choosing", "working", "done"]);
    // When the window is closed the view is gone, and the window is told so.
    let number = view["id"].as_u64().unwrap();
    h.core.requests().dismiss_request(number);
    assert_eq!(current_request(&h), Value::Null);
    assert_eq!(request_phases(&h, from), ["choosing", "working", "done", "closed"]);

    // With a default device there is no question at all.
    set_default_device(&h, Some(&id));
    let from = h.events.lock().unwrap().len();
    h.shell(ShellAction::SendDefault, &[&a, &b]);
    wait_until("the view to be done", || current_request(&h)["phase"] == "done");
    assert_eq!(request_phases(&h, from), ["working", "done"]);
    assert_eq!(current_request(&h)["items"].as_array().unwrap().len(), 2);
}

fn set_request_close_seconds(h: &Harness, seconds: u32) {
    let mut settings = h.ipc("get_snapshot", Value::Null).unwrap()["settings"].clone();
    settings["request_close_seconds"] = json!(seconds);
    assert_eq!(h.ipc("save_settings", json!({ "settings": settings })), Ok(Value::Null));
}

#[test]
fn how_long_the_window_of_a_request_stays_after_success_is_the_setting_and_zero_is_at_once() {
    use scrcpy_agent_lib::core::requests::shell_request::ShellAction;
    let h = Harness::start("request-close-time", Scenario::usb_device());
    let id = h.wait_for_online_device();
    set_default_device(&h, Some(&id));
    let a = local_file("rc1", "a.txt", b"a");
    let close_after = |h: &Harness| {
        wait_until("the view to be done", || current_request(h)["phase"] == "done");
        current_request(h)["close_after_ms"].clone()
    };
    // Two seconds unless it was changed.
    h.shell(ShellAction::SendDefault, &[&a]);
    assert_eq!(close_after(&h), json!(2000));
    // A change is used by the next request, with no restart.
    set_request_close_seconds(&h, 5);
    h.shell(ShellAction::SendDefault, &[&a]);
    assert_eq!(close_after(&h), json!(5000));
    // 0: the window does not wait.
    set_request_close_seconds(&h, 0);
    h.shell(ShellAction::SendDefault, &[&a]);
    assert_eq!(close_after(&h), json!(0));
    // A request that waits for a choice uses the setting too once it is carried out.
    set_request_close_seconds(&h, 3);
    h.shell(ShellAction::SendChoose, &[&a]);
    assert_eq!(h.ipc("resolve_choice", json!({ "device": id })), Ok(Value::Null));
    assert_eq!(close_after(&h), json!(3000));
}

#[test]
fn a_file_that_goes_wrong_keeps_the_view_open_with_the_reason_while_the_others_are_done() {
    use scrcpy_agent_lib::core::requests::shell_request::ShellAction;
    let h = Harness::start("request-failed", Scenario::usb_device());
    let id = h.wait_for_online_device();
    set_default_device(&h, Some(&id));
    let note = local_file("rf1", "n.txt", b"n");
    let pdf = local_file("rf2", "p.pdf", b"p");
    let from = h.events.lock().unwrap().len();
    h.shell(ShellAction::OpenDefault, &[&note, &pdf]);
    wait_until("the view to end", || matches!(current_request(&h)["phase"].as_str(), Some("done" | "failed")));
    let view = current_request(&h);
    // The note was opened; no app of the device opens a PDF.
    assert_eq!(
        view["items"],
        json!([
            { "file": note, "state": "done" },
            { "file": pdf, "state": "failed", "error": { "code": "no_app" } },
        ])
    );
    assert_eq!((view["phase"].as_str(), view["close_after_ms"].clone()), (Some("failed"), Value::Null));
    assert_eq!(request_phases(&h, from), ["working", "failed"]);
}

#[test]
fn a_newer_request_takes_the_place_of_the_view_and_the_window_of_the_older_one_cannot_close_it() {
    use scrcpy_agent_lib::core::requests::shell_request::ShellAction;
    let h = Harness::start("request-replace", Scenario::usb_device());
    h.wait_for_online_device();
    let a = local_file("rp1", "a.txt", b"a");
    h.shell(ShellAction::SendChoose, &[&a]);
    assert_eq!(current_request(&h)["action"], "send_choose");
    let older = current_request(&h)["id"].as_u64().unwrap();
    h.shell(ShellAction::OpenChoose, &[&a]);
    assert_eq!(current_request(&h)["action"], "open_choose");
    let newer = current_request(&h)["id"].as_u64().unwrap();
    assert_ne!(older, newer);
    assert_eq!(current_request(&h)["action"], "open_choose");
    // The window that was showing the older one closes: the newer one stays.
    h.core.requests().dismiss_request(older);
    assert_eq!(current_request(&h)["id"].as_u64(), Some(newer));
    // Not wanting a device any more takes the view away.
    let from = h.events.lock().unwrap().len();
    h.core.requests().cancel_choice();
    assert_eq!(current_request(&h), Value::Null);
    assert_eq!(request_phases(&h, from), ["closed"]);
    // Asking for a view that is not there does nothing.
    h.core.requests().dismiss_request(newer);
    assert_eq!(request_phases(&h, from), ["closed"]);
}

#[test]
fn opening_from_the_menu_sends_and_starts_the_app_and_says_how_it_ended() {
    use scrcpy_agent_lib::core::requests::shell_request::ShellAction;
    let h = Harness::start("shell-open", Scenario::usb_device());
    let id = h.wait_for_online_device();
    set_default_device(&h, Some(&id));
    let note = local_file("so1", "n.txt", b"n");
    let pdf = local_file("so2", "p.pdf", b"p");
    h.shell(ShellAction::OpenDefault, &[&note, &pdf]);
    wait_until("both to be answered", || {
        h.events.lock().unwrap().iter().filter(|e| matches!(e, Event::Opened(_))).count() == 2
    });
    let opened: Vec<_> = h
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            Event::Opened(o) => Some((**o).clone()),
            _ => None,
        })
        .collect();
    assert_eq!(opened[0].file, note);
    assert_eq!(opened[0].remote.as_deref(), Some("/sdcard/Download/scrcpy-agent/n.txt"));
    assert_eq!(opened[0].error, None);
    // The file nothing can open is reported, and was not sent.
    assert_eq!(opened[1].file, pdf);
    assert_eq!(opened[1].error, Some(scrcpy_agent_lib::core::error::CoreError::NoApp));
    assert_eq!(h.adb_server.pushed().len(), 1);
}

fn set_keyboard(h: &Harness, mode: &str) {
    let mut settings = h.ipc("get_snapshot", Value::Null).unwrap()["settings"].clone();
    settings["keyboard_mode"] = json!(mode);
    assert_eq!(h.ipc("save_settings", json!({ "settings": settings })), Ok(Value::Null));
}

#[test]
fn the_keyboard_mode_of_the_setting_is_given_to_scrcpy_for_the_screen_and_for_an_app() {
    let scenario = Scenario { scrcpy_version: "v5.0.1", ..Scenario::usb_device() };
    let h = Harness::start("keyboard", scenario);
    let id = h.wait_for_online_device();
    // The default is a physical keyboard.
    let (_, args) = h.open_and_read_arguments();
    assert!(args.iter().any(|a| a == "--keyboard=uhid"), "{args:?}");
    h.release_scrcpy(&id);
    std::fs::remove_file(h.install.root.join("scrcpy-args.txt")).unwrap();
    std::fs::remove_file(h.install.root.join("scrcpy-release.txt")).unwrap();

    // The standard mode is what scrcpy does without an argument; changed in the
    // settings, it applies to the next screen at once.
    set_keyboard(&h, "sdk");
    let (_, args) = h.open_and_read_arguments();
    assert!(!args.iter().any(|a| a.starts_with("--keyboard")), "{args:?}");
    h.release_scrcpy(&id);
    std::fs::remove_file(h.install.root.join("scrcpy-args.txt")).unwrap();
    std::fs::remove_file(h.install.root.join("scrcpy-release.txt")).unwrap();
    set_keyboard(&h, "uhid");

    // And to an app's window.
    assert_eq!(open_notes(&h, &id), Ok(Value::Null));
    let args = String::from_utf8(h.file_of_scrcpy("scrcpy-args.txt")).unwrap();
    assert!(args.lines().any(|a| a == "--keyboard=uhid"), "{args}");
}

fn set_bit_rate(h: &Harness, mbps: u32) {
    let mut settings = h.ipc("get_snapshot", Value::Null).unwrap()["settings"].clone();
    settings["video_bit_rate_mbps"] = json!(mbps);
    assert_eq!(h.ipc("save_settings", json!({ "settings": settings })), Ok(Value::Null));
}

#[test]
fn the_video_bit_rate_of_the_setting_is_given_to_scrcpy_for_the_screen_and_for_an_app() {
    let scenario = Scenario { scrcpy_version: "v5.0.1", ..Scenario::usb_device() };
    let h = Harness::start("bit-rate", scenario);
    let id = h.wait_for_online_device();
    disable_toolbar(&h);
    // Scrcpy's own choice is no argument.
    let (_, args) = h.open_and_read_arguments();
    assert!(!args.iter().any(|a| a.starts_with("--video-bit-rate")), "{args:?}");
    h.release_scrcpy(&id);
    std::fs::remove_file(h.install.root.join("scrcpy-args.txt")).unwrap();
    std::fs::remove_file(h.install.root.join("scrcpy-release.txt")).unwrap();

    // A rate set in the settings applies to the next screen at once, and to an app's window.
    set_bit_rate(&h, 16);
    let (_, args) = h.open_and_read_arguments();
    assert!(args.iter().any(|a| a == "--video-bit-rate=16M"), "{args:?}");
    h.release_scrcpy(&id);
    std::fs::remove_file(h.install.root.join("scrcpy-args.txt")).unwrap();
    assert_eq!(open_notes(&h, &id), Ok(Value::Null));
    let args = String::from_utf8(h.file_of_scrcpy("scrcpy-args.txt")).unwrap();
    assert!(args.lines().any(|a| a == "--video-bit-rate=16M"), "{args}");

    // A number that is not allowed is the default (scrcpy's own choice), as in a file.
    let mut settings = h.ipc("get_snapshot", Value::Null).unwrap()["settings"].clone();
    assert_eq!(settings["video_bit_rate_mbps"], json!(16));
    settings["video_bit_rate_mbps"] = json!(201);
    assert_eq!(h.ipc("save_settings", json!({ "settings": settings })), Ok(Value::Null));
    assert_eq!(h.ipc("get_snapshot", Value::Null).unwrap()["settings"]["video_bit_rate_mbps"], json!(0));
}

#[test]
fn the_menu_of_an_app_has_the_built_in_entries_and_those_of_addons_and_runs_an_addons_entry() {
    let h = Harness::start("app-menu", Scenario::usb_device());
    let id = h.wait_for_online_device();
    let built_in = |enabled: bool| {
        json!([
            { "id": "open", "label": null, "enabled": enabled },
            { "id": "open_on_main_screen", "label": null, "enabled": enabled },
            { "id": "toggle_favorite", "label": null, "enabled": enabled },
            { "id": "shortcut_desktop", "label": null, "enabled": enabled },
            { "id": "shortcut_start_menu", "label": null, "enabled": enabled },
        ])
    };
    assert_eq!(h.ipc("app_menu", json!({ "device": id, "package": "com.example.notes" })), Ok(built_in(true)));
    let chosen: std::sync::Arc<Mutex<Vec<(String, String)>>> = Default::default();
    let record = chosen.clone();
    h.core
        .addons()
        .register_app_menu_item(
            "tools",
            "share",
            "Share",
            std::sync::Arc::new(move |d, p| record.lock().unwrap().push((d.to_string(), p.to_string()))),
        )
        .unwrap();
    let menu = h.ipc("app_menu", json!({ "device": id, "package": "com.example.notes" })).unwrap();
    assert_eq!(menu.as_array().unwrap().len(), 6);
    let item = json!({ "addon": { "addon": "tools", "id": "share" } });
    assert_eq!(menu[5], json!({ "id": item, "label": "Share", "enabled": true }));
    assert_eq!(
        h.ipc("run_app_menu_item", json!({ "device": id, "package": "com.example.notes", "item": item })),
        Ok(Value::Null)
    );
    assert_eq!(*chosen.lock().unwrap(), [(id.clone(), "com.example.notes".to_string())]);
    // The built-in entries are carried out by the UI with their own commands.
    assert_eq!(
        h.ipc("run_app_menu_item", json!({ "device": id, "package": "com.example.notes", "item": "open" })),
        Err(json!({ "code": "not_available" }))
    );
    assert_eq!(
        h.ipc("app_menu", json!({ "device": id, "package": "not a package" })),
        Err(json!({ "code": "invalid_package" }))
    );
    assert_eq!(
        h.ipc("app_menu", json!({ "device": "nobody", "package": "com.example.notes" })),
        Err(json!({ "code": "unknown_device" }))
    );
    // Taking an add-on away takes its entries away.
    h.core.addons().remove_addon("tools");
    assert_eq!(h.ipc("app_menu", json!({ "device": id, "package": "com.example.notes" })), Ok(built_in(true)));
}

#[test]
fn an_app_opens_where_the_menu_says_instead_of_where_the_devices_settings_say() {
    let scenario = Scenario { scrcpy_version: "v5.0.1", ..Scenario::usb_device() };
    let h = Harness::start("app-open-where", scenario);
    let id = h.wait_for_online_device();
    disable_toolbar(&h);
    let key = format!("{id}#app:com.example.notes");
    let open = |display: Value| {
        let opened = h.ipc("open_app", json!({ "device": id, "package": "com.example.notes", "name": "Notes", "display": display }));
        assert_eq!(opened, Ok(Value::Null));
        let args = String::from_utf8(h.file_of_scrcpy("scrcpy-args.txt")).unwrap();
        h.release_scrcpy(&key);
        std::fs::remove_file(h.install.root.join("scrcpy-args.txt")).unwrap();
        std::fs::remove_file(h.install.root.join("scrcpy-release.txt")).unwrap();
        args
    };
    // The device's settings (a display of its own) unless the menu asks for the main screen.
    assert!(open(Value::Null).lines().any(|a| a.starts_with("--new-display")));
    assert!(!open(json!("main_display")).lines().any(|a| a.starts_with("--new-display")));
    assert!(open(json!("virtual_display")).lines().any(|a| a.starts_with("--new-display")));
    change_device_settings(&h, &id, json!({ "apps_in_virtual_display": false }));
    assert!(!open(Value::Null).lines().any(|a| a.starts_with("--new-display")));
    assert!(open(json!("virtual_display")).lines().any(|a| a.starts_with("--new-display")));
}

#[test]
fn a_shortcut_opens_the_app_with_the_name_the_list_had_and_a_device_that_is_not_ready_is_refused() {
    let scenario = Scenario { scrcpy_version: "v5.0.1", ..Scenario::usb_device() };
    let h = Harness::start("app-shortcut", scenario);
    let id = h.wait_for_online_device();
    disable_toolbar(&h);
    // The list the device window kept (the name of the app is in it, not in the shortcut).
    let cache = scrcpy_agent_lib::core::apps::AppCache {
        updated_at: 1,
        apps: vec![scrcpy_agent_lib::core::apps::AppEntry {
            package: "com.example.notes".into(),
            name: "Notes Pro".into(),
            system: false,
        }],
    };
    h.core.store().device_of(&id).unwrap().save::<store::Apps>(&cache).unwrap();
    let package = scrcpy_agent_lib::core::apps::PackageName::parse("com.example.notes").unwrap();
    let opened = h.block_on(h.core.apps().open_app_from_shortcut(&id, &package)).unwrap();
    assert!(opened.title.starts_with("Notes Pro - "), "{}", opened.title);
    assert_eq!(opened.key, format!("{id}#app:com.example.notes"));
    h.release_scrcpy(&opened.key);
    // An app that is not in the list is named by its package.
    let other = scrcpy_agent_lib::core::apps::PackageName::parse("com.example.other").unwrap();
    let opened = h.block_on(h.core.apps().open_app_from_shortcut(&id, &other)).unwrap();
    assert!(opened.title.starts_with("com.example.other - "), "{}", opened.title);
    h.release_scrcpy(&opened.key);
    // A device that is not there, or not ready, is refused as such.
    let gone = h.block_on(h.core.apps().open_app_from_shortcut("nobody", &package));
    assert!(matches!(gone, Err(scrcpy_agent_lib::core::error::CoreError::UnknownDevice)));
}

/// The places of the shortcuts for a test: folders of the test (the real desktop and menu
/// of the user are not touched), with the kind of shortcut this system has.
#[cfg(any(windows, target_os = "linux"))]
fn test_shortcut_places(root: &std::path::Path, owns_start_menu_folder: bool) -> scrcpy_agent_lib::ShortcutPlaces {
    use scrcpy_agent_lib::core::{integration::shortcuts::ShortcutKind, window::layout::Os};
    scrcpy_agent_lib::ShortcutPlaces {
        kind: ShortcutKind::of(Os::current()).expect("this system has shortcuts"),
        desktop: root.join("desktop"),
        start_menu: root.join("start").join("scrcpy-agent"),
        owns_start_menu_folder,
        exe: std::env::current_exe().unwrap(),
    }
}

/// The shortcuts of an app are made, found and removed through the commands the
/// menu uses, in folders of the test.
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn a_shortcut_of_an_app_can_be_found_and_removed_from_each_place_apart() {
    use tauri::Manager;
    let scenario = Scenario { scrcpy_version: "v5.0.1", ..Scenario::usb_device() };
    let h = Harness::start("app-shortcut-remove", scenario);
    let id = h.wait_for_online_device();
    let root = h.dir.join("shortcut-places");
    h._app.manage(test_shortcut_places(&root, true));
    let args = |place: &str| json!({ "device": id, "package": "com.example.notes", "place": place });
    let state = || h.ipc("app_shortcut_state", json!({ "device": id, "package": "com.example.notes" }));
    assert_eq!(state(), Ok(json!({ "desktop": false, "start_menu": false })));

    h.ipc("create_app_shortcut", args("desktop")).unwrap();
    h.ipc("create_app_shortcut", args("start_menu")).unwrap();
    assert_eq!(state(), Ok(json!({ "desktop": true, "start_menu": true })));

    // Each place on its own; the Start menu folder this program made goes with its last shortcut.
    h.ipc("remove_app_shortcut", args("start_menu")).unwrap();
    assert_eq!(state(), Ok(json!({ "desktop": true, "start_menu": false })));
    assert!(!root.join("start").join("scrcpy-agent").exists());
    assert!(root.join("desktop").exists());
    // Removing what is not there is not an error.
    assert_eq!(h.ipc("remove_app_shortcut", args("start_menu")), Ok(Value::Null));
    h.ipc("remove_app_shortcut", args("desktop")).unwrap();
    assert_eq!(state(), Ok(json!({ "desktop": false, "start_menu": false })));
    // The desktop folder is never removed.
    assert!(root.join("desktop").exists());
    // A device that is not there is refused as such.
    let gone = h.ipc("remove_app_shortcut", json!({ "device": "nobody", "package": "com.example.notes", "place": "desktop" }));
    assert_eq!(gone.unwrap_err()["code"], "unknown_device");
}

/// The device is shown by a name the user can change, and a shortcut is named after it; what
/// the menu knows of its shortcuts must not depend on the name the device has now.
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn the_shortcuts_of_a_device_are_still_known_after_the_device_is_renamed() {
    use tauri::Manager;
    let scenario = Scenario { scrcpy_version: "v5.0.1", ..Scenario::usb_device() };
    let h = Harness::start("app-shortcut-rename", scenario);
    let id = h.wait_for_online_device();
    let root = h.dir.join("shortcut-places-rename");
    h._app.manage(test_shortcut_places(&root, true));
    let args = |place: &str| json!({ "device": id, "package": "com.example.notes", "place": place });
    let state = || h.ipc("app_shortcut_state", json!({ "device": id, "package": "com.example.notes" }));
    let files = |folder: &str| -> Vec<String> {
        std::fs::read_dir(root.join(folder))
            .map(|d| d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default()
    };

    h.ipc("create_app_shortcut", args("desktop")).unwrap();
    let before = files("desktop");
    assert_eq!(before.len(), 1);
    assert_eq!(state(), Ok(json!({ "desktop": true, "start_menu": false })));

    // The device is given another name: the shortcut is the same one, and can still be taken away.
    assert_eq!(h.ipc("set_device_label", json!({ "device": id, "label": "Other name" })), Ok(Value::Null));
    assert_eq!(state(), Ok(json!({ "desktop": true, "start_menu": false })));

    // Making it again replaces it (under the name the device has now) and leaves no second one.
    h.ipc("create_app_shortcut", args("desktop")).unwrap();
    let after = files("desktop");
    assert_eq!(after.len(), 1, "{after:?}");
    assert_ne!(after, before);
    assert!(after[0].contains("Other name"), "{after:?}");

    h.ipc("remove_app_shortcut", args("desktop")).unwrap();
    assert!(files("desktop").is_empty());
    assert_eq!(state(), Ok(json!({ "desktop": false, "start_menu": false })));

    // A shortcut whose file was taken away by hand is not there any more.
    h.ipc("create_app_shortcut", args("start_menu")).unwrap();
    assert_eq!(state(), Ok(json!({ "desktop": false, "start_menu": true })));
    for name in files("start/scrcpy-agent") {
        std::fs::remove_file(root.join("start").join("scrcpy-agent").join(name)).unwrap();
    }
    assert_eq!(state(), Ok(json!({ "desktop": false, "start_menu": false })));
    // And the one made before the renaming does not come back as another.
    assert_eq!(h.ipc("remove_app_shortcut", args("start_menu")), Ok(Value::Null));
}

/// On Linux the menu of applications is a folder of every program of the user, not of this one:
/// it stays when its last shortcut of this program goes.
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn a_folder_of_the_menu_that_is_not_this_programs_own_is_not_removed_with_its_last_shortcut() {
    use tauri::Manager;
    let scenario = Scenario { scrcpy_version: "v5.0.1", ..Scenario::usb_device() };
    let h = Harness::start("app-shortcut-shared-folder", scenario);
    let id = h.wait_for_online_device();
    let root = h.dir.join("shortcut-places-shared");
    h._app.manage(test_shortcut_places(&root, false));
    let args = json!({ "device": id, "package": "com.example.notes", "place": "start_menu" });
    h.ipc("create_app_shortcut", args.clone()).unwrap();
    let folder = root.join("start").join("scrcpy-agent");
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1);
    // Another program's file in the same folder, which must stay whatever happens.
    std::fs::write(folder.join("other-program.desktop"), "[Desktop Entry]\n").unwrap();
    h.ipc("remove_app_shortcut", args.clone()).unwrap();
    assert!(folder.join("other-program.desktop").exists());
    // Even with nothing left in it, the folder is not this program's to remove.
    std::fs::remove_file(folder.join("other-program.desktop")).unwrap();
    h.ipc("create_app_shortcut", args.clone()).unwrap();
    h.ipc("remove_app_shortcut", args).unwrap();
    assert!(folder.exists(), "the folder of the menu is not removed");
}

fn app_usage(h: &Harness, id: &str) -> Value {
    h.ipc("app_usage", json!({ "device": id })).unwrap()
}

#[test]
fn the_apps_opened_from_the_program_are_kept_newest_first_each_once_and_at_most_twenty() {
    let scenario = Scenario { scrcpy_version: "v5.0.1", ..Scenario::usb_device() };
    let h = Harness::start("recent-apps", scenario);
    let id = h.wait_for_online_device();
    disable_toolbar(&h);
    assert_eq!(app_usage(&h, &id), json!({ "favorites": [], "recent": [] }));
    let open = |package: &str| {
        let opened = h.ipc("open_app", json!({ "device": id, "package": package, "name": "x" }));
        assert_eq!(opened, Ok(Value::Null), "{package}");
        // The window goes away at once so the same app can be opened again.
        let key = format!("{id}#app:{package}");
        std::fs::write(h.install.root.join("scrcpy-release.txt"), b"go").unwrap();
        wait_until("the window to end", || !h.core.sessions().has_session(&key));
        std::fs::remove_file(h.install.root.join("scrcpy-args.txt")).unwrap();
        std::fs::remove_file(h.install.root.join("scrcpy-release.txt")).unwrap();
    };
    open("com.example.one");
    open("com.example.two");
    assert_eq!(app_usage(&h, &id)["recent"], json!(["com.example.two", "com.example.one"]));
    // Opened again, it moves to the front and is not listed twice.
    open("com.example.one");
    assert_eq!(app_usage(&h, &id)["recent"], json!(["com.example.one", "com.example.two"]));
    // An app that could not be opened is not recent.
    assert!(h.ipc("open_app", json!({ "device": id, "package": "not a package", "name": "x" })).is_err());
    assert_eq!(app_usage(&h, &id)["recent"].as_array().unwrap().len(), 2);
    // At most twenty are kept; the oldest go.
    for n in 0..22 {
        open(&format!("com.example.app{n}"));
    }
    let recent = app_usage(&h, &id)["recent"].clone();
    assert_eq!(recent.as_array().unwrap().len(), 20);
    assert_eq!(recent[0], json!("com.example.app21"));
    assert_eq!(recent[19], json!("com.example.app2"));
    // It is in the file of the known devices.
    let saved = h.core.store().known_devices();
    assert_eq!(saved.iter().find(|k| k.id == id).unwrap().apps.recent.len(), 20);
}

#[test]
fn an_app_is_marked_as_a_favourite_and_unmarked_and_only_a_package_of_a_known_device_can_be() {
    let h = Harness::start("favourite-apps", Scenario::usb_device());
    let id = h.wait_for_online_device();
    let mark = |package: &str, favorite: bool| {
        h.ipc("set_app_favorite", json!({ "device": id, "package": package, "favorite": favorite }))
    };
    assert_eq!(mark("com.example.one", true), Ok(Value::Null));
    assert_eq!(mark("com.example.two", true), Ok(Value::Null));
    // In the order they were marked, each once.
    assert_eq!(mark("com.example.one", true), Ok(Value::Null));
    assert_eq!(app_usage(&h, &id)["favorites"], json!(["com.example.one", "com.example.two"]));
    assert_eq!(mark("com.example.one", false), Ok(Value::Null));
    assert_eq!(app_usage(&h, &id)["favorites"], json!(["com.example.two"]));
    // Taking away what is not there is not an error.
    assert_eq!(mark("com.example.one", false), Ok(Value::Null));
    assert_eq!(mark("+com.example.one", true), Err(json!({ "code": "invalid_package" })));
    assert_eq!(mark("not a package", true), Err(json!({ "code": "invalid_package" })));
    assert_eq!(
        h.ipc("set_app_favorite", json!({ "device": "nobody", "package": "com.example.one", "favorite": true })),
        Err(json!({ "code": "unknown_device" }))
    );
    let saved = h.core.store().known_devices();
    assert_eq!(saved.iter().find(|k| k.id == id).unwrap().apps.favorites, ["com.example.two"]);
}

#[test]
fn what_is_set_for_a_device_is_kept_with_it_and_a_device_that_is_not_there_has_none() {
    let h = Harness::start("device-settings", Scenario::usb_device());
    let id = h.wait_for_online_device();
    assert_eq!(
        device_settings(&h, &id),
        json!({
            "keyboard_mode": null, "video_bit_rate_mbps": null, "max_size": 0,
            "apps_in_virtual_display": true, "keep_awake": false,
        })
    );
    change_device_settings(&h, &id, json!({ "keyboard_mode": "sdk", "video_bit_rate_mbps": 20, "max_size": 1280 }));
    let again = device_settings(&h, &id);
    assert_eq!((&again["keyboard_mode"], &again["video_bit_rate_mbps"], &again["max_size"]), (&json!("sdk"), &json!(20), &json!(1280)));
    // It is in the file of the known devices, so it is still there after a restart.
    let saved = h.core.store().known_devices();
    assert_eq!(saved.iter().find(|k| k.id == id).unwrap().settings.max_size.pixels(), Some(1280));
    // A number that is not allowed is the general setting, and the rest is kept.
    change_device_settings(&h, &id, json!({ "max_size": 50, "video_bit_rate_mbps": 500, "keep_awake": true }));
    let after = device_settings(&h, &id);
    assert_eq!((&after["max_size"], &after["video_bit_rate_mbps"], &after["keep_awake"]), (&json!(0), &json!(null), &json!(true)));
    assert_eq!(after["keyboard_mode"], json!("sdk"));
    // Nothing is kept for a device that is not known.
    assert_eq!(
        h.ipc("set_device_settings", json!({ "device": "nobody", "settings": after })),
        Err(json!({ "code": "unknown_device" }))
    );
}

#[test]
fn a_devices_own_settings_win_over_the_general_ones_for_its_screen_and_its_apps() {
    let scenario = Scenario { scrcpy_version: "v5.0.1", ..Scenario::usb_device() };
    let h = Harness::start("device-over-general", scenario);
    let id = h.wait_for_online_device();
    disable_toolbar(&h);
    set_bit_rate(&h, 8);
    // The general settings: a physical keyboard and 8 Mbps.
    let (_, args) = h.open_and_read_arguments();
    assert!(args.iter().any(|a| a == "--keyboard=uhid"), "{args:?}");
    assert!(args.iter().any(|a| a == "--video-bit-rate=8M"), "{args:?}");
    assert!(!args.iter().any(|a| a.starts_with("--max-size")), "{args:?}");
    h.release_scrcpy(&id);
    std::fs::remove_file(h.install.root.join("scrcpy-args.txt")).unwrap();
    std::fs::remove_file(h.install.root.join("scrcpy-release.txt")).unwrap();

    // This device's own: the standard keyboard, 20 Mbps and a smaller picture.
    change_device_settings(&h, &id, json!({ "keyboard_mode": "sdk", "video_bit_rate_mbps": 20, "max_size": 1280 }));
    let (_, args) = h.open_and_read_arguments();
    assert!(!args.iter().any(|a| a.starts_with("--keyboard")), "{args:?}");
    assert!(args.iter().any(|a| a == "--video-bit-rate=20M"), "{args:?}");
    assert!(args.iter().any(|a| a == "--max-size=1280"), "{args:?}");
    h.release_scrcpy(&id);
    std::fs::remove_file(h.install.root.join("scrcpy-args.txt")).unwrap();

    // The same for an app's window.
    assert_eq!(open_notes(&h, &id), Ok(Value::Null));
    let args = String::from_utf8(h.file_of_scrcpy("scrcpy-args.txt")).unwrap();
    assert!(args.lines().any(|a| a == "--video-bit-rate=20M" || a == "--max-size=1280"), "{args}");
    assert!(args.lines().any(|a| a == "--max-size=1280"), "{args}");
    assert!(!args.lines().any(|a| a.starts_with("--keyboard")), "{args}");
}

#[test]
fn an_app_opens_in_a_display_of_its_own_and_keeps_the_device_awake_as_the_devices_settings_say() {
    let scenario = Scenario { scrcpy_version: "v5.0.1", ..Scenario::usb_device() };
    let h = Harness::start("device-app-settings", scenario);
    let id = h.wait_for_online_device();
    disable_toolbar(&h);
    let first = open_notes(&h, &id);
    assert_eq!(first, Ok(Value::Null));
    let args = String::from_utf8(h.file_of_scrcpy("scrcpy-args.txt")).unwrap();
    // By default: a display of its own, and not kept awake.
    assert!(args.lines().any(|a| a.starts_with("--new-display")), "{args}");
    assert!(!args.lines().any(|a| a == "--stay-awake"), "{args}");
    let key = format!("{id}#app:com.example.notes");
    h.release_scrcpy(&key);
    std::fs::remove_file(h.install.root.join("scrcpy-args.txt")).unwrap();
    std::fs::remove_file(h.install.root.join("scrcpy-release.txt")).unwrap();

    change_device_settings(&h, &id, json!({ "apps_in_virtual_display": false, "keep_awake": true }));
    assert_eq!(open_notes(&h, &id), Ok(Value::Null));
    let args = String::from_utf8(h.file_of_scrcpy("scrcpy-args.txt")).unwrap();
    assert!(!args.lines().any(|a| a.starts_with("--new-display")), "{args}");
    assert!(args.lines().any(|a| a == "--stay-awake"), "{args}");
    h.release_scrcpy(&key);
}

#[test]
fn a_keyboard_mode_a_scrcpy_version_was_not_checked_for_is_not_guessed_and_the_user_is_told() {
    // The default "installed" version has no adapter.
    let h = Harness::start("keyboard-unchecked", Scenario::usb_device());
    h.wait_for_online_device();
    set_keyboard(&h, "uhid");
    let (_, args) = h.open_and_read_arguments();
    assert!(!args.iter().any(|a| a.starts_with("--keyboard")), "{args:?}");
    wait_until("the notice", || {
        h.events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::Notice(n) if n.code == NoticeCode::OptionsUnavailable))
    });
}

fn open_notes(h: &Harness, id: &str) -> Result<Value, Value> {
    h.ipc("open_app", json!({ "device": id, "package": "com.example.notes", "name": "Notes" }))
}

/// What the device's own settings are now, as the UI would read them.
fn device_settings(h: &Harness, id: &str) -> Value {
    h.ipc("device_settings", json!({ "device": id })).unwrap()
}

/// Changes some of the device's own settings and writes them.
fn change_device_settings(h: &Harness, id: &str, changes: Value) {
    let mut settings = device_settings(h, id);
    for (key, value) in changes.as_object().unwrap() {
        settings[key] = value.clone();
    }
    assert_eq!(h.ipc("set_device_settings", json!({ "device": id, "settings": settings })), Ok(Value::Null));
}

fn disable_toolbar(h: &Harness) {
    let mut settings = h.ipc("get_snapshot", Value::Null).unwrap()["settings"].clone();
    settings["toolbar"]["enabled"] = json!(false);
    assert_eq!(h.ipc("save_settings", json!({ "settings": settings })), Ok(Value::Null));
}

#[test]
fn the_window_of_an_app_goes_through_the_proxy_for_its_toolbar_and_the_buttons_act_on_that_window() {
    // The standard mode: the proxy is there because the toolbar of the window needs it.
    let scenario = Scenario { scrcpy_mode: ScrcpyMode::Official, ..Scenario::proxy(video_socket(b"h264", &ONE_GROUP)) };
    let h = Harness::start("app-toolbar", scenario);
    let id = h.wait_for_online_device();
    assert_eq!(open_notes(&h, &id), Ok(Value::Null));
    let args = String::from_utf8(h.file_of_scrcpy("scrcpy-args.txt")).unwrap();
    assert!(args.lines().any(|a| a == "--force-adb-forward"), "{args}");
    let key = format!("{id}#app:com.example.notes");
    wait_until("the client's message", || control_bytes(&h) == [7]);

    // Its buttons are those of the window, not of the device's own screen.
    let buttons = h.ipc("toolbar_buttons", json!({ "device": key })).unwrap();
    assert_eq!(buttons[0], json!({ "id": "back", "label": null, "enabled": true, "unavailable": null }));
    assert_eq!(buttons[6], json!({ "id": "rotate", "label": null, "enabled": true, "unavailable": null }));
    assert_eq!(h.ipc("press_button", json!({ "device": key, "button": "back" })), Ok(Value::Null));
    wait_until("the key", || control_bytes(&h).len() == 1 + 14 + 14);
    assert_eq!(control_bytes(&h), [vec![7], hex(BACK_KEY_DOWN), hex(BACK_KEY_UP)].concat());
    // Nothing went to the device's own screen through adb.
    assert_eq!(h.device_commands(), Vec::<(String, String)>::new());
    h.release_scrcpy(&key);
}

#[test]
fn the_window_of_an_app_has_no_proxy_when_there_is_no_toolbar_and_the_standard_mode_is_on() {
    let scenario = Scenario { scrcpy_mode: ScrcpyMode::Official, ..Scenario::proxy(video_socket(b"h264", &ONE_GROUP)) };
    let h = Harness::start("app-no-toolbar", scenario);
    let id = h.wait_for_online_device();
    disable_toolbar(&h);
    assert_eq!(open_notes(&h, &id), Ok(Value::Null));
    let args = String::from_utf8(h.file_of_scrcpy("scrcpy-args.txt")).unwrap();
    assert!(!args.lines().any(|a| a == "--force-adb-forward"), "{args}");
    h.release_scrcpy(&format!("{id}#app:com.example.notes"));
}

#[test]
fn a_button_of_an_apps_window_without_a_control_channel_is_refused_and_never_sent_to_the_device_screen() {
    // No proxy (no toolbar, standard mode): adb's `input` would act on the
    // device's own screen, not on the window of the app.
    let scenario = Scenario { scrcpy_version: "v5.0.1", ..Scenario::usb_device() };
    let h = Harness::start("app-button-refused", scenario);
    let id = h.wait_for_online_device();
    disable_toolbar(&h);
    assert_eq!(open_notes(&h, &id), Ok(Value::Null));
    let key = format!("{id}#app:com.example.notes");
    let buttons = h.ipc("toolbar_buttons", json!({ "device": key })).unwrap();
    for (position, name) in ["back", "home", "app_switch", "power", "volume_up", "volume_down", "rotate", "notifications"].iter().enumerate() {
        assert_eq!(buttons[position], json!({ "id": name, "label": null, "enabled": false, "unavailable": "needs_proxy" }), "{name}");
    }
    // What has no action at all is still "not yet", not "needs a proxy".
    assert_eq!(buttons[8]["unavailable"], json!("not_yet"));
    assert_eq!(h.ipc("press_button", json!({ "device": key, "button": "back" })), Err(json!({ "code": "no_proxy" })));
    assert_eq!(h.device_commands(), Vec::<(String, String)>::new());
    // The device's own screen is not affected: its buttons still work through adb.
    assert_eq!(h.ipc("press_button", json!({ "device": id, "button": "back" })), Ok(Value::Null));
    assert_eq!(h.device_commands(), [command("TESTDEV0001", "input keyevent 4")]);
    // A key of a device that is not there is refused as such.
    assert_eq!(
        h.ipc("press_button", json!({ "device": "nobody#app:com.example.notes", "button": "back" })),
        Err(json!({ "code": "unknown_device" }))
    );
    h.release_scrcpy(&key);
}

#[test]
fn only_one_app_window_of_a_device_keeps_it_awake_and_the_next_one_can_once_it_is_closed() {
    let scenario = Scenario { scrcpy_version: "v5.0.1", ..Scenario::usb_device() };
    let h = Harness::start("app-awake", scenario);
    let id = h.wait_for_online_device();
    disable_toolbar(&h);
    let open = |package: &str, keep_awake: bool| {
        change_device_settings(&h, &id, json!({ "keep_awake": keep_awake, "apps_in_virtual_display": false }));
        h.ipc("open_app", json!({ "device": id, "package": package, "name": "x" }))
    };
    let awake_flag = "--screen-off-timeout=3600";
    // The arguments of the scrcpy that was started for this package.
    let args_for = |package: &str| -> Vec<String> {
        let wanted = format!("--start-app={package}");
        let path = h.install.root.join("scrcpy-args.txt");
        let read = || -> Vec<String> {
            std::fs::read_to_string(&path).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default()
        };
        wait_until("the arguments of this window", || read().contains(&wanted));
        // Written in one go, but give it a moment to be complete.
        std::thread::sleep(Duration::from_millis(100));
        read()
    };

    assert_eq!(open("com.example.one", true), Ok(Value::Null));
    assert!(args_for("com.example.one").iter().any(|a| a == awake_flag));
    // A second window that also asks for it does not get the flags.
    std::fs::remove_file(h.install.root.join("scrcpy-args.txt")).unwrap();
    assert_eq!(open("com.example.two", true), Ok(Value::Null));
    assert!(!args_for("com.example.two").iter().any(|a| a == awake_flag));
    // One that does not ask never gets them.
    std::fs::remove_file(h.install.root.join("scrcpy-args.txt")).unwrap();
    assert_eq!(open("com.example.three", false), Ok(Value::Null));
    assert!(!args_for("com.example.three").iter().any(|a| a == awake_flag));

    // Once all of them are closed, the next one that asks gets the flags again.
    for package in ["one", "two", "three"] {
        let key = format!("{id}#app:com.example.{package}");
        h.release_scrcpy(&key);
    }
    std::fs::remove_file(h.install.root.join("scrcpy-args.txt")).unwrap();
    std::fs::remove_file(h.install.root.join("scrcpy-release.txt")).unwrap();
    assert_eq!(open("com.example.four", true), Ok(Value::Null));
    assert!(args_for("com.example.four").iter().any(|a| a == awake_flag));
    h.release_scrcpy(&format!("{id}#app:com.example.four"));
}

#[test]
fn a_video_stream_that_is_not_understood_is_logged_and_scrcpy_still_gets_all_of_it() {
    // A codec the adapter has not been checked against.
    let video = video_socket(b"h265", &ONE_GROUP);
    let h = Harness::start("proxy-bad-stream", Scenario::proxy(video.clone()));
    let (id, _) = h.open_and_read_arguments();
    assert_eq!(h.file_of_scrcpy("scrcpy-received.bin"), video);
    wait_until("the stream error to be logged", || {
        level_and_code(&open_console(&h)).contains(&pair("warn", "proxy_stream_error"))
    });
    // Nothing of the packets was taken apart.
    let initial = h.core.sessions().subscribe_video(&id).unwrap().initial;
    assert!(!initial.iter().any(|e| matches!(e, VideoEvent::Packet(_))), "{initial:?}");
    h.release_scrcpy(&id);
}

// ------------------------------------------------------------- the debug console

/// Opens the console the way the UI does and returns the lines it is given.
fn open_console(h: &Harness) -> Vec<Value> {
    h.ipc("open_debug_console", Value::Null).expect("open the console").as_array().expect("a list").clone()
}

/// (level, code) of each line.
fn level_and_code(lines: &[Value]) -> Vec<(String, String)> {
    lines
        .iter()
        .map(|l| (l["level"].as_str().unwrap().to_string(), l["code"].as_str().unwrap().to_string()))
        .collect()
}

/// How many `log` events the application has sent so far.
fn log_events(h: &Harness) -> Vec<scrcpy_agent_lib::core::log::LogLine> {
    h.events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            Event::Log(line) => Some(line.clone()),
            _ => None,
        })
        .collect()
}

fn pair(level: &str, code: &str) -> (String, String) {
    (level.to_string(), code.to_string())
}

#[test]
fn the_console_hands_over_the_log_so_far_and_then_streams_new_lines_until_it_is_closed() {
    let h = Harness::start("console", Scenario::usb_device());
    let id = h.wait_for_online_device();
    assert_eq!(h.ipc("press_button", json!({ "device": id, "button": "back" })), Ok(Value::Null));

    // Nothing is sent while the console is closed, but the log is kept.
    assert!(log_events(&h).is_empty());
    let history = open_console(&h);
    assert!(level_and_code(&history).contains(&pair("debug", "button_pressed")), "{history:?}");
    // The lines have the shape the UI reads.
    assert!(history[0]["seq"].is_u64() && history[0]["at_ms"].is_u64() && history[0]["message"].is_string());

    // From now on new lines arrive as events.
    assert_eq!(h.ipc("press_button", json!({ "device": id, "button": "home" })), Ok(Value::Null));
    wait_until("a log event", || log_events(&h).iter().any(|l| format!("{:?}", l.code) == "ButtonPressed"));

    assert_eq!(h.ipc("close_debug_console", Value::Null), Ok(Value::Null));
    let sent = log_events(&h).len();
    assert_eq!(h.ipc("press_button", json!({ "device": id, "button": "power" })), Ok(Value::Null));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(log_events(&h).len(), sent, "a line was sent to a closed console");

    // The lines written while it was closed are in the log when it is opened again.
    let again = level_and_code(&open_console(&h));
    assert_eq!(again.iter().filter(|p| **p == pair("debug", "button_pressed")).count(), 3, "{again:?}");
}

/// The windows of the application whose label is `console`.
fn console_windows(h: &Harness) -> usize {
    use tauri::Manager;
    h._app.webview_windows().keys().filter(|label| label.as_str() == "console").count()
}

#[test]
fn the_console_opens_in_a_window_of_its_own_and_asking_again_does_not_open_a_second() {
    let h = Harness::start("console-window", Scenario::usb_device());
    h.wait_until_ready();
    assert_eq!(console_windows(&h), 0);
    assert_eq!(h.ipc("open_console_window", Value::Null), Ok(Value::Null));
    assert_eq!(console_windows(&h), 1);
    // Asked again (the settings button pressed twice): the same window.
    assert_eq!(h.ipc("open_console_window", Value::Null), Ok(Value::Null));
    assert_eq!(console_windows(&h), 1);
}

// (What closing a window does is decided by `after_destroyed` in lib.rs, which has
// its own unit tests: Tauri's test runtime never reports a window as destroyed.)

#[test]
fn a_button_that_fails_is_a_warning_in_the_log() {
    let h = Harness::start(
        "console-button",
        Scenario { failing_commands: &[("input keyevent 4", 1)], ..Scenario::usb_device() },
    );
    let id = h.wait_for_online_device();
    assert!(h.ipc("press_button", json!({ "device": id, "button": "back" })).is_err());
    assert!(level_and_code(&open_console(&h)).contains(&pair("warn", "button_failed")));
}

#[test]
fn saving_the_settings_is_logged() {
    let h = Harness::start("console-settings", Scenario::usb_device());
    h.wait_until_ready();
    let settings = h.ipc("get_snapshot", Value::Null).unwrap()["settings"].clone();
    assert_eq!(h.ipc("save_settings", json!({ "settings": settings })), Ok(Value::Null));
    assert!(level_and_code(&open_console(&h)).contains(&pair("info", "settings_saved")));
}

#[test]
fn connecting_by_address_is_logged_with_its_outcome() {
    let h = Harness::start("console-connect", Scenario::usb_device());
    h.wait_until_ready();
    // Nothing listens on port 1 here, so the connection cannot be made.
    assert_eq!(
        h.ipc("connect_device", json!({ "address": "127.0.0.1:1" })),
        Err(json!({ "code": "unreachable" }))
    );
    let lines = level_and_code(&open_console(&h));
    let wanted = [pair("info", "connect_attempt"), pair("warn", "connect_outcome")];
    let mine: Vec<_> = lines.iter().filter(|p| wanted.contains(p)).cloned().collect();
    assert_eq!(mine, wanted);
}

#[test]
fn a_device_that_is_being_connected_to_is_not_connected_to_again_by_hand() {
    let h = Harness::start("connect-busy", Scenario::usb_device());
    h.wait_until_ready();
    let address: SocketAddr = "127.0.0.1:1".parse().unwrap();
    // Another connection to this device is going on (whichever port it is at).
    let held = h.core.devices().connect_gate().try_begin("127.0.0.1:2".parse().unwrap()).expect("a permit");
    assert_eq!(
        h.ipc("connect_device", json!({ "address": address.to_string() })),
        Err(json!({ "code": "connect_in_progress" }))
    );
    // Nothing was tried, so nothing was logged as an attempt.
    let attempts = level_and_code(&open_console(&h)).iter().filter(|p| **p == pair("info", "connect_attempt")).count();
    assert_eq!(attempts, 0);
    // When the other connection has ended the device can be connected to again.
    drop(held);
    assert_eq!(
        h.ipc("connect_device", json!({ "address": address.to_string() })),
        Err(json!({ "code": "unreachable" }))
    );
}

#[test]
fn pairing_is_logged_but_never_with_its_code() {
    let h = Harness::start("console-pair", Scenario::usb_device());
    h.wait_until_ready();
    // No device shows up afterwards, so this ends in "could not be confirmed".
    assert!(h.ipc("pair_device", json!({ "address": "127.0.0.1:1", "code": "654321" })).is_err());
    let lines = open_console(&h);
    let summary = level_and_code(&lines);
    assert!(summary.contains(&pair("info", "pair_attempt")), "{summary:?}");
    assert!(summary.contains(&pair("warn", "pair_outcome")), "{summary:?}");
    for line in &lines {
        assert!(!line["message"].as_str().unwrap().contains("654321"), "the pairing code was logged: {line}");
    }
}

#[test]
fn a_screen_session_is_logged_from_its_start_to_its_end() {
    let h = Harness::start("console-session", Scenario::usb_device());
    let id = h.wait_for_online_device();
    assert_eq!(h.ipc("open_device", json!({ "id": id })), Ok(Value::Null));
    wait_until("the session to end", || {
        level_and_code(&open_console(&h)).contains(&pair("info", "session_ended"))
    });
    let lines = level_and_code(&open_console(&h));
    let session: Vec<_> = lines.iter().filter(|p| p.1.starts_with("session_")).cloned().collect();
    assert_eq!(session, [pair("info", "session_started"), pair("info", "session_ended")]);
}

#[test]
fn a_look_at_the_devices_that_cannot_ask_adb_is_logged() {
    let h = Harness::start("console-poll", Scenario::usb_device());
    h.wait_for_online_device();
    h.adb_server.refuse_connections(true);
    wait_until("a failed look to be logged", || {
        level_and_code(&open_console(&h)).contains(&pair("warn", "poll_failed"))
    });
}

#[test]
fn a_button_whose_command_fails_on_the_device_reports_the_exit_code() {
    let h = Harness::start(
        "button-fails",
        Scenario { failing_commands: &[("input keyevent 4", 1)], ..Scenario::usb_device() },
    );
    let id = h.wait_for_online_device();
    assert_eq!(
        h.ipc("press_button", json!({ "device": id, "button": "back" })),
        Err(json!({ "code": "command_failed", "detail": 1 }))
    );
    // Another button on the same device is not affected.
    assert_eq!(h.ipc("press_button", json!({ "device": id, "button": "home" })), Ok(Value::Null));
}

#[test]
fn a_button_press_that_cannot_be_carried_out_says_why_and_runs_nothing() {
    let h = Harness::start("button-errors", Scenario::usb_device());
    let id = h.wait_for_online_device();

    assert_eq!(
        h.ipc("press_button", json!({ "device": "NOBODY", "button": "back" })),
        Err(json!({ "code": "unknown_device" }))
    );
    // Buttons without an action of their own: a built-in one and an add-on's.
    assert_eq!(
        h.ipc("press_button", json!({ "device": id, "button": "screenshot" })),
        Err(json!({ "code": "not_available" }))
    );
    assert_eq!(
        h.ipc("press_button", json!({ "device": id, "button": { "addon": { "addon": "clicker", "id": "tap" } } })),
        Err(json!({ "code": "not_available" }))
    );
    assert_eq!(h.device_commands(), Vec::<(String, String)>::new());
}

#[test]
fn a_button_press_for_a_device_that_is_only_seen_on_the_network_is_refused() {
    let h = Harness::start(
        "button-offline",
        Scenario { devices: NO_DEVICES, mdns: REAL_MDNS, ..Scenario::usb_device() },
    );
    h.wait_for("the tablet to be listed", |s| s.devices.iter().any(|d| d.id == "a1b2c3d4").then_some(()));
    assert_eq!(
        h.ipc("press_button", json!({ "device": "a1b2c3d4", "button": "back" })),
        Err(json!({ "code": "not_online" }))
    );
    assert_eq!(h.device_commands(), Vec::<(String, String)>::new());
}

#[test]
fn strangers_on_the_network_are_listed_but_never_connected() {
    let h = Harness::start(
        "strangers",
        Scenario { devices: NO_DEVICES, mdns: REAL_MDNS, ..Scenario::usb_device() },
    );
    let found = h.wait_for("the devices on the network to be listed", |s| {
        (s.devices.len() == 3).then(|| s.devices.clone())
    });
    assert!(found.iter().all(|d| d.state == DeviceState::Discovered && !d.known));
    let tablet = found.iter().find(|d| d.id == "a1b2c3d4").expect("the tablet");
    assert_eq!(tablet.address.as_deref(), Some("192.168.250.139:36600"));

    // Let several polls pass, then check adb was never asked to connect.
    std::thread::sleep(Duration::from_millis(2500));
    let connects: Vec<_> = h.adb_services().into_iter().filter(|s| s.starts_with("host:connect:")).collect();
    assert!(connects.is_empty(), "connected to a stranger: {connects:?}");
}

#[test]
fn a_known_device_that_reappears_on_the_network_is_connected_again() {
    let h = Harness::start(
        "reconnect",
        Scenario { devices: NO_DEVICES, mdns: REAL_MDNS, known: &["a1b2c3d4"], ..Scenario::usb_device() },
    );
    h.wait_for("the known tablet to be seen", |s| {
        s.devices.iter().any(|d| d.id == "a1b2c3d4" && d.state == DeviceState::Discovered).then_some(())
    });
    // The tablet came back on a new port; adb is asked to connect to that one.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let connected = h.adb_services().iter().any(|s| s == "host:connect:192.168.250.139:36600");
        if connected {
            break;
        }
        assert!(Instant::now() < deadline, "adb was never asked to connect: {:?}", h.adb_services());
        std::thread::sleep(Duration::from_millis(50));
    }
    // ...and only that one, not the two other devices on the network.
    let connects: Vec<_> = h.adb_services().into_iter().filter(|s| s.starts_with("host:connect:")).collect();
    assert!(connects.iter().all(|s| s == "host:connect:192.168.250.139:36600"), "{connects:?}");
}
