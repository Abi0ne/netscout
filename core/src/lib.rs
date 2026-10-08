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
//!   `start_scan`, `cancel`, `scan_host`, `wake_on_lan`.
//! * [`ScanObserver`] — callback interface the engine streams results through.
//! * [`ProfileStore`] (via `open_profile_store(dir)`) — saved scans:
//!   `list`, `load`, `load_all`, `save`, `update`, `set_notes`, `rename`, `delete`;
//!   `diff_hosts` compares a scan with a saved profile (`ScanDiff`, `HostChange`);
//!   `match_profile`/`host_matches` search profiles (notes by `device_key`);
//!   `recognize_network` finds the profile of the network being scanned
//!   (`NetworkMatch`); `profiles_csv` exports profiles as CSV.
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

pub mod classify;
mod engine;
pub mod error;
pub mod export;
pub mod icmp;
pub mod liveness;
pub mod names;
pub mod oui;
pub mod platform;
pub mod profiles;
pub mod recognize;
pub mod scanner;
pub mod targets;
pub mod tcp_probe;
pub mod types;
pub mod wol;

pub use error::ScanError;
pub use export::profiles_csv;
pub use platform::requires_multicast_lock;
pub use profiles::{
    device_key, diff_hosts, host_matches, match_profile, open_profile_store, HostChange,
    ProfileMatch, ProfileStore, ProfileSummary, SavedProfile, ScanDiff,
};
pub use recognize::{recognize_network, NetworkMatch};
pub use scanner::{new_scanner, Scanner};
pub use types::{
    DeviceType, DeviceTypeCount, Host, NetworkInfo, Port, PortState, Progress, ScanConfig,
    ScanObserver, ScanPhase, ScanProfile, ServiceInfo, SsdpInfo, Summary, Transport,
};

/// The workspace version (`[workspace.package] version`), for UIs built in
/// Rust that link the core directly (the Linux app).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn detect_networks_reads_the_os() {
        let s = new_scanner().expect("runtime ok");
        match s.detect_networks() {
            Ok(nets) => assert!(!nets.is_empty()),
            Err(e) => assert!(matches!(e, ScanError::NoNetwork), "{e}"),
        }
    }

    #[test]
    fn injected_network_takes_precedence() {
        let s = new_scanner().expect("runtime ok");
        let info = NetworkInfo {
            interface: "wlan0".into(),
            ipv4: "192.168.1.5".into(),
            cidr: "192.168.1.0/24".into(),
            gateway: Some("192.168.1.1".into()),
            dns: vec![],
        };
        s.clone().set_network_info(info);
        let nets = s.detect_networks().unwrap();
        assert_eq!(nets.len(), 1);
        assert_eq!(nets[0].interface, "wlan0");
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

    /// Observer that forwards `on_finished` to a channel.
    struct Done(std::sync::Mutex<std::sync::mpsc::Sender<Summary>>);

    impl ScanObserver for Done {
        fn on_host(&self, _host: Host) {}
        fn on_progress(&self, _progress: Progress) {}
        fn on_finished(&self, summary: Summary) {
            let _ = self.0.lock().unwrap().send(summary);
        }
        fn on_error(&self, _message: String) {}
    }

    fn config(target: &str) -> ScanConfig {
        ScanConfig {
            targets: vec![target.into()],
            profile: ScanProfile::Quick,
            concurrency: 16,
            per_host_concurrency: 4,
            timeout_ms: 300,
        }
    }

    #[test]
    fn loopback_scan_finishes_with_host_up() {
        let s = new_scanner().expect("runtime ok");
        let (tx, rx) = std::sync::mpsc::channel();
        s.clone()
            .start_scan(config("127.0.0.1"), Box::new(Done(tx.into())))
            .unwrap();
        let summary = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("scan finished");
        assert_eq!(summary.scanned_hosts, 1);
        assert_eq!(summary.discovered_hosts, 1);
    }

    #[test]
    fn cancel_stops_scan_and_later_scans_still_run() {
        let s = new_scanner().expect("runtime ok");
        // TEST-NET-1 (RFC 5737): nothing answers, so the scan would take a while.
        let (tx, rx) = std::sync::mpsc::channel();
        s.clone()
            .start_scan(config("192.0.2.0/24"), Box::new(Done(tx.into())))
            .unwrap();
        s.clone().cancel().unwrap();
        assert!(rx.recv_timeout(std::time::Duration::from_secs(3)).is_err());

        let (tx, rx) = std::sync::mpsc::channel();
        s.clone()
            .start_scan(config("127.0.0.1"), Box::new(Done(tx.into())))
            .unwrap();
        assert!(rx.recv_timeout(std::time::Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn requires_multicast_lock_matches_target() {
        assert_eq!(requires_multicast_lock(), cfg!(target_os = "android"));
    }
}
