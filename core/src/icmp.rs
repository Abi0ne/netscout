//! Unprivileged ICMP echo (ping) discovery.
//!
//! One `SOCK_DGRAM` + `IPPROTO_ICMP` socket per scan. For unprivileged datagram
//! ICMP sockets the **kernel assigns the ICMP identifier** (the demultiplex key
//! that routes replies back to the socket that sent the request) and fills in
//! the checksum, on both Linux (`ping_group_range`) and macOS/BSD. So the
//! engine does *not* rely on the id value: it tags each request with a
//! 16-bit **sequence** and a payload **magic**, and a single receiver thread
//! demultiplexes replies by (reply source IP, sequence) — verified against the
//! magic — which uniquely identifies each in-flight request.
//!
//! * One socket, one receiver thread, N in-flight requests.
//! * RTT is measured against a `sent_at` instant captured just before `send`.
//! * A request resolves when its reply is matched, when its timeout elapses,
//!   or (on socket teardown) via a closed channel.
//!
//! Availability: [`ping_available`] is true on macOS and on Linux when the
//! process gid is inside `net.ipv4.ping_group_range`; false on Android (no raw
//! ICMP there — discovery falls back to TCP, which is why a connect/refused is
//! still "up").

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::os::fd::AsRawFd;
use std::os::unix::io::RawFd;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use socket2::{Domain, Socket, Type};
use tokio::sync::oneshot;

/// Bytes of echo payload after the 8-byte ICMP header. Kept small so the
/// datagram stays well under any LAN MTU.
pub const PAYLOAD: usize = 56;

/// Magic the payload carries; a reply must echo it back to be accepted (guards
/// against an unrelated ICMP reply reusing our sequence). "NSC2".
const MAGIC: u32 = 0x4e_53_43_32;

/// A resolved echo reply with its measured round-trip time.
#[derive(Debug, Clone)]
pub struct PingResult {
    /// Round-trip time measured at the receiver.
    pub rtt: Duration,
}

/// One in-flight request, awaiting its reply.
struct Pending {
    sent_at: Instant,
    tx: oneshot::Sender<PingResult>,
}

/// An ICMP echo client: one blocking datagram socket + one receiver thread.
///
/// Share across host tasks via `Arc`; every task calls [`PingClient::ping`].
pub struct PingClient {
    socket: Socket,
    /// (reply source IP, sequence) -> in-flight request.
    pending: Mutex<HashMap<(IpAddr, u16), Pending>>,
    _rx_thread: JoinHandle<()>,
}

impl PingClient {
    /// Open the unprivileged ICMP socket and start its receiver thread.
    ///
    /// Fails with a `PermissionDenied`-flavoured `io::Error` when the OS will
    /// not grant an unprivileged ICMP socket.
    pub fn new() -> Result<Self, io::Error> {
        let socket = unsafe {
            Socket::new(Domain::IP, Type::DGRAM, Some(1 /* IPPROTO_ICMP */))
        }?;
        // Blocking recv with a 1 s read timeout so the receiver thread can
        // wake periodically (and notice socket teardown) instead of blocking
        // forever.
        socket.set_read_timeout(Some(Duration::from_secs(1)))?;

        let pending = Arc::new(Mutex::new(HashMap::new()));
        let rx_socket = socket.try_clone()?;
        let rx_pending = Arc::clone(&pending);
        let rx_thread = std::thread::Builder::new()
            .name("icmp-receiver".into())
            .spawn(move || Self::recv_loop(rx_socket, &rx_pending))?;
        Ok(Self { socket, pending, _rx_thread: rx_thread })
    }

