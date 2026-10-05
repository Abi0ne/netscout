//! Hostname resolution for discovered hosts, from three unprivileged sources:
//!
//! * **Reverse DNS** — `getnameinfo` with `NI_NAMEREQD` through the OS
//!   resolver. Works where the LAN's DNS has PTR records (many home routers
//!   register DHCP client names; plenty of office DNS servers do not). The
//!   call is blocking, so the engine runs it on tokio's blocking pool.
//! * **mDNS / Bonjour** — a reverse (`PTR …in-addr.arpa`) query sent straight
//!   to each host's UDP port 5353 (an RFC 6762 §5.5 direct unicast query):
//!   Macs, iPhones, printers, speakers and much IoT answer with their
//!   `.local` name.
//! * **NetBIOS** — a node-status (`NBSTAT`) query to UDP port 137: Windows
//!   machines and Samba servers answer with their computer name.
//!
//! mDNS and NetBIOS share one shape: a single UDP socket sends one query per
//! host, then collects answers until a deadline, matching them by source
//! address. Hosts that do not run the service simply never answer.

use std::collections::HashMap;
use std::ffi::CStr;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::time::Instant;

/// Longest hostname `getnameinfo` can return (`NI_MAXHOST`).
const NI_MAXHOST: usize = 1025;

const MDNS_PORT: u16 = 5353;
const NETBIOS_PORT: u16 = 137;
const DNS_TYPE_PTR: u16 = 12;
const NB_TYPE_NBSTAT: u16 = 0x21;

/// Reverse-resolve `ip`. `None` when there is no PTR record (or the lookup
/// fails); never returns the address itself.
pub fn reverse_lookup(ip: Ipv4Addr) -> Option<String> {
    let mut sin: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        sin.sin_len = std::mem::size_of::<libc::sockaddr_in>() as u8;
    }
    sin.sin_family = libc::AF_INET as libc::sa_family_t;
    sin.sin_addr.s_addr = u32::from(ip).to_be();

    let mut host = [0 as libc::c_char; NI_MAXHOST];
    // SAFETY: `sin` is a valid sockaddr_in of the length passed, `host` is
    // writable for its full length; no service buffer is requested.
    let rc = unsafe {
        libc::getnameinfo(
            (&sin as *const libc::sockaddr_in).cast(),
            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            host.as_mut_ptr(),
            host.len() as libc::socklen_t,
            std::ptr::null_mut(),
            0,
            libc::NI_NAMEREQD,
        )
    };
    if rc != 0 {
        return None;
    }
    let name = unsafe { CStr::from_ptr(host.as_ptr()) }.to_string_lossy();
    clean_hostname(&name, ip)
}

/// mDNS names of `hosts`, by direct unicast reverse queries to port 5353.
pub async fn mdns_names(hosts: &[Ipv4Addr], wait: Duration) -> HashMap<Ipv4Addr, String> {
    query_all(hosts, MDNS_PORT, wait, build_mdns_query, |ip, buf| {
        parse_mdns_ptr(buf).and_then(|n| clean_hostname(&n, ip))
    })
    .await
}

/// NetBIOS computer names of `hosts`, by node-status queries to port 137.
pub async fn netbios_names(hosts: &[Ipv4Addr], wait: Duration) -> HashMap<Ipv4Addr, String> {
    query_all(hosts, NETBIOS_PORT, wait, build_nbstat_query, |ip, buf| {
        parse_nbstat_name(buf).and_then(|n| clean_hostname(&n, ip))
    })
    .await
}

/// Send `build(ip, id)` to every `ip:port` from one socket, then collect
/// replies until `wait` elapses (or every host answered). Replies are matched
/// by source address and transaction id.
async fn query_all(
    hosts: &[Ipv4Addr],
    port: u16,
    wait: Duration,
    build: fn(Ipv4Addr, u16) -> Vec<u8>,
    parse: impl Fn(Ipv4Addr, &[u8]) -> Option<String>,
) -> HashMap<Ipv4Addr, String> {
    let mut found = HashMap::new();
    let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await else {
        return found;
    };
    let mut ids: HashMap<Ipv4Addr, u16> = HashMap::new();
    for (i, &ip) in hosts.iter().enumerate() {
        let id = 0x4e00 ^ i as u16;
        // A send error (host unreachable, …) just means no answer.
        if socket.send_to(&build(ip, id), (ip, port)).await.is_ok() {
            ids.insert(ip, id);
        }
    }

    let deadline = Instant::now() + wait;
    let mut buf = [0u8; 1500];
    while found.len() < ids.len() {
        let Ok(Ok((n, from))) = tokio::time::timeout_at(deadline, socket.recv_from(&mut buf)).await
        else {
            break; // deadline reached (or socket error)
        };
        let SocketAddr::V4(from) = from else { continue };
        let ip = *from.ip();
        let reply = &buf[..n];
        let id_matches =
            reply.len() >= 2 && ids.get(&ip) == Some(&u16::from_be_bytes([reply[0], reply[1]]));
        if !id_matches || found.contains_key(&ip) {
            continue;
        }
        if let Some(name) = parse(ip, reply) {
            found.insert(ip, name);
        }
    }
    found
}

