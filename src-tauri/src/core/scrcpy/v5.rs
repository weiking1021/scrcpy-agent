//! The protocol of scrcpy 5.0.1.
//!
//! The video socket (checked against a real 5.0.1 stream; the documentation of
//! scrcpy, `doc/develop.md`, agrees):
//!
//! * a dummy byte (only when scrcpy connects to the device), then 64 bytes of
//!   device name, zero padded (only on the first socket);
//! * 4 bytes of codec id (`h264`);
//! * packets of a 12 byte header and a payload. A header whose first bit is 1
//!   is a session packet and has no payload: bytes 4..8 are the width and bytes
//!   8..12 the height, both big-endian. Any other header starts a media
//!   packet: bit 62 is set for a config packet, bit 61 for a key frame, the
//!   lower 61 bits are the time in microseconds (all big-endian over the first
//!   8 bytes), and bytes 8..12 are the size of the payload that follows.

use crate::core::apps::{AppDisplay, AppEntry, PackageName};
use crate::core::device::display::ScreenSize;
use crate::core::model::{BitRateMbps, KeyboardMode, MaxSize};
use super::protocol::{
    ControlFraming, ControlMessage, KeyAction, PacketKind, ProtocolError, ScrcpyProtocol, StreamPlan, TouchAction,
    VideoCodec, VideoEvent, VideoPacket, VideoStreamParser, VideoStreamStart,
};
use super::version::ScrcpyVersion;

/// The versions checked against real traffic.
const CHECKED: [ScrcpyVersion; 1] = [ScrcpyVersion::new(5, 0, 1)];

pub struct V5;

/// The screen-off timeout while a window that keeps the device awake is open:
/// one hour. scrcpy sets it for as long as it runs and then puts the old one back.
const KEEP_AWAKE_SECONDS: u32 = 3600;

impl ScrcpyProtocol for V5 {
    fn supported_versions(&self) -> &'static [ScrcpyVersion] {
        &CHECKED
    }

    fn proxy_args(&self, port: u16) -> Vec<String> {
        // `--tunnel-port` is the port the client connects to; it implies
        // `--force-adb-forward`, which is named anyway so the intent is plain.
        vec!["--force-adb-forward".into(), format!("--tunnel-port={port}")]
    }

    fn encode_control(&self, message: &ControlMessage) -> Result<Vec<u8>, ProtocolError> {
        Ok(match message {
            ControlMessage::InjectKeycode { action, keycode, repeat, meta_state } => {
                let mut bytes = vec![TYPE_INJECT_KEYCODE, key_action(*action)];
                bytes.extend(keycode.to_be_bytes());
                bytes.extend(repeat.to_be_bytes());
                bytes.extend(meta_state.to_be_bytes());
                bytes
            }
            ControlMessage::InjectTouch {
                action,
                pointer_id,
                x,
                y,
                screen_width,
                screen_height,
                pressure,
                action_button,
                buttons,
            } => {
                let mut bytes = vec![TYPE_INJECT_TOUCH_EVENT, touch_action(*action)];
                bytes.extend(pointer_id.to_be_bytes());
                bytes.extend(position(*x, *y, *screen_width, *screen_height));
                bytes.extend(unsigned_fixed_point(*pressure).to_be_bytes());
                bytes.extend(action_button.to_be_bytes());
                bytes.extend(buttons.to_be_bytes());
                bytes
            }
            ControlMessage::InjectScroll { x, y, screen_width, screen_height, horizontal, vertical, buttons } => {
                let mut bytes = vec![TYPE_INJECT_SCROLL_EVENT];
                bytes.extend(position(*x, *y, *screen_width, *screen_height));
                bytes.extend(scroll_fixed_point(*horizontal).to_be_bytes());
                bytes.extend(scroll_fixed_point(*vertical).to_be_bytes());
                bytes.extend(buttons.to_be_bytes());
                bytes
            }
            ControlMessage::BackOrScreenOn { action } => vec![TYPE_BACK_OR_SCREEN_ON, key_action(*action)],
            ControlMessage::ExpandNotificationPanel => vec![TYPE_EXPAND_NOTIFICATION_PANEL],
            ControlMessage::ExpandSettingsPanel => vec![TYPE_EXPAND_SETTINGS_PANEL],
            ControlMessage::CollapsePanels => vec![TYPE_COLLAPSE_PANELS],
            ControlMessage::RotateDevice => vec![TYPE_ROTATE_DEVICE],
        })
    }

    fn list_apps_args(&self, serial: &str) -> Vec<String> {
        vec![format!("--serial={serial}"), "--list-apps".into()]
    }

    fn parse_app_list(&self, output: &str) -> Vec<AppEntry> {
        // The server announces the list with this line; what comes before it is
        // the banner and scrcpy's own messages.
        let mut in_list = false;
        let mut apps = Vec::new();
        for line in output.lines() {
            let line = line.trim_end();
            if !in_list {
                in_list = line.strip_prefix("[server] INFO: ") == Some("List of apps:");
            } else if let Some(app) = parse_app_line(line) {
                apps.push(app);
            }
        }
        apps
    }

    fn start_app_args(
        &self,
        package: &PackageName,
        display: AppDisplay,
        size: Option<ScreenSize>,
        keep_awake: bool,
    ) -> Vec<String> {
        let mut args = Vec::new();
        if keep_awake {
            // `--stay-awake` only works while the device is plugged in; the
            // screen-off timeout also holds over a wireless connection. scrcpy
            // restores the initial values when it ends.
            args.push("--stay-awake".into());
            args.push(format!("--screen-off-timeout={KEEP_AWAKE_SECONDS}"));
        }
        if display == AppDisplay::VirtualDisplay {
            // A display of its own. Without a size scrcpy takes the natural one
            // of the main display, whichever way the device is held now.
            args.push(match size {
                Some(s) => format!("--new-display={}x{}", s.width, s.height),
                None => "--new-display".into(),
            });
        }
        // The app by its exact package name (scrcpy reads a leading `+` or `?` as
        // an instruction; a `PackageName` can have neither).
        args.push(format!("--start-app={}", package.as_str()));
        args
    }

    fn keyboard_args(&self, mode: KeyboardMode) -> Vec<String> {
        match mode {
            // What scrcpy does without an argument.
            KeyboardMode::Sdk => Vec::new(),
            KeyboardMode::Uhid => vec!["--keyboard=uhid".into()],
        }
    }

    fn video_bit_rate_args(&self, rate: BitRateMbps) -> Vec<String> {
        rate.mbps().map(|mbps| format!("--video-bit-rate={mbps}M")).into_iter().collect()
    }

    fn max_size_args(&self, size: MaxSize) -> Vec<String> {
        size.pixels().map(|pixels| format!("--max-size={pixels}")).into_iter().collect()
    }

    fn control_framing(&self) -> Box<dyn ControlFraming> {
        Box::new(V5ControlFraming { buffer: Vec::new(), failed: None })
    }

    fn control_socket_after_video(&self, plan: &StreamPlan) -> Option<usize> {
        // The sockets are opened in the order video, audio, control.
        (plan.video && plan.control).then_some(usize::from(plan.audio))
    }

    fn is_tunnel_socket(&self, name: &str) -> bool {
        // `scrcpy_<SCID>`, the id being a 31-bit number written in hexadecimal
        // (scrcpy's `doc/develop.md`).
        name.strip_prefix("scrcpy_").is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_hexdigit()))
    }

    fn video_parser(&self, start: VideoStreamStart) -> Box<dyn VideoStreamParser> {
        let stage = if start.dummy_byte {
            Stage::DummyByte
        } else if start.device_name {
            Stage::DeviceName
        } else {
            Stage::Codec
        };
        Box::new(V5VideoParser { stage, device_name: start.device_name, buffer: Vec::new(), failed: None })
    }
}

