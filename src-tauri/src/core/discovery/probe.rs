//! Whether a device's port answers, asked of the network before adb is asked to connect: `adb connect` to an address
//! that does not answer can take more than twenty seconds, a plain connection is given up after two.
//!
//! The way of asking is behind `PortProbe`, so what plans connections is tested without a network.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::time::Duration;

/// How long the network is waited for.
pub const PROBE_LIMIT: Duration = Duration::from_secs(2);

pub trait PortProbe: Send + Sync + 'static {
    /// Whether a connection to the address is accepted within the limit of the probe.
    fn is_open(&self, address: SocketAddr) -> Pin<Box<dyn Future<Output = bool> + Send + '_>>;
}

/// Asks with a plain TCP connection.
pub struct TcpProbe {
    pub limit: Duration,
}

impl Default for TcpProbe {
    fn default() -> Self {
        TcpProbe { limit: PROBE_LIMIT }
    }
}

impl PortProbe for TcpProbe {
    fn is_open(&self, address: SocketAddr) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        let limit = self.limit;
        Box::pin(async move {
            matches!(tokio::time::timeout(limit, tokio::net::TcpStream::connect(address)).await, Ok(Ok(_)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_port_that_accepts_is_open_and_one_that_refuses_is_not() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let open = listener.local_addr().unwrap();
        let probe = TcpProbe::default();
        assert!(probe.is_open(open).await, "a listening port was said to be closed");
        drop(listener);
        assert!(!probe.is_open(open).await, "a port nobody listens on was said to be open");
    }

    #[tokio::test]
    async fn the_answer_comes_within_the_limit_even_for_an_address_that_does_not_answer() {
        // 192.0.2.0/24 is for documentation: nothing answers there (a network may refuse at once, which is as good).
        let probe = TcpProbe { limit: Duration::from_millis(200) };
        let began = std::time::Instant::now();
        let open = probe.is_open("192.0.2.1:9".parse().unwrap()).await;
        assert!(!open);
        assert!(began.elapsed() < Duration::from_secs(3), "took {:?}", began.elapsed());
    }
}
