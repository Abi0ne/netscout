//! The single error type that crosses the FFI boundary.

use thiserror::Error as ThisError;

/// Every error the engine can report. Exposed to Swift/Kotlin as a flat error
/// (a single foreign error type carrying a variant name + `Display` message).
#[derive(Debug, ThisError, uniffi::Error)]
#[uniffi(flat_error)]
pub enum ScanError {
    /// A scan was cancelled by the caller before completion.
    #[error("scan cancelled")]
    Cancelled,

    /// The operation is not possible on this platform without privileges.
    #[error("unsupported on this platform: {0}")]
    Unsupported(String),

    /// A network-level failure (socket error, ICMP unreachable, drop, …).
    #[error("network error: {0}")]
    Network(String),

    /// No scannable network interface could be found.
    #[error("no network interface found")]
    NoNetwork,

    /// The scan configuration is invalid.
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    /// A per-probe or overall timeout was exceeded.
    #[error("timed out: {0}")]
    Timeout(String),

    /// The feature is not yet implemented (Phase 1 stubs return this).
    #[error("not implemented")]
    NotImplemented,

    /// An unexpected internal failure.
    #[error("internal error: {0}")]
    Internal(String),
}

impl ScanError {
    /// Convenience for the scan loop: is this the cancellation signal?
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}
