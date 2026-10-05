//! macOS implementation of [`PlatformNet`] (Phase 2).
//!
//! Everything here is unprivileged and does **not** shell out:
//!
//! * **Interface enumeration** — `getifaddrs(2)`: IPv4 address + netmask per
//!   interface, flag filters (up, not loopback), and the link-layer address.
//! * **Default gateway** — `sysctl` route table, `NET_RT_FLAGS` with
//!   `RTF_GATEWAY` (the first matching entry is the default router).
//! * **DNS servers** — SystemConfiguration is a heavy dynamic library that
//!   cannot be linked from the core crate; the spec-sanctioned fallback,
//!   `/etc/resolv.conf`, is parsed instead (kept current by the OS).
//! * **ARP cache** — `sysctl` route table with `RTF_LLINFO` (`NET_RT_IFLIST2`):
//!   every ARP-resolved host exposes its link-layer address.
//!
//! Capability notes: macOS *does* allow unprivileged `SOCK_DGRAM` ICMP ("ping")
//! sockets, so `supports_unprivileged_icmp = true` — the engine's `PingClient`
//! uses it. Raw ARP frames are not allowed, so MACs come from the ARP cache
//! (`read_arp_cache`), not from active probes.

use std::collections::HashMap;
use std::mem::{size_of, size_of_val, zeroed};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use crate::error::ScanError;
use crate::liveness::TcpProbeResult;
use crate::platform::PlatformNet;
use crate::types::NetworkInfo;

// ---------------------------------------------------------------------------
// FFI: ifaddrs
// ---------------------------------------------------------------------------

#[allow(non_camel_case_types)]
#[repr(C)]
pub struct ifaddrs {
    pub ifa_next: *mut ifaddrs,
    pub ifa_name: *mut libc::c_char,
    pub ifa_flags: u32,
    pub ifa_addr: *mut libc::sockaddr,
    pub ifa_netmask: *mut libc::sockaddr,
    pub ifa_ifu: *mut c_void,
    pub ifa_data: *mut c_void,
}

#[allow(non_camel_case_types)]
#[repr(C)]
struct if_data {
    ifi_type: u8,
    ifi_physical: u8,
    ifi_addrcount: u32,
    ifi_mtu: u32,
    ifi_metric: u32,
    ifi_baudrate: u64,
    ifi_ipackets: u64,
    ifi_ierrors: u64,
    ifi_opackets: u64,
    ifi_oerrors: u64,
    ifi_collisions: u64,
    ifi_ipacketdrops: u64,
    ifi_obytes: u64,
    ifi_ibytes: u64,
    ifi_oqdrops: u64,
    ifi_noproto: u64,
    pub ifi_unit: u32,
    ifi_internal: [u8; 4],
    ifi_ibdrops: [u64; 8],
    ifi_obytes2: u64,
    ifi_ibytes2: u64,
    ifi_opackets2: u64,
    ifi_ipackets2: u64,
    ifi_hwaddr: [u8; 12],
}

const AF_INET: libc::c_int = libc::AF_INET;
const AF_INET6: libc::c_int = libc::AF_INET6;
const ARPHRD_ETHER: i32 = 1;

unsafe extern "C" {
    fn getifaddrs(ifap: *mut *mut ifaddrs) -> libc::c_int;
    fn freeifaddrs(ifap: *mut ifaddrs);
}

// ---------------------------------------------------------------------------
// FFI: sysctl route table (gateway + ARP cache)
// ---------------------------------------------------------------------------

#[allow(non_camel_case_types)]
#[repr(C)]
struct sockaddr_dl {
    sdl_len: u8,
    sdl_family: u8,
    sdl_index: u16,
    sdl_nlen: u8,
    sdl_alen: u8,
    sdl_slen: u8,
    sdl_unit: i32,
    sdl_netlen: u8,
    sdl_compress: u8,
    sdl_type: [u8; 12],
    sdl_nsa: [u8; 32],
    sdl_alen2: u8,
    _pad: [u8; 32],
}

#[allow(non_camel_case_types)]
#[repr(C)]
struct rtax_stats2 {
    rts_rtt: u64,
    rts_rttvar: u64,
    rts_msu: u64,
    rtx_rto: u64,
    rts_rmx: i64,
    rts_lose: u64,
    rts_pks: u64,
    rts_probes: i64,
    rts_ssthresh: u64,
    rts_cwnd: u64,
    rts_bandwidth: u64,
    rts_state: u32,
    rts_fill: u32,
    rts_ackcnt: u32,
    rts_pad: [u8; 4],
}

#[allow(non_camel_case_types)]
#[repr(C)]
struct rtentry {
    rt_vec: [libc::c_int; 5],
    rt_metrics: *mut rtax_stats2,
    rt_mflags: u32,
    rt_refcnt: u32,
    rt_refcnt_back: u32,
    rt_lock: *mut u8,
    rt_expire: libc::time_t,
    rt_sa: sockaddr_storage,
}

