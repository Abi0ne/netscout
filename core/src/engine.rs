//! The scan pipeline behind `Scanner::start_scan` / `Scanner::scan_host`.
//!
//! 1. **Plan** (synchronous, in the FFI call): parse and expand the targets,
//!    dedupe, apply profile defaults, reject oversized jobs.
//! 2. **Probe** (async, on the scanner's runtime): every host gets an ICMP
//!    echo and a TCP connect sweep over the profile's discovery ports, both
//!    concurrently. Hosts in flight are bounded by `concurrency`; open
//!    sockets by a budget derived from the process fd limit.
//! 3. **ARP** — after the sweep the OS ARP cache holds the neighbours our
//!    probes just resolved: their MACs (and the MACs' IEEE vendors) are
//!    attached to known hosts, and in-target addresses that answered ARP but
//!    nothing else are reported too (devices that drop ping and every probed
//!    port).
//! 4. **Names and second look** — mDNS, NetBIOS and reverse DNS for every
//!    discovered host, concurrently, bounded by a deadline (see `names`);
//!    alongside, the ports that stayed silent on hosts now known to be up are
//!    tried again with a longer deadline ([`recheck_timeout`]): slow embedded
//!    stacks and Wi-Fi clients in power save often miss the first, short
//!    window, and a longer one lets TCP retransmit the SYN. Only up hosts are
//!    retried, so the sweep keeps its pace. Hosts that get a name or a port
//!    are re-emitted.
//!    With the deep profile, names that cross subnets come next, from the
//!    ports now known (see `deep_names`): DNS asked directly, SMB, and TLS
//!    certificates.
//! 5. **Classify** — each host's device type from the evidence gathered
//!    (see `classify`); hosts whose type is now known are re-emitted.
//! 6. **Finish** — `on_finished` with the summary. A cancelled scan stops at
//!    the next await point and emits nothing further.

use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::classify;
use crate::deep_names;
use crate::error::ScanError;
use crate::icmp::{self, PingClient};
use crate::names;
use crate::oui;
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
/// Reverse-DNS lookups in flight at once, and the deadline for each name
/// source.
const NAME_CONCURRENCY: usize = 32;
const NAME_TIMEOUT: Duration = Duration::from_secs(2);

