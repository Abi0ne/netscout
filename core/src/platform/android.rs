//! Android implementation of [`PlatformNet`].
//!
//! On Android the engine has **no raw sockets**: the app process cannot open
//! the raw/ICMP/packet sockets that ARP- and ping-based discovery need, and
//! interface enumeration is the platform's job. Kotlin therefore supplies the
//! network via `Scanner::set_network_info(…)` and drives discovery; every
//! primitive here reports [`ScanError::Unsupported`] in Phase 1.
//!
//! Note for Phase 4: Android *does* permit ordinary TCP/UDP `connect()` from
//! the app process, so the later port-scanning path will implement
//! `tcp_probe` for Android rather than leaving it `Unsupported`.

use std::time::Duration;

use crate::error::ScanError;
use crate::platform::PlatformNet;
use crate::types::NetworkInfo;

pub struct AndroidNet;

impl PlatformNet for AndroidNet {
    fn supports_unprivileged_icmp(&self) -> bool {
        false
    }
    fn supports_unprivileged_arp(&self) -> bool {
        false
    }
    fn supports_tcp_connect(&self) -> bool {
        false
    }

    fn enumerate_networks(&self) -> Result<Vec<NetworkInfo>, ScanError> {
        Err(ScanError::Unsupported(
            "Android injects network info from Kotlin via set_network_info".into(),
        ))
    }
    fn resolve_mac(
        &self,
        _target: &str,
        _iface: &str,
        _timeout: Duration,
    ) -> Result<Option<String>, ScanError> {
        Err(ScanError::Unsupported(
            "no raw ARP sockets on Android".into(),
        ))
    }
    fn ping(
        &self,
        _target: &str,
        _iface: &str,
        _timeout: Duration,
    ) -> Result<Option<u32>, ScanError> {
        Err(ScanError::Unsupported(
            "no unprivileged ICMP on Android".into(),
        ))
    }
    fn tcp_probe(&self, _target: &str, _port: u16, _timeout: Duration) -> Result<bool, ScanError> {
        Err(ScanError::Unsupported(
            "no engine sockets on Android in Phase 1".into(),
        ))
    }
}
