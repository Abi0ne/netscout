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
                .background {
                    GeometryReader { geometry in
                        Color.clear
                            .onAppear { model.sidebarWidth = geometry.size.width }
                            .onChange(of: geometry.size.width) { model.sidebarWidth = $1 }
                            .onDisappear { model.sidebarWidth = 0 }
                    }
                }
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
                        // The same backdrop as the sidebar (and, see
                        // WindowTitle, in the toolbar above it too).
                        .background { SidebarBackdrop().ignoresSafeArea() }
                        .background {
                            GeometryReader { geometry in
                                Color.clear
                                    .onAppear { model.deviceCardWidth = geometry.size.width }
                                    .onChange(of: geometry.size.width) { model.deviceCardWidth = $1 }
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
        .background(WindowTitle(
            leadingSeparatorWidth: model.sidebarWidth,
            trailingBackdropWidth: model.selectedTab == .scan && model.showDeviceCard ? model.deviceCardWidth : 0
        ))
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
/// Over the device card, the title bar takes the sidebar's material, as it
/// does over the sidebar, so the card's column has one colour top to bottom.
/// Over the sidebar, the toolbar's bottom line (which the system draws only
/// on hover there) is always drawn, as over the other columns.
private struct WindowTitle: NSViewRepresentable {
    /// The sidebar's width, 0 when it is hidden.
    let leadingSeparatorWidth: CGFloat
    /// The device card's width, 0 when it is hidden.
    let trailingBackdropWidth: CGFloat

    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        DispatchQueue.main.async { context.coordinator.install(in: view.window) }
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        let width = trailingBackdropWidth
        let sidebar = leadingSeparatorWidth
        DispatchQueue.main.async {
            context.coordinator.install(in: nsView.window)
            context.coordinator.setBackdropWidth(width)
            context.coordinator.setSeparatorWidth(sidebar)
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    @MainActor
    final class Coordinator: NSObject {
        private var label: NSTextField?
        private var backdrop: NSView?
        private var backdropWidth: NSLayoutConstraint?
        private var separator: NSView?
        private var separatorWidth: NSLayoutConstraint?

        func setSeparatorWidth(_ width: CGFloat) {
            guard let separator, let separatorWidth else { return }
            separator.isHidden = width <= 0
            if separatorWidth.constant != width { separatorWidth.constant = width }
        }

        /// A line along the title bar's bottom edge, from the leading end.
        private func installSeparator(in titlebar: NSView) {
            let line = NSBox()
            line.boxType = .separator
            line.translatesAutoresizingMaskIntoConstraints = false
            titlebar.addSubview(line)
            let width = line.widthAnchor.constraint(equalToConstant: 0)
            NSLayoutConstraint.activate([
                line.leadingAnchor.constraint(equalTo: titlebar.leadingAnchor),
                line.bottomAnchor.constraint(equalTo: titlebar.bottomAnchor),
                line.heightAnchor.constraint(equalToConstant: 1),
                width,
            ])
            line.isHidden = true
            separator = line
            separatorWidth = width
        }

        func setBackdropWidth(_ width: CGFloat) {
            guard let backdrop, let backdropWidth else { return }
            backdrop.isHidden = width <= 0
            if backdropWidth.constant != width { backdropWidth.constant = width }
        }

        /// The sidebar's material at the title bar's trailing end, below the
        /// toolbar's controls, with the column's divider on its left.
        private func installBackdrop(in titlebar: NSView) {
            let effect = NSVisualEffectView()
            effect.material = .sidebar
            effect.blendingMode = .behindWindow
            effect.state = .followsWindowActiveState
            effect.translatesAutoresizingMaskIntoConstraints = false
            let divider = NSBox()
            divider.boxType = .separator
            divider.translatesAutoresizingMaskIntoConstraints = false
            effect.addSubview(divider)
            let toolbarView = titlebar.subviews.first { String(describing: type(of: $0)) == "NSToolbarView" }
            titlebar.addSubview(effect, positioned: .below, relativeTo: toolbarView)
            let width = effect.widthAnchor.constraint(equalToConstant: 0)
            NSLayoutConstraint.activate([
                effect.trailingAnchor.constraint(equalTo: titlebar.trailingAnchor),
                effect.topAnchor.constraint(equalTo: titlebar.topAnchor),
                effect.bottomAnchor.constraint(equalTo: titlebar.bottomAnchor),
                width,
                divider.leadingAnchor.constraint(equalTo: effect.leadingAnchor),
                divider.topAnchor.constraint(equalTo: effect.topAnchor),
                divider.bottomAnchor.constraint(equalTo: effect.bottomAnchor),
                divider.widthAnchor.constraint(equalToConstant: 1),
            ])
            effect.isHidden = true
            backdrop = effect
            backdropWidth = width
        }

        func install(in window: NSWindow?) {
            guard let window else { return }
            window.titleVisibility = .hidden
            if let label {
                updateVisibility(label)
                return
            }
            guard let titlebar = window.standardWindowButton(.closeButton)?.superview else { return }
            installBackdrop(in: titlebar)
            installSeparator(in: titlebar)
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

