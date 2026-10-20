//! Talking to the adb server directly, in its own protocol, instead of running
//! `adb.exe` and reading what it prints.
//!
//! The protocol (checked against a real adb server, version 37.0.1, and
//! against what the official `adb` client sends):
//!
//! * A request is four hexadecimal digits (the length in bytes) followed by
//!   the service name, e.g. `000chost:version`.
//! * The answer starts with `OKAY` or `FAIL`. `FAIL` is followed by a length
//!   (four hex digits) and a message. After `OKAY` comes whatever the service
//!   returns: for the `host:` services that give text, a length-prefixed
//!   block; for `host:transport:<serial>` nothing, the connection then talks
//!   to that device and the next request is a device service such as
//!   `shell,v2:<command>`.
//! * `shell,v2:` answers with packets: one id byte (1 standard output,
//!   2 standard error, 3 exit code), a four-byte little-endian length, and the
//!   data. The exit code is a fact; the old `shell:` service has none.
//!
//! What the messages say (the text of `connect` and `pair` answers, which adb
//! translates into the user's language) is for showing to the user only.
//! Whether a connection or a pairing worked is decided from the device list.

pub use super::api::{AdbApi, AdbError, DeviceTracker, ShellOutput};
use super::{AdbEntry, Forward, MdnsService, parse_devices, parse_forwards, parse_mdns};
use std::future::Future;
use std::net::SocketAddr;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The longest request the four hex digits can express.
pub const MAX_REQUEST: usize = 0xFFFF;

/// Turns a service name into the bytes to send.
pub fn encode_request(service: &str) -> Result<Vec<u8>, AdbError> {
    if service.len() > MAX_REQUEST {
        return Err(AdbError::RequestTooLong(service.len()));
    }
    let mut bytes = format!("{:04x}", service.len()).into_bytes();
    bytes.extend_from_slice(service.as_bytes());
    Ok(bytes)
}

/// Four hexadecimal digits as a number. Only digits count: `u32::from_str_radix`
/// alone would also take a leading `+`.
fn parse_hex4(digits: &[u8; 4]) -> Option<usize> {
    if !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    usize::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()
}

async fn read_array<S: AsyncRead + Unpin, const N: usize>(stream: &mut S) -> Result<[u8; N], AdbError> {
    let mut bytes = [0u8; N];
    stream.read_exact(&mut bytes).await?;
    Ok(bytes)
}

/// Reads `OKAY` (returns `Ok`) or `FAIL` plus its message (returns the error).
pub async fn read_status<S: AsyncRead + Unpin>(stream: &mut S) -> Result<(), AdbError> {
    match &read_array::<_, 4>(stream).await? {
        b"OKAY" => Ok(()),
        b"FAIL" => {
            let message = read_block(stream).await?;
            Err(AdbError::Fail(String::from_utf8_lossy(&message).into_owned()))
        }
        other => Err(AdbError::UnknownStatus(*other)),
    }
}

/// Reads one length-prefixed block.
pub async fn read_block<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Vec<u8>, AdbError> {
    let digits = read_array::<_, 4>(stream).await?;
    let len = parse_hex4(&digits).ok_or(AdbError::BadLength(digits))?;
    let mut block = vec![0u8; len];
    stream.read_exact(&mut block).await?;
    Ok(block)
}

/// Reads `shell,v2:` packets up to the exit packet, which ends the command.
pub async fn read_shell_v2<S: AsyncRead + Unpin>(stream: &mut S) -> Result<ShellOutput, AdbError> {
    const STDOUT: u8 = 1;
    const STDERR: u8 = 2;
    const EXIT: u8 = 3;
    let mut output = ShellOutput::default();
    loop {
        // The stream may only end between packets, and only after the exit packet.
        let mut id = [0u8; 1];
        if stream.read(&mut id).await? == 0 {
            return Err(AdbError::MissingExit);
        }
        let len = u32::from_le_bytes(read_array::<_, 4>(stream).await?) as u64;
        // Read what is announced without reserving it first: the length comes
        // from the other side.
        let mut data = Vec::new();
        let got = (&mut *stream).take(len).read_to_end(&mut data).await?;
        if (got as u64) < len {
            return Err(AdbError::Truncated);
        }
        match id[0] {
            STDOUT => output.stdout.extend(data),
            STDERR => output.stderr.extend(data),
            EXIT => {
                let [code] = data[..] else {
                    return Err(AdbError::BadExitPacket(data.len()));
                };
                output.exit = code;
                return Ok(output);
            }
            other => return Err(AdbError::UnknownShellPacket(other)),
        }
    }
}

/// The mode a pushed file gets: a regular file, owner may write, everybody may
/// read (0100644), as the number the `sync:` service wants.
const PUSH_MODE: u32 = 0o100644;

/// The most bytes of file in one `DATA` message of the `sync:` service.
const SYNC_DATA_MAX: usize = 64 * 1024;

/// A message of the `sync:` service: four letters and a little-endian number
/// (the length of what follows, or for `DONE` the modification time).
fn sync_header(id: &[u8; 4], number: u32) -> [u8; 8] {
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(id);
    bytes[4..].copy_from_slice(&number.to_le_bytes());
    bytes
}

/// Opens a connection to the adb server.
pub trait Connect: Send + Sync + 'static {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send;
    fn open(&self) -> impl Future<Output = std::io::Result<Self::Stream>> + Send;
}

/// The adb server on a TCP address (by default `127.0.0.1:5037`).
pub struct TcpConnect(pub SocketAddr);

