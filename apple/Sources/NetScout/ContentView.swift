import AppKit
import NetScoutCore
import SwiftUI

/// The window: the scan and the saved profiles, switched by the tabs at the
/// start of the toolbar. A comparison opens as a sheet over either.
struct RootView: View {
    @Environment(ScanModel.self) private var model

    var body: some View {
        @Bindable var model = model
        // One split view for both tabs, so the toolbar keeps its layout when
        // switching: only the panes' contents change. The status bar goes
        // below it, not over it (a safe-area inset is ignored by the columns,
        // which would run under the bar).
        VStack(spacing: 0) {
            NavigationSplitView {
                Group {
                    switch model.selectedTab {
                    case .scan: SidebarView(typeFilter: $model.typeFilter)
                    case .profiles: ProfilesSidebar()
                    }
                }
                .navigationSplitViewColumnWidth(min: 240, ideal: 270, max: 360)
            } detail: {
                Group {
                    switch model.selectedTab {
                    case .scan: ScanTable()
                    case .profiles: ProfilesDetail()
                    }
                }
                // The device card: a narrow inspector on the right of the scan.
                .inspector(isPresented: Binding(
                    get: { model.selectedTab == .scan && model.showDeviceCard },
                    set: { if model.selectedTab == .scan { model.showDeviceCard = $0 } }
                )) {
                    DeviceCard()
                        .inspectorColumnWidth(min: 280, ideal: 320, max: 440)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        // The same backdrop as the sidebar and, see
                        // WindowTitle, the whole title bar.
                        .background { SidebarBackdrop().ignoresSafeArea() }
                        .background {
                            GeometryReader { geometry in
                                Color.clear
                                    .onAppear { setDeviceCardWidth(geometry.size.width) }
                                    .onChange(of: geometry.size.width) { setDeviceCardWidth($1) }
                            }
                        }
                }
            }
            StatusBar()
        }
        .toolbar {
            // Right after the sidebar, where the middle pane starts.
            ToolbarItem(placement: .navigation) {
                TabSwitcher(selection: $model.selectedTab)
            }
            // One search field, in the same place, filters whichever tab is on
            // screen. On the scan tab it sits right above the device card, as
            // wide as it, and the card's show/hide button closes the toolbar.
            ToolbarItem(placement: .automatic) {
                HStack(spacing: 8) {
                    ToolbarSearchField(
                        text: model.selectedTab == .scan ? $model.scanSearch : $model.profileSearch,
                        prompt: model.selectedTab == .scan ? "IP, nome, produttore, MAC" : "Profilo, rete, IP, nome, MAC, note"
                    )
                    .frame(width: searchWidth)
                    if model.selectedTab == .scan {
                        Button {
                            withAnimation { model.showDeviceCard.toggle() }
                        } label: {
                            Label("Scheda dispositivo", systemImage: "sidebar.right")
                        }
                        .help(model.showDeviceCard ? "Nascondi la scheda del dispositivo" : "Mostra la scheda del dispositivo")
                    }
                }
            }
        }
        .hidingToolbarTitle()
        .background(WindowTitle())
        .background(WindowCloseGuard { UnsavedNotesPrompt.allowsClosing(model) })
        .sheet(item: $model.comparison) { comparison in
            ComparisonView(comparison: comparison)
        }
        .alert(
            "Rete riconosciuta",
            isPresented: Binding(
                get: { model.networkSuggestion != nil && model.errorMessage == nil },
                set: { if !$0 { model.networkSuggestion = nil } }
            ),
            presenting: model.networkSuggestion
        ) { match in
            Button(model.isScanning ? "Confronta a fine scansione" : "Confronta") {
                model.compareWhenFinished(profile: match.profileId)
            }
            Button("Mostra profilo") { model.showProfile(id: match.profileId) }
            Button("Ignora", role: .cancel) {}
        } message: { match in
            Text(match.explanation)
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

    /// On the scan tab, the device card's width less the button beside the
    /// field; otherwise a usual search field width.
    private var searchWidth: CGFloat {
        guard model.selectedTab == .scan, model.showDeviceCard, model.deviceCardWidth > 0 else { return 240 }
        return max(160, model.deviceCardWidth - Self.cardButtonWidth)
    }

    /// The card's width sizes the search field in the toolbar, which can move
    /// the card again: written after the current layout pass, in whole points
    /// and only on a real change, so the two never chase each other within one
    /// pass (AppKit aborts after too many constraint passes in a row).
    private func setDeviceCardWidth(_ width: CGFloat) {
        let width = width.rounded()
        DispatchQueue.main.async {
            if abs(model.deviceCardWidth - width) >= 1 { model.deviceCardWidth = width }
        }
    }

    /// The card button and the gaps around it.
    private static let cardButtonWidth: CGFloat = 52
}

extension NetworkMatch {
    /// Why the network was recognized, for the user.
    var explanation: String {
        let others = Int(matchedDevices) - (gatewayMatched ? 1 : 0)
        let evidence: String
        if gatewayMatched {
            evidence = others > 0
                ? "c'è lo stesso router (stesso MAC) e altri \(others) dispositivi del profilo"
                : "c'è lo stesso router (stesso MAC)"
        } else {
            evidence = "ci sono \(matchedDevices) dei \(profileDevices) dispositivi del profilo, riconosciuti dal MAC"
        }
        return "Questa sembra la rete del profilo «\(profileName)»: \(evidence)."
    }
}

/// The app's name in the middle of the window's top edge, whatever the panes'
/// widths. The system title would sit at the start of the toolbar, so it is
/// hidden (it stays the window's name in the Window menu and Mission Control)
/// and a label is laid over the title bar instead, hidden while the toolbar's
/// controls would cover it in a narrow window.
///
/// The title bar is one piece, the sidebar's material from edge to edge: no
/// line along its bottom, no column dividers through it, and no darker band
/// over the middle pane.
private struct WindowTitle: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        DispatchQueue.main.async { context.coordinator.install(in: view.window) }
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        DispatchQueue.main.async { context.coordinator.install(in: nsView.window) }
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    @MainActor
    final class Coordinator: NSObject {
        private var label: NSTextField?

        /// The sidebar's material across the whole title bar, below the
        /// toolbar's controls.
        private func installBackdrop(in titlebar: NSView) {
            let effect = NSVisualEffectView()
            effect.material = .sidebar
            effect.blendingMode = .behindWindow
            effect.state = .followsWindowActiveState
            effect.translatesAutoresizingMaskIntoConstraints = false
            let toolbarView = titlebar.subviews.first { String(describing: type(of: $0)) == "NSToolbarView" }
            titlebar.addSubview(effect, positioned: .below, relativeTo: toolbarView)
            NSLayoutConstraint.activate([
                effect.leadingAnchor.constraint(equalTo: titlebar.leadingAnchor),
                effect.trailingAnchor.constraint(equalTo: titlebar.trailingAnchor),
                effect.topAnchor.constraint(equalTo: titlebar.topAnchor),
                effect.bottomAnchor.constraint(equalTo: titlebar.bottomAnchor),
            ])
        }

        /// The view the system draws the title bar's lines in (along the
        /// bottom and through it at the sidebar's edge), above everything else.
        private func hideDecorations(of titlebar: NSView) {
            titlebar.superview?.subviews
                .filter { String(describing: type(of: $0)) == "_NSTitlebarDecorationView" }
                .forEach { $0.isHidden = true }
        }

        func install(in window: NSWindow?) {
            guard let window else { return }
            window.titleVisibility = .hidden
            if let label {
                if let titlebar = label.superview { hideDecorations(of: titlebar) }
                updateVisibility(label)
                return
            }
            guard let titlebar = window.standardWindowButton(.closeButton)?.superview else { return }
            window.titlebarSeparatorStyle = .none
            installBackdrop(in: titlebar)
            hideDecorations(of: titlebar)
            let label = NSTextField(labelWithString: window.title.isEmpty ? "NetScout" : window.title)
            label.font = .systemFont(ofSize: NSFont.systemFontSize, weight: .semibold)
            label.textColor = .secondaryLabelColor
            label.isSelectable = false
            label.setAccessibilityIdentifier("windowTitle")
            label.translatesAutoresizingMaskIntoConstraints = false
            titlebar.addSubview(label)
            NSLayoutConstraint.activate([
                label.centerXAnchor.constraint(equalTo: titlebar.centerXAnchor),
                label.centerYAnchor.constraint(equalTo: titlebar.centerYAnchor),
            ])
            self.label = label
            titlebar.postsFrameChangedNotifications = true
            NotificationCenter.default.addObserver(
                self, selector: #selector(titlebarChanged), name: NSView.frameDidChangeNotification, object: titlebar)
            NotificationCenter.default.addObserver(
                self, selector: #selector(titlebarChanged), name: NSWindow.didResizeNotification, object: window)
            DispatchQueue.main.async { self.updateVisibility(label) }
        }

        @objc private func titlebarChanged() {
            guard let label else { return }
            DispatchQueue.main.async { self.updateVisibility(label) }
        }

        /// Hidden while any toolbar control overlaps it, with some margin.
        private func updateVisibility(_ label: NSTextField) {
            guard let window = label.window, let toolbar = window.toolbar else { return }
            label.superview?.layoutSubtreeIfNeeded()
            let frame = label.convert(label.bounds, to: nil).insetBy(dx: -12, dy: 0)
            let covered = toolbar.items.contains { item in
                guard item.itemIdentifier != .flexibleSpace, item.itemIdentifier != .space,
                      let view = item.view, view.window == window, !view.isHiddenOrHasHiddenAncestor else { return false }
                return frame.intersects(view.convert(view.bounds, to: nil))
            }
            label.isHidden = covered
        }
    }
}

extension View {
    /// Without the title SwiftUI puts after the toolbar's leading items: the
    /// window's name is the label in the middle of the title bar.
    @ViewBuilder
    fileprivate func hidingToolbarTitle() -> some View {
        if #available(macOS 15, *) {
            toolbar(removing: .title)
        } else {
            self
        }
    }
}

/// The sidebar's material, for the device card on the other side.
private struct SidebarBackdrop: NSViewRepresentable {
    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = .sidebar
        view.blendingMode = .behindWindow
        view.state = .followsWindowActiveState
        return view
    }

    func updateNSView(_ nsView: NSVisualEffectView, context: Context) {}
}

/// The scan tab's table: all the width between the sidebar and the card.
struct ScanTable: View {
    @Environment(ScanModel.self) private var model

