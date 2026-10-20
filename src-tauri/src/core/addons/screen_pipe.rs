//! Feeds the live video of a screen to a program of an add-on (the permission `screen_capture`).
//!
//! What the program gets on its standard input is the H.264 stream as the proxy sees it, in Annex B form: the
//! parameter sets, then every frame, each as the device encoded it and nothing else (the picture size is in the
//! parameter sets). The program decodes it with whatever it likes (`ffmpeg`, PyAV, OpenCV...); this program has no
//! decoder, and the video does not pass through the add-on's script.
//!
//! A program that reads slower than the screen changes cannot be waited for. A piece that cannot be written
//! (`WriteError::Full`), or that the subscription lost (`RecvError::Lagged`), breaks the frames that follow it, so
//! those are dropped too, up to the next key frame, which can be decoded alone.

use crate::core::addons::process::WriteError;
use crate::core::apps::{PackageName, SessionKey};
use crate::core::scrcpy::protocol::{PacketKind, VideoEvent};
use crate::core::scrcpy::proxy::VideoSubscription;
use tokio::sync::broadcast::error::RecvError;

/// Why a screen was not fed to a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipeError {
    /// The device has no screen open that can be followed.
    NoScreen,
    /// The add-on has no such program running.
    NoProcess,
}

/// The name a screen goes by inside the program: the id of the device for its own screen, and the device with the
/// package for the window of an app. `None` when `app` is not a package name (no screen can have such a name).
pub fn screen_key(device: &str, app: Option<&str>) -> Option<String> {
    match app {
        None => Some(device.to_string()),
        Some(package) => PackageName::parse(package).map(|package| SessionKey::of_app(device, &package)),
    }
}

/// What became of one event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fed {
    /// Its bytes were written, or it is not something that is written (the name, the codec, the size).
    Handled,
    /// It was dropped, to be written later by a key frame.
    Dropped,
    /// The program is not running any more: nothing more should be fed.
    Ended,
}

/// Decides which packets are written, keeping to the rule above.
#[derive(Debug, Default)]
pub struct VideoFeeder {
    skipping: bool,
}

impl VideoFeeder {
    pub fn new() -> Self {
        VideoFeeder::default()
    }

    /// Something was lost between the stream and here: the frames that follow cannot be decoded until a key frame.
    pub fn lost(&mut self) {
        self.skipping = true;
    }

    /// Writes what the event carries, by `write`.
    pub fn feed(&mut self, event: &VideoEvent, write: &mut dyn FnMut(Vec<u8>) -> Result<(), WriteError>) -> Fed {
        let VideoEvent::Packet(packet) = event else { return Fed::Handled };
        // A key frame can be decoded alone, so it ends a stretch of dropped frames; the frames after a drop cannot.
        if packet.kind == PacketKind::Key {
            self.skipping = false;
        }
        if self.skipping && packet.kind == PacketKind::Delta {
            return Fed::Dropped;
        }
        match write(packet.payload.clone()) {
            Ok(()) => Fed::Handled,
            Err(WriteError::Full) => {
                self.skipping = true;
                Fed::Dropped
            }
            Err(WriteError::NotRunning) => Fed::Ended,
        }
    }
}

/// How feeding ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PumpEnd {
    /// The screen's stream ended (the screen was closed).
    StreamEnded,
    /// The program is not running any more.
    ProcessEnded,
}

