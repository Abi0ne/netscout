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
                .task { await updater.check(quiet: true) }
        }
        .defaultSize(width: 1200, height: 720)
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
        }
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
