//! The proxy between scrcpy and the device, which exists so add-ons can have
//! what needs scrcpy's internal protocol (the live video) while the user keeps
//! seeing scrcpy's own window.
//!
//! scrcpy is started to connect to the proxy (`--force-adb-forward
//! --tunnel-port=<port>`, whatever its version's adapter says); every
//! connection is relayed unchanged, both ways, to the forward scrcpy set up. The
//! first connection that gets data from the device is the video socket; what it
//! carries is also taken apart by the version's adapter and offered to whoever
//! subscribes.

use crate::core::adb::api::AdbApi;
use crate::core::adb::{Forward, ForwardRemote};
use crate::core::log::{LogCode, LogLevel, Logger};
use super::protocol::{
    ControlFraming, ControlMessage, PacketKind, ProtocolError, ScrcpyProtocol, StreamPlan, VideoCodec, VideoEvent,
    VideoPacket, VideoStreamStart,
};
use std::collections::VecDeque;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

/// The most bytes of the packets after a key frame that are kept for someone
/// who follows late.
const GROUP_CAP: usize = 32 * 1024 * 1024;

/// How many events a slow subscriber may be behind before it loses some.
const EVENT_BACKLOG: usize = 512;

/// How long the forward scrcpy creates is waited for, in tries.
const FORWARD_TRIES: u32 = 100;
const FORWARD_WAIT: Duration = Duration::from_millis(50);

/// What a subscriber gets: the stream so far in a form that can be decoded from
/// its first packet, and then every new event.
pub struct VideoSubscription {
    /// The device name, codec and picture size, the parameter packet, and the
    /// packets since the last key frame (the key frame first).
    pub initial: Vec<VideoEvent>,
    pub receiver: broadcast::Receiver<VideoEvent>,
}

/// What a late subscriber needs to start decoding at once, kept up to date as
/// events pass.
pub struct StreamState {
    /// The most bytes of packets after a key frame that are kept.
    cap: usize,
    name: Option<String>,
    codec: Option<VideoCodec>,
    session: Option<(u32, u32)>,
    config: Option<VideoPacket>,
    /// The key frame and what follows it, as long as it fits `cap`.
    group: Vec<VideoPacket>,
    group_bytes: usize,
    /// Whether frames are being added to the group: after a key frame, until
    /// the group no longer fits.
    group_open: bool,
}

impl StreamState {
    pub fn new(cap: usize) -> Self {
        StreamState {
            cap,
            name: None,
            codec: None,
            session: None,
            config: None,
            group: Vec::new(),
            group_bytes: 0,
            group_open: false,
        }
    }

    fn drop_group(&mut self) {
        self.group.clear();
        self.group_bytes = 0;
        self.group_open = false;
    }

    /// Takes note of an event of the stream.
    pub fn apply(&mut self, event: &VideoEvent) {
        match event {
            VideoEvent::DeviceName(name) => self.name = Some(name.clone()),
            VideoEvent::Codec(codec) => self.codec = Some(*codec),
            VideoEvent::Session { width, height } => {
                // A new picture size is a new encoder stream: what was kept
                // belongs to the old one.
                self.session = Some((*width, *height));
                self.config = None;
                self.drop_group();
            }
            VideoEvent::Packet(packet) => match packet.kind {
                PacketKind::Config => self.config = Some(packet.clone()),
                PacketKind::Key => {
                    self.drop_group();
                    if packet.payload.len() <= self.cap {
                        self.group_bytes = packet.payload.len();
                        self.group.push(packet.clone());
                        self.group_open = true;
                    }
                }
                PacketKind::Delta if self.group_open => {
                    self.group_bytes += packet.payload.len();
                    if self.group_bytes > self.cap {
                        // A group that does not fit is of no use without its
                        // start: drop it whole and wait for the next key frame.
                        self.drop_group();
                    } else {
                        self.group.push(packet.clone());
                    }
                }
                PacketKind::Delta => {}
            },
        }
    }

    /// The events a new subscriber starts with.
    pub fn initial(&self) -> Vec<VideoEvent> {
        let mut events = Vec::new();
        events.extend(self.name.clone().map(VideoEvent::DeviceName));
        events.extend(self.codec.map(VideoEvent::Codec));
        events.extend(self.session.map(|(width, height)| VideoEvent::Session { width, height }));
        events.extend(self.config.clone().map(VideoEvent::Packet));
        events.extend(self.group.iter().cloned().map(VideoEvent::Packet));
        events
    }
}

