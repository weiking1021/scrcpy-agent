//! Reaching a paired device without mDNS: a phone or tablet that is asleep does not announce itself, but its port may
//! still answer. For a paired device that is not connected, has no announcement to follow and has an address it
//! was last connected at, the address is tried (the port is asked first, then adb); when that does not bring it
//! back, it is left alone for a while, longer each time, and mDNS goes on as before.
//!
//! This is the plan only: which devices to try now. It touches no network and reads the time it is given, so
//! it is tested with a clock of its own.

use super::{PropsMap, entry_id};
use crate::core::adb::{AdbEntry, AdbState, MdnsKind, MdnsService, parse_user_address};
use crate::core::model::KnownDevice;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// The wait after the first failure, doubled after each (see `backoff_after`), up to `BACKOFF_CAP`.
pub const BACKOFF_FIRST: Duration = Duration::from_secs(15);
pub const BACKOFF_CAP: Duration = Duration::from_secs(300);

/// The most devices tried in one round.
pub const MAX_PER_ROUND: usize = 4;

/// How long a device is left alone after this many failures in a row (the first is 1).
pub fn backoff_after(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    BACKOFF_FIRST.saturating_mul(1u32 << doublings).min(BACKOFF_CAP)
}

/// How each paired device has fared since it was last connected.
#[derive(Default)]
pub struct FallbackState {
    by_device: HashMap<String, (u32, Instant)>,
}

impl FallbackState {
    pub fn new() -> Self {
        FallbackState::default()
    }

    /// The address was tried and the device did not come back.
    pub fn record_failure(&mut self, id: &str, now: Instant) {
        let failures = self.by_device.get(id).map_or(0, |(failures, _)| *failures).saturating_add(1);
        self.by_device.insert(id.to_string(), (failures, now + backoff_after(failures)));
    }

    /// The device is connected (or announced again): it starts from the first wait next time.
    pub fn record_success(&mut self, id: &str) {
        self.by_device.remove(id);
    }

    /// Whether the device may be tried at `now`.
    pub fn may_try(&self, id: &str, now: Instant) -> bool {
        self.by_device.get(id).is_none_or(|(_, until)| now >= *until)
    }
}

/// A device to try, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub id: String,
    pub address: SocketAddr,
    /// A leftover entry of adb is at the address (offline): it is dropped first, or adb would do nothing.
    pub drop_stale_entry: bool,
}

