//! The scan pipeline behind `Scanner::start_scan` / `Scanner::scan_host`.
//!
//! 1. **Plan** (synchronous, in the FFI call): parse and expand the targets,
//!    dedupe, apply profile defaults, reject oversized jobs.
//! 2. **Probe** (async, on the scanner's runtime): every host gets an ICMP
//!    echo and a TCP connect sweep over the profile's discovery ports, both
//!    concurrently. Hosts in flight are bounded by `concurrency`; open
//!    sockets by a budget derived from the process fd limit.
//! 3. **ARP** — after the sweep the OS ARP cache holds the neighbours our
//!    probes just resolved: their MACs are attached to known hosts, and
//!    in-target addresses that answered ARP but nothing else are reported too
//!    (devices that drop ping and every probed port).
//! 4. **Finish** — `on_finished` with the summary. A cancelled scan stops at
//!    the next await point and emits nothing further.

use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::error::ScanError;
use crate::icmp::{self, PingClient};
use crate::platform;
use crate::targets;
use crate::tcp_probe;
use crate::types::{
    DeviceType, DeviceTypeCount, Host, Port, PortState, Progress, ScanConfig, ScanObserver,
    ScanPhase, ScanProfile, Summary, Transport,
};

/// Largest job accepted (a /16).
pub const MAX_HOSTS: u64 = 65_536;
/// Upper bound on hosts in flight, whatever the config asks for.
const MAX_CONCURRENCY: usize = 1024;
/// Minimum interval between two `on_progress` events.
const PROGRESS_EVERY: Duration = Duration::from_millis(150);

/// Discovery ports per profile: common services across routers, computers,
/// phones, printers, NAS, cameras and media devices.
const QUICK_PORTS: &[u16] = &[22, 53, 80, 443, 445, 8080, 62078];
const STANDARD_PORTS: &[u16] = &[
    21, 22, 23, 53, 80, 139, 443, 445, 548, 554, 631, 1883, 3389, 5000, 7000, 8008, 8080, 8443,
    9100, 62078,
];
const DEEP_PORTS: &[u16] = &[
    21, 22, 23, 25, 53, 80, 110, 111, 135, 139, 143, 443, 445, 515, 548, 554, 631, 993, 1883, 2049,
    3000, 3306, 3389, 5000, 5001, 5432, 5900, 7000, 8000, 8008, 8009, 8080, 8443, 8888, 9000, 9100,
    32400, 49152, 62078,
];

/// A validated, expanded scan request.
#[derive(Debug, Clone)]
pub struct ScanJob {
    pub addrs: Vec<Ipv4Addr>,
    pub ports: &'static [u16],
    pub concurrency: usize,
    pub per_host_concurrency: usize,
    pub timeout: Duration,
}

/// Validate `config` and expand it into a [`ScanJob`].
pub fn plan(config: &ScanConfig) -> Result<ScanJob, ScanError> {
    if config.concurrency == 0 {
        return Err(ScanError::InvalidConfig("concurrency must be > 0".into()));
    }
    let specs = targets::parse_targets(&config.targets)?;
    let count: u64 = specs.iter().map(|s| s.host_count()).sum();
    if count > MAX_HOSTS {
        return Err(ScanError::InvalidConfig(format!(
            "{count} addresses requested; the limit is {MAX_HOSTS} (a /16)"
        )));
    }
    let mut seen = HashSet::new();
    let addrs: Vec<Ipv4Addr> = specs
        .iter()
        .flat_map(|s| s.expand())
        .filter(|a| seen.insert(*a))
        .collect();
    if addrs.is_empty() {
        return Err(ScanError::InvalidConfig(
            "targets expand to no addresses".into(),
        ));
    }

    let (ports, default_timeout) = match config.profile {
        ScanProfile::Quick => (QUICK_PORTS, 500),
        ScanProfile::Standard => (STANDARD_PORTS, 1000),
        ScanProfile::Deep => (DEEP_PORTS, 1500),
    };
    let timeout_ms = if config.timeout_ms == 0 {
        default_timeout
    } else {
        config.timeout_ms
    };
    Ok(ScanJob {
        addrs,
        ports,
        concurrency: (config.concurrency as usize).min(MAX_CONCURRENCY),
        per_host_concurrency: (config.per_host_concurrency as usize).max(1),
        timeout: Duration::from_millis(timeout_ms),
    })
}

/// Shared state of one running scan.
struct Ctx {
    observer: Arc<dyn ScanObserver>,
    started: Instant,
    total: u32,
    scanned: AtomicU32,
    discovered: AtomicU32,
    open_ports: AtomicU32,
    hosts: Mutex<HashMap<Ipv4Addr, Host>>,
    last_progress: Mutex<Instant>,
}

impl Ctx {
    fn progress(&self, phase: ScanPhase) -> Progress {
        Progress {
            total_hosts: self.total,
            scanned_hosts: self.scanned.load(Ordering::Relaxed),
            discovered_hosts: self.discovered.load(Ordering::Relaxed),
            open_ports_found: self.open_ports.load(Ordering::Relaxed),
            elapsed_ms: self.started.elapsed().as_millis() as u64,
            phase,
        }
    }

