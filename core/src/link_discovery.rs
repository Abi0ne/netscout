//! Devices on the same link whatever their address, for the deep profile.
//!
//! A device whose address belongs to another network (a NAS still on its
//! factory 192.168.0.x, a camera on a link-local 169.254.x) shares the cable
//! but never gets a probe: the sweep only covers the target, and the machine
//! would route any packet for that address to the gateway. What does reach
//! it is multicast and the limited broadcast 255.255.255.255, which every
//! device on the segment receives, whatever its subnet; the discovery tools
//! of NAS, cameras and routers work this way. Sources, all unprivileged:
//!
//! * **mDNS** — a multicast query from port 5353 for common service types.
//!   Answers from port 5353 are multicast too (RFC 6762 §6), so they arrive
//!   from any address; their A records give name and address.
//! * **MikroTik MNDP** — the 4-byte request broadcast to UDP 5678, answered
//!   on the same port: MAC, identity, board, address.
//! * **Hikvision SADP** — an XML inquiry multicast to 239.255.255.250:37020,
//!   answered by multicast: MAC, address, model.
//! * **Ubiquiti discovery** — the v1 request broadcast to UDP 10001: MAC,
//!   address, name, model. The answer is unicast, so it only arrives from
//!   devices that can route back to this machine.
//!
//! Each interface gets its own sockets, bound to it, so the broadcasts leave
//! on every link (not only the one of the default route). Hosts answer with
//! their own data; MACs come from the payloads, as UDP does not show the
//! sender's link-layer address.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::names;
use crate::types::DeviceType;

const MDNS_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const MDNS_PORT: u16 = 5353;
const MNDP_PORT: u16 = 5678;
const SADP_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const SADP_PORT: u16 = 37020;
const UBNT_PORT: u16 = 10001;

const DNS_TYPE_A: u16 = 1;
const DNS_TYPE_PTR: u16 = 12;

/// Service types asked over mDNS: what computers, printers, NAS, speakers,
/// TVs, cameras and smart-home devices advertise.
const SERVICE_TYPES: &[&str] = &[
    "_services._dns-sd._udp",
    "_device-info._tcp",
    "_workstation._tcp",
    "_http._tcp",
    "_ssh._tcp",
    "_smb._tcp",
    "_afpovertcp._tcp",
    "_ipp._tcp",
    "_printer._tcp",
    "_pdl-datastream._tcp",
    "_airplay._tcp",
    "_raop._tcp",
    "_googlecast._tcp",
    "_hap._tcp",
    "_companion-link._tcp",
    "_rfb._tcp",
    "_sonos._tcp",
    "_spotify-connect._tcp",
    "_axis-video._tcp",
    "_rtsp._tcp",
];

/// A network interface of this machine to ask on.
#[derive(Debug, Clone)]
pub struct Link {
    pub name: String,
    pub ipv4: Ipv4Addr,
}

/// A device that answered, with what it said about itself.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub ip: Ipv4Addr,
    /// "aa:bb:cc:dd:ee:ff".
    pub mac: Option<String>,
    pub name: Option<String>,
    pub model: Option<String>,
    /// What the protocol says the device is (a MikroTik is a router, …).
    pub kind: Option<DeviceType>,
}

impl Found {
    fn new(ip: Ipv4Addr) -> Self {
        Self { ip, mac: None, name: None, model: None, kind: None }
    }

    /// Fill what is missing from `other` (same device).
    fn absorb(&mut self, other: Found) {
        self.mac = self.mac.take().or(other.mac);
        self.name = self.name.take().or(other.name);
        self.model = self.model.take().or(other.model);
        self.kind = self.kind.take().or(other.kind);
    }
}

