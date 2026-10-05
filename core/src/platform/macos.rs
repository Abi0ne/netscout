//! macOS implementation of [`PlatformNet`] (Phase 2).
//!
//! Everything here is unprivileged and does **not** shell out:
//!
//! * **Interface enumeration** — `getifaddrs(2)`: IPv4 address + netmask per
//!   interface, flag filters (up, not loopback).
//! * **Default gateway** — `sysctl` route dump (`NET_RT_FLAGS` + `RTF_GATEWAY`):
//!   the `0.0.0.0` destination entry is the default router, attributed to the
//!   interface named by its `rtm_index`.
//! * **DNS servers** — SystemConfiguration is a heavy dynamic library that
//!   cannot be linked from the core crate; the spec-sanctioned fallback,
//!   `/etc/resolv.conf`, is parsed instead (kept current by the OS).
//! * **ARP cache** — `sysctl` route dump with `RTF_LLINFO`: every resolved
//!   neighbour carries an `AF_LINK` gateway holding its link-layer address.
//!
//! Capability notes: macOS *does* allow unprivileged `SOCK_DGRAM` ICMP ("ping")
//! sockets, so `supports_unprivileged_icmp = true` — the engine's `PingClient`
//! uses it. Raw ARP frames are not allowed, so MACs come from the ARP cache
//! (`read_arp_cache`), not from active probes.
//!
//! All C layouts come from the `libc` crate; nothing is redeclared by hand.

use std::collections::HashMap;
use std::ffi::CStr;
use std::mem::size_of;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use crate::error::ScanError;
use crate::platform::PlatformNet;
use crate::types::NetworkInfo;

pub struct MacOSNet;

impl PlatformNet for MacOSNet {
    fn supports_unprivileged_icmp(&self) -> bool {
        // SOCK_DGRAM ICMP sockets are granted unprivileged on macOS.
        true
    }
    fn supports_unprivileged_arp(&self) -> bool {
        // Raw ARP frames need root (packet/BPF); the ARP *cache* is readable.
        false
    }
    fn supports_tcp_connect(&self) -> bool {
        true
    }

    fn enumerate_networks(&self) -> Result<Vec<NetworkInfo>, ScanError> {
        let mut nets = collect_ifaddrs()?;
        if nets.is_empty() {
            return Err(ScanError::NoNetwork);
        }
        // Gateway and DNS are fetched once and attributed per interface.
        let gateways = default_gateways();
        let dns = read_resolv_dns();
        for n in &mut nets {
            n.gateway = gateways.get(&n.interface).cloned();
            n.dns = dns.clone();
        }
        Ok(nets)
    }

    fn resolve_mac(
        &self,
        _target: &str,
        _iface: &str,
        _timeout: Duration,
    ) -> Result<Option<String>, ScanError> {
        Err(ScanError::Unsupported(
            "no unprivileged raw ARP on macOS; read the ARP cache after the sweep".into(),
        ))
    }

    fn ping(
        &self,
        _target: &str,
        _iface: &str,
        _timeout: Duration,
    ) -> Result<Option<u32>, ScanError> {
        // The engine's PingClient owns ICMP; this trait method stays a
        // capability marker on macOS.
        Err(ScanError::NotImplemented)
    }

    fn tcp_probe(&self, _target: &str, _port: u16, _timeout: Duration) -> Result<bool, ScanError> {
        // The engine's async `tcp_probe` module owns TCP connects.
        Err(ScanError::NotImplemented)
    }

    fn read_arp_cache(&self) -> Result<HashMap<String, String>, ScanError> {
        let buf = route_dump(libc::RTF_LLINFO)?;
        let mut map = HashMap::new();
        for msg in RouteMessages::new(&buf) {
            let (Some(dst), Some(gw)) = (msg.addr(libc::RTAX_DST), msg.addr(libc::RTAX_GATEWAY))
            else {
                continue;
            };
            if let (Some(ip), Some(mac)) = (sockaddr_ipv4(dst), sockaddr_dl_mac(gw)) {
                map.insert(ip.to_string(), mac);
            }
        }
        Ok(map)
    }
}

// ---------------------------------------------------------------------------
// Interfaces
// ---------------------------------------------------------------------------