/// Deadline of the second look at silent ports: long enough for TCP to
/// retransmit the SYN once or twice (macOS: after ~1 s, then ~2 s).
fn recheck_timeout(timeout: Duration) -> Duration {
    (timeout * 2).clamp(Duration::from_secs(2), Duration::from_secs(3))
}

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
    /// Deep profile: also the names that cross subnets.
    pub deep: bool,
    /// Name servers to ask directly (deep profile), set by the scanner.
    pub name_servers: Vec<Ipv4Addr>,
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
        deep: config.profile == ScanProfile::Deep,
        name_servers: Vec::new(),
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
    /// Ports of up hosts that neither accepted nor refused in the sweep.
    silent: Mutex<HashMap<Ipv4Addr, Vec<u16>>>,
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
        silent: Mutex::new(HashMap::new()),
        last_progress: Mutex::new(Instant::now()),
    });
    ctx.report(ScanPhase::Probing, true);
    let gateways = local_gateways();

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
            if let Some((host, silent)) = found {
                if !silent.is_empty() {
                    lock(&ctx.silent).insert(ip, silent);
                }
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
    attach_arp(&ctx, &job.addrs, job.ports);
    if cancel.is_cancelled() {
        return;
    }
    let second_look = async {
        tokio::join!(
            resolve_names(&ctx),
            recheck_silent_ports(&ctx, recheck_timeout(job.timeout), Arc::clone(&sockets)),
        )
    };
    if cancel.run_until_cancelled(second_look).await.is_none() {
        return;
    }
    if job.deep {
        let names = deep_resolve_names(&ctx, &job.name_servers, Arc::clone(&sockets));
        if cancel.run_until_cancelled(names).await.is_none() {
            return;
        }
    }
    classify_hosts(&ctx, &gateways);

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

/// Probe one address; `Some((host, silent ports))` if anything proved it up.
async fn probe_host(
    ip: Ipv4Addr,
    p: ProbeParams,
    pinger: Option<&PingClient>,
    sockets: Arc<Semaphore>,
) -> Option<(Host, Vec<u16>)> {
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
    let host = Host {
        ip: ip.to_string(),
        mac: None,
        vendor: None,
        hostnames: Vec::new(),
        device_type: DeviceType::Unknown,
        open_ports: tcp.open.iter().map(|&n| open_port(n)).collect(),
        rtt_ms: ping.map(|p| p.rtt.as_secs_f64() * 1000.0),
        mdns_services: Vec::new(),
        ssdp_info: None,
        first_seen: now,
        last_seen: now,
    };
    Some((host, tcp.silent))
}

fn open_port(number: u16) -> Port {
    Port {
        number,
        transport: Transport::Tcp,
        state: PortState::Open,
        service: service_name(number).map(str::to_string),
        version: None,
    }
}

/// Second look at the ports that stayed silent on up hosts (see module docs,
/// step 4). Every host's ports go at once: the hosts are up and their ARP
/// entries warm, and the socket budget still caps the total.
async fn recheck_silent_ports(ctx: &Ctx, timeout: Duration, sockets: Arc<Semaphore>) {
    let pending: Vec<(Ipv4Addr, Vec<u16>)> = lock(&ctx.silent).drain().collect();
    let mut set = JoinSet::new();
    for (ip, ports) in pending {
        let sockets = Arc::clone(&sockets);
        set.spawn(async move {
            let found = tcp_probe::probe(ip, &ports, timeout, ports.len(), sockets).await;
            (ip, found.open)
        });
    }
    while let Some(joined) = set.join_next().await {
        let Ok((ip, open)) = joined else { continue };
        if open.is_empty() {
            continue;
        }
        let updated = ctx.lock_hosts().get_mut(&ip).map(|h| {
            let before = h.open_ports.len();
            for n in open {
                if !h.open_ports.iter().any(|p| p.number == n) {
                    h.open_ports.push(open_port(n));
                }
            }
            h.open_ports.sort_by_key(|p| p.number);
            (h.open_ports.len() - before, h.clone())
        });
        if let Some((added, h)) = updated {
            ctx.open_ports.fetch_add(added as u32, Ordering::Relaxed);
            ctx.observer.on_host(h);
        }
    }
    ctx.report(ScanPhase::Resolving, true);
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Merge the OS ARP cache into the results (see module docs, step 3). Hosts
/// known only from ARP answered no probe in time: all of `ports` get the
/// second look.
fn attach_arp(ctx: &Ctx, targets: &[Ipv4Addr], ports: &[u16]) {
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
                h.vendor = oui::vendor(&mac).map(str::to_string);
                h.mac = Some(mac.clone());
                h.clone()
            })
        };
        match updated {
            Some(h) => ctx.observer.on_host(h),
            None => {
                lock(&ctx.silent).insert(addr, ports.to_vec());
                ctx.add_host(Host {
                    ip,
                    vendor: oui::vendor(&mac).map(str::to_string),
                    mac: Some(mac),
                    hostnames: Vec::new(),
                    device_type: DeviceType::Unknown,
                    open_ports: Vec::new(),
                    rtt_ms: None,
                    mdns_services: Vec::new(),
                    ssdp_info: None,
                    first_seen: now,
                    last_seen: now,
                })
            }
        }
    }
}

/// Name every discovered host (see module docs, step 4): mDNS, NetBIOS and
/// reverse DNS run concurrently; each host keeps the distinct names found, in
/// that order of preference, and is re-emitted if it got any.
async fn resolve_names(ctx: &Ctx) {
    let ips: Vec<Ipv4Addr> = ctx.lock_hosts().keys().copied().collect();
    let (mdns, netbios, dns) = tokio::join!(
        names::mdns_names(&ips, NAME_TIMEOUT),
        names::netbios_names(&ips, NAME_TIMEOUT),
        reverse_dns(&ips),
    );
    for ip in ips {
        let mut found: Vec<String> = Vec::new();
        for name in [mdns.get(&ip), netbios.get(&ip), dns.get(&ip)]
            .into_iter()
            .flatten()
        {
            if !found.iter().any(|f| f.eq_ignore_ascii_case(name)) {
                found.push(name.clone());
            }
        }
        if found.is_empty() {
            continue;
        }
        let updated = ctx.lock_hosts().get_mut(&ip).map(|h| {
            h.hostnames = found;
            h.clone()
        });
        if let Some(h) = updated {
            ctx.observer.on_host(h);
        }
    }
    ctx.report(ScanPhase::Resolving, true);
}

