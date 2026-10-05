//! Unprivileged ICMP echo (ping) discovery.
//!
//! One `SOCK_DGRAM` + `IPPROTO_ICMP` socket per scan. Behaviour differs per OS:
//!
//! * **macOS** — the request is sent as built (we compute the checksum), and
//!   received datagrams **include the IP header**; the socket may also see
//!   echo replies meant for other processes.
//! * **Linux** (`ping_group_range`) — the kernel rewrites the identifier and
//!   checksum, and delivers only this socket's replies, without IP header.
//!
//! So the engine does *not* rely on the identifier: each request carries a
//! 16-bit **sequence** (unique per client while in flight) and a payload with
//! a **magic** plus a per-client **token**. A single receiver thread
//! demultiplexes replies by (source IP, sequence) after checking magic and
//! token, which uniquely identifies each in-flight request.
//!
//! * One socket, one sender thread, one receiver thread, N in-flight requests.
//! * Sends never run on the async runtime: macOS can block `sendto` on a
//!   socket indefinitely (e.g. while the Local Network permission prompt is
//!   pending), which would stall every runtime worker. The sender thread
//!   absorbs that; a request whose send never happens just times out.
//! * RTT is measured against a `sent_at` instant captured just before `send`.
//! * A request resolves when its reply is matched or its timeout elapses.
//! * Dropping the client stops the receiver thread within ~100 ms; the sender
//!   thread exits once its queue is drained (or is abandoned if stuck).
//!
//! Availability: [`ping_available`] is true on macOS and on Linux when the
//! process gid is inside `net.ipv4.ping_group_range`; false on Android (no
//! unprivileged ICMP there — discovery falls back to TCP, which is why a
//! connect/refused is still "up").

use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use tokio::sync::oneshot;

/// Bytes of echo payload after the 8-byte ICMP header. Kept small so the
/// datagram stays well under any LAN MTU.
pub const PAYLOAD: usize = 56;

const ICMP_HEADER: usize = 8;
const ICMP_ECHO_REPLY: u8 = 0;
const ICMP_ECHO_REQUEST: u8 = 8;

/// Magic the payload carries; a reply must echo it back to be accepted (guards
/// against an unrelated ICMP reply reusing our sequence). "NSC2".
const MAGIC: u32 = 0x4e53_4332;

/// How often the receiver thread wakes to check for shutdown.
const RECV_POLL: Duration = Duration::from_millis(100);

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

type PendingMap = Mutex<HashMap<(Ipv4Addr, u16), Pending>>;

/// One queued echo request: destination, sequence, datagram.
type Outgoing = (Ipv4Addr, u16, [u8; ICMP_HEADER + PAYLOAD]);

/// An ICMP echo client: one blocking datagram socket, a sender thread and a
/// receiver thread.
///
/// Share across host tasks via `Arc`; every task calls [`PingClient::ping`].
pub struct PingClient {
    /// Queue to the sender thread; `None` once the client is dropping.
    outgoing: Option<mpsc::Sender<Outgoing>>,
    pending: Arc<PendingMap>,
    next_seq: AtomicU16,
    /// Per-client token echoed in the payload (separates concurrent clients).
    token: u32,
    stop: Arc<AtomicBool>,
    rx_thread: Option<JoinHandle<()>>,
}

