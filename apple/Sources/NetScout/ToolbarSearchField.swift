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
        DispatchQueue.main.async { Self.pushToTrailingEdge(field) }
        context.coordinator.text = $text
        if field.stringValue != text { field.stringValue = text }
        field.placeholderString = prompt
        field.toolTip = "Cerca: \(prompt)"
    }

    /// SwiftUI lines the items up after the centred title; a flexible space
    /// before the field's item moves it, and what follows, to the right edge
    /// of the toolbar, where the device card is.
    @MainActor
    private static func pushToTrailingEdge(_ field: NSView) {
        guard let toolbar = field.window?.toolbar,
              let index = toolbar.items.firstIndex(where: { item in
                  item.view.map { field.isDescendant(of: $0) } ?? false
              }) else { return }
        if index == 0 || toolbar.items[index - 1].itemIdentifier != .flexibleSpace {
            toolbar.insertItem(withItemIdentifier: .flexibleSpace, at: index)
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator(text: $text) }

    final class Coordinator: NSObject, NSSearchFieldDelegate {
        var text: Binding<String>

        init(text: Binding<String>) { self.text = text }

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