// ---------------------------------------------------------------------------
// mDNS
// ---------------------------------------------------------------------------

/// A DNS query for `PTR d.c.b.a.in-addr.arpa`, class IN with the
/// unicast-response bit set.
fn build_mdns_query(ip: Ipv4Addr, id: u16) -> Vec<u8> {
    let mut q = Vec::with_capacity(64);
    q.extend(id.to_be_bytes());
    q.extend([0, 0]); // flags: standard query
    q.extend([0, 1, 0, 0, 0, 0, 0, 0]); // 1 question
    let o = ip.octets();
    for label in [
        o[3].to_string(),
        o[2].to_string(),
        o[1].to_string(),
        o[0].to_string(),
        "in-addr".into(),
        "arpa".into(),
    ] {
        q.push(label.len() as u8);
        q.extend(label.as_bytes());
    }
    q.push(0);
    q.extend(DNS_TYPE_PTR.to_be_bytes());
    q.extend(0x8001u16.to_be_bytes()); // QU bit + class IN
    q
}

/// The target name of the first PTR answer in a DNS response.
fn parse_mdns_ptr(msg: &[u8]) -> Option<String> {
    if msg.len() < 12 || msg[2] & 0x80 == 0 {
        return None; // not a response
    }
    let qdcount = u16::from_be_bytes([msg[4], msg[5]]);
    let ancount = u16::from_be_bytes([msg[6], msg[7]]);
    let mut off = 12;
    for _ in 0..qdcount {
        off = skip_name(msg, off)? + 4; // type + class
    }
    for _ in 0..ancount {
        off = skip_name(msg, off)?;
        let rr = msg.get(off..off + 10)?;
        let rtype = u16::from_be_bytes([rr[0], rr[1]]);
        let rdlen = u16::from_be_bytes([rr[8], rr[9]]) as usize;
        let rdata = off + 10;
        if rtype == DNS_TYPE_PTR {
            return read_name(msg, rdata);
        }
        off = rdata + rdlen;
    }
    None
}

/// Offset just past the (possibly compressed) name starting at `off`.
fn skip_name(msg: &[u8], mut off: usize) -> Option<usize> {
    loop {
        let len = *msg.get(off)?;
        match len {
            0 => return Some(off + 1),
            l if l & 0xc0 == 0xc0 => return Some(off + 2),
            l => off += 1 + l as usize,
        }
    }
}

