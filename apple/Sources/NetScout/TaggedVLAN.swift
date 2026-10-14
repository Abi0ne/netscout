import Foundation
import NetScoutCore
import Observation
import SystemConfiguration

/// A tagged VLAN that reaches the Mac on the same cable as the network in
/// access, scanned along with the scan's target.
///
/// It is offered when the interface chosen for the scan is an Ethernet port
/// that can carry VLANs, and rides on that port.
///
/// Turning the option on listens to the port for a while: the tags of the
/// frames other devices send there fill a menu, with the network and the
/// devices seen in each VLAN (any tag can still be written by hand).
///
/// The Mac only sees a tagged VLAN through a VLAN interface of its own:
/// NetScout creates `vlanN` on the port, gives it an address (from the
/// VLAN's DHCP, or the one written here) and from then on scans the VLAN's
/// network like any other. The interface stays up after the scan, so a deep
/// scan or a terminal session to a device of the VLAN still works, until the
/// option is turned off, its settings change or the app quits.
///
/// Listening and creating interfaces both need an administrator: they go
/// through `VLANAgent`, started with the password the first time either is
/// needed and kept until NetScout quits, so the password is asked once per
/// session and nothing is left behind (even after a crash). An interface
/// made this way is not a network service: the system settings are never
/// touched, and a restart clears it.
@MainActor
@Observable
final class TaggedVLAN {
    enum Addressing: String, CaseIterable {
        case dhcp, manual
    }

    /// What the VLAN interface is made from.
    struct Settings: Equatable {
        var tag: Int
        var parent: String
        var addressing: Addressing
        /// Address and prefix, for manual addressing.
        var address: IPv4Interface?
    }

    /// The VLAN interface up and its network.
    struct Active: Equatable {
        let settings: Settings
        let interface: String
        let ipv4: String
        let cidr: String
    }

    /// Not remembered between launches: every session starts with the
    /// option off and empty.
    var enabled = false {
        didSet {
            guard enabled != oldValue else { return }
            settingsChanged()
            if enabled { discover() } else { stopDiscovery() }
        }
    }
    /// The VLAN ID as written (1–4094).
    var tag = "" {
        didSet { settingsChanged() }
    }
    /// The tag is written by hand ("Altro…" in the menu) rather than picked
    /// among the VLANs heard on the cable.
    var typingTag = false
    /// The port the VLAN rides on: the scan's interface, when it can carry
    /// VLANs; `nil` hides the option.
    private(set) var parent: String? {
        didSet {
            guard parent != oldValue else { return }
            settingsChanged()
            discovered = []
            discoveryProblem = nil
            if enabled, parent != nil { discover() } else { stopDiscovery() }
        }
    }
    var addressing = Addressing.dhcp {
        didSet { settingsChanged() }
    }
    /// The manual address with its prefix, e.g. "172.20.20.250/24".
    var address = "" {
        didSet { settingsChanged() }
    }

    private(set) var active: Active?
    private(set) var isPreparing = false
    /// The ports of the Mac that can carry VLANs.
    private(set) var capable: Set<String> = []

    /// The VLANs heard on the port, by tag.
    private(set) var discovered: [DiscoveredVLAN] = []
    private(set) var isDiscovering = false
    /// Why the listening found nothing, if it did.
    private(set) var discoveryProblem: String?
    private var discovery: Task<Void, Never>?
    /// How long a listening lasts.
    static let listenSeconds = 30

    private var agent: VLANAgent?
    private var agentStarting: Task<VLANAgent, Error>?

    init() {
        // Settings remembered by earlier builds.
        for key in ["vlan.enabled", "vlan.tag", "vlan.addressing", "vlan.address", "vlan.parent"] {
            UserDefaults.standard.removeObject(forKey: key)
        }
        // What earlier sessions left in the temporary folder: the commands
        // directory of an agent long gone, markers of earlier builds.
        let files = FileManager.default
        let temporary = files.temporaryDirectory
        for name in (try? files.contentsOfDirectory(atPath: temporary.path(percentEncoded: false))) ?? []
        where name.hasPrefix("netscout-agent-") || name.hasPrefix("netscout-vlan-") {
            try? files.removeItem(at: temporary.appending(path: name))
        }
    }

