//! Default Unix implementation (Linux and other non-macOS/non-Android Unix).
//!
//! Used for local development on this build host and as the reference for the
//! unprivileged probe techniques. Phase 1 stubs.
//!
//! Capability notes (Linux, no root):
//! * **ICMP** — a "ping socket" (`SO_ICMP`) is available to users in the
//!   kernel's `net.ipv4.ping_group_range`, so unprivileged ICMP is *conditional*
//!   (`supports_unprivileged_icmp = true` — attempt, then fall back).
//! * **ARP** — raw ARP packet sockets need `CAP_NET_RAW`, so not unprivileged
//!   (`supports_unprivileged_arp = false`).
//! * **TCP connect** — always available.

use std::time::Duration;

use crate::error::ScanError;
use crate::platform::PlatformNet;
use crate::types::NetworkInfo;

pub struct UnixNet;

impl PlatformNet for UnixNet {
    fn supports_unprivileged_icmp(&self) -> bool {
        true
    }
    fn supports_unprivileged_arp(&self) -> bool {
        false
    }
    fn supports_tcp_connect(&self) -> bool {
        true
    }

    fn enumerate_networks(&self) -> Result<Vec<NetworkInfo>, ScanError> {
        // Phase 2: parse /sys/class/net or getifaddrs; derive CIDR + gateway.
        Err(ScanError::NotImplemented)
    }
    fn resolve_mac(
        &self,
        _target: &str,
        _iface: &str,
        _timeout: Duration,
    ) -> Result<Option<String>, ScanError> {
        Err(ScanError::NotImplemented)
    }
    fn ping(
        &self,
        _target: &str,
        _iface: &str,
        _timeout: Duration,
    ) -> Result<Option<u32>, ScanError> {
        Err(ScanError::NotImplemented)
    }
    fn tcp_probe(&self, _target: &str, _port: u16, _timeout: Duration) -> Result<bool, ScanError> {
        Err(ScanError::NotImplemented)
    }
    fn read_arp_cache(&self) -> Result<std::collections::HashMap<String, String>, ScanError> {
        // Phase 2: parse /proc/net/arp. Empty map = no MACs, scan still works.
        Ok(std::collections::HashMap::new())
    }
}
