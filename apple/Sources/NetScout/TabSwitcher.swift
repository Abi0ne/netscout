import SwiftUI

/// The window's two tabs, as pills in the toolbar (⌘1, ⌘2).
struct TabSwitcher: View {
    @Binding var selection: AppTab

    var body: some View {
        HStack(spacing: 2) {
            tab(.scan, "Scansione", symbol: "dot.radiowaves.left.and.right", key: "1")
            tab(.profiles, "Profili salvati", symbol: "archivebox", key: "2")
        }
        .padding(2)
    }

    private func tab(_ tab: AppTab, _ title: String, symbol: String, key: KeyEquivalent) -> some View {
        TabPill(title: title, symbol: symbol, selected: selection == tab) {
            withAnimation(.snappy(duration: 0.2)) { selection = tab }
        }
        .keyboardShortcut(key)
        .help("\(title) (⌘\(String(key.character)))")
    }
}

private struct TabPill: View {
    let title: String
    let symbol: String
    let selected: Bool
    let action: () -> Void
    @ViewState private var hovering = false

    var body: some View {
        Button(action: action) {
            Label(title, systemImage: symbol)
                .labelStyle(.titleAndIcon)
                .font(.callout.weight(selected ? .semibold : .regular))
                .foregroundStyle(selected ? Color.white : hovering ? Color.primary : Color.secondary)
                .padding(.horizontal, 12)
                .padding(.vertical, 5)
                .background {
                    Capsule()
                        .fill(selected ? AnyShapeStyle(Color.accentColor) : AnyShapeStyle(Color.primary.opacity(hovering ? 0.08 : 0)))
                }
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .accessibilityLabel(title)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}
