import Foundation
import Network
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
    /// Devices known from a saved profile but not found by this scan, keyed
    /// by `offlineID`. They are shown greyed out and can be woken (WoL).
    private(set) var offlineHosts: [String: Host] = [:]
    private(set) var progress: NetScoutCore.Progress?
    private(set) var summary: Summary?
    private(set) var isScanning = false
    /// IPs with a deep single-host scan in flight.
    private(set) var deepScanning: Set<String> = []
    /// Target and depth of the last scan started (what a saved profile records).
    private(set) var scannedTarget = ""
    private(set) var scannedProfile: ScanProfile = .standard

    /// Saved profiles, newest first.
    private(set) var profiles: [ProfileSummary] = []
    /// The same profiles with their devices, by id (search, recognition, export).
    private(set) var savedProfiles: [String: SavedProfile] = [:]
    /// Notes edited but not saved yet: profile id → all its notes (by `deviceKey`).
    private(set) var noteDrafts: [String: [String: String]] = [:]
    /// The comparison on screen, if any.
    var comparison: ProfileComparison?

    /// The tab on screen and the profile selected in the profiles tab.
    var selectedTab: AppTab = .scan
    var selectedProfileID: String?
    /// Each tab's search text (one field in the window toolbar serves both).
    var scanSearch = ""
    var profileSearch = ""
    /// The scan tab's device card (inspector) is shown, and its width: the
    /// search field above it takes the same width.
    var showDeviceCard = true
    var deviceCardWidth: CGFloat = 0
    /// The sidebar's width, 0 while it is hidden.
    var sidebarWidth: CGFloat = 0
    /// The scan tab's selected device and type filter (kept while the
    /// profiles tab is on screen).
    var scanSelection: String?
    var typeFilter: DeviceType?

    /// The profile this scan was saved as, if it was.
    private(set) var savedScanProfileID: String?

    /// The saved profile this scan's network was recognized as, if any.
    private(set) var recognizedNetwork: NetworkMatch?
    /// The recognized network on offer in an alert.
    var networkSuggestion: NetworkMatch?
    /// Compare with this profile as soon as the scan finishes.
    private var pendingComparisonID: String?
    /// Identifiable devices seen at the last recognition attempt; the next
    /// one runs only when there are more.
    private var recognitionBasis = -1
    private var suggestedThisScan = false

    /// Host updates waiting for the next coalesced flush (see `enqueue`).
    fileprivate var pendingHosts: [String: Host] = [:]
    fileprivate var flushScheduled = false

    /// Set for a few seconds after the networks change under a running app.
    private(set) var networkNotice: String?

    private var scanner: Scanner?
    private var pathMonitor: NWPathMonitor?
    private var networkPoll: Timer?
    private var profileStore: ProfileStore?
    /// Bumped on every start/cancel so late events of an old scan are dropped.
    private var generation = 0

    init() {
        do {
            scanner = try newScanner()
            detectNetworks()
            watchNetworks()
            // `NetScout --scan` starts scanning right away (handy for testing).
            if CommandLine.arguments.contains("--scan") {
                startScan()
            }
        } catch {
            errorMessage = "Impossibile avviare il motore: \(error)"
        }
        openProfiles()
    }

    var sortedHosts: [Host] {
        hosts.values.sorted { ipValue($0.ip) < ipValue($1.ip) }
    }

    func detectNetworks() {
        guard let scanner else { return }
        do {
            networks = try scanner.detectNetworks()
            debugLog("networks: \(networks.map { "\($0.interface) \($0.cidr) gw \($0.gateway ?? "-")" })")
            if target.isEmpty, let net = Self.preferredNetwork(networks) {
                target = net.cidr
            }
        } catch {
            errorMessage = "Nessuna rete trovata: \(error)"
        }
    }

    /// The network to scan by default: the one with a gateway, else the first.
    private static func preferredNetwork(_ networks: [NetworkInfo]) -> NetworkInfo? {
        networks.first(where: { $0.gateway != nil }) ?? networks.first
    }

    /// Follow interface and address changes while the app runs: macOS
    /// reports path changes at once (re-read now and again shortly, as DHCP
    /// may still be assigning the address), and a light poll catches the
    /// changes it does not report, like a new lease on the same network.
    private func watchNetworks() {
        let monitor = NWPathMonitor()
        monitor.pathUpdateHandler = { [weak self] _ in
            DispatchQueue.main.async {
                MainActor.assumeIsolated { self?.refreshNetworks() }
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + 3) {
                MainActor.assumeIsolated { self?.refreshNetworks() }
            }
        }
        monitor.start(queue: .global(qos: .utility))
        pathMonitor = monitor
        networkPoll = Timer.scheduledTimer(withTimeInterval: 10, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.refreshNetworks() }
        }
    }

    /// Re-read the networks; when they changed, update the list and, if the
    /// target was a network that no longer exists (or none), move it to the
    /// current one. A target the user typed is left alone.
    func refreshNetworks() {
        guard let scanner else { return }
        let current = (try? scanner.detectNetworks()) ?? []
        guard current != networks else { return }
        let old = networks
        networks = current
        debugLog("networks changed: \(current.map { "\($0.interface) \($0.cidr)" })")

        let targetWasDetected = old.contains { $0.cidr == target }
        let targetGone = !current.contains { $0.cidr == target }
        if target.isEmpty || (targetWasDetected && targetGone),
           let net = Self.preferredNetwork(current) {
            target = net.cidr
        }
        let summary = current.isEmpty
            ? "nessuna rete attiva"
            : current.map { "\($0.interface) \($0.ipv4)" }.joined(separator: ", ")
        networkNotice = "Rete cambiata: \(summary)"
        DispatchQueue.main.asyncAfter(deadline: .now() + 10) { [weak self] in
            MainActor.assumeIsolated {
                if self?.networkNotice?.hasSuffix(summary) == true { self?.networkNotice = nil }
            }
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
        offlineHosts = [:]
        progress = nil
        summary = nil
        errorMessage = nil
        comparison = nil
        recognizedNetwork = nil
        savedScanProfileID = nil
        networkSuggestion = nil
        pendingComparisonID = nil
        recognitionBasis = -1
        suggestedThisScan = false
        scannedTarget = trimmed
        scannedProfile = profile
        let observer = ObserverBridge { [weak self] event in
            self?.apply(event, generation: gen)
        }
        scanner.setNameServers(servers: NameServers.configured)
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
            case .host(let host):
                self.hosts[host.ip] = host
                self.dropOffline(matching: [host])
            case .finished: self.deepScanning.remove(ip)
            case .error(let message): self.errorMessage = message
            case .progress: break
            }
        }
        scanner.setNameServers(servers: NameServers.configured)
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
            if let id = pendingComparisonID {
                pendingComparisonID = nil
                compareScan(withProfile: id)
            }
        case .error(let message):
            errorMessage = message
        }
    }
}