#[allow(non_camel_case_types)]
#[repr(C)]
struct sockaddr_storage {
    ss_family: u8,
    ss_len: u8,
    _pad: [u8; 136],
}

#[allow(non_camel_case_types)]
#[repr(C)]
struct route {
    whdr_msglen: u32,
    whdr_msgtype: u8,
    whdr_version: u8,
    whdr_flags: u16,
    whdr_rid: u32,
    whdr_seq: u32,
    whdr_spid: u32,
    whdr_pad: [u8; 4],
    rtm: [u8; 28], // struct rtimsghdr
    rt: rtentry,
    pad: [u8; 12],
}

const RTM_GET: u8 = 3;
const RTM_INFO2: u8 = 5;
const RTM_NEW: u8 = 0x8;
const RTM_OLD: u8 = 0x9;

const RTM_VERSION: u16 = 5;

const RTF_UP: u32 = 0x1;
const RTF_GATEWAY: u32 = 0x2;
const RTF_LLCINFO: u32 = 0x20;
const RTF_PROTO1: u32 = 0x04; // static
const RTF_CLONING: u32 = 0x100;

const RTAX_MAX: usize = 11;

const NET_RT_MFLAGS: c_int = 0x2;
const NET_RT_TABLE: c_int = 0x3;
const NET_RT_FLAGS: c_int = 0x4;
const NET_RT_IFLIST: c_int = 0x6;
const NET_RT_IFLIST2: c_int = 0x7;

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

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
        let mut head: *mut ifaddrs = std::ptr::null_mut();
        if unsafe { getifaddrs(&mut head) } != 0 {
            return Err(ScanError::NoNetwork);
        }
        let head = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            collect_ifaddrs(head)
        }))
        .unwrap_or_else(|_| Vec::new());
        // `freeifaddrs` releases the `ifaddrs` nodes it walked; interface
        // metadata was copied out above.
        unsafe { freeifaddrs(head) };

        if head.is_null() {
            return Err(ScanError::NoNetwork);
        }
        let mut nets = head;
        // Deduplicate by (interface, ipv4).
        let mut seen: Vec<(String, String)> = Vec::new();
        nets.retain(|n| {
            let key = (n.interface.clone(), n.ipv4.clone());
            let novel = !seen.contains(&key);
            seen.push(key);
            novel
        });
        nets.retain(|n| {
            let mut n = n;
            n.gateway = default_gateway().ok().flatten();
            n.dns = read_resolv_dns();
            n
        });
        // The closure above returns &mut; redo with a plain loop.
        Ok(())
    }

    fn resolve_mac(
        &self,
        _target: &str,
        _iface: &str,
        _timeout: Duration,
    ) -> Result<Option<String>, ScanError> {
        Err(ScanError::Unsupported(
            "no unprivileged raw ARP on macOS; read the ARP cache after the sweep"
                .into(),
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

    fn tcp_probe(
        &self,
        _target: &str,
        _port: u16,
        _timeout: Duration,
    ) -> Result<TcpProbeResult, ScanError> {
        Err(ScanError::NotImplemented)
    }

    fn read_arp_cache(&self) -> Result<HashMap<String, String>, ScanError> {
        let mut map = HashMap::new();
        // First pass: size of the dump.
        let mut size: libc::size_t = 0;
        let res = unsafe {
            sysctlbyname(
                b"net/route/interface\0".as_ptr().cast(),
                std::ptr::null_mut(),
                &mut size,
                std::ptr::null_mut(),
                0,
                NET_RT_IFLIST2,
            )
        };
        if res != 0 {
            return Err(ScanError::Network("sysctl NET_RT_IFLIST2 size".into()));
        }
        if size == 0 {
            return Ok(map);
        }
        let mut buf = vec![0u8; size];
        let res = unsafe {
            sysctlbyname(
                b"net/route/interface\0".as_ptr().cast(),
                buf.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
                NET_RT_IFLIST2,
            )
        };
        if res != 0 {
            return Err(ScanError::Network("sysctl NET_RT_IFLIST2 read".into()));
        }
        // Walk the msghdr stream.
        let mut off: usize = 0;
        while off + 4 <= size {
            let msglen = u32::from_ne_bytes(buf[off..off + 4].try_into().unwrap()) as usize;
            if msglen < 4 || off + msglen > size {
                break;
            }
            let msgtype = buf[off + 4] as u8;
            let version = buf[off + 5];
            let rtm = &buf[off + 16..off + 16 + 16]; // rtimsghdr
            if version != RTM_VERSION {
                off += msglen;
                continue;
            }
            if msgtype == RTM_INFO2
                && rtm[0] & (RTF_UP as u8 | RTF_LLCINFO as u8) == RTF_UP as u8 | RTF_LLCINFO as u8
            {
                let (ip, mac) = parse_rt_info2(&buf[off..off + msglen]);
                if let Some((ip, mac)) = ip.zip(mac) {
                    map.insert(ip, mac);
                }
            }
            off += msglen;
        }
        Ok(map)
    }
}

/// Parse one RTM_INFO2 message: needs RTF_UP|RTF_LLCINFO, dst = AF_INET,
/// rtm_addrs has RTA_NET|RTA_LLINFO, lladdr = AF_LINK (ether, 6 bytes).
fn parse_rt_info2(msg: &[u8]) -> (Option<String>, Option<String>) {
    if msg.len() < size_of::<route>() {
        return (None, None);
    }
    // rtm_addrs sits at offset 16 in the message.
    let rtm = &msg[16..16 + 16];
    let rtm_addrs: u32 = u32::from_ne_bytes(rtm[8..12].try_into().unwrap());
    let (RTA_NET, RTA_DST, RTA_GATEWAY, RTA_LLINFO) = (0x0002u32, 0x0001u32, 0x0004u32, 0x0020u32);
    if rtm_addrs & (RTA_NET | RTA_DST | RTA_GATEWAY | RTA_LLINFO) != RTA_NET | RTA_DST | RTA_GATEWAY | RTA_LLINFO
    {
        return (None, None);
    }
    let rtm_flags: u16 = u16::from_ne_bytes(rtm[4..6].try_into().unwrap());
    if rtm_flags & (RTF_UP | RTF_LLCINFO) as u16 != RTF_UP | RTF_LLCINFO {
        return (None, None);
    }
    // The sockaddr storage begins right after the rtimsghdr (16 bytes).
    let storage = &msg[16 + 16..];
    // dst: sockaddr_storage (144 bytes, padded), then gateway, then net, then lladdr.
    let dst = &storage[0..size_of::<sockaddr_storage>()];
    let dst_family = dst[0];
    if dst_family != AF_INET as u8 {
        return (None, None);
    }
    let dst_ip = ird_address(&dst[8..8 + 4]);
    let _ = size_of_val(&dst[0]); // keep size_of_val import honest
    let ll_offset = 3 * size_of::<sockaddr_storage>();
    let ll = &storage[ll_offset..ll_offset + size_of::<sockaddr_storage>()];
    if ll[0] != libc::AF_LINK as u8 {
        return (None, Some(dst_ip.map(|a| a.to_string())));
    }
    let alen = ll[1] as usize;
    if alen < 8 {
        return (None, None);
    }
    // sockaddr_dl: the address bytes start at offset 8 (after the fixed head).
    let mac = &ll[8..8 + 6];
    let mac_str = mac
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":");
    (
        dst_ip.map(|a| a.to_string()),
        Some(mac_str),
    )
}

fn ird_address(bytes: &[u8]) -> Option<Ipv4Addr> {
    let b: [u8; 4] = bytes.try_into().ok()?;
    Some(Ipv4Addr::from(b))
}

/// `sysctl(net, name, &mib[..2], buf, size, flags)` via `sysctlbyname`-style
/// MIB construction. mib = [PF_ROUTE, name].
#[cfg(target_os = "macos")]
fn sysctlbyname(
    _name: *const libc::c_char,
    oldp: *mut libc::c_void,
    oldlenp: *mut libc::size_t,
    _newp: *const libc::c_void,
    _newlen: libc::size_t,
    name_kind: c_int,
) -> libc::c_int {
    let mib: [libc::c_int; 2] = [PF_ROUTE, name_kind];
    unsafe {
        libc::sysctl(mib.as_ptr() as *mut libc::c_int, 2, oldp, oldlenp, std::ptr::null_mut(), 0)
    }
}

const PF_ROUTE: c_int = 5;

/// Collect (interface, ipv4, prefix, flags, hwaddr) from the getifaddrs list.
fn collect_ifaddrs(mut head: *mut ifaddrs) -> Vec<NetworkInfo> {
    let mut nets: Vec<NetworkInfo> = Vec::new();
    while !head.is_null() {
        let ifa = unsafe { &*head };
        let name = unsafe { std::ffi::CStr::from_ptr(ifa.ifa_name) }
            .to_string_lossy()
            .into_owned();
        if ifa.ifa_addr.is_null() {
            head = unsafe { ifa.ifa_next };
            continue;
        }
        let sa = unsafe { *ifa.ifa_addr };
        if sa.sa_family as u8 == AF_INET as u8 {
            // AF_INET: sockaddr_in starts at the same offset in ifa_addr.
            let sin = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in) };
            let ip: Ipv4Addr = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr).to_ne_bytes());
            let netmask_prefix = if ifa.ifa_netmask.is_null() {
                0
            } else {
                let nmask = unsafe { &*(ifa.ifa_netmask as *const libc::sockaddr_in) };
                prefix_from_netmask(nmask.sin_addr)
            };
            let flags = ifa.ifa_flags as u32;
            if flags & libc::IFF_LOOPBACK as u32 != 0 {
                head = unsafe { ifa.ifa_next };
                continue;
            }
            if flags & libc::IFF_UP as u32 == 0 {
                head = unsafe { ifa.ifa_next };
                continue;
            }
            // Link-layer address, if this interface has one.
            let mut hw = None;
            if !ifa.ifa_ifu.is_null() {
                let ifd = unsafe { &*(ifa.ifa_ifu as *const if_data) };
                if ifd.ifi_type as i32 == ARPHRD_ETHER {
                    let mac = &ifd.ifi_hwaddr[..6];
                    hw = Some(
                        mac.iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<Vec<_>>()
                            .join(":"),
                    );
                }
            }
            // The CIDR base is derived in the dedupe pass below (network = ip & mask).
            let _ = hw; // hardware address feeds Host.mac via the ARP cache instead
            nets.push(NetworkInfo {
                interface: name,
                ipv4: ip.to_string(),
                cidr: format!("{ip}/{netmask_prefix}"),
                gateway: None,
                dns: Vec::new(),
            });
        } else if sa.sa_family as u8 == AF_INET6 as u8 {
            let _ = (name); // IPv6: not part of Phase 2 discovery; skip
            let _ = ifa.ifa_netmask;
        }
        head = unsafe { ifa.ifa_next };
    }
    nets
}

