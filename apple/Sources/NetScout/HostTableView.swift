import AppKit
import NetScoutCore
import SwiftUI

/// One table row: the host plus the sortable keys the columns need. A device
/// that is off (known from a profile, not found now) has its own id.
struct HostRow: Identifiable {
    let host: Host
    var offline = false
    /// The user's note (profiles tab only).
    var note = ""
    /// Where it was found (a scan with a tagged VLAN only).
    var segment: ScanSegment?
    var id: String { offline ? ScanModel.offlineID(host) : host.ip }
    var ipKey: UInt32 { ipValue(host.ip) }
    var typeKey: String { host.deviceType.label }
    var nameKey: String { host.displayName ?? "" }
    var vendorKey: String { host.vendor ?? "" }
    var macKey: String { host.mac ?? "" }
    var rttKey: Double { offline ? .infinity : host.rttMs ?? .greatestFiniteMagnitude }
    var portsKey: Int { host.openPorts.count }
    var noteKey: String { note }
    var segmentKey: ScanSegment { segment ?? .native }
}

/// The editable notes column of a profile's table.
struct NoteEditing {
    let note: (Host) -> String
    let setNote: (String, Host) -> Void
    /// Locked notes are only shown.
    var editable = true
}

/// The table's columns: titles and sort keys.
enum HostColumn: CaseIterable {
    case type, ip, network, name, vendor, mac, latency, ports, note

    var title: String {
        switch self {
        case .type: "Tipo"
        case .ip: "IP"
        case .network: "VLAN"
        case .name: "Nome"
        case .vendor: "Produttore"
        case .mac: "MAC"
        case .latency: "Latenza"
        case .ports: "Porte"
        case .note: "Note"
        }
    }

    /// The column's comparator, as the table builds it from `value:`.
    func comparator(ascending: Bool) -> KeyPathComparator<HostRow> {
        let order: SortOrder = ascending ? .forward : .reverse
        return switch self {
        case .type: KeyPathComparator(\HostRow.typeKey, comparator: .localizedStandard, order: order)
        case .ip: KeyPathComparator(\HostRow.ipKey, order: order)
        case .network: KeyPathComparator(\HostRow.segmentKey, order: order)
        case .name: KeyPathComparator(\HostRow.nameKey, comparator: .localizedStandard, order: order)
        case .vendor: KeyPathComparator(\HostRow.vendorKey, comparator: .localizedStandard, order: order)
        case .mac: KeyPathComparator(\HostRow.macKey, comparator: .localizedStandard, order: order)
        case .latency: KeyPathComparator(\HostRow.rttKey, order: order)
        case .ports: KeyPathComparator(\HostRow.portsKey, order: order)
        case .note: KeyPathComparator(\HostRow.noteKey, comparator: .localizedStandard, order: order)
        }
    }

    init?(_ comparator: KeyPathComparator<HostRow>) {
        guard let column = Self.allCases.first(where: {
            $0.comparator(ascending: true).keyPath == comparator.keyPath
        }) else { return nil }
        self = column
    }
}

/// One column taking part in the sort.
struct SortKey: Equatable {
    var column: HostColumn
    var ascending: Bool
}

struct HostTableView: View {
    let hosts: [Host]
    /// Devices that are off, listed greyed out after the others.
    var offline: [Host] = []
    @Binding var selection: String?
    /// Context-menu action for a device that is off (e.g. forget it).
    var forgetOffline: ((String) -> Void)?
    /// The sort, most important column first. A click on a header sorts by
    /// that column alone (again: reversed), as before; ⇧-click adds the
    /// column after the others, then reverses it, then takes it out.
    @ViewState private var sortKeys = [SortKey(column: .ip, ascending: true)]

    /// When set, the table has an editable "Note" column.
    var notes: NoteEditing?
    /// When set, the table has a "VLAN" column: where each device was found.
    var segment: ((Host) -> ScanSegment?)?
    /// The row under the mouse: its note shows the pencil.
    @ViewState private var hoveredRow: String?

    var body: some View {
        Group {
            // One table per set of columns: the column builder of macOS 14.0
            // takes no `if`.
            switch (notes, segment != nil) {
            case (let notes?, true):
                Table(rows, selection: $selection, sortOrder: sortOrder) {
                    leadingColumns
                    networkColumn
                    columns
                    noteColumn(notes)
                }
            case (let notes?, false):
                Table(rows, selection: $selection, sortOrder: sortOrder) {
                    leadingColumns
                    columns
                    noteColumn(notes)
                }
            case (nil, true):
                Table(rows, selection: $selection, sortOrder: sortOrder) {
                    leadingColumns
                    networkColumn
                    columns
                }
            case (nil, false):
                Table(rows, selection: $selection, sortOrder: sortOrder) {
                    leadingColumns
                    columns
                }
            }
        }
        .contextMenu(forSelectionType: String.self) { ids in
            if let id = ids.first, let host = rows.first(where: { $0.id == id })?.host {
                Button("Copia indirizzo IP") { copy(host.ip) }
                Button("Copia indirizzo MAC") { copy(host.mac ?? "") }
                    .disabled(host.mac == nil)
            }
            if let forgetOffline, let id = ids.first, id.hasPrefix("off-") {
                Divider()
                Button("Dimentica dispositivo spento") { forgetOffline(id) }
            }
        }
        .background(TableRowHover { index in
            let rows = rows
            hoveredRow = index.flatMap { $0 < rows.count ? rows[$0].id : nil }
        })
        .background(TableHeaderActions(
            contentWidth: { fitWidth($0) },
            addSort: { sortKeys = Self.sort(sortKeys, clicking: $0, adding: true) }
        ))
    }

