//! TCP connect liveness probes.
//!
//! A connect to `target:port` with a short deadline is a probe in itself:
//! SYN-ACK ⇒ [`TcpProbeResult::Connected`], RST ⇒ [`TcpProbeResult::Refused`]
//! (host up — discovery counts it as "up"), timeout ⇒ [`TcpProbeResult::Unknown`].
//!
//! All ports for one host are attempted in parallel within a bounded pool (at
//! most `max_concurrent` connects at a time), so a /24 sweep is paced by the
//! slowest *answers*, not by the length of the port list. A process-wide
//! socket budget (`sockets`) keeps the total number of open descriptors under
//! the OS limit (macOS defaults to 256).

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::liveness::TcpProbeResult;

/// Outcome of probing one host.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HostProbe {
    /// True if any port proved the host is up (connected or refused).
    pub up: bool,
    /// Ports that accepted a connection, ascending.
    pub open: Vec<u16>,
}

/// Connect to every port in `ports` on `host` and report the combined outcome.
/// The full port list is always probed, so `open` is complete for this call.
pub async fn probe(
    host: Ipv4Addr,
    ports: &[u16],
    timeout: Duration,
    max_concurrent: usize,
    sockets: Arc<Semaphore>,
) -> HostProbe {
    let max_concurrent = max_concurrent.max(1);
    let mut out = HostProbe::default();
    let mut set: JoinSet<(u16, TcpProbeResult)> = JoinSet::new();

    for &port in ports {
        if set.len() >= max_concurrent {
            fold(set.join_next().await, &mut out);
        }
        let sockets = Arc::clone(&sockets);
        set.spawn(async move {
            // The permit is held for the socket's lifetime; a closed
            // semaphore (scan torn down) just skips the probe.
            let Ok(_permit) = sockets.acquire_owned().await else {
                return (port, TcpProbeResult::Unknown);
            };
            (port, connect(SocketAddr::from((host, port)), timeout).await)
        });
    }
    while let Some(r) = set.join_next().await {
        fold(Some(r), &mut out);
    }
    out.open.sort_unstable();
    out
}

fn fold(r: Option<Result<(u16, TcpProbeResult), tokio::task::JoinError>>, out: &mut HostProbe) {
    if let Some(Ok((port, res))) = r {
        out.up |= res.proves_up();
        if res == TcpProbeResult::Connected {
            out.open.push(port);
        }
    }
}

/// One async connect, raced against `timeout` and classified. The stream is
/// dropped immediately: this is a pure liveness probe.
pub async fn connect(addr: SocketAddr, timeout: Duration) -> TcpProbeResult {
    match tokio::time::timeout(timeout, TcpStream::connect(addr)).await {
        Ok(Ok(_stream)) => TcpProbeResult::Connected,
        // RST arrived — from the host (port closed) or the local stack
        // (e.g. loopback) — the host is up either way.
        Ok(Err(e)) if e.kind() == io::ErrorKind::ConnectionRefused => TcpProbeResult::Refused,
        Ok(Err(_)) | Err(_) => TcpProbeResult::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connected_proves_up() {
        assert!(TcpProbeResult::Connected.proves_up());
    }

    #[test]
    fn refused_proves_up() {
        assert!(TcpProbeResult::Refused.proves_up());
    }

    #[test]
    fn unknown_does_not_prove_up() {
        assert!(!TcpProbeResult::Unknown.proves_up());
    }

    #[tokio::test]
    async fn loopback_open_and_closed_ports() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let open = listener.local_addr().unwrap().port();
        // Bind-and-drop to find a port that is (very likely) closed.
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let r = probe(
            Ipv4Addr::LOCALHOST,
            &[open, closed],
            Duration::from_secs(2),
            4,
            Arc::new(Semaphore::new(8)),
        )
        .await;
        assert!(r.up);
        assert_eq!(r.open, vec![open]);
    }
}
