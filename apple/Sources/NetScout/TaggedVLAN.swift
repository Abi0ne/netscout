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
/// The Mac only sees a tagged VLAN through a VLAN interface of its own, and
/// creating one needs an administrator: NetScout asks for the password once,
/// creates `vlanN` on the chosen Ethernet port, gives it an address (from the
/// VLAN's DHCP, or the one written here) and from then on scans the VLAN's
/// network like any other. The interface stays up after the scan, so a deep
/// scan or a terminal session to a device of the VLAN still works, until the
/// option is turned off, its settings change or the app quits.
///
/// The same privileged call leaves a small root watcher behind that removes
/// the interface as soon as NetScout's marker file is deleted or NetScout's
/// process is gone (even after a crash), so no second password is needed and
/// nothing is left behind. An interface made this way is not a network
/// service: the system settings are never touched, and a restart clears it.
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

    var enabled: Bool {
        didSet { Self.defaults.set(enabled, forKey: Keys.enabled); settingsChanged() }
    }
    /// The VLAN ID as written (1–4094).
    var tag: String {
        didSet { Self.defaults.set(tag, forKey: Keys.tag); settingsChanged() }
    }
    /// The port the VLAN rides on: the scan's interface, when it can carry
    /// VLANs; `nil` hides the option.
    private(set) var parent: String? {
        didSet { settingsChanged() }
    }
    var addressing: Addressing {
        didSet { Self.defaults.set(addressing.rawValue, forKey: Keys.addressing); settingsChanged() }
    }
    /// The manual address with its prefix, e.g. "172.20.20.250/24".
    var address: String {
        didSet { Self.defaults.set(address, forKey: Keys.address); settingsChanged() }
    }

    private(set) var active: Active?
    private(set) var isPreparing = false
    /// The ports of the Mac that can carry VLANs.
    private(set) var capable: Set<String> = []

    /// Deleting this file tells the root watcher to remove the interface.
    private var marker: URL?

    private enum Keys {
        static let enabled = "vlan.enabled"
        static let tag = "vlan.tag"
        static let addressing = "vlan.addressing"
        static let address = "vlan.address"
    }

    private static var defaults: UserDefaults { .standard }

    init() {
        let d = Self.defaults
        enabled = d.bool(forKey: Keys.enabled)
        tag = d.string(forKey: Keys.tag) ?? ""
        addressing = d.string(forKey: Keys.addressing).flatMap(Addressing.init) ?? .dhcp
        address = d.string(forKey: Keys.address) ?? ""
    }

    /// The settings as written, or what is wrong with them.
    var settings: Result<Settings, VLANError> {
        guard let id = Int(tag.trimmingCharacters(in: .whitespaces)), (1...4094).contains(id) else {
            return .failure(.invalid("Il tag VLAN va da 1 a 4094."))
        }
        guard let parent else {
            return .failure(.invalid("L'interfaccia scelta non può ricevere VLAN taggate."))
        }
        var settings = Settings(tag: id, parent: parent, addressing: addressing, address: nil)
        if addressing == .manual {
            guard let iface = IPv4Interface(address), iface.prefix <= 30 else {
                return .failure(.invalid("Scrivi l'indirizzo con il prefisso, per esempio 172.20.20.250/24."))
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

    /// The VLAN interface up with the current settings: the one already
    /// made, else a new one (asking for the administrator password).
    /// `networks` are the other networks of the Mac: a VLAN network that
    /// overlaps one of them is refused.
    func prepare(networks: [NetworkInfo]) async throws -> Active {
        let settings = try settings.get()
        if let active, active.settings == settings { return active }
        deactivate()

        if let iface = settings.address, let clash = Self.overlap(iface.network, networks) {
            throw VLANError.overlap(iface.network.description, clash)
        }

        isPreparing = true
        defer { isPreparing = false }
        let marker = FileManager.default.temporaryDirectory
            .appending(path: "netscout-vlan-\(UUID().uuidString)")
        guard FileManager.default.createFile(atPath: marker.path(percentEncoded: false), contents: nil) else {
            throw VLANError.failed("impossibile creare il file di controllo in \(marker.path(percentEncoded: false))")
        }
        let output: String
        do {
            output = try await Self.runPrivileged(Self.upCommand(settings, marker: marker))
        } catch {
            try? FileManager.default.removeItem(at: marker)
            throw error
        }
        self.marker = marker

        // The last line: "vlan0 172.20.20.250 0xffffff00".
        let fields = output.split(separator: "\n").last?.split(separator: " ").map(String.init) ?? []
        guard fields.count == 3, let mask = UInt32(fields[2].dropFirst(2), radix: 16),
              let iface = IPv4Interface(fields[1] + "/\(mask.nonzeroBitCount)") else {
            deactivate()
            throw VLANError.failed("risposta inattesa: \(output)")
        }
        let others = networks.filter { $0.interface != fields[0] }
        if let clash = Self.overlap(iface.network, others) {
            deactivate()
            throw VLANError.overlap(iface.network.description, clash)
        }
        let active = Active(settings: settings, interface: fields[0], ipv4: fields[1], cidr: iface.network.description)
        self.active = active
        debugLog("vlan up: \(active)")
        return active
    }

    /// Remove the VLAN interface (the root watcher does it within a second).
    func deactivate() {
        if let marker { try? FileManager.default.removeItem(at: marker) }
        marker = nil
        if let active { debugLog("vlan down: \(active.interface)") }
        active = nil
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

    // MARK: - Privileged commands

    /// Creates the interface, addresses it and leaves the watcher; prints
    /// "<interface> <address> <hex netmask>". Arguments: parent, tag, mode,
    /// address, netmask, marker file, NetScout's pid. No single quotes in
    /// here: it is passed to `sh -c` inside them.
    private static let upScript = #"""
        parent=$1; tag=$2; mode=$3; addr=$4; mask=$5; marker=$6; app=$7
        busy=""
        for attempt in 1 2 3 4 5; do
          busy=""
          for v in $(ifconfig -l); do
            case $v in vlan*) ifconfig $v | grep -q "vlan: $tag parent interface: $parent$" && busy=$v ;; esac
          done
          [ -z "$busy" ] && break
          sleep 1
        done
        if [ -n "$busy" ]; then echo "la VLAN $tag su $parent esiste già ($busy)" >&2; exit 3; fi
        n=0
        while ifconfig vlan$n >/dev/null 2>&1; do n=$((n+1)); done
        if=vlan$n
        down() { ipconfig set $if NONE >/dev/null 2>&1; ifconfig $if destroy; }
        ifconfig $if create || exit 1
        if ! ifconfig $if vlan $tag vlandev $parent || ! ifconfig $if up; then down; exit 1; fi
        if [ "$mode" = dhcp ]; then
          ipconfig set $if DHCP
          ip=""; i=0
          while [ -z "$ip" ] && [ $i -lt 30 ]; do sleep 1; ip=$(ipconfig getifaddr $if); i=$((i+1)); done
          if [ -z "$ip" ]; then down; echo "nessuna risposta dal DHCP della VLAN $tag in 30 secondi" >&2; exit 4; fi
          m=$(ipconfig getoption $if subnet_mask)
          # Keep the leased address but drop router and DNS: the VLAN must
          # never take over the default route or the name servers.
          ipconfig set $if MANUAL $ip $m
          i=0
          while ! ifconfig $if | grep -q "inet $ip " && [ $i -lt 10 ]; do sleep 1; i=$((i+1)); done
        elif ! ifconfig $if inet $addr netmask $mask; then
          down; exit 1
        fi
        nohup /bin/sh -c "while kill -0 $app 2>/dev/null && [ -e $marker ]; do sleep 1; done; ipconfig set $if NONE; ifconfig $if destroy" >/dev/null 2>&1 &
        echo "$if $(ifconfig $if | awk "/inet /{print \$2, \$4; exit}")"
        """#

    private static func upCommand(_ settings: Settings, marker: URL) -> String {
        let args = [
            settings.parent,
            String(settings.tag),
            settings.addressing.rawValue,
            settings.address?.ip ?? "-",
            settings.address?.netmask ?? "-",
            marker.path(percentEncoded: false),
            String(ProcessInfo.processInfo.processIdentifier),
        ]
        // Every argument is a validated name, number or address, or a path
        // in the user's temporary folder; quoted anyway.
        return "/bin/sh -c '\(upScript)' netscout-vlan " + args.map { "'\($0)'" }.joined(separator: " ")
    }

    /// Run `command` as root through the system's administrator prompt.
    private static func runPrivileged(_ command: String) async throws -> String {
        let escaped = command
            .replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"")
        let script = "do shell script \"\(escaped)\" with administrator privileges without altering line endings "
            + "with prompt \"NetScout deve creare un'interfaccia VLAN per scansionare la rete taggata.\""
        return try await withCheckedThrowingContinuation { continuation in
            let process = Process()
            process.executableURL = URL(filePath: "/usr/bin/osascript")
            process.arguments = ["-e", script]
            let out = Pipe(), err = Pipe()
            process.standardOutput = out
            process.standardError = err
            process.terminationHandler = { p in
                let stdout = String(decoding: out.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
                let stderr = String(decoding: err.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
                if p.terminationStatus == 0 {
                    continuation.resume(returning: stdout.trimmingCharacters(in: .whitespacesAndNewlines))
                } else if stderr.contains("-128") {
                    continuation.resume(throwing: VLANError.cancelled)
                } else {
                    // "osascript: … execution error: <message> (<code>)"
                    var message = stderr.trimmingCharacters(in: .whitespacesAndNewlines)
                    if let r = message.range(of: "execution error: ") { message = String(message[r.upperBound...]) }
                    if let r = message.range(of: #"\s*\(-?\d+\)$"#, options: .regularExpression) { message.removeSubrange(r) }
                    continuation.resume(throwing: VLANError.failed(message))
                }
            }
            do {
                try process.run()
            } catch {
                continuation.resume(throwing: VLANError.failed("\(error)"))
            }
        }
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
    var ip: String { Self.dotted(address) }
    var netmask: String { Self.dotted(mask) }
    var description: String { "\(ip)/\(prefix)" }

    private static func dotted(_ value: UInt32) -> String {
        [24, 16, 8, 0].map { String((value >> $0) & 0xff) }.joined(separator: ".")
    }
}