    private func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    // MARK: Sorting

    /// The sort as the table sees it. The table reports a plain header
    /// click by putting that column's comparator first; ⇧-clicks come from
    /// `TableHeaderActions`.
    private var sortOrder: Binding<[KeyPathComparator<HostRow>]> {
        Binding(
            get: { sortKeys.map { $0.column.comparator(ascending: $0.ascending) } },
            set: { new in
                guard let first = new.first, let clicked = HostColumn(first) else { return }
                sortKeys = Self.sort(sortKeys, clicking: clicked, adding: false)
            }
        )
    }

    static func sort(_ keys: [SortKey], clicking column: HostColumn, adding: Bool) -> [SortKey] {
        guard adding else {
            // As before: the column alone, reversed if it already led.
            let ascending = keys.first?.column == column ? !(keys.first?.ascending ?? false) : true
            return [SortKey(column: column, ascending: ascending)]
        }
        var keys = keys
        if let i = keys.firstIndex(where: { $0.column == column }) {
            if keys[i].ascending {
                keys[i].ascending = false
            } else if keys.count > 1 {
                keys.remove(at: i)
            } else {
                keys[i].ascending = true
            }
        } else {
            keys.append(SortKey(column: column, ascending: true))
        }
        return keys
    }

    /// The header title: with several sort columns, each one shows its
    /// priority and direction ("Nome ²▼").
    private func title(_ column: HostColumn) -> String {
        guard sortKeys.count > 1, let i = sortKeys.firstIndex(where: { $0.column == column }) else {
            return column.title
        }
        let digits = ["⁰", "¹", "²", "³", "⁴", "⁵", "⁶", "⁷", "⁸", "⁹"]
        let rank = String(i + 1).compactMap { $0.wholeNumberValue.map { digits[$0] } }.joined()
        return "\(column.title) \(rank)\(sortKeys[i].ascending ? "▲" : "▼")"
    }

    // MARK: Columns

    /// Type and IP, before the network column.
    @TableColumnBuilder<HostRow, KeyPathComparator<HostRow>>
    private var leadingColumns: some TableColumnContent<HostRow, KeyPathComparator<HostRow>> {
        TableColumn(title(.type), value: \.typeKey) { row in
            Label(row.host.deviceType.label, systemImage: row.host.deviceType.symbol)
                .foregroundStyle(row.offline || row.host.deviceType == .unknown ? .secondary : .primary)
        }
        .width(min: 90, ideal: 120)
        .customizationID("type")
        TableColumn(title(.ip), value: \.ipKey) { row in
            Text(row.host.ip).monospacedDigit()
                .foregroundStyle(row.offline ? .secondary : .primary)
        }
        .width(min: 95, ideal: 110)
        .customizationID("ip")
    }

    private var networkColumn: some TableColumnContent<HostRow, KeyPathComparator<HostRow>> {
        TableColumn(title(.network), value: \.segmentKey) { row in
            if let segment = row.segment {
                Text(segment.shortLabel)
                    .monospacedDigit()
                    .foregroundStyle(segment.tint)
            }
        }
        .width(min: 70, ideal: 85)
        .customizationID("network")
    }

    private func noteColumn(_ notes: NoteEditing) -> some TableColumnContent<HostRow, KeyPathComparator<HostRow>> {
        TableColumn(title(.note), value: \.noteKey) { row in
            if notes.editable {
                NoteField(text: Binding(
                    get: { notes.note(row.host) },
                    set: { notes.setNote($0, row.host) }
                ), hovered: hoveredRow == row.id)
            } else {
                Text(row.note.isEmpty ? "—" : row.note)
                    .foregroundStyle(row.note.isEmpty ? .tertiary : .secondary)
                    .lineLimit(1)
                    .help(row.note)
            }
        }
        .width(min: 140, ideal: 240)
        .customizationID("note")
    }