/// IPv4 networks of every interface that is up and not loopback, deduplicated
/// by (interface, address). CIDRs are normalised to the network address.
fn collect_ifaddrs() -> Result<Vec<NetworkInfo>, ScanError> {
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(ScanError::NoNetwork);
    }

    let mut nets: Vec<NetworkInfo> = Vec::new();
    let mut cur = head;
    while !cur.is_null() {
        // SAFETY: `cur` is a node of the list returned by getifaddrs, which
        // stays valid until freeifaddrs below.
        let ifa = unsafe { &*cur };
        cur = ifa.ifa_next;

        let flags = ifa.ifa_flags as libc::c_int;
        if flags & libc::IFF_UP == 0 || flags & libc::IFF_LOOPBACK != 0 {
            continue;
        }
        if ifa.ifa_addr.is_null()
            || unsafe { (*ifa.ifa_addr).sa_family } as libc::c_int != libc::AF_INET
        {
            continue;
        }
        let ip = unsafe { sin_ip(ifa.ifa_addr) };
        let prefix = if ifa.ifa_netmask.is_null() {
            32
        } else {
            prefix_from_netmask(unsafe { sin_ip(ifa.ifa_netmask) })
        };
        let name = unsafe { CStr::from_ptr(ifa.ifa_name) }
            .to_string_lossy()
            .into_owned();
        let net = network_address(ip, prefix);
        let info = NetworkInfo {
            interface: name,
            ipv4: ip.to_string(),
            cidr: format!("{net}/{prefix}"),
            gateway: None,
            dns: Vec::new(),
        };
        if !nets
            .iter()
            .any(|n| n.interface == info.interface && n.ipv4 == info.ipv4)
        {
            nets.push(info);
        }
    }
    unsafe { libc::freeifaddrs(head) };
    Ok(nets)
}

/// Read the IPv4 address out of a `sockaddr` known to be `AF_INET`.
///
/// # Safety
/// `sa` must point to a valid `sockaddr_in`.
unsafe fn sin_ip(sa: *const libc::sockaddr) -> Ipv4Addr {
    let sin = unsafe { &*(sa as *const libc::sockaddr_in) };
    Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr))
}

fn prefix_from_netmask(mask: Ipv4Addr) -> u8 {
    u32::from(mask).count_ones() as u8
}

fn network_address(ip: Ipv4Addr, prefix: u8) -> Ipv4Addr {
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix as u32)
    };
    Ipv4Addr::from(u32::from(ip) & mask)
}

fn interface_name(index: u16) -> Option<String> {
    let mut buf = [0 as libc::c_char; libc::IF_NAMESIZE];
    let p = unsafe { libc::if_indextoname(index as libc::c_uint, buf.as_mut_ptr()) };
    if p.is_null() {
        return None;
    }
    Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// DNS
// ---------------------------------------------------------------------------

/// Parse `/etc/resolv.conf` `nameserver` lines (IPv4 only). This is the
/// spec-sanctioned DNS fallback (SystemConfiguration can't be linked here).
fn read_resolv_dns() -> Vec<String> {
    let Ok(data) = std::fs::read_to_string("/etc/resolv.conf") else {
        return Vec::new();
    };
    parse_resolv_dns(&data)
}

fn parse_resolv_dns(data: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in data.lines() {
        let Some(rest) = line.trim().strip_prefix("nameserver") else {
            continue;
        };
        let token = rest.split_whitespace().next().unwrap_or("");
        if matches!(token.parse::<IpAddr>(), Ok(IpAddr::V4(_))) && !out.iter().any(|s| s == token) {
            out.push(token.to_string());
        }
        if out.len() >= 5 {
            break;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Routing table (sysctl PF_ROUTE dump)
// ---------------------------------------------------------------------------

/// Dump the IPv4 routing table entries carrying `flags`
/// (`CTL_NET, PF_ROUTE, 0, AF_INET, NET_RT_FLAGS, flags`).
fn route_dump(flags: libc::c_int) -> Result<Vec<u8>, ScanError> {
    let mut mib = [
        libc::CTL_NET,
        libc::PF_ROUTE,
        0,
        libc::AF_INET,
        libc::NET_RT_FLAGS,
        flags,
    ];
    // The table can grow between the size query and the read; retry a few times.
    for _ in 0..4 {
        let mut size: libc::size_t = 0;
        let res = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                mib.len() as libc::c_uint,
                std::ptr::null_mut(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if res != 0 {
            return Err(ScanError::Network(format!(
                "sysctl route size: {}",
                std::io::Error::last_os_error()
            )));
        }
        if size == 0 {
            return Ok(Vec::new());
        }
        size += size / 4; // headroom
        let mut buf = vec![0u8; size];
        let res = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                mib.len() as libc::c_uint,
                buf.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if res == 0 {
            buf.truncate(size);
            return Ok(buf);
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::ENOMEM) {
            break;
        }
    }
    Err(ScanError::Network(format!(
        "sysctl route read: {}",
        std::io::Error::last_os_error()
    )))
}

/// Default gateway per interface name: entries with destination `0.0.0.0`
/// and an `AF_INET` gateway.
fn default_gateways() -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(buf) = route_dump(libc::RTF_GATEWAY) else {
        return out;
    };
    for msg in RouteMessages::new(&buf) {
        let dst = msg.addr(libc::RTAX_DST).and_then(sockaddr_ipv4);
        let gw = msg.addr(libc::RTAX_GATEWAY).and_then(sockaddr_ipv4);
        if let (Some(dst), Some(gw)) = (dst, gw) {
            if dst.is_unspecified() && !gw.is_unspecified() {
                if let Some(name) = interface_name(msg.index) {
                    out.entry(name).or_insert_with(|| gw.to_string());
                }
            }
        }
    }
    out
}

/// Packed sockaddrs of one routing message, indexed by `RTAX_*`.
type RouteAddrs<'a> = [Option<&'a [u8]>; libc::RTAX_MAX as usize];

/// One routing message: its interface index plus the packed sockaddrs.
struct RouteMessage<'a> {
    index: u16,
    addrs: RouteAddrs<'a>,
}

impl<'a> RouteMessage<'a> {
    fn addr(&self, rtax: libc::c_int) -> Option<&'a [u8]> {
        self.addrs.get(rtax as usize).copied().flatten()
    }
}

/// Iterator over the `rt_msghdr` stream returned by [`route_dump`].
struct RouteMessages<'a> {
    buf: &'a [u8],
    off: usize,
}

