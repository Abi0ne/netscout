//! Host-liveness results shared by the probes and the engine.

/// The outcome of a TCP connect probe.
///
/// A connect is a three-way handshake, so the outcome is observable even when
/// the peer is up but does not listen on the port:
///
/// * [`Connected`] — SYN-ACK arrived (host reachable, port open).
/// * [`Refused`] — RST arrived, from the remote host (host **up**, port
///   closed) or from the local stack for a local/loopback target.
/// * [`Unknown`] — no answer within the timeout (filtered path, host down, or
///   the connect could not even be initiated).
///
/// Host discovery counts `Connected` **and** `Refused` as "up"; only
/// `Unknown` leaves the host undetermined by this probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpProbeResult {
    Connected,
    Refused,
    Unknown,
}

impl TcpProbeResult {
    /// True when this outcome proves the host is up.
    pub fn proves_up(self) -> bool {
        matches!(self, Self::Connected | Self::Refused)
    }
}