/// Decode the (possibly compressed) name at `off`, dot-separated.
fn read_name(msg: &[u8], mut off: usize) -> Option<String> {
    let mut labels: Vec<String> = Vec::new();
    // Bound pointer chasing so a malicious loop cannot spin forever.
    for _ in 0..64 {
        let len = *msg.get(off)?;
        match len {
            0 => return Some(labels.join(".")),
            l if l & 0xc0 == 0xc0 => {
                let lo = *msg.get(off + 1)?;
                off = (((l & 0x3f) as usize) << 8) | lo as usize;
            }
            l => {
                let label = msg.get(off + 1..off + 1 + l as usize)?;
                labels.push(String::from_utf8_lossy(label).into_owned());
                off += 1 + l as usize;
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// NetBIOS
// ---------------------------------------------------------------------------

/// A NetBIOS node-status request for the wildcard name `*`.
fn build_nbstat_query(_ip: Ipv4Addr, id: u16) -> Vec<u8> {
    let mut q = Vec::with_capacity(50);
    q.extend(id.to_be_bytes());
    q.extend([0, 0]); // flags
    q.extend([0, 1, 0, 0, 0, 0, 0, 0]); // 1 question
                                        // First-level encoding of "*" padded with NULs to 16 bytes: each byte
                                        // becomes two letters 'A' + nibble.
    q.push(32);
    let mut raw = [0u8; 16];
    raw[0] = b'*';
    for b in raw {
        q.push(b'A' + (b >> 4));
        q.push(b'A' + (b & 0x0f));
    }
    q.push(0);
    q.extend(NB_TYPE_NBSTAT.to_be_bytes());
    q.extend(1u16.to_be_bytes()); // class IN
    q
}

/// The computer name from a node-status response: the first unique
/// (non-group) name with suffix 0x00 (the workstation service).
fn parse_nbstat_name(msg: &[u8]) -> Option<String> {
    if msg.len() < 12 || msg[2] & 0x80 == 0 {
        return None;
    }
    // Header, then the echoed question name, then type/class/ttl/rdlength.
    let off = skip_name(msg, 12)? + 10;
    let count = *msg.get(off)? as usize;
    for i in 0..count {
        let entry = msg.get(off + 1 + i * 18..off + 1 + (i + 1) * 18)?;
        let (name, suffix, flags) = (&entry[..15], entry[15], entry[16]);
        if suffix == 0x00 && flags & 0x80 == 0 {
            let name = String::from_utf8_lossy(name).trim().to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------

/// Normalise a resolver answer: trim the trailing dot, drop empty answers and
/// answers that are just the address again.
fn clean_hostname(name: &str, ip: Ipv4Addr) -> Option<String> {
    let name = name.trim().trim_end_matches('.');
    if name.is_empty() || name.parse::<Ipv4Addr>() == Ok(ip) {
        return None;
    }
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_hostname_normalises() {
        let ip = Ipv4Addr::new(192, 168, 1, 10);
        assert_eq!(clean_hostname("nas.lan.", ip).as_deref(), Some("nas.lan"));
        assert_eq!(clean_hostname("192.168.1.10", ip), None);
        assert_eq!(clean_hostname("", ip), None);
    }

    #[test]
    fn loopback_resolves() {
        // /etc/hosts maps 127.0.0.1 to "localhost" on macOS and Linux.
        assert_eq!(
            reverse_lookup(Ipv4Addr::LOCALHOST).as_deref(),
            Some("localhost")
        );
    }

    #[test]
    fn mdns_query_shape() {
        let q = build_mdns_query(Ipv4Addr::new(192, 168, 1, 7), 0x1234);
        assert_eq!(&q[..2], &[0x12, 0x34]);
        // First label is the last octet.
        assert_eq!(&q[12..14], &[1, b'7']);
        assert!(q.ends_with(&[0, 12, 0x80, 0x01]));
    }

    /// A PTR response whose answer name is a compression pointer back to the
    /// question, as mDNSResponder sends it.
    fn mdns_reply(target: &str) -> Vec<u8> {
        let mut m = build_mdns_query(Ipv4Addr::new(10, 0, 0, 5), 7);
        m[2] = 0x84; // response, authoritative
        m[7] = 1; // ancount = 1
        m.extend([0xc0, 12]); // name -> question
        m.extend([0, 12, 0x80, 1, 0, 0, 0, 120]); // PTR, IN|flush, ttl
        let mut rdata = Vec::new();
        for label in target.split('.') {
            rdata.push(label.len() as u8);
            rdata.extend(label.as_bytes());
        }
        rdata.push(0);
        m.extend((rdata.len() as u16).to_be_bytes());
        m.extend(rdata);
        m
    }

    #[test]
    fn mdns_ptr_parsed() {
        let m = mdns_reply("Living-Room.local");
        assert_eq!(parse_mdns_ptr(&m).as_deref(), Some("Living-Room.local"));
        // A query (QR bit clear) is not a reply.
        let q = build_mdns_query(Ipv4Addr::new(10, 0, 0, 5), 7);
        assert_eq!(parse_mdns_ptr(&q), None);
        // Truncated replies are rejected, not panicked on.
        assert_eq!(parse_mdns_ptr(&m[..m.len() - 3]), None);
    }

    #[test]
    fn name_pointer_loop_is_bounded() {
        let mut m = vec![0u8; 12];
        m.extend([0xc0, 12]); // points to itself
        assert_eq!(read_name(&m, 12), None);
    }

    #[test]
    fn nbstat_query_encodes_wildcard() {
        let q = build_nbstat_query(Ipv4Addr::LOCALHOST, 1);
        assert_eq!(q[12], 32);
        assert_eq!(&q[13..15], b"CK"); // '*' = 0x2A -> 'C','K'
        assert_eq!(&q[15..17], b"AA"); // NUL padding
        assert!(q.ends_with(&[0, 0x21, 0, 1]));
    }

    #[test]
    fn nbstat_name_parsed() {
        let mut m = build_nbstat_query(Ipv4Addr::LOCALHOST, 1);
        m[2] = 0x84;
        m[5] = 0; // qdcount 0 in a response…
        m[7] = 1; // …ancount 1, the name echoed at offset 12
        m.extend([0, 0, 0, 0]); // ttl (type and class are already there)
        let entries: [(&[u8; 15], u8, u8); 2] = [
            (b"WORKGROUP      ", 0x00, 0x84), // group name: skipped
            (b"DESKTOP-42     ", 0x00, 0x04),
        ];
        m.extend(((1 + entries.len() * 18) as u16).to_be_bytes());
        m.push(entries.len() as u8);
        for (name, suffix, flags) in entries {
            m.extend(name);
            m.push(suffix);
            m.extend([flags, 0]);
        }
        assert_eq!(parse_nbstat_name(&m).as_deref(), Some("DESKTOP-42"));
    }
}