// MARK: - Saved profiles

/// A finished scan compared with a saved profile.
struct ProfileComparison: Identifiable {
    let profile: ProfileSummary
    let diff: ScanDiff
    /// The current scan probed a different target or used a different depth,
    /// so some differences may come from that rather than from the network.
    let targetDiffers: Bool
    let depthDiffers: Bool
    var id: String { profile.id }
}

extension ScanModel {
    /// A finished scan with results can be saved or compared.
    var hasFinishedScan: Bool {
        summary != nil && !isScanning && !hosts.isEmpty
    }

    /// Suggested name for a new profile.
    var suggestedProfileName: String {
        "\(scannedTarget) · \(Date().formatted(date: .abbreviated, time: .shortened))"
    }

    private func openProfiles() {
        do {
            let dir = try FileManager.default
                .url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
                .appending(path: "NetScout/Profiles", directoryHint: .isDirectory)
            profileStore = try openProfileStore(dir: dir.path(percentEncoded: false))
            refreshProfiles()
        } catch {
            errorMessage = "Impossibile aprire i profili salvati: \(error)"
        }
    }

    func refreshProfiles() {
        guard let profileStore else { return }
        do {
            profiles = try profileStore.list()
            savedProfiles = Dictionary(
                try profileStore.loadAll().map { ($0.id, $0) },
                uniquingKeysWith: { first, _ in first }
            )
        } catch {
            errorMessage = "Impossibile leggere i profili: \(error)"
        }
        noteDrafts = noteDrafts.filter { savedProfiles[$0.key] != nil }
    }