/// The state of the stream and the way to hear of its new events. An event is
/// recorded and sent while the state is locked, so a subscriber neither misses
/// nor repeats one between the start it is given and what it is sent.
struct Hub {
    state: Mutex<StreamState>,
    sender: broadcast::Sender<VideoEvent>,
}

impl Hub {
    fn publish(&self, event: VideoEvent) {
        let mut state = self.state.lock().unwrap();
        state.apply(&event);
        // Nobody listening is not an error.
        let _ = self.sender.send(event);
    }

    fn subscribe(&self) -> VideoSubscription {
        let state = self.state.lock().unwrap();
        VideoSubscription { initial: state.initial(), receiver: self.sender.subscribe() }
    }
}

/// Finds the local port of the forward scrcpy created for this device: one that
/// was not there before scrcpy was started and leads to scrcpy's tunnel socket.
struct Upstream<A> {
    adb: A,
    serial: String,
    before: Vec<Forward>,
    protocol: Arc<dyn ScrcpyProtocol>,
    found: Mutex<Option<u16>>,
}

impl<A: AdbApi> Upstream<A> {
    async fn port(&self) -> Option<u16> {
        if let Some(port) = *self.found.lock().unwrap() {
            return Some(port);
        }
        for _ in 0..FORWARD_TRIES {
            if let Ok(forwards) = self.adb.forwards().await
                && let Some(forward) = forwards.iter().find(|f| {
                    f.serial == self.serial
                        && !self.before.contains(f)
                        && matches!(&f.remote, ForwardRemote::LocalAbstract(name) if self.protocol.is_tunnel_socket(name))
                })
            {
                *self.found.lock().unwrap() = Some(forward.local_port);
                return Some(forward.local_port);
            }
            tokio::time::sleep(FORWARD_WAIT).await;
        }
        None
    }
}

/// A proxy for one screen. Dropping it ends it.
pub struct Proxy {
    port: u16,
    hub: Arc<Hub>,
    tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
    logger: Logger,
    serial: String,
    protocol: Arc<dyn ScrcpyProtocol>,
    /// Where our own control messages go, while the control socket is up.
    control: ControlSlot,
    /// Whether scrcpy was started with a control socket the proxy can use.
    offers_control: bool,
}

type ControlSlot = Arc<Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>>;

/// What the relay of the control socket needs to put our messages in.
struct ControlLink {
    messages: mpsc::UnboundedReceiver<Vec<u8>>,
    framing: Box<dyn ControlFraming>,
    slot: ControlSlot,
}

impl Proxy {
    /// Starts listening on a free local port. `before` is the list of forwards
    /// as it was before scrcpy was started.
    pub async fn start<A: AdbApi>(
        adb: A,
        serial: String,
        before: Vec<Forward>,
        protocol: Arc<dyn ScrcpyProtocol>,
        plan: StreamPlan,
        logger: Logger,
    ) -> std::io::Result<Proxy> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        let hub = Arc::new(Hub {
            state: Mutex::new(StreamState::new(GROUP_CAP)),
            sender: broadcast::channel(EVENT_BACKLOG).0,
        });
        let upstream = Arc::new(Upstream { adb, serial: serial.clone(), before, protocol: protocol.clone(), found: Mutex::new(None) });
        let tasks: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::default();
        let video_claimed = Arc::new(AtomicBool::new(false));
        let control: ControlSlot = Arc::default();
        let control_position = protocol.control_socket_after_video(&plan);