// The type of each control message (scrcpy's ControlMessage.java). Those used
// to send are in the recorded traffic, except settings panel and rotate.
const TYPE_INJECT_KEYCODE: u8 = 0;
const TYPE_INJECT_TEXT: u8 = 1;
const TYPE_INJECT_TOUCH_EVENT: u8 = 2;
const TYPE_INJECT_SCROLL_EVENT: u8 = 3;
const TYPE_BACK_OR_SCREEN_ON: u8 = 4;
const TYPE_EXPAND_NOTIFICATION_PANEL: u8 = 5;
const TYPE_EXPAND_SETTINGS_PANEL: u8 = 6;
const TYPE_COLLAPSE_PANELS: u8 = 7;
const TYPE_GET_CLIPBOARD: u8 = 8;
const TYPE_SET_CLIPBOARD: u8 = 9;
const TYPE_SET_DISPLAY_POWER: u8 = 10;
const TYPE_ROTATE_DEVICE: u8 = 11;
const TYPE_UHID_CREATE: u8 = 12;
const TYPE_UHID_INPUT: u8 = 13;
const TYPE_UHID_DESTROY: u8 = 14;
const TYPE_OPEN_HARD_KEYBOARD_SETTINGS: u8 = 15;
const TYPE_START_APP: u8 = 16;
const TYPE_RESET_VIDEO: u8 = 17;
const TYPE_CAMERA_SET_TORCH: u8 = 18;
const TYPE_CAMERA_ZOOM_IN: u8 = 19;
const TYPE_CAMERA_ZOOM_OUT: u8 = 20;
const TYPE_RESIZE_DISPLAY: u8 = 21;
const TYPE_SCAN_FILE: u8 = 22;

fn key_action(action: KeyAction) -> u8 {
    match action {
        KeyAction::Down => 0,
        KeyAction::Up => 1,
    }
}

/// Android's `MotionEvent` action numbers.
fn touch_action(action: TouchAction) -> u8 {
    match action {
        TouchAction::Down => 0,
        TouchAction::Up => 1,
        TouchAction::Move => 2,
        TouchAction::HoverMove => 7,
    }
}

/// A position and the size of the screen it is on: 4 + 4 + 2 + 2 bytes.
fn position(x: u32, y: u32, width: u16, height: u16) -> Vec<u8> {
    [x.to_be_bytes().to_vec(), y.to_be_bytes().to_vec(), width.to_be_bytes().to_vec(), height.to_be_bytes().to_vec()]
        .concat()
}

/// 0.0 to 1.0 as 0 to 65535 (1.0 is the largest value), held to that range.
fn unsigned_fixed_point(value: f32) -> u16 {
    ((value * 65536.0) as i64).clamp(0, 0xffff) as u16
}

/// Wheel notches as a signed 16 bit number in which 16 notches are 1.0 (the
/// recorded notch is -2048 = -1/16), held to the range of the number.
fn scroll_fixed_point(notches: f32) -> i16 {
    ((notches / 16.0 * 32768.0) as i64).clamp(i16::MIN as i64, i16::MAX as i64) as i16
}

