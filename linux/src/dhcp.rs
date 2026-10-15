//! Just enough DHCP to lease an address on a new VLAN interface, run by the
//! root helper (see `agent`): discover, request, and the address with its
//! prefix. The lease is never renewed and the router and name servers it
//! offers are ignored, like the macOS app's (which takes the address and
//! drops the rest).
//!
//! The interface has no address yet, so the messages go through a packet
//! socket, IP and UDP headers written here, and the answers are asked as
//! broadcasts.

use std::net::Ipv4Addr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::agent::{if_index, parse_mac};

const DISCOVER: u8 = 1;
const OFFER: u8 = 2;
const REQUEST: u8 = 3;
const ACK: u8 = 5;
const NAK: u8 = 6;
const MAGIC: [u8; 4] = [99, 130, 83, 99];

/// Lease an address on `iface` within `wait`: the address and its prefix.
pub fn lease(iface: &str, wait: Duration) -> Result<(Ipv4Addr, u32), String> {
    let mac = std::fs::read_to_string(format!("/sys/class/net/{iface}/address"))
        .ok()
        .and_then(|t| parse_mac(t.trim()))
        .ok_or_else(|| format!("{iface} non ha un indirizzo MAC"))?;
    let socket = IpSocket::open(iface)?;
    let deadline = Instant::now() + wait;
    let xid = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() ^ std::process::id())
        .unwrap_or(0x4e53_0001);

    while Instant::now() < deadline {
        // Discover, every 4 seconds until an offer comes.
        socket.send(&message(xid, &mac, DISCOVER, None, None))?;
        let Some(offer) = socket.wait_for(xid, &mac, &[OFFER], Duration::from_secs(4), deadline)
        else {
            continue;
        };
        let Some(server) = offer.server else { continue };
        for _ in 0..3 {
            socket.send(&message(xid, &mac, REQUEST, Some(offer.address), Some(server)))?;
            match socket.wait_for(xid, &mac, &[ACK, NAK], Duration::from_secs(3), deadline) {
                Some(reply) if reply.kind == ACK => {
                    let mask = reply.mask.or(offer.mask).unwrap_or(Ipv4Addr::new(255, 255, 255, 0));
                    return Ok((reply.address, u32::from(mask).count_ones()));
                }
                Some(_) => break, // NAK: start again
                None => {}
            }
        }
    }
    Err("nessuna offerta".into())
}

/// What a server answered.
struct Reply {
    kind: u8,
    address: Ipv4Addr,
    mask: Option<Ipv4Addr>,
    server: Option<Ipv4Addr>,
}

/// A DHCP message from this client: BOOTP with `kind` and, for a request,
/// the address offered and the server that offered it.
fn message(
    xid: u32,
    mac: &[u8; 6],
    kind: u8,
    requested: Option<Ipv4Addr>,
    server: Option<Ipv4Addr>,
) -> Vec<u8> {
    let mut m = vec![0u8; 236];
    m[0] = 1; // BOOTREQUEST
    m[1] = 1; // Ethernet
    m[2] = 6;
    m[4..8].copy_from_slice(&xid.to_be_bytes());
    m[10] = 0x80; // answer by broadcast: there is no address to answer to
    m[28..34].copy_from_slice(mac);
    m.extend_from_slice(&MAGIC);
    m.extend_from_slice(&[53, 1, kind]);
    m.extend_from_slice(&[61, 7, 1]);
    m.extend_from_slice(mac);
    if let Some(ip) = requested {
        m.extend_from_slice(&[50, 4]);
        m.extend_from_slice(&ip.octets());
    }
    if let Some(ip) = server {
        m.extend_from_slice(&[54, 4]);
        m.extend_from_slice(&ip.octets());
    }
    // Subnet mask only: router and DNS are not wanted.
    m.extend_from_slice(&[55, 1, 1, 255]);
    while m.len() < 300 {
        m.push(0);
    }
    m
}

/// `payload` from 0.0.0.0:68 to 255.255.255.255:67, as an IPv4 packet.
fn udp_packet(payload: &[u8]) -> Vec<u8> {
    let udp_len = 8 + payload.len();
    let total = 20 + udp_len;
    let mut p = Vec::with_capacity(total);
    p.extend_from_slice(&[0x45, 0x10]);
    p.extend_from_slice(&(total as u16).to_be_bytes());
    p.extend_from_slice(&[0, 0, 0, 0, 64, 17, 0, 0]);
    p.extend_from_slice(&[0, 0, 0, 0]);
    p.extend_from_slice(&[255, 255, 255, 255]);
    let sum = checksum(&p[..20]);
    p[10..12].copy_from_slice(&sum.to_be_bytes());
    p.extend_from_slice(&68u16.to_be_bytes());
    p.extend_from_slice(&67u16.to_be_bytes());
    p.extend_from_slice(&(udp_len as u16).to_be_bytes());
    p.extend_from_slice(&[0, 0]); // no UDP checksum (allowed over IPv4)
    p.extend_from_slice(payload);
    p
}

