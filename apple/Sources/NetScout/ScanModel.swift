import Foundation
import NetScoutCore
import Observation

/// App state: the engine handle, the scan configuration and the results.
///
/// All mutation happens on the main actor. Engine callbacks arrive on Rust
/// worker threads; `ObserverBridge` hops them onto the main queue in order.
@MainActor
@Observable
final class ScanModel {
    var networks: [NetworkInfo] = []
    /// What to scan: a CIDR, an IP, a range ("192.168.1.10-50"), or a list.
    var target = ""
    var profile: ScanProfile = .standard
    var errorMessage: String?

    private(set) var hosts: [String: Host] = [:]
    private(set) var progress: NetScoutCore.Progress?
    private(set) var summary: Summary?
    private(set) var isScanning = false
    /// IPs with a deep single-host scan in flight.
    private(set) var deepScanning: Set<String> = []

    /// Host updates waiting for the next coalesced flush (see `enqueue`).
    fileprivate var pendingHosts: [String: Host] = [:]
    fileprivate var flushScheduled = false

    private var scanner: Scanner?
    /// Bumped on every start/cancel so late events of an old scan are dropped.
    private var generation = 0

    init() {
        do {
            scanner = try newScanner()
            detectNetworks()
            // `NetScout --scan` starts scanning right away (handy for testing).
            if CommandLine.arguments.contains("--scan") {
                startScan()
            }
        } catch {
            errorMessage = "Impossibile avviare il motore: \(error)"
        }
    }

    var sortedHosts: [Host] {
        hosts.values.sorted { ipValue($0.ip) < ipValue($1.ip) }
    }

    func detectNetworks() {
        guard let scanner else { return }
        do {
            networks = try scanner.detectNetworks()
            debugLog("networks: \(networks.map { "\($0.interface) \($0.cidr) gw \($0.gateway ?? "-")" })")
            if target.isEmpty,
               let net = networks.first(where: { $0.gateway != nil }) ?? networks.first {
                target = net.cidr
            }
        } catch {
            errorMessage = "Nessuna rete trovata: \(error)"
        }
    }

    func startScan() {
        guard let scanner, !isScanning else { return }
        let trimmed = target.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else {
            errorMessage = "Indica cosa scansionare (es. 192.168.1.0/24)."
            return
        }
        generation += 1
        let gen = generation
        pendingHosts = [:]
        hosts = [:]
        progress = nil
        summary = nil
        errorMessage = nil
        let observer = ObserverBridge { [weak self] event in
            self?.apply(event, generation: gen)
        }
        do {
            try scanner.startScan(
                config: ScanConfig(
                    targets: [trimmed],
                    profile: profile,
                    concurrency: 64,
                    perHostConcurrency: 8,
                    timeoutMs: 0
                ),
                observer: observer
            )
            isScanning = true
            debugLog("scan started: \(trimmed) \(profile)")
        } catch {
            debugLog("scan failed to start: \(error)")
            errorMessage = "\(error)"
        }
    }

    func cancel() {
        try? scanner?.cancel()
        generation += 1
        pendingHosts = [:]
        isScanning = false
        deepScanning = []
    }

    /// Re-scan one host with the deep profile; its row is updated in place.
    func deepScan(ip: String) {
        guard let scanner, !deepScanning.contains(ip) else { return }
        deepScanning.insert(ip)
        let gen = generation
        let observer = ObserverBridge { [weak self] event in
            guard let self, gen == self.generation else { return }
            switch event {
            case .host(let host): self.hosts[host.ip] = host
            case .finished: self.deepScanning.remove(ip)
            case .error(let message): self.errorMessage = message
            case .progress: break
            }
        }
        do {
            try scanner.scanHost(ip: ip, observer: observer)
        } catch {
            deepScanning.remove(ip)
            errorMessage = "\(error)"
        }
    }

    private func apply(_ event: ScanEvent, generation gen: Int) {
        debugLog("event: \(event)")
        guard gen == generation else { return }
        switch event {
        case .host(let host):
            enqueue(host)
        case .progress(let p):
            progress = p
        case .finished(let s):
            flushHosts()
            summary = s
            isScanning = false
        case .error(let message):
            errorMessage = message
        }
    }
}

extension ScanModel {
    /// Hosts arrive in bursts (one event per discovery, MAC, name, type…).
    /// Applying each one separately makes the table reload hundreds of times
    /// and trips AppKit's reentrancy checks, so updates are coalesced and
    /// applied at most every 150 ms.
    fileprivate func enqueue(_ host: Host) {
        pendingHosts[host.ip] = host
        guard !flushScheduled else { return }
        flushScheduled = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) { [weak self] in
            MainActor.assumeIsolated { self?.flushHosts() }
        }
    }

    fileprivate func flushHosts() {
        flushScheduled = false
        guard !pendingHosts.isEmpty else { return }
        hosts.merge(pendingHosts) { _, new in new }
        pendingHosts = [:]
    }
}

/// One engine callback, as a value.
enum ScanEvent {
    case host(Host)
    case progress(NetScoutCore.Progress)
    case finished(Summary)
    case error(String)
}

/// The engine's `ScanObserver`, forwarding every callback to the main queue.
/// `DispatchQueue.main` is FIFO, so events keep the order the engine sent.
final class ObserverBridge: ScanObserver, @unchecked Sendable {
    private let deliver: @MainActor (ScanEvent) -> Void

    init(_ deliver: @escaping @MainActor (ScanEvent) -> Void) {
        self.deliver = deliver
    }

    private func send(_ event: ScanEvent) {
        DispatchQueue.main.async { [deliver] in
            MainActor.assumeIsolated { deliver(event) }
        }
    }

    func onHost(host: Host) { send(.host(host)) }
    func onProgress(progress: NetScoutCore.Progress) { send(.progress(progress)) }
    func onFinished(summary: Summary) { send(.finished(summary)) }
    func onError(message: String) { send(.error(message)) }
}

/// Diagnostics on stderr, enabled by `NETSCOUT_DEBUG=1`.
func debugLog(_ message: @autoclosure () -> String) {
    guard ProcessInfo.processInfo.environment["NETSCOUT_DEBUG"] != nil else { return }
    FileHandle.standardError.write(Data("[netscout] \(message())\n".utf8))
}

/// Numeric value of a dotted IPv4 address, for sorting.
func ipValue(_ ip: String) -> UInt32 {
    ip.split(separator: ".").reduce(0) { ($0 << 8) | (UInt32($1) ?? 0) }
}