impl Connect for TcpConnect {
    type Stream = tokio::net::TcpStream;
    async fn open(&self) -> std::io::Result<Self::Stream> {
        tokio::net::TcpStream::connect(self.0).await
    }
}

/// A client of the adb server. Every call uses its own connection, which is
/// how the adb protocol is meant to be used.
pub struct AdbClient<C: Connect> {
    pub connect: C,
}

impl<C: Connect> AdbClient<C> {
    pub fn new(connect: C) -> Self {
        AdbClient { connect }
    }

    /// A new connection to the server.
    async fn open(&self) -> Result<C::Stream, AdbError> {
        Ok(self.connect.open().await?)
    }

    /// Sends a request and checks the status that follows.
    async fn send<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S, service: &str) -> Result<(), AdbError> {
        stream.write_all(&encode_request(service)?).await?;
        stream.flush().await?;
        read_status(stream).await
    }

    /// A `host:` service whose answer is one block.
    async fn host_block(&self, service: &str) -> Result<Vec<u8>, AdbError> {
        let mut stream = self.open().await?;
        Self::send(&mut stream, service).await?;
        read_block(&mut stream).await
    }

    /// The server's protocol version (`host:version`).
    pub async fn version(&self) -> Result<u32, AdbError> {
        let block = self.host_block("host:version").await?;
        let digits: &[u8; 4] = block.as_slice().try_into().map_err(|_| AdbError::BadVersion(block.clone()))?;
        parse_hex4(digits).map(|v| v as u32).ok_or(AdbError::BadVersion(block))
    }

    /// A `host:` service that answers with a message for the user.
    async fn host_message(&self, service: &str) -> Result<String, AdbError> {
        let block = self.host_block(service).await?;
        Ok(String::from_utf8_lossy(&block).into_owned())
    }
}

/// A `host:track-devices-l` connection.
pub struct Tracker<S> {
    stream: S,
}

impl<S: AsyncRead + Unpin + Send> DeviceTracker for Tracker<S> {
    async fn next(&mut self) -> Result<Vec<AdbEntry>, AdbError> {
        // Between two pushes the connection may end: that is the server going
        // away. Inside a push it is a broken answer.
        let mut first = [0u8; 1];
        if self.stream.read(&mut first).await? == 0 {
            return Err(AdbError::Closed);
        }
        let mut rest = [0u8; 3];
        self.stream.read_exact(&mut rest).await?;
        let digits = [first[0], rest[0], rest[1], rest[2]];
        let len = parse_hex4(&digits).ok_or(AdbError::BadLength(digits))?;
        let mut block = vec![0u8; len];
        self.stream.read_exact(&mut block).await?;
        Ok(parse_devices(&String::from_utf8_lossy(&block)))
    }
}

/// The first way to get adb's answers: the adb server's own protocol.
impl<C: Connect> AdbApi for AdbClient<C> {
    type Tracker = Tracker<C::Stream>;

    async fn track_devices(&self) -> Result<Self::Tracker, AdbError> {
        let mut stream = self.open().await?;
        Self::send(&mut stream, "host:track-devices-l").await?;
        Ok(Tracker { stream })
    }

    async fn devices(&self) -> Result<Vec<AdbEntry>, AdbError> {
        let block = self.host_block("host:devices-l").await?;
        Ok(parse_devices(&String::from_utf8_lossy(&block)))
    }

    async fn forwards(&self) -> Result<Vec<Forward>, AdbError> {
        let block = self.host_block("host:list-forward").await?;
        Ok(parse_forwards(&String::from_utf8_lossy(&block)))
    }

    async fn mdns_services(&self) -> Result<Vec<MdnsService>, AdbError> {
        let block = self.host_block("host:mdns:services").await?;
        Ok(parse_mdns(&String::from_utf8_lossy(&block)))
    }

    async fn shell(&self, serial: &str, command: &str) -> Result<ShellOutput, AdbError> {
        let mut stream = self.open().await?;
        Self::send(&mut stream, &format!("host:transport:{serial}")).await?;
        Self::send(&mut stream, &format!("shell,v2:{command}")).await?;
        read_shell_v2(&mut stream).await
    }

    async fn push(
        &self,
        serial: &str,
        local: &std::path::Path,
        remote: &str,
        progress: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<(), AdbError> {
        use tokio::io::AsyncReadExt as _;
        if remote.is_empty() || remote.contains(['\0', '\n']) {
            return Err(AdbError::BadRemotePath);
        }
        // The file is looked at before the device is asked for anything.
        let mut file = tokio::fs::File::open(local).await?;
        let modified = file
            .metadata()
            .await?
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| u32::try_from(d.as_secs()).unwrap_or(u32::MAX));

        let mut stream = self.open().await?;
        Self::send(&mut stream, &format!("host:transport:{serial}")).await?;
        Self::send(&mut stream, "sync:").await?;
        let spec = format!("{remote},{PUSH_MODE}");
        stream.write_all(&sync_header(b"SEND", spec.len() as u32)).await?;
        stream.write_all(spec.as_bytes()).await?;

        let mut buffer = vec![0u8; SYNC_DATA_MAX];
        let mut sent = 0u64;
        loop {
            // A piece is full unless the file ends in it.
            let mut filled = 0;
            while filled < buffer.len() {
                let n = file.read(&mut buffer[filled..]).await?;
                if n == 0 {
                    break;
                }
                filled += n;
            }
            if filled == 0 {
                break;
            }
            stream.write_all(&sync_header(b"DATA", filled as u32)).await?;
            stream.write_all(&buffer[..filled]).await?;
            sent += filled as u64;
            progress(sent);
        }
        stream.write_all(&sync_header(b"DONE", modified)).await?;
        stream.flush().await?;

        // The device's verdict: the id, then a number that is 0 for `OKAY` and
        // the length of the message for `FAIL`.
        let id = read_array::<_, 4>(&mut stream).await?;
        let number = u32::from_le_bytes(read_array::<_, 4>(&mut stream).await?);
        let result = match &id {
            b"OKAY" => Ok(()),
            b"FAIL" => {
                let mut message = Vec::new();
                (&mut stream).take(u64::from(number)).read_to_end(&mut message).await?;
                Err(AdbError::Fail(String::from_utf8_lossy(&message).into_owned()))
            }
            other => Err(AdbError::UnknownStatus(*other)),
        };
        // Say goodbye; the verdict is already known.
        let _ = stream.write_all(&sync_header(b"QUIT", 0)).await;
        result
    }

