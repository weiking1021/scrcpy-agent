//! The timers of one add-on (`setTimeout`, `setInterval`): when each is due. The script runtime and
//! the thread that waits for them are `runtime`'s; this only keeps the table, so it is the same on
//! every system and does not read the clock itself.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// There are already as many timers as the limit says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitReached;

struct Timer {
    due: Instant,
    /// For an interval, the time between two runs (never less than the least).
    every: Option<Duration>,
}

pub struct Timers {
    limit: usize,
    min_interval: Duration,
    timers: HashMap<u32, Timer>,
}

impl Timers {
    pub fn new(limit: usize, min_interval: Duration) -> Timers {
        Timers { limit, min_interval, timers: HashMap::new() }
    }

    /// Starts a timer that is due after `delay`; a `repeat` one is due again and again at that interval,
    /// made at least as long as the least.
    pub fn start(&mut self, key: u32, delay: Duration, repeat: bool, now: Instant) -> Result<(), LimitReached> {
        if self.timers.len() >= self.limit {
            return Err(LimitReached);
        }
        let delay = if repeat { delay.max(self.min_interval) } else { delay };
        self.timers.insert(key, Timer { due: now + delay, every: repeat.then_some(delay) });
        Ok(())
    }

    pub fn clear(&mut self, key: u32) {
        self.timers.remove(&key);
    }

    /// When the next timer is due, if there is one.
    pub fn next_due(&self) -> Option<Instant> {
        self.timers.values().map(|t| t.due).min()
    }

    /// The timers that are due at `now`, the earliest first. A timer that runs once is gone after this;
    /// an interval is due again one interval from now (so a script that was busy does not get a burst).
    pub fn take_due(&mut self, now: Instant) -> Vec<u32> {
        let mut due: Vec<(Instant, u32)> =
            self.timers.iter().filter(|(_, t)| t.due <= now).map(|(&key, t)| (t.due, key)).collect();
        due.sort();
        for (_, key) in &due {
            match self.timers.get_mut(key).and_then(|t| t.every.map(|every| (t, every))) {
                Some((timer, every)) => timer.due = now + every,
                None => {
                    self.timers.remove(key);
                }
            }
        }
        due.into_iter().map(|(_, key)| key).collect()
    }
}
