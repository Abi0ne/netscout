//! Default Unix implementation (Linux and other non-macOS/non-Android Unix).
//!
//! Everything here is unprivileged and does **not** shell out:
//!
//! * **Interface enumeration** — `getifaddrs(3)`, shared with macOS
//!   ([`super::ifaddrs`]).
//! * **Default gateway** — `/proc/net/route`: the `00000000` destination entry
//!   flagged `RTF_GATEWAY`, attributed to its `Iface` column.
//! * **DNS servers** — `/run/systemd/resolve/resolv.conf` when present (the
//!   real upstream servers behind systemd-resolved), else `/etc/resolv.conf`
//!   (which under systemd-resolved only names the `127.0.0.53` stub).
//! * **ARP cache** — `/proc/net/arp`, complete (`ATF_COM`) entries only.
//!
//! Capability notes (Linux, no root):
//! * **ICMP** — a "ping socket" is available to users in the kernel's
//!   `net.ipv4.ping_group_range`, so unprivileged ICMP is *conditional*; the
//!   engine's `PingClient` checks [`crate::icmp::ping_available`].
//! * **ARP** — raw ARP packet sockets need `CAP_NET_RAW`, so not unprivileged
//!   (`supports_unprivileged_arp = false`); MACs come from the ARP cache.
//! * **TCP connect** — always available.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::time::Duration;

use crate::error::ScanError;
use crate::platform::{ifaddrs, PlatformNet};
use crate::types::NetworkInfo;

/// `RTF_GATEWAY` route flag (`<linux/route.h>`).
const RTF_GATEWAY: u32 = 0x2;
/// `ATF_COM` ARP flag: the entry holds a resolved hardware address.
const ATF_COM: u32 = 0x2;

const RESOLV_PATHS: &[&str] = &["/run/systemd/resolve/resolv.conf", "/etc/resolv.conf"];

pub struct UnixNet;

impl PlatformNet for UnixNet {
    fn supports_unprivileged_icmp(&self) -> bool {
        crate::icmp::ping_available()
    }
    fn supports_unprivileged_arp(&self) -> bool {
        false
    }
    fn supports_tcp_connect(&self) -> bool {
        true
    }

    fn enumerate_networks(&self) -> Result<Vec<NetworkInfo>, ScanError> {
        let mut nets = ifaddrs::collect_ifaddrs()?;
        if nets.is_empty() {
            return Err(ScanError::NoNetwork);
        }
        let gateways = std::fs::read_to_string("/proc/net/route")
            .map(|d| parse_default_gateways(&d))
            .unwrap_or_default();
        let dns = ifaddrs::read_resolv_dns(RESOLV_PATHS);
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
            "raw ARP needs CAP_NET_RAW on Linux; read the ARP cache after the sweep".into(),
        ))
    }

    fn ping(
        &self,
        _target: &str,
        _iface: &str,
        _timeout: Duration,
    ) -> Result<Option<u32>, ScanError> {
        // The engine's PingClient owns ICMP; this trait method stays a
        // capability marker.
        Err(ScanError::NotImplemented)
    }

    fn tcp_probe(&self, _target: &str, _port: u16, _timeout: Duration) -> Result<bool, ScanError> {
        // The engine's async `tcp_probe` module owns TCP connects.
        Err(ScanError::NotImplemented)
    }

    fn read_arp_cache(&self) -> Result<HashMap<String, String>, ScanError> {
        // No /proc (non-Linux Unix) = no MACs; the scan still works.
        Ok(std::fs::read_to_string("/proc/net/arp")
            .map(|d| parse_arp_cache(&d))
            .unwrap_or_default())
    }
}

/// Default gateway per interface from `/proc/net/route`. Columns: `Iface
/// Destination Gateway Flags …`; addresses are hex `u32`s in host byte order.
fn parse_default_gateways(data: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in data.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        let [iface, dst, gw, flags, ..] = cols[..] else {
            continue;
        };
        let (Ok(dst), Ok(gw), Ok(flags)) = (
            u32::from_str_radix(dst, 16),
            u32::from_str_radix(gw, 16),
            u32::from_str_radix(flags, 16),
        ) else {
            continue;
        };
        if dst == 0 && gw != 0 && flags & RTF_GATEWAY != 0 {
            out.entry(iface.to_string())
                .or_insert_with(|| Ipv4Addr::from(gw.to_ne_bytes()).to_string());
        }
    }
    out
}

/// IPv4 -> MAC from `/proc/net/arp`. Columns: `IP-address HW-type Flags
/// HW-address Mask Device`; incomplete and all-zero entries are skipped.
fn parse_arp_cache(data: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in data.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        let [ip, _hw_type, flags, mac, ..] = cols[..] else {
            continue;
        };
        let complete = flags
            .strip_prefix("0x")
            .and_then(|f| u32::from_str_radix(f, 16).ok())
            .is_some_and(|f| f & ATF_COM != 0);
        if !complete || mac == "00:00:00:00:00:00" || ip.parse::<Ipv4Addr>().is_err() {
            continue;
        }
        out.insert(ip.to_string(), mac.to_ascii_lowercase());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_gateway_from_proc_route() {
        // 0101A8C0 = 192.168.1.1 in little-endian host order.
        let data =
            "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                    enp6s0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
                    enp6s0\t0001A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n";
        let gws = parse_default_gateways(data);
        assert_eq!(gws.len(), 1);
        if cfg!(target_endian = "little") {
            assert_eq!(gws["enp6s0"], "192.168.1.1");
        }
    }

    #[test]
    fn arp_cache_keeps_complete_entries() {
        let data = "IP address       HW type     Flags       HW address            Mask     Device\n\
                    192.168.1.1      0x1         0x2         AA:BB:CC:01:02:03     *        enp6s0\n\
                    192.168.1.9      0x1         0x0         00:00:00:00:00:00     *        enp6s0\n";
        let arp = parse_arp_cache(data);
        assert_eq!(arp.len(), 1);
        assert_eq!(arp["192.168.1.1"], "aa:bb:cc:01:02:03");
    }

    /// Smoke test against the live OS: must not error, and every network has
    /// a well-formed CIDR.
    #[test]
    fn live_enumeration_and_arp_cache_do_not_fail() {
        if let Ok(nets) = UnixNet.enumerate_networks() {
            for n in nets {
                assert!(n.cidr.parse::<ipnet::Ipv4Net>().is_ok(), "{n:?}");
            }
        }
        assert!(UnixNet.read_arp_cache().is_ok());
    }
}