        let accept = {
            let (hub, tasks, logger, protocol, control) =
                (hub.clone(), tasks.clone(), logger.clone(), protocol.clone(), control.clone());
            tokio::spawn(async move {
                // How many connections came after the video socket was found.
                let mut after_video = 0usize;
                let mut accepted = 0usize;
                loop {
                    let Ok((down, _)) = listener.accept().await else { break };
                    accepted += 1;
                    // Connections before the video socket delivered anything are
                    // scrcpy trying while the device's server starts. Those after
                    // it are its other sockets, in order: audio, then control.
                    let position = video_claimed.load(Ordering::SeqCst).then(|| {
                        after_video += 1;
                        after_video - 1
                    });
                    // The device gives each socket its role (video, audio,
                    // control) by the order they reach it, and scrcpy opens them
                    // one right after the other. So the connection to the device
                    // is made here, before the next one is accepted, never from
                    // a task of its own: tasks would race and swap them.
                    let Some(up) = connect_upstream(&upstream, &logger).await else { continue };
                    if let Some(position) = position {
                        let role = if Some(position) == control_position {
                            "the control socket".to_string()
                        } else {
                            format!("socket {position} after the video one (control is {control_position:?})")
                        };
                        logger.log(LogLevel::Info, LogCode::ProxySocket, format!("connection {accepted} is {role}"));
                    }
                    let link = (position.is_some() && position == control_position).then(|| {
                        let (sender, messages) = mpsc::unbounded_channel();
                        *control.lock().unwrap() = Some(sender);
                        ControlLink { messages, framing: protocol.control_framing(), slot: control.clone() }
                    });
                    let relay = tokio::spawn(relay(
                        Connection { down, up, number: accepted },
                        protocol.clone(),
                        video_claimed.clone(),
                        link,
                        hub.clone(),
                        logger.clone(),
                    ));
                    tasks.lock().unwrap().push(relay);
                }
            })
        };
        tasks.lock().unwrap().push(accept);
        logger.log(LogLevel::Info, LogCode::ProxyStarted, format!("proxy for {serial} listening on 127.0.0.1:{port}"));
        Ok(Proxy { port, hub, tasks, logger, serial, protocol, control, offers_control: control_position.is_some() })
    }

    /// Whether control messages can be sent through this proxy. (The control
    /// socket may still be coming up; a send then says it is not there yet.)
    pub fn offers_control(&self) -> bool {
        self.offers_control
    }

    /// The local port scrcpy is told to connect to.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Follows the video: the stream so far, and then what comes.
    pub fn subscribe(&self) -> VideoSubscription {
        self.hub.subscribe()
    }

    /// Sends a control message to the device, between two of scrcpy's own.
    /// Returns once it is queued; it is written when scrcpy's last message is
    /// whole.
    pub fn send(&self, message: &ControlMessage) -> Result<(), ProxyError> {
        let bytes = self.protocol.encode_control(message)?;
        let slot = self.control.lock().unwrap();
        let sender = slot.as_ref().ok_or(ProxyError::NoControlSocket)?;
        sender.send(bytes).map_err(|_| ProxyError::NoControlSocket)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProxyError {
    /// scrcpy has no control socket, or it is not up yet or already gone.
    #[error("there is no control socket")]
    NoControlSocket,
    #[error("{0}")]
    Protocol(#[from] ProtocolError),
}

impl Drop for Proxy {
    fn drop(&mut self) {
        for task in self.tasks.lock().unwrap().drain(..) {
            task.abort();
        }
        self.logger.log(LogLevel::Info, LogCode::ProxyStopped, format!("proxy for {} stopped", self.serial));
    }
}

/// Connects to the forward scrcpy created, which leads to the device.
async fn connect_upstream<A: AdbApi>(upstream: &Upstream<A>, logger: &Logger) -> Option<TcpStream> {
    let Some(port) = upstream.port().await else {
        logger.log(LogLevel::Warn, LogCode::ProxyStreamError, "the forward scrcpy created was not found");
        return None;
    };
    match TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await {
        Ok(up) => Some(up),
        Err(_) => {
            logger.log(LogLevel::Warn, LogCode::ProxyStreamError, format!("cannot connect to the forward on port {port}"));
            None
        }
    }
}

/// Copies what scrcpy sends on the control socket to the device, and writes
/// our own messages in between: a message of ours waits until everything scrcpy
/// has sent is whole messages. If scrcpy sends something that cannot be followed,
/// ours are no longer sent (the socket is still relayed).
async fn carry_control(
    from_scrcpy: &mut tokio::net::tcp::OwnedReadHalf,
    to_device: &mut tokio::net::tcp::OwnedWriteHalf,
    mut link: ControlLink,
    logger: &Logger,
) {
    let mut buffer = vec![0u8; 16 * 1024];
    let mut waiting: VecDeque<Vec<u8>> = VecDeque::new();
    let mut following = true;
    let mut from_scrcpy_total = 0usize;
    'carry: loop {
        tokio::select! {
            read = from_scrcpy.read(&mut buffer) => {
                let n = match read {
                    Ok(0) | Err(_) => break 'carry,
                    Ok(n) => n,
                };
                from_scrcpy_total += n;
                if to_device.write_all(&buffer[..n]).await.is_err() {
                    break 'carry;
                }
                if following && let Err(error) = link.framing.feed(&buffer[..n]) {
                    logger.log(
                        LogLevel::Warn,
                        LogCode::ProxyStreamError,
                        format!("control messages are no longer followed, so none is sent: {error}"),
                    );
                    following = false;
                    waiting.clear();
                    link.messages.close();
                    link.slot.lock().unwrap().take();
                }
            }
            Some(message) = link.messages.recv(), if following => {
                if !link.framing.at_boundary() {
                    logger.log(
                        LogLevel::Debug,
                        LogCode::ControlWaiting,
                        format!("a message of {} bytes waits for scrcpy's own message to end", message.len()),
                    );
                }
                waiting.push_back(message);
            }
        }
        if following && link.framing.at_boundary() {
            while let Some(message) = waiting.pop_front() {
                if to_device.write_all(&message).await.is_err() {
                    break 'carry;
                }
                logger.log(
                    LogLevel::Debug,
                    LogCode::ControlWritten,
                    format!("wrote {} bytes to the device (scrcpy had sent {from_scrcpy_total} bytes)", message.len()),
                );
            }
        }
    }
    // The socket is gone: nothing more can be sent on it.
    link.slot.lock().unwrap().take();
}

/// The two ends of one connection of scrcpy, and its number (in the order
/// scrcpy made them).
struct Connection {
    /// Towards scrcpy.
    down: TcpStream,
    /// Towards the device.
    up: TcpStream,
    number: usize,
}

/// One connection of scrcpy: relayed unchanged to the device and back. The
/// first connection that gets data from the device is the video socket, which is
/// also taken apart.
async fn relay(
    connection: Connection,
    protocol: Arc<dyn ScrcpyProtocol>,
    video_claimed: Arc<AtomicBool>,
    control: Option<ControlLink>,
    hub: Arc<Hub>,
    logger: Logger,
) {
    let Connection { down, up, number } = connection;
    let (mut down_read, mut down_write) = down.into_split();
    let (mut up_read, mut up_write) = up.into_split();

    // From scrcpy to the device: unchanged. On the control socket our own
    // messages are added, but only where one of scrcpy's has ended.
    let to_device = {
        let logger = logger.clone();
        tokio::spawn(async move {
            match control {
                None => {
                    let _ = tokio::io::copy(&mut down_read, &mut up_write).await;
                }
                Some(link) => carry_control(&mut down_read, &mut up_write, link, &logger).await,
            }
            let _ = up_write.shutdown().await;
        })
    };

    // From the device to scrcpy: unchanged, and the video socket is also followed.
    let mut parser = None;
    let mut seen_any = false;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let n = match up_read.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if !seen_any {
            seen_any = true;
            if !video_claimed.swap(true, Ordering::SeqCst) {
                logger.log(LogLevel::Info, LogCode::ProxySocket, format!("connection {number} is the video socket"));
                // The first socket of a forward tunnel starts with a dummy byte
                // and the device name.
                parser = Some(protocol.video_parser(VideoStreamStart { dummy_byte: true, device_name: true }));
            }
        }
        if down_write.write_all(&buffer[..n]).await.is_err() {
            break;
        }
        if let Some(active) = parser.as_mut() {
            match active.feed(&buffer[..n]) {
                Ok(events) => events.into_iter().for_each(|event| hub.publish(event)),
                Err(error) => {
                    logger.log(LogLevel::Warn, LogCode::ProxyStreamError, format!("the video is no longer followed: {error}"));
                    parser = None;
                }
            }
        }
    }
    let _ = down_write.shutdown().await;
    to_device.abort();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::scrcpy::protocol::VideoPacket;

    fn packet(kind: PacketKind, pts_us: u64, size: usize) -> VideoEvent {
        VideoEvent::Packet(VideoPacket { kind, pts_us, payload: vec![0xAB; size] })
    }

    fn start() -> Vec<VideoEvent> {
        vec![
            VideoEvent::DeviceName("Test Device".into()),
            VideoEvent::Codec(VideoCodec::H264),
            VideoEvent::Session { width: 1280, height: 720 },
        ]
    }

    fn state_after(cap: usize, events: &[VideoEvent]) -> StreamState {
        let mut state = StreamState::new(cap);
        for event in events {
            state.apply(event);
        }
        state
    }

    #[test]
    fn a_new_subscriber_gets_what_is_needed_to_decode_from_its_first_packet() {
        let mut events = start();
        events.extend([
            packet(PacketKind::Config, 0, 24),
            packet(PacketKind::Key, 100, 3000),
            packet(PacketKind::Delta, 200, 300),
            packet(PacketKind::Delta, 300, 300),
        ]);
        assert_eq!(state_after(1 << 20, &events).initial(), events);
    }

    #[test]
    fn a_newer_key_frame_replaces_the_frames_before_it() {
        let mut events = start();
        events.extend([
            packet(PacketKind::Config, 0, 24),
            packet(PacketKind::Key, 100, 3000),
            packet(PacketKind::Delta, 200, 300),
            packet(PacketKind::Key, 300, 3000),
            packet(PacketKind::Delta, 400, 300),
        ]);
        let mut expected = start();
        expected.extend([
            packet(PacketKind::Config, 0, 24),
            packet(PacketKind::Key, 300, 3000),
            packet(PacketKind::Delta, 400, 300),
        ]);
        assert_eq!(state_after(1 << 20, &events).initial(), expected);
    }

    #[test]
    fn a_newer_config_packet_replaces_the_older_one() {
        let mut events = start();
        events.extend([packet(PacketKind::Config, 0, 24), packet(PacketKind::Config, 5, 40)]);
        let mut expected = start();
        expected.push(packet(PacketKind::Config, 5, 40));
        assert_eq!(state_after(1 << 20, &events).initial(), expected);
    }

    #[test]
    fn a_new_picture_size_starts_the_stream_over() {
        let mut events = start();
        events.extend([
            packet(PacketKind::Config, 0, 24),
            packet(PacketKind::Key, 100, 3000),
            packet(PacketKind::Delta, 200, 300),
            // The device was rotated: a new encoder stream follows.
            VideoEvent::Session { width: 720, height: 1280 },
        ]);
        let expected = vec![
            VideoEvent::DeviceName("Test Device".into()),
            VideoEvent::Codec(VideoCodec::H264),
            VideoEvent::Session { width: 720, height: 1280 },
        ];
        assert_eq!(state_after(1 << 20, &events).initial(), expected);
    }

    #[test]
    fn frames_that_have_no_key_frame_before_them_are_not_kept() {
        let mut events = start();
        events.extend([packet(PacketKind::Delta, 100, 300), packet(PacketKind::Delta, 200, 300)]);
        assert_eq!(state_after(1 << 20, &events).initial(), start());
    }

    #[test]
    fn a_group_too_big_to_keep_is_dropped_whole_until_the_next_key_frame() {
        // Cap 100: the key frame (60) and a delta (30) fit, the second delta does not.
        let mut events = start();
        events.extend([
            packet(PacketKind::Key, 100, 60),
            packet(PacketKind::Delta, 200, 30),
            packet(PacketKind::Delta, 300, 30),
            packet(PacketKind::Delta, 400, 10),
        ]);
        // Nothing of the group is offered: a delta without its key is no use.
        assert_eq!(state_after(100, &events).initial(), start());
        // The next key frame starts a group that fits.
        events.push(packet(PacketKind::Key, 500, 60));
        let mut expected = start();
        expected.push(packet(PacketKind::Key, 500, 60));
        assert_eq!(state_after(100, &events).initial(), expected);
    }

    #[test]
    fn a_key_frame_bigger_than_the_cap_is_not_kept_and_its_frames_neither() {
        let mut events = start();
        events.extend([packet(PacketKind::Key, 100, 101), packet(PacketKind::Delta, 200, 1)]);
        assert_eq!(state_after(100, &events).initial(), start());
    }
}