fn checksum(header: &[u8]) -> u16 {
    let mut sum: u32 = header
        .chunks(2)
        .map(|c| u32::from(u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])))
        .sum();
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// A DHCP answer for `xid` and `mac` in an IPv4 packet, if it is one.
fn parse_reply(packet: &[u8], xid: u32, mac: &[u8; 6]) -> Option<Reply> {
    let ihl = usize::from(*packet.first()? & 0x0f) * 4;
    if packet.first()? >> 4 != 4 || *packet.get(9)? != 17 {
        return None;
    }
    let udp = packet.get(ihl..)?;
    if u16::from_be_bytes([*udp.get(2)?, *udp.get(3)?]) != 68 {
        return None;
    }
    let m = udp.get(8..)?;
    if m.len() < 240 || m[0] != 2 || m[4..8] != xid.to_be_bytes() || m[28..34] != mac[..] {
        return None;
    }
    if m[236..240] != MAGIC {
        return None;
    }
    let address = Ipv4Addr::new(m[16], m[17], m[18], m[19]);
    let (mut kind, mut mask, mut server) = (0, None, None);
    let mut i = 240;
    while i < m.len() {
        let code = m[i];
        if code == 255 {
            break;
        }
        if code == 0 {
            i += 1;
            continue;
        }
        let len = usize::from(*m.get(i + 1)?);
        let value = m.get(i + 2..i + 2 + len)?;
        match (code, value) {
            (53, [k]) => kind = *k,
            (1, [a, b, c, d]) => mask = Some(Ipv4Addr::new(*a, *b, *c, *d)),
            (54, [a, b, c, d]) => server = Some(Ipv4Addr::new(*a, *b, *c, *d)),
            _ => {}
        }
        i += 2 + len;
    }
    Some(Reply {
        kind,
        address,
        mask,
        server,
    })
}

/// A packet socket for IPv4 packets on one interface (the link header is
/// the kernel's).
struct IpSocket {
    fd: libc::c_int,
    index: libc::c_int,
}

impl IpSocket {
    fn open(iface: &str) -> Result<Self, String> {
        let index = if_index(iface)?;
        let proto = (libc::ETH_P_IP as u16).to_be();
        // SAFETY: plain socket call.
        let fd = unsafe { libc::socket(libc::AF_PACKET, libc::SOCK_DGRAM, proto as libc::c_int) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let socket = Self { fd, index };
        let addr = socket.address(false);
        // SAFETY: the address and its size.
        let bound = unsafe {
            libc::bind(
                fd,
                (&addr as *const libc::sockaddr_ll).cast(),
                std::mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t,
            )
        };
        if bound != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let timeout = libc::timeval {
            tv_sec: 0,
            tv_usec: 500_000,
        };
        // SAFETY: the option value and its size.
        unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                (&timeout as *const libc::timeval).cast(),
                std::mem::size_of::<libc::timeval>() as libc::socklen_t,
            );
        }
        Ok(socket)
    }

    /// This interface, to the Ethernet broadcast address when `broadcast`.
    fn address(&self, broadcast: bool) -> libc::sockaddr_ll {
        // SAFETY: a zeroed C struct, filled below.
        let mut addr: libc::sockaddr_ll = unsafe { std::mem::zeroed() };
        addr.sll_family = libc::AF_PACKET as u16;
        addr.sll_protocol = (libc::ETH_P_IP as u16).to_be();
        addr.sll_ifindex = self.index;
        if broadcast {
            addr.sll_halen = 6;
            addr.sll_addr[..6].copy_from_slice(&[0xff; 6]);
        }
        addr
    }

    fn send(&self, dhcp: &[u8]) -> Result<(), String> {
        let packet = udp_packet(dhcp);
        let to = self.address(true);
        // SAFETY: the packet and the address, with their sizes.
        let sent = unsafe {
            libc::sendto(
                self.fd,
                packet.as_ptr().cast(),
                packet.len(),
                0,
                (&to as *const libc::sockaddr_ll).cast(),
                std::mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t,
            )
        };
        if sent < 0 {
            Err(std::io::Error::last_os_error().to_string())
        } else {
            Ok(())
        }
    }

    /// The first answer of one of `kinds` within `wait` (and `deadline`).
    fn wait_for(
        &self,
        xid: u32,
        mac: &[u8; 6],
        kinds: &[u8],
        wait: Duration,
        deadline: Instant,
    ) -> Option<Reply> {
        let until = (Instant::now() + wait).min(deadline);
        let mut buf = vec![0u8; 2048];
        while Instant::now() < until {
            // SAFETY: the buffer and its size.
            let len = unsafe {
                libc::recv(self.fd, buf.as_mut_ptr().cast(), buf.len(), 0)
            };
            if len <= 0 {
                continue;
            }
            if let Some(reply) = parse_reply(&buf[..len as usize], xid, mac) {
                if kinds.contains(&reply.kind) {
                    return Some(reply);
                }
            }
        }
        None
    }
}

impl Drop for IpSocket {
    fn drop(&mut self) {
        // SAFETY: our own descriptor, closed once.
        unsafe {
            libc::close(self.fd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_back_an_answer() {
        let mac = [2, 0, 0, 0, 0, 1];
        let mut m = message(7, &mac, REQUEST, Some(Ipv4Addr::new(10, 0, 0, 9)), None);
        // Turn the request into the server's ACK.
        m[0] = 2;
        m[16..20].copy_from_slice(&[10, 0, 0, 9]);
        let options = 240;
        m[options + 2] = ACK;
        m.truncate(options + 3);
        m.extend_from_slice(&[1, 4, 255, 255, 254, 0, 54, 4, 10, 0, 0, 1, 255]);
        let mut packet = udp_packet(&m);
        packet[20..22].copy_from_slice(&67u16.to_be_bytes());
        packet[22..24].copy_from_slice(&68u16.to_be_bytes());
        let reply = parse_reply(&packet, 7, &mac).unwrap();
        assert_eq!(reply.kind, ACK);
        assert_eq!(reply.address, Ipv4Addr::new(10, 0, 0, 9));
        assert_eq!(reply.mask, Some(Ipv4Addr::new(255, 255, 254, 0)));
        assert_eq!(reply.server, Some(Ipv4Addr::new(10, 0, 0, 1)));
        assert!(parse_reply(&packet, 8, &mac).is_none());
    }

    #[test]
    fn header_checksum() {
        let p = udp_packet(&[0; 300]);
        assert_eq!(checksum(&p[..20]), 0);
    }
}
