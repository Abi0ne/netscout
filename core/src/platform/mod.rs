//! Platform-specific network primitives, isolated behind [`PlatformNet`] so the
//! scan engine stays 100% shared. The concrete implementation is selected at
//! compile time with `cfg(target_os)`.
//!
//! * `macos`  — getifaddrs + sysctl route dumps.
//! * `android`— engine has no raw sockets here; Kotlin drives discovery. All
//!   primitives report [`ScanError::Unsupported`] for now.
//! * `unix`   — default Unix (Linux) impl: getifaddrs + `/proc/net`.
//! * `ifaddrs`— interface/DNS helpers shared by `macos` and `unix`.

#[cfg(target_os = "android")]
pub mod android;
#[cfg(not(target_os = "android"))]
mod ifaddrs;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(not(any(target_os = "macos", target_os = "android")))]
pub mod unix;

use std::time::Duration;

use crate::error::ScanError;
use crate::types::NetworkInfo;

/// Capabilities + low-level probe primitives for the current OS.
///
/// Methods are synchronous and short-lived; the scan engine wraps them in
/// runtime tasks / `spawn_blocking` as needed and enforces timeouts above.
pub trait PlatformNet: Send + Sync {
    /// True if the OS permits unprivileged ICMP (ping) sockets.
    fn supports_unprivileged_icmp(&self) -> bool;
    /// True if the OS permits unprivileged raw ARP frames.
    fn supports_unprivileged_arp(&self) -> bool;
    /// True if `TCP connect()` can be used as a liveness probe.
    fn supports_tcp_connect(&self) -> bool;

    /// Enumerate scannable networks from the OS (non-privileged).
    fn enumerate_networks(&self) -> Result<Vec<NetworkInfo>, ScanError>;

    /// Resolve `target` to a MAC via ARP on `iface`. `None` if no reply.
    fn resolve_mac(
        &self,
        target: &str,
        iface: &str,
        timeout: Duration,
    ) -> Result<Option<String>, ScanError>;

    /// ICMP `target` on `iface`; returns RTT in milliseconds if it responds.
    fn ping(&self, target: &str, iface: &str, timeout: Duration) -> Result<Option<u32>, ScanError>;

    /// TCP-connect probe to `target:port` on `iface`; `true` on a successful connect.
    fn tcp_probe(&self, target: &str, port: u16, timeout: Duration) -> Result<bool, ScanError>;

    /// Read the OS ARP cache: map of IPv4 string -> MAC string.
    /// Read *after* the sweep so replies we just triggered are already cached.
    /// Platforms without an unprivileged ARP view return an empty map.
    fn read_arp_cache(&self) -> Result<std::collections::HashMap<String, String>, ScanError>;
}

/// The implementation for the current compilation target.
pub fn current() -> Box<dyn PlatformNet> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacOSNet)
    }
    #[cfg(target_os = "android")]
    {
        Box::new(android::AndroidNet)
    }
    #[cfg(not(any(target_os = "macos", target_os = "android")))]
    {
        Box::new(unix::UnixNet)
    }
}

/// True on platforms where multicast must be explicitly unlocked (Android's
/// `WifiManager` multicast lock). Other platforms default to multicast-on.
pub fn requires_multicast_lock() -> bool {
    cfg!(target_os = "android")
}