    /// The settings as written, or what is wrong with them.
    var settings: Result<Settings, VLANError> {
        let written = tag.trimmingCharacters(in: .whitespaces)
        guard !written.isEmpty || typingTag else {
            return .failure(.invalid("Scegli la VLAN dal menu."))
        }
        guard let id = Int(written), (1...4094).contains(id) else {
            return .failure(.invalid("Il tag VLAN va da 1 a 4094."))
        }
        guard let parent else {
            return .failure(.invalid("L'interfaccia scelta non può ricevere VLAN taggate."))
        }
        var settings = Settings(tag: id, parent: parent, addressing: addressing, address: nil)
        if addressing == .manual {
            guard let iface = IPv4Interface(address), iface.prefix <= 30 else {
                return .failure(.invalid("Scrivi l'indirizzo con il prefisso, per esempio \(suggestedAddress ?? "172.20.20.250/24")."))
            }
            settings.address = iface
        }
        return .success(settings)
    }

    /// The option is offered (the scan's interface can carry VLANs).
    var isAvailable: Bool { parent != nil }
    /// A VLAN is scanned along with the target.
    var isOn: Bool { enabled && isAvailable }

    var problem: String? {
        guard isOn, case .failure(let error) = settings else { return nil }
        return error.localizedDescription
    }

    /// The chosen VLAN, as heard on the cable.
    var chosen: DiscoveredVLAN? { discovered.first { String($0.tag) == tag } }

    /// A fixed address to suggest in the chosen VLAN: .250 in the network
    /// heard most.
    var suggestedAddress: String? { chosen?.mainNetwork.map { "\($0).250/24" } }

    /// Re-read which ports can carry VLANs; a port that no longer can (the
    /// cable or the dock was unplugged) stops being the parent.
    func refreshCapable() {
        let all = SCVLANInterfaceCopyAvailablePhysicalInterfaces() as NSArray as? [SCNetworkInterface] ?? []
        capable = Set(all.compactMap { SCNetworkInterfaceGetBSDName($0).map { $0 as String } })
        if let parent, !capable.contains(parent), active?.settings.parent != parent {
            self.parent = nil
        }
    }

    /// The scan's interface changed to `interface`: the VLAN rides on it if
    /// it can. A target that is no interface's network (`nil`) and the VLAN's
    /// own interface leave the choice as it is.
    func follow(interface: String?) {
        guard let interface, interface != active?.interface else { return }
        parent = capable.contains(interface) || active?.settings.parent == interface ? interface : nil
    }

    // MARK: Listening for VLANs

    /// Listen to the port for `listenSeconds`, filling `discovered` as the
    /// tags come (asking for the password if the agent is not running).
    func discover() {
        guard let parent else { return }
        discovery?.cancel()
        discovered = []
        discoveryProblem = nil
        isDiscovering = true
        discovery = Task { [weak self] in
            await self?.listen(on: parent)
        }
    }

    private func listen(on parent: String) async {
        do {
            let agent = try await agentReady()
            let path = try await agent.send(["listen", parent, String(Self.listenSeconds)])
            debugLog("vlan listen on \(parent): \(path)")
            let file = URL(filePath: path)
            while !Task.isCancelled {
                let text = (try? String(contentsOf: file, encoding: .utf8)) ?? ""
                discovered = DiscoveredVLAN.parse(text)
                if text.hasSuffix("end\n") || !agent.isAlive { break }
                try await Task.sleep(for: .seconds(1))
            }
            guard !Task.isCancelled else { return }
            if discovered.isEmpty {
                discoveryProblem = "Nessuna VLAN taggata sentita su \(parent) in \(Self.listenSeconds) secondi: scegli Altro… per scrivere il tag."
            }
        } catch {
            guard !Task.isCancelled else { return }
            discoveryProblem = (error as? VLANError) == .cancelled
                ? "Ricerca delle VLAN annullata: password non inserita. Puoi scrivere il tag con Altro…"
                : "Ricerca delle VLAN non riuscita: \((error as? VLANError)?.detail ?? error.localizedDescription)"
        }
        isDiscovering = false
    }

