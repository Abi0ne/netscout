import NetScoutCore
import SwiftUI

/// The differences between the finished scan and a saved profile.
struct ComparisonView: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(ScanModel.self) private var model
    let comparison: ProfileComparison
    @ViewState private var offlineAdded = false
    @ViewState private var confirmUpdate = false

    private var diff: ScanDiff { comparison.diff }
    private var cameBack: [HostChange] { diff.changed.filter(\.cameBack) }
    private var moved: [HostChange] { diff.changed.filter { !$0.cameBack && $0.ipChanged } }
    private var otherChanges: [HostChange] { diff.changed.filter { !$0.cameBack && !$0.ipChanged } }
    private var hasDifferences: Bool {
        !diff.added.isEmpty || !diff.removed.isEmpty || !diff.stillOffline.isEmpty
            || !diff.changed.isEmpty || !diff.replaced.isEmpty
    }

    /// Devices of the profile not found now: gone off, still off, or whose
    /// address another device now uses.
    private var offlineDevices: [Host] {
        diff.removed + diff.stillOffline + diff.replaced.map(\.before)
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
                        Section("Spenti o scomparsi (\(diff.removed.count))") {
                            ForEach(diff.removed, id: \.ip) { host in
                                OfflineLine(host: host)
                            }
                        }
                    }
                    if !diff.stillOffline.isEmpty {
                        Section("Ancora spenti (\(diff.stillOffline.count))") {
                            ForEach(diff.stillOffline, id: \.ip) { host in
                                OfflineLine(host: host)
                            }
                        }
                    }
                    if !cameBack.isEmpty {
                        changeSection("Di nuovo accesi", cameBack, mark: "power.circle.fill", tint: .green)
                    }
                    if !moved.isEmpty {
                        changeSection("Stesso MAC, IP diverso", moved, mark: "arrow.triangle.swap", tint: .blue)
                    }
                    if !diff.replaced.isEmpty {
                        Section("Stesso IP, MAC diverso (\(diff.replaced.count))") {
                            ForEach(diff.replaced, id: \.after.ip) { change in
                                VStack(alignment: .leading, spacing: 6) {
                                    HostLine(host: change.after, mark: "exclamationmark.triangle.fill", tint: .orange)
                                    Text("Prima a questo indirizzo c'era un altro dispositivo, ora non trovato:")
                                        .font(.callout)
                                        .foregroundStyle(.secondary)
                                        .padding(.leading, 30)
                                    OfflineLine(host: change.before).padding(.leading, 30)
                                }
                                .padding(.vertical, 2)
                            }
                        }
                    }
                    if !otherChanges.isEmpty {
                        changeSection("Altre modifiche", otherChanges, mark: "pencil.circle.fill", tint: .orange)
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
            footer.padding(12)
        }
        .frame(minWidth: 720, idealWidth: 800, minHeight: 520, idealHeight: 640)
        .confirmationDialog(
            "Aggiornare il profilo «\(comparison.profile.name)»?",
            isPresented: $confirmUpdate
        ) {
            Button("Aggiorna profilo") {
                model.addOffline(offlineDevices)
                if model.updateProfile(id: comparison.profile.id) { dismiss() }
            }
        } message: {
            Text("Il profilo verrà sostituito da questa scansione: \(model.hosts.count) dispositivi accesi e \(offlineCount) spenti, che restano nel profilo per il Wake-on-LAN.")
        }
    }

    /// Devices off after merging this comparison's ones into the scan.
    private var offlineCount: Int {
        Set(model.offlineHosts.keys).union(offlineDevices.map(ScanModel.offlineID)).count
    }

    private var footer: some View {
        HStack(spacing: 10) {
            if !offlineDevices.isEmpty {
                Button {
                    model.addOffline(offlineDevices)
                    offlineAdded = true
                } label: {
                    Label(offlineAdded ? "Spenti aggiunti alla scansione" : "Aggiungi gli spenti alla scansione (\(offlineDevices.count))",
                          systemImage: offlineAdded ? "checkmark" : "plus")
                }
                .disabled(offlineAdded)
                .help("Mostra nella scansione i dispositivi non trovati, per accenderli con il Wake-on-LAN")
            }
            Button {
                confirmUpdate = true
            } label: {
                Label("Aggiorna il profilo…", systemImage: "arrow.triangle.2.circlepath")
            }
            .help("Salva questa scansione nel profilo, compresi i dispositivi spenti")
            Spacer()
            Button("Chiudi") { dismiss() }
                .keyboardShortcut(.defaultAction)
        }
    }

    private func changeSection(_ title: String, _ changes: [HostChange], mark: String, tint: Color) -> some View {
        Section("\(title) (\(changes.count))") {
            ForEach(changes, id: \.after.ip) { change in
                VStack(alignment: .leading, spacing: 4) {
                    HostLine(host: change.after, mark: mark, tint: tint)
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

    private var header: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Confronto con «\(comparison.profile.name)»")
                .font(.title2.weight(.semibold))
            Text("Profilo salvato \(comparison.profile.createdAtDate.formatted(date: .abbreviated, time: .shortened)) · \(comparison.profile.target)")
                .foregroundStyle(.secondary)
            HStack(spacing: 8) {
                CountBadge(count: diff.added.count, label: "nuovi", tint: .green)
                CountBadge(count: diff.removed.count + diff.stillOffline.count, label: "spenti", tint: .red)
                CountBadge(count: moved.count, label: "IP cambiato", tint: .blue)
                CountBadge(count: diff.replaced.count, label: "MAC cambiato", tint: .orange)
                CountBadge(count: cameBack.count + otherChanges.count, label: "modificati", tint: .orange)
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

/// A device not found now, with its Wake-on-LAN button.
private struct OfflineLine: View {
    let host: Host

    var body: some View {
        HStack {
            HostLine(host: host, mark: "power.circle", tint: .red)
            Spacer()
            WakeButton(host: host, compact: true)
        }
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
        if cameBack { lines.append("Era spento quando è stato salvato il profilo") }
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
