//! # netscout-core
//!
//! A fast, **non-privileged** LAN network scanner engine written in Rust and
//! shared verbatim across platforms over [UniFFI](https://unidiffic.github.io).
//! The macOS (Swift) and later Android (Kotlin) apps are thin UIs over this
//! crate: they never contain scanning logic.
//!
//! ## Public FFI surface
//! * `new_scanner()` — the FFI constructor, returning a reference-counted
//!   [`Scanner`] handle (associated constructors are not supported in
//!   library-mode proc-macro UniFFI, so the constructor is a free function).
//! * [`Scanner`] — the engine object: `detect_networks`, `set_network_info`,
//!   `start_scan`, `cancel`, `scan_host`.
//! * [`ScanObserver`] — callback interface the engine streams results through.
//! * [`ScanError`] — the FFI error enum.
//! * Records: `Host`, `Port`, `ServiceInfo`, `SsdpInfo`, `NetworkInfo`, `ScanConfig`,
//!   `Progress`, `Summary`, `DeviceTypeCount`.
//! * Enums: `DeviceType`, `Transport`, `PortState`, `ScanProfile`, `ScanPhase`.
//!
//! ## Design notes
//! * **Threading** — `Scanner` owns a private `tokio` multi-thread runtime, so a
//!   scanner created on any FFI thread still runs its async probes.
//! * **Cancellation** — a global `CancellationToken` (tripped by `cancel()`) plus
//!   per-scan child tokens let in-flight work stop at the next await point.
//! * **FFI safety** — results cross the boundary only through the `ScanObserver`
//!   callback interface (UniFFI-managed vtable, `Send + Sync + 'static`) and the
//!   `ScanError` type; no raw pointers or unwinds ever cross the boundary.

pub mod error;
pub mod platform;
pub mod scanner;
pub mod types;

pub use error::ScanError;
pub use platform::requires_multicast_lock;
pub use scanner::{new_scanner, Scanner};
pub use types::{
    DeviceType, DeviceTypeCount, Host, NetworkInfo, Port, PortState, Progress, ScanConfig,
    ScanObserver, ScanPhase, ScanProfile, ServiceInfo, SsdpInfo, Summary, Transport,
};

// Wire every `#[derive(uniffi::…)]` / `#[uniffi::…]` item above into the FFI
// scaffolding. Must be the last item in the crate.
uniffi::setup_scaffolding!();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanner_new_ok() {
        assert!(
            new_scanner().is_ok(),
            "runtime creation should succeed in tests"
        );
    }

    #[test]
    fn detect_networks_is_stubbed() {
        let s = new_scanner().expect("runtime ok");
        assert!(matches!(
            s.clone().detect_networks(),
            Err(ScanError::NotImplemented)
        ));
    }

    #[test]
    fn error_messages_render() {
        let e = ScanError::Unsupported("no raw sockets".into());
        assert_eq!(
            e.to_string(),
            "unsupported on this platform: no raw sockets"
        );
        assert!(ScanError::Cancelled.is_cancelled());
        assert!(!ScanError::Network("x".into()).is_cancelled());
    }

    #[test]
    fn requires_multicast_lock_matches_target() {
        assert_eq!(requires_multicast_lock(), cfg!(target_os = "android"));
    }
}