/// The deep profile's names (see `deep_names`), added after the ones already
/// found; hosts that got a new name are re-emitted.
async fn deep_resolve_names(ctx: &Ctx, servers: &[Ipv4Addr], sockets: Arc<Semaphore>) {
    let targets: Vec<deep_names::Target> = ctx
        .lock_hosts()
        .iter()
        .map(|(&ip, h)| deep_names::Target {
            ip,
            open_ports: h.open_ports.iter().map(|p| p.number).collect(),
        })
        .collect();
    let found = deep_names::resolve(&targets, servers, sockets).await;
    for (ip, names) in found {
        let updated = ctx.lock_hosts().get_mut(&ip).and_then(|h| {
            let before = h.hostnames.len();
            for name in names {
                if !h.hostnames.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
                    h.hostnames.push(name);
                }
            }
            (h.hostnames.len() > before).then(|| h.clone())
        });
        if let Some(h) = updated {
            ctx.observer.on_host(h);
        }
    }
    ctx.report(ScanPhase::Resolving, true);
}

/// Reverse DNS for `ips` on the blocking pool, at most [`NAME_CONCURRENCY`]
/// lookups at once, each abandoned after [`NAME_TIMEOUT`].
/// Assign each host its device type; re-emit the hosts that changed.
fn classify_hosts(ctx: &Ctx, gateways: &[Ipv4Addr]) {
    let changed: Vec<Host> = ctx
        .lock_hosts()
        .values_mut()
        .filter_map(|h| {
            let t = classify::classify(h, gateways);
            (t != h.device_type).then(|| {
                h.device_type = t;
                h.clone()
            })
        })
        .collect();
    for h in changed {
        ctx.observer.on_host(h);
    }
}

/// Default routers of this machine's networks (empty where the platform
/// cannot enumerate them).
fn local_gateways() -> Vec<Ipv4Addr> {
    platform::current()
        .enumerate_networks()
        .unwrap_or_default()
        .iter()
        .filter_map(|n| n.gateway.as_deref()?.parse().ok())
        .collect()
}

async fn reverse_dns(ips: &[Ipv4Addr]) -> HashMap<Ipv4Addr, String> {
    let limit = Arc::new(Semaphore::new(NAME_CONCURRENCY));
    let mut lookups = JoinSet::new();
    for &ip in ips {
        let Ok(permit) = Arc::clone(&limit).acquire_owned().await else {
            break;
        };
        lookups.spawn(async move {
            // The permit lives as long as the blocking call, so a lookup the
            // resolver never answers still counts against the limit.
            let lookup = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                names::reverse_lookup(ip)
            });
            let name = tokio::time::timeout(NAME_TIMEOUT, lookup).await;
            (ip, name.ok().and_then(|r| r.ok()).flatten())
        });
    }
    let mut out = HashMap::new();
    while let Some(done) = lookups.join_next().await {
        if let Ok((ip, Some(name))) = done {
            out.insert(ip, name);
        }
    }
    out
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

/// Sockets the scan may hold open at once: the soft fd limit (raised first
/// when low) minus headroom
/// for the host app, within sane bounds.
fn socket_budget() -> usize {
    let mut lim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    let soft = if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } == 0 {
        // Apps launched from the Finder get a soft limit of 256: raise it
        // (up to the hard limit) so the budget is not the bottleneck.
        let want = WANT_FDS.min(lim.rlim_max);
        if lim.rlim_cur < want {
            let raised = libc::rlimit {
                rlim_cur: want,
                rlim_max: lim.rlim_max,
            };
            if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raised) } == 0 {
                lim.rlim_cur = want;
            }
        }
        lim.rlim_cur
    } else {
        256
    };
    (soft.saturating_sub(64) as usize).clamp(16, 1024)
}

/// Descriptor soft limit the engine asks for (enough for a 1024-socket budget).
const WANT_FDS: libc::rlim_t = 2048;

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