/// One app of the list: a space, a mark (`*` system, `-` other), a space, the
/// name (padded with spaces) and the package, which is the last word.
fn parse_app_line(line: &str) -> Option<AppEntry> {
    let rest = line.strip_prefix(' ')?;
    let (mark, rest) = rest.split_at_checked(1)?;
    let system = match mark {
        "*" => true,
        "-" => false,
        _ => return None,
    };
    let rest = rest.strip_prefix(' ')?;
    let (name, package) = rest.trim_end().rsplit_once(char::is_whitespace)?;
    let package = PackageName::parse(package)?;
    let name = name.trim();
    (!name.is_empty()).then(|| AppEntry { package: package.as_str().to_string(), name: name.to_string(), system })
}

/// The biggest text a control message may announce: far above what is sent.
const MAX_CONTROL_TEXT: u32 = 64 * 1024 * 1024;

/// Follows the control socket far enough to know where each message ends
/// (layouts from the server's reader; the recorded traffic parsed to its last
/// byte with them).
struct V5ControlFraming {
    /// The part of a message that has not ended yet.
    buffer: Vec<u8>,
    failed: Option<ProtocolError>,
}

/// How long the message that starts `bytes` is, once enough of it is there to
/// tell; `None` when more is needed.
fn control_message_len(bytes: &[u8]) -> Result<Option<usize>, ProtocolError> {
    let Some(&kind) = bytes.first() else { return Ok(None) };
    let fixed = |len: usize| Ok(Some(len));
    // A length of `size` bytes at `at`, after `head` bytes that precede the text.
    let counted = |at: usize, size: usize, head: usize| -> Result<Option<usize>, ProtocolError> {
        let Some(field) = bytes.get(at..at + size) else { return Ok(None) };
        let len = field.iter().fold(0u32, |n, b| (n << 8) | u32::from(*b));
        if len > MAX_CONTROL_TEXT {
            return Err(ProtocolError::PacketTooLarge(len));
        }
        Ok(Some(head + len as usize))
    };
    match kind {
        TYPE_INJECT_KEYCODE => fixed(1 + 1 + 4 + 4 + 4),
        TYPE_INJECT_TOUCH_EVENT => fixed(1 + 1 + 8 + 12 + 2 + 4 + 4),
        TYPE_INJECT_SCROLL_EVENT => fixed(1 + 12 + 2 + 2 + 4),
        TYPE_BACK_OR_SCREEN_ON | TYPE_GET_CLIPBOARD | TYPE_SET_DISPLAY_POWER | TYPE_CAMERA_SET_TORCH => fixed(2),
        TYPE_EXPAND_NOTIFICATION_PANEL
        | TYPE_EXPAND_SETTINGS_PANEL
        | TYPE_COLLAPSE_PANELS
        | TYPE_ROTATE_DEVICE
        | TYPE_OPEN_HARD_KEYBOARD_SETTINGS
        | TYPE_RESET_VIDEO
        | TYPE_CAMERA_ZOOM_IN
        | TYPE_CAMERA_ZOOM_OUT => fixed(1),
        TYPE_RESIZE_DISPLAY => fixed(1 + 2 + 2),
        TYPE_UHID_DESTROY => fixed(1 + 2),
        TYPE_INJECT_TEXT | TYPE_SCAN_FILE => counted(1, 4, 1 + 4),
        TYPE_SET_CLIPBOARD => counted(1 + 8 + 1, 4, 1 + 8 + 1 + 4),
        TYPE_START_APP => counted(1, 1, 1 + 1),
        TYPE_UHID_INPUT => counted(1 + 2, 2, 1 + 2 + 2),
        TYPE_UHID_CREATE => {
            // id, vendor, product, then a name with a 1 byte length, then data with a 2 byte length.
            let name_len = match bytes.get(1 + 2 + 2 + 2) {
                Some(len) => usize::from(*len),
                None => return Ok(None),
            };
            let head = 1 + 6 + 1 + name_len;
            counted(head, 2, head + 2)
        }
        other => Err(ProtocolError::UnknownControlMessage(other)),
    }
}

