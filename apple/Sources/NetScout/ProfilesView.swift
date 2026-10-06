import NetScoutCore
import SwiftUI

/// The saved-profiles tab: the list of profiles and, for the selected one, the
/// devices it recorded.
struct ProfilesView: View {
    @Environment(ScanModel.self) private var model
    @ViewState private var selection: String?
    @ViewState private var renaming: ProfileSummary?
    @ViewState private var newName = ""
    @ViewState private var deleting: ProfileSummary?

    var body: some View {
        NavigationSplitView {
            List(model.profiles, id: \.id, selection: $selection) { profile in
                ProfileRow(profile: profile)
                    .contextMenu {
                        Button("Rinomina…") {
                            newName = profile.name
                            renaming = profile
                        }
                        Button("Elimina…", role: .destructive) { deleting = profile }
                    }
            }
            .overlay {
                if model.profiles.isEmpty {
                    ContentUnavailableView(
                        "Nessun profilo",
                        systemImage: "archivebox",
                        description: Text("A scansione finita, usa «Salva come profilo…».")
                    )
                }
            }
            .navigationSplitViewColumnWidth(min: 240, ideal: 280, max: 360)
        } detail: {
            if let id = selection, let summary = model.profiles.first(where: { $0.id == id }) {
                ProfileDetailView(summary: summary)
                    .id(summary.id)
            } else {
                ContentUnavailableView(
                    "Nessun profilo selezionato",
                    systemImage: "archivebox",
                    description: Text("Seleziona un profilo dalla lista.")
                )
            }
        }
        .onAppear { model.refreshProfiles() }
        .alert(
            "Rinomina profilo",
            isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } }),
            presenting: renaming
        ) { profile in
            TextField("Nome", text: $newName)
            Button("Rinomina") { model.renameProfile(id: profile.id, to: newName) }
                .disabled(newName.trimmingCharacters(in: .whitespaces).isEmpty)
            Button("Annulla", role: .cancel) {}
        }
        .confirmationDialog(
            "Eliminare il profilo «\(deleting?.name ?? "")»?",
            isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }),
            presenting: deleting
        ) { profile in
            Button("Elimina", role: .destructive) {
                if selection == profile.id { selection = nil }
                model.deleteProfile(id: profile.id)
            }
        } message: { _ in
            Text("L'operazione non si può annullare.")
        }
    }
}

private struct ProfileRow: View {
    let profile: ProfileSummary

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(profile.name).font(.body.weight(.medium))
            Text(profile.createdAtDate.formatted(date: .abbreviated, time: .shortened))
                .font(.caption)
                .foregroundStyle(.secondary)
            Text("\(profile.target) · \(profile.hostCount) dispositivi\(profile.offlineCount > 0 ? " · \(profile.offlineCount) spenti" : "")")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .padding(.vertical, 2)
    }
}

/// One profile: what was scanned, when, and the devices found.
private struct ProfileDetailView: View {
    @Environment(ScanModel.self) private var model
    let summary: ProfileSummary
    @ViewState private var profile: SavedProfile?
    @ViewState private var selection: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 4) {
                Text(summary.name).font(.title2.weight(.semibold))
                Text("\(summary.target) · profilo \(summary.scanProfile.label.lowercased()) · \(summary.hostCount) dispositivi · salvato \(summary.createdAtDate.formatted(date: .abbreviated, time: .shortened))\(summary.updatedAtDate.map { ", aggiornato \($0.formatted(date: .abbreviated, time: .shortened))" } ?? "")")
                    .foregroundStyle(.secondary)
            }
            .padding(16)
            Divider()
            HostTableView(hosts: profile?.hosts ?? [], offline: profile?.offlineHosts ?? [], selection: $selection)
        }
        .toolbar {
            ToolbarItem {
                Button {
                    model.compareScan(withProfile: summary.id)
                } label: {
                    Label("Confronta con la scansione corrente", systemImage: "arrow.left.arrow.right")
                }
                .help(model.hasFinishedScan
                    ? "Trova le differenze tra la scansione corrente e questo profilo"
                    : "Esegui prima una scansione completa")
                .disabled(!model.hasFinishedScan)
            }
        }
        .task(id: summary.id) { profile = model.loadProfile(id: summary.id) }
    }
}

extension ProfileSummary {
    var createdAtDate: Date { Date(timeIntervalSince1970: Double(createdAt) / 1000) }
    var updatedAtDate: Date? { updatedAt.map { Date(timeIntervalSince1970: Double($0) / 1000) } }
}