    var body: some View {
        @Bindable var model = model
        HostTableView(
            hosts: visibleHosts,
            offline: visibleOffline,
            selection: $model.scanSelection,
            forgetOffline: { id in
                if model.scanSelection == id { model.scanSelection = nil }
                model.forgetOffline(id: id)
            },
            notes: notes
        )
        .overlay { emptyState }
    }

    private var visibleHosts: [Host] { model.sortedHosts.filter(matches) }
    private var visibleOffline: [Host] { model.sortedOfflineHosts.filter(matches) }

    private func matches(_ host: Host) -> Bool {
        if let typeFilter = model.typeFilter, host.deviceType != typeFilter { return false }
        let query = model.scanSearch.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return true }
        return hostMatches(host: host, note: notes.note(host), query: query)
    }

    /// The notes of the profile the scan belongs to (saved as, or recognized
    /// as); without one, the column only shows dashes.
    private var notes: NoteEditing {
        guard let id = model.scanNotesProfileID else {
            return NoteEditing(note: { _ in "" }, setNote: { _, _ in }, editable: false)
        }
        return NoteEditing(
            note: { model.note(profile: id, host: $0) },
            setNote: { model.setNote($0, profile: id, host: $1) }
        )
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
            ContentUnavailableView.search(text: model.scanSearch)
        }
    }
}

/// The scan tab's card of the selected device.
struct DeviceCard: View {
    @Environment(ScanModel.self) private var model

    var body: some View {
        if let id = model.scanSelection, let host = model.hosts[id] {
            HostDetailView(host: host)
        } else if let id = model.scanSelection, let host = model.offlineHosts[id] {
            HostDetailView(host: host, offline: true)
        } else {
            ContentUnavailableView(
                "Nessun dispositivo selezionato",
                systemImage: "network",
                description: Text("Seleziona un dispositivo dalla lista.")
            )
        }
    }
}