impl ControlFraming for V5ControlFraming {
    fn feed(&mut self, bytes: &[u8]) -> Result<(), ProtocolError> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        self.buffer.extend_from_slice(bytes);
        while !self.buffer.is_empty() {
            match control_message_len(&self.buffer) {
                Ok(Some(len)) if self.buffer.len() >= len => {
                    self.buffer.drain(..len);
                }
                Ok(_) => break,
                Err(error) => {
                    self.failed = Some(error.clone());
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    fn at_boundary(&self) -> bool {
        self.failed.is_none() && self.buffer.is_empty()
    }
}

/// The longest packet that is believed: far above any real frame, far below
/// what a wrong layout would announce.
const MAX_PACKET: u32 = 64 * 1024 * 1024;

const DEVICE_NAME_LEN: usize = 64;
const HEADER_LEN: usize = 12;

#[derive(Clone, Copy)]
enum Stage {
    DummyByte,
    DeviceName,
    Codec,
    Packets,
}

struct V5VideoParser {
    stage: Stage,
    /// Whether a device name comes after the dummy byte.
    device_name: bool,
    /// Bytes received and not used yet.
    buffer: Vec<u8>,
    /// Once the stream is not understood it stays so.
    failed: Option<ProtocolError>,
}

impl V5VideoParser {
    fn step(&mut self, pos: &mut usize, events: &mut Vec<VideoEvent>) -> Result<bool, ProtocolError> {
        let rest = &self.buffer[*pos..];
        match self.stage {
            Stage::DummyByte => {
                if rest.is_empty() {
                    return Ok(false);
                }
                *pos += 1;
                self.stage = if self.device_name { Stage::DeviceName } else { Stage::Codec };
            }
            Stage::DeviceName => {
                let Some(field) = rest.get(..DEVICE_NAME_LEN) else { return Ok(false) };
                let end = field.iter().position(|&b| b == 0).unwrap_or(DEVICE_NAME_LEN);
                events.push(VideoEvent::DeviceName(String::from_utf8_lossy(&field[..end]).into_owned()));
                *pos += DEVICE_NAME_LEN;
                self.stage = Stage::Codec;
            }
            Stage::Codec => {
                let Some(id) = rest.get(..4) else { return Ok(false) };
                if id != b"h264" {
                    return Err(ProtocolError::UnsupportedCodec([id[0], id[1], id[2], id[3]]));
                }
                events.push(VideoEvent::Codec(VideoCodec::H264));
                *pos += 4;
                self.stage = Stage::Packets;
            }
            Stage::Packets => {
                let Some(header) = rest.get(..HEADER_LEN) else { return Ok(false) };
                let be32 = |at: usize| u32::from_be_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]]);
                if header[0] & 0x80 != 0 {
                    events.push(VideoEvent::Session { width: be32(4), height: be32(8) });
                    *pos += HEADER_LEN;
                    return Ok(true);
                }
                let size = be32(8);
                if size > MAX_PACKET {
                    return Err(ProtocolError::PacketTooLarge(size));
                }
                let Some(payload) = rest.get(HEADER_LEN..HEADER_LEN + size as usize) else { return Ok(false) };
                let flags_and_time = u64::from_be_bytes(header[..8].try_into().expect("8 bytes"));
                let kind = if header[0] & 0x40 != 0 {
                    PacketKind::Config
                } else if header[0] & 0x20 != 0 {
                    PacketKind::Key
                } else {
                    PacketKind::Delta
                };
                events.push(VideoEvent::Packet(VideoPacket {
                    kind,
                    pts_us: flags_and_time & 0x1FFF_FFFF_FFFF_FFFF,
                    payload: payload.to_vec(),
                }));
                *pos += HEADER_LEN + size as usize;
            }
        }
        Ok(true)
    }
}