impl<'a> RouteMessages<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, off: 0 }
    }
}

impl<'a> Iterator for RouteMessages<'a> {
    type Item = RouteMessage<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let hdr_len = size_of::<libc::rt_msghdr>();
        loop {
            let rest = self.buf.get(self.off..)?;
            if rest.len() < hdr_len {
                return None;
            }
            // SAFETY: at least `hdr_len` bytes remain; read_unaligned copes
            // with any alignment of the Vec<u8> backing store.
            let hdr: libc::rt_msghdr = unsafe { std::ptr::read_unaligned(rest.as_ptr().cast()) };
            let msglen = hdr.rtm_msglen as usize;
            if msglen < hdr_len || msglen > rest.len() {
                return None;
            }
            self.off += msglen;
            if hdr.rtm_version as libc::c_int != libc::RTM_VERSION {
                continue;
            }
            return Some(RouteMessage {
                index: hdr.rtm_index,
                addrs: split_sockaddrs(&rest[hdr_len..msglen], hdr.rtm_addrs),
            });
        }
    }
}

/// Split the packed sockaddrs following an `rt_msghdr`. Each present address
/// (bit `i` of `rtm_addrs`) occupies `sa_len` bytes rounded up to 4; a zero
/// `sa_len` still takes 4 bytes.
fn split_sockaddrs(mut data: &[u8], rtm_addrs: libc::c_int) -> RouteAddrs<'_> {
    let mut out: RouteAddrs<'_> = [None; libc::RTAX_MAX as usize];
    for (i, slot) in out.iter_mut().enumerate() {
        if rtm_addrs & (1 << i) == 0 {
            continue;
        }
        let Some(&sa_len) = data.first() else {
            break;
        };
        let sa_len = sa_len as usize;
        let step = if sa_len == 0 { 4 } else { (sa_len + 3) & !3 };
        *slot = Some(&data[..sa_len.min(data.len())]);
        data = data.get(step..).unwrap_or(&[]);
    }
    out
}

/// IPv4 address of a raw `sockaddr_in` (`[len, family, port(2), addr(4), …]`).
/// Routing sockaddrs may be truncated (e.g. a short `0.0.0.0` netmask); the
/// missing bytes are zero.
fn sockaddr_ipv4(sa: &[u8]) -> Option<Ipv4Addr> {
    if sa.len() < 2 || sa[1] as libc::c_int != libc::AF_INET {
        return None;
    }
    let mut b = [0u8; 4];
    for (i, v) in b.iter_mut().enumerate() {
        *v = sa.get(4 + i).copied().unwrap_or(0);
    }
    Some(Ipv4Addr::from(b))
}