    async fn connect(&self, address: SocketAddr) -> Result<String, AdbError> {
        self.host_message(&format!("host:connect:{address}")).await
    }

    async fn disconnect(&self, address: SocketAddr) -> Result<String, AdbError> {
        self.host_message(&format!("host:disconnect:{address}")).await
    }

    async fn pair(&self, address: SocketAddr, code: &str) -> Result<String, AdbError> {
        self.host_message(&format!("host:pair:{code}:{address}")).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::adb::{AdbState, MdnsKind, SerialKind};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

    // Bytes in the form a real adb server (37.0.1) sends, checked against one.
    // Every value in them (serials, models, addresses, numbers) is invented; the
    // length prefixes are those of the invented text. Hex is used where the
    // bytes are not text.

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
    }

    /// What the server answers to `host:devices-l` with one wireless device.
    const DEVICES_L: &[u8] = b"OKAY0072adb-a1b2c3d4-Ab12Cd._adb-tls-connect._tcp. device product:fake_global model:MODEL0001 device:fake transport_id:16\n";
    const SERIAL: &str = "adb-a1b2c3d4-Ab12Cd._adb-tls-connect._tcp.";

    /// What adb says when `connect` does not work: free text in the language of the user's
    /// computer, which is handed on as it is. Invented sentences in three languages (the
    /// third, Traditional Chinese, is written in escapes so that no source line holds it).
    const CONNECT_SAID_IN_THREE_LANGUAGES: [&str; 3] = [
        "cannot connect to 127.0.0.1:1: Verbindung nicht möglich, da der Zielcomputer sie verweigerte. (10061)",
        "cannot connect to 127.0.0.1:1: Не удалось подключиться: компьютер отверг запрос. (10061)",
        "cannot connect to 127.0.0.1:1: \u{7121}\u{6cd5}\u{9023}\u{7dda}\u{3002} (10061)",
    ];

    /// An answer of the server: `OKAY`, the length of the text in four hexadecimal digits, the text.
    fn okay_with(text: &str) -> Vec<u8> {
        [b"OKAY".as_slice(), format!("{:04x}", text.len()).as_bytes(), text.as_bytes()].concat()
    }

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    // framing --------------------------------------------------------------------

    #[test]
    fn a_request_is_its_length_in_four_hex_digits_and_then_the_service() {
        // The two requests used for the captures above.
        assert_eq!(encode_request("host:version").unwrap(), b"000chost:version");
        assert_eq!(encode_request("host:devices-l").unwrap(), b"000ehost:devices-l");
    }

    #[test]
    fn the_length_counts_bytes_not_characters() {
        // "€€" is two characters and six bytes: 11 + 6 = 17 = 0x11.
        assert_eq!(encode_request("shell:echo €€").unwrap(), "0011shell:echo €€".as_bytes());
    }

    #[test]
    fn a_request_longer_than_four_hex_digits_can_say_is_refused() {
        let longest = "a".repeat(MAX_REQUEST);
        assert_eq!(&encode_request(&longest).unwrap()[..4], b"ffff");
        let too_long = "a".repeat(MAX_REQUEST + 1);
        assert_eq!(encode_request(&too_long), Err(AdbError::RequestTooLong(MAX_REQUEST + 1)));
    }

    async fn status_of(bytes: &[u8]) -> Result<(), AdbError> {
        let mut input = bytes;
        read_status(&mut input).await
    }

    #[tokio::test]
    async fn okay_is_success_and_fail_carries_the_servers_message() {
        assert_eq!(status_of(b"OKAY").await, Ok(()));
        // Both in the form a real server answers.
        assert_eq!(
            status_of(b"FAIL0014unknown host service").await,
            Err(AdbError::Fail("unknown host service".into()))
        );
        assert_eq!(
            status_of(b"FAIL0021device 'no-such-serial' not found").await,
            Err(AdbError::Fail("device 'no-such-serial' not found".into()))
        );
    }

    #[tokio::test]
    async fn an_answer_that_is_not_in_the_protocol_is_reported_as_such() {
        assert_eq!(status_of(b"WAT?").await, Err(AdbError::UnknownStatus(*b"WAT?")));
        assert_eq!(status_of(b"FAILzzzz").await, Err(AdbError::BadLength(*b"zzzz")));
        // The connection ends inside the status, inside the length, inside the message.
        assert_eq!(status_of(b"OK").await, Err(AdbError::Truncated));
        assert_eq!(status_of(b"FAIL00").await, Err(AdbError::Truncated));
        assert_eq!(status_of(b"FAIL0014unknown").await, Err(AdbError::Truncated));
        assert_eq!(status_of(b"").await, Err(AdbError::Truncated));
    }

