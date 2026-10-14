import AppKit
import NetScoutCore
import Observation
import SwiftUI
import SystemConfiguration

/// The switch the Mac is plugged into, from the LLDP or CDP announcement
/// it sends on the port (about every 30 s; CDP every 60 s).
///
/// Hearing it needs an administrator: it goes through the root helper of
/// the tagged VLANs (`TaggedVLAN.agentReady`), so the password is asked
/// once per session for both. The answer is about one port of the Mac and
/// is kept for that port only.
@MainActor
@Observable
final class LinkedSwitch {
    enum State: Equatable {
        case idle
        case listening(port: String)
        case found(LinkNeighbor, port: String, at: Date)
        /// Nothing announced on the port within `listenSeconds`.
        case silent(port: String)
        case failed(String)
    }

    private(set) var state = State.idle
    private var task: Task<Void, Never>?
    private let vlan: TaggedVLAN

    /// LLDP every 30 s, CDP every 60 s, and some margin.
    static let listenSeconds = 65

    init(vlan: TaggedVLAN) {
        self.vlan = vlan
    }

    /// Listen on `port` for the announcement (asking for the password if
    /// the helper is not running).
    func identify(port: String) {
        task?.cancel()
        state = .listening(port: port)
        task = Task { [weak self] in
            await self?.listen(on: port)
        }
    }

    func cancel() {
        task?.cancel()
        task = nil
        state = .idle
    }

    /// The scan's interface changed: an answer about another port no
    /// longer holds.
    func follow(port: String?) {
        switch state {
        case .found(_, let p, _), .silent(let p), .listening(let p):
            if p != port { cancel() }
        case .failed:
            state = .idle
        case .idle:
            break
        }
    }

    private func listen(on port: String) async {
        do {
            let agent = try await vlan.agentReady()
            let path = try await agent.send(
                ["neighbor", port, String(Self.listenSeconds)],
                timeout: .seconds(Self.listenSeconds + 15)
            )
            guard !Task.isCancelled else { return }
            let data = (try? Data(contentsOf: URL(filePath: path))) ?? Data()
            try? FileManager.default.removeItem(atPath: path)
            if let neighbor = parseLinkNeighborPcap(pcap: data) {
                debugLog("neighbor on \(port): \(neighbor)")
                state = .found(neighbor, port: port, at: Date())
            } else {
                state = .silent(port: port)
            }
        } catch {
            guard !Task.isCancelled else { return }
            state = .failed((error as? VLANError) == .cancelled
                ? "Password di amministratore non inserita."
                : "Ascolto non riuscito: \((error as? VLANError)?.detail ?? error.localizedDescription)")
        }
    }

    /// The Mac's Wi-Fi ports: switches' announcements do not cross an
    /// access point.
    static func isWiFi(_ port: String) -> Bool {
        let all = SCNetworkInterfaceCopyAll() as NSArray as? [SCNetworkInterface] ?? []
        return all.contains { iface in
            SCNetworkInterfaceGetBSDName(iface) as String? == port
                && SCNetworkInterfaceGetInterfaceType(iface) == kSCNetworkInterfaceTypeIEEE80211
        }
    }

    /// The port's name as the system shows it ("USB 10/100/1G/2.5G LAN").
    static func displayName(_ port: String) -> String {
        let all = SCNetworkInterfaceCopyAll() as NSArray as? [SCNetworkInterface] ?? []
        let name = all.first { SCNetworkInterfaceGetBSDName($0) as String? == port }
            .flatMap { SCNetworkInterfaceGetLocalizedDisplayName($0) as String? }
        return name.map { "\($0) (\(port))" } ?? port
    }
}

extension ScanModel {
    /// The port to hear the switch on: the scan's interface when it is
    /// wired, else the first wired port with a network (the Mac can be on
    /// the same network through Wi-Fi and the cable).
    var switchPort: String? {
        if let interface, !LinkedSwitch.isWiFi(interface) { return interface }
        return networks.map(\.interface).first { $0.hasPrefix("en") && !LinkedSwitch.isWiFi($0) }
    }
}

// MARK: - Toolbar button

/// The toolbar button: a network device with a question mark; a click
/// shows what the switch announced, listening first if needed.
///
/// The popover is AppKit's, not `.popover`: the toolbar is rebuilt
/// whenever its items change (the icon pulses while listening), and a
/// SwiftUI popover attached to an item closes with it.
struct LinkedSwitchButton: View {
    @Environment(ScanModel.self) private var model