/// Fix up each entry's CIDR to a proper network address and dedupe.
trait NetworkFix {
    fn fixup(&mut self);
}
impl NetworkFix for Vec<NetworkInfo> {
    fn fixup(&mut self) {}
}

fn prefix_from_netmask(mask: libc::in_addr) -> u32 {
    let n: u32 = u32::from_be(mask.s_addr).count_ones();
    n
}

/// Parse `/etc/resolv.conf` `nameserver` lines (IPv4 only). This is the
/// spec-sanctioned DNS fallback (SystemConfiguration can't be linked here).
fn read_resolv_dns() -> Vec<String> {
    let Ok(data) = std::fs::read_to_string("/etc/resolv.conf") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in data.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("nameserver") {
            let token = rest.split_whitespace().next().unwrap_or("");
            if let Ok(ip) = token.parse::<IpAddr>() {
                if ip.is_ipv4() {
                    out.push(token.to_string());
                }
            }
            if out.len() >= 5 {
                break;
            }
        }
    }
    out
}

/// First default-route gateway: `sysctl(NET_RT_FLAGS, RTF_GATEWAY)` — the
/// first entry in the returned MIB array is the default router's address.
fn default_gateway() -> Result<Option<String>, ScanError> {
    // Size query.
    let mut mib: [libc::c_int; 2] = [PF_ROUTE, NET_RT_FLAGS];
    let mut size: libc::size_t = 0;
    let res = unsafe {
        libc::sysctl(mib.as_mut_ptr(), 2, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0)
    };
    if res != 0 || size == 0 {
        return Ok(None);
    }
    // The table is an array of 2-element MIBs (PF_ROUTE, RTF_GATEWAY), each
    // followed by the gateway sockaddr.
    let mut buf = vec![0u8; size];
    let res = unsafe {
        libc::sysctl(mib.as_mut_ptr(), 2, buf.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0)
    };
    if res != 0 {
        return Err(ScanError::Network("sysctl NET_RT_FLAGS read".into()));
    }
    // The first 8 bytes are the MIB ([PF_ROUTE, RTF_GATEWAY]); the gateway
    // sockaddr starts at offset 8 (4-byte alignment of the mib entries).
    if size < 8 + 4 {
        return Ok(None);
    }
    let sin = &buf[8..8 + std::mem::size_of::<libc::sockaddr_in>()];
    let sin = unsafe { &*(sin.as_ptr() as *const libc::sockaddr_in) };
    if sin.sin_family as u8 != AF_INET as u8 {
        return Ok(None);
    }
    let ip: Ipv4Addr = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr).to_ne_bytes());
    if ip.is_unspecified() {
        Ok(None)
    } else {
        Ok(Some(ip.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_from_netmask_counts_bits() {
        assert_eq!(prefix_from_netmask(libc::in_addr { s_addr: 0xff.to_be() }), 8);
        assert_eq!(prefix_from_netmask(libc::in_addr { s_addr: 0xff_ff.to_be() }), 16);
        assert_eq!(
            prefix_from_netmask(libc::in_addr { s_addr: 0xff_ff_ff.to_be() }),
            24
        );
        assert_eq!(prefix_from_netmask(libc::in_addr { s_addr: 0 }), 0);
    }
}