/// Ask on every link in `links` for `wait`; one entry per device address,
/// the machine's own addresses excluded.
pub async fn discover(links: &[Link], wait: Duration) -> Vec<Found> {
    let mut tasks = JoinSet::new();
    for link in links {
        let l = link.clone();
        tasks.spawn(async move { mdns(&l, wait).await });
        let l = link.clone();
        tasks.spawn(async move { mndp(&l, wait).await });
        let l = link.clone();
        tasks.spawn(async move { sadp(&l, wait).await });
        let l = link.clone();
        tasks.spawn(async move { ubnt(&l, wait).await });
    }
    let mut by_ip: HashMap<Ipv4Addr, Found> = HashMap::new();
    while let Some(done) = tasks.join_next().await {
        for f in done.unwrap_or_default() {
            if !usable(f.ip) || links.iter().any(|l| l.ipv4 == f.ip) {
                continue;
            }
            match by_ip.get_mut(&f.ip) {
                Some(known) => known.absorb(f),
                None => {
                    by_ip.insert(f.ip, f);
                }
            }
        }
    }
    let mut found: Vec<Found> = by_ip.into_values().collect();
    found.sort_by_key(|f| u32::from(f.ip));
    found
}

/// A unicast host address (not 0/8, loopback, multicast or broadcast).
fn usable(ip: Ipv4Addr) -> bool {
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ip.octets()[0] == 0)
}

// ---------------------------------------------------------------------------
// Sockets
// ---------------------------------------------------------------------------

/// A UDP socket on `link` bound to `port` (shared with other listeners),
/// allowed to broadcast and, with `group`, a member of that multicast group.
fn link_socket(link: &Link, port: u16, group: Option<Ipv4Addr>) -> Option<UdpSocket> {
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).ok()?;
    s.set_reuse_address(true).ok()?;
    setsockopt_int(&s, libc::SOL_SOCKET, libc::SO_REUSEPORT, 1);
    bind_to_interface(&s, &link.name);
    s.set_broadcast(true).ok()?;
    s.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port).into()).ok()?;
    if let Some(group) = group {
        s.join_multicast_v4(&group, &link.ipv4).ok()?;
        s.set_multicast_if_v4(&link.ipv4).ok()?;
        s.set_multicast_loop_v4(false).ok()?;
        let _ = s.set_multicast_ttl_v4(255);
    }
    s.set_nonblocking(true).ok()?;
    UdpSocket::from_std(s.into()).ok()
}

fn setsockopt_int(s: &Socket, level: libc::c_int, name: libc::c_int, value: libc::c_int) -> bool {
    use std::os::fd::AsRawFd;
    // SAFETY: a valid fd and a c_int option value of the size passed.
    unsafe {
        libc::setsockopt(
            s.as_raw_fd(),
            level,
            name,
            (&value as *const libc::c_int).cast(),
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        ) == 0
    }
}