    /// Name to ports.
    @TableColumnBuilder<HostRow, KeyPathComparator<HostRow>>
    private var columns: some TableColumnContent<HostRow, KeyPathComparator<HostRow>> {
        TableColumn(title(.name), value: \.nameKey) { row in
            Text(row.host.displayName ?? "—")
                .foregroundStyle(row.host.displayName == nil ? .tertiary : row.offline ? .secondary : .primary)
        }
        .width(min: 120, ideal: 190)
        .customizationID("name")
        TableColumn(title(.vendor), value: \.vendorKey) { row in
            Text(row.host.vendor ?? "—")
                .foregroundStyle(row.host.vendor == nil ? .tertiary : .primary)
        }
        .width(min: 110, ideal: 170)
        .customizationID("vendor")
        TableColumn(title(.mac), value: \.macKey) { row in
            Text(row.host.mac ?? "—")
                .font(.system(.body, design: .monospaced))
                .foregroundStyle(.secondary)
        }
        .width(min: 130, ideal: 145)
        .customizationID("mac")
        TableColumn(title(.latency), value: \.rttKey) { row in
            if row.offline {
                Label("Spento", systemImage: "power")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.orange)
            } else if row.host.offSubnet {
                Label("Fuori rete", systemImage: "cable.connector")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.purple)
                    .help("Sullo stesso cavo, ma con un indirizzo di un'altra rete: il Mac non lo raggiunge")
            } else {
                Text(row.latencyText)
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
            }
        }
        .width(min: 60, ideal: 70)
        .customizationID("latency")
        TableColumn(title(.ports), value: \.portsKey) { row in
            Text(row.portsText)
                .foregroundStyle(.secondary)
                .help(row.host.openPorts.map { "\($0.number) \($0.service ?? "")" }.joined(separator: "\n"))
        }
        .width(min: 60, ideal: 120)
        .customizationID("ports")
    }

    private var rows: [HostRow] {
        let order = sortKeys.map { $0.column.comparator(ascending: $0.ascending) }
        let note = { (host: Host) in notes?.note(host) ?? "" }
        let segment = { (host: Host) in self.segment?(host) }
        return hosts.map { HostRow(host: $0, note: note($0), segment: segment($0)) }.sorted(using: order)
            + offline.map { HostRow(host: $0, offline: true, note: note($0), segment: segment($0)) }.sorted(using: order)
    }

    // MARK: Fitting a column to its content

    /// Width of the widest value of `column` in the rows shown, as drawn by
    /// its cells (without the table's padding).
    private func fitWidth(_ column: HostColumn) -> CGFloat {
        let size = NSFont.systemFontSize
        let body = NSFont.systemFont(ofSize: size)
        let digits = NSFont.monospacedDigitSystemFont(ofSize: size, weight: .regular)
        let mono = NSFont.monospacedSystemFont(ofSize: size, weight: .regular)
        let caption = NSFont.systemFont(ofSize: NSFont.smallSystemFontSize, weight: .medium)
        let icon: CGFloat = 22 // a Label's symbol and spacing
        func width(_ text: String, _ font: NSFont) -> CGFloat {
            ceil((text as NSString).size(withAttributes: [.font: font]).width)
        }
        let all = rows
        let widths: [CGFloat] = all.map { row in
            switch column {
            case .type: width(row.host.deviceType.label, body) + icon
            case .ip: width(row.host.ip, digits)
            case .network: width(row.segment?.shortLabel ?? "", body)
            case .name: width(row.host.displayName ?? "—", body)
            case .vendor: width(row.host.vendor ?? "—", body)
            case .mac: width(row.host.mac ?? "—", mono)
            case .latency: row.offline ? width("Spento", caption) + icon : width(row.latencyText, digits)
            case .ports: width(row.portsText, body)
            case .note: width(row.note.isEmpty ? "Aggiungi una nota" : row.note, body)
            }
        }
        return widths.max() ?? 0
    }
}

extension HostRow {
    var latencyText: String { host.rttMs.map { String(format: "%.1f ms", $0) } ?? "—" }
    var portsText: String { host.openPorts.map { String($0.number) }.joined(separator: " ") }
}

/// A note cell: plain text that edits in place. On the row under the mouse
/// a pencil shows it can be written (click it, or the text, to edit).
private struct NoteField: View {
    @Binding var text: String
    let hovered: Bool
    @FocusState private var focused: Bool

    var body: some View {
        HStack(spacing: 4) {
            TextField("Nota", text: $text, prompt: Text(hovered || focused ? "Aggiungi una nota" : ""))
                .textFieldStyle(.plain)
                .lineLimit(1)
                .focused($focused)
            if hovered && !focused {
                Button {
                    focused = true
                } label: {
                    Image(systemName: "pencil")
                        .foregroundStyle(.tint)
                }
                .buttonStyle(.plain)
                .help(text.isEmpty ? "Aggiungi una nota" : "Modifica la nota")
                .transition(.opacity)
            }
        }
        .animation(.easeOut(duration: 0.12), value: hovered)
    }
}
