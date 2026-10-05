//! TCP connect liveness probes.
//!
//! A connect to `target:port` with a short deadline is a probe in itself:
//! SYN-ACK ⇒ [`TcpProbeResult::Connected`], RST ⇒ [`TcpProbeResult::Refused`]
//! (host up — discovery counts it as "up"), timeout ⇒ [`TcpProbeResult::Unknown`].
//!
//! All ports for one host are attempted in parallel within a bounded pool (at
//! most `max_concurrent` connects at a time), so a /24 sweep is paced by the
//! slowest *answers*, not by the length of the port list.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use tokio::task::JoinHandle;

use crate::liveness::TcpProbeResult;

/// Connect to every port in `ports` on `host` and report the combined outcome.
///
/// Returns `(up, open)` where `up` is true if any probe proved liveness and
/// `open` lists every port that accepted a connection (best-effort: the full
/// port list is always probed, so `open` is complete for this call).
pub async fn probe(
    host: Ipv4Addr,
    ports: &[u16],
    timeout: Duration,
    max_concurrent: usize,
) -> (bool, Vec<(u16, TcpProbeResult)>) {
    let max_concurrent = max_concurrent.max(1);
    let mut up = false;
    let mut open: Vec<(u16, TcpProbeResult)> = Vec::new();

    let mut pending: Vec<JoinHandle<(u16, TcpProbeResult)>> = Vec::new();
    for &port in ports {
        pending.push(join_probe(host, port, timeout));
        if pending.len() == max_concurrent {
            drain(&mut pending, &mut up, &mut open).await;
        }
    }
    drain(&mut pending, &mut up, &mut open).await;

    (up, open)
}

/// Await every in-flight probe in the current batch and fold its results.
async fn drain(
    pending: &mut Vec<JoinHandle<(u16, TcpProbeResult)>>,
    up: &mut bool,
    open: &mut Vec<(u16, TcpProbeResult)>,
) {
    while let Some(fut) = pending.pop() {
        if let Ok((port, res)) = fut.await {
            if res.proves_up() {
                *up = true;
            }
            if matches!(res, TcpProbeResult::Connected) {
                open.push((port, res));
            }
        }
    }
}

/// One connect, raced against `timeout` and classified.
fn join_probe(
    host: Ipv4Addr,
    port: u16,
    timeout: Duration,
) -> JoinHandle<(u16, TcpProbeResult)> {
    let addr = SocketAddr::V4((host, port).into());
    tokio::spawn(async move {
        let outcome = tokio::time::timeout(timeout, connect_blocking(addr)).await;
        let res = match outcome {
            Ok(Ok(())) => TcpProbeResult::Connected,
            Ok(Err(e)) => match e.kind() {
                // RST arrived — from the host (port closed) or the local stack
                // (e.g. loopback) — the host is up either way.
                io::ErrorKind::ConnectionRefused => TcpProbeResult::Refused,
                _ => TcpProbeResult::Unknown,
            },
            Err(_) => TcpProbeResult::Unknown, // timeout
        };
        (port, res)
    })
}

/// A connect raced against a deadline via a blocking `TcpStream::connect`
/// (spawned by the caller on a runtime worker thread, so the FFI thread is
/// never blocked). The stream is dropped immediately: this is a pure
/// liveness probe.
fn connect_blocking(addr: SocketAddr) -> io::Result<()> {
    TcpStream::connect(addr)?;
    Ok(())
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
}