impl PingClient {
    /// Open the unprivileged ICMP socket and start its receiver thread.
    ///
    /// Fails with a `PermissionDenied`-flavoured `io::Error` when the OS will
    /// not grant an unprivileged ICMP socket.
    pub fn new() -> Result<Self, io::Error> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::ICMPV4))?;
        // Blocking recv with a short read timeout so the receiver thread can
        // wake periodically and notice shutdown.
        socket.set_read_timeout(Some(RECV_POLL))?;

        let token = client_token();
        let pending: Arc<PendingMap> = Arc::new(Mutex::new(HashMap::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let rx_socket = socket.try_clone()?;
        let rx_pending = Arc::clone(&pending);
        let rx_stop = Arc::clone(&stop);
        let rx_thread = std::thread::Builder::new()
            .name("icmp-receiver".into())
            .spawn(move || recv_loop(&rx_socket, &rx_pending, &rx_stop, token))?;
        let (outgoing, queue) = mpsc::channel::<Outgoing>();
        let tx_pending = Arc::clone(&pending);
        std::thread::Builder::new()
            .name("icmp-sender".into())
            .spawn(move || send_loop(&socket, &queue, &tx_pending))?;
        Ok(Self {
            outgoing: Some(outgoing),
            pending,
            next_seq: AtomicU16::new(0),
            token,
            stop,
            rx_thread: Some(rx_thread),
        })
    }

    /// Send one echo request to `target` and await its reply for up to
    /// `timeout`.
    ///
    /// * `Ok(Some(r))` — a reply matched within the window.
    /// * `Ok(None)`    — no reply before the timeout (host down, the path
    ///   filtered ICMP, or the OS refused or held back the send).
    /// * `Err(_)`      — the client is shutting down.
    pub async fn ping(
        &self,
        target: Ipv4Addr,
        timeout: Duration,
    ) -> Result<Option<PingResult>, io::Error> {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let datagram = build_echo_request(seq, self.token);

        let (tx, rx) = oneshot::channel::<PingResult>();
        let sent_at = Instant::now();
        lock(&self.pending).insert((target, seq), Pending { sent_at, tx });

        let queued = self
            .outgoing
            .as_ref()
            .is_some_and(|q| q.send((target, seq, datagram)).is_ok());
        if !queued {
            lock(&self.pending).remove(&(target, seq));
            return Err(io::Error::other("ping client is shutting down"));
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(r)) => Ok(Some(r)),
            Ok(Err(_)) => Ok(None), // receiver gone without a reply
            Err(_) => {
                lock(&self.pending).remove(&(target, seq));
                Ok(None) // timed out
            }
        }
    }
}

impl Drop for PingClient {
    fn drop(&mut self) {
        // Closing the queue ends the sender thread. It is not joined: if the
        // OS holds it inside `sendto`, waiting would hang the scan's teardown.
        self.outgoing = None;
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.rx_thread.take() {
            let _ = t.join();
        }
    }
}

/// Lock the pending map, recovering from poisoning (a panicked holder cannot
/// leave the map logically inconsistent: every operation is a single insert
/// or remove).
fn lock(m: &PendingMap) -> MutexGuard<'_, HashMap<(Ipv4Addr, u16), Pending>> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Send loop: one blocking `send_to` per queued request. `sent_at` is reset
/// right before the send so RTT excludes queueing; a failed send drops the
/// pending entry, which resolves that ping as "no reply".
fn send_loop(socket: &Socket, queue: &mpsc::Receiver<Outgoing>, pending: &PendingMap) {
    for (target, seq, datagram) in queue {
        if let Some(p) = lock(pending).get_mut(&(target, seq)) {
            p.sent_at = Instant::now();
        } else {
            continue; // already timed out while queued
        }
        let addr = SockAddr::from(SocketAddrV4::new(target, 0));
        if let Err(e) = socket.send_to_with_flags(&datagram, &addr, libc::MSG_DONTWAIT) {
            if std::env::var_os("NETSCOUT_DEBUG").is_some() {
                eprintln!(
                    "[netscout-core] icmp send to {target} failed: {e} ({:?})",
                    e.raw_os_error()
                );
            }
            lock(pending).remove(&(target, seq));
        }
    }
}

/// Blocking receive loop: `recv_from`, parse the reply, route it to the
/// waiting future. Runs until `stop` is set or the socket fails.
fn recv_loop(socket: &Socket, pending: &PendingMap, stop: &AtomicBool, token: u32) {
    let fd = socket.as_raw_fd();
    let mut buf = [0u8; 1500];
    while !stop.load(Ordering::Relaxed) {
        let mut addr: libc::sockaddr_in = unsafe { std::mem::zeroed() };
        let mut addrlen = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
        // SAFETY: buf and addr are valid for the lengths passed.
        let n = unsafe {
            libc::recvfrom(
                fd,
                buf.as_mut_ptr().cast(),
                buf.len(),
                0,
                (&mut addr as *mut libc::sockaddr_in).cast(),
                &mut addrlen,
            )
        };
        if n < 0 {
            match io::Error::last_os_error().raw_os_error() {
                Some(libc::EAGAIN) | Some(libc::EINTR) | Some(libc::ETIMEDOUT) => continue,
                _ => return,
            }
        }
        let Some(seq) = parse_echo_reply(&buf[..n as usize], token) else {
            continue;
        };
        let src = Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr));
        if let Some(p) = lock(pending).remove(&(src, seq)) {
            let _ = p.tx.send(PingResult {
                rtt: p.sent_at.elapsed(),
            });
        }
    }
}

