import AppKit
import SwiftUI

@main
struct NetScoutApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @ViewState private var model = ScanModel()
    @ViewState private var updater = Updater()

    var body: some Scene {
        WindowGroup("NetScout") {
            RootView()
                .environment(model)
                .environment(updater)
                .frame(minWidth: 900, minHeight: 520)
                .task {
                    updater.isBusy = { [model] in model.isScanning }
                    await updater.runPeriodicChecks()
                }
        }
        .defaultSize(width: 1440, height: 820)
        .commands {
            CommandGroup(after: .appInfo) {
                Button("Controlla aggiornamenti…") {
                    Task { await updater.check() }
                }
            }
            CommandGroup(after: .newItem) {
                Button(model.isScanning ? "Ferma scansione" : "Avvia scansione") {
                    model.isScanning ? model.cancel() : model.startScan()
                }
                .keyboardShortcut("r")
            }
            CommandGroup(replacing: .help) {
                HelpMenuItem()
            }
        }

        Settings {
            SettingsView()
                .environment(updater)
        }

        Window("Guida di NetScout", id: HelpMenuItem.windowID) {
            HelpView()
        }
        .defaultSize(width: 900, height: 640)
    }
}

/// Aiuto → Guida di NetScout (⌘?).
private struct HelpMenuItem: View {
    static let windowID = "help"
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Button("Guida di NetScout") { openWindow(id: Self.windowID) }
            .keyboardShortcut("?", modifiers: .command)
    }
}

/// Makes the app a regular foreground app even when launched as a bare
/// executable (`swift run`), and quits when the last window closes.
final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        NSApp.activate()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }
}
