import AppKit
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
        // The app name sits in the middle of the toolbar instead of the
        // standard title on the left.
        .toolbar {
            ToolbarItem(placement: .principal) {
                Text("NetScout").font(.headline)
            }
        }
        .background(WindowTitleHider())
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

/// Hides the window's own title (it stays the window's name in the Window
/// menu and Mission Control) so only the centred one shows.
private struct WindowTitleHider: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        DispatchQueue.main.async { view.window?.titleVisibility = .hidden }
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        nsView.window?.titleVisibility = .hidden
    }
}

/// The scan tab.
struct ContentView: View {
    @Environment(ScanModel.self) private var model
    @ViewState private var selection: String?
    @ViewState private var typeFilter: DeviceType?
    @ViewState private var search = ""
    @ViewState private var showDetail = true

    var body: some View {
        NavigationSplitView {
            SidebarView(typeFilter: $typeFilter)
                .navigationSplitViewColumnWidth(min: 240, ideal: 270, max: 340)
        } detail: {
            // The scan table takes all the width; the device card is a narrow
            // inspector on the right.
            HostTableView(hosts: visibleHosts, offline: visibleOffline, selection: $selection) { id in
                if selection == id { selection = nil }
                model.forgetOffline(id: id)
            }
            .searchable(text: $search, placement: .toolbar, prompt: "IP, nome, produttore, MAC")
            .overlay { emptyState }
            .inspector(isPresented: $showDetail) {
                hostDetail
                    .inspectorColumnWidth(min: 280, ideal: 320, max: 440)
            }
            .toolbar {
                ToolbarItem(placement: .primaryAction) {
                    Button {
                        showDetail.toggle()
                    } label: {
                        Label("Scheda dispositivo", systemImage: "sidebar.right")
                    }
                    .help(showDetail ? "Nascondi la scheda del dispositivo" : "Mostra la scheda del dispositivo")
                }
            }
        }
        .navigationTitle("NetScout")
    }

    @ViewBuilder
    private var hostDetail: some View {
        if let id = selection, let host = model.hosts[id] {
            HostDetailView(host: host)
        } else if let id = selection, let host = model.offlineHosts[id] {
            HostDetailView(host: host, offline: true)
        } else {
            ContentUnavailableView(
                "Nessun dispositivo selezionato",
                systemImage: "network",
                description: Text("Seleziona un dispositivo dalla lista.")
            )
        }
    }

    private var visibleHosts: [Host] { model.sortedHosts.filter(matches) }
    private var visibleOffline: [Host] { model.sortedOfflineHosts.filter(matches) }

    private func matches(_ host: Host) -> Bool {
        if let typeFilter, host.deviceType != typeFilter { return false }
        let query = search.trimmingCharacters(in: .whitespaces).lowercased()
        guard !query.isEmpty else { return true }
        let fields = [host.ip, host.mac ?? "", host.vendor ?? ""] + host.hostnames
        return fields.contains { $0.lowercased().contains(query) }
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
        } else if visibleHosts.isEmpty && visibleOffline.isEmpty {
            ContentUnavailableView.search(text: search)
        }
    }
}