    #[tokio::test]
    async fn a_block_is_its_length_and_that_many_bytes() {
        let mut version: &[u8] = b"00040029";
        assert_eq!(read_block(&mut version).await, Ok(b"0029".to_vec()));
        // An empty list of services is a block of length zero.
        let mut empty: &[u8] = b"0000";
        assert_eq!(read_block(&mut empty).await, Ok(Vec::new()));
        // Only the announced length is taken; the rest stays unread.
        let mut two: &[u8] = b"0002abcd";
        assert_eq!(read_block(&mut two).await, Ok(b"ab".to_vec()));
        assert_eq!(two, b"cd");
        let mut short: &[u8] = b"0005ab";
        assert_eq!(read_block(&mut short).await, Err(AdbError::Truncated));
        let mut bad: &[u8] = b"00g1x";
        assert_eq!(read_block(&mut bad).await, Err(AdbError::BadLength(*b"00g1")));
        // A sign is not a digit, even though Rust's number parser takes one.
        let mut signed: &[u8] = b"+004abcd";
        assert_eq!(read_block(&mut signed).await, Err(AdbError::BadLength(*b"+004")));
    }

    async fn shell_of(bytes: &[u8]) -> Result<ShellOutput, AdbError> {
        let mut input = bytes;
        read_shell_v2(&mut input).await
    }

    #[tokio::test]
    async fn shell_packets_are_sorted_into_output_error_and_exit_code() {
        // `getprop ro.build.version.sdk`: stdout "30\n", exit 0 (the packets in
        // the form the server sends them; the number is invented).
        let out = shell_of(&hex("010300000033300a030100000000")).await.unwrap();
        assert_eq!(out, ShellOutput { stdout: b"30\n".to_vec(), stderr: Vec::new(), exit: 0 });
        // `exit 3`: no output, exit 3.
        let out = shell_of(&hex("030100000003")).await.unwrap();
        assert_eq!(out, ShellOutput { stdout: Vec::new(), stderr: Vec::new(), exit: 3 });
        // `echo out; echo err 1>&2`.
        let out = shell_of(&hex("01040000006f75740a02040000006572720a030100000000")).await.unwrap();
        assert_eq!(out, ShellOutput { stdout: b"out\n".to_vec(), stderr: b"err\n".to_vec(), exit: 0 });
    }

    #[tokio::test]
    async fn broken_shell_streams_are_errors_not_guesses() {
        // Output but no exit packet: the command may not have finished.
        assert_eq!(shell_of(&hex("010300000033300a")).await, Err(AdbError::MissingExit));
        assert_eq!(shell_of(b"").await, Err(AdbError::MissingExit));
        // A packet id adb does not define.
        assert_eq!(shell_of(&hex("090100000000")).await, Err(AdbError::UnknownShellPacket(9)));
        // An exit packet must carry exactly one byte.
        assert_eq!(shell_of(&hex("03020000000001")).await, Err(AdbError::BadExitPacket(2)));
        // The stream ends in the middle of a packet.
        assert_eq!(shell_of(&hex("0105000000aabb")).await, Err(AdbError::Truncated));
        assert_eq!(shell_of(&hex("0105")).await, Err(AdbError::Truncated));
        // A packet that announces 4 GiB but has one byte: an error, not an allocation.
        assert_eq!(shell_of(&hex("01ffffffffaa")).await, Err(AdbError::Truncated));
    }

    // against the real adb server ------------------------------------------------

    /// Run with `cargo test --lib live_adb_server -- --ignored --nocapture`
    /// while an adb server is running. The server is `127.0.0.1:5037` unless
    /// `SCRCPY_AGENT_LIVE_ADB` names another address. The shell part runs on
    /// the device named by `SCRCPY_AGENT_LIVE_SERIAL`, or else on any online
    /// device; without one it is skipped. Nothing about the devices is printed.
    #[tokio::test]
    #[ignore = "needs a running adb server"]
    async fn live_adb_server() {
        let server = std::env::var("SCRCPY_AGENT_LIVE_ADB").unwrap_or_else(|_| "127.0.0.1:5037".into());
        let client = AdbClient::new(TcpConnect(addr(&server)));
        let version = client.version().await.expect("host:version");
        println!("adb server protocol version: {version}");
        assert!(version > 0);
        let devices = client.devices().await.expect("host:devices-l");
        println!("devices listed: {}", devices.len());
        // The first push of a tracking connection is the current list.
        let mut tracker = client.track_devices().await.expect("host:track-devices-l");
        let pushed = tokio::time::timeout(std::time::Duration::from_secs(5), tracker.next())
            .await
            .expect("the first push comes at once")
            .expect("a push");
        println!("first push: {} device(s)", pushed.len());
        let wanted = std::env::var("SCRCPY_AGENT_LIVE_SERIAL").ok();
        let online = devices
            .iter()
            .filter(|d| d.state == AdbState::Device)
            .find(|d| wanted.as_ref().is_none_or(|w| *w == d.serial));
        let Some(online) = online else {
            println!("no online device: the shell part was not exercised");
            return;
        };
        let out = client.shell(&online.serial, "getprop ro.build.version.sdk").await.expect("shell,v2");
        assert_eq!(out.exit, 0);
        assert!(out.stderr.is_empty());
        assert!(String::from_utf8(out.stdout).unwrap().trim().parse::<u32>().is_ok());
        let failing = client.shell(&online.serial, "exit 3").await.expect("shell,v2");
        assert_eq!(failing.exit, 3);

        // A push of more than two pieces, and of a text, read back from the device.
        let remote_big = format!("/sdcard/Download/scrcpy-agent-live-{}-big.bin", std::process::id());
        let remote_text = format!("/sdcard/Download/scrcpy-agent-live-{}-text.txt", std::process::id());
        let (big, _) = temp_file("live-big", 200_000);
        let (text, _) = temp_file("live-text", 0);
        std::fs::write(&text, "one line\nsecond line\n").unwrap();
        let reported = Mutex::new(Vec::new());
        let progress = |sent: u64| reported.lock().unwrap().push(sent);
        client.push(&online.serial, &big, &remote_big, &progress).await.expect("push");
        client.push(&online.serial, &text, &remote_text, &|_| {}).await.expect("push");
        assert_eq!(reported.lock().unwrap().last(), Some(&200_000));
        let size = client.shell(&online.serial, &format!("wc -c < {remote_big}")).await.expect("shell,v2");
        assert_eq!(String::from_utf8(size.stdout).unwrap().trim(), "200000");
        let read_back = client.shell(&online.serial, &format!("cat {remote_text}")).await.expect("shell,v2");
        assert_eq!(read_back.stdout, b"one line\nsecond line\n");
        client.shell(&online.serial, &format!("rm {remote_big} {remote_text}")).await.expect("shell,v2");
        let _ = std::fs::remove_file(big);
        let _ = std::fs::remove_file(text);
        println!("push: 2 files sent and read back");
    }

