import NetScoutCore
import SwiftUI

/// Scan target, profile, start/stop, progress and the per-type breakdown
/// (click a type to filter the table).
struct SidebarView: View {
    @Environment(ScanModel.self) private var model
    @Binding var typeFilter: DeviceType?
    @ViewState private var savingName: String?

    var body: some View {
        @Bindable var model = model
        List {
            Section("Rete") {
                if !model.networks.isEmpty {
                    Picker("Interfaccia", selection: $model.target) {
                        ForEach(model.networks, id: \.self) { net in
                            Text("\(net.interface) · \(net.cidr)").tag(net.cidr)
                        }
                        if !model.networks.contains(where: { $0.cidr == model.target }) {
                            Text("Personalizzata").tag(model.target)
                        }
                    }
                    .labelsHidden()
                }
                TextField("Destinazione", text: $model.target, prompt: Text("192.168.1.0/24"))
                    .textFieldStyle(.roundedBorder)
                    .onSubmit { model.startScan() }
                Picker("Profilo", selection: $model.profile) {
                    ForEach([ScanProfile.quick, .standard, .deep], id: \.self) { p in
                        Text(p.label).tag(p)
                    }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
            }

            Section {
                scanButton
                if let p = model.progress {
                    ProgressInfo(progress: p, running: model.isScanning)
                }
            }

            if model.hasFinishedScan {
                Section("Risultato") {
                    Button {
                        savingName = model.suggestedProfileName
                    } label: {
                        Label("Salva come profilo…", systemImage: "square.and.arrow.down")
                    }
                    Menu {
                        ForEach(model.profiles, id: \.id) { profile in
                            Button(profile.name) { model.compareScan(withProfile: profile.id) }
                        }
                    } label: {
                        Label("Confronta con profilo", systemImage: "arrow.left.arrow.right")
                    }
                    .disabled(model.profiles.isEmpty)
                    .help(model.profiles.isEmpty ? "Nessun profilo salvato" : "Trova le differenze rispetto a un profilo salvato")
                }
            }

            if !model.hosts.isEmpty {
                Section("Dispositivi") {
                    TypeRow(label: "Tutti", symbol: "square.grid.2x2", count: model.hosts.count,
                            selected: typeFilter == nil) { typeFilter = nil }
                    ForEach(typeCounts, id: \.type) { entry in
                        TypeRow(label: entry.type.label, symbol: entry.type.symbol, count: entry.count,
                                selected: typeFilter == entry.type) {
                            typeFilter = typeFilter == entry.type ? nil : entry.type
                        }
                    }
                }
            }
        }
        .listStyle(.sidebar)
        .alert(
            "Salva come profilo",
            isPresented: Binding(get: { savingName != nil }, set: { if !$0 { savingName = nil } })
        ) {
            TextField("Nome", text: Binding(get: { savingName ?? "" }, set: { savingName = $0 }))
            Button("Salva") {
                if let name = savingName { model.saveScan(as: name) }
            }
            .disabled(savingName?.trimmingCharacters(in: .whitespaces).isEmpty ?? true)
            Button("Annulla", role: .cancel) {}
        } message: {
            Text("Salva i \(model.hosts.count) dispositivi trovati per confrontarli con le prossime scansioni.")
        }
    }

    private var scanButton: some View {
        Button {
            model.isScanning ? model.cancel() : model.startScan()
        } label: {
            Label(
                model.isScanning ? "Ferma" : "Avvia scansione",
                systemImage: model.isScanning ? "stop.fill" : "play.fill"
            )
            .frame(maxWidth: .infinity)
        }
        .controlSize(.large)
        .buttonStyle(.borderedProminent)
        .tint(model.isScanning ? .red : .accentColor)
    }

    private var typeCounts: [(type: DeviceType, count: Int)] {
        let counts = Dictionary(grouping: model.hosts.values) { (h: Host) in h.deviceType }.mapValues(\.count)
        return DeviceType.allCases.compactMap { t in counts[t].map { (t, $0) } }
            .sorted { $0.count > $1.count }
    }
}

private struct ProgressInfo: View {
    let progress: NetScoutCore.Progress
    let running: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if running {
                ProgressView(
                    value: Double(progress.scannedHosts),
                    total: Double(max(progress.totalHosts, 1))
                )
            }
            Text(running ? progress.phase.label : "Completata")
                .font(.caption.weight(.semibold))
            Text("\(progress.discoveredHosts) attivi su \(progress.totalHosts) · \(progress.openPortsFound) porte aperte · \(String(format: "%.1f", Double(progress.elapsedMs) / 1000)) s")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .padding(.vertical, 2)
    }
}

private struct TypeRow: View {
    let label: String
    let symbol: String
    let count: Int
    let selected: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack {
                Label(label, systemImage: symbol)
                Spacer()
                Text("\(count)")
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(.vertical, 2)
        .listRowBackground(selected ? Color.accentColor.opacity(0.18) : Color.clear)
    }
}
