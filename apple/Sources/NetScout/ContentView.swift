import NetScoutCore
import SwiftUI

/// The window: the scan and the saved profiles, as tabs. A comparison opens
/// as a sheet over either.
struct RootView: View {
    @Environment(ScanModel.self) private var model

    var body: some View {
        @Bindable var model = model
        TabView {
            ContentView()
                .tabItem { Label("Scansione", systemImage: "dot.radiowaves.left.and.right") }
            ProfilesView()
                .tabItem { Label("Profili salvati", systemImage: "archivebox") }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) { StatusBar() }
        .sheet(item: $model.comparison) { comparison in
            ComparisonView(comparison: comparison)
        }
        .alert(
            "Errore",
            isPresented: Binding(
                get: { model.errorMessage != nil },
                set: { if !$0 { model.errorMessage = nil } }
            ),
            presenting: model.errorMessage
        ) { _ in
            Button("OK") {}
        } message: { message in
            Text(message)
        }
    }
}

/// The scan tab.
struct ContentView: View {
    @Environment(ScanModel.self) private var model
    @ViewState private var selection: String?
    @ViewState private var typeFilter: DeviceType?
    @ViewState private var search = ""

    var body: some View {
        NavigationSplitView {
            SidebarView(typeFilter: $typeFilter)
                .navigationSplitViewColumnWidth(min: 240, ideal: 270, max: 340)
        } content: {
            HostTableView(hosts: visibleHosts, selection: $selection)
                .navigationSplitViewColumnWidth(min: 520, ideal: 680)
                .searchable(text: $search, placement: .toolbar, prompt: "IP, nome, produttore, MAC")
                .overlay { emptyState }
        } detail: {
            if let ip = selection, let host = model.hosts[ip] {
                HostDetailView(host: host)
            } else {
                ContentUnavailableView(
                    "Nessun dispositivo selezionato",
                    systemImage: "network",
                    description: Text("Seleziona un dispositivo dalla lista.")
                )
            }
        }
        .navigationTitle("NetScout")
    }

    private var visibleHosts: [Host] {
        let query = search.trimmingCharacters(in: .whitespaces).lowercased()
        return model.sortedHosts.filter { host in
            if let typeFilter, host.deviceType != typeFilter { return false }
            guard !query.isEmpty else { return true }
            let fields = [host.ip, host.mac ?? "", host.vendor ?? ""] + host.hostnames
            return fields.contains { $0.lowercased().contains(query) }
        }
    }

    @ViewBuilder
    private var emptyState: some View {
        if model.hosts.isEmpty {
            if model.isScanning {
                ProgressView("Scansione in corso…")
            } else {
                ContentUnavailableView(
                    "Nessuna scansione",
                    systemImage: "dot.radiowaves.left.and.right",
                    description: Text("Scegli la rete e premi Avvia (⌘R).")
                )
            }
        } else if visibleHosts.isEmpty {
            ContentUnavailableView.search(text: search)
        }
    }
}
