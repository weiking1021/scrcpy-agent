//! How much an add-on may use. A script that goes beyond is stopped; it never takes the program with it.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The most memory the script runtime may hold.
    pub memory_bytes: usize,
    /// How long `main.js` may run (until it returns; work it started with promises goes on after).
    pub init_time: Duration,
    /// How long one callback may run at a time, from the moment it starts (waiting for the host counts).
    pub call_time: Duration,
    /// The most characters of a line of the log; a longer one is cut and ends with `…`.
    pub max_log_chars: usize,
    /// The most timers (`setTimeout`, `setInterval`) that wait at one time.
    pub max_timers: usize,
    /// The shortest time between two runs of an interval; a shorter one is made this long.
    pub min_interval: Duration,
    /// The most events that wait for an add-on that is busy; when there are more, the oldest are dropped.
    pub max_pending_events: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            memory_bytes: 32 * 1024 * 1024,
            init_time: Duration::from_secs(5),
            call_time: Duration::from_secs(5),
            max_log_chars: 2000,
            max_timers: 100,
            min_interval: Duration::from_millis(10),
            max_pending_events: 100,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limits_of_an_add_on_are_these_unless_told_otherwise() {
        assert_eq!(
            Limits::default(),
            Limits {
                memory_bytes: 32 * 1024 * 1024,
                init_time: Duration::from_secs(5),
                call_time: Duration::from_secs(5),
                max_log_chars: 2000,
                max_timers: 100,
                min_interval: Duration::from_millis(10),
                max_pending_events: 100,
            }
        );
    }
}
