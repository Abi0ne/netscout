//! The engine object exposed to platforms.
//!
//! FFI shape (proc-macro UniFFI): `Scanner` is a `uniffi::Object`, so the
//! platform holds an `Arc<Scanner>` (in Swift: a `Scanner` class instance).
//!
//! * The constructor is the free function [`new_scanner`] — `#[uniffi::export]`
//!   does not support associated functions, and object constructors in
//!   library-mode UniFFI are free functions.
//! * Exported methods take `self: Arc<Self>`: UniFFI lifts the `Arc` from the
//!   foreign handle on every call, so each call owns a refcount and the object
//!   lives as long as the platform holds it. The exported impl block **is** the
//!   implementation (no separate `&self` variants exist — a name cannot be
//!   overloaded by receiver type).
//!
//! ```text
//! let scanner = new_scanner()?;            // holds Arc<Scanner> internally
//! let nets = scanner.detect_networks()?;   // or set_network_info() on Android
//! scanner.start_scan(config, observer)?;   // returns immediately
//! … later, if the user hits Cancel:
//! scanner.cancel()?;
//! ```

use std::sync::{Arc, Mutex};

use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;

use crate::error::ScanError;
use crate::platform;
use crate::types::{NetworkInfo, ScanConfig, ScanObserver};

/// A long-lived scanner that owns a **private** tokio runtime.
///
/// Construct it through [`new_scanner`] (the FFI constructor).
#[derive(uniffi::Object)]
pub struct Scanner {
    /// Private multi-thread runtime. Every probe runs here, independent of the
    /// (non-tokio) FFI thread that constructed the `Scanner`.
    /// (Phase 1: declared but not yet consumed — scan tasks call `runtime.spawn`
    /// from `start_scan`/`scan_host` starting in Phase 2, which removes the allow.)
    #[allow(dead_code)]
    runtime: Runtime,
    /// Global switch: `cancel()` trips this, which drops every in-flight scan.
    cancel: CancellationToken,
    /// Network info injected by the platform (Android/Kotlin) when it cannot
    /// enumerate interfaces itself. `None` on platforms that can self-detect.
    injected_network: Mutex<Option<NetworkInfo>>,
}

impl Scanner {
    /// Create a scanner and start its private runtime.
    ///
    /// Fails only if the runtime cannot be created (e.g. process out of
    /// resources) — hence a `Result` rather than a fallible-free constructor.
    fn build() -> Result<Self, ScanError> {
        let runtime = Runtime::new()
            .map_err(|e| ScanError::Internal(format!("failed to create tokio runtime: {e}")))?;
        Ok(Self {
            runtime,
            cancel: CancellationToken::new(),
            injected_network: Mutex::new(None),
        })
    }
}

/// Construct the scanning engine. The returned handle is reference-counted:
/// drop it (or release it in Swift) and, when the last reference goes away,
/// the private tokio runtime is shut down with it.
#[uniffi::export]
pub fn new_scanner() -> Result<Arc<Scanner>, ScanError> {
    Scanner::build().map(Arc::new)
}

#[uniffi::export]
impl Scanner {
    /// Discover the local networks this host can scan (one [`NetworkInfo`] per
    /// viable interface). Non-privileged: uses OS interface enumeration only.
    pub fn detect_networks(self: Arc<Self>) -> Result<Vec<NetworkInfo>, ScanError> {
        // Phase 2: delegate to `platform::current().enumerate_networks()`.
        let _ = platform::current();
        Err(ScanError::NotImplemented)
    }

    /// Inject network info from the platform. Android's engine has no raw
    /// interface enumeration, so Kotlin calls this with the known Wi-Fi subnet
    /// (and the app's own interface) before starting a scan. A poisoned lock
    /// (previously poisoned by a panic) is ignored — injection simply doesn't
    /// take effect.
    pub fn set_network_info(self: Arc<Self>, info: NetworkInfo) {
        if let Ok(mut guard) = self.injected_network.lock() {
            *guard = Some(info);
        }
    }

    /// Begin a **non-blocking** scan. Returns immediately; results stream
    /// through `observer` on the private runtime. Stop it with
    /// [`Scanner::cancel`].
    ///
    /// Concurrency is bounded: a `tokio::sync::Semaphore` sized to
    /// `config.concurrency` caps how many hosts are in flight, and each host
    /// task gets a child of the per-scan cancellation token.
    pub fn start_scan(
        self: Arc<Self>,
        config: ScanConfig,
        observer: Box<dyn ScanObserver>,
    ) -> Result<(), ScanError> {
        if config.targets.is_empty() {
            return Err(ScanError::InvalidConfig("targets must not be empty".into()));
        }
        if config.concurrency == 0 {
            return Err(ScanError::InvalidConfig("concurrency must be > 0".into()));
        }
        // Phase 2: build a per-scan child `CancellationToken`, expand `targets`
        // into concrete /32s, create the bounded `Semaphore`, and spawn one task
        // per host on `self.runtime`, streaming to `observer`.
        let _ = observer;
        Err(ScanError::NotImplemented)
    }

    /// Cancel the active scan (idempotent, non-blocking). In-flight probes stop
    /// at their next await point. The observer is **not** sent a cancellation
    /// event — the caller already knows it asked for this.
    pub fn cancel(self: Arc<Self>) -> Result<(), ScanError> {
        self.cancel.cancel();
        Ok(())
    }

    /// Scan a single host and stream its data through `observer` using the same
    /// protocol as `start_scan` (`on_host` → `on_finished`). Non-blocking.
    pub fn scan_host(
        self: Arc<Self>,
        ip: String,
        observer: Box<dyn ScanObserver>,
    ) -> Result<(), ScanError> {
        if ip.is_empty() {
            return Err(ScanError::InvalidConfig("ip must not be empty".into()));
        }
        let _ = observer;
        Err(ScanError::NotImplemented)
    }
}
