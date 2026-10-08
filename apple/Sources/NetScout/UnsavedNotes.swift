import AppKit
import SwiftUI

/// Asks what to do with notes not saved yet before the window closes or the
/// app quits.
@MainActor
enum UnsavedNotesPrompt {
    /// True when closing may go ahead: nothing unsaved, or the user saved or
    /// discarded the notes.
    static func allowsClosing(_ model: ScanModel) -> Bool {
        let names = model.unsavedProfileNames
        guard !names.isEmpty else { return true }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = names.count == 1
            ? "Salvare le note del profilo «\(names[0])»?"
            : "Salvare le note di \(names.count) profili?"
        alert.informativeText = (names.count == 1
            ? "Hai modificato delle note senza salvarle."
            : "Hai modificato delle note senza salvarle nei profili " + names.map { "«\($0)»" }.joined(separator: ", ") + ".")
            + " Se non le salvi, le modifiche andranno perse."
        alert.addButton(withTitle: "Salva")
        alert.addButton(withTitle: "Annulla")
        let discard = alert.addButton(withTitle: "Non salvare")
        discard.hasDestructiveAction = true
        switch alert.runModal() {
        case .alertFirstButtonReturn:
            return model.saveAllNotes()
        case .alertThirdButtonReturn:
            model.discardAllNotes()
            return true
        default:
            return false
        }
    }
}

/// Lets `shouldClose` veto closing the window (close button, ⌘W). SwiftUI
/// owns the window's delegate, so a proxy sits in front of it and forwards
/// everything else.
struct WindowCloseGuard: NSViewRepresentable {
    let shouldClose: @MainActor () -> Bool

    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        DispatchQueue.main.async { install(on: view.window, context: context) }
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        context.coordinator.shouldClose = shouldClose
        install(on: nsView.window, context: context)
    }

    func makeCoordinator() -> CloseGuardDelegate { CloseGuardDelegate(shouldClose: shouldClose) }

    private func install(on window: NSWindow?, context: Context) {
        guard let window, window.delegate !== context.coordinator else { return }
        context.coordinator.original = window.delegate
        window.delegate = context.coordinator
    }
}

final class CloseGuardDelegate: NSObject, NSWindowDelegate {
    weak var original: NSWindowDelegate?
    var shouldClose: @MainActor () -> Bool

    init(shouldClose: @escaping @MainActor () -> Bool) {
        self.shouldClose = shouldClose
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        guard MainActor.assumeIsolated({ shouldClose() }) else { return false }
        return original?.windowShouldClose?(sender) ?? true
    }

    override func responds(to aSelector: Selector!) -> Bool {
        super.responds(to: aSelector) || (original?.responds(to: aSelector) ?? false)
    }

    override func forwardingTarget(for aSelector: Selector!) -> Any? {
        original?.responds(to: aSelector) == true ? original : super.forwardingTarget(for: aSelector)
    }
}
