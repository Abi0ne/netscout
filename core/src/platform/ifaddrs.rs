//! Interface and DNS helpers shared by the macOS and Linux implementations.
//!
//! * **Interface enumeration** — `getifaddrs(3)`: IPv4 address + netmask per
//!   interface, flag filters (up, not loopback). Same API on both OSes.
//! * **DNS servers** — `nameserver` lines of a `resolv.conf`-format file.

use std::ffi::CStr;
use std::net::{IpAddr, Ipv4Addr};

use crate::error::ScanError;
use crate::types::NetworkInfo;

/// IPv4 networks of every interface that is up and not loopback, deduplicated
/// by (interface, address). CIDRs are normalised to the network address.
/// Gateway and DNS are left empty for the caller to fill in.
pub(crate) fn collect_ifaddrs() -> Result<Vec<NetworkInfo>, ScanError> {
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

/// IPv4 `nameserver`s of the first readable file in `paths`.
pub(crate) fn read_resolv_dns(paths: &[&str]) -> Vec<String> {
    paths
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .map(|data| parse_resolv_dns(&data))
        .unwrap_or_default()
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
}