/// The paired devices to try now, the most recently used first, at most `max`.
pub fn plan_fallback(
    known: &[KnownDevice],
    entries: &[AdbEntry],
    mdns: &[MdnsService],
    props: &PropsMap,
    state: &FallbackState,
    now: Instant,
    max: usize,
) -> Vec<Candidate> {
    let mut candidates: Vec<(u64, Candidate)> = Vec::new();
    for device in known {
        // The address is read as an address (the same rule as the one a user types); text that is not one is not tried.
        let Some(address) = device.last_address.as_deref().and_then(parse_user_address) else { continue };
        let connected = entries.iter().any(|e| {
            e.state == AdbState::Device && (entry_id(e, props, mdns) == device.id || e.network_address() == Some(address))
        });
        // A device that announces itself is connected by its announcement (at the port it announces).
        let announced = mdns.iter().any(|m| m.kind != MdnsKind::Pairing && m.serial().as_deref() == Some(device.id.as_str()));
        // Waiting for the approval on the phone: asking again only raises the prompt again.
        let waiting = entries.iter().any(|e| {
            e.network_address() == Some(address) && matches!(e.state, AdbState::Unauthorized | AdbState::Authorizing)
        });
        if connected || announced || waiting || !state.may_try(&device.id, now) {
            continue;
        }
        let drop_stale_entry = entries.iter().any(|e| e.network_address() == Some(address));
        candidates.push((device.last_seen, Candidate { id: device.id.clone(), address, drop_stale_entry }));
    }
    candidates.sort_by_key(|(last_seen, _)| std::cmp::Reverse(*last_seen));
    candidates.into_iter().map(|(_, candidate)| candidate).take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::adb::{Props, parse_devices, parse_mdns};

    // Every address is one that documentation uses (RFC 5737); the names and values are made up.

    fn props_of(serial: &str) -> Props {
        Props { serial: serial.into(), model: "Model".into(), android: "1".into(), sdk: Some(1) }
    }

    fn known(id: &str, address: Option<&str>, last_seen: u64) -> KnownDevice {
        KnownDevice {
            id: id.into(),
            model: "Model".into(),
            label: None,
            android: None,
            sdk: None,
            last_address: address.map(String::from),
            last_seen,
            first_seen: 0,
            settings: Default::default(),
            apps: Default::default(),
        }
    }

    fn plan(known: &[KnownDevice], devices: &str, mdns: &str, state: &FallbackState, props: &PropsMap) -> Vec<Candidate> {
        plan_fallback(known, &parse_devices(devices), &parse_mdns(mdns), props, state, Instant::now(), MAX_PER_ROUND)
    }

    fn ids(candidates: &[Candidate]) -> Vec<&str> {
        candidates.iter().map(|c| c.id.as_str()).collect()
    }

    #[test]
    fn the_wait_doubles_from_the_first_failure_up_to_the_cap() {
        let waits: Vec<u64> = (1..=7).map(|n| backoff_after(n).as_secs()).collect();
        assert_eq!(waits, [15, 30, 60, 120, 240, 300, 300]);
    }

    #[test]
    fn a_device_that_failed_is_left_alone_for_its_wait_and_a_success_starts_it_over() {
        let mut state = FallbackState::new();
        let t0 = Instant::now();
        assert!(state.may_try("aaa", t0), "a device nobody tried is let through");
        state.record_failure("aaa", t0);
        assert!(!state.may_try("aaa", t0 + Duration::from_secs(14)));
        assert!(state.may_try("aaa", t0 + Duration::from_secs(15)));
        state.record_failure("aaa", t0 + Duration::from_secs(15));
        assert!(!state.may_try("aaa", t0 + Duration::from_secs(44)), "the second wait is 30 seconds");
        assert!(state.may_try("aaa", t0 + Duration::from_secs(45)));
        assert!(state.may_try("bbb", t0), "another device has its own count");
        state.record_success("aaa");
        state.record_failure("aaa", t0 + Duration::from_secs(60));
        assert!(!state.may_try("aaa", t0 + Duration::from_secs(74)));
        assert!(state.may_try("aaa", t0 + Duration::from_secs(75)), "after a success the first wait is 15 seconds again");
    }

    #[test]
    fn a_paired_device_that_is_not_connected_and_not_announced_is_tried_at_the_address_it_was_last_connected_at() {
        let devices = [known("aaa", Some("192.0.2.10:40000"), 5)];
        let planned = plan(&devices, "", "", &FallbackState::new(), &PropsMap::new());
        assert_eq!(
            planned,
            vec![Candidate { id: "aaa".into(), address: "192.0.2.10:40000".parse().unwrap(), drop_stale_entry: false }]
        );
    }

    #[test]
    fn a_device_that_is_connected_by_usb_or_by_wifi_is_not_tried() {
        let devices = [known("aaa", Some("192.0.2.10:40000"), 5)];
        // (Without a connection it is tried: what follows is the difference the connection makes.)
        assert_eq!(ids(&plan(&devices, "", "", &FallbackState::new(), &PropsMap::new())), ["aaa"]);
        // By USB: the entry is the serial itself.
        let usb = plan(&devices, "aaa device usb:1-1 model:Model\n", "", &FallbackState::new(), &PropsMap::new());
        assert!(usb.is_empty(), "{usb:?}");
        // By Wi-Fi: the entry is the address, and the device told its serial.
        let mut props = PropsMap::new();
        props.insert("192.0.2.10:40000".into(), props_of("aaa"));
        let wifi = plan(&devices, "192.0.2.10:40000 device model:Model\n", "", &FallbackState::new(), &props);
        assert!(wifi.is_empty(), "{wifi:?}");
    }

    #[test]
    fn a_device_that_announces_itself_is_left_to_the_connection_by_its_announcement() {
        let devices = [known("aaa", Some("192.0.2.10:40000"), 5)];
        let announced = "adb-aaa-x1y2z3\t_adb-tls-connect._tcp.\t192.0.2.10:41000\n";
        assert!(plan(&devices, "", announced, &FallbackState::new(), &PropsMap::new()).is_empty());
        // A service for pairing is not the device being there to connect to.
        let pairing = "adb-aaa-x1y2z3\t_adb-tls-pairing._tcp.\t192.0.2.10:41000\n";
        assert_eq!(ids(&plan(&devices, "", pairing, &FallbackState::new(), &PropsMap::new())), ["aaa"]);
    }

    #[test]
    fn a_device_without_a_usable_address_is_not_tried() {
        let devices = [
            known("none", None, 9),
            known("text", Some("not an address"), 8),
            known("noport", Some("192.0.2.12"), 7),
            known("good", Some("192.0.2.13:40000"), 1),
        ];
        let planned = plan(&devices, "", "", &FallbackState::new(), &PropsMap::new());
        assert_eq!(ids(&planned), ["good"]);
    }

    #[test]
    fn a_device_that_waits_for_the_approval_on_the_phone_is_not_asked_again() {
        let devices = [known("aaa", Some("192.0.2.10:40000"), 5)];
        // (Without the entry it is tried: what follows is the difference the waiting makes.)
        assert_eq!(ids(&plan(&devices, "", "", &FallbackState::new(), &PropsMap::new())), ["aaa"]);
        for waiting in ["unauthorized", "authorizing"] {
            let listed = format!("192.0.2.10:40000 {waiting}\n");
            assert!(plan(&devices, &listed, "", &FallbackState::new(), &PropsMap::new()).is_empty(), "{waiting}");
        }
    }

    #[test]
    fn a_leftover_offline_entry_at_the_address_is_dropped_before_the_connection() {
        let devices = [known("aaa", Some("192.0.2.10:40000"), 5)];
        let planned = plan(&devices, "192.0.2.10:40000 offline\n", "", &FallbackState::new(), &PropsMap::new());
        assert_eq!(planned.len(), 1);
        assert!(planned[0].drop_stale_entry);
    }

    #[test]
    fn a_device_in_its_wait_is_not_tried_and_the_most_recently_used_come_first_up_to_the_most_allowed() {
        let devices = [
            known("old", Some("192.0.2.20:1"), 1),
            known("newest", Some("192.0.2.21:1"), 9),
            known("waiting", Some("192.0.2.22:1"), 8),
            known("middle", Some("192.0.2.23:1"), 5),
            known("fourth", Some("192.0.2.24:1"), 4),
            known("fifth", Some("192.0.2.25:1"), 3),
        ];
        let mut state = FallbackState::new();
        state.record_failure("waiting", Instant::now());
        let planned = plan(&devices, "", "", &state, &PropsMap::new());
        assert_eq!(ids(&planned), ["newest", "middle", "fourth", "fifth"], "{MAX_PER_ROUND} at most, the newest first");
    }
}
