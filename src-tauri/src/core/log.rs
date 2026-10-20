//! The system's debug log: what the core did and why, kept in memory (the
//! newest lines) and shown live while the debug console is open.
//!
//! Lines carry a typed code and a level; the message is for people to read and
//! nothing branches on it. Secrets (pairing codes) are never written.

use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// How serious or how detailed a line is. Ordered from the most important.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}

/// What a line is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogCode {
    /// Looking at the devices failed (adb could not be asked).
    PollFailed,
    /// adb was told to connect to or drop a known device, on its own.
    AutoConnect,
    /// The user asked to connect to an address.
    ConnectAttempt,
    ConnectOutcome,
    /// The user sent a pairing code (the code is not logged).
    PairAttempt,
    PairOutcome,
    SessionStarted,
    SessionEnded,
    /// A line scrcpy printed before its window ended.
    ScrcpyOutput,
    ButtonPressed,
    ButtonFailed,
    /// A file sent to a device, or that could not be.
    Transfer,
    SettingsSaved,
    /// A proxy is listening for a screen's scrcpy.
    ProxyStarted,
    ProxyStopped,
    /// The proxy mode is on but this screen opens without it.
    ProxyUnavailable,
    /// The video stream was not understood; it is no longer taken apart, but
    /// scrcpy still gets everything.
    ProxyStreamError,
    /// Which connection of scrcpy is which socket.
    ProxySocket,
    /// An add-on was installed, removed or given other permissions, or did something.
    Addon,
    /// A control message of ours was written to the device.
    ControlWritten,
    /// A control message of ours waits for scrcpy's own message to end.
    ControlWaiting,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LogLine {
    /// 1 for the first line ever; it keeps counting when old lines are dropped.
    pub seq: u64,
    /// Unix time in milliseconds.
    pub at_ms: u64,
    pub level: LogLevel,
    pub code: LogCode,
    pub message: String,
}

/// Where lines go while the console is open.
pub type LiveSink = Arc<dyn Fn(LogLine) + Send + Sync>;

struct Inner {
    capacity: usize,
    next_seq: u64,
    lines: VecDeque<LogLine>,
    live: Option<LiveSink>,
}

/// The log. Cloning gives another handle to the same log.
#[derive(Clone)]
pub struct Logger {
    inner: Arc<Mutex<Inner>>,
}

impl Logger {
    /// A log that keeps the newest `capacity` lines.
    pub fn new(capacity: usize) -> Self {
        Logger {
            inner: Arc::new(Mutex::new(Inner { capacity, next_seq: 1, lines: VecDeque::new(), live: None })),
        }
    }

    /// Writes a line. The live sink is called while the log is locked, so lines
    /// reach it in order and none falls between `attach` and what it hands over.
    pub fn log(&self, level: LogLevel, code: LogCode, message: impl Into<String>) {
        let mut inner = self.inner.lock().unwrap();
        let at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let line = LogLine { seq: inner.next_seq, at_ms, level, code, message: message.into() };
        inner.next_seq += 1;
        if inner.capacity > 0 {
            if inner.lines.len() == inner.capacity {
                inner.lines.pop_front();
            }
            inner.lines.push_back(line.clone());
        }
        if let Some(live) = &inner.live {
            live(line);
        }
    }

    /// The kept lines, oldest first.
    pub fn history(&self) -> Vec<LogLine> {
        self.inner.lock().unwrap().lines.iter().cloned().collect()
    }

    /// Starts delivering new lines to `live` and returns what is kept so far.
    /// Nothing is missed or delivered twice between the two.
    pub fn attach(&self, live: LiveSink) -> Vec<LogLine> {
        let mut inner = self.inner.lock().unwrap();
        inner.live = Some(live);
        inner.lines.iter().cloned().collect()
    }