    /// Save the finished scan as a new profile; returns it on success.
    @discardableResult
    func saveScan(as name: String) -> ProfileSummary? {
        guard let profileStore, hasFinishedScan else { return nil }
        do {
            let saved = try profileStore.save(
                name: name, target: scannedTarget, scanProfile: scannedProfile,
                hosts: sortedHosts, offlineHosts: sortedOfflineHosts
            )
            // The notes shown with the scan go along with it.
            let notes = scanNotes
            if !notes.isEmpty { try profileStore.setNotes(id: saved.id, notes: notes) }
            savedScanProfileID = saved.id
            refreshProfiles()
            return saved
        } catch {
            errorMessage = "Salvataggio non riuscito: \(error)"
            return nil
        }
    }

    func loadProfile(id: String) -> SavedProfile? {
        do {
            return try profileStore?.load(id: id)
        } catch {
            errorMessage = "Impossibile aprire il profilo: \(error)"
            return nil
        }
    }

    func renameProfile(id: String, to name: String) {
        do {
            try profileStore?.rename(id: id, name: name)
        } catch {
            errorMessage = "Impossibile rinominare il profilo: \(error)"
        }
        refreshProfiles()
    }

    func deleteProfile(id: String) {
        do {
            try profileStore?.delete(id: id)
        } catch {
            errorMessage = "Impossibile eliminare il profilo: \(error)"
        }
        if comparison?.profile.id == id { comparison = nil }
        if selectedProfileID == id { selectedProfileID = nil }
        noteDrafts[id] = nil
        refreshProfiles()
    }

    /// Compare the finished scan with profile `id` and show the result.
    func compareScan(withProfile id: String) {
        guard hasFinishedScan, let saved = loadProfile(id: id),
              let summary = profiles.first(where: { $0.id == id }) else { return }
        comparison = ProfileComparison(
            profile: summary,
            diff: diffHosts(baseline: saved.hosts, baselineOffline: saved.offlineHosts, current: sortedHosts),
            targetDiffers: saved.target != scannedTarget,
            depthDiffers: saved.scanProfile != scannedProfile
        )
    }
}

// MARK: - Notes on devices

extension ScanModel {
    /// The note on screen for `host` in profile `id`: the draft, else the saved one.
    func note(profile id: String, host: Host) -> String {
        notes(profile: id)[deviceKey(host: host)] ?? ""
    }

    /// Every note of profile `id` as on screen.
    func notes(profile id: String) -> [String: String] {
        noteDrafts[id] ?? savedProfiles[id]?.notes ?? [:]
    }

    func setNote(_ text: String, profile id: String, host: Host) {
        var notes = notes(profile: id)
        notes[deviceKey(host: host)] = text
        let saved = savedProfiles[id]?.notes ?? [:]
        noteDrafts[id] = Self.clean(notes) == Self.clean(saved) ? nil : notes
    }

    func hasUnsavedNotes(profile id: String) -> Bool { noteDrafts[id] != nil }

    /// Names of the profiles with notes not saved yet.
    var unsavedProfileNames: [String] {
        profiles.filter { noteDrafts[$0.id] != nil }.map(\.name)
    }

    @discardableResult
    func saveNotes(profile id: String) -> Bool {
        guard let profileStore, let draft = noteDrafts[id] else { return true }
        do {
            try profileStore.setNotes(id: id, notes: draft)
            noteDrafts[id] = nil
            refreshProfiles()
            return true
        } catch {
            errorMessage = "Salvataggio delle note non riuscito: \(error)"
            return false
        }
    }

    func discardNotes(profile id: String) {
        noteDrafts[id] = nil
    }

    /// Save every draft; false if one could not be saved (it is kept).
    func saveAllNotes() -> Bool {
        noteDrafts.keys.sorted().reduce(true) { ok, id in saveNotes(profile: id) && ok }
    }

    func discardAllNotes() {
        noteDrafts = [:]
    }

    /// Profile `id` with the notes on screen (for search and export).
    func profileWithDrafts(_ id: String) -> SavedProfile? {
        guard var profile = savedProfiles[id] else { return nil }
        profile.notes = Self.clean(notes(profile: id))
        return profile
    }

    /// The profile whose notes the scan shows and edits: the one it was
    /// saved as, else the one its network was recognized as.
    var scanNotesProfileID: String? {
        [savedScanProfileID, recognizedNetwork?.profileId]
            .compactMap { $0 }
            .first { savedProfiles[$0] != nil }
    }

