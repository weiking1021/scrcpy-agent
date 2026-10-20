//! Who is connecting to which device right now, so that two connections to one device are never made at once
//! and a device that does not answer holds up only itself.
//!
//! A connection is begun by asking for a permit for the address (`ConnectGate::try_begin`): the permit is for the
//! IP address, not the port, because the port of one phone changes and a new port is the same phone. The permit is
//! given back when it is dropped, whichever way the work ended (it finished, it failed, it panicked, its task was
//! cancelled); there is no other way to give it back. Asking never waits: a device that is being connected to is
//! skipped (or told so), so nobody queues behind it. A permit is held for `CONNECT_LIMIT` at most (`run_gated`), so a
//! connection that never ends does not shut the device out for good.

use std::collections::HashSet;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// The longest a permit is held. A device whose port answered connects in a moment; this is for one that does not.
pub const CONNECT_LIMIT: Duration = Duration::from_secs(10);

/// The addresses that are being connected to.
#[derive(Default)]
pub struct ConnectGate {
    busy: Mutex<HashSet<IpAddr>>,
}

/// The right to connect to one device (one IP address); given back when dropped.
pub struct ConnectPermit {
    gate: Arc<ConnectGate>,
    ip: IpAddr,
}

/// Why work was not done or not finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateError {
    /// Another connection to this device is going on.
    Busy,
    /// The work did not end within the limit; it was given up and the permit given back.
    TimedOut,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ConnectGate {
    pub fn new() -> Arc<Self> {
        Arc::new(ConnectGate::default())
    }

    /// The permit for the device at this address, unless a connection to it is going on. Never waits.
    pub fn try_begin(self: &Arc<Self>, address: SocketAddr) -> Option<ConnectPermit> {
        let ip = address.ip();
        // Looking and taking are one step under one lock, so two askers cannot both be let through. The lock is
        // never held across an `await`.
        if locked(&self.busy).insert(ip) { Some(ConnectPermit { gate: self.clone(), ip }) } else { None }
    }

    /// Whether a connection to the device at this IP address is going on.
    pub fn is_busy(&self, ip: IpAddr) -> bool {
        locked(&self.busy).contains(&ip)
    }
}

impl Drop for ConnectPermit {
    fn drop(&mut self) {
        locked(&self.gate.busy).remove(&self.ip);
    }
}