    /// Emit progress, throttled unless `force`.
    fn report(&self, phase: ScanPhase, force: bool) {
        {
            let mut last = self.last_progress.lock().unwrap_or_else(|e| e.into_inner());
            if !force && last.elapsed() < PROGRESS_EVERY {
                return;
            }
            *last = Instant::now();
        }
        self.observer.on_progress(self.progress(phase));
    }

    /// Record a new host and stream it.
    fn add_host(&self, host: Host) {
        self.discovered.fetch_add(1, Ordering::Relaxed);
        self.open_ports
            .fetch_add(host.open_ports.len() as u32, Ordering::Relaxed);
        self.observer.on_host(host.clone());
        let ip: Ipv4Addr = host.ip.parse().expect("engine-built IP");
        self.lock_hosts().insert(ip, host);
    }

    fn lock_hosts(&self) -> std::sync::MutexGuard<'_, HashMap<Ipv4Addr, Host>> {
        self.hosts.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Run `job`, streaming to `observer`, until done or `cancel` trips.
pub async fn run(job: ScanJob, observer: Arc<dyn ScanObserver>, cancel: CancellationToken) {
    let ctx = Arc::new(Ctx {
        observer,
        started: Instant::now(),
        total: job.addrs.len() as u32,
        scanned: AtomicU32::new(0),
        discovered: AtomicU32::new(0),
        open_ports: AtomicU32::new(0),
        hosts: Mutex::new(HashMap::new()),
        last_progress: Mutex::new(Instant::now()),
    });
    ctx.report(ScanPhase::Probing, true);

    let pinger = if icmp::ping_available() {
        match PingClient::new() {
            Ok(c) => Some(Arc::new(c)),
            Err(e) => {
                ctx.observer
                    .on_error(format!("ICMP unavailable ({e}); using TCP probes only"));
                None
            }
        }
    } else {
        None
    };

    let in_flight = Arc::new(Semaphore::new(job.concurrency));
    let sockets = Arc::new(Semaphore::new(socket_budget()));
    let mut tasks = JoinSet::new();

    for &ip in &job.addrs {
        let permit = tokio::select! {
            _ = cancel.cancelled() => return,
            p = Arc::clone(&in_flight).acquire_owned() => match p {
                Ok(p) => p,
                Err(_) => return,
            },
        };
        let ctx = Arc::clone(&ctx);
        let pinger = pinger.clone();
        let sockets = Arc::clone(&sockets);
        let cancel = cancel.clone();
        let probe = ProbeParams {
            ports: job.ports,
            timeout: job.timeout,
            per_host_concurrency: job.per_host_concurrency,
        };
        tasks.spawn(async move {
            let probed = cancel
                .run_until_cancelled(probe_host(ip, probe, pinger.as_deref(), sockets))
                .await;
            drop(permit);
            let Some(found) = probed else { return };
            if let Some(host) = found {
                ctx.add_host(host);
            }
            ctx.scanned.fetch_add(1, Ordering::Relaxed);
            ctx.report(ScanPhase::Probing, false);
        });
    }
    while tasks.join_next().await.is_some() {}
    if cancel.is_cancelled() {
        return;
    }

    ctx.report(ScanPhase::Resolving, true);
    attach_arp(&ctx, &job.addrs);
    if cancel.is_cancelled() {
        return;
    }

    let summary = summarize(&ctx);
    ctx.report(ScanPhase::Done, true);
    ctx.observer.on_finished(summary);
}

/// The per-host slice of a [`ScanJob`] (cheap to copy into each task).
#[derive(Clone, Copy)]
struct ProbeParams {
    ports: &'static [u16],
    timeout: Duration,
    per_host_concurrency: usize,
}

/// Probe one address; `Some(host)` if anything proved it up.
async fn probe_host(
    ip: Ipv4Addr,
    p: ProbeParams,
    pinger: Option<&PingClient>,
    sockets: Arc<Semaphore>,
) -> Option<Host> {
    let ping = async {
        match pinger {
            Some(c) => c.ping(ip, p.timeout).await.ok().flatten(),
            None => None,
        }
    };
    let tcp = tcp_probe::probe(ip, p.ports, p.timeout, p.per_host_concurrency, sockets);
    let (ping, tcp) = tokio::join!(ping, tcp);
    if ping.is_none() && !tcp.up {
        return None;
    }
    let now = now_ms();
    Some(Host {
        ip: ip.to_string(),
        mac: None,
        vendor: None,
        hostnames: Vec::new(),
        device_type: DeviceType::Unknown,
        open_ports: tcp
            .open
            .iter()
            .map(|&number| Port {
                number,
                transport: Transport::Tcp,
                state: PortState::Open,
                service: service_name(number).map(str::to_string),
                version: None,
            })
            .collect(),
        rtt_ms: ping.map(|p| p.rtt.as_secs_f64() * 1000.0),
        mdns_services: Vec::new(),
        ssdp_info: None,
        first_seen: now,
        last_seen: now,
    })
}

/// Merge the OS ARP cache into the results (see module docs, step 3).
fn attach_arp(ctx: &Ctx, targets: &[Ipv4Addr]) {
    let arp = match platform::current().read_arp_cache() {
        Ok(m) => m,
        Err(e) => {
            ctx.observer.on_error(format!("ARP cache unavailable: {e}"));
            return;
        }
    };
    let targets: HashSet<Ipv4Addr> = targets.iter().copied().collect();
    let now = now_ms();
    for (ip, mac) in arp {
        let Ok(addr) = ip.parse::<Ipv4Addr>() else {
            continue;
        };
        if !targets.contains(&addr) {
            continue;
        }
        let updated = {
            let mut hosts = ctx.lock_hosts();
            hosts.get_mut(&addr).map(|h| {
                h.mac = Some(mac.clone());
                h.clone()
            })
        };
        match updated {
            Some(h) => ctx.observer.on_host(h),
            None => ctx.add_host(Host {
                ip,
                mac: Some(mac),
                vendor: None,
                hostnames: Vec::new(),
                device_type: DeviceType::Unknown,
                open_ports: Vec::new(),
                rtt_ms: None,
                mdns_services: Vec::new(),
                ssdp_info: None,
                first_seen: now,
                last_seen: now,
            }),
        }
    }
}

fn summarize(ctx: &Ctx) -> Summary {
    let hosts = ctx.lock_hosts();
    let mut by_type: Vec<DeviceTypeCount> = Vec::new();
    for h in hosts.values() {
        match by_type.iter_mut().find(|c| c.device_type == h.device_type) {
            Some(c) => c.count += 1,
            None => by_type.push(DeviceTypeCount {
                device_type: h.device_type,
                count: 1,
            }),
        }
    }
    Summary {
        total_hosts: ctx.total,
        scanned_hosts: ctx.scanned.load(Ordering::Relaxed),
        discovered_hosts: hosts.len() as u32,
        total_open_ports: hosts.values().map(|h| h.open_ports.len() as u32).sum(),
        elapsed_ms: ctx.started.elapsed().as_millis() as u64,
        by_device_type: by_type,
    }
}

/// Sockets the scan may hold open at once: the soft fd limit minus headroom
/// for the host app, within sane bounds.
fn socket_budget() -> usize {
    let mut lim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    let soft = if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } == 0 {
        lim.rlim_cur
    } else {
        256
    };
    (soft.saturating_sub(64) as usize).clamp(16, 1024)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// IANA service name for the discovery ports.
fn service_name(port: u16) -> Option<&'static str> {
    Some(match port {
        21 => "ftp",
        22 => "ssh",
        23 => "telnet",
        25 => "smtp",
        53 => "domain",
        80 => "http",
        110 => "pop3",
        111 => "sunrpc",
        135 => "msrpc",
        139 => "netbios-ssn",
        143 => "imap",
        443 => "https",
        445 => "microsoft-ds",
        515 => "printer",
        548 => "afp",
        554 => "rtsp",
        631 => "ipp",
        993 => "imaps",
        1883 => "mqtt",
        2049 => "nfs",
        3000 => "http-alt",
        3306 => "mysql",
        3389 => "ms-wbt-server",
        5000 => "upnp",
        5001 => "commplex-link",
        5432 => "postgresql",
        5900 => "vnc",
        7000 => "airplay",
        8000 => "http-alt",
        8008 => "http-alt",
        8009 => "ajp13",
        8080 => "http-proxy",
        8443 => "https-alt",
        8888 => "http-alt",
        9000 => "cslistener",
        9100 => "jetdirect",
        32400 => "plex",
        49152 => "upnp",
        62078 => "iphone-sync",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(targets: &[&str]) -> ScanConfig {
        ScanConfig {
            targets: targets.iter().map(|s| s.to_string()).collect(),
            profile: ScanProfile::Quick,
            concurrency: 32,
            per_host_concurrency: 4,
            timeout_ms: 0,
        }
    }

    #[test]
    fn plan_expands_dedupes_and_defaults() {
        let job = plan(&config(&["10.0.0.0/30", "10.0.0.1, 10.0.0.9"])).unwrap();
        assert_eq!(
            job.addrs,
            vec![
                Ipv4Addr::new(10, 0, 0, 1),
                Ipv4Addr::new(10, 0, 0, 2),
                Ipv4Addr::new(10, 0, 0, 9)
            ]
        );
        assert_eq!(job.ports, QUICK_PORTS);
        assert_eq!(job.timeout, Duration::from_millis(500));
    }

    #[test]
    fn plan_rejects_bad_configs() {
        assert!(plan(&config(&["10.0.0.0/8"])).is_err());
        assert!(plan(&config(&[])).is_err());
        let mut c = config(&["10.0.0.1"]);
        c.concurrency = 0;
        assert!(plan(&c).is_err());
    }

    #[test]
    fn every_discovery_port_has_a_service_name() {
        for &p in DEEP_PORTS.iter().chain(STANDARD_PORTS).chain(QUICK_PORTS) {
            assert!(service_name(p).is_some(), "port {p}");
        }
    }
}