/// Send and receive only through interface `name`, so a broadcast to
/// 255.255.255.255 leaves on that link. Best effort: without it the system
/// picks the interface of the default route.
fn bind_to_interface(s: &Socket, name: &str) {
    let Ok(cname) = std::ffi::CString::new(name) else { return };
    #[cfg(target_os = "macos")]
    {
        // SAFETY: a NUL-terminated name.
        let index = unsafe { libc::if_nametoindex(cname.as_ptr()) };
        if index != 0 {
            setsockopt_int(s, libc::IPPROTO_IP, libc::IP_BOUND_IF, index as libc::c_int);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        use std::os::fd::AsRawFd;
        let bytes = cname.as_bytes_with_nul();
        // SAFETY: a valid fd and the interface name with its length.
        unsafe {
            libc::setsockopt(
                s.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_BINDTODEVICE,
                bytes.as_ptr().cast(),
                bytes.len() as libc::socklen_t,
            );
        }
    }
}

/// Send `msg` to `to`, collect answers for half of `wait`, send again (a
/// sleeping device may miss the first) and collect for the other half.
async fn ask(
    socket: &UdpSocket,
    msg: &[u8],
    to: SocketAddrV4,
    wait: Duration,
    mut parse: impl FnMut(Ipv4Addr, &[u8]) -> Vec<Found>,
) -> Vec<Found> {
    let mut found = Vec::new();
    let mut buf = vec![0u8; 9000];
    for _ in 0..2 {
        let _ = socket.send_to(msg, to).await;
        let deadline = Instant::now() + wait / 2;
        loop {
            match tokio::time::timeout_at(deadline, socket.recv_from(&mut buf)).await {
                Ok(Ok((n, SocketAddr::V4(from)))) => found.extend(parse(*from.ip(), &buf[..n])),
                Ok(Ok(_)) => continue,
                Ok(Err(_)) | Err(_) => break,
            }
        }
    }
    found
}

// ---------------------------------------------------------------------------
// mDNS
// ---------------------------------------------------------------------------

async fn mdns(link: &Link, wait: Duration) -> Vec<Found> {
    let Some(socket) = link_socket(link, MDNS_PORT, Some(MDNS_GROUP)) else {
        return Vec::new();
    };
    let to = SocketAddrV4::new(MDNS_GROUP, MDNS_PORT);
    ask(&socket, &mdns_query(), to, wait, |_, msg| mdns_hosts(msg)).await
}

/// One query with a PTR question per service type (multicast answers).
fn mdns_query() -> Vec<u8> {
    let mut q = Vec::with_capacity(512);
    q.extend([0, 0, 0, 0]); // id 0, flags 0 (standard query)
    q.extend((SERVICE_TYPES.len() as u16).to_be_bytes());
    q.extend([0, 0, 0, 0, 0, 0]);
    for service in SERVICE_TYPES {
        for label in service.split('.').chain(["local"]) {
            q.push(label.len() as u8);
            q.extend(label.as_bytes());
        }
        q.push(0);
        q.extend(DNS_TYPE_PTR.to_be_bytes());
        q.extend(1u16.to_be_bytes()); // class IN, multicast answer
    }
    q
}

/// The A records of an mDNS response, in any section: name and address.
fn mdns_hosts(msg: &[u8]) -> Vec<Found> {
    let mut found = Vec::new();
    if msg.len() < 12 || msg[2] & 0x80 == 0 {
        return found; // not a response
    }
    let count = |i: usize| u16::from_be_bytes([msg[i], msg[i + 1]]) as usize;
    let (qd, records) = (count(4), count(6) + count(8) + count(10));
    let mut off = 12;
    for _ in 0..qd {
        let Some(end) = names::skip_name(msg, off) else { return found };
        off = end + 4;
    }
    for _ in 0..records {
        let Some(name_end) = names::skip_name(msg, off) else { break };
        let Some(rr) = msg.get(name_end..name_end + 10) else { break };
        let rtype = u16::from_be_bytes([rr[0], rr[1]]);
        let rdlen = u16::from_be_bytes([rr[8], rr[9]]) as usize;
        let rdata = name_end + 10;
        if rtype == DNS_TYPE_A && rdlen == 4 {
            if let (Some(name), Some(a)) = (names::read_name(msg, off), msg.get(rdata..rdata + 4)) {
                let ip = Ipv4Addr::new(a[0], a[1], a[2], a[3]);
                let mut f = Found::new(ip);
                f.name = meaningful_name(&name, ip);
                found.push(f);
            }
        }
        off = rdata + rdlen;
    }
    found
}

/// `name` without the trailing dot, unless it says nothing: the address
/// spelled out ("192-168-1-6.local"), "none", or a bare hex identifier
/// ("022A8AB66DA30000.local", as Matter devices use).
fn meaningful_name(name: &str, ip: Ipv4Addr) -> Option<String> {
    let name = name.trim_end_matches('.');
    let label = name.split('.').next().unwrap_or_default();
    let spelled = ip.to_string().replace('.', "-");
    let hex_id = label.len() >= 12 && label.chars().all(|c| c.is_ascii_hexdigit());
    let empty = label.is_empty() || ["none", "localhost"].contains(&label.to_ascii_lowercase().as_str());
    (!empty && !hex_id && label != spelled).then(|| name.to_string())
}

// ---------------------------------------------------------------------------
// MikroTik MNDP
// ---------------------------------------------------------------------------

async fn mndp(link: &Link, wait: Duration) -> Vec<Found> {
    let Some(socket) = link_socket(link, MNDP_PORT, None) else {
        return Vec::new();
    };
    let to = SocketAddrV4::new(Ipv4Addr::BROADCAST, MNDP_PORT);
    ask(&socket, &[0, 0, 0, 0], to, wait, |from, msg| {
        parse_mndp(from, msg).into_iter().collect()
    })
    .await
}

/// An MNDP announcement: a 4-byte header, then TLVs (type and length as
/// big-endian u16). 1 MAC, 5 identity, 8 platform, 12 board, 17 IPv4.
fn parse_mndp(from: Ipv4Addr, msg: &[u8]) -> Option<Found> {
    if msg.len() <= 4 {
        return None; // a request (ours or another tool's)
    }
    let (mut mac, mut identity, mut platform, mut board, mut ip) = (None, None, None, None, None);
    let mut off = 4;
    while let Some(head) = msg.get(off..off + 4) {
        let t = u16::from_be_bytes([head[0], head[1]]);
        let len = u16::from_be_bytes([head[2], head[3]]) as usize;
        let value = msg.get(off + 4..off + 4 + len)?;
        match t {
            1 if len == 6 => mac = Some(format_mac(value)),
            5 => identity = text(value),
            8 => platform = text(value),
            12 => board = text(value),
            17 if len == 4 => ip = Some(Ipv4Addr::new(value[0], value[1], value[2], value[3])),
            _ => {}
        }
        off += 4 + len;
    }
    mac.as_ref()?;
    let mut f = Found::new(ip.unwrap_or(from));
    f.mac = mac;
    f.name = identity;
    f.model = match (platform, board) {
        (Some(p), Some(b)) => Some(format!("{p} {b}")),
        (p, b) => b.or(p),
    };
    f.kind = Some(DeviceType::Router);
    Some(f)
}

// ---------------------------------------------------------------------------
// Hikvision SADP
// ---------------------------------------------------------------------------

async fn sadp(link: &Link, wait: Duration) -> Vec<Found> {
    let Some(socket) = link_socket(link, SADP_PORT, Some(SADP_GROUP)) else {
        return Vec::new();
    };
    let probe = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><Probe><Uuid>{}</Uuid><Types>inquiry</Types></Probe>",
        probe_uuid()
    );
    let to = SocketAddrV4::new(SADP_GROUP, SADP_PORT);
    ask(&socket, probe.as_bytes(), to, wait, |_, msg| {
        parse_sadp(msg).into_iter().collect()
    })
    .await
}

