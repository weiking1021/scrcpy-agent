//! Our own mDNS browser for adb services.
//!
//! `adb mdns services` is not reliable: on a real network it listed some
//! wireless-debugging devices and silently missed others that answered a
//! plain mDNS query. Browsing ourselves and merging the result with adb's
//! list closes that gap without installing anything on the device.

use crate::core::adb::{MdnsKind, MdnsService};
use anyhow::Result;
use mdns_sd::{IfKind, ScopedIp, ServiceDaemon, ServiceEvent};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

const SERVICE_TYPES: [&str; 3] = [
    "_adb-tls-connect._tcp.local.",
    "_adb-tls-pairing._tcp.local.",
    "_adb._tcp.local.",
];

pub struct MdnsBrowser {
    // Kept alive for as long as the browser exists.
    _daemon: ServiceDaemon,
    services: Arc<Mutex<HashMap<String, MdnsService>>>,
}

impl MdnsBrowser {
    /// Starts browsing in the background. Must be called inside a Tokio runtime.
    pub fn start() -> Result<Self> {
        let daemon = ServiceDaemon::new()?;
        // adb connects over IPv4, and IPv6 multicast fails on many setups.
        let _ = daemon.disable_interface(IfKind::IPv6);
        let services: Arc<Mutex<HashMap<String, MdnsService>>> = Default::default();
        for service_type in SERVICE_TYPES {
            let rx = daemon.browse(service_type)?;
            let services = services.clone();
            tokio::spawn(async move {
                while let Ok(event) = rx.recv_async().await {
                    apply(&services, event);
                }
            });
        }
        Ok(MdnsBrowser { _daemon: daemon, services })
    }

    pub fn snapshot(&self) -> Vec<MdnsService> {
        let mut v: Vec<_> = self.services.lock().unwrap().values().cloned().collect();
        v.sort_by(|a, b| a.instance.cmp(&b.instance));
        v
    }
}

fn apply(map: &Mutex<HashMap<String, MdnsService>>, event: ServiceEvent) {
    match event {
        ServiceEvent::ServiceResolved(info) => {
            // adb connects over IPv4 only.
            let ip = info.addresses.iter().find_map(|a| match a {
                ScopedIp::V4(v4) => Some(*v4.addr()),
                _ => None,
            });
            let Some(ip) = ip else { return };
            if let Some(service) = to_service(&info.ty_domain, &info.fullname, ip, info.port) {
                map.lock().unwrap().insert(info.fullname.clone(), service);
            }
        }
        ServiceEvent::ServiceRemoved(_, fullname) => {
            map.lock().unwrap().remove(&fullname);
        }
        _ => {}
    }
}

/// Builds a service from what the mDNS library reports: the service type
/// with its domain (`_adb-tls-connect._tcp.local.`) and the full instance
/// name (`<instance>.<service type and domain>`). `None` if it is not one of
/// the adb services or the names do not fit together.
fn to_service(ty_domain: &str, fullname: &str, ip: Ipv4Addr, port: u16) -> Option<MdnsService> {
    let kind = MdnsKind::parse(ty_domain)?;
    let instance = fullname.strip_suffix(ty_domain)?.strip_suffix('.')?;
    Some(MdnsService {
        instance: instance.to_string(),
        kind,
        address: SocketAddr::from((ip, port)),
    })
}

/// Combines our own result with adb's. When both know the same service
/// (same kind and instance name) ours wins, because adb's list can hold a
/// stale port after wireless debugging was toggled.
pub fn merge(own: Vec<MdnsService>, from_adb: Vec<MdnsService>) -> Vec<MdnsService> {
    let mut out = own;
    for s in from_adb {
        if !out.iter().any(|o| o.kind == s.kind && o.instance == s.instance) {
            out.push(s);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc(instance: &str, kind: MdnsKind, address: &str) -> MdnsService {
        MdnsService { instance: instance.into(), kind, address: address.parse().unwrap() }
    }

    #[test]
    fn own_result_wins_over_a_stale_adb_entry() {
        let own = vec![svc("adb-A-x", MdnsKind::Connect, "10.0.0.1:41900")];
        let adb = vec![
            svc("adb-A-x", MdnsKind::Connect, "10.0.0.1:40400"),
            svc("adb-B", MdnsKind::Legacy, "10.0.0.2:5555"),
        ];
        let m = merge(own, adb);
        assert_eq!(m.len(), 2);
        let stale: SocketAddr = "10.0.0.1:40400".parse().unwrap();
        let fresh: SocketAddr = "10.0.0.1:41900".parse().unwrap();
        assert!(m.iter().any(|s| s.address == fresh));
        assert!(!m.iter().any(|s| s.address == stale));
    }

    #[test]
    fn converts_what_the_mdns_library_reports() {
        // The shapes the library reports (the values are invented).
        let ip = Ipv4Addr::new(192, 168, 250, 139);
        let s = to_service(
            "_adb-tls-connect._tcp.local.",
            "adb-a1b2c3d4-Ab12Cd._adb-tls-connect._tcp.local.",
            ip,
            36600,
        )
        .unwrap();
        assert_eq!(s.kind, MdnsKind::Connect);
        assert_eq!(s.instance, "adb-a1b2c3d4-Ab12Cd");
        assert_eq!(s.address, "192.168.250.139:36600".parse().unwrap());
        assert_eq!(s.serial().as_deref(), Some("a1b2c3d4"));

        let legacy =
            to_service("_adb._tcp.local.", "adb-TESTSERIAL001._adb._tcp.local.", ip, 5555).unwrap();
        assert_eq!(legacy.kind, MdnsKind::Legacy);
        assert_eq!(legacy.serial().as_deref(), Some("TESTSERIAL001"));
    }

    #[test]
    fn rejects_services_that_are_not_adb_or_do_not_fit_together() {
        let ip = Ipv4Addr::new(10, 0, 0, 1);
        assert!(to_service("_http._tcp.local.", "x._http._tcp.local.", ip, 80).is_none());
        // The full name must end with the service type.
        assert!(to_service("_adb._tcp.local.", "x._other._tcp.local.", ip, 5555).is_none());
        // ...and be separated from the instance by a dot.
        assert!(to_service("_adb._tcp.local.", "x_adb._tcp.local.", ip, 5555).is_none());
    }

    /// Needs a real network with adb devices on it (it is skipped by default):
    /// `cargo test live_browse -- --ignored --nocapture`.
    /// It fails when nothing is found, so a broken browser cannot go unnoticed.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn live_browse() {
        let b = MdnsBrowser::start().unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(8)).await;
        let found = b.snapshot();
        for s in &found {
            println!("{:?} {} {}", s.kind, s.instance, s.address);
        }
        assert!(!found.is_empty(), "the mDNS browser found no adb service at all");
    }
}
