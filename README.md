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
| `apple/` | macOS app. Links `core` as an arm64 (Apple Silicon) XCFramework. |
| `scripts/` | `generate-bindings.sh`, `build-xcframework.sh`, `update-oui.sh` (refreshes `core/data/oui.tsv`, the embedded MAC-vendor table). |
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

## Trying a scan from the terminal

```
cargo run --release --example scan                    # first detected LAN
cargo run --release --example scan -- 192.168.1.0/24  # explicit targets
cargo run --release --example scan -- --deep          # --quick | --standard | --deep
```

## Building the macOS framework

```
scripts/generate-bindings.sh   # emit apple/Generated/netscout_core.swift
scripts/build-xcframework.sh   # arm64 (Apple Silicon) .xcframework
```