    func stopDiscovery() {
        guard isDiscovering else { return }
        discovery?.cancel()
        discovery = nil
        isDiscovering = false
        if let agent { Task { _ = try? await agent.send(["quiet"]) } }
    }

    // MARK: The VLAN interface

    /// The VLAN interface up with the current settings: the one already
    /// made, else a new one (asking for the administrator password if the
    /// agent is not running). `networks` are the other networks of the
    /// Mac: a VLAN network that overlaps one of them is refused.
    func prepare(networks: [NetworkInfo]) async throws -> Active {
        let settings = try settings.get()
        if let active, active.settings == settings { return active }
        deactivate()

        if let iface = settings.address, let clash = Self.overlap(iface.network, networks) {
            throw VLANError.overlap(iface.network.description, clash)
        }

        isPreparing = true
        defer { isPreparing = false }
        let agent = try await agentReady()
        // DHCP: up to 30 s for the lease, 10 s for the address to settle.
        let output = try await agent.send([
            "up", String(settings.tag), settings.parent, settings.addressing.rawValue,
            settings.address?.ip ?? "-", settings.address?.netmask ?? "-",
        ], timeout: .seconds(60))

        // "vlan0 172.20.20.250 0xffffff00".
        let fields = output.split(separator: " ").map(String.init)
        guard fields.count == 3, let mask = UInt32(fields[2].dropFirst(2), radix: 16),
              let iface = IPv4Interface(fields[1] + "/\(mask.nonzeroBitCount)") else {
            if let name = fields.first { _ = try? await agent.send(["down", name]) }
            throw VLANError.failed("risposta inattesa: \(output)")
        }
        let others = networks.filter { $0.interface != fields[0] }
        if let clash = Self.overlap(iface.network, others) {
            _ = try? await agent.send(["down", fields[0]])
            throw VLANError.overlap(iface.network.description, clash)
        }
        let active = Active(settings: settings, interface: fields[0], ipv4: fields[1], cidr: iface.network.description)
        self.active = active
        debugLog("vlan up: \(active)")
        return active
    }

    /// Remove the VLAN interface.
    func deactivate() {
        guard let active else { return }
        debugLog("vlan down: \(active.interface)")
        self.active = nil
        if let agent { Task { _ = try? await agent.send(["down", active.interface]) } }
    }

    /// Back to the settings of a new session: the option off, no tag,
    /// DHCP, and the VLAN interface removed. The helper keeps running, so
    /// the password is not asked again.
    func reset() {
        enabled = false
        stopDiscovery()
        deactivate()
        tag = ""
        typingTag = false
        addressing = .dhcp
        address = ""
        discovered = []
        discoveryProblem = nil
    }

    /// Something to reset: the option on, a tag, or a VLAN interface up.
    var canReset: Bool {
        enabled || active != nil || !tag.isEmpty || addressing != .dhcp || !address.isEmpty
    }

    /// NetScout is quitting: the agent removes what it made and exits.
    func shutdown() {
        agent?.stop()
        agent = nil
        active = nil
    }

    /// The running agent, else a new one (one password prompt at a time).
    func agentReady() async throws -> VLANAgent {
        if let agent, agent.isAlive { return agent }
        agent = nil
        active = nil
        if let agentStarting { return try await agentStarting.value }
        let starting = Task { try await VLANAgent.start() }
        agentStarting = starting
        defer { agentStarting = nil }
        let agent = try await starting.value
        self.agent = agent
        return agent
    }

    private func settingsChanged() {
        guard let active else { return }
        if !isOn || (try? settings.get()) != active.settings { deactivate() }
    }

    /// The interface of the first network in `networks` overlapping `net`.
    private static func overlap(_ net: IPv4Interface, _ networks: [NetworkInfo]) -> String? {
        networks.first { other in
            guard let o = IPv4Interface(other.cidr) else { return false }
            let prefix = min(net.prefix, o.prefix)
            let mask: UInt32 = prefix == 0 ? 0 : ~0 << (32 - prefix)
            return net.address & mask == o.address & mask
        }.map { "\($0.interface) (\($0.cidr))" }
    }
}

