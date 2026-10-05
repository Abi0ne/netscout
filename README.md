# NetScout

A fast, **non-privileged** LAN network scanner. The scanning engine is written
once in Rust (`core/`) and shared verbatim across platforms over UniFFI; each
platform is a thin UI (Swift/SwiftUI for macOS now, Kotlin for Android later).

```
core/     netscout-core — tokio async engine, exposed via UniFFI
apple/    SwiftUI macOS 14+ app (MVVM, @Observable, Swift concurrency)
android/  (placeholder) Kotlin UI reusing the same engine as an AAR
scripts/  bindgen + XCFramework build automation
```

## Layout

| Path | What |
| --- | --- |
| `core/` | Rust library crate `netscout-core`. Owns its own tokio runtime; streams results through the `ScanObserver` callback interface; cancellable; no root. |
| `core/src/platform/` | `trait PlatformNet` + per-OS impls selected by `cfg(target_os)`. |
| `apple/` | macOS app. Links `core` as a universal XCFramework. |
| `scripts/` | `generate-bindings.sh`, `build-xcframework.sh`. |
| `android/` | Placeholder for the later Kotlin port. |

## Public core API (UniFFI surface)

- **`Scanner`** — `new`, `detect_networks`, `set_network_info`, `start_scan`, `cancel`, `scan_host`.
- **`ScanObserver`** — async callback interface: `on_host`, `on_progress`, `on_finished`, `on_error`.
- **`ScanError`** — FFI error enum (thiserror + `uniffi::Error`).
- **Records** — `Host`, `Port`, `ServiceInfo`, `SsdpInfo`, `NetworkInfo`, `ScanConfig`, `Progress`, `Summary`, `DeviceTypeCount`.
- **Enums** — `DeviceType`, `Transport`, `PortState`, `ScanProfile`, `ScanPhase`.

## Building the core (any host, for verification)

```
cargo build
cargo clippy -- -D warnings
cargo test
```

## Building the macOS framework

```
scripts/generate-bindings.sh   # emit apple/Generated/netscout_core.swift
scripts/build-xcframework.sh   # universal x86_64 + arm64 .xcframework
```
