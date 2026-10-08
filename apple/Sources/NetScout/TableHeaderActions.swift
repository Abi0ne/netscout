import AppKit
import SwiftUI

/// Header gestures SwiftUI's `Table` does not offer:
///
/// * **⇧-click** on a column title adds the column to the sort (`addSort`).
///   The table itself ignores the click when ⇧ is down.
/// * **Double-click on a divider** fits the column at its left to its
///   content: the widest value shown (`contentWidth`) or the title,
///   whichever is wider. The other columns keep their widths; the last one
///   takes up the difference.
///
/// SwiftUI keeps its `NSTableView` to itself, so this sits behind the table
/// (`.background`) and watches the window's clicks on the header of the
/// table it covers.
struct TableHeaderActions: NSViewRepresentable {
    let contentWidth: (HostColumn) -> CGFloat
    let addSort: (HostColumn) -> Void

    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        context.coordinator.anchor = view
        context.coordinator.start()
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        context.coordinator.contentWidth = contentWidth
        context.coordinator.addSort = addSort
    }

    static func dismantleNSView(_ nsView: NSView, coordinator: Coordinator) {
        coordinator.stop()
    }

    func makeCoordinator() -> Coordinator { Coordinator(contentWidth: contentWidth, addSort: addSort) }

    final class Coordinator {
        weak var anchor: NSView?
        var contentWidth: (HostColumn) -> CGFloat
        var addSort: (HostColumn) -> Void
        private var monitor: Any?

        /// How far from a divider a click still counts as on it.
        private let slop: CGFloat = 4

        init(contentWidth: @escaping (HostColumn) -> CGFloat, addSort: @escaping (HostColumn) -> Void) {
            self.contentWidth = contentWidth
            self.addSort = addSort
        }

        func start() {
            monitor = NSEvent.addLocalMonitorForEvents(matching: .leftMouseDown) { [weak self] event in
                guard let self else { return event }
                return MainActor.assumeIsolated { self.handle(event) } ? nil : event
            }
        }

        func stop() {
            if let monitor { NSEvent.removeMonitor(monitor) }
            monitor = nil
        }

        /// Act on a click in our table's header; true if it was taken.
        @MainActor
        private func handle(_ event: NSEvent) -> Bool {
            let shift = event.modifierFlags.contains(.shift)
            guard event.clickCount == 2 || (event.clickCount == 1 && shift),
                  let anchor, let window = anchor.window, event.window === window,
                  let hit = window.contentView?.hitTest(event.locationInWindow),
                  let header = Self.headerView(at: hit),
                  let table = header.tableView,
                  Self.covers(table, anchor) else { return false }
            let point = header.convert(event.locationInWindow, from: nil)
            let divider = (0..<table.numberOfColumns).first {
                abs(header.headerRect(ofColumn: $0).maxX - point.x) <= slop
            }
            if event.clickCount == 2, let divider {
                fit(table.tableColumns[divider], in: table)
                return true
            }
            if event.clickCount == 1, shift, divider == nil {
                let index = header.column(at: point)
                guard index >= 0, let column = Self.column(titled: table.tableColumns[index].title) else { return false }
                addSort(column)
                return true
            }
            return false
        }

        @MainActor
        private func fit(_ tableColumn: NSTableColumn, in table: NSTableView) {
            guard let column = Self.column(titled: tableColumn.title) else { return }
            // Cells are inset by the spacing between columns, plus a little air.
            let content = contentWidth(column) + table.intercellSpacing.width + 6
            let bounds = NSRect(x: 0, y: 0, width: 10_000, height: 100)
            let title = tableColumn.headerCell.cellSize(forBounds: bounds).width + 18 // + sort arrow
            let width = min(max(ceil(max(content, title)), tableColumn.minWidth), tableColumn.maxWidth)
            // Only the last column gives or takes the difference.
            let style = table.columnAutoresizingStyle
            table.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
            tableColumn.width = width
            table.sizeLastColumnToFit()
            table.columnAutoresizingStyle = style
        }

        /// The table header containing `view`, if any.
        private static func headerView(at view: NSView) -> NSTableHeaderView? {
            var v: NSView? = view
            while let current = v {
                if let header = current as? NSTableHeaderView { return header }
                v = current.superview
            }
            return nil
        }

        /// Whether `table` is the one this view sits behind (same place in
        /// the window): each tab's table has its own.
        fileprivate static func covers(_ table: NSTableView, _ anchor: NSView) -> Bool {
            guard let scroll = table.enclosingScrollView else { return false }
            let a = anchor.convert(anchor.bounds, to: nil)
            let t = scroll.convert(scroll.bounds, to: nil)
            return a.insetBy(dx: -2, dy: -2).contains(t.insetBy(dx: 2, dy: 2))
        }

        /// The column of a header title, which may end with its sort rank ("Nome ²▼").
        private static func column(titled title: String) -> HostColumn? {
            HostColumn.allCases.first { title == $0.title || title.hasPrefix($0.title + " ") }
        }
    }
}

/// Reports the row under the mouse (its index, or nil), anywhere across it:
/// SwiftUI's `Table` has no row hover. Like `TableHeaderActions`, it sits
/// behind the table and watches the window's mouse moves over it.
struct TableRowHover: NSViewRepresentable {
    let onHover: (Int?) -> Void

    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        context.coordinator.anchor = view
        context.coordinator.start()
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        context.coordinator.onHover = onHover
        // Windows get no mouse moves unless asked.
        DispatchQueue.main.async { nsView.window?.acceptsMouseMovedEvents = true }
    }

    static func dismantleNSView(_ nsView: NSView, coordinator: Coordinator) {
        coordinator.stop()
    }

    func makeCoordinator() -> Coordinator { Coordinator(onHover: onHover) }

    final class Coordinator {
        weak var anchor: NSView?
        var onHover: (Int?) -> Void
        private var monitor: Any?
        private var current: Int?

        init(onHover: @escaping (Int?) -> Void) { self.onHover = onHover }

        func start() {
            monitor = NSEvent.addLocalMonitorForEvents(matching: [.mouseMoved, .scrollWheel, .mouseExited]) { [weak self] event in
                guard let self else { return event }
                MainActor.assumeIsolated { self.track(event) }
                return event
            }
        }

        func stop() {
            if let monitor { NSEvent.removeMonitor(monitor) }
            monitor = nil
        }

        @MainActor
        private func track(_ event: NSEvent) {
            guard let anchor, let window = anchor.window else { return }
            var row: Int?
            if event.window === window, event.type != .mouseExited,
               let table = Self.table(at: window.contentView?.hitTest(event.locationInWindow)),
               TableHeaderActions.Coordinator.covers(table, anchor) {
                let index = table.row(at: table.convert(event.locationInWindow, from: nil))
                row = index >= 0 ? index : nil
            }
            guard row != current else { return }
            current = row
            onHover(row)
        }

        /// The table containing `view`, if any.
        private static func table(at view: NSView?) -> NSTableView? {
            var v = view
            while let current = v {
                if let table = current as? NSTableView { return table }
                v = current.superview
            }
            return nil
        }
    }
}