/// Feeds a subscription (what the stream has so far, then every new event) by `write`, until the stream or the
/// program ends.
pub async fn pump(
    subscription: VideoSubscription,
    mut write: impl FnMut(Vec<u8>) -> Result<(), WriteError> + Send,
) -> PumpEnd {
    let VideoSubscription { initial, mut receiver } = subscription;
    let mut feeder = VideoFeeder::new();
    for event in &initial {
        if feeder.feed(event, &mut write) == Fed::Ended {
            return PumpEnd::ProcessEnded;
        }
    }
    loop {
        match receiver.recv().await {
            Ok(event) => {
                if feeder.feed(&event, &mut write) == Fed::Ended {
                    return PumpEnd::ProcessEnded;
                }
            }
            Err(RecvError::Lagged(_)) => feeder.lost(),
            Err(RecvError::Closed) => return PumpEnd::StreamEnded,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::scrcpy::protocol::{VideoCodec, VideoPacket};
    use std::sync::{Arc, Mutex};
    use tokio::sync::broadcast;

    // The bytes are made up: the feeder only passes payloads on, it does not read them.

    fn packet(kind: PacketKind, payload: &[u8]) -> VideoEvent {
        VideoEvent::Packet(VideoPacket { kind, pts_us: 0, payload: payload.to_vec() })
    }

    /// What was written, and a `write` that records it and fails with the answers it was given, in order
    /// (`Ok` when they run out).
    struct Sink {
        written: Vec<Vec<u8>>,
        answers: Vec<Result<(), WriteError>>,
    }

    impl Sink {
        fn new(answers: Vec<Result<(), WriteError>>) -> Self {
            Sink { written: Vec::new(), answers }
        }
        fn write(&mut self, bytes: Vec<u8>) -> Result<(), WriteError> {
            let answer = if self.answers.is_empty() { Ok(()) } else { self.answers.remove(0) };
            if answer.is_ok() {
                self.written.push(bytes);
            }
            answer
        }
    }

    fn feed_all(feeder: &mut VideoFeeder, sink: &mut Sink, events: &[VideoEvent]) -> Vec<Fed> {
        events.iter().map(|e| feeder.feed(e, &mut |bytes| sink.write(bytes))).collect()
    }

    #[test]
    fn a_screen_is_the_device_or_the_window_of_an_app_on_it_and_a_name_that_is_not_a_package_is_no_screen() {
        assert_eq!(screen_key("d1", None).as_deref(), Some("d1"));
        assert_eq!(screen_key("d1", Some("com.example.app")).as_deref(), Some("d1#app:com.example.app"));
        // Only a package name is an app: nothing else is put into the name of a screen.
        for bad in ["", "app", "com.example.", "+com.example.app", "com example", "com.example.app#app:x"] {
            assert_eq!(screen_key("d1", Some(bad)), None, "{bad:?}");
        }
    }

    #[test]
    fn only_the_packets_are_written_each_as_it_is_and_in_order() {
        let mut feeder = VideoFeeder::new();
        let mut sink = Sink::new(vec![]);
        let events = [
            VideoEvent::DeviceName("name".into()),
            VideoEvent::Codec(VideoCodec::H264),
            VideoEvent::Session { width: 320, height: 200 },
            packet(PacketKind::Config, &[0, 0, 0, 1, 0x67]),
            packet(PacketKind::Key, &[0, 0, 0, 1, 0x65, 9]),
            packet(PacketKind::Delta, &[0, 0, 0, 1, 0x41, 8]),
        ];
        let fed = feed_all(&mut feeder, &mut sink, &events);
        assert_eq!(fed, vec![Fed::Handled; 6]);
        assert_eq!(sink.written, vec![vec![0, 0, 0, 1, 0x67], vec![0, 0, 0, 1, 0x65, 9], vec![0, 0, 0, 1, 0x41, 8]]);
    }

    #[test]
    fn a_frame_that_could_not_be_written_takes_the_frames_after_it_until_the_next_key_frame() {
        let mut feeder = VideoFeeder::new();
        // The second delta finds the program full; the writes after it would all have gone through.
        let mut sink = Sink::new(vec![Ok(()), Ok(()), Err(WriteError::Full)]);
        let events = [
            packet(PacketKind::Key, &[1]),
            packet(PacketKind::Delta, &[2]),
            packet(PacketKind::Delta, &[3]),
            packet(PacketKind::Delta, &[4]),
            packet(PacketKind::Key, &[5]),
            packet(PacketKind::Delta, &[6]),
        ];
        let fed = feed_all(&mut feeder, &mut sink, &events);
        assert_eq!(fed, vec![Fed::Handled, Fed::Handled, Fed::Dropped, Fed::Dropped, Fed::Handled, Fed::Handled]);
        assert_eq!(sink.written, vec![vec![1], vec![2], vec![5], vec![6]]);
    }

    #[test]
    fn what_the_stream_lost_is_treated_the_same_way() {
        let mut feeder = VideoFeeder::new();
        let mut sink = Sink::new(vec![]);
        feed_all(&mut feeder, &mut sink, &[packet(PacketKind::Key, &[1])]);
        feeder.lost();
        let fed = feed_all(&mut feeder, &mut sink, &[packet(PacketKind::Delta, &[2]), packet(PacketKind::Key, &[3])]);
        assert_eq!(fed, vec![Fed::Dropped, Fed::Handled]);
        assert_eq!(sink.written, vec![vec![1], vec![3]]);
    }

    #[test]
    fn a_program_that_is_gone_ends_the_feeding() {
        let mut feeder = VideoFeeder::new();
        let mut sink = Sink::new(vec![Err(WriteError::NotRunning)]);
        let events = [VideoEvent::Session { width: 320, height: 200 }, packet(PacketKind::Key, &[1])];
        let fed = feed_all(&mut feeder, &mut sink, &events);
        assert_eq!(fed, vec![Fed::Handled, Fed::Ended]);
        assert!(sink.written.is_empty());
    }

    fn subscription(initial: Vec<VideoEvent>, capacity: usize) -> (broadcast::Sender<VideoEvent>, VideoSubscription) {
        let (sender, receiver) = broadcast::channel(capacity);
        (sender, VideoSubscription { initial, receiver })
    }

    /// A `write` that records into a list the test can read afterwards.
    type Written = Arc<Mutex<Vec<Vec<u8>>>>;

    fn recorder(answer: Result<(), WriteError>) -> (Written, impl FnMut(Vec<u8>) -> Result<(), WriteError> + Send) {
        let written: Written = Arc::default();
        let seen = written.clone();
        (written, move |bytes| {
            if answer.is_ok() {
                seen.lock().unwrap().push(bytes);
            }
            answer
        })
    }

    #[tokio::test]
    async fn what_the_stream_has_so_far_comes_first_then_every_new_event_and_feeding_ends_with_the_stream() {
        let (sender, subscription) = subscription(vec![packet(PacketKind::Config, &[1]), packet(PacketKind::Key, &[2])], 16);
        let (written, write) = recorder(Ok(()));
        sender.send(packet(PacketKind::Delta, &[3])).unwrap();
        sender.send(packet(PacketKind::Delta, &[4])).unwrap();
        drop(sender);
        let end = tokio::time::timeout(std::time::Duration::from_secs(5), pump(subscription, write)).await;
        assert_eq!(end, Ok(PumpEnd::StreamEnded));
        assert_eq!(*written.lock().unwrap(), vec![vec![1], vec![2], vec![3], vec![4]]);
    }

    #[tokio::test]
    async fn feeding_ends_when_the_program_is_gone_though_the_stream_goes_on() {
        let (sender, subscription) = subscription(vec![packet(PacketKind::Key, &[1])], 16);
        let (_, write) = recorder(Err(WriteError::NotRunning));
        let end = tokio::time::timeout(std::time::Duration::from_secs(5), pump(subscription, write)).await;
        assert_eq!(end, Ok(PumpEnd::ProcessEnded));
        drop(sender);
    }

    #[tokio::test]
    async fn a_pump_that_fell_behind_the_stream_goes_on_from_the_next_key_frame() {
        // Room for two events: of the six sent before the pump reads, only the last two are left.
        let (sender, subscription) = subscription(vec![], 2);
        for event in [
            packet(PacketKind::Key, &[1]),
            packet(PacketKind::Delta, &[2]),
            packet(PacketKind::Delta, &[3]),
            packet(PacketKind::Delta, &[4]),
            packet(PacketKind::Key, &[5]),
            packet(PacketKind::Delta, &[6]),
        ] {
            sender.send(event).unwrap();
        }
        drop(sender);
        let (written, write) = recorder(Ok(()));
        let end = tokio::time::timeout(std::time::Duration::from_secs(5), pump(subscription, write)).await;
        assert_eq!(end, Ok(PumpEnd::StreamEnded));
        assert_eq!(*written.lock().unwrap(), vec![vec![5], vec![6]]);
    }
}