/// A random-looking UUID for the probe (devices echo it, nothing checks it).
fn probe_uuid() -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        ^ (std::process::id() as u128) << 64;
    let h = format!("{n:032X}");
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// A SADP `ProbeMatch`: address, MAC ("44-19-b6-…") and model.
fn parse_sadp(msg: &[u8]) -> Option<Found> {
    let xml = std::str::from_utf8(msg).ok()?;
    if !xml.contains("<ProbeMatch") {
        return None; // a probe
    }
    let ip: Ipv4Addr = xml_tag(xml, "IPv4Address")?.parse().ok()?;
    let mut f = Found::new(ip);
    f.mac = xml_tag(xml, "MAC").and_then(|m| parse_mac(&m));
    f.model = xml_tag(xml, "DeviceDescription");
    // Hikvision models: DS-2… cameras, DS-7…/8…/9… recorders.
    f.kind = Some(match f.model.as_deref() {
        Some(m) if ["DS-7", "DS-8", "DS-9"].iter().any(|p| m.starts_with(p)) => DeviceType::Nvr,
        _ => DeviceType::Camera,
    });
    Some(f)
}

/// The text of the first `<tag>…</tag>`, trimmed; `None` if empty.
fn xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = start + xml[start..].find(&format!("</{tag}>"))?;
    let value = xml[start..end].trim();
    (!value.is_empty()).then(|| value.to_string())
}

// ---------------------------------------------------------------------------
// Ubiquiti discovery
// ---------------------------------------------------------------------------

async fn ubnt(link: &Link, wait: Duration) -> Vec<Found> {
    let Some(socket) = link_socket(link, 0, None) else {
        return Vec::new();
    };
    let to = SocketAddrV4::new(Ipv4Addr::BROADCAST, UBNT_PORT);
    ask(&socket, &[1, 0, 0, 0], to, wait, |from, msg| {
        parse_ubnt(from, msg).into_iter().collect()
    })
    .await
}

