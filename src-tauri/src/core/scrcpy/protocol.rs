//! scrcpy's internal protocol, behind an interface that has one adapter per
//! supported scrcpy version.
//!
//! The protocol is internal to scrcpy: its own documentation says it may change
//! at any time and that client and server must be the same version. So nothing
//! outside the adapters knows a byte layout. A version without an adapter has
//! none of the features that need the protocol (the proxy mode); the official
//! scrcpy window still works.

use crate::core::apps::{AppDisplay, AppEntry, PackageName};
use crate::core::device::display::ScreenSize;
use crate::core::model::{BitRateMbps, KeyboardMode, MaxSize};
use super::version::ScrcpyVersion;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    /// A codec this adapter has not been checked against.
    #[error("unsupported video codec id {0:?}")]
    UnsupportedCodec([u8; 4]),
    /// A packet bigger than any real frame: the layout is not what was assumed.
    #[error("a packet of {0} bytes")]
    PacketTooLarge(u32),
    /// The client sent a control message of a type this adapter does not know,
    /// so where the next one starts is not known either.
    #[error("unknown control message type {0}")]
    UnknownControlMessage(u8),
    /// This version cannot send this control message.
    #[error("this control message is not supported by this scrcpy version")]
    UnsupportedControlMessage,
}

/// Whether a key or button goes down or up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Down,
    Up,
}

/// What a touch (or the mouse standing in for one) does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchAction {
    Down,
    Up,
    Move,
    /// The pointer moves without being pressed.
    HoverMove,
}

/// A message to the device on the control socket. Values are what Android
/// uses (key codes, meta state, button state).
#[derive(Debug, Clone, PartialEq)]
pub enum ControlMessage {
    InjectKeycode { action: KeyAction, keycode: u32, repeat: u32, meta_state: u32 },
    InjectTouch {
        action: TouchAction,
        /// -1 stands for the mouse.
        pointer_id: i64,
        x: u32,
        y: u32,
        /// The size of the screen the position is on.
        screen_width: u16,
        screen_height: u16,
        /// From 0.0 to 1.0.
        pressure: f32,
        action_button: u32,
        buttons: u32,
    },
    InjectScroll {
        x: u32,
        y: u32,
        screen_width: u16,
        screen_height: u16,
        /// In wheel notches; scrolling up and to the right is positive.
        horizontal: f32,
        vertical: f32,
        buttons: u32,
    },
    BackOrScreenOn { action: KeyAction },
    ExpandNotificationPanel,
    ExpandSettingsPanel,
    CollapsePanels,
    RotateDevice,
}

/// Which sockets a scrcpy was started with: they are opened in the order video,
/// audio, control, those that are off left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamPlan {
    pub video: bool,
    pub audio: bool,
    pub control: bool,
}

/// Follows what scrcpy sends on the control socket far enough to know where a
/// message ends, so that our own message is only ever put between two of
/// scrcpy's.
pub trait ControlFraming: Send {
    /// Takes in bytes the client sent.
    fn feed(&mut self, bytes: &[u8]) -> Result<(), ProtocolError>;

    /// Whether everything fed so far is whole messages. After an error it is
    /// never true again: where messages end is no longer known.
    fn at_boundary(&self) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
}

/// What a video packet is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketKind {
    /// Parameter sets (for H.264, SPS and PPS), needed before any frame.
    Config,
    /// A frame that can be decoded on its own.
    Key,
    /// A frame that needs the ones before it.
    Delta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoPacket {
    pub kind: PacketKind,
    /// The presentation time in microseconds, as the device stamps it.
    pub pts_us: u64,
    /// The encoded data (for H.264, Annex B).
    pub payload: Vec<u8>,
}

/// What the video socket says, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoEvent {
    /// The device's name (only on the first socket).
    DeviceName(String),
    Codec(VideoCodec),
    /// The size of the picture, at the start and whenever it changes (rotation).
    Session { width: u32, height: u32 },
    Packet(VideoPacket),
}

/// What comes before the video data on this connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoStreamStart {
    /// A dummy byte first: scrcpy connected to the device (forward tunnel).
    pub dummy_byte: bool,
    /// The device name first: this is the first socket.
    pub device_name: bool,
}