    var body: some View {
        Button {
            identifyIfNeeded()
            LinkedSwitchPopover.toggle(model: model)
        } label: {
            Label {
                Text("Switch collegato")
            } icon: {
                LinkedSwitchIcon(listening: isListening)
            }
        }
        .background(PopoverAnchor())
        .help("Switch collegato: a quale switch e porta è collegato il Mac (LLDP/CDP)")
    }

    private var isListening: Bool {
        if case .listening = model.linkedSwitch.state { true } else { false }
    }

    /// Listen when there is no answer yet for the scan's port.
    private func identifyIfNeeded() {
        guard let port = model.switchPort else { return }
        switch model.linkedSwitch.state {
        case .idle, .failed: model.linkedSwitch.identify(port: port)
        case .found(_, let p, _), .silent(let p), .listening(let p):
            if p != port { model.linkedSwitch.identify(port: port) }
        }
    }
}

/// Records the button's view, for the popover to point at.
private struct PopoverAnchor: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        LinkedSwitchPopover.anchor = view
        return view
    }

    func updateNSView(_ view: NSView, context: Context) {
        LinkedSwitchPopover.anchor = view
    }
}

/// A network device on its cable with a small question mark at its top
/// right, drawn like the toolbar's other symbols.
private struct LinkedSwitchIcon: View {
    let listening: Bool

    var body: some View {
        Image(systemName: "rectangle.connected.to.line.below")
            .overlay(alignment: .topTrailing) {
                Image(systemName: "questionmark")
                    .font(.system(size: 8, weight: .black))
                    .offset(x: 6, y: -6)
            }
            .symbolEffect(.pulse, isActive: listening)
    }
}

// MARK: - Popover

private struct LinkedSwitchPopover: View {
    @Environment(ScanModel.self) private var model

    /// The toolbar button's view, as last laid out.
    static weak var anchor: NSView?
    private static var shown: NSPopover?

    /// Show the popover under the button, or close it if shown.
    static func toggle(model: ScanModel) {
        if let shown, shown.isShown {
            shown.performClose(nil)
            return
        }
        let popover = NSPopover()
        // Not .transient: the password prompt takes the focus, and the
        // popover must still be there with the answer.
        popover.behavior = .semitransient
        popover.animates = true
        let host = NSHostingController(rootView: LinkedSwitchPopover().environment(model))
        host.sizingOptions = .preferredContentSize
        popover.contentViewController = host
        shown = popover
        if let anchor, anchor.window != nil {
            popover.show(relativeTo: anchor.bounds, of: anchor, preferredEdge: .minY)
        } else if let content = NSApp.keyWindow?.contentView {
            // Top right of the window, under the toolbar.
            let rect = NSRect(x: content.bounds.maxX - 40, y: content.bounds.maxY - 1, width: 20, height: 1)
            popover.show(relativeTo: rect, of: content, preferredEdge: .minY)
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            content
        }
        .padding(16)
        .frame(width: 360)
    }

    private var port: String? { model.switchPort }

    @ViewBuilder
    private var content: some View {
        if port == nil {
            message(
                symbol: "cable.connector", tint: .secondary,
                title: "Il Mac non è collegato via cavo",
                text: "Gli switch annunciano chi sono solo sul cavo: in Wi‑Fi l'access point non fa passare gli annunci. Collega il Mac a una porta di rete."
            )
        } else {
            switch model.linkedSwitch.state {
            case .idle:
                retry(title: "Identifica lo switch")
            case .listening(let port):
                listening(port)
            case .found(let neighbor, let port, let date):
                NeighborDetails(neighbor: neighbor, port: port, date: date) {
                    model.linkedSwitch.identify(port: port)
                }
            case .silent(let port):
                message(
                    symbol: "exclamationmark.triangle.fill", tint: .orange,
                    title: "Nessun annuncio dallo switch",
                    text: "In \(LinkedSwitch.listenSeconds) secondi su \(LinkedSwitch.displayName(port)) non è arrivato nessun annuncio LLDP o CDP: lo switch non è gestito oppure ha questi annunci disattivati."
                )
                retry(title: "Ascolta di nuovo")
            case .failed(let problem):
                message(symbol: "exclamationmark.triangle.fill", tint: .orange, title: "Switch non identificato", text: problem)
                retry(title: "Riprova")
            }
        }
    }

