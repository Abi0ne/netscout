# apple/ — macOS app

SwiftUI app for macOS 14+ (Apple Silicon), built with **SwiftPM and the
Command Line Tools only** — no Xcode project. Edit it in any editor (VS Code
with the Swift extension works well).

```
scripts/build-app.sh          # Rust static lib → swift build → NetScout.app
open apple/build/NetScout.app # add `--args --scan` to start scanning at launch
```

## Layout

```
apple/
├── Package.swift
└── Sources/
    ├── netscout_coreFFI/     # UniFFI C header + modulemap (generated, checked in)
    ├── NetScoutCore/         # UniFFI Swift bindings (generated, checked in),
    │                         # linked against target/…/libnetscout_core.a
    └── NetScout/             # the app
        ├── NetScoutApp.swift     # @main, window, ⌘R
        ├── ScanModel.swift       # @Observable state + ObserverBridge
        ├── ContentView.swift     # sidebar | table | detail
        ├── SidebarView.swift     # target, profile, progress, type filter
        ├── HostTableView.swift   # sortable device table
        ├── HostDetailView.swift  # one device, deep re-scan
        ├── DeviceType+UI.swift   # labels and SF Symbols
        └── Compat.swift          # CLT-only build workarounds
```

`ScanModel` is the only place that talks to the engine: it owns the
`Scanner`, and its `ObserverBridge` (the `ScanObserver` implementation)
forwards engine callbacks — which arrive on Rust worker threads — to the main
queue in order.

Regenerate the two generated targets with `scripts/generate-bindings.sh`
whenever the core's public API changes.

## Building without Xcode

* `@State` in the macOS 27 SDK is a macro whose plugin ships only with Xcode;
  views use the underlying property wrapper via `ViewState` (`Compat.swift`).
* The app bundle (Info.plist, including the Local Network usage string) is
  assembled and ad-hoc signed by `scripts/build-app.sh`. On first scan macOS
  asks for permission to access the local network.
