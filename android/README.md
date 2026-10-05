# android/ — Kotlin app (Phase 4+)

Placeholder. The Android port reuses **100% of the scanning engine**: the same
`netscout-core` crate is compiled for `aarch64-unknown-linux-android` (and
`arm64-v8a`) and loaded as a native library; a Kotlin `PlatformNet`/observer
bridge implements the UniFFI `ScanObserver` and calls
`Scanner.set_network_info(…)` to inject the Wi-Fi subnet (Kotlin is the only
component that can read Android's network config).

Only the UI (Jetpack Compose) and this thin bridge are new; no scanning logic
is rewritten. Built out in Phases 4+.
