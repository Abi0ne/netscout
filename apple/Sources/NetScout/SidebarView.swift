import NetScoutCore
import SwiftUI

/// The devices found, by type (click a type to filter the table), what the
/// scan recognized and saved, and, fixed at the bottom, the scan target,
/// profile, start/stop and progress.
struct SidebarView: View {
    @Environment(ScanModel.self) private var model
    @Binding var typeFilter: DeviceType?
    @ViewState private var savingName: String?
    /// The sidebar's height: the scan panel turns compact when it is short,
    /// to leave room for the list.
    @ViewState private var height: CGFloat = 0

    /// Below this sidebar height the scan panel takes two rows.
    private static let compactBelow: CGFloat = 560
    private var compact: Bool { height > 0 && height < Self.compactBelow }

    var body: some View {
        List {
            if !model.hosts.isEmpty {
                Section {
                    TypeRow(label: "Tutti", symbol: "square.grid.2x2", count: model.hosts.count,
                            selected: typeFilter == nil) { typeFilter = nil }
                    ForEach(typeCounts, id: \.type) { entry in
                        TypeRow(label: entry.type.label, symbol: entry.type.symbol, count: entry.count,
                                selected: typeFilter == entry.type) {
                            typeFilter = typeFilter == entry.type ? nil : entry.type
                        }
                    }
                } header: {
                    // A little larger than the other headings, and apart from its list.
                    Text("Dispositivi")
                        .font(.system(size: 13, weight: .semibold))
                        .padding(.bottom, 6)
                }
            }

            if !model.hosts.isEmpty {
                notesSection
            }

            if let match = model.recognizedNetwork {
                Section("Rete riconosciuta") {
                    Button {
                        model.showProfile(id: match.profileId)
                    } label: {
                        Label {
                            VStack(alignment: .leading, spacing: 2) {
                                Text("Rete di «\(match.profileName)»")
                                if model.comparisonPending {
                                    Text("Confronto a fine scansione")
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                }
                            }
                        } icon: {
                            Image(systemName: "checkmark.seal.fill").foregroundStyle(.green)
                        }
                    }
                    .buttonStyle(.plain)
                    .help(match.explanation + " Clic per aprire il profilo.")
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
        }
        .overlay {
            if model.hosts.isEmpty {
                Text(model.isScanning ? "Ricerca dei dispositivi…" : "I dispositivi trovati compariranno qui, divisi per tipo.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .padding()
            }
        }
        // The scan controls stay at the bottom, whatever the list's length,
        // and shrink to two rows when the window is short.
        .safeAreaInset(edge: .bottom, spacing: 0) { scanPanel }
        .background {
            GeometryReader { geometry in
                Color.clear
                    .onAppear { height = geometry.size.height }
                    .onChange(of: geometry.size.height) { height = $1 }
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

    /// Where the scan's notes go and, when there are some, the notes not
    /// saved yet.
    @ViewBuilder
    private var notesSection: some View {
        Section("Note") {
            if let id = model.scanNotesProfileID, let profile = model.profiles.first(where: { $0.id == id }) {
                Label {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Nel profilo «\(profile.name)»")
                            .lineLimit(1)
                            .truncationMode(.middle)
                        Text("Passa sulla riga e clicca la matita, o scrivi nella scheda del dispositivo.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                } icon: {
                    Image(systemName: "pencil")
                        .foregroundStyle(.tint)
                }
                .help("Le note scritte su questa scansione vanno nel profilo «\(profile.name)»")
                if model.hasUnsavedNotes(profile: id) {
                    UnsavedNotesBar(profile: id, shortcut: true)
                }
            } else {
                Label {
                    Text(model.hasFinishedScan
                        ? "Per scrivere note, salva la scansione come profilo."
                        : "Le note si potranno scrivere a scansione finita, salvandola come profilo, o subito se la rete è già in un profilo.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                } icon: {
                    Image(systemName: "pencil.slash").foregroundStyle(.tertiary)
                }
            }
        }
    }

    /// What to scan and how, the start button and the progress.
    private var scanPanel: some View {
        Group {
            if compact { compactScanPanel } else { fullScanPanel }
        }
        .padding(compact ? 8 : 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
        .animation(.snappy, value: compact)
    }

    private var fullScanPanel: some View {
        @Bindable var model = model
        return VStack(alignment: .leading, spacing: 8) {
            Text("Rete").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
            if !model.networks.isEmpty {
                Picker("Interfaccia", selection: $model.target) {
                    interfaceOptions
                }
                .labelsHidden()
            }
            targetField
            profilePicker
            scanButton
                .controlSize(.large)
                .padding(.top, 2)
            if let p = model.progress {
                ProgressInfo(progress: p, running: model.isScanning)
            }
        }
    }

    /// The same controls in two rows: the interface goes in a menu beside the
    /// destination, the start button beside the profile.
    private var compactScanPanel: some View {
        @Bindable var model = model
        return VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 4) {
                targetField
                if !model.networks.isEmpty {
                    Menu {
                        Picker("Interfaccia", selection: $model.target) {
                            interfaceOptions
                        }
                        .pickerStyle(.inline)
                        .labelsHidden()
                    } label: {
                        Image(systemName: "network")
                    }
                    .menuStyle(.borderlessButton)
                    .fixedSize()
                    .help("Scegli l'interfaccia di rete")
                }
            }
            HStack(spacing: 6) {
                profilePicker
                scanButton
                    .fixedSize()
            }
            if let p = model.progress {
                ProgressInfo(progress: p, running: model.isScanning, compact: true)
            }
        }
    }

    @ViewBuilder
    private var interfaceOptions: some View {
        ForEach(model.networks, id: \.self) { net in
            Text("\(net.interface) · \(net.cidr)").tag(net.cidr)
        }
        if !model.networks.contains(where: { $0.cidr == model.target }) {
            Text("Personalizzata").tag(model.target)
        }
    }

    private var targetField: some View {
        @Bindable var model = model
        return TextField("Destinazione", text: $model.target, prompt: Text("192.168.1.0/24"))
            .textFieldStyle(.roundedBorder)
            .onSubmit { model.startScan() }
    }

    private var profilePicker: some View {
        @Bindable var model = model
        return Picker("Profilo", selection: $model.profile) {
            ForEach([ScanProfile.quick, .standard, .deep], id: \.self) { p in
                Text(p.label).tag(p)
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
    }

    private var scanButton: some View {
        Button {
            model.isScanning ? model.cancel() : model.startScan()
        } label: {
            Label(
                model.isScanning ? "Ferma" : (compact ? "Avvia" : "Avvia scansione"),
                systemImage: model.isScanning ? "stop.fill" : "play.fill"
            )
            .frame(maxWidth: compact ? nil : .infinity)
        }
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
    /// One line of figures, without the phase.
    var compact = false

    var body: some View {
        if compact {
            VStack(alignment: .leading, spacing: 4) {
                if running {
                    ProgressView(
                        value: Double(progress.scannedHosts),
                        total: Double(max(progress.totalHosts, 1))
                    )
                    .controlSize(.small)
                }
                Text("\(running ? progress.phase.label : "Completata") · \(progress.discoveredHosts)/\(progress.totalHosts) attivi · \(String(format: "%.1f", Double(progress.elapsedMs) / 1000)) s")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
        } else {
            full
        }
    }

    private var full: some View {
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

/// "Not saved", with Discard and Save, for profile `profile`'s notes.
struct UnsavedNotesBar: View {
    @Environment(ScanModel.self) private var model
    let profile: String
    /// Save answers ⌘S (only one bar on screen may have it).
    var shortcut = false

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "pencil.circle.fill").foregroundStyle(.orange)
            Text("Non salvate").foregroundStyle(.orange)
            Spacer()
            Button("Annulla") { model.discardNotes(profile: profile) }
                .controlSize(.small)
            Button("Salva") { model.saveNotes(profile: profile) }
                .keyboardShortcut(shortcut ? KeyboardShortcut("s") : nil)
                .controlSize(.small)
                .buttonStyle(.borderedProminent)
                .help("Salva le note nel profilo (⌘S)")
        }
        .font(.callout)
    }
}