/// Build an echo request: `[type=8, code=0, checksum, id, seq, payload…]`.
/// Multi-byte fields are big-endian (network order).
fn build_echo_request(seq: u16, token: u32) -> [u8; ICMP_HEADER + PAYLOAD] {
    let mut d = [0u8; ICMP_HEADER + PAYLOAD];
    d[0] = ICMP_ECHO_REQUEST;
    d[1] = 0;
    // d[2..4] checksum, filled below. d[4..6] identifier: our token's low
    // half (Linux overwrites it; nothing depends on it).
    d[4..6].copy_from_slice(&(token as u16).to_be_bytes());
    d[6..8].copy_from_slice(&seq.to_be_bytes());
    d[8..12].copy_from_slice(&MAGIC.to_be_bytes());
    d[12..16].copy_from_slice(&token.to_be_bytes());
    let sum = checksum(&d);
    d[2..4].copy_from_slice(&sum.to_be_bytes());
    d
}

/// Parse a received datagram; returns its sequence if it is an echo reply
/// carrying our magic and token. Skips a leading IPv4 header (macOS).
fn parse_echo_reply(buf: &[u8], token: u32) -> Option<u16> {
    let icmp = match buf.first() {
        Some(b) if b >> 4 == 4 => buf.get(((b & 0x0f) as usize) * 4..)?,
        _ => buf,
    };
    if icmp.len() < ICMP_HEADER + 8 || icmp[0] != ICMP_ECHO_REPLY || icmp[1] != 0 {
        return None;
    }
    let magic = u32::from_be_bytes(icmp[8..12].try_into().ok()?);
    let tok = u32::from_be_bytes(icmp[12..16].try_into().ok()?);
    if magic != MAGIC || tok != token {
        return None;
    }
    Some(u16::from_be_bytes([icmp[6], icmp[7]]))
}

/// RFC 1071 Internet checksum.
fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for chunk in data.chunks(2) {
        let word = match chunk {
            [a, b] => u16::from_be_bytes([*a, *b]),
            [a] => u16::from_be_bytes([*a, 0]),
            _ => 0,
        };
        sum += word as u32;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// A per-client token: process id mixed with the current time.
fn client_token() -> u32 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    std::process::id().rotate_left(16) ^ nanos
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
    let gid = unsafe { libc::getegid() };
    if gid == 0 {
        return true;
    }
    let Ok(data) = std::fs::read_to_string("/proc/sys/net/ipv4/ping_group_range") else {
        return false;
    };
    let mut it = data.split_whitespace().map(|v| v.parse::<u32>());
    match (it.next(), it.next()) {
        (Some(Ok(lo)), Some(Ok(hi))) => (lo..=hi).contains(&gid),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn as_reply(mut d: [u8; ICMP_HEADER + PAYLOAD]) -> [u8; ICMP_HEADER + PAYLOAD] {
        d[0] = ICMP_ECHO_REPLY;
        d
    }

    #[test]
    fn request_checksum_verifies() {
        let d = build_echo_request(7, 0xdead_beef);
        assert_eq!(d[0], ICMP_ECHO_REQUEST);
        // A correct checksum makes the whole message sum to zero.
        assert_eq!(checksum(&d), 0);
    }

    #[test]
    fn reply_parsed_without_ip_header() {
        let d = as_reply(build_echo_request(42, 1234));
        assert_eq!(parse_echo_reply(&d, 1234), Some(42));
    }

    #[test]
    fn reply_parsed_after_ip_header() {
        let mut buf = vec![0x45u8];
        buf.extend([0u8; 19]); // rest of a 20-byte IPv4 header
        buf.extend(as_reply(build_echo_request(9, 77)));
        assert_eq!(parse_echo_reply(&buf, 77), Some(9));
    }

    #[test]
    fn foreign_or_non_reply_rejected() {
        let d = as_reply(build_echo_request(1, 5));
        assert_eq!(parse_echo_reply(&d, 6), None); // other client's token
        let req = build_echo_request(1, 5);
        assert_eq!(parse_echo_reply(&req, 5), None); // a request, not a reply
        assert_eq!(parse_echo_reply(&d[..10], 5), None); // truncated
    }

    /// Live check: pinging loopback must answer.
    #[tokio::test]
    async fn ping_loopback() {
        if !ping_available() {
            return;
        }
        let client = PingClient::new().expect("icmp socket");
        let r = client
            .ping(Ipv4Addr::LOCALHOST, Duration::from_secs(2))
            .await
            .expect("send");
        assert!(r.is_some(), "no echo reply from 127.0.0.1");
    }
}
