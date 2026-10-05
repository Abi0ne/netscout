import NetScoutCore
import SwiftUI

/// The differences between the finished scan and a saved profile.
struct ComparisonView: View {
    @Environment(\.dismiss) private var dismiss
    let comparison: ProfileComparison

    private var diff: ScanDiff { comparison.diff }
    private var hasDifferences: Bool {
        !diff.added.isEmpty || !diff.removed.isEmpty || !diff.changed.isEmpty
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header.padding(20)
            Divider()
            if hasDifferences {
                List {
                    if !diff.added.isEmpty {
                        Section("Nuovi dispositivi (\(diff.added.count))") {
                            ForEach(diff.added, id: \.ip) { host in
                                HostLine(host: host, mark: "plus.circle.fill", tint: .green)
                            }
                        }
                    }
                    if !diff.removed.isEmpty {
                        Section("Non più presenti (\(diff.removed.count))") {
                            ForEach(diff.removed, id: \.ip) { host in
                                HostLine(host: host, mark: "minus.circle.fill", tint: .red)
                            }
                        }
                    }
                    if !diff.changed.isEmpty {
                        Section("Modificati (\(diff.changed.count))") {
                            ForEach(diff.changed, id: \.after.ip) { change in
                                VStack(alignment: .leading, spacing: 4) {
                                    HostLine(host: change.after, mark: "pencil.circle.fill", tint: .orange)
                                    ForEach(change.descriptions, id: \.self) { line in
                                        Text(line)
                                            .font(.callout)
                                            .foregroundStyle(.secondary)
                                            .padding(.leading, 30)
                                    }
                                }
                                .padding(.vertical, 2)
                            }
                        }
                    }
                }
            } else {
                ContentUnavailableView(
                    "Nessuna differenza",
                    systemImage: "checkmark.circle",
                    description: Text("La rete è uguale a quella salvata nel profilo.")
                )
            }
            Divider()
            HStack {
                Spacer()
                Button("Chiudi") { dismiss() }
                    .keyboardShortcut(.defaultAction)
            }
            .padding(12)
        }
        .frame(minWidth: 640, idealWidth: 720, minHeight: 480, idealHeight: 600)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Confronto con «\(comparison.profile.name)»")
                .font(.title2.weight(.semibold))
            Text("Profilo salvato \(comparison.profile.createdAtDate.formatted(date: .abbreviated, time: .shortened)) · \(comparison.profile.target)")
                .foregroundStyle(.secondary)
            HStack(spacing: 8) {
                CountBadge(count: diff.added.count, label: "nuovi", tint: .green)
                CountBadge(count: diff.removed.count, label: "scomparsi", tint: .red)
                CountBadge(count: diff.changed.count, label: "modificati", tint: .orange)
                CountBadge(count: Int(diff.unchanged), label: "invariati", tint: .secondary)
            }
            if comparison.targetDiffers || comparison.depthDiffers {
                Label(caveat, systemImage: "exclamationmark.triangle.fill")
                    .font(.callout)
                    .foregroundStyle(.orange)
            }
        }
    }

    private var caveat: String {
        var parts: [String] = []
        if comparison.targetDiffers {
            parts.append("la scansione ha una destinazione diversa (\(comparison.profile.target) nel profilo)")
        }
        if comparison.depthDiffers {
            parts.append("il profilo è stato salvato con la scansione \(comparison.profile.scanProfile.label.lowercased()): le porte non verificate in una delle due risultano aperte o chiuse")
        }
        return "Attenzione: " + parts.joined(separator: "; ") + "."
    }
}

private struct HostLine: View {
    let host: Host
    let mark: String
    let tint: Color

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: mark).foregroundStyle(tint)
            Image(systemName: host.deviceType.symbol)
                .foregroundStyle(.secondary)
                .frame(width: 18)
            VStack(alignment: .leading, spacing: 1) {
                Text(host.displayName ?? host.ip).font(.body.weight(.medium))
                Text([host.ip, host.vendor, host.mac].compactMap { $0 }.joined(separator: " · "))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
        }
    }
}

private struct CountBadge: View {
    let count: Int
    let label: String
    let tint: Color

    var body: some View {
        Text("\(count) \(label)")
            .font(.callout.weight(.medium))
            .monospacedDigit()
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .foregroundStyle(count == 0 ? Color.secondary : tint)
            .background((count == 0 ? Color.secondary : tint).opacity(0.12), in: Capsule())
    }
}

extension HostChange {
    /// One line per difference, for display.
    var descriptions: [String] {
        var lines: [String] = []
        if ipChanged { lines.append("IP: \(before.ip) → \(after.ip)") }
        if macChanged { lines.append("MAC: \(before.mac ?? "—") → \(after.mac ?? "—")") }
        if vendorChanged { lines.append("Produttore: \(before.vendor ?? "—") → \(after.vendor ?? "—")") }
        if deviceTypeChanged { lines.append("Tipo: \(before.deviceType.label) → \(after.deviceType.label)") }
        if !openedPorts.isEmpty { lines.append("Porte aperte in più: \(ports(openedPorts, in: after))") }
        if !closedPorts.isEmpty { lines.append("Porte non più aperte: \(ports(closedPorts, in: before))") }
        if !addedHostnames.isEmpty { lines.append("Nuovi nomi: \(addedHostnames.joined(separator: ", "))") }
        if !removedHostnames.isEmpty { lines.append("Nomi scomparsi: \(removedHostnames.joined(separator: ", "))") }
        return lines
    }

    /// "443 (https), 8080" — the service name when the scan knew it.
    private func ports(_ numbers: [UInt16], in host: Host) -> String {
        numbers.map { n in
            if let service = host.openPorts.first(where: { $0.number == n })?.service {
                "\(n) (\(service))"
            } else {
                "\(n)"
            }
        }
        .joined(separator: ", ")
    }
}