/// A VLAN heard on the cable: its tag, the devices that sent frames tagged
/// with it and the networks of their addresses.
struct DiscoveredVLAN: Identifiable, Equatable {
    let tag: Int
    /// Source MACs.
    var devices: Set<String> = []
    /// First three octets ("172.20.20") → frames seen, at least.
    var networks: [String: Int] = [:]

    var id: Int { tag }

    /// The network heard most, as its first three octets.
    var mainNetwork: String? { networks.max { $0.value < $1.value }?.key }

    /// "VLAN 69 · 172.20.20.x · 4 dispositivi".
    var label: String {
        let count = devices.count == 1 ? "1 dispositivo" : "\(devices.count) dispositivi"
        return (["VLAN \(tag)"] + (mainNetwork.map { ["\($0).x"] } ?? []) + [count]).joined(separator: " · ")
    }

    /// The agent's summary lines ("mac TAG MAC", "net TAG A.B.C COUNT"),
    /// by tag.
    static func parse(_ text: String) -> [DiscoveredVLAN] {
        var byTag: [Int: DiscoveredVLAN] = [:]
        for line in text.split(separator: "\n") {
            let f = line.split(separator: " ").map(String.init)
            guard f.count >= 3, let tag = Int(f[1]), (1...4094).contains(tag) else { continue }
            var vlan = byTag[tag] ?? DiscoveredVLAN(tag: tag)
            switch f[0] {
            case "mac": vlan.devices.insert(f[2])
            case "net" where f.count >= 4: vlan.networks[f[2]] = max(vlan.networks[f[2]] ?? 0, Int(f[3]) ?? 0)
            case "frames": break
            default: continue
            }
            byTag[tag] = vlan
        }
        return byTag.values.sorted { $0.tag < $1.tag }
    }
}

enum VLANError: LocalizedError, Equatable {
    case invalid(String)
    case cancelled
    case overlap(String, String)
    case failed(String)

    var errorDescription: String? {
        switch self {
        case .invalid(let message): message
        case .cancelled: "VLAN non attivata: password di amministratore non inserita."
        case .overlap(let net, let other): "La rete della VLAN, \(net), si sovrappone a \(other): è già raggiungibile senza VLAN."
        case .failed(let message): "Impossibile attivare la VLAN: \(message)"
        }
    }

    /// The message without "Impossibile attivare la VLAN".
    var detail: String {
        if case .failed(let message) = self { message } else { errorDescription ?? "" }
    }
}

/// An IPv4 address with its prefix length ("172.20.20.250/24").
struct IPv4Interface: Equatable, CustomStringConvertible {
    let address: UInt32
    let prefix: Int

    init?(_ text: String) {
        let parts = text.trimmingCharacters(in: .whitespaces).split(separator: "/")
        guard parts.count == 2, NameServers.isIPv4(String(parts[0])),
              let prefix = Int(parts[1]), (0...32).contains(prefix) else { return nil }
        self.address = ipValue(String(parts[0]))
        self.prefix = prefix
    }

    private init(address: UInt32, prefix: Int) {
        self.address = address
        self.prefix = prefix
    }

    private var mask: UInt32 { prefix == 0 ? 0 : ~0 << (32 - prefix) }

    var network: IPv4Interface { IPv4Interface(address: address & mask, prefix: prefix) }
    /// `ip` (dotted) is in this network.
    func contains(_ ip: String) -> Bool {
        NameServers.isIPv4(ip) && ipValue(ip) & mask == address & mask
    }
    var ip: String { Self.dotted(address) }
    var netmask: String { Self.dotted(mask) }
    var description: String { "\(ip)/\(prefix)" }

    private static func dotted(_ value: UInt32) -> String {
        [24, 16, 8, 0].map { String((value >> $0) & 0xff) }.joined(separator: ".")
    }
}
