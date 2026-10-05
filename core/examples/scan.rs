//! Command-line driver for the engine, through the same public API the apps use.
//!
//! ```text
//! cargo run --release --example scan                    # first detected LAN
//! cargo run --release --example scan -- 192.168.1.0/24  # explicit targets
//! cargo run --release --example scan -- --deep          # profile: --quick | --standard | --deep
//! ```

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::sync::{mpsc, Arc, Mutex};

use netscout_core::{new_scanner, Host, Progress, ScanConfig, ScanObserver, ScanProfile, Summary};

#[derive(Default)]
struct Shared {
    hosts: Mutex<BTreeMap<Ipv4Addr, Host>>,
}

struct Printer {
    shared: Arc<Shared>,
    done: Mutex<mpsc::Sender<Summary>>,
}

impl ScanObserver for Printer {
    fn on_host(&self, host: Host) {
        if let Ok(ip) = host.ip.parse() {
            self.shared.hosts.lock().unwrap().insert(ip, host);
        }
    }
    fn on_progress(&self, p: Progress) {
        eprint!(
            "\r{:?}: {}/{} scanned, {} found, {} open ports, {:.1}s   ",
            p.phase,
            p.scanned_hosts,
            p.total_hosts,
            p.discovered_hosts,
            p.open_ports_found,
            p.elapsed_ms as f64 / 1000.0
        );
    }
    fn on_finished(&self, summary: Summary) {
        let _ = self.done.lock().unwrap().send(summary);
    }
    fn on_error(&self, message: String) {
        eprintln!("\nwarning: {message}");
    }
}

fn main() {
    let mut profile = ScanProfile::Standard;
    let mut targets = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--quick" => profile = ScanProfile::Quick,
            "--standard" => profile = ScanProfile::Standard,
            "--deep" => profile = ScanProfile::Deep,
            _ => targets.push(arg),
        }
    }

    let scanner = new_scanner().expect("scanner");
    if targets.is_empty() {
        let nets = scanner.clone().detect_networks().expect("detect networks");
        for n in &nets {
            eprintln!(
                "{:<8} {:<16} {:<18} gw {}",
                n.interface,
                n.ipv4,
                n.cidr,
                n.gateway.as_deref().unwrap_or("-")
            );
        }
        // Prefer a network with a gateway (a real LAN), first one wins.
        let net = nets
            .iter()
            .find(|n| n.gateway.is_some())
            .or(nets.first())
            .expect("no network to scan");
        eprintln!("scanning {} on {}", net.cidr, net.interface);
        targets.push(net.cidr.clone());
    }

    let shared = Arc::new(Shared::default());
    let (tx, rx) = mpsc::channel();
    scanner
        .clone()
        .start_scan(
            ScanConfig {
                targets,
                profile,
                concurrency: 64,
                per_host_concurrency: 8,
                timeout_ms: 0,
            },
            Box::new(Printer {
                shared: Arc::clone(&shared),
                done: Mutex::new(tx),
            }),
        )
        .expect("start scan");
    let summary = rx.recv().expect("scan finished");
    eprintln!();

    println!(
        "{:<16} {:<18} {:<24} {:<28} {:>8}  OPEN PORTS",
        "IP", "MAC", "VENDOR", "NAME", "RTT"
    );
    for host in shared.hosts.lock().unwrap().values() {
        let ports = host
            .open_ports
            .iter()
            .map(|p| match &p.service {
                Some(s) => format!("{}/{s}", p.number),
                None => p.number.to_string(),
            })
            .collect::<Vec<_>>()
            .join(" ");
        println!(
            "{:<16} {:<18} {:<24} {:<28} {:>8}  {}",
            host.ip,
            host.mac.as_deref().unwrap_or("-"),
            clip(host.vendor.as_deref(), 24),
            clip(host.hostnames.first().map(String::as_str), 28),
            host.rtt_ms
                .map(|r| format!("{r:.1}ms"))
                .unwrap_or_else(|| "-".into()),
            ports
        );
    }
    println!(
        "\n{} hosts up of {} scanned, {} open ports, {:.1}s",
        summary.discovered_hosts,
        summary.scanned_hosts,
        summary.total_open_ports,
        summary.elapsed_ms as f64 / 1000.0
    );
}

/// `-` for a missing value; long values cut to `width` with an ellipsis.
fn clip(s: Option<&str>, width: usize) -> String {
    match s {
        None => "-".into(),
        Some(s) if s.chars().count() <= width => s.into(),
        Some(s) => s.chars().take(width - 1).chain(['…']).collect(),
    }
}
