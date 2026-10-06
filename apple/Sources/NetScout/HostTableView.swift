import NetScoutCore
import SwiftUI

/// One table row: the host plus the sortable keys the columns need. A device
/// that is off (known from a profile, not found now) has its own id.
struct HostRow: Identifiable {
    let host: Host
    var offline = false
    var id: String { offline ? ScanModel.offlineID(host) : host.ip }
    var ipKey: UInt32 { ipValue(host.ip) }
    var typeKey: String { host.deviceType.label }
    var nameKey: String { host.displayName ?? "" }
    var vendorKey: String { host.vendor ?? "" }
    var macKey: String { host.mac ?? "" }
    var rttKey: Double { offline ? .infinity : host.rttMs ?? .greatestFiniteMagnitude }
    var portsKey: Int { host.openPorts.count }
}

struct HostTableView: View {
    let hosts: [Host]
    /// Devices that are off, listed greyed out after the others.
    var offline: [Host] = []
    @Binding var selection: String?
    /// Context-menu action for a device that is off (e.g. forget it).
    var forgetOffline: ((String) -> Void)?
    @ViewState private var sortOrder = [KeyPathComparator(\HostRow.ipKey)]

    var body: some View {
        Table(rows, selection: $selection, sortOrder: $sortOrder) {
            TableColumn("Tipo", value: \.typeKey) { row in
                Label(row.host.deviceType.label, systemImage: row.host.deviceType.symbol)
                    .foregroundStyle(row.offline || row.host.deviceType == .unknown ? .secondary : .primary)
            }
            .width(min: 90, ideal: 120)
            TableColumn("IP", value: \.ipKey) { row in
                Text(row.host.ip).monospacedDigit()
                    .foregroundStyle(row.offline ? .secondary : .primary)
            }
            .width(min: 95, ideal: 110)
            TableColumn("Nome", value: \.nameKey) { row in
                Text(row.host.displayName ?? "—")
                    .foregroundStyle(row.host.displayName == nil ? .tertiary : row.offline ? .secondary : .primary)
            }
            .width(min: 120, ideal: 190)
            TableColumn("Produttore", value: \.vendorKey) { row in
                Text(row.host.vendor ?? "—")
                    .foregroundStyle(row.host.vendor == nil ? .tertiary : .primary)
            }
            .width(min: 110, ideal: 170)
            TableColumn("MAC", value: \.macKey) { row in
                Text(row.host.mac ?? "—")
                    .font(.system(.body, design: .monospaced))
                    .foregroundStyle(.secondary)
            }
            .width(min: 130, ideal: 145)
            TableColumn("Latenza", value: \.rttKey) { row in
                if row.offline {
                    Label("Spento", systemImage: "power")
                        .font(.caption.weight(.medium))
                        .foregroundStyle(.orange)
                } else {
                    Text(row.host.rttMs.map { String(format: "%.1f ms", $0) } ?? "—")
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                }
            }
            .width(min: 60, ideal: 70)
            TableColumn("Porte", value: \.portsKey) { row in
                Text(row.host.openPorts.map { String($0.number) }.joined(separator: " "))
                    .foregroundStyle(.secondary)
                    .help(row.host.openPorts.map { "\($0.number) \($0.service ?? "")" }.joined(separator: "\n"))
            }
            .width(min: 60, ideal: 120)
        }
        .contextMenu(forSelectionType: String.self) { ids in
            if let forgetOffline, let id = ids.first, id.hasPrefix("off-") {
                Button("Dimentica dispositivo spento") { forgetOffline(id) }
            }
        }
    }

    private var rows: [HostRow] {
        hosts.map { HostRow(host: $0) }.sorted(using: sortOrder)
            + offline.map { HostRow(host: $0, offline: true) }.sorted(using: sortOrder)
    }
}