/// A v1 discovery answer: version 1, command 0, length, then TLVs (type u8,
/// length big-endian u16). 1 MAC, 2 MAC + IPv4, 11 hostname, 12 short model,
/// 20 full model.
fn parse_ubnt(from: Ipv4Addr, msg: &[u8]) -> Option<Found> {
    if msg.len() <= 4 || msg[0] != 1 || msg[1] != 0 {
        return None;
    }
    let (mut mac, mut ip, mut name, mut short, mut full) = (None, None, None, None, None);
    let mut off = 4;
    while let Some(head) = msg.get(off..off + 3) {
        let t = head[0];
        let len = u16::from_be_bytes([head[1], head[2]]) as usize;
        let value = msg.get(off + 3..off + 3 + len)?;
        match t {
            1 if len == 6 => mac = mac.or(Some(format_mac(value))),
            2 if len == 10 => {
                mac = mac.or(Some(format_mac(&value[..6])));
                ip = ip.or(Some(Ipv4Addr::new(value[6], value[7], value[8], value[9])));
            }
            11 => name = text(value),
            12 => short = text(value),
            20 => full = text(value),
            _ => {}
        }
        off += 3 + len;
    }
    mac.as_ref()?;
    let mut f = Found::new(ip.unwrap_or(from));
    f.mac = mac;
    f.name = name;
    f.model = full.or(short);
    Some(f)
}

// ---------------------------------------------------------------------------

fn format_mac(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(":")
}

/// "44-19-B6-01-02-03" or "44:19:b6:01:02:03" → "44:19:b6:01:02:03".
fn parse_mac(text: &str) -> Option<String> {
    let parts: Vec<u8> = text
        .split(['-', ':'])
        .map(|p| u8::from_str_radix(p, 16))
        .collect::<Result<_, _>>()
        .ok()?;
    (parts.len() == 6).then(|| format_mac(&parts))
}