impl VideoStreamParser for V5VideoParser {
    fn feed(&mut self, bytes: &[u8]) -> Result<Vec<VideoEvent>, ProtocolError> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        // (the error stays: a stream that was not understood is not resumed)
        self.buffer.extend_from_slice(bytes);
        let mut events = Vec::new();
        let mut pos = 0;
        let outcome = loop {
            match self.step(&mut pos, &mut events) {
                Ok(true) => continue,
                Ok(false) => break Ok(()),
                Err(error) => break Err(error),
            }
        };
        self.buffer.drain(..pos);
        match outcome {
            Ok(()) => Ok(events),
            Err(error) => {
                self.failed = Some(error.clone());
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Packet headers in the layout of scrcpy 5.0.1's video socket (checked once
    // against a real one). Every value is invented: the screen size, the
    // timestamps, the packet sizes, the device name and the payloads.
    const SESSION: &str = "8000000000000500000002d0"; // 1280 x 720
    const SESSION_ROTATED: &str = "80000000000002d000000500"; // 720 x 1280
    const CONFIG: &str = "400000000000000000000018"; // 24 bytes
    const KEY: &str = "20000000000f4240000007d0"; // 2000 bytes, at 1_000_000 us
    const DELTAS: [&str; 3] = [
        "000000000010c8e00000012c", // 300 bytes, at 1_100_000 us
        "0000000000124f80000000fa", // 250 bytes, at 1_200_000 us
        "000000000013d620000000c8", // 200 bytes, at 1_300_000 us
    ];

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
    }

    /// Invented payload bytes of a given size.
    fn payload(size: usize) -> Vec<u8> {
        (0..size).map(|i| (i % 251) as u8).collect()
    }

    fn packet(header: &str, size: usize) -> Vec<u8> {
        [hex(header), payload(size)].concat()
    }

    fn name_field(name: &str) -> Vec<u8> {
        let mut field = name.as_bytes().to_vec();
        field.resize(64, 0);
        field
    }

    /// The whole stream of the capture: prelude, codec, session, config, key
    /// frame, three delta frames.
    fn stream(dummy: bool, name: bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        if dummy {
            bytes.push(0);
        }
        if name {
            bytes.extend(name_field("Test Device"));
        }
        bytes.extend(b"h264");
        bytes.extend(hex(SESSION));
        bytes.extend(packet(CONFIG, 24));
        bytes.extend(packet(KEY, 2000));
        for (header, size) in DELTAS.iter().zip([300, 250, 200]) {
            bytes.extend(packet(header, size));
        }
        bytes
    }

    fn pkt(kind: PacketKind, pts_us: u64, size: usize) -> VideoEvent {
        VideoEvent::Packet(VideoPacket { kind, pts_us, payload: payload(size) })
    }

    fn expected(name: bool) -> Vec<VideoEvent> {
        let mut events = Vec::new();
        if name {
            events.push(VideoEvent::DeviceName("Test Device".into()));
        }
        events.extend([
            VideoEvent::Codec(VideoCodec::H264),
            VideoEvent::Session { width: 1280, height: 720 },
            pkt(PacketKind::Config, 0, 24),
            pkt(PacketKind::Key, 1_000_000, 2000),
            pkt(PacketKind::Delta, 1_100_000, 300),
            pkt(PacketKind::Delta, 1_200_000, 250),
            pkt(PacketKind::Delta, 1_300_000, 200),
        ]);
        events
    }

    fn parser(dummy_byte: bool, device_name: bool) -> Box<dyn VideoStreamParser> {
        V5.video_parser(VideoStreamStart { dummy_byte, device_name })
    }

    /// Feeds the bytes in pieces of `piece` bytes and collects every event.
    fn parse_in_pieces(mut parser: Box<dyn VideoStreamParser>, bytes: &[u8], piece: usize) -> Vec<VideoEvent> {
        bytes.chunks(piece).flat_map(|c| parser.feed(c).expect("a valid stream")).collect()
    }

    #[test]
    fn scrcpy_is_sent_to_the_proxy_by_forcing_the_forward_tunnel_to_the_proxys_port() {
        // From scrcpy 5.0.1's own `--help`: --tunnel-port sets the port the
        // client connects to, and implies --force-adb-forward.
        assert_eq!(V5.proxy_args(27000), ["--force-adb-forward", "--tunnel-port=27000"]);
        assert_eq!(V5.proxy_args(65535), ["--force-adb-forward", "--tunnel-port=65535"]);
    }

    // Control messages: a shift key going down, a mouse move, one notch of the
    // wheel, back, expand and collapse of the notification panel. The layouts
    // are those the real scrcpy 5.0.1 client sends (checked once against it); the
    // positions and the screen size in them are invented. The type numbers of
    // the others are from scrcpy's ControlMessage.java and their layouts from
    // the server's reader.
    const KEY_SHIFT_DOWN: &str = "00000000003b0000000000000041";
    // x 640, y 360 on a screen of 1280 x 720.
    const MOUSE_MOVE: &str = "0207ffffffffffffffff0000028000000168050002d0ffff0000000000000000";
    // x 500, y 300 on the same screen.
    const WHEEL_NOTCH: &str = "03000001f40000012c050002d00000f80000000000";
    const BACK_DOWN: &str = "0400";
    const EXPAND_NOTIFICATIONS: &str = "05";
    const COLLAPSE_PANELS: &str = "07";

    fn encoded(message: ControlMessage) -> String {
        V5.encode_control(&message)
            .expect("a supported message")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    #[test]
    fn control_messages_are_encoded_into_the_bytes_the_real_client_sent() {
        assert_eq!(
            encoded(ControlMessage::InjectKeycode { action: KeyAction::Down, keycode: 59, repeat: 0, meta_state: 65 }),
            KEY_SHIFT_DOWN
        );
        assert_eq!(
            encoded(ControlMessage::InjectTouch {
                action: TouchAction::HoverMove,
                pointer_id: -1,
                x: 640,
                y: 360,
                screen_width: 1280,
                screen_height: 720,
                pressure: 1.0,
                action_button: 0,
                buttons: 0,
            }),
            MOUSE_MOVE
        );
        assert_eq!(
            encoded(ControlMessage::InjectScroll {
                x: 500,
                y: 300,
                screen_width: 1280,
                screen_height: 720,
                horizontal: 0.0,
                vertical: -1.0,
                buttons: 0,
            }),
            WHEEL_NOTCH
        );
        assert_eq!(encoded(ControlMessage::BackOrScreenOn { action: KeyAction::Down }), BACK_DOWN);
        assert_eq!(encoded(ControlMessage::ExpandNotificationPanel), EXPAND_NOTIFICATIONS);
        assert_eq!(encoded(ControlMessage::CollapsePanels), COLLAPSE_PANELS);
        // From the source, not recorded: the type numbers 6 and 11.
        assert_eq!(encoded(ControlMessage::ExpandSettingsPanel), "06");
        assert_eq!(encoded(ControlMessage::RotateDevice), "0b");
    }

    #[test]
    fn a_key_goes_up_with_action_one_and_other_actions_and_touch_kinds_have_their_numbers() {
        let key_up = encoded(ControlMessage::InjectKeycode { action: KeyAction::Up, keycode: 4, repeat: 0, meta_state: 0 });
        // type, action, key code, repeat, meta state
        assert_eq!(key_up, "0001000000040000000000000000");
        assert_eq!(encoded(ControlMessage::BackOrScreenOn { action: KeyAction::Up }), "0401");
        for (action, number) in
            [(TouchAction::Down, "00"), (TouchAction::Up, "01"), (TouchAction::Move, "02"), (TouchAction::HoverMove, "07")]
        {
            let bytes = encoded(ControlMessage::InjectTouch {
                action,
                pointer_id: 0,
                x: 0,
                y: 0,
                screen_width: 0,
                screen_height: 0,
                pressure: 0.0,
                action_button: 0,
                buttons: 0,
            });
            assert_eq!(&bytes[2..4], number, "{action:?}");
        }
    }

    #[test]
    fn pressure_and_scrolling_are_fixed_point_numbers() {
        let touch = |pressure: f32| {
            let bytes = encoded(ControlMessage::InjectTouch {
                action: TouchAction::Down,
                pointer_id: 0,
                x: 0,
                y: 0,
                screen_width: 1,
                screen_height: 1,
                pressure,
                action_button: 0,
                buttons: 0,
            });
            // The pressure is the 2 bytes before the two 4-byte button fields.
            bytes[bytes.len() - 20..bytes.len() - 16].to_string()
        };
        assert_eq!(touch(1.0), "ffff"); // recorded: a mouse move with 1.0
        assert_eq!(touch(0.0), "0000");
        assert_eq!(touch(0.5), "8000");
        // Out of range is held to the range (a choice, not recorded).
        assert_eq!(touch(-3.0), "0000");
        assert_eq!(touch(7.0), "ffff");

        let scroll = |vertical: f32| {
            let bytes = encoded(ControlMessage::InjectScroll {
                x: 0,
                y: 0,
                screen_width: 1,
                screen_height: 1,
                horizontal: 0.0,
                vertical,
                buttons: 0,
            });
            bytes[bytes.len() - 12..bytes.len() - 8].to_string()
        };
        assert_eq!(scroll(-1.0), "f800"); // recorded: one notch down
        assert_eq!(scroll(1.0), "0800");
        assert_eq!(scroll(0.0), "0000");
        // Far beyond the range is held to the largest value (a choice, not recorded).
        assert_eq!(scroll(1000.0), "7fff");
        assert_eq!(scroll(-1000.0), "8000");
    }

    fn hex_bytes(hex: &str) -> Vec<u8> {
        (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect()
    }

    /// Feeds the bytes one at a time and returns where a boundary was seen.
    fn boundaries(framing: &mut dyn ControlFraming, bytes: &[u8]) -> Vec<usize> {
        let mut seen = Vec::new();
        for (i, byte) in bytes.iter().enumerate() {
            framing.feed(std::slice::from_ref(byte)).expect("a known message");
            if framing.at_boundary() {
                seen.push(i + 1);
            }
        }
        seen
    }

    // What `scrcpy --list-apps` prints (5.0.1), in the form it was seen to have:
    // a banner, scrcpy's own lines, the server's, then one line per app with a
    // mark, the name padded with spaces (by characters, so a name of wide
    // characters leaves more spaces) and the package. Every value is invented.
    const APP_LIST: &str = "scrcpy 5.0.1 <https://github.com/Genymobile/scrcpy>\r\n\
INFO: ADB device found:\r\n\
INFO:     --> (tcpip)  SERIAL-A            device  MODEL-A\r\n\
C:\\somewhere\\scrcpy-server: 1 file pushed, 0 skipped. 10.2 MB/s (733930 bytes in 0.069s)\r\n\
[server] INFO: Device: [Brand] Brand Model (Android 11)\r\n\
[server] INFO: Processing Android apps... (this may take some time)\r\n\
[server] INFO: List of apps:\r\n\
\x20* Browser                         com.example.browser\r\n\
\x20* Settings Manager                 com.example.settings\r\n\
\x20- Notes App                       org.example.notes\r\n\
\x20- Web Shortcut - Status           org.chromium.webapk.a1b2c3d4e5f60708_v2\r\n";

    fn app(package: &str, name: &str, system: bool) -> AppEntry {
        AppEntry { package: package.into(), name: name.into(), system }
    }

    #[test]
    fn the_apps_are_the_lines_after_the_servers_list_heading() {
        assert_eq!(
            V5.parse_app_list(APP_LIST),
            [
                app("com.example.browser", "Browser", true),
                app("com.example.settings", "Settings Manager", true),
                app("org.example.notes", "Notes App", false),
                // A name may hold spaces and dashes: the package is the last word.
                app("org.chromium.webapk.a1b2c3d4e5f60708_v2", "Web Shortcut - Status", false),
            ]
        );
    }

    #[test]
    fn lines_that_only_look_like_apps_are_not_taken() {
        // Before the heading: nothing counts, even a line that has the shape.
        let before = " * Browser                         com.example.browser\n[server] INFO: List of apps:\n";
        assert_eq!(V5.parse_app_list(before), Vec::new());
        // After it, a line must have a mark, a name and a package name that is one.
        // (a backslash at the end of a line also swallows the spaces that start the
        // next one, so the space before each mark is written out)
        let text = "[server] INFO: List of apps:\n\
\x20* Good                            com.example.good\n\
\x20? Unknown mark                    com.example.mark\n\
\x20- Bad package                     not a package\n\
\x20- Bad package two                 +com.example.plus\n\
\x20-NoSpaceAfterMark                 com.example.nospace\n\
\n\
INFO: something else\n\
\x20- Last                            com.example.last\n";
        assert_eq!(V5.parse_app_list(text), [app("com.example.good", "Good", true), app("com.example.last", "Last", false)]);
        // No list at all.
        assert_eq!(V5.parse_app_list(""), Vec::new());
        assert_eq!(V5.parse_app_list("ERROR: Could not connect to the device\n"), Vec::new());
    }

    #[test]
    fn listing_and_opening_apps_are_asked_of_scrcpy_with_its_own_arguments() {
        assert_eq!(V5.list_apps_args("SERIAL-A"), ["--serial=SERIAL-A", "--list-apps"]);
        let package = PackageName::parse("com.example.notes").unwrap();
        // Own display: `--new-display[=<w>x<h>]` and the app by its exact package
        // name (scrcpy 5.0.1's `--help`).
        let sideways = ScreenSize::new(1920, 1200);
        assert_eq!(
            V5.start_app_args(&package, AppDisplay::VirtualDisplay, sideways, false),
            ["--new-display=1920x1200", "--start-app=com.example.notes"]
        );
        assert_eq!(
            V5.start_app_args(&package, AppDisplay::VirtualDisplay, None, false),
            ["--new-display", "--start-app=com.example.notes"]
        );
        // The size only matters for a display of its own.
        assert_eq!(
            V5.start_app_args(&package, AppDisplay::MainDisplay, sideways, false),
            ["--start-app=com.example.notes"]
        );
        // Keeping the device awake: `--stay-awake` (plugged in) and a long
        // screen-off timeout (any connection), whichever display.
        assert_eq!(
            V5.start_app_args(&package, AppDisplay::VirtualDisplay, sideways, true),
            ["--stay-awake", "--screen-off-timeout=3600", "--new-display=1920x1200", "--start-app=com.example.notes"]
        );
        assert_eq!(
            V5.start_app_args(&package, AppDisplay::MainDisplay, None, true),
            ["--stay-awake", "--screen-off-timeout=3600", "--start-app=com.example.notes"]
        );
    }

    #[test]
    fn the_keyboard_mode_is_one_argument_and_the_default_is_none() {
        // From scrcpy 5.0.1's own `--help`: --keyboard=mode with "sdk" and "uhid".
        assert_eq!(V5.keyboard_args(KeyboardMode::Uhid), ["--keyboard=uhid"]);
        assert!(V5.keyboard_args(KeyboardMode::Sdk).is_empty());
    }

    #[test]
    fn the_video_bit_rate_is_one_argument_in_megabits_and_scrcpys_own_choice_is_none() {
        // From scrcpy 5.0.1's own `--help`: --video-bit-rate=value, 'M' is x1000000.
        assert_eq!(V5.video_bit_rate_args(BitRateMbps::new(12).unwrap()), ["--video-bit-rate=12M"]);
        assert_eq!(V5.video_bit_rate_args(BitRateMbps::new(200).unwrap()), ["--video-bit-rate=200M"]);
        assert!(V5.video_bit_rate_args(BitRateMbps::default()).is_empty());
    }

    #[test]
    fn the_longest_side_of_the_video_is_one_argument_and_the_devices_own_size_is_none() {
        // From scrcpy 5.0.1's own `--help`: --max-size=value, 0 is unlimited.
        assert_eq!(V5.max_size_args(MaxSize::new(1280).unwrap()), ["--max-size=1280"]);
        assert_eq!(V5.max_size_args(MaxSize::new(100).unwrap()), ["--max-size=100"]);
        assert!(V5.max_size_args(MaxSize::default()).is_empty());
    }

    #[test]
    fn the_end_of_each_message_is_found_in_a_stream_of_the_recorded_messages() {
        let all = [KEY_SHIFT_DOWN, MOUSE_MOVE, WHEEL_NOTCH, BACK_DOWN, EXPAND_NOTIFICATIONS, COLLAPSE_PANELS];
        let bytes: Vec<u8> = all.iter().flat_map(|m| hex_bytes(m)).collect();
        // 14, 32, 21, 2, 1 and 1 bytes: the boundaries are where each one ends.
        assert_eq!(boundaries(&mut *V5.control_framing(), &bytes), [14, 46, 67, 69, 70, 71]);
        // Before anything is fed, and between messages, it is a boundary.
        assert!(V5.control_framing().at_boundary());
        // Whatever the pieces, the result is the same.
        for piece in [2, 3, 5, 13, 31, 71] {
            let mut framing = V5.control_framing();
            let mut fed = 0;
            for chunk in bytes.chunks(piece) {
                framing.feed(chunk).unwrap();
                fed += chunk.len();
                assert_eq!(framing.at_boundary(), [14, 46, 67, 69, 70, 71].contains(&fed), "pieces of {piece}, after {fed}");
            }
        }
    }

    #[test]
    fn messages_with_text_and_lengths_are_followed_by_their_own_lengths() {
        // Layouts from the server's reader (not recorded): strings carry a 4 byte
        // length, except the names of an app and of a HID device, which carry 1.
        let messages: Vec<Vec<u8>> = vec![
            [vec![1], 2u32.to_be_bytes().to_vec(), b"ab".to_vec()].concat(), // INJECT_TEXT
            [vec![9], vec![0; 8], vec![1], 3u32.to_be_bytes().to_vec(), b"abc".to_vec()].concat(), // SET_CLIPBOARD
            [vec![12], vec![0, 1, 0, 2, 0, 3], vec![4], b"name".to_vec(), vec![0, 3], vec![9, 9, 9]].concat(), // UHID_CREATE
            [vec![13], vec![0, 1], vec![0, 2], vec![7, 7]].concat(), // UHID_INPUT
            vec![14, 0, 1],                                           // UHID_DESTROY
            [vec![16], vec![5], b"a.b.c".to_vec()].concat(),          // START_APP
            [vec![22], 4u32.to_be_bytes().to_vec(), b"/a/b".to_vec()].concat(), // SCAN_FILE
            vec![21, 0, 10, 0, 20],                                   // RESIZE_DISPLAY
            vec![18, 1],                                              // CAMERA_SET_TORCH
            vec![19],
            vec![20],
        ];
        let mut ends = Vec::new();
        let mut total = 0;
        for m in &messages {
            total += m.len();
            ends.push(total);
        }
        let bytes: Vec<u8> = messages.concat();
        assert_eq!(boundaries(&mut *V5.control_framing(), &bytes), ends);
    }

    #[test]
    fn a_message_type_that_is_not_known_ends_the_following_for_good() {
        let mut framing = V5.control_framing();
        assert_eq!(framing.feed(&[99]), Err(ProtocolError::UnknownControlMessage(99)));
        assert!(!framing.at_boundary());
        // Even bytes that would be a whole message now do not make it known again.
        let _ = framing.feed(&hex_bytes(COLLAPSE_PANELS));
        assert!(!framing.at_boundary());
    }

    #[test]
    fn the_control_socket_comes_after_the_audio_socket_when_there_is_one() {
        let plan = |video, audio, control| StreamPlan { video, audio, control };
        assert_eq!(V5.control_socket_after_video(&plan(true, true, true)), Some(1));
        assert_eq!(V5.control_socket_after_video(&plan(true, false, true)), Some(0));
        assert_eq!(V5.control_socket_after_video(&plan(true, true, false)), None);
        // Without video there is no first socket to count from.
        assert_eq!(V5.control_socket_after_video(&plan(false, true, true)), None);
    }

    #[test]
    fn the_tunnel_socket_is_named_scrcpy_and_a_hexadecimal_id() {
        for yes in ["scrcpy_1a2b3c4d", "scrcpy_7FFFFFFF", "scrcpy_0"] {
            assert!(V5.is_tunnel_socket(yes), "{yes}");
        }
        for no in ["scrcpy_", "scrcpy_xyz", "scrcpy_1a2b3c4dg", "Scrcpy_1a", "scrcpy1a2b", "other_1a2b", "", "xscrcpy_1a"] {
            assert!(!V5.is_tunnel_socket(no), "{no:?}");
        }
    }

    #[test]
    fn the_stream_is_taken_apart_into_what_it_says() {
        let events = parse_in_pieces(parser(true, true), &stream(true, true), usize::MAX);
        assert_eq!(events, expected(true));
    }

    #[test]
    fn the_result_does_not_depend_on_how_the_bytes_arrive() {
        let bytes = stream(true, true);
        for piece in [1, 2, 3, 7, 12, 13, 64, 65, 1000, 4096] {
            assert_eq!(parse_in_pieces(parser(true, true), &bytes, piece), expected(true), "pieces of {piece}");
        }
    }

    #[test]
    fn the_dummy_byte_and_the_device_name_are_only_there_when_the_connection_has_them() {
        // Not the first socket, and a reverse tunnel: neither is sent.
        assert_eq!(parse_in_pieces(parser(false, false), &stream(false, false), 100), expected(false));
        // Only the name.
        assert_eq!(parse_in_pieces(parser(false, true), &stream(false, true), 100), expected(true));
    }

    #[test]
    fn a_device_name_ends_at_its_first_zero_and_may_fill_all_64_bytes() {
        let mut p = parser(false, true);
        let mut bytes = name_field("A");
        bytes.extend(b"h264");
        assert_eq!(p.feed(&bytes).unwrap()[0], VideoEvent::DeviceName("A".into()));
        let mut p = parser(false, true);
        let mut bytes = "n".repeat(64).into_bytes();
        bytes.extend(b"h264");
        assert_eq!(p.feed(&bytes).unwrap()[0], VideoEvent::DeviceName("n".repeat(64)));
    }

    #[test]
    fn a_stream_that_was_not_understood_stays_failed() {
        let mut p = parser(false, false);
        assert_eq!(p.feed(b"h265"), Err(ProtocolError::UnsupportedCodec(*b"h265")));
        // Even bytes that would be fine now are not taken up again.
        assert_eq!(p.feed(b"h264"), Err(ProtocolError::UnsupportedCodec(*b"h265")));
    }

    #[test]
    fn a_new_picture_size_in_the_middle_of_the_stream_is_reported_when_it_comes() {
        let mut p = parser(false, false);
        let mut first = b"h264".to_vec();
        first.extend(hex(SESSION));
        first.extend(packet(DELTAS[0], 300));
        p.feed(&first).unwrap();
        // The device was rotated.
        let events = p.feed(&hex(SESSION_ROTATED)).unwrap();
        assert_eq!(events, [VideoEvent::Session { width: 720, height: 1280 }]);
    }

    #[test]
    fn a_stream_of_a_codec_that_was_not_checked_is_refused_not_guessed() {
        for id in [*b"h265", *b"av01", *b"vp09", *b"\0\0\0\0"] {
            let mut p = parser(false, false);
            assert_eq!(p.feed(&id), Err(ProtocolError::UnsupportedCodec(id)), "{id:?}");
        }
    }

    #[test]
    fn a_packet_larger_than_any_frame_means_the_layout_is_not_what_was_assumed() {
        let mut p = parser(false, false);
        let mut bytes = b"h264".to_vec();
        // A media header announcing 64 MiB + 1.
        bytes.extend([0u8; 8]);
        bytes.extend((64u32 * 1024 * 1024 + 1).to_be_bytes());
        assert_eq!(p.feed(&bytes), Err(ProtocolError::PacketTooLarge(64 * 1024 * 1024 + 1)));
        // 64 MiB itself is allowed to be announced: nothing is allocated for it up front.
        let mut p = parser(false, false);
        let mut bytes = b"h264".to_vec();
        bytes.extend([0u8; 8]);
        bytes.extend((64u32 * 1024 * 1024).to_be_bytes());
        // Only the codec is known so far; the packet waits for its payload.
        assert_eq!(p.feed(&bytes), Ok(vec![VideoEvent::Codec(VideoCodec::H264)]));
    }

    #[test]
    fn the_flags_of_a_media_header_are_read_from_their_own_bits() {
        let mut p = parser(false, false);
        let mut bytes = b"h264".to_vec();
        // First byte of the header and the lowest byte of the time. The last
        // one has no flag bit set: its time fills the 61 bits below the flags.
        for (first, pts_low) in [(0x40u8, 0u8), (0x20, 1), (0x00, 2), (0x1f, 3)] {
            let mut header = [0u8; 12];
            header[0] = first;
            header[7] = pts_low;
            header[11] = 1; // one payload byte
            bytes.extend(header);
            bytes.push(9);
        }
        let events = p.feed(&bytes).unwrap();
        let summary: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                VideoEvent::Packet(p) => Some((p.kind, p.pts_us)),
                _ => None,
            })
            .collect();
        assert_eq!(
            summary,
            [
                (PacketKind::Config, 0),
                (PacketKind::Key, 1),
                (PacketKind::Delta, 2),
                (PacketKind::Delta, (0x1fu64 << 56) | 3),
            ]
        );
    }
}
