# apple/ — macOS app (Phase 2/3)

SwiftUI app for macOS 14+. MVVM with `@Observable` view models and Swift
concurrency. Links the Rust engine as an arm64 **XCFramework**
(`libs/NetScoutCore.xcframework`, built by `scripts/build-xcframework.sh`) and
imports the generated bindings (`Generated/netscout_core.swift`, emitted by
`scripts/generate-bindings.sh`).

## Planned structure

```
apple/
├── NetScout.xcodeproj/
├── Generated/            # UniFFI Swift bindings (checked in)
│   └── netscout_core.swift
├── libs/                 # NetScoutCore.xcframework (build output, gitignored)
└── Sources/
    ├── App/              # @main, WindowGroup, DI
    ├── Models/           # @Observable ScanViewModel, HostStore
    ├── ViewModels/
    ├── Views/
    │   ├── NetworkListView.swift
    │   ├── HostDetailView.swift
    │   └── ScanControlsView.swift
    └── Bridge/           # wraps Scanner + ScanObserver for Swift concurrency
```

The `Bridge/` layer is the only place that touches `netscout_core`: it owns a
`Scanner`, implements `ScanObserver` as a `@MainActor`-bound async conformance,
and forwards events to the `@Observable` view model.

Placeholder only — the app is built in Phases 2–3.