/// A printable string value, trimmed of NULs and spaces; `None` if empty.
fn text(value: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(value);
    let s = s.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    (!s.is_empty()).then(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mdns_query_asks_every_service_type() {
        let q = mdns_query();
        assert_eq!(u16::from_be_bytes([q[4], q[5]]) as usize, SERVICE_TYPES.len());
        assert!(q.ends_with(&[5, b'l', b'o', b'c', b'a', b'l', 0, 0, 12, 0, 1]));
    }

    /// A response with a PTR answer and the A record in the additional section.
    #[test]
    fn mdns_response_yields_its_a_records() {
        let mut m = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 1];
        // answer: _http._tcp.local PTR nas._http._tcp.local
        let ptr_name = m.len();
        for l in ["_http", "_tcp", "local"] {
            m.push(l.len() as u8);
            m.extend(l.as_bytes());
        }
        m.push(0);
        m.extend([0, 12, 0, 1, 0, 0, 0, 120, 0, 6]);
        m.extend([3, b'n', b'a', b's', 0xc0, ptr_name as u8]);
        // additional: nas.local A 10.0.0.5
        m.extend([3, b'n', b'a', b's', 0xc0, (ptr_name + 11) as u8]);
        m.extend([0, 1, 0x80, 1, 0, 0, 0, 120, 0, 4, 10, 0, 0, 5]);
        let found = mdns_hosts(&m);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].ip, Ipv4Addr::new(10, 0, 0, 5));
        assert_eq!(found[0].name.as_deref(), Some("nas.local"));
    }

    #[test]
    fn meaningless_mdns_names_are_dropped() {
        let ip = Ipv4Addr::new(192, 168, 1, 6);
        assert_eq!(meaningful_name("Saeko.local.", ip).as_deref(), Some("Saeko.local"));
        assert_eq!(meaningful_name("192-168-1-6.local", ip), None);
        assert_eq!(meaningful_name("none.local", ip), None);
        assert_eq!(meaningful_name("022A8AB66DA30000.local", ip), None);
        assert_eq!(meaningful_name("cafe.local", ip).as_deref(), Some("cafe.local"));
    }

    #[test]
    fn mdns_ignores_queries() {
        assert!(mdns_hosts(&mdns_query()).is_empty());
    }

    fn tlv16(t: u16, v: &[u8]) -> Vec<u8> {
        let mut out = t.to_be_bytes().to_vec();
        out.extend((v.len() as u16).to_be_bytes());
        out.extend(v);
        out
    }

    #[test]
    fn mndp_announcement() {
        let mut m = vec![0, 0, 0, 1];
        m.extend(tlv16(1, &[0x4c, 0x5e, 0x0c, 1, 2, 3]));
        m.extend(tlv16(5, b"core-router"));
        m.extend(tlv16(8, b"MikroTik"));
        m.extend(tlv16(12, b"RB5009UG+S+"));
        m.extend(tlv16(17, &[192, 168, 88, 1]));
        let f = parse_mndp(Ipv4Addr::new(10, 1, 1, 1), &m).unwrap();
        assert_eq!(f.ip, Ipv4Addr::new(192, 168, 88, 1));
        assert_eq!(f.mac.as_deref(), Some("4c:5e:0c:01:02:03"));
        assert_eq!(f.name.as_deref(), Some("core-router"));
        assert_eq!(f.model.as_deref(), Some("MikroTik RB5009UG+S+"));
        assert_eq!(f.kind, Some(DeviceType::Router));
        assert!(parse_mndp(Ipv4Addr::new(10, 1, 1, 1), &[0, 0, 0, 0]).is_none());
    }

    #[test]
    fn sadp_probe_match() {
        let xml = "<?xml version=\"1.0\"?><ProbeMatch><Uuid>X</Uuid><MAC>44-19-b6-0a-0b-0c</MAC>\
                   <IPv4Address>192.0.0.64</IPv4Address><DeviceDescription>DS-2CD2143G0-I</DeviceDescription></ProbeMatch>";
        let f = parse_sadp(xml.as_bytes()).unwrap();
        assert_eq!(f.ip, Ipv4Addr::new(192, 0, 0, 64));
        assert_eq!(f.mac.as_deref(), Some("44:19:b6:0a:0b:0c"));
        assert_eq!(f.model.as_deref(), Some("DS-2CD2143G0-I"));
        assert_eq!(f.kind, Some(DeviceType::Camera));
        let nvr = xml.replace("DS-2CD2143G0-I", "DS-7608NI-K2");
        assert_eq!(parse_sadp(nvr.as_bytes()).unwrap().kind, Some(DeviceType::Nvr));
        assert!(parse_sadp(b"<Probe><Types>inquiry</Types></Probe>").is_none());
    }

    #[test]
    fn ubnt_answer() {
        let tlv = |t: u8, v: &[u8]| {
            let mut out = vec![t];
            out.extend((v.len() as u16).to_be_bytes());
            out.extend(v);
            out
        };
        let mut body = tlv(2, &[0x24, 0x5a, 0x4c, 1, 2, 3, 192, 168, 1, 20]);
        body.extend(tlv(11, b"UAP-AC-Lite"));
        body.extend(tlv(12, b"U7LT"));
        let mut m = vec![1, 0];
        m.extend((body.len() as u16).to_be_bytes());
        m.extend(body);
        let f = parse_ubnt(Ipv4Addr::new(10, 0, 0, 9), &m).unwrap();
        assert_eq!(f.ip, Ipv4Addr::new(192, 168, 1, 20));
        assert_eq!(f.mac.as_deref(), Some("24:5a:4c:01:02:03"));
        assert_eq!(f.name.as_deref(), Some("UAP-AC-Lite"));
        assert_eq!(f.model.as_deref(), Some("U7LT"));
        assert!(parse_ubnt(Ipv4Addr::new(10, 0, 0, 9), &[1, 0, 0, 0]).is_none());
    }

    #[test]
    fn found_absorbs_missing_fields() {
        let mut a = Found::new(Ipv4Addr::new(10, 0, 0, 1));
        a.name = Some("a".into());
        let mut b = Found::new(Ipv4Addr::new(10, 0, 0, 1));
        b.name = Some("b".into());
        b.mac = Some("00:11:22:33:44:55".into());
        a.absorb(b);
        assert_eq!(a.name.as_deref(), Some("a"));
        assert_eq!(a.mac.as_deref(), Some("00:11:22:33:44:55"));
    }
}