    /// Send one echo request to `target` (IPv4, port 0) and await its reply for
    /// up to `timeout`.
    ///
    /// * `Ok(Some(r))` — a reply matched within the window.
    /// * `Ok(None)`    — sent, but no reply before the timeout (host down, or
    ///   the path filtered ICMP).
    /// * `Err(_)`      — the request could not be sent at all.
    ///
    /// `seq` must be unique per in-flight request; the engine uses a per-host
    /// sequence.
    pub fn ping(
        &self,
        target: SocketAddr,
        seq: u16,
        timeout: Duration,
    ) -> Result<Option<PingResult>, io::Error> {
        let ip = match target {
            SocketAddr::V4(v4) => IpAddr::V4(v4.ip()),
            SocketAddr::V6(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "IPv6 not supported",
                ))
            }
        };

        // Build the datagram: [type=0, code=0, id=<kernel fills>, seq, payload…].
        let mut datagram = [0u8; 8 + PAYLOAD];
        datagram[0] = 0; // ICMP echo request
        datagram[1] = 0;
        // datagram[2..4] (id) left 0 — the kernel assigns it.
        datagram[4..6].copy_from_slice(&seq.to_ne_bytes());
        datagram[8..12].copy_from_slice(&MAGIC.to_ne_bytes());
        datagram[12..16].copy_from_slice(&seq.to_ne_bytes());

        let (tx, rx) = oneshot::channel::<PingResult>();
        let sent_at = Instant::now();
        {
            let mut p = self.pending.lock().unwrap();
            p.insert((ip, seq), Pending { sent_at, tx });
        }

        if let Err(e) = self.socket.send_to(&datagram, target) {
            self.pending.lock().unwrap().remove(&(ip, seq));
            return Err(e);
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(r)) => Ok(Some(r)),
            Ok(Err(_)) => Ok(None), // sender dropped without a reply
            Err(_) => {
                self.pending.lock().unwrap().remove(&(ip, seq));
                Ok(None) // timed out
            }
        }
    }

    /// The socket's raw fd (for optional tuning / group assignment).
    pub fn raw_fd(&self) -> RawFd {
        self.socket.as_raw_fd()
    }

    /// Blocking receive loop: `recvfrom`, parse the reply, route it to the
    /// waiting future. Runs until the socket is closed (teardown) then exits.
    fn recv_loop(socket: Socket, pending: &Arc<Mutex<HashMap<(IpAddr, u16), Pending>>>) {
        let fd = socket.as_raw_fd();
        let mut buf = [0u8; 1024];
        loop {
            let mut addr: libc::sockaddr_in = unsafe { std::mem::zeroed() };
            let mut addrlen: libc::socklen_t = std::mem::size_of::<libc::sockaddr_in>() as _;
            let n = unsafe {
                libc::recvfrom(
                    fd,
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    0,
                    &mut addr as *mut libc::sockaddr_in as *mut libc::sockaddr,
                    &mut addrlen,
                )
            };
            if n < 0 {
                // Read-timeout / would-block: poll again. Any other error
                // (socket closed, EBADF, …) ends the ping path; still-pending
                // futures are dropped with `pending` when the client is dropped.
                if !matches!(
                    std::io::Error::last_os_error().raw_os_error().copied(),
                    Some(libc::EAGAIN)
                        | Some(libc::EWOULDBLOCK)
                        | Some(libc::ETIMEDOUT)
                ) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            if n < 8 || !is_echo_reply(&buf) {
                continue;
            }
            let seq = u16::from_ne_bytes([buf[4], buf[5]]);
            // Integrity: the 16-byte magic at offset 8 must echo our request.
            let magic = u32::from_ne_bytes([buf[8], buf[9], buf[10], buf[11]]);
            if magic != MAGIC {
                continue;
            }
            let src_ip = IpAddr::V4(Ipv4Addr::from(u32::from_be_bytes(addr.sin_addr.s_addr.to_ne_bytes())));
            let rtt = Instant::now().saturating_duration_since({
                // Look up to read sent_at; the entry is removed below.
                let guard = pending.lock().unwrap();
                match guard.get(&(src_ip, seq)) {
                    Some(p) => p.sent_at,
                    None => continue,
                }
            });
            if let Some(p) = pending.lock().unwrap().remove(&(src_ip, seq)) {
                let _ = p.tx.send(PingResult { rtt });
            }
        }
    }
}

fn is_echo_reply(buf: &[u8]) -> bool {
    // A DGRAM ICMP socket delivers the ICMP message starting at offset 0 after
    // the kernel strips the IP header: [type, code, id, seq, payload…].
    buf.len() >= 8 && buf[0] == 0 && buf[1] == 0
}

/// True if the current process may open an unprivileged ICMP ping socket.
#[cfg(target_os = "macos")]
pub fn ping_available() -> bool {
    true
}

#[cfg(target_os = "android")]
pub fn ping_available() -> bool {
    false
}

#[cfg(not(any(target_os = "macos", target_os = "android")))]
pub fn ping_available() -> bool {
    ping_group_range_allows_us()
}

#[cfg(not(any(target_os = "macos", target_os = "android")))]
fn ping_group_range_allows_us() -> bool {
    use std::fs;
    let gid = unsafe { libc::getegid() };
    if gid == 0 {
        return true;
    }
    let Ok(data) = fs::read_to_string("/proc/sys/net/ipv4/ping_group_range") else {
        return false;
    };
    let Ok((lo, hi)) = data
        .split_once('-')
        .and_then(|(l, h)| l.trim().parse::<u32>().ok().zip(h.trim().parse::<u32>().ok()))
    else {
        return false;
    };
    (lo..=hi).contains(&(gid as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_echo_reply_requires_type_and_code_zero() {
        let mut buf = [0u8; 8];
        assert!(is_echo_reply(&buf));
        buf[0] = 3; // dest unreachable
        assert!(!is_echo_reply(&buf));
        assert!(!is_echo_reply(&[0u8; 2]));
    }
}
