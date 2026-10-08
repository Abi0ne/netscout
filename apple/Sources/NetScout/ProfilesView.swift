import AppKit
import NetScoutCore
import SwiftUI
import UniformTypeIdentifiers

// The saved-profiles tab: the list of profiles in the sidebar and, for the
// selected one, the devices it recorded with the user's notes.

/// The list of profiles (filtered by the search), with rename and delete.
struct ProfilesSidebar: View {
    @Environment(ScanModel.self) private var model
    @ViewState private var renaming: ProfileSummary?
    @ViewState private var newName = ""
    @ViewState private var deleting: ProfileSummary?

    var body: some View {
        @Bindable var model = model
        List(model.visibleProfiles, id: \.id, selection: $model.selectedProfileID) { profile in
            ProfileRow(profile: profile, unsaved: model.hasUnsavedNotes(profile: profile.id))
                .contextMenu {
                    Button("Rinomina…") {
                        newName = profile.name
                        renaming = profile
                    }
                    Button("Esporta CSV…") { exportProfiles([profile.id], name: profile.name, model: model) }
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
            } else if model.visibleProfiles.isEmpty {
                ContentUnavailableView.search(text: model.profileSearch)
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
            Button("Elimina", role: .destructive) { model.deleteProfile(id: profile.id) }
        } message: { profile in
            Text(model.hasUnsavedNotes(profile: profile.id)
                ? "Anche le note non salvate andranno perse. L'operazione non si può annullare."
                : "L'operazione non si può annullare.")
        }
    }
}

/// The selected profile, and the CSV export.
struct ProfilesDetail: View {
    @Environment(ScanModel.self) private var model

    var body: some View {
        Group {
            if let id = model.selectedProfileID, let summary = model.profiles.first(where: { $0.id == id }) {
                ProfileDetailView(summary: summary, query: model.deviceQuery(forProfile: id))
                    .id(summary.id)
            } else {
                ContentUnavailableView(
                    "Nessun profilo selezionato",
                    systemImage: "archivebox",
                    description: Text("Seleziona un profilo dalla lista.")
                )
            }
        }
        // Here rather than in ProfileDetailView, which is rebuilt for each
        // profile: an item that comes and goes makes the toolbar re-lay out
        // and its items jump.
        .toolbar {
            ToolbarItem {
                Button {
                    if let id = model.selectedProfileID { model.compareScan(withProfile: id) }
                } label: {
                    Label("Confronta con la scansione corrente", systemImage: "arrow.left.arrow.right")
                }
                .help(model.selectedProfileID == nil
                    ? "Seleziona un profilo"
                    : model.hasFinishedScan
                        ? "Trova le differenze tra la scansione corrente e questo profilo"
                        : "Esegui prima una scansione completa")
                .disabled(model.selectedProfileID == nil || !model.hasFinishedScan)
            }
            ToolbarItem {
                Menu {
                    if let id = model.selectedProfileID,
                       let profile = model.profiles.first(where: { $0.id == id }) {
                        Button("Profilo selezionato…") { exportProfiles([id], name: profile.name, model: model) }
                    }
                    if model.isSearchingProfiles {
                        Button("Profili trovati (\(model.visibleProfiles.count))…") {
                            exportProfiles(model.visibleProfiles.map(\.id), name: "profili", model: model)
                        }
                        .disabled(model.visibleProfiles.isEmpty)
                    }
                    Button("Tutti i profili (\(model.profiles.count))…") {
                        exportProfiles(model.profiles.map(\.id), name: "profili", model: model)
                    }
                } label: {
                    Label("Esporta CSV", systemImage: "square.and.arrow.up")
                }
                .help("Esporta i profili salvati in un file CSV, con le note")
                .disabled(model.profiles.isEmpty)
            }
        }
    }
}

extension ScanModel {
    var isSearchingProfiles: Bool { !profileSearch.trimmingCharacters(in: .whitespaces).isEmpty }

    /// The profiles matching the search.
    var visibleProfiles: [ProfileSummary] {
        guard isSearchingProfiles else { return profiles }
        return profiles.filter { profileMatch($0.id) != .none }
    }

