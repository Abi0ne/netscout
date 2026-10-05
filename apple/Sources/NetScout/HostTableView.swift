import NetScoutCore
import SwiftUI

/// One table row: the host plus the sortable keys the columns need.
struct HostRow: Identifiable {
    let host: Host
    var id: String { host.ip }
    var ipKey: UInt32 { ipValue(host.ip) }
    var typeKey: String { host.deviceType.label }
    var nameKey: String { host.displayName ?? "" }
    var vendorKey: String { host.vendor ?? "" }
    var macKey: String { host.mac ?? "" }
    var rttKey: Double { host.rttMs ?? .infinity }
    var portsKey: Int { host.openPorts.count }
}

struct HostTableView: View {
    let hosts: [Host]
    @Binding var selection: String?
    @ViewState private var sortOrder = [KeyPathComparator(\HostRow.ipKey)]

    var body: some View {
        Table(rows, selection: $selection, sortOrder: $sortOrder) {
            TableColumn("Tipo", value: \.typeKey) { row in
                Label(row.host.deviceType.label, systemImage: row.host.deviceType.symbol)
                    .foregroundStyle(row.host.deviceType == .unknown ? .secondary : .primary)
            }
            .width(min: 90, ideal: 120)
            TableColumn("IP", value: \.ipKey) { row in
                Text(row.host.ip).monospacedDigit()
            }
            .width(min: 95, ideal: 110)
            TableColumn("Nome", value: \.nameKey) { row in
                Text(row.host.displayName ?? "—")
                    .foregroundStyle(row.host.displayName == nil ? .tertiary : .primary)
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
                Text(row.host.rttMs.map { String(format: "%.1f ms", $0) } ?? "—")
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
            }
            .width(min: 60, ideal: 70)
            TableColumn("Porte", value: \.portsKey) { row in
                Text(row.host.openPorts.map { String($0.number) }.joined(separator: " "))
                    .foregroundStyle(.secondary)
                    .help(row.host.openPorts.map { "\($0.number) \($0.service ?? "")" }.joined(separator: "\n"))
            }
            .width(min: 60, ideal: 120)
        }
    }

    private var rows: [HostRow] {
        hosts.map(HostRow.init).sorted(using: sortOrder)
    }
}