/// Takes the bytes of a video socket apart as they arrive, in pieces of any
/// size.
pub trait VideoStreamParser: Send {
    fn feed(&mut self, bytes: &[u8]) -> Result<Vec<VideoEvent>, ProtocolError>;
}

/// One version's way of speaking the protocol.
pub trait ScrcpyProtocol: Send + Sync {
    /// The versions this adapter has been checked against with real traffic.
    fn supported_versions(&self) -> &'static [ScrcpyVersion];

    fn video_parser(&self, start: VideoStreamStart) -> Box<dyn VideoStreamParser>;

    /// The arguments that make scrcpy connect to a proxy listening on this
    /// local port instead of to the device's own connection.
    fn proxy_args(&self, port: u16) -> Vec<String>;

    /// Whether this is the name of the socket scrcpy's server listens on on the
    /// device, which tells the forward scrcpy created from any other forward.
    fn is_tunnel_socket(&self, name: &str) -> bool;

    /// The bytes that carry this message on the control socket.
    fn encode_control(&self, message: &ControlMessage) -> Result<Vec<u8>, ProtocolError>;

    fn control_framing(&self) -> Box<dyn ControlFraming>;

    /// The arguments that make scrcpy print the apps of a device and end.
    fn list_apps_args(&self, serial: &str) -> Vec<String>;

    /// The apps in what scrcpy printed for `list_apps_args` (both its output
    /// streams, one after the other). Anything that is not an app line is left
    /// out.
    fn parse_app_list(&self, output: &str) -> Vec<AppEntry>;

    /// The arguments that make scrcpy open this app where `display` says. A
    /// display of its own has `size` if it is known (the orientation the device
    /// is held in now), else scrcpy's default. `keep_awake` also asks scrcpy to
    /// keep the device from going to sleep while the window is open.
    fn start_app_args(
        &self,
        package: &PackageName,
        display: AppDisplay,
        size: Option<ScreenSize>,
        keep_awake: bool,
    ) -> Vec<String>;

    /// The arguments that make scrcpy send the keyboard this way. Empty for the
    /// default (`Sdk`), which is what scrcpy does without any.
    fn keyboard_args(&self, mode: KeyboardMode) -> Vec<String>;

    /// The arguments that set the bit rate of the video; empty for scrcpy's own
    /// choice.
    fn video_bit_rate_args(&self, rate: BitRateMbps) -> Vec<String>;

    /// The arguments that limit the longest side of the video; empty for the
    /// device's own size.
    fn max_size_args(&self, size: MaxSize) -> Vec<String>;

    /// Where the control socket is among the sockets that follow the video
    /// one, counting from 0 for the first of them (audio comes before control);
    /// `None` if this plan has no control socket.
    fn control_socket_after_video(&self, plan: &StreamPlan) -> Option<usize>;
}

/// The adapter for a scrcpy version, if one has been checked against it.
pub fn protocol_for(version: &ScrcpyVersion) -> Option<Box<dyn ScrcpyProtocol>> {
    // Every adapter there is; a new version's adapter is added here.
    let adapters: [Box<dyn ScrcpyProtocol>; 1] = [Box::new(super::v5::V5)];
    adapters.into_iter().find(|adapter| adapter.supported_versions().contains(version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_versions_that_were_checked_against_real_traffic_have_an_adapter() {
        let v = |t: &str| ScrcpyVersion::parse(t).unwrap();
        assert!(protocol_for(&v("5.0.1")).is_some());
        // Neighbours were never checked: no guessing that they behave the same.
        for unchecked in ["5.0.0", "5.0.2", "5.1.0", "4.9.9", "6.0.0", "3.3.4"] {
            assert!(protocol_for(&v(unchecked)).is_none(), "{unchecked}");
        }
    }

    #[test]
    fn an_adapter_lists_the_versions_it_serves() {
        let adapter = protocol_for(&ScrcpyVersion::new(5, 0, 1)).unwrap();
        assert_eq!(adapter.supported_versions(), [ScrcpyVersion::new(5, 0, 1)]);
    }
}