    private func profileMatch(_ id: String) -> ProfileMatch {
        guard let profile = profileWithDrafts(id) else { return .none }
        return matchProfile(profile: profile, query: profileSearch)
    }

    /// The query that narrows the profile's devices: none when the profile
    /// itself matches (by name or network), so all its devices show.
    func deviceQuery(forProfile id: String) -> String {
        isSearchingProfiles && profileMatch(id) == .devices ? profileSearch : ""
    }
}

/// Ask where to save profiles `ids` as CSV, and save them.
@MainActor
private func exportProfiles(_ ids: [String], name: String, model: ScanModel) {
    let panel = NSSavePanel()
    panel.title = "Esporta profili in CSV"
    panel.allowedContentTypes = [.commaSeparatedText]
    panel.canCreateDirectories = true
    let date = Date().formatted(.iso8601.year().month().day())
    panel.nameFieldStringValue = "NetScout \(name) \(date).csv"
        .replacingOccurrences(of: "/", with: "-")
        .replacingOccurrences(of: ":", with: "-")
    guard panel.runModal() == .OK, let url = panel.url else { return }
    do {
        try model.csv(profiles: ids).write(to: url, atomically: true, encoding: .utf8)
    } catch {
        model.errorMessage = "Esportazione non riuscita: \(error.localizedDescription)"
    }
}

private struct ProfileRow: View {
    let profile: ProfileSummary
    let unsaved: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 6) {
                Text(profile.name).font(.body.weight(.medium))
                if unsaved {
                    Circle()
                        .fill(.orange)
                        .frame(width: 7, height: 7)
                        .help("Note modificate, non salvate")
                }
            }
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

/// One profile: what was scanned, when, and the devices found, with notes.
private struct ProfileDetailView: View {
    @Environment(ScanModel.self) private var model
    let summary: ProfileSummary
    /// Shows only the devices matching it, when not empty.
    let query: String
    @ViewState private var selection: String?

    private var profile: SavedProfile? { model.savedProfiles[summary.id] }
    private var unsaved: Bool { model.hasUnsavedNotes(profile: summary.id) }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .top, spacing: 12) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(summary.name).font(.title2.weight(.semibold))
                    Text("\(summary.target) · profilo \(summary.scanProfile.label.lowercased()) · \(summary.hostCount) dispositivi · salvato \(summary.createdAtDate.formatted(date: .abbreviated, time: .shortened))\(summary.updatedAtDate.map { ", aggiornato \($0.formatted(date: .abbreviated, time: .shortened))" } ?? "")")
                        .foregroundStyle(.secondary)
                }
                Spacer()
                if unsaved {
                    Label("Note non salvate", systemImage: "pencil.circle.fill")
                        .font(.callout)
                        .foregroundStyle(.orange)
                        .padding(.top, 4)
                    Button("Annulla modifiche") { model.discardNotes(profile: summary.id) }
                    Button("Salva note") { model.saveNotes(profile: summary.id) }
                        .keyboardShortcut("s")
                        .buttonStyle(.borderedProminent)
                }
            }
            .padding(16)
            Divider()
            HostTableView(
                hosts: filtered(profile?.hosts ?? []),
                offline: filtered(profile?.offlineHosts ?? []),
                selection: $selection,
                notes: NoteEditing(
                    note: { model.note(profile: summary.id, host: $0) },
                    setNote: { model.setNote($0, profile: summary.id, host: $1) }
                )
            )
        }
    }

    private func filtered(_ hosts: [Host]) -> [Host] {
        guard !query.isEmpty else { return hosts }
        return hosts.filter {
            hostMatches(host: $0, note: model.note(profile: summary.id, host: $0), query: query)
        }
    }
}

extension ProfileSummary {
    var createdAtDate: Date { Date(timeIntervalSince1970: Double(createdAt) / 1000) }
    var updatedAtDate: Date? { updatedAt.map { Date(timeIntervalSince1970: Double($0) / 1000) } }
}