/// Does `work` while holding the permit for `address`, for at most `limit`.
pub async fn run_gated<T>(
    gate: &Arc<ConnectGate>,
    address: SocketAddr,
    limit: Duration,
    work: impl Future<Output = T>,
) -> Result<T, GateError> {
    let _permit = gate.try_begin(address).ok_or(GateError::Busy)?;
    tokio::time::timeout(limit, work).await.map_err(|_| GateError::TimedOut)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    // The addresses are the ones documentation uses (RFC 5737); nothing is connected to.
    fn at(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    #[test]
    fn a_device_has_one_permit_at_a_time_whatever_its_port_and_other_devices_have_their_own() {
        let gate = ConnectGate::new();
        let first = gate.try_begin(at("192.0.2.1:40000"));
        assert!(first.is_some(), "the first connection to a device is let through");
        assert!(gate.is_busy("192.0.2.1".parse().unwrap()));
        assert!(gate.try_begin(at("192.0.2.1:41000")).is_none(), "the same device on another port is the same device");
        assert!(gate.try_begin(at("192.0.2.1:40000")).is_none());
        let other = gate.try_begin(at("192.0.2.2:40000"));
        assert!(other.is_some(), "another device is not held up by this one");
        assert!(!gate.is_busy("192.0.2.3".parse().unwrap()));
    }

    #[test]
    fn dropping_the_permit_gives_it_back_and_only_that_one() {
        let gate = ConnectGate::new();
        let a = gate.try_begin(at("192.0.2.1:1")).expect("a permit");
        let b = gate.try_begin(at("192.0.2.2:1")).expect("a permit");
        drop(a);
        assert!(!gate.is_busy("192.0.2.1".parse().unwrap()));
        assert!(gate.is_busy("192.0.2.2".parse().unwrap()));
        assert!(gate.try_begin(at("192.0.2.1:2")).is_some());
        drop(b);
        assert!(!gate.is_busy("192.0.2.2".parse().unwrap()));
    }

    #[test]
    fn a_permit_is_given_back_when_the_work_that_held_it_panics() {
        let gate = ConnectGate::new();
        let for_thread = gate.clone();
        let ended = std::thread::spawn(move || {
            let _permit = for_thread.try_begin(at("192.0.2.1:1")).expect("a permit");
            panic!("the work failed");
        })
        .join();
        assert!(ended.is_err());
        assert!(gate.try_begin(at("192.0.2.1:2")).is_some(), "the permit stayed with the panicked work");
    }

    #[test]
    fn of_many_that_ask_at_once_for_one_device_exactly_one_gets_it() {
        let gate = ConnectGate::new();
        let start = Arc::new(std::sync::Barrier::new(16));
        let handles: Vec<_> = (0..16)
            .map(|i| {
                let (gate, start) = (gate.clone(), start.clone());
                std::thread::spawn(move || {
                    start.wait();
                    // The ports differ, the device is one.
                    gate.try_begin(at(&format!("192.0.2.1:{}", 40000 + i)))
                })
            })
            .collect();
        let permits: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(permits.iter().filter(|p| p.is_some()).count(), 1);
    }

    #[tokio::test]
    async fn work_that_is_let_through_gives_its_result_and_the_permit_back() {
        let gate = ConnectGate::new();
        let done = run_gated(&gate, at("192.0.2.1:1"), Duration::from_secs(5), async { 7 }).await;
        assert_eq!(done, Ok(7));
        assert!(!gate.is_busy("192.0.2.1".parse().unwrap()));
    }

    #[tokio::test]
    async fn work_for_a_device_that_is_busy_is_not_started() {
        let gate = ConnectGate::new();
        let _held = gate.try_begin(at("192.0.2.1:1")).expect("a permit");
        let started = Arc::new(AtomicBool::new(false));
        let flag = started.clone();
        let done = run_gated(&gate, at("192.0.2.1:2"), Duration::from_secs(5), async move {
            flag.store(true, Ordering::SeqCst);
        })
        .await;
        assert_eq!(done, Err(GateError::Busy));
        assert!(!started.load(Ordering::SeqCst), "the work ran although the device was busy");
    }

    #[tokio::test]
    async fn work_that_does_not_end_is_given_up_at_the_limit_and_the_device_is_free_again() {
        let gate = ConnectGate::new();
        let began = std::time::Instant::now();
        let done = run_gated(&gate, at("192.0.2.1:1"), Duration::from_millis(50), std::future::pending::<()>()).await;
        assert_eq!(done, Err(GateError::TimedOut));
        assert!(began.elapsed() < Duration::from_secs(5), "took {:?}", began.elapsed());
        assert!(gate.try_begin(at("192.0.2.1:2")).is_some(), "the permit stayed with the work that was given up");
    }

    #[tokio::test]
    async fn a_device_that_does_not_answer_holds_up_only_itself() {
        let gate = ConnectGate::new();
        let stuck_gate = gate.clone();
        let stuck = tokio::spawn(async move {
            run_gated(&stuck_gate, at("192.0.2.1:1"), Duration::from_secs(30), std::future::pending::<()>()).await
        });
        // Give the stuck work the moment it needs to take its permit.
        for _ in 0..200 {
            if gate.is_busy("192.0.2.1".parse().unwrap()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(gate.is_busy("192.0.2.1".parse().unwrap()), "the stuck work never took its permit");
        let other = run_gated(&gate, at("192.0.2.2:1"), Duration::from_secs(5), async { "connected" }).await;
        assert_eq!(other, Ok("connected"));
        // Its task being cancelled gives its permit back too.
        stuck.abort();
        let _ = stuck.await;
        assert!(!gate.is_busy("192.0.2.1".parse().unwrap()));
    }
}