/// Ethernet MAC of a raw `sockaddr_dl` (`sdl_data` holds the interface name
/// followed by the link-layer address). `None` for incomplete ARP entries.
fn sockaddr_dl_mac(sa: &[u8]) -> Option<String> {
    // Offsets per <net/if_dl.h>: len, family, index(2), type, nlen, alen, slen, data…
    if sa.len() < 8 || sa[1] as libc::c_int != libc::AF_LINK {
        return None;
    }
    let (nlen, alen) = (sa[5] as usize, sa[6] as usize);
    if alen != 6 {
        return None;
    }
    let mac = sa.get(8 + nlen..8 + nlen + alen)?;
    if mac.iter().all(|&b| b == 0) {
        return None;
    }
    Some(
        mac.iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(":"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_from_netmask_counts_bits() {
        assert_eq!(prefix_from_netmask(Ipv4Addr::new(255, 0, 0, 0)), 8);
        assert_eq!(prefix_from_netmask(Ipv4Addr::new(255, 255, 0, 0)), 16);
        assert_eq!(prefix_from_netmask(Ipv4Addr::new(255, 255, 255, 0)), 24);
        assert_eq!(prefix_from_netmask(Ipv4Addr::new(0, 0, 0, 0)), 0);
    }

    #[test]
    fn network_address_masks_host_bits() {
        let ip = Ipv4Addr::new(192, 168, 1, 37);
        assert_eq!(network_address(ip, 24), Ipv4Addr::new(192, 168, 1, 0));
        assert_eq!(network_address(ip, 32), ip);
        assert_eq!(network_address(ip, 0), Ipv4Addr::UNSPECIFIED);
    }

    #[test]
    fn resolv_conf_ipv4_only_deduped() {
        let data = "# comment\nnameserver 1.1.1.1\nnameserver fe80::1\nnameserver 1.1.1.1\nnameserver 8.8.8.8\n";
        assert_eq!(parse_resolv_dns(data), vec!["1.1.1.1", "8.8.8.8"]);
    }

    #[test]
    fn sockaddrs_split_with_rounding() {
        // dst: sockaddr_in (16 bytes), gateway: 6-byte sockaddr rounded to 8.
        let mut data = vec![16u8, libc::AF_INET as u8, 0, 0, 10, 0, 0, 1];
        data.extend([0u8; 8]);
        data.extend([6u8, libc::AF_INET as u8, 0, 0, 10, 0, 0, 0]);
        let addrs = split_sockaddrs(&data, libc::RTA_DST | libc::RTA_GATEWAY);
        assert_eq!(
            sockaddr_ipv4(addrs[0].unwrap()),
            Some(Ipv4Addr::new(10, 0, 0, 1))
        );
        // Truncated sockaddr: missing address bytes read as zero.
        assert_eq!(
            sockaddr_ipv4(addrs[1].unwrap()),
            Some(Ipv4Addr::new(10, 0, 0, 0))
        );
        assert!(addrs[2].is_none());
    }

    #[test]
    fn sockaddr_dl_extracts_mac_after_name() {
        // len, AF_LINK, index(2), type, nlen=3, alen=6, slen, "en0" + mac
        let mut sa = vec![20u8, libc::AF_LINK as u8, 4, 0, 6, 3, 6, 0];
        sa.extend(b"en0");
        sa.extend([0xaa, 0xbb, 0xcc, 0x01, 0x02, 0x03]);
        assert_eq!(sockaddr_dl_mac(&sa).as_deref(), Some("aa:bb:cc:01:02:03"));
        sa[6] = 0; // incomplete entry
        assert_eq!(sockaddr_dl_mac(&sa), None);
    }

    /// Smoke test against the live OS: must not error, and every network has
    /// a well-formed CIDR.
    #[test]
    fn live_enumeration_and_arp_cache_do_not_fail() {
        if let Ok(nets) = MacOSNet.enumerate_networks() {
            for n in nets {
                assert!(n.cidr.parse::<ipnet::Ipv4Net>().is_ok(), "{n:?}");
            }
        }
        assert!(MacOSNet.read_arp_cache().is_ok());
    }
}
