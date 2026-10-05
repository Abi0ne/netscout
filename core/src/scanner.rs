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

use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};

use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;

use crate::engine;
use crate::error::ScanError;
use crate::platform;
use crate::types::{NetworkInfo, ScanConfig, ScanObserver, ScanProfile};

/// A long-lived scanner that owns a **private** tokio runtime.
///
/// Construct it through [`new_scanner`] (the FFI constructor).
#[derive(uniffi::Object)]
pub struct Scanner {
    /// Private multi-thread runtime. Every probe runs here, independent of the
    /// (non-tokio) FFI thread that constructed the `Scanner`. `None` only
    /// while being dropped.
    runtime: Option<Runtime>,
    /// Parent of every running scan's token. `cancel()` trips it and installs
    /// a fresh one, so later scans are unaffected.
    cancel: Mutex<CancellationToken>,
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
            runtime: Some(runtime),
            cancel: Mutex::new(CancellationToken::new()),
            injected_network: Mutex::new(None),
        })
    }

    /// Spawn `job` on the private runtime under a child of the cancel token.
    fn launch(
        &self,
        job: engine::ScanJob,
        observer: Box<dyn ScanObserver>,
    ) -> Result<(), ScanError> {
        let token = self
            .cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .child_token();
        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| ScanError::Internal("scanner is shutting down".into()))?;
        runtime.spawn(engine::run(job, Arc::from(observer), token));
        Ok(())
    }
}

impl Drop for Scanner {
    fn drop(&mut self) {
        self.cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .cancel();
        // Never blocks, so dropping the last handle is safe from any thread,
        // including from inside an observer callback.
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_background();
        }
    }
}

/// Construct the scanning engine. The returned handle is reference-counted:
/// drop it (or release it in Swift) and, when the last reference goes away,
/// running scans are cancelled and the private runtime is shut down.
#[uniffi::export]
pub fn new_scanner() -> Result<Arc<Scanner>, ScanError> {
    Scanner::build().map(Arc::new)
}

#[uniffi::export]
impl Scanner {
    /// Discover the local networks this host can scan (one [`NetworkInfo`] per
    /// viable interface). Non-privileged: uses OS interface enumeration only.
    /// Network info injected with `set_network_info` takes precedence.
    pub fn detect_networks(self: Arc<Self>) -> Result<Vec<NetworkInfo>, ScanError> {
        let injected = self
            .injected_network
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match injected {
            Some(info) => Ok(vec![info]),
            None => platform::current().enumerate_networks(),
        }
    }

    /// Inject network info from the platform. Android's engine has no raw
    /// interface enumeration, so Kotlin calls this with the known Wi-Fi subnet
    /// (and the app's own interface) before starting a scan.
    pub fn set_network_info(self: Arc<Self>, info: NetworkInfo) {
        *self
            .injected_network
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(info);
    }

    /// Begin a **non-blocking** scan. Returns immediately; results stream
    /// through `observer` on the private runtime. Stop it with
    /// [`Scanner::cancel`].
    ///
    /// Concurrency is bounded: at most `config.concurrency` hosts are in
    /// flight, and each host probes at most `config.per_host_concurrency`
    /// ports at a time. `timeout_ms = 0` selects the profile's default.
    pub fn start_scan(
        self: Arc<Self>,
        config: ScanConfig,
        observer: Box<dyn ScanObserver>,
    ) -> Result<(), ScanError> {
        if config.targets.is_empty() {
            return Err(ScanError::InvalidConfig("targets must not be empty".into()));
        }
        let job = engine::plan(&config)?;
        self.launch(job, observer)
    }

    /// Cancel every active scan (idempotent, non-blocking). In-flight probes
    /// stop at their next await point. The observer is **not** sent a
    /// cancellation event — the caller already knows it asked for this.
    pub fn cancel(self: Arc<Self>) -> Result<(), ScanError> {
        let old = std::mem::replace(
            &mut *self.cancel.lock().unwrap_or_else(|e| e.into_inner()),
            CancellationToken::new(),
        );
        old.cancel();
        Ok(())
    }

    /// Scan a single host and stream its data through `observer` using the same
    /// protocol as `start_scan` (`on_host` → `on_finished`). Non-blocking.
    pub fn scan_host(
        self: Arc<Self>,
        ip: String,
        observer: Box<dyn ScanObserver>,
    ) -> Result<(), ScanError> {
        let ip = ip.trim();
        if ip.is_empty() {
            return Err(ScanError::InvalidConfig("ip must not be empty".into()));
        }
        if ip.parse::<Ipv4Addr>().is_err() {
            return Err(ScanError::InvalidConfig(format!(
                "'{ip}' is not an IPv4 address"
            )));
        }
        let job = engine::plan(&ScanConfig {
            targets: vec![ip.to_string()],
            profile: ScanProfile::Deep,
            concurrency: 1,
            per_host_concurrency: 16,
            timeout_ms: 0,
        })?;
        self.launch(job, observer)
    }
}