    // the client, against a scripted server ----------------------------------------

    /// One connection: the request expected, and the bytes to answer with.
    type Script = Vec<(&'static [u8], Vec<u8>)>;

    /// Connects to a scripted server: each connection expects the requests of
    /// one script and answers with the bytes given after each. The requests
    /// the server really received are kept.
    struct Scripted {
        connections: Mutex<Vec<Script>>,
        received: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl Scripted {
        fn new(connections: Vec<Script>) -> (Self, Arc<Mutex<Vec<Vec<u8>>>>) {
            let received = Arc::new(Mutex::new(Vec::new()));
            (Scripted { connections: Mutex::new(connections), received: received.clone() }, received)
        }
    }

    impl Connect for Scripted {
        type Stream = tokio::io::DuplexStream;
        async fn open(&self) -> std::io::Result<Self::Stream> {
            let script = {
                let mut all = self.connections.lock().unwrap();
                assert!(!all.is_empty(), "the client opened more connections than the test expects");
                all.remove(0)
            };
            let (client, mut server) = duplex(64 * 1024);
            let received = self.received.clone();
            tokio::spawn(async move {
                for (expected, answer) in script {
                    // The request is length-prefixed, so it can be read exactly.
                    let mut head = [0u8; 4];
                    if server.read_exact(&mut head).await.is_err() {
                        return;
                    }
                    let len = usize::from_str_radix(std::str::from_utf8(&head).unwrap(), 16).unwrap();
                    let mut body = vec![0u8; len];
                    if server.read_exact(&mut body).await.is_err() {
                        return;
                    }
                    let mut request = head.to_vec();
                    request.extend(body);
                    assert_eq!(request, expected, "the request the client sent");
                    received.lock().unwrap().push(request);
                    let _ = server.write_all(&answer).await;
                }
                // Dropping the server end closes the stream: the end of a shell command.
            });
            Ok(client)
        }
    }

    #[tokio::test]
    async fn the_version_is_the_hex_number_in_the_answer() {
        let (script, received) = Scripted::new(vec![vec![(b"000chost:version", b"OKAY00040029".to_vec())]]);
        assert_eq!(AdbClient::new(script).version().await, Ok(0x29));
        assert_eq!(received.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_device_list_is_the_parsed_answer_to_devices_l() {
        let (script, _) = Scripted::new(vec![vec![(b"000ehost:devices-l", DEVICES_L.to_vec())]]);
        let devices = AdbClient::new(script).devices().await.unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].serial, SERIAL);
        assert_eq!(devices[0].kind, SerialKind::MdnsName { instance: "adb-a1b2c3d4-Ab12Cd".into() });
        assert_eq!(devices[0].state, AdbState::Device);
        assert_eq!(devices[0].model.as_deref(), Some("MODEL0001"));
        assert!(!devices[0].is_usb);
    }

    #[tokio::test]
    async fn the_forwards_are_the_parsed_lines_of_the_answer_to_list_forward() {
        use crate::core::adb::ForwardRemote;
        // Real: with none set up the answer is an empty block.
        let (script, _) = Scripted::new(vec![vec![(b"0011host:list-forward", b"OKAY0000".to_vec())]]);
        assert_eq!(AdbClient::new(script).forwards().await, Ok(Vec::new()));

        let text = "SERIAL-A tcp:27183 localabstract:scrcpy_1a2b3c4d\n";
        let answer = [b"OKAY".to_vec(), format!("{:04x}{text}", text.len()).into_bytes()].concat();
        let (script, _) = Scripted::new(vec![vec![(b"0011host:list-forward", answer)]]);
        assert_eq!(
            AdbClient::new(script).forwards().await,
            Ok(vec![Forward {
                serial: "SERIAL-A".into(),
                local_port: 27183,
                remote: ForwardRemote::LocalAbstract("scrcpy_1a2b3c4d".into())
            }])
        );
    }

    #[tokio::test]
    async fn no_mdns_services_is_an_empty_list() {
        // A server that has discovered nothing answers with an empty block.
        let (script, _) = Scripted::new(vec![vec![(b"0012host:mdns:services", b"OKAY0000".to_vec())]]);
        assert_eq!(AdbClient::new(script).mdns_services().await, Ok(Vec::new()));
    }

    #[tokio::test]
    async fn the_mdns_services_are_the_parsed_lines_of_the_answer() {
        // The form of a real server's answer: instance, type and address, tab
        // separated (the values are invented).
        let answer = "OKAY0082adb-ABCDE12345-Zq7Rt3\t_adb-tls-connect._tcp\t192.168.250.146:41500\n\
                      adb-a1b2c3d4-Ab12Cd\t_adb-tls-connect._tcp\t192.168.250.139:36600\n";
        let (script, _) = Scripted::new(vec![vec![(b"0012host:mdns:services", answer.as_bytes().to_vec())]]);
        let services = AdbClient::new(script).mdns_services().await.unwrap();
        let summary: Vec<_> = services.iter().map(|s| (s.serial(), s.kind, s.address)).collect();
        assert_eq!(
            summary,
            [
                (Some("ABCDE12345".to_string()), MdnsKind::Connect, addr("192.168.250.146:41500")),
                (Some("a1b2c3d4".to_string()), MdnsKind::Connect, addr("192.168.250.139:36600")),
            ]
        );
    }

    // pushed device lists -----------------------------------------------------------

    // The form of the pushes a real adb server makes while devices are
    // connected and dropped (the values are invented). Only the first push is preceded by OKAY,
    // every push holds the whole list, and several arrive in one read.
    const PUSH_PHONE: &str = "002f192.168.250.146:41500  offline transport_id:11\n";
    const PUSH_BOTH: &str = "005e192.168.250.139:36600  offline transport_id:12\n\
                             192.168.250.146:41500  offline transport_id:11\n";
    const PUSH_TABLET: &str = "002f192.168.250.139:36600  offline transport_id:12\n";
    const TRACK_REQUEST: &[u8] = b"0014host:track-devices-l";

    fn serials(entries: &[AdbEntry]) -> Vec<(&str, AdbState)> {
        entries.iter().map(|e| (e.serial.as_str(), e.state)).collect()
    }

    #[tokio::test]
    async fn each_push_is_the_whole_list_at_that_moment_and_the_end_is_reported() {
        let answer = format!("OKAY{PUSH_PHONE}{PUSH_BOTH}{PUSH_TABLET}");
        let (script, received) = Scripted::new(vec![vec![(TRACK_REQUEST, answer.into_bytes())]]);
        let mut tracker = AdbClient::new(script).track_devices().await.unwrap();
        assert_eq!(serials(&tracker.next().await.unwrap()), [("192.168.250.146:41500", AdbState::Offline)]);
        assert_eq!(
            serials(&tracker.next().await.unwrap()),
            [("192.168.250.139:36600", AdbState::Offline), ("192.168.250.146:41500", AdbState::Offline)]
        );
        assert_eq!(serials(&tracker.next().await.unwrap()), [("192.168.250.139:36600", AdbState::Offline)]);
        // The server ended the connection between two pushes.
        assert_eq!(tracker.next().await.err(), Some(AdbError::Closed));
        assert_eq!(received.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn an_empty_list_is_a_push_like_any_other() {
        let answer = format!("OKAY0000{PUSH_PHONE}");
        let (script, _) = Scripted::new(vec![vec![(TRACK_REQUEST, answer.into_bytes())]]);
        let mut tracker = AdbClient::new(script).track_devices().await.unwrap();
        assert_eq!(tracker.next().await.unwrap(), Vec::new());
        assert_eq!(tracker.next().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_connection_that_ends_inside_a_push_is_truncated_not_closed() {
        let (script, _) = Scripted::new(vec![vec![(TRACK_REQUEST, b"OKAY002f192.168.100".to_vec())]]);
        let mut tracker = AdbClient::new(script).track_devices().await.unwrap();
        assert_eq!(tracker.next().await.err(), Some(AdbError::Truncated));
    }

    #[tokio::test]
    async fn a_refused_track_request_is_the_servers_failure() {
        let (script, _) = Scripted::new(vec![vec![(TRACK_REQUEST, b"FAIL0014unknown host service".to_vec())]]);
        assert_eq!(
            AdbClient::new(script).track_devices().await.err(),
            Some(AdbError::Fail("unknown host service".into()))
        );
    }

    #[tokio::test]
    async fn a_shell_command_first_selects_the_device_and_then_runs_on_one_connection() {
        let transport = format!("{:04x}host:transport:{SERIAL}", "host:transport:".len() + SERIAL.len());
        let transport: &'static [u8] = Box::leak(transport.into_bytes().into_boxed_slice());
        let command: &'static [u8] = b"0025shell,v2:getprop ro.build.version.sdk";
        let (script, received) = Scripted::new(vec![vec![
            (transport, b"OKAY".to_vec()),
            (command, [b"OKAY".to_vec(), hex("010300000033300a030100000000")].concat()),
        ]]);
        let out = AdbClient::new(script).shell(SERIAL, "getprop ro.build.version.sdk").await.unwrap();
        assert_eq!(out, ShellOutput { stdout: b"30\n".to_vec(), stderr: Vec::new(), exit: 0 });
        assert_eq!(received.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn an_unknown_device_stops_the_shell_command_before_it_is_sent() {
        let transport: &'static [u8] = b"001dhost:transport:no-such-serial";
        let (script, received) = Scripted::new(vec![vec![(
            transport,
            b"FAIL0021device 'no-such-serial' not found".to_vec(),
        )]]);
        let result = AdbClient::new(script).shell("no-such-serial", "exit 3").await;
        assert_eq!(result, Err(AdbError::Fail("device 'no-such-serial' not found".into())));
        assert_eq!(received.lock().unwrap().len(), 1, "the command must not follow a refused transport");
    }

    #[tokio::test]
    async fn a_command_that_fails_on_the_device_is_a_result_with_an_exit_code() {
        let transport = format!("{:04x}host:transport:{SERIAL}", "host:transport:".len() + SERIAL.len());
        let transport: &'static [u8] = Box::leak(transport.into_bytes().into_boxed_slice());
        let (script, _) = Scripted::new(vec![vec![
            (transport, b"OKAY".to_vec()),
            (b"000fshell,v2:exit 3",[b"OKAY".to_vec(), hex("030100000003")].concat()),
        ]]);
        let out = AdbClient::new(script).shell(SERIAL, "exit 3").await.unwrap();
        assert_eq!(out.exit, 3);
    }

    // push (the sync service) --------------------------------------------------------

    /// What the device side of a push saw, in order.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Seen {
        Request(String),
        Send(String),
        Data(Vec<u8>),
        Done(u32),
    }

    /// What the stand-in device answers.
    #[derive(Clone, Copy)]
    enum Verdict {
        Okay,
        /// Refuses the file after it was sent (no space, no permission).
        Fail(&'static str),
        /// Does not know the device: refuses the first request.
        NoDevice(&'static str),
    }

    /// The stand-in with what it saw and how many connections were made to it.
    type Recorded = (SyncServer, Arc<Mutex<Vec<Seen>>>, Arc<Mutex<usize>>);

    struct SyncServer {
        seen: Arc<Mutex<Vec<Seen>>>,
        opened: Arc<Mutex<usize>>,
        verdict: Verdict,
    }

    impl SyncServer {
        fn new(verdict: Verdict) -> Recorded {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let opened = Arc::new(Mutex::new(0));
            (SyncServer { seen: seen.clone(), opened: opened.clone(), verdict }, seen, opened)
        }
    }

    async fn read_request(server: &mut tokio::io::DuplexStream) -> Option<String> {
        let mut head = [0u8; 4];
        server.read_exact(&mut head).await.ok()?;
        let len = usize::from_str_radix(std::str::from_utf8(&head).ok()?, 16).ok()?;
        let mut body = vec![0u8; len];
        server.read_exact(&mut body).await.ok()?;
        String::from_utf8(body).ok()
    }

    impl Connect for SyncServer {
        type Stream = tokio::io::DuplexStream;
        async fn open(&self) -> std::io::Result<Self::Stream> {
            *self.opened.lock().unwrap() += 1;
            let (client, mut server) = duplex(64 * 1024);
            let seen = self.seen.clone();
            let verdict = self.verdict;
            tokio::spawn(async move {
                let Some(request) = read_request(&mut server).await else { return };
                seen.lock().unwrap().push(Seen::Request(request));
                if let Verdict::NoDevice(message) = verdict {
                    let _ = server.write_all(format!("FAIL{:04x}{message}", message.len()).as_bytes()).await;
                    return;
                }
                let _ = server.write_all(b"OKAY").await;
                let Some(request) = read_request(&mut server).await else { return };
                seen.lock().unwrap().push(Seen::Request(request));
                let _ = server.write_all(b"OKAY").await;
                loop {
                    let mut head = [0u8; 8];
                    if server.read_exact(&mut head).await.is_err() {
                        return;
                    }
                    let len = u32::from_le_bytes(head[4..].try_into().unwrap());
                    match &head[..4] {
                        b"SEND" | b"DATA" => {
                            let mut body = vec![0u8; len as usize];
                            if server.read_exact(&mut body).await.is_err() {
                                return;
                            }
                            seen.lock().unwrap().push(if &head[..4] == b"SEND" {
                                Seen::Send(String::from_utf8(body).unwrap())
                            } else {
                                Seen::Data(body)
                            });
                        }
                        b"DONE" => {
                            seen.lock().unwrap().push(Seen::Done(len));
                            let answer = match verdict {
                                Verdict::Fail(message) => {
                                    [b"FAIL".to_vec(), (message.len() as u32).to_le_bytes().to_vec(), message.as_bytes().to_vec()]
                                        .concat()
                                }
                                _ => [b"OKAY".to_vec(), 0u32.to_le_bytes().to_vec()].concat(),
                            };
                            let _ = server.write_all(&answer).await;
                        }
                        _ => return,
                    }
                }
            });
            Ok(client)
        }
    }

    /// A file of invented bytes in the temporary folder.
    fn temp_file(name: &str, size: usize) -> (std::path::PathBuf, Vec<u8>) {
        let path = std::env::temp_dir().join(format!("scrcpy-agent-push-{}-{name}", std::process::id()));
        let content: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &content).unwrap();
        (path, content)
    }

    fn modified_seconds(path: &std::path::Path) -> u32 {
        let modified = std::fs::metadata(path).unwrap().modified().unwrap();
        modified.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as u32
    }

    /// Runs a push and returns its result with the progress it reported.
    async fn push_with_progress(
        client: &AdbClient<SyncServer>,
        local: &std::path::Path,
        remote: &str,
    ) -> (Result<(), AdbError>, Vec<u64>) {
        let reported = Mutex::new(Vec::new());
        let progress = |sent: u64| reported.lock().unwrap().push(sent);
        let result = client.push(SERIAL, local, remote, &progress).await;
        (result, reported.into_inner().unwrap())
    }

    #[tokio::test]
    async fn a_file_is_sent_in_pieces_of_at_most_64_kib_and_arrives_unchanged() {
        let (path, content) = temp_file("big", 150_000);
        let (server, seen, _) = SyncServer::new(Verdict::Okay);
        let client = AdbClient::new(server);
        let (result, progress) = push_with_progress(&client, &path, "/sdcard/Download/x.bin").await;
        assert_eq!(result, Ok(()));
        let seen = seen.lock().unwrap().clone();
        let transport = format!("host:transport:{SERIAL}");
        assert_eq!(
            seen,
            [
                Seen::Request(transport),
                Seen::Request("sync:".into()),
                // The path, a comma and the file mode (0100644) in decimal.
                Seen::Send("/sdcard/Download/x.bin,33188".into()),
                Seen::Data(content[..65_536].to_vec()),
                Seen::Data(content[65_536..131_072].to_vec()),
                Seen::Data(content[131_072..].to_vec()),
                Seen::Done(modified_seconds(&path)),
            ]
        );
        // How much had been sent after each piece.
        assert_eq!(progress, [65_536, 131_072, 150_000]);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn an_empty_file_is_a_send_and_a_done_without_data() {
        let (path, _) = temp_file("empty", 0);
        let (server, seen, _) = SyncServer::new(Verdict::Okay);
        let client = AdbClient::new(server);
        let (result, progress) = push_with_progress(&client, &path, "/sdcard/Download/empty.txt").await;
        assert_eq!(result, Ok(()));
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen[2..], [Seen::Send("/sdcard/Download/empty.txt,33188".into()), Seen::Done(modified_seconds(&path))]);
        assert!(progress.is_empty(), "{progress:?}");
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn a_file_the_device_refuses_after_it_was_sent_is_the_devices_failure() {
        let (path, _) = temp_file("refused", 1000);
        let (server, _, _) = SyncServer::new(Verdict::Fail("No space left on device"));
        let client = AdbClient::new(server);
        let (result, _) = push_with_progress(&client, &path, "/sdcard/Download/x.bin").await;
        assert_eq!(result, Err(AdbError::Fail("No space left on device".into())));
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn an_unknown_device_stops_the_push_before_the_sync_service_is_asked_for() {
        let (path, _) = temp_file("nodevice", 1000);
        let (server, seen, _) = SyncServer::new(Verdict::NoDevice("device 'x' not found"));
        let client = AdbClient::new(server);
        let (result, progress) = push_with_progress(&client, &path, "/sdcard/Download/x.bin").await;
        assert_eq!(result, Err(AdbError::Fail("device 'x' not found".into())));
        assert_eq!(seen.lock().unwrap().len(), 1, "only the transport request");
        assert!(progress.is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn a_path_the_protocol_cannot_carry_is_refused_before_anything_is_sent() {
        let (path, _) = temp_file("badpath", 10);
        let (server, seen, opened) = SyncServer::new(Verdict::Okay);
        let client = AdbClient::new(server);
        for bad in ["", "/sdcard/with\nbreak", "/sdcard/with\0nul"] {
            let (result, _) = push_with_progress(&client, &path, bad).await;
            assert_eq!(result, Err(AdbError::BadRemotePath), "{bad:?}");
        }
        assert_eq!(*opened.lock().unwrap(), 0, "no connection was made");
        assert!(seen.lock().unwrap().is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn a_file_that_cannot_be_read_is_an_io_error_and_nothing_is_sent() {
        let missing = std::env::temp_dir().join("scrcpy-agent-push-no-such-file");
        let (server, seen, _) = SyncServer::new(Verdict::Okay);
        let client = AdbClient::new(server);
        let (result, _) = push_with_progress(&client, &missing, "/sdcard/Download/x.bin").await;
        assert_eq!(result, Err(AdbError::Io(std::io::ErrorKind::NotFound)));
        // The file is looked at first, so the device is never asked for anything.
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_answer_to_connect_is_handed_on_as_it_was_said_whatever_the_language() {
        for said in CONNECT_SAID_IN_THREE_LANGUAGES {
            // The request was recorded from the official adb client.
            let (script, received) = Scripted::new(vec![vec![(b"0018host:connect:127.0.0.1:1", okay_with(said))]]);
            let client = AdbClient::new(script);
            assert_eq!(client.connect(addr("127.0.0.1:1")).await, Ok(said.to_string()));
            assert_eq!(received.lock().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn pair_and_disconnect_send_the_requests_the_official_client_sends() {
        // The requests were recorded from the official adb client.
        let (script, received) = Scripted::new(vec![
            vec![(b"001chost:pair:123456:127.0.0.1:1", b"OKAY0027Failed: Unable to start pairing client.".to_vec())],
            vec![(
                b"001bhost:disconnect:127.0.0.1:1",
                b"FAIL001cno such device '127.0.0.1:1'".to_vec(),
            )],
        ]);
        let client = AdbClient::new(script);
        // `OKAY` even though the pairing failed: the text is not a verdict.
        assert_eq!(
            client.pair(addr("127.0.0.1:1"), "123456").await,
            Ok("Failed: Unable to start pairing client.".to_string())
        );
        assert_eq!(
            client.disconnect(addr("127.0.0.1:1")).await,
            Err(AdbError::Fail("no such device '127.0.0.1:1'".into()))
        );
        assert_eq!(received.lock().unwrap().len(), 2);
    }
}