    private func listening(_ port: String) -> some View {
        HStack(alignment: .top, spacing: 12) {
            ProgressView().controlSize(.small).padding(.top, 2)
            VStack(alignment: .leading, spacing: 4) {
                Text("In ascolto dello switch…").font(.headline)
                Text("Su \(LinkedSwitch.displayName(port)). Gli switch si annunciano ogni 30 secondi circa (Cisco ogni 60): l'attesa dura al massimo \(LinkedSwitch.listenSeconds) secondi.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Button("Annulla") { model.linkedSwitch.cancel() }
                    .buttonStyle(.link)
                    .padding(.top, 2)
            }
        }
    }

    private func message(symbol: String, tint: Color, title: String, text: String) -> some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: symbol)
                .font(.title2)
                .foregroundStyle(tint)
                .frame(width: 28)
            VStack(alignment: .leading, spacing: 4) {
                Text(title).font(.headline)
                Text(text)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func retry(title: String) -> some View {
        HStack {
            Spacer()
            Button(title) {
                if let port { model.linkedSwitch.identify(port: port) }
            }
            .keyboardShortcut(.defaultAction)
        }
    }
}

/// What the switch announced.
private struct NeighborDetails: View {
    @Environment(ScanModel.self) private var model
    let neighbor: LinkNeighbor
    let port: String
    let date: Date
    let refresh: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: symbol)
                .font(.system(size: 26))
                .foregroundStyle(.tint)
                .frame(width: 34)
            VStack(alignment: .leading, spacing: 3) {
                Text(neighbor.systemName ?? neighbor.chassisId ?? "Dispositivo senza nome")
                    .font(.title3.weight(.semibold))
                    .textSelection(.enabled)
                if let description = neighbor.model ?? neighbor.description {
                    Text(description)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                }
            }
        }
        Divider()
        Grid(alignment: .leading, horizontalSpacing: 14, verticalSpacing: 7) {
            if let p = neighbor.port {
                row("Porta dello switch") {
                    Text(p).font(.body.weight(.semibold)).monospacedDigit()
                }
            }
            if let d = neighbor.portDescription, d != neighbor.port {
                row("Descrizione porta") { Text(d) }
            }
            if let ip = neighbor.managementIp {
                row("Gestione") {
                    HStack(spacing: 6) {
                        Text(ip).monospacedDigit()
                        Button {
                            if let url = URL(string: "http://\(ip)") { NSWorkspace.shared.open(url) }
                        } label: {
                            Image(systemName: "arrow.up.right.square")
                        }
                        .buttonStyle(.borderless)
                        .help("Apri la pagina dello switch nel browser")
                    }
                }
            }
            if let id = neighbor.chassisId {
                row("Identificativo") { Text(id).font(.system(.body, design: .monospaced)) }
            }
            if !neighbor.capabilities.isEmpty {
                row("Tipo") { Text(neighbor.capabilities.joined(separator: ", ")) }
            }
        }
        .textSelection(.enabled)
        if let description = neighbor.description, neighbor.model != nil {
            Text(description)
                .font(.caption)
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
        Divider()
        HStack {
            Text("\(neighbor.protocol) · \(port) · \(date.formatted(date: .omitted, time: .shortened))")
                .font(.caption)
                .foregroundStyle(.secondary)
            Spacer()
            Button {
                refresh()
            } label: {
                Image(systemName: "arrow.clockwise")
            }
            .buttonStyle(.borderless)
            .help("Ascolta di nuovo l'annuncio")
            if let ip = neighbor.managementIp, model.hosts[ip] != nil {
                Button("Mostra nella tabella") {
                    model.scanSelection = ip
                    model.typeFilter = nil
                    model.segmentFilter = nil
                    model.scanSearch = ""
                    model.showDeviceCard = true
                }
            }
        }
    }

    private var symbol: String {
        let caps = neighbor.capabilities
        if caps.contains("Switch") { return "rectangle.connected.to.line.below" }
        if caps.contains("Router") { return "wifi.router" }
        if caps.contains("Access point") { return "wifi" }
        if caps.contains("Telefono") { return "phone" }
        return "network"
    }

    private func row(_ label: String, @ViewBuilder value: () -> some View) -> some View {
        GridRow {
            Text(label)
                .foregroundStyle(.secondary)
                .gridColumnAlignment(.trailing)
            value()
        }
    }
}
