import AppKit
import SwiftUI

/// The window's search field. Unlike `.searchable`, its width is ours to set
/// (it follows the device card below it) and it never collapses into a
/// button when the window is narrow.
struct ToolbarSearchField: NSViewRepresentable {
    @Binding var text: String
    let prompt: String

    func makeNSView(context: Context) -> NSSearchField {
        let field = NSSearchField()
        field.delegate = context.coordinator
        field.sendsSearchStringImmediately = true
        field.target = context.coordinator
        field.action = #selector(Coordinator.changed(_:))
        field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        field.setAccessibilityLabel("Cerca")
        return field
    }

    func updateNSView(_ field: NSSearchField, context: Context) {
        let coordinator = context.coordinator
        DispatchQueue.main.async { coordinator.keepAtTrailingEdge(field) }
        coordinator.text = $text
        if field.stringValue != text { field.stringValue = text }
        field.placeholderString = prompt
        field.toolTip = "Cerca: \(prompt)"
    }

    func makeCoordinator() -> Coordinator { Coordinator(text: $text) }

    final class Coordinator: NSObject, NSSearchFieldDelegate {
        var text: Binding<String>
        private weak var field: NSView?
        private weak var observed: NSToolbar?

        init(text: Binding<String>) { self.text = text }

        /// SwiftUI lines the items up after the tabs; a flexible space before
        /// the field's item moves it, and what follows, to the right edge of
        /// the toolbar, where the device card is. SwiftUI rebuilds the items
        /// when another view's toolbar changes (switching profiles, say) and
        /// drops the space, so it is put back after every change.
        @MainActor
        func keepAtTrailingEdge(_ field: NSView) {
            self.field = field
            guard let toolbar = field.window?.toolbar else { return }
            if observed !== toolbar {
                observed = toolbar
                for name in [NSToolbar.willAddItemNotification, NSToolbar.didRemoveItemNotification] {
                    NotificationCenter.default.addObserver(
                        self, selector: #selector(toolbarChanged), name: name, object: toolbar)
                }
            }
            Self.pushToTrailingEdge(field, in: toolbar)
        }

        @objc private func toolbarChanged() {
            // After the change is complete (and outside the toolbar's own
            // item update).
            DispatchQueue.main.async { [weak self] in
                guard let field = self?.field, let toolbar = field.window?.toolbar else { return }
                Self.pushToTrailingEdge(field, in: toolbar)
            }
        }

        /// Exactly one flexible space, right before the field's item.
        @MainActor
        private static func pushToTrailingEdge(_ field: NSView, in toolbar: NSToolbar) {
            guard let index = toolbar.items.firstIndex(where: { item in
                item.view.map { field.isDescendant(of: $0) } ?? false
            }) else { return }
            if index > 0, toolbar.items[index - 1].itemIdentifier == .flexibleSpace {
                return
            }
            // A stray space left elsewhere by a rebuild would split the room.
            if let stray = toolbar.items.firstIndex(where: { $0.itemIdentifier == .flexibleSpace }) {
                toolbar.removeItem(at: stray)
                pushToTrailingEdge(field, in: toolbar)
                return
            }
            toolbar.insertItem(withItemIdentifier: .flexibleSpace, at: index)
        }

        func controlTextDidChange(_ notification: Notification) {
            guard let field = notification.object as? NSSearchField else { return }
            text.wrappedValue = field.stringValue
        }

        /// Typing and the clear button (×) both end up here.
        @objc func changed(_ field: NSSearchField) {
            text.wrappedValue = field.stringValue
        }
    }
}
