//! Public data types exposed across the FFI boundary.
//!
//! Conventions:
//! * IP addresses, MACs, hostnames and CIDRs are plain `String`s — trivially
//!   FFI-safe and unambiguous for a UI. (Internally the engine parses them.)
//! * Timestamps are **epoch milliseconds** (`i64`).
//! * `rtt_ms` is `f64` to preserve sub-millisecond precision on a LAN.

/// Coarse device classification, used for grouping and UI iconography.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DeviceType {
    Router,
    Modem,
    Computer,
    Laptop,
    Server,
    Phone,
    Tablet,
    Printer,
    Camera,
    Nvr,
    Nasc,
    SmartTv,
    Gaming,
    Voip,
    Audio,
    Iot,
    Other,
    Unknown,
}

/// L4 transport of a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Transport {
    Tcp,
    Udp,
}

/// State of a port after a probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PortState {
    Open,
    Closed,
    Filtered,
    OpenFiltered,
}

/// High-level scan depth. Selects the port list, probe timeout, and the
/// breadth of service identification (mDNS/SSDP/banner grabbing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ScanProfile {
    Quick,
    Standard,
    Deep,
}

/// Which phase the current scan is in (surfaced via `Progress`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ScanPhase {
    Resolving,
    Probing,
    PortScanning,
    Done,
}

/// A single probed port on a host.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Port {
    pub number: u16,
    pub transport: Transport,
    pub state: PortState,
    /// IANA service name when the port maps to a known one (e.g. "http").
    pub service: Option<String>,
    /// Captured service banner/version, if any.
    pub version: Option<String>,
}

/// An mDNS (DNS-SD) advertised service observed on a host.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ServiceInfo {
    /// Service type, e.g. "_http._tcp" or "_airplay._tcp".
    pub service_type: String,
    /// Instance name, e.g. "Living Room TV".
    pub name: String,
    /// Owning hostname or IP.
    pub host: String,
    pub port: u16,
    pub domain: Option<String>,
    /// TXT record values (service parameters).
    pub txt: Vec<String>,
}

/// A UPnP/SSDP device descriptor observed on a host.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SsdpInfo {
    /// Unique Service Name, e.g. "uuid:…".
    pub usn: String,
    /// HTTP location of the device description.
    pub location: String,
    /// `SERVER` header, used for vendor fingerprinting.
    pub server: Option<String>,
    pub friendly_name: Option<String>,
}

/// A device discovered on the network, with everything we currently know.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Host {
    /// IPv4 address, dotted (e.g. "192.168.1.24").
    pub ip: String,
    /// MAC formatted "aa:bb:cc:dd:ee:ff"; `None` if unresolvable without privileges.
    pub mac: Option<String>,
    /// OUI vendor (e.g. "Apple, Inc."), best-effort.
    pub vendor: Option<String>,
    pub hostnames: Vec<String>,
    pub device_type: DeviceType,
    pub open_ports: Vec<Port>,
    /// Round-trip time in ms (sub-ms precision), if a live probe succeeded.
    pub rtt_ms: Option<f64>,
    pub mdns_services: Vec<ServiceInfo>,
    pub ssdp_info: Option<SsdpInfo>,
    /// Epoch ms when first seen this session.
    pub first_seen: i64,
    /// Epoch ms of the most recent successful probe.
    pub last_seen: i64,
}

/// A scannable network, as derived from a single interface.
#[derive(Debug, Clone, uniffi::Record)]
pub struct NetworkInfo {
    /// Interface name, e.g. "en0".
    pub interface: String,
    /// IPv4 address assigned to the interface.
    pub ipv4: String,
    /// Network in CIDR form, e.g. "192.168.1.0/24".
    pub cidr: String,
    /// Default gateway, if the OS reports one for this interface.
    pub gateway: Option<String>,
    /// Configured DNS servers for this interface.
    pub dns: Vec<String>,
}

/// A full scan request.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ScanConfig {
    /// Target specs: each entry is an IPv4 address ("192.168.1.10") or a CIDR
    /// ("192.168.1.0/24"). The engine expands CIDRs into concrete /32s.
    pub targets: Vec<String>,
    pub profile: ScanProfile,
    /// Maximum hosts scanned concurrently (global cap).
    pub concurrency: u32,
    /// Maximum ports probed concurrently per host.
    pub per_host_concurrency: u32,
    /// Per-probe timeout in milliseconds.
    pub timeout_ms: u64,
}

/// Live progress of a running scan.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Progress {
    pub total_hosts: u32,
    pub scanned_hosts: u32,
    pub discovered_hosts: u32,
    pub open_ports_found: u32,
    pub elapsed_ms: u64,
    pub phase: ScanPhase,
}

/// Per-`DeviceType` aggregate in a [`Summary`].
#[derive(Debug, Clone, uniffi::Record)]
pub struct DeviceTypeCount {
    pub device_type: DeviceType,
    pub count: u32,
}

/// Final result of a completed scan.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Summary {
    pub total_hosts: u32,
    pub scanned_hosts: u32,
    pub discovered_hosts: u32,
    pub total_open_ports: u32,
    pub elapsed_ms: u64,
    pub by_device_type: Vec<DeviceTypeCount>,
}

/// Callback interface the engine streams results to the platform.
///
/// Methods take `&self`; a call from the engine lifts the foreign
/// implementation (UniFFI-managed vtable) from a handle and forwards through
/// it. The engine holds a `Box<dyn ScanObserver>` for the duration of a scan.
#[uniffi::export(callback_interface)]
pub trait ScanObserver: Send + Sync + 'static {
    /// A host was discovered, or its data was updated. May fire multiple times
    /// per host as data accumulates (identity = `Host.ip`).
    fn on_host(&self, host: Host);

    /// Periodic progress updates while the scan is running.
    fn on_progress(&self, progress: Progress);

    /// The scan finished normally. Emitted exactly once, after the final `on_host`.
    fn on_finished(&self, summary: Summary);

    /// A non-fatal error occurred mid-scan; the scan may continue.
    fn on_error(&self, message: String);
}