    /// Stops delivering lines; they are still kept.
    pub fn detach(&self) {
        self.inner.lock().unwrap().live = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(lines: &[LogLine]) -> Vec<(u64, LogCode)> {
        lines.iter().map(|l| (l.seq, l.code)).collect()
    }

    #[test]
    fn only_the_newest_lines_are_kept_oldest_first() {
        let log = Logger::new(3);
        for code in [
            LogCode::PollFailed,
            LogCode::AutoConnect,
            LogCode::ConnectAttempt,
            LogCode::ConnectOutcome,
            LogCode::PairAttempt,
        ] {
            log.log(LogLevel::Info, code, "x");
        }
        // Lines 1 and 2 were dropped; the numbers did not start over.
        assert_eq!(
            codes(&log.history()),
            [(3, LogCode::ConnectAttempt), (4, LogCode::ConnectOutcome), (5, LogCode::PairAttempt)]
        );
    }

    #[test]
    fn a_line_keeps_what_it_was_given() {
        let log = Logger::new(10);
        log.log(LogLevel::Warn, LogCode::ButtonFailed, String::from("exit code 1"));
        let line = log.history().remove(0);
        assert_eq!((line.seq, line.level, line.code, line.message.as_str()), (1, LogLevel::Warn, LogCode::ButtonFailed, "exit code 1"));
        // Unix milliseconds: after the year 2020.
        assert!(line.at_ms > 1_577_836_800_000, "{}", line.at_ms);
    }

    #[test]
    fn lines_are_delivered_live_only_between_attach_and_detach() {
        let log = Logger::new(10);
        log.log(LogLevel::Info, LogCode::SettingsSaved, "before");
        let delivered: Arc<Mutex<Vec<LogLine>>> = Arc::default();
        let sink = delivered.clone();
        let history = log.attach(Arc::new(move |line| sink.lock().unwrap().push(line)));
        // What happened before is handed over once, not delivered again.
        assert_eq!(codes(&history), [(1, LogCode::SettingsSaved)]);
        log.log(LogLevel::Info, LogCode::SessionStarted, "while attached");
        log.detach();
        log.log(LogLevel::Info, LogCode::SessionEnded, "after");
        assert_eq!(codes(&delivered.lock().unwrap()), [(2, LogCode::SessionStarted)]);
        // Nothing is lost by not being delivered.
        assert_eq!(
            codes(&log.history()),
            [(1, LogCode::SettingsSaved), (2, LogCode::SessionStarted), (3, LogCode::SessionEnded)]
        );
    }

    #[test]
    fn a_clone_is_the_same_log() {
        let log = Logger::new(10);
        let other = log.clone();
        other.log(LogLevel::Debug, LogCode::ButtonPressed, "x");
        assert_eq!(codes(&log.history()), [(1, LogCode::ButtonPressed)]);
    }

    #[test]
    fn levels_are_ordered_from_the_most_important_and_written_in_lower_case() {
        assert!(LogLevel::Error < LogLevel::Warn && LogLevel::Warn < LogLevel::Info && LogLevel::Info < LogLevel::Debug);
        for (level, text) in
            [(LogLevel::Error, "error"), (LogLevel::Warn, "warn"), (LogLevel::Info, "info"), (LogLevel::Debug, "debug")]
        {
            assert_eq!(serde_json::to_value(level).unwrap(), text);
        }
        assert_eq!(serde_json::to_value(LogCode::ConnectOutcome).unwrap(), "connect_outcome");
    }

    #[test]
    fn a_line_crosses_to_the_ui_with_these_fields() {
        let log = Logger::new(10);
        log.log(LogLevel::Info, LogCode::PairAttempt, "pairing");
        let json = serde_json::to_value(&log.history()[0]).unwrap();
        assert_eq!(json["seq"], 1);
        assert_eq!(json["level"], "info");
        assert_eq!(json["code"], "pair_attempt");
        assert_eq!(json["message"], "pairing");
        assert!(json["at_ms"].is_u64());
    }
}