    /// The notes on screen for the scan's devices.
    private var scanNotes: [String: String] {
        guard let id = scanNotesProfileID else { return [:] }
        let keys = Set((Array(hosts.values) + Array(offlineHosts.values)).map { deviceKey(host: $0) })
        return Self.clean(notes(profile: id)).filter { keys.contains($0.key) }
    }

    private static func clean(_ notes: [String: String]) -> [String: String] {
        notes.mapValues { $0.trimmingCharacters(in: .whitespacesAndNewlines) }.filter { !$0.value.isEmpty }
    }
}

// MARK: - Recognizing the network

/// The window's tabs.
enum AppTab: Hashable {
    case scan, profiles
}

extension ScanModel {
    /// Look for this scan's network among the saved profiles, each time
    /// devices with a stable identifier turn up, and offer the match once.
    fileprivate func recognizeNetwork() {
        guard !savedProfiles.isEmpty else { return }
        let basis = hosts.values.filter { $0.mac != nil || $0.ssdpInfo != nil }.count
        guard basis > recognitionBasis else { return }
        recognitionBasis = basis
        let gateway = networks.compactMap(\.gateway).first { hosts[$0] != nil }
        let match = NetScoutCore.recognizeNetwork(
            current: Array(hosts.values), gatewayIp: gateway, profiles: Array(savedProfiles.values)
        )
        recognizedNetwork = match
        if let match, !suggestedThisScan, comparison == nil {
            suggestedThisScan = true
            networkSuggestion = match
            debugLog("network recognized: \(match)")
        }
    }

    /// Compare with the recognized profile now, or when the scan finishes.
    func compareWhenFinished(profile id: String) {
        if isScanning {
            pendingComparisonID = id
        } else {
            compareScan(withProfile: id)
        }
    }

    var comparisonPending: Bool { pendingComparisonID != nil }

    func showProfile(id: String) {
        selectedProfileID = id
        selectedTab = .profiles
    }
}

// MARK: - CSV export

extension ScanModel {
    /// The profiles `ids` as CSV, with the notes on screen.
    func csv(profiles ids: [String]) -> String {
        profilesCsv(profiles: ids.compactMap(profileWithDrafts))
    }
}

// MARK: - Devices that are off

extension ScanModel {
    /// Stable id of a device that is off (it has no live IP of its own).
    nonisolated static func offlineID(_ host: Host) -> String {
        "off-" + (host.mac?.lowercased() ?? host.ip)
    }

    var sortedOfflineHosts: [Host] {
        offlineHosts.values.sorted { ipValue($0.ip) < ipValue($1.ip) }
    }

    /// Add devices that are off to this scan (skipping any found up).
    func addOffline(_ devices: [Host]) {
        let upMACs = Set(hosts.values.compactMap { $0.mac?.lowercased() })
        for device in devices {
            if let mac = device.mac?.lowercased() {
                if upMACs.contains(mac) { continue }
            } else if hosts[device.ip] != nil {
                continue
            }
            offlineHosts[Self.offlineID(device)] = device
        }
    }

    func forgetOffline(id: String) {
        offlineHosts[id] = nil
    }

    /// A device that turns up is no longer off.
    fileprivate func dropOffline(matching found: [Host]) {
        guard !offlineHosts.isEmpty else { return }
        for host in found {
            if let mac = host.mac?.lowercased() {
                offlineHosts["off-" + mac] = nil
            }
        }
    }

    /// Replace profile `id` with this scan: the devices up and those off.
    @discardableResult
    func updateProfile(id: String) -> Bool {
        guard let profileStore, hasFinishedScan else { return false }
        do {
            try profileStore.update(
                id: id, target: scannedTarget, scanProfile: scannedProfile,
                hosts: sortedHosts, offlineHosts: sortedOfflineHosts
            )
            refreshProfiles()
            return true
        } catch {
            errorMessage = "Aggiornamento del profilo non riuscito: \(error)"
            return false
        }
    }

    /// Send a Wake-on-LAN packet to `host`; returns an error message on failure.
    func wake(_ host: Host) -> String? {
        guard let scanner, let mac = host.mac else { return "MAC sconosciuto: impossibile inviare Wake-on-LAN." }
        do {
            try scanner.wakeOnLan(mac: mac, ip: host.ip)
            return nil
        } catch {
            return "Wake-on-LAN non inviato: \(error)"
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
        dropOffline(matching: Array(pendingHosts.values))
        pendingHosts = [:]
        recognizeNetwork()
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
